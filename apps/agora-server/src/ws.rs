//! The real-time channel: one WebSocket per client at `/api/v1/ws`.
//!
//! The client authenticates with its first frame (browsers cannot set headers on a WebSocket,
//! so a header would not work for the future web client). Until then nothing is sent to it; a
//! wrong token gets one `error` event and the socket closes. After that the server pushes
//! [`ServerEvent`]s and accepts typing notices and pings. The session is checked again every
//! minute, and a logout closes its sockets at once.
//!
//! Calls ride on the same socket: `call_join`, `call_update` and `call_leave` frames say who is
//! in a channel's call, and binary frames carry each person's end-to-end encrypted audio and
//! video, which the server passes to the others in the call without reading
//! ([`agora_protocol::media`]).

use std::sync::Arc;
use std::time::Duration;

use agora_core::{roles, ChannelId};
use agora_protocol::{
    media, CallLeftReason, CallParticipant, ClientFrame, ErrorCode, ServerEvent,
    WS_AUTH_TIMEOUT_SECS,
};
use axum::extract::ws::{CloseFrame, Message as WsMessage, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::time::{interval, timeout, Instant};

use crate::auth::{now_ms, session_for_token, Auth};
use crate::hub::{CallChanges, JoinError, Outgoing};
use crate::state::AppState;

/// Largest text frame a client may send. Binary frames (call media) may reach
/// [`media::MAX_FRAME`].
const MAX_TEXT_FRAME: usize = 16 * 1024;
const PING_EVERY: Duration = Duration::from_secs(30);
const RECHECK_SESSION_EVERY: Duration = Duration::from_secs(60);
/// Typing notices relayed per user per minute.
const TYPING_PER_MINUTE: u32 = 30;

pub async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(media::MAX_FRAME)
        .max_frame_size(media::MAX_FRAME)
        .on_upgrade(move |socket| handle(state, socket))
}

fn json(event: &ServerEvent) -> WsMessage {
    WsMessage::Text(Utf8Bytes::from(
        serde_json::to_string(event).unwrap_or_default(),
    ))
}

async fn reject(mut socket: WebSocket, code: ErrorCode, message: &str) {
    let _ = socket
        .send(json(&ServerEvent::Error {
            code,
            message: message.to_owned(),
        }))
        .await;
    // 4001 tells the client not to retry with this session; 1011 says try again later.
    let close = if code == ErrorCode::Unauthenticated {
        CloseFrame {
            code: 4001,
            reason: Utf8Bytes::from_static("unauthenticated"),
        }
    } else {
        CloseFrame {
            code: 1011,
            reason: Utf8Bytes::from_static("try again later"),
        }
    };
    let _ = socket.send(WsMessage::Close(Some(close))).await;
}

/// `Ok(None)`: no valid session. `Err`: the server could not check (the client should retry).
async fn authenticate(state: &AppState, socket: &mut WebSocket) -> Result<Option<Auth>, ()> {
    let first = match timeout(Duration::from_secs(WS_AUTH_TIMEOUT_SECS), socket.next()).await {
        Ok(Some(Ok(frame))) => frame,
        _ => return Ok(None),
    };
    let WsMessage::Text(text) = first else {
        return Ok(None);
    };
    let Ok(ClientFrame::Auth { token }) = serde_json::from_str::<ClientFrame>(&text) else {
        return Ok(None);
    };
    session_for_token(state, &token).await.map_err(|_| ())
}

async fn handle(state: AppState, mut socket: WebSocket) {
    let auth = match authenticate(&state, &mut socket).await {
        Ok(Some(auth)) => auth,
        Ok(None) => {
            reject(
                socket,
                ErrorCode::Unauthenticated,
                "invalid or expired session",
            )
            .await;
            return;
        }
        Err(()) => {
            reject(
                socket,
                ErrorCode::Internal,
                "the server cannot check sessions right now",
            )
            .await;
            return;
        }
    };
    let user = auth.user.clone();
    let Some(registered) = state.hub.register(
        user.id,
        auth.session_id,
        state.config.limits.connections_per_user,
    ) else {
        reject(
            socket,
            ErrorCode::RateLimited,
            "too many connections for this account",
        )
        .await;
        return;
    };
    let conn = registered.id;
    let mut rx = registered.rx;
    let mut media_rx = registered.media;
    tracing::debug!(user = %user.id, conn, "websocket connected");

    if registered.came_online {
        announce_presence(&state, &auth, true).await;
    }
    let (mut sink, mut stream) = socket.split();
    if sink
        .send(json(&ServerEvent::Ready { user: user.clone() }))
        .await
        .is_err()
    {
        finish(&state, &auth, conn).await;
        return;
    }
    for event in calls_going_on(&state, &auth).await {
        if sink.send(json(&event)).await.is_err() {
            finish(&state, &auth, conn).await;
            return;
        }
    }

    let mut budget = Budget::new(state.config.limits.call_bytes_per_second);
    let mut ping = interval(PING_EVERY);
    let mut recheck = interval(RECHECK_SESSION_EVERY);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(Outgoing::Event(text)) => {
                    if sink.send(WsMessage::Text(Utf8Bytes::from(text.as_ref()))).await.is_err() { break; }
                }
                Some(Outgoing::Close) | None => {
                    let _ = sink.send(WsMessage::Close(Some(CloseFrame { code: 4001, reason: Utf8Bytes::from_static("session ended") }))).await;
                    break;
                }
                Some(Outgoing::Shutdown) => {
                    let _ = sink.send(WsMessage::Close(Some(CloseFrame { code: 1012, reason: Utf8Bytes::from_static("server restarting") }))).await;
                    break;
                }
            },
            frame = media_rx.recv() => {
                if let Some(frame) = frame {
                    if sink.send(WsMessage::Binary(frame.to_vec().into())).await.is_err() { break; }
                }
            },
            incoming = stream.next() => match incoming {
                Some(Ok(WsMessage::Binary(bytes))) => {
                    last_seen = Instant::now();
                    relay_media(&state, &auth, conn, &mut budget, &bytes);
                }
                Some(Ok(WsMessage::Text(text))) if text.len() <= MAX_TEXT_FRAME => {
                    last_seen = Instant::now();
                    match serde_json::from_str::<ClientFrame>(&text) {
                        Ok(ClientFrame::Ping) => {
                            if sink.send(json(&ServerEvent::Pong)).await.is_err() { break; }
                        }
                        Ok(ClientFrame::Typing { channel_id, thread_root_id }) => {
                            relay_typing(&state, &auth, channel_id, thread_root_id).await;
                        }
                        Ok(ClientFrame::CallJoin { channel_id, audio, video }) => {
                            if let Some(refusal) = join_call(&state, &auth, conn, channel_id, (audio, video)).await {
                                if sink.send(json(&refusal)).await.is_err() { break; }
                            }
                        }
                        Ok(ClientFrame::CallUpdate { channel_id, audio, video }) => {
                            if let Some(people) = state.hub.update_call(channel_id, conn, audio, video) {
                                announce_call(&state, channel_id, people).await;
                            }
                        }
                        Ok(ClientFrame::CallLeave { channel_id }) => {
                            let changes = state.hub.leave_call(Some(channel_id), conn);
                            announce_calls(&state, changes).await;
                        }
                        // Auth again or unknown frames are ignored, never fatal.
                        Ok(ClientFrame::Auth { .. }) | Err(_) => {}
                    }
                }
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => { last_seen = Instant::now(); }
            },
            _ = ping.tick() => {
                if last_seen.elapsed() > PING_EVERY * 3 { break; }
                if sink.send(WsMessage::Ping(Default::default())).await.is_err() { break; }
            }
            _ = recheck.tick() => {
                match state.store.session(&auth.token_hash, now_ms()).await {
                    Ok(Some(_)) => {
                        if let Some(left) = recheck_call(&state, &auth, conn).await {
                            if sink.send(json(&left)).await.is_err() { break; }
                        }
                    }
                    Ok(None) => {
                        let _ = sink.send(json(&ServerEvent::Error { code: ErrorCode::Unauthenticated, message: "session ended".into() })).await;
                        break;
                    }
                    // A database hiccup is not a logout: drop the socket and let the client retry.
                    Err(_) => break,
                }
            }
        }
    }
    finish(&state, &auth, conn).await;
}

async fn finish(state: &AppState, auth: &Auth, conn: u64) {
    let changes = state.hub.leave_call(None, conn);
    announce_calls(state, changes).await;
    if state.hub.unregister(auth.user.id, conn) {
        announce_presence(state, auth, false).await;
    }
    tracing::debug!(user = %auth.user.id, conn, "websocket closed");
}

async fn announce_presence(state: &AppState, auth: &Auth, online: bool) {
    if let Ok(colleagues) = state.store.colleagues(auth.user.id).await {
        state.hub.send(
            &colleagues,
            &ServerEvent::Presence {
                user_id: auth.user.id,
                online,
            },
        );
    }
}

async fn relay_typing(
    state: &AppState,
    auth: &Auth,
    channel: agora_core::ChannelId,
    thread_root_id: Option<agora_core::MessageId>,
) {
    if !state
        .limiter
        .allow(&format!("typing:{}", auth.user.id), TYPING_PER_MINUTE)
    {
        return;
    }
    let Ok(access) = crate::routes::channel_access(state, channel, auth.user.id).await else {
        return;
    };
    if !roles::can_post(
        access.role,
        access.channel.kind,
        access.member,
        access.channel.archived,
    ) {
        return;
    }
    let Ok(mut audience) = state.store.channel_audience(&access.channel).await else {
        return;
    };
    audience.retain(|u| *u != auth.user.id);
    state.hub.send(
        &audience,
        &ServerEvent::Typing {
            channel_id: channel,
            thread_root_id,
            user: auth.user.clone(),
        },
    );
}

/// Whether `auth` may be in `channel`'s call: a member who can post there. Only the members'
/// devices hold the conversation's keys, so nobody else could follow the call anyway.
async fn may_call(state: &AppState, auth: &Auth, channel: ChannelId) -> bool {
    crate::routes::channel_access(state, channel, auth.user.id)
        .await
        .is_ok_and(|a| a.member && a.can_post())
}

/// Join a call. Returns the event to send back when it was refused.
async fn join_call(
    state: &AppState,
    auth: &Auth,
    conn: u64,
    channel: ChannelId,
    media: (bool, bool),
) -> Option<ServerEvent> {
    if !may_call(state, auth, channel).await {
        return Some(ServerEvent::CallLeft {
            channel_id: channel,
            reason: CallLeftReason::NoAccess,
        });
    }
    let max = state.config.limits.call_participants.max(1);
    match state
        .hub
        .join_call(channel, auth.user.id, conn, media, max, now_ms())
    {
        Ok(changes) => {
            announce_calls(state, changes).await;
            None
        }
        Err(JoinError::Full) => Some(ServerEvent::CallLeft {
            channel_id: channel,
            reason: CallLeftReason::Full,
        }),
        Err(JoinError::Gone) => None,
    }
}

/// Take the connection out of its call if the person can no longer take part in the channel.
async fn recheck_call(state: &AppState, auth: &Auth, conn: u64) -> Option<ServerEvent> {
    let channel = state.hub.call_of(conn)?;
    if may_call(state, auth, channel).await {
        return None;
    }
    let changes = state.hub.leave_call(Some(channel), conn);
    announce_calls(state, changes).await;
    Some(ServerEvent::CallLeft {
        channel_id: channel,
        reason: CallLeftReason::NoAccess,
    })
}

async fn announce_calls(state: &AppState, changes: CallChanges) {
    for (channel, people) in changes.changed {
        announce_call(state, channel, people).await;
    }
}

/// Tell everyone who can read `channel` who is in its call now.
async fn announce_call(state: &AppState, channel: ChannelId, participants: Vec<CallParticipant>) {
    let Ok(Some(row)) = state.store.channel(channel).await else {
        return;
    };
    let Ok(audience) = state.store.channel_audience(&row).await else {
        return;
    };
    state.hub.send(
        &audience,
        &ServerEvent::CallChanged {
            channel_id: channel,
            participants,
        },
    );
}

/// The calls going on in channels `auth` can read, for a connection that just opened.
async fn calls_going_on(state: &AppState, auth: &Auth) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    for (channel, participants) in state.hub.calls() {
        if crate::routes::channel_access(state, channel, auth.user.id)
            .await
            .is_ok()
        {
            events.push(ServerEvent::CallChanged {
                channel_id: channel,
                participants,
            });
        }
    }
    events
}

/// Pass a media frame to the others in the call, within the connection's byte budget.
/// Anything wrong (not in that call, malformed, over budget) drops the frame silently: media is
/// lossy, and the sender learns nothing it could not see from `call_changed` already.
fn relay_media(state: &AppState, auth: &Auth, conn: u64, budget: &mut Budget, bytes: &[u8]) {
    let Some(up) = media::Up::parse(bytes) else {
        return;
    };
    if !budget.spend(bytes.len()) {
        return;
    }
    let frame: Arc<[u8]> = up.relay(auth.user.id).into();
    state.hub.relay(up.channel, conn, frame);
}

/// A token bucket of bytes per second, holding at most two seconds' worth.
struct Budget {
    per_second: f64,
    tokens: f64,
    last: Instant,
}

impl Budget {
    fn new(per_second: usize) -> Budget {
        let per_second = per_second as f64;
        Budget {
            per_second,
            tokens: per_second,
            last: Instant::now(),
        }
    }

    fn spend(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        let refill = now.duration_since(self.last).as_secs_f64() * self.per_second;
        self.tokens = (self.tokens + refill).min(self.per_second * 2.0);
        self.last = now;
        let cost = bytes as f64;
        if self.tokens < cost {
            return false;
        }
        self.tokens -= cost;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_budget_refuses_bursts_beyond_it() {
        let mut budget = Budget::new(1000);
        assert!(budget.spend(900));
        assert!(!budget.spend(900));
        assert!(budget.spend(50));
    }
}
