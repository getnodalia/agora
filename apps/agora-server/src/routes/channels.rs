use agora_core::{roles, validate, ChannelId, ChannelKind, WorkspaceId};
use agora_protocol::{
    AddChannelMemberRequest, Channel, CreateChannelRequest, MarkReadRequest, OpenDirectRequest,
    Receipt, ServerEvent, UpdateChannelRequest,
};
use agora_storage::ChannelRow;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use super::{channel_access, workspace_role};
use crate::auth::{now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Tell everyone who can see `channel` that it changed.
async fn announce(state: &AppState, channel: &ChannelRow) -> AppResult<()> {
    let audience = state.store.channel_audience(channel).await?;
    state.hub.send(
        &audience,
        &ServerEvent::ChannelChanged {
            workspace_id: channel.workspace_id,
            channel_id: channel.id,
        },
    );
    Ok(())
}

async fn view(state: &AppState, id: ChannelId, auth: &Auth) -> AppResult<Json<Channel>> {
    Ok(Json(
        state
            .store
            .channel_view(id, auth.user.id)
            .await?
            .ok_or_else(AppError::not_found)?,
    ))
}

pub async fn list(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Channel>>> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    Ok(Json(
        state.store.channels_for(ws, auth.user.id, role).await?,
    ))
}

pub async fn get_one(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Channel>> {
    channel_access(&state, ChannelId(id), auth.user.id).await?;
    view(&state, ChannelId(id), &auth).await
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CreateChannelRequest>,
) -> AppResult<(StatusCode, Json<Channel>)> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    if !role.can_create_channel() {
        return Err(AppError::forbidden("guests cannot create channels"));
    }
    super::check_write(&state, auth.user.id)?;
    let name = validate::channel_name(&req.name)?;
    let topic = validate::topic(&req.topic)?;
    let kind = if req.private {
        ChannelKind::Private
    } else {
        ChannelKind::Public
    };
    let id = state
        .store
        .create_channel(ws, &name, &topic, kind, auth.user.id, now_ms())
        .await
        .map_err(|e| match e {
            agora_storage::StoreError::Conflict => {
                AppError::conflict("a channel with that name already exists")
            }
            other => other.into(),
        })?;
    let row = state
        .store
        .channel(id)
        .await?
        .ok_or_else(AppError::internal)?;
    announce(&state, &row).await?;
    Ok((StatusCode::CREATED, view(&state, id, &auth).await?))
}

pub async fn update(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<UpdateChannelRequest>,
) -> AppResult<Json<Channel>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    let created_it = access.channel.created_by == Some(auth.user.id);
    if !roles::can_manage_channel(access.role, access.channel.kind, access.member, created_it) {
        return Err(AppError::forbidden(
            "only admins and the channel's creator can change it",
        ));
    }
    if access.channel.is_default && req.archived == Some(true) {
        return Err(AppError::forbidden(
            "the default channel cannot be archived",
        ));
    }
    let name = req
        .name
        .as_deref()
        .map(validate::channel_name)
        .transpose()?;
    let topic = req.topic.as_deref().map(validate::topic).transpose()?;
    state
        .store
        .update_channel(
            access.channel.id,
            name.as_deref(),
            topic.as_deref(),
            req.archived,
        )
        .await
        .map_err(|e| match e {
            agora_storage::StoreError::Conflict => {
                AppError::conflict("a channel with that name already exists")
            }
            other => other.into(),
        })?;
    announce(&state, &access.channel).await?;
    view(&state, access.channel.id, &auth).await
}

pub async fn join(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Channel>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    if !access.member {
        if !roles::can_self_join(access.role, access.channel.kind) {
            return Err(AppError::forbidden("you need to be added to this channel"));
        }
        state
            .store
            .join_channel(access.channel.id, auth.user.id, now_ms())
            .await?;
        announce(&state, &access.channel).await?;
    }
    view(&state, access.channel.id, &auth).await
}

pub async fn leave(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    if access.channel.kind == ChannelKind::Direct {
        return Err(AppError::forbidden("direct conversations cannot be left"));
    }
    if state
        .store
        .leave_channel(access.channel.id, auth.user.id)
        .await?
    {
        announce(&state, &access.channel).await?;
        if access.channel.kind == ChannelKind::Private {
            state.hub.send(
                &[auth.user.id],
                &ServerEvent::ChannelRemoved {
                    workspace_id: access.channel.workspace_id,
                    channel_id: access.channel.id,
                },
            );
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_member(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<AddChannelMemberRequest>,
) -> AppResult<Json<Channel>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    if !roles::can_add_members(access.role, access.channel.kind, access.member) {
        return Err(AppError::forbidden("you cannot add people to this channel"));
    }
    // The person must belong to the same workspace; otherwise they do not exist from here.
    state
        .store
        .role(access.channel.workspace_id, req.user_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if state
        .store
        .join_channel(access.channel.id, req.user_id, now_ms())
        .await?
    {
        announce(&state, &access.channel).await?;
    }
    view(&state, access.channel.id, &auth).await
}

pub async fn open_direct(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<OpenDirectRequest>,
) -> AppResult<Json<Channel>> {
    let ws = WorkspaceId(id);
    workspace_role(&state, ws, auth.user.id).await?;
    state
        .store
        .role(ws, req.user_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let channel = state
        .store
        .open_direct(ws, auth.user.id, req.user_id, now_ms())
        .await?;
    let row = state
        .store
        .channel(channel)
        .await?
        .ok_or_else(AppError::internal)?;
    announce(&state, &row).await?;
    view(&state, channel, &auth).await
}

pub async fn mark_read(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<MarkReadRequest>,
) -> AppResult<StatusCode> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    let before = state.store.receipt(access.channel.id, auth.user.id).await?;
    if let Some(marker) = state
        .store
        .mark_read(access.channel.id, auth.user.id, req.message_id)
        .await?
    {
        state.hub.send(
            &[auth.user.id],
            &ServerEvent::ReadMarker {
                channel_id: access.channel.id,
                message_id: marker,
            },
        );
        let after = state.store.receipt(access.channel.id, auth.user.id).await?;
        announce_receipt(&state, &access.channel, before, after).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// One of the caller's devices received messages up to `message_id`.
pub async fn mark_delivered(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<MarkReadRequest>,
) -> AppResult<StatusCode> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    let before = state.store.receipt(access.channel.id, auth.user.id).await?;
    let after = state
        .store
        .mark_delivered(access.channel.id, auth.user.id, req.message_id)
        .await?;
    announce_receipt(&state, &access.channel, before, after).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// How far each member has received and read a conversation the caller can read.
pub async fn receipts(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Receipt>>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    let receipts = state.store.receipts(access.channel.id).await?;
    Ok(Json(receipts.into_iter().map(wire_receipt).collect()))
}

fn wire_receipt(r: agora_storage::Receipt) -> Receipt {
    Receipt {
        user_id: r.user,
        delivered_id: r.delivered,
        read_id: r.read,
    }
}

/// Tell everyone who can read the channel that a member's receipt moved (and nothing when it
/// did not, so repeated calls cost no traffic).
async fn announce_receipt(
    state: &AppState,
    channel: &ChannelRow,
    before: Option<agora_storage::Receipt>,
    after: Option<agora_storage::Receipt>,
) -> AppResult<()> {
    let Some(after) = after else {
        return Ok(());
    };
    if before == Some(after) {
        return Ok(());
    }
    let audience = state.store.channel_audience(channel).await?;
    state.hub.send(
        &audience,
        &ServerEvent::Receipt {
            channel_id: channel.id,
            receipt: wire_receipt(after),
        },
    );
    Ok(())
}
