use agora_core::{limits, roles, validate, ChannelId, MessageId, UserId, WorkspaceId};
use agora_e2e::Kind;
use agora_protocol::{
    EditMessageRequest, LookupMessagesRequest, Message, MessagePage, PostMessageRequest,
    ReactRequest, SearchResults, ServerEvent, Thread,
};
use agora_storage::messages::{Inserted, NewMessage};
use agora_storage::ChannelRow;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{channel_access, check_write, workspace_role};
use crate::auth::{now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Replies returned for one thread.
const THREAD_MAX: u32 = 1_000;
/// Different emoji one message can carry.
const REACTION_KINDS_MAX: i64 = 24;

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    before: Option<i64>,
    after: Option<i64>,
    limit: Option<u32>,
}

pub async fn history(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<MessagePage>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    if q.before.is_some() && q.after.is_some() {
        return Err(AppError::invalid("use before or after, not both"));
    }
    let limit = q
        .limit
        .unwrap_or(limits::HISTORY_PAGE_DEFAULT)
        .clamp(1, limits::HISTORY_PAGE_MAX);
    let (messages, has_more) = state
        .store
        .history(
            access.channel.id,
            q.before.map(MessageId),
            q.after.map(MessageId),
            limit,
        )
        .await?;
    Ok(Json(MessagePage { messages, has_more }))
}

/// Largest encrypted message: the longest body, in the widest UTF-8, plus MLS framing.
const CIPHERTEXT_MAX: usize = limits::MESSAGE_MAX_CHARS * 4 + 4_096;
/// People one message can mention.
const MENTIONS_MAX: usize = 50;

/// The people a message mentions who can read the channel. The client names them, since the
/// server cannot read the text.
fn mentioned(listed: &[UserId], audience: &[UserId]) -> AppResult<Vec<UserId>> {
    if listed.len() > MENTIONS_MAX {
        return Err(AppError::invalid("too many mentions"));
    }
    let mut ids: Vec<UserId> = listed
        .iter()
        .copied()
        .filter(|id| audience.contains(id))
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The encrypted text of a new or edited message: the MLS application message and its epoch.
fn ciphertext(
    channel: &ChannelRow,
    body: &str,
    ciphertext: Option<&str>,
) -> AppResult<(Vec<u8>, u64)> {
    if !body.is_empty() {
        return Err(AppError::invalid(
            "messages are end-to-end encrypted: send the text as ciphertext, never as body",
        ));
    }
    let Some(ciphertext) = ciphertext else {
        return Err(AppError::invalid("ciphertext is required"));
    };
    if ciphertext.len() > CIPHERTEXT_MAX {
        return Err(AppError::invalid("the message is too long"));
    }
    let data = super::e2e::decode("ciphertext", ciphertext)?;
    let epoch = super::e2e::check_mls(channel.id, &data, Kind::Application)?;
    Ok((data, epoch))
}

pub async fn post(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<PostMessageRequest>,
) -> AppResult<(StatusCode, Json<Message>)> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    if !access.can_post() {
        return Err(AppError::forbidden(if access.channel.archived {
            "this channel is archived"
        } else {
            "you cannot post here"
        }));
    }
    check_write(&state, auth.user.id)?;
    let (data, epoch) = ciphertext(&access.channel, &req.body, req.ciphertext.as_deref())?;
    let nonce = validate::nonce(&req.client_nonce)?;
    if let Some(root) = req.thread_root_id {
        let (root_channel, _, root_parent, root_deleted) = state
            .store
            .message_meta(root)
            .await?
            .ok_or_else(AppError::not_found)?;
        if root_channel != access.channel.id || root_deleted {
            return Err(AppError::not_found());
        }
        if root_parent.is_some() {
            return Err(AppError::invalid(
                "reply to the thread's first message, not to a reply",
            ));
        }
    }
    // Writing in a public channel joins it, as it would in the sidebar.
    if !access.member {
        state
            .store
            .join_channel(access.channel.id, auth.user.id, now_ms())
            .await?;
    }
    let audience = state.store.channel_audience(&access.channel).await?;
    let mentions = mentioned(&req.mentions, &audience)?;
    let inserted = state
        .store
        .insert_message(NewMessage {
            channel: access.channel.id,
            author: auth.user.id,
            thread_root: req.thread_root_id,
            body: "",
            nonce: &nonce,
            mentioned: &mentions,
            ciphertext: Some((&data, epoch)),
            now: now_ms(),
        })
        .await?;
    match inserted {
        Inserted::Created(message) => {
            state.hub.send(
                &audience,
                &ServerEvent::MessageCreated {
                    message: message.clone(),
                },
            );
            if let Some(root) = message.thread_root_id {
                if let Some(root) = state.store.message(root).await? {
                    state
                        .hub
                        .send(&audience, &ServerEvent::MessageUpdated { message: root });
                }
            }
            Ok((StatusCode::CREATED, Json(message)))
        }
        Inserted::Existing(message) => {
            // A retry: the same nonce must mean the same message, in the same place.
            if message.channel_id != access.channel.id
                || message.thread_root_id != req.thread_root_id
            {
                return Err(AppError::conflict(
                    "client_nonce was already used for another message",
                ));
            }
            Ok((StatusCode::OK, Json(message)))
        }
    }
}

/// Load a message for an action by the caller, checking they can read its channel.
async fn message_access(
    state: &AppState,
    id: MessageId,
    user: UserId,
) -> AppResult<(super::ChannelAccess, UserId, Option<MessageId>, bool)> {
    let (channel, author, root, deleted) = state
        .store
        .message_meta(id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let access = channel_access(state, channel, user).await?;
    Ok((access, author, root, deleted))
}

pub async fn thread(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Thread>> {
    let (_, _, parent, _) = message_access(&state, MessageId(id), auth.user.id).await?;
    let root_id = parent.unwrap_or(MessageId(id));
    let root = state
        .store
        .message(root_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let replies = state.store.replies(root_id, THREAD_MAX).await?;
    Ok(Json(Thread { root, replies }))
}

pub async fn edit(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<EditMessageRequest>,
) -> AppResult<Json<Message>> {
    let id = MessageId(id);
    let (access, author, _, deleted) = message_access(&state, id, auth.user.id).await?;
    if !roles::can_edit_message(auth.user.id, author, deleted) || access.channel.archived {
        return Err(AppError::forbidden("you can only edit your own messages"));
    }
    check_write(&state, auth.user.id)?;
    let (data, epoch) = ciphertext(&access.channel, &req.body, req.ciphertext.as_deref())?;
    let audience = state.store.channel_audience(&access.channel).await?;
    let mentions = mentioned(&req.mentions, &audience)?;
    state
        .store
        .edit_message(
            id,
            "",
            Some((&data, epoch)),
            auth.user.id,
            &mentions,
            now_ms(),
        )
        .await?;
    let message = state
        .store
        .message(id)
        .await?
        .ok_or_else(AppError::internal)?;
    state.hub.send(
        &audience,
        &ServerEvent::MessageUpdated {
            message: message.clone(),
        },
    );
    Ok(Json(message))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let id = MessageId(id);
    let (access, author, root, deleted) = message_access(&state, id, auth.user.id).await?;
    if deleted {
        return Ok(StatusCode::NO_CONTENT);
    }
    if !roles::can_delete_message(auth.user.id, author, access.role) {
        return Err(AppError::forbidden("you can only delete your own messages"));
    }
    check_write(&state, auth.user.id)?;
    state.store.delete_message(id, now_ms()).await?;
    let audience = state.store.channel_audience(&access.channel).await?;
    state.hub.send(
        &audience,
        &ServerEvent::MessageDeleted {
            channel_id: access.channel.id,
            message_id: id,
            thread_root_id: root,
        },
    );
    if let Some(root) = root {
        if let Some(root) = state.store.message(root).await? {
            state
                .hub
                .send(&audience, &ServerEvent::MessageUpdated { message: root });
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn react(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<ReactRequest>,
) -> AppResult<Json<Vec<agora_protocol::Reaction>>> {
    let id = MessageId(id);
    let (access, _, _, deleted) = message_access(&state, id, auth.user.id).await?;
    if deleted || !access.can_post() {
        return Err(AppError::forbidden("you cannot react here"));
    }
    check_write(&state, auth.user.id)?;
    let emoji = validate::reaction(&req.emoji)?;
    if state.store.reaction_kinds(id).await? >= REACTION_KINDS_MAX {
        let existing = state
            .store
            .message(id)
            .await?
            .map(|m| m.reactions)
            .unwrap_or_default();
        if !existing.iter().any(|r| r.emoji == emoji) {
            return Err(AppError::invalid(
                "this message has too many different reactions",
            ));
        }
    }
    let reactions = state
        .store
        .toggle_reaction(id, auth.user.id, &emoji, now_ms())
        .await?;
    let audience = state.store.channel_audience(&access.channel).await?;
    state.hub.send(
        &audience,
        &ServerEvent::ReactionsChanged {
            channel_id: access.channel.id,
            message_id: id,
            reactions: reactions.clone(),
        },
    );
    Ok(Json(reactions))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: String,
}

pub async fn search(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<SearchResults>> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    let query = validate::search_query(&q.q)?;
    let messages = state
        .store
        .search(
            ws,
            auth.user.id,
            role == agora_core::Role::Guest,
            &query,
            50,
        )
        .await?;
    Ok(Json(SearchResults { messages }))
}

/// `POST /workspaces/{id}/messages/lookup`: messages by id, for a client's own search results.
pub async fn lookup(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<LookupMessagesRequest>,
) -> AppResult<Json<SearchResults>> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    if req.ids.len() > 100 {
        return Err(AppError::invalid("ask for at most 100 messages"));
    }
    let messages = state
        .store
        .lookup_messages(ws, auth.user.id, role == agora_core::Role::Guest, &req.ids)
        .await?;
    Ok(Json(SearchResults { messages }))
}
