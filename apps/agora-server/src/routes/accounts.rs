use std::net::SocketAddr;

use agora_core::validate;
use agora_protocol::{
    ErrorCode, LoginRequest, RegisterRequest, RevokeOtherSessionsRequest, ServerInfo,
    SessionResponse, UpdateProfileRequest, User,
};
use agora_storage::StoreError;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;

use super::client_ip;
use crate::auth::{self, now_ms, Auth};
use crate::config::RegistrationPolicy;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub async fn server_info(State(state): State<AppState>) -> Json<ServerInfo> {
    Json(ServerInfo {
        name: state.config.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        api_version: agora_protocol::API_VERSION,
        registration: state.config.registration.into(),
    })
}

fn check_auth_rate(state: &AppState, peer: SocketAddr, headers: &HeaderMap) -> AppResult<()> {
    let ip = client_ip(state, peer, headers);
    if state
        .limiter
        .allow(&format!("auth:{ip}"), state.config.limits.auth_per_minute)
    {
        Ok(())
    } else {
        Err(AppError::rate_limited())
    }
}

async fn new_session(state: &AppState, user: User) -> AppResult<SessionResponse> {
    let token = auth::random_token(32);
    let now = now_ms();
    let expires_at = now.saturating_add(i64::from(state.config.session_days) * 86_400_000);
    state
        .store
        .create_session(&auth::token_hash(&token), user.id, now, expires_at)
        .await?;
    Ok(SessionResponse {
        token,
        expires_at,
        user,
    })
}

pub async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> AppResult<(StatusCode, Json<SessionResponse>)> {
    check_auth_rate(&state, peer, &headers)?;
    let username = validate::username(&req.username)?;
    let display_name = validate::display_name(&req.display_name)?;
    validate::password(&req.password)?;
    let invite = req
        .invite_code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());

    // The very first account can always be created, so a fresh invite-only server can be set up.
    let first = state.store.user_count().await? == 0;
    let allowed = match state.config.registration {
        RegistrationPolicy::Open => true,
        RegistrationPolicy::InviteOnly => invite.is_some() || first,
        RegistrationPolicy::Closed => first,
    };
    if !allowed {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::RegistrationClosed,
            "this server only accepts new accounts with an invitation",
        ));
    }
    // Check the invitation before creating anything, so a bad code leaves no orphan account.
    let invite_hash = invite.map(auth::token_hash);
    if let Some(hash) = &invite_hash {
        if !state.store.invite_usable(hash, now_ms()).await? {
            return Err(AppError::new(
                StatusCode::GONE,
                ErrorCode::NotFound,
                "this invitation is not valid, has expired or has been used up",
            ));
        }
    }

    let hash = auth::hash_password(req.password).await?;
    let user = match state
        .store
        .create_user(&username, &display_name, &hash, now_ms())
        .await
    {
        Ok(u) => u,
        Err(StoreError::Conflict) => {
            let mut e = AppError::conflict("that username is taken");
            e.body.field = Some("username".into());
            return Err(e);
        }
        Err(e) => return Err(e.into()),
    };
    if let Some(hash) = invite_hash {
        // Another person may have used the last place in between; the account still stands.
        if let Err(e) = super::workspaces::redeem(&state, &hash, &user).await {
            tracing::info!(status = %e.status, "invitation could not be used at registration");
        }
    }
    tracing::info!(user = %user.id, "account created");
    Ok((StatusCode::CREATED, Json(new_session(&state, user).await?)))
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<SessionResponse>> {
    check_auth_rate(&state, peer, &headers)?;
    let wrong = || {
        AppError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
            "wrong username or password",
        )
    };
    if req.password.chars().count() > agora_core::limits::PASSWORD_MAX {
        return Err(wrong());
    }
    let username = req.username.trim().to_lowercase();
    let found = state.store.user_credentials(&username).await?;
    let hash = found
        .as_ref()
        .map_or_else(|| state.dummy_hash.to_string(), |(_, h)| h.clone());
    let ok = auth::verify_password(req.password, hash).await;
    match found {
        Some((user, _)) if ok => Ok(Json(new_session(&state, user).await?)),
        _ => Err(wrong()),
    }
}

pub async fn logout(State(state): State<AppState>, auth: Auth) -> AppResult<StatusCode> {
    if let Some(id) = state.store.delete_session(&auth.token_hash).await? {
        state.hub.close_session(id);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Require the account password again before revoking other sessions.
pub async fn revoke_other_sessions(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<RevokeOtherSessionsRequest>,
) -> AppResult<StatusCode> {
    if !state.limiter.allow(
        &format!("reauth:{}", auth.user.id),
        state.config.limits.auth_per_minute,
    ) {
        return Err(AppError::rate_limited());
    }
    let wrong = || AppError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
        "authentication failed",
    );
    if req.password.chars().count() > agora_core::limits::PASSWORD_MAX {
        return Err(wrong());
    }
    let (_, hash) = state.store.user_credentials(&auth.user.username).await?
        .ok_or_else(wrong)?;
    if !auth::verify_password(req.password, hash).await {
        return Err(wrong());
    }
    let ids = state.store.delete_other_sessions(auth.user.id, auth.session_id).await?;
    for id in &ids {
        state.hub.close_session(*id);
    }
    tracing::info!(user = %auth.user.id, count = ids.len(), "other sessions revoked");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(auth: Auth) -> Json<User> {
    Json(auth.user)
}

pub async fn update_me(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<UpdateProfileRequest>,
) -> AppResult<Json<User>> {
    let display_name = validate::display_name(&req.display_name)?;
    state
        .store
        .set_display_name(auth.user.id, &display_name)
        .await?;
    Ok(Json(User {
        display_name,
        ..auth.user
    }))
}
