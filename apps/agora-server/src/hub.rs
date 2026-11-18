//! Live connections and event fan-out.
//!
//! Each WebSocket registers here with the user and session it belongs to. An event is
//! serialised once and queued to every connection of every recipient; the server decides the
//! recipients from the database (who may read the channel *now*), so a removed member stops
//! receiving events at once. A connection whose queue is full is dropped rather than allowed to
//! grow without bound; its client reconnects and catches up from history.
//!
//! Calls live here too: who is in each channel's call, and a second, smaller queue per
//! connection for their audio and video. Media is lossy by nature, so a full media queue drops
//! the frame instead of the connection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use agora_core::{ChannelId, UserId};
use agora_protocol::{CallParticipant, ServerEvent, Timestamp};
use tokio::sync::mpsc;

/// Events queued per connection before it is considered too slow and dropped.
const QUEUE: usize = 512;
/// Media frames queued per connection; beyond that, frames are dropped.
const MEDIA_QUEUE: usize = 64;

#[derive(Clone, Debug)]
pub enum Outgoing {
    Event(Arc<str>),
    /// Close the connection (logout, session expired).
    Close,
    /// The server is stopping: close, and let the client reconnect.
    Shutdown,
}

#[derive(Debug)]
struct Conn {
    id: u64,
    session_id: i64,
    tx: mpsc::Sender<Outgoing>,
    media: mpsc::Sender<Arc<[u8]>>,
}

/// One connection in a call.
#[derive(Debug)]
struct Seat {
    user: UserId,
    conn: u64,
    audio: bool,
    video: bool,
    joined_at: Timestamp,
    tx: mpsc::Sender<Outgoing>,
    media: mpsc::Sender<Arc<[u8]>>,
}

impl Seat {
    fn participant(&self) -> CallParticipant {
        CallParticipant {
            user_id: self.user,
            audio: self.audio,
            video: self.video,
            joined_at: self.joined_at,
        }
    }
}

#[derive(Debug, Default)]
pub struct Hub {
    next: AtomicU64,
    conns: Mutex<HashMap<UserId, Vec<Conn>>>,
    calls: Mutex<HashMap<ChannelId, Vec<Seat>>>,
}

/// What joining a call changed: the calls whose participants are now different.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CallChanges {
    pub changed: Vec<(ChannelId, Vec<CallParticipant>)>,
}

/// A call refused a new participant.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinError {
    Full,
    /// The connection is not registered (it is closing).
    Gone,
}

/// What registering a connection did.
pub struct Registered {
    pub id: u64,
    pub rx: mpsc::Receiver<Outgoing>,
    /// Audio and video from the other people in this connection's call.
    pub media: mpsc::Receiver<Arc<[u8]>>,
    /// This is the user's first live connection: they just came online.
    pub came_online: bool,
}

impl Hub {
    /// Register a connection, unless the user already has `max` of them.
    pub fn register(&self, user: UserId, session_id: i64, max: usize) -> Option<Registered> {
        let mut conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        let list = conns.entry(user).or_default();
        list.retain(|c| !c.tx.is_closed());
        if list.len() >= max {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(QUEUE);
        let (media_tx, media) = mpsc::channel(MEDIA_QUEUE);
        let came_online = list.is_empty();
        list.push(Conn {
            id,
            session_id,
            tx,
            media: media_tx,
        });
        Some(Registered {
            id,
            rx,
            media,
            came_online,
        })
    }

    /// Remove a connection. Returns true when it was the user's last one.
    pub fn unregister(&self, user: UserId, id: u64) -> bool {
        let mut conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(list) = conns.get_mut(&user) else {
            return false;
        };
        list.retain(|c| c.id != id);
        if list.is_empty() {
            conns.remove(&user);
            return true;
        }
        false
    }

    pub fn is_online(&self, user: UserId) -> bool {
        let conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        conns.get(&user).is_some_and(|l| !l.is_empty())
    }

    /// Queue `event` to every connection of each user in `to`.
    pub fn send(&self, to: &[UserId], event: &ServerEvent) {
        let Ok(json) = serde_json::to_string(event) else {
            return;
        };
        let json: Arc<str> = json.into();
        let conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        for user in to {
            for c in conns.get(user).into_iter().flatten() {
                // A full queue means a stalled client: dropping the message would leave a hole,
                // so close it instead and let it resynchronise.
                if c.tx.try_send(Outgoing::Event(json.clone())).is_err() {
                    let _ = c.tx.try_send(Outgoing::Close);
                }
            }
        }
    }

    /// Close every connection opened with a session (on logout).
    pub fn close_session(&self, session_id: i64) {
        let conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        for c in conns
            .values()
            .flatten()
            .filter(|c| c.session_id == session_id)
        {
            let _ = c.tx.try_send(Outgoing::Close);
        }
    }

    /// Close every connection (server shutdown). Clients reconnect to the next process.
    pub fn close_all(&self) {
        let conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        for c in conns.values().flatten() {
            let _ = c.tx.try_send(Outgoing::Shutdown);
        }
    }

    pub fn connection_count(&self) -> usize {
        let conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        conns.values().map(Vec::len).sum()
    }

    /// Put connection `conn` of `user` in `channel`'s call, unless it already holds `max`
    /// people. The connection leaves any other call, and the person's other devices leave this
    /// one (each is told with [`ServerEvent::CallLeft`]).
    pub fn join_call(
        &self,
        channel: ChannelId,
        user: UserId,
        conn: u64,
        media: (bool, bool),
        max: usize,
        now: Timestamp,
    ) -> Result<CallChanges, JoinError> {
        let (tx, media_tx) = {
            let conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
            let c = conns
                .get(&user)
                .and_then(|l| l.iter().find(|c| c.id == conn))
                .ok_or(JoinError::Gone)?;
            (c.tx.clone(), c.media.clone())
        };
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changes = CallChanges::default();
        if let Some(seats) = calls.get_mut(&channel) {
            if let Some(seat) = seats.iter_mut().find(|s| s.conn == conn) {
                seat.audio = media.0;
                seat.video = media.1;
                changes.changed.push((channel, list(seats)));
                return Ok(changes);
            }
            let others = seats.iter().filter(|s| s.user != user).count();
            if others >= max {
                return Err(JoinError::Full);
            }
        }
        // Leave whatever call this connection was in.
        for (other, seats) in calls.iter_mut() {
            if *other != channel && seats.iter().any(|s| s.conn == conn) {
                seats.retain(|s| s.conn != conn);
                changes.changed.push((*other, list(seats)));
            }
        }
        let seats = calls.entry(channel).or_default();
        let mut joined_at = now;
        for moved in seats.iter().filter(|s| s.user == user) {
            joined_at = joined_at.min(moved.joined_at);
            let _ = moved
                .tx
                .try_send(Outgoing::Event(event_json(&ServerEvent::CallLeft {
                    channel_id: channel,
                    reason: agora_protocol::CallLeftReason::OtherDevice,
                })));
        }
        seats.retain(|s| s.user != user);
        seats.push(Seat {
            user,
            conn,
            audio: media.0,
            video: media.1,
            joined_at,
            tx,
            media: media_tx,
        });
        changes.changed.push((channel, list(seats)));
        calls.retain(|_, seats| !seats.is_empty());
        Ok(changes)
    }

    /// Change what a connection sends in a call. `None` when it is not in it.
    pub fn update_call(
        &self,
        channel: ChannelId,
        conn: u64,
        audio: bool,
        video: bool,
    ) -> Option<Vec<CallParticipant>> {
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let seats = calls.get_mut(&channel)?;
        let seat = seats.iter_mut().find(|s| s.conn == conn)?;
        if seat.audio == audio && seat.video == video {
            return None;
        }
        seat.audio = audio;
        seat.video = video;
        Some(list(seats))
    }

    /// Take a connection out of `channel`'s call, or out of every call with `None`.
    pub fn leave_call(&self, channel: Option<ChannelId>, conn: u64) -> CallChanges {
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changes = CallChanges::default();
        for (id, seats) in calls.iter_mut() {
            if channel.is_some_and(|c| c != *id) {
                continue;
            }
            let before = seats.len();
            seats.retain(|s| s.conn != conn);
            if seats.len() != before {
                changes.changed.push((*id, list(seats)));
            }
        }
        calls.retain(|_, seats| !seats.is_empty());
        changes
    }

    /// The call a connection is in.
    pub fn call_of(&self, conn: u64) -> Option<ChannelId> {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        calls
            .iter()
            .find(|(_, seats)| seats.iter().any(|s| s.conn == conn))
            .map(|(id, _)| *id)
    }

    /// Every call going on, with who is in it.
    pub fn calls(&self) -> Vec<(ChannelId, Vec<CallParticipant>)> {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        calls.iter().map(|(id, seats)| (*id, list(seats))).collect()
    }

    /// Queue `frame` to everyone else in `channel`'s call. False when `conn` is not in it.
    pub fn relay(&self, channel: ChannelId, conn: u64, frame: Arc<[u8]>) -> bool {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(seats) = calls.get(&channel) else {
            return false;
        };
        if !seats.iter().any(|s| s.conn == conn) {
            return false;
        }
        for seat in seats.iter().filter(|s| s.conn != conn) {
            // A slow receiver loses frames, never its connection.
            let _ = seat.media.try_send(frame.clone());
        }
        true
    }
}

fn list(seats: &[Seat]) -> Vec<CallParticipant> {
    seats.iter().map(Seat::participant).collect()
}

fn event_json(event: &ServerEvent) -> Arc<str> {
    serde_json::to_string(event).unwrap_or_default().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn events_reach_only_their_recipients() {
        let hub = Hub::default();
        let mut a = hub.register(UserId(1), 10, 4).unwrap();
        let mut b = hub.register(UserId(2), 20, 4).unwrap();
        hub.send(&[UserId(1)], &ServerEvent::Pong);
        assert!(matches!(a.rx.recv().await, Some(Outgoing::Event(_))));
        assert!(b.rx.try_recv().is_err());
    }

    #[test]
    fn connections_per_user_are_capped_and_presence_tracked() {
        let hub = Hub::default();
        let first = hub.register(UserId(1), 1, 2).unwrap();
        assert!(first.came_online);
        let second = hub.register(UserId(1), 1, 2).unwrap();
        assert!(!second.came_online);
        assert!(hub.register(UserId(1), 1, 2).is_none());
        assert!(!hub.unregister(UserId(1), first.id));
        assert!(hub.unregister(UserId(1), second.id));
        assert!(!hub.is_online(UserId(1)));
    }

    #[tokio::test]
    async fn calls_relay_to_the_others_and_move_between_devices() {
        let hub = Hub::default();
        let ch = ChannelId(5);
        let mut ana = hub.register(UserId(1), 10, 4).unwrap();
        let mut luis = hub.register(UserId(2), 20, 4).unwrap();
        let mut eva = hub.register(UserId(3), 30, 4).unwrap();
        let mut ana_phone = hub.register(UserId(1), 11, 4).unwrap();

        // Someone not in the call cannot send into it.
        assert!(!hub.relay(ch, ana.id, Arc::from(&b"x"[..])));
        hub.join_call(ch, UserId(1), ana.id, (true, false), 2, 100)
            .unwrap();
        hub.join_call(ch, UserId(2), luis.id, (true, true), 2, 200)
            .unwrap();
        // Two others already: full.
        assert_eq!(
            hub.join_call(ch, UserId(3), eva.id, (true, true), 2, 300),
            Err(JoinError::Full)
        );
        assert!(hub.relay(ch, ana.id, Arc::from(&b"hola"[..])));
        assert_eq!(&*luis.media.recv().await.unwrap(), b"hola");
        assert!(ana.media.try_recv().is_err());
        assert!(eva.media.try_recv().is_err());

        // The same person from another device takes the seat and keeps when they joined.
        let changes = hub
            .join_call(ch, UserId(1), ana_phone.id, (false, false), 2, 400)
            .unwrap();
        let (_, people) = changes.changed.last().unwrap();
        assert_eq!(people.len(), 2);
        assert!(people
            .iter()
            .any(|p| p.user_id == UserId(1) && p.joined_at == 100));
        assert!(matches!(ana.rx.recv().await, Some(Outgoing::Event(e)) if e.contains("call_left")));
        assert!(!hub.relay(ch, ana.id, Arc::from(&b"x"[..])));
        assert_eq!(hub.call_of(ana_phone.id), Some(ch));

        // A slow receiver loses frames, not its connection.
        for _ in 0..(MEDIA_QUEUE + 10) {
            assert!(hub.relay(ch, luis.id, Arc::from(&b"v"[..])));
        }
        assert!(ana_phone.rx.try_recv().is_err());

        hub.leave_call(None, luis.id);
        hub.leave_call(Some(ch), ana_phone.id);
        assert!(hub.calls().is_empty());
        let _ = eva.rx.try_recv();
    }

    #[tokio::test]
    async fn logout_closes_that_session_only() {
        let hub = Hub::default();
        let mut a = hub.register(UserId(1), 10, 4).unwrap();
        let mut b = hub.register(UserId(1), 11, 4).unwrap();
        hub.close_session(10);
        assert!(matches!(a.rx.recv().await, Some(Outgoing::Close)));
        assert!(b.rx.try_recv().is_err());
    }
}
