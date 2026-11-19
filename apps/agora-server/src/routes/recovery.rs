//! Changing a password, and getting back into an account without it: the recovery key the
//! person keeps, or a one-use code from whoever runs the server. No email is needed.

use std::net::SocketAddr;

use agora_core::validate;
use agora_protocol::{
    ChangePasswordRequest, ErrorCode, RecoveryKey, RecoveryKeyRequest, RecoveryStatus,
    ResetPasswordRequest, ResetPasswordResponse,
};
use agora_storage::recovery::ResetKind;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;

use super::client_ip;
use crate::auth::{self, now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::mfa_crypto;
use crate::mfa_service;
use crate::recovery::{self, Purpose};
use crate::state::AppState;

fn wrong() -> AppError {
    AppError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
        "authentication failed",
    )
}

fn bad_code() -> AppError {
    let mut e = AppError::invalid("that recovery code is not valid or has expired");
    e.body.field = Some("code".into());
    e
}

/// Attempts at passwords for one signed-in account, per minute.
fn check_rate(state: &AppState, auth: &Auth) -> AppResult<()> {
    if state.limiter.allow(
        &format!("reauth:{}", auth.user.id),
        state.config.limits.auth_per_minute,
    ) {
        Ok(())
    } else {
        Err(AppError::rate_limited())
    }
}

async fn check_password(state: &AppState, auth: &Auth, password: String) -> AppResult<()> {
    if password.chars().count() > agora_core::limits::PASSWORD_MAX {
        return Err(wrong());
    }
    let (_, hash) = state
        .store
        .user_credentials(&auth.user.username)
        .await?
        .ok_or_else(wrong)?;
    if auth::verify_password(password, hash).await {
        Ok(())
    } else {
        Err(wrong())
    }
}

pub async fn status(State(state): State<AppState>, auth: Auth) -> AppResult<Json<RecoveryStatus>> {
    Ok(Json(RecoveryStatus {
        has_key: state.store.has_recovery_key(auth.user.id).await?,
    }))
}

/// A new recovery key; the one before it stops working. Shown once.
pub async fn create_key(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<RecoveryKeyRequest>,
) -> AppResult<Json<RecoveryKey>> {
    check_rate(&state, &auth)?;
    check_password(&state, &auth, req.password).await?;
    let key = recovery::new_code();
    state
        .store
        .set_recovery_key(
            auth.user.id,
            &recovery::hash(auth.user.id, Purpose::Key, &key),
            now_ms(),
        )
        .await?;
    tracing::info!(user = %auth.user.id, "recovery key created");
    Ok(Json(RecoveryKey { recovery_key: key }))
}

/// Change the password. Every other session is signed out.
pub async fn change_password(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    check_rate(&state, &auth)?;
    validate::password(&req.new_password)?;
    check_password(&state, &auth, req.password).await?;
    let hash = auth::hash_password(req.new_password).await?;
    let ids = state
        .store
        .change_password(auth.user.id, &hash, auth.session_id)
        .await?;
    for id in &ids {
        state.hub.close_session(*id);
    }
    tracing::info!(user = %auth.user.id, "password changed");
    Ok(StatusCode::NO_CONTENT)
}

/// Set a new password with the recovery key or an operator's code, and sign in.
pub async fn reset(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ResetPasswordRequest>,
) -> AppResult<Json<ResetPasswordResponse>> {
    let ip = client_ip(&state, peer, &headers);
    let username = req.username.trim().to_lowercase();
    // Codes are guessable in a way passwords are not: limit them per address and per account.
    if !state
        .limiter
        .allow(&format!("auth:{ip}"), state.config.limits.auth_per_minute)
        || !state.limiter.allow(
            &format!("reset:{username}"),
            state.config.limits.auth_per_minute.min(5),
        )
    {
        return Err(AppError::rate_limited());
    }
    validate::password(&req.new_password)?;
    // Hash first, so an unknown account costs the same time as a known one.
    let new_hash = auth::hash_password(req.new_password.clone()).await?;
    let Some((user, _)) = state.store.user_credentials(&username).await? else {
        return Err(bad_code());
    };
    let now = now_ms();
    let operator = recovery::hash(user.id, Purpose::Operator, &req.code);
    let key = recovery::hash(user.id, Purpose::Key, &req.code);
    let kind = state
        .store
        .peek_reset(user.id, &operator, &key, now)
        .await?
        .ok_or_else(bad_code)?;
    if kind == ResetKind::Key && state.store.has_totp(user.id).await? {
        verify_second_factor(&state, &user, &req, now).await?;
    }
    let (_, sessions) = state
        .store
        .reset_password(user.id, &operator, &key, &new_hash, now)
        .await?
        .ok_or_else(bad_code)?;
    for id in sessions {
        state.hub.close_session(id);
    }
    let new_key = recovery::new_code();
    state
        .store
        .set_recovery_key(
            user.id,
            &recovery::hash(user.id, Purpose::Key, &new_key),
            now,
        )
        .await?;
    tracing::info!(user = %user.id, "password reset");
    let session = super::accounts::new_session(&state, user).await?;
    Ok(Json(ResetPasswordResponse {
        session,
        recovery_key: new_key,
    }))
}

/// The recovery key alone is not enough for an account protected by two-step verification.
async fn verify_second_factor(
    state: &AppState,
    user: &agora_protocol::User,
    req: &ResetPasswordRequest,
    now: i64,
) -> AppResult<()> {
    let needed = || {
        AppError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::MfaRequired,
            "enter the code from your authenticator app",
        )
    };
    let totp = req.totp.as_deref().map(mfa_crypto::normalize_code);
    let spare = req.recovery_code.as_deref().map(mfa_crypto::normalize_code);
    let internal = |e: mfa_service::Error| {
        tracing::error!(error = %e, "two-step check during a password reset failed");
        AppError::internal()
    };
    let ok = match (
        totp.filter(|c| !c.is_empty()),
        spare.filter(|c| !c.is_empty()),
    ) {
        (Some(code), _) => {
            mfa_service::verify_current_code(&state.store, &state.mfa.key, user.id, &code, now)
                .await
                .map_err(internal)?
        }
        (None, Some(code)) => {
            // The session this creates is signed out with the rest by the reset.
            let hash = auth::token_hash(&auth::random_token(32));
            mfa_service::verify_recovery_and_create_session(
                &state.store,
                user.id,
                &code,
                &hash,
                now,
                now.saturating_add(60_000),
            )
            .await
            .map_err(internal)?
            .is_some()
        }
        (None, None) => return Err(needed()),
    };
    if ok {
        Ok(())
    } else {
        let mut e = AppError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::MfaRequired,
            "that code is not valid",
        );
        e.body.field = Some("code".into());
        Err(e)
    }
}
