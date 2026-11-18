use agora_core::{limits, validate, InviteId, Role, WorkspaceId};
use agora_protocol::{
    CreateInviteRequest, CreateWorkspaceRequest, Invite, JoinWorkspaceRequest, Member, ServerEvent,
    User, Workspace,
};
use agora_storage::workspaces::NewInvite;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::Json;

use super::workspace_role;
use crate::auth::{self, now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Workspaces one person may create; keeps a single account from filling the disk.
const CREATED_MAX: usize = 50;

/// Icons are bounded metadata, never treated as encrypted content.
pub(super) const ICON_MAX_BYTES: usize = 128 * 1024;
const ICON_MAX_DIMENSION: u32 = 512;

fn valid_icon(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > ICON_MAX_BYTES {
        return false;
    }
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let Ok(mut reader) = decoder.read_info() else {
        return false;
    };
    let info = reader.info();
    if info.width == 0
        || info.height == 0
        || info.width > ICON_MAX_DIMENSION
        || info.height > ICON_MAX_DIMENSION
        || info.animation_control.is_some()
        || reader.output_buffer_size() > 2 * 1024 * 1024
    {
        return false;
    }
    let mut output = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut output).is_ok()
}

pub async fn get_icon(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<([(header::HeaderName, &'static str); 1], Vec<u8>)> {
    let ws = WorkspaceId(id);
    workspace_role(&state, ws, auth.user.id).await?;
    let icon = state
        .store
        .workspace_icon(ws)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(([(header::CONTENT_TYPE, "image/png")], icon))
}

pub async fn put_icon(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    bytes: Bytes,
) -> AppResult<StatusCode> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    if !role.is_admin() {
        return Err(AppError::forbidden(
            "only workspace owners and admins can change the icon",
        ));
    }
    super::check_write(&state, auth.user.id)?;
    if !valid_icon(&bytes) {
        return Err(AppError::invalid(
            "icon must be a static PNG of at most 512 × 512 pixels and 128 KiB",
        ));
    }
    state.store.set_workspace_icon(ws, Some(&bytes)).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_icon(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    if !role.is_admin() {
        return Err(AppError::forbidden(
            "only workspace owners and admins can change the icon",
        ));
    }
    super::check_write(&state, auth.user.id)?;
    state.store.set_workspace_icon(ws, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(State(state): State<AppState>, auth: Auth) -> AppResult<Json<Vec<Workspace>>> {
    Ok(Json(state.store.workspaces_of(auth.user.id).await?))
}

pub async fn get_one(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Workspace>> {
    let ws = state
        .store
        .workspace(WorkspaceId(id), auth.user.id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(Json(ws))
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<CreateWorkspaceRequest>,
) -> AppResult<(StatusCode, Json<Workspace>)> {
    let name = validate::workspace_name(&req.name)?;
    let owned = state
        .store
        .workspaces_of(auth.user.id)
        .await?
        .iter()
        .filter(|w| w.role == Role::Owner)
        .count();
    if owned >= CREATED_MAX {
        return Err(AppError::forbidden(
            "you already own the maximum number of workspaces",
        ));
    }
    let id = state
        .store
        .create_workspace(&name, auth.user.id, now_ms())
        .await?;
    let ws = state
        .store
        .workspace(id, auth.user.id)
        .await?
        .ok_or_else(AppError::internal)?;
    Ok((StatusCode::CREATED, Json(ws)))
}

pub async fn members(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Member>>> {
    let ws = WorkspaceId(id);
    workspace_role(&state, ws, auth.user.id).await?;
    let mut members = state.store.members(ws).await?;
    for m in &mut members {
        m.online = state.hub.is_online(m.user.id);
    }
    Ok(Json(members))
}

pub async fn create_invite(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CreateInviteRequest>,
) -> AppResult<(StatusCode, Json<Invite>)> {
    let ws = WorkspaceId(id);
    let role = workspace_role(&state, ws, auth.user.id).await?;
    if !role.can_invite() {
        return Err(AppError::forbidden(
            "only owners and admins can invite people",
        ));
    }
    if let Some(owner) = state.store.workspace_organization_owner(ws).await? {
        if owner != auth.user.id {
            return Err(AppError::forbidden(
                "only the organization owner can invite people to its workspaces",
            ));
        }
    }
    super::check_write(&state, auth.user.id)?;
    let max_uses = req.max_uses.unwrap_or(1);
    let hours = req.expires_in_hours.unwrap_or(168);
    if !(1..=1_000).contains(&max_uses) || !(1..=720).contains(&hours) {
        return Err(AppError::invalid(
            "max_uses must be 1–1000 and expires_in_hours 1–720",
        ));
    }
    let invite_role = req.role.unwrap_or(Role::Member);
    if !matches!(invite_role, Role::Member | Role::Guest) {
        return Err(AppError::invalid(
            "an invitation can make people members or guests",
        ));
    }
    let code = auth::random_token(15);
    let now = now_ms();
    let expires_at = now.saturating_add(i64::from(hours) * 3_600_000);
    let invite_id = state
        .store
        .create_invite(NewInvite {
            workspace: ws,
            code_hash: &auth::token_hash(&code),
            role: invite_role,
            by: auth.user.id,
            max_uses,
            expires_at,
            now,
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(Invite {
            id: InviteId(invite_id),
            workspace_id: ws,
            code,
            max_uses,
            expires_at,
            role: invite_role,
        }),
    ))
}

/// Use an invitation for `user` and tell the workspace's members.
pub async fn redeem(state: &AppState, code_hash: &[u8], user: &User) -> AppResult<Workspace> {
    let joined = state
        .store
        .redeem_invite(code_hash, user.id, now_ms())
        .await?;
    notify_joined(state, user, joined).await?;
    state
        .store
        .workspace(joined.workspace_id, user.id)
        .await?
        .ok_or_else(AppError::internal)
}

/// Notify existing workspace members after a committed join.
pub async fn notify_joined(
    state: &AppState,
    user: &User,
    joined: agora_storage::workspaces::Joined,
) -> AppResult<()> {
    if joined.new_member {
        if state.store.member_count(joined.workspace_id).await?
            > i64::from(limits::WORKSPACE_MEMBERS_MAX)
        {
            tracing::warn!(workspace = %joined.workspace_id, "workspace is over its member limit");
        }
        let member = Member {
            user: user.clone(),
            role: joined.role,
            joined_at: now_ms(),
            online: state.hub.is_online(user.id),
        };
        let audience = state
            .store
            .workspace_member_ids(joined.workspace_id)
            .await?;
        state.hub.send(
            &audience,
            &ServerEvent::MemberJoined {
                workspace_id: joined.workspace_id,
                member,
            },
        );
    }
    Ok(())
}

pub async fn join(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<JoinWorkspaceRequest>,
) -> AppResult<Json<Workspace>> {
    if !state.limiter.allow(
        &format!("join:{}", auth.user.id),
        state.config.limits.auth_per_minute,
    ) {
        return Err(AppError::rate_limited());
    }
    let code = req.code.trim();
    if code.is_empty() || code.len() > 64 {
        return Err(AppError::not_found());
    }
    Ok(Json(
        redeem(&state, &auth::token_hash(code), &auth.user).await?,
    ))
}
