//! Two-step verification (TOTP) for the signed-in account: set up, enable, disable and new
//! recovery codes. Sign-in with a second factor is in `accounts::login`.

use agora_protocol::{
    ErrorCode, MfaCodeRequest, MfaDisableRequest, MfaRecoveryCodes, MfaSetup, MfaSetupRequest,
    MfaStatus,
};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use zeroize::Zeroizing;

use crate::auth::{self, now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::mfa_crypto;
use crate::mfa_service;
use crate::state::AppState;

fn wrong() -> AppError {
    AppError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
        "authentication failed",
    )
}

fn wrong_code() -> AppError {
    let mut e = AppError::invalid("that code is not valid; check the time on your device");
    e.body.field = Some("code".into());
    e
}

/// Attempts at passwords and codes for one signed-in account, per minute.
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

fn crypto_failed(e: impl std::fmt::Display) -> AppError {
    tracing::error!(error = %e, "two-step verification failed");
    AppError::internal()
}

pub async fn status(State(state): State<AppState>, auth: Auth) -> AppResult<Json<MfaStatus>> {
    let enabled = state.store.has_totp(auth.user.id).await?;
    let left = if enabled {
        state.store.recovery_codes_left(auth.user.id).await?
    } else {
        0
    };
    Ok(Json(MfaStatus {
        enabled,
        recovery_codes_left: u32::try_from(left).unwrap_or(0),
    }))
}

pub async fn setup(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<MfaSetupRequest>,
) -> AppResult<Json<MfaSetup>> {
    check_rate(&state, &auth)?;
    check_password(&state, &auth, req.password).await?;
    if state.store.has_totp(auth.user.id).await? {
        return Err(AppError::conflict("two-step verification is already on"));
    }
    let secret = mfa_crypto::new_totp_secret().map_err(crypto_failed)?;
    let setup = MfaSetup {
        secret: mfa_crypto::base32(secret.as_ref()),
        uri: mfa_crypto::otpauth_uri(&state.config.name, &auth.user.username, secret.as_ref()),
    };
    state.mfa.set_pending(auth.user.id, secret, now_ms());
    Ok(Json(setup))
}

/// Recovery codes to show once, and their hashes to store.
type RecoveryCodes = (Vec<Zeroizing<String>>, Vec<[u8; 32]>);

fn new_recovery_codes(user: agora_core::UserId) -> AppResult<RecoveryCodes> {
    let mut codes = Vec::with_capacity(10);
    let mut hashes = Vec::with_capacity(10);
    for _ in 0..10 {
        let code = mfa_crypto::new_recovery_token().map_err(crypto_failed)?;
        hashes.push(mfa_crypto::recovery_hash(user, &code).ok_or_else(AppError::internal)?);
        codes.push(code);
    }
    Ok((codes, hashes))
}

/// Confirm the pending secret with a code. Every other session is signed out, so the second
/// factor protects them from now on.
pub async fn enable(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<MfaCodeRequest>,
) -> AppResult<Json<MfaRecoveryCodes>> {
    check_rate(&state, &auth)?;
    let now = now_ms();
    let secret = state
        .mfa
        .pending(auth.user.id, now)
        .ok_or_else(|| AppError::invalid("start again: the setup expired"))?;
    let code = mfa_crypto::normalize_code(&req.code);
    let step = mfa_crypto::verify_totp(
        secret.as_ref(),
        &code,
        u64::try_from(now / 1000).unwrap_or(0),
        None,
    )
    .map_err(crypto_failed)?
    .ok_or_else(wrong_code)?;
    let sealed = state
        .mfa
        .key
        .seal(auth.user.id, secret.as_ref())
        .map_err(crypto_failed)?;
    let (codes, hashes) = new_recovery_codes(auth.user.id)?;
    match state
        .store
        .enroll_totp(
            auth.user.id,
            &sealed,
            i64::try_from(step).unwrap_or(i64::MAX),
            &hashes,
            now,
        )
        .await
    {
        Ok(()) => {}
        Err(agora_storage::StoreError::Conflict) => {
            return Err(AppError::conflict("two-step verification is already on"))
        }
        Err(e) => return Err(e.into()),
    }
    state.mfa.clear_pending(auth.user.id);
    let ids = state
        .store
        .delete_other_sessions(auth.user.id, auth.session_id)
        .await?;
    for id in &ids {
        state.hub.close_session(*id);
    }
    tracing::info!(user = %auth.user.id, "two-step verification enabled");
    Ok(Json(MfaRecoveryCodes {
        recovery_codes: codes.iter().map(|c| c.to_string()).collect(),
    }))
}

pub async fn disable(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<MfaDisableRequest>,
) -> AppResult<StatusCode> {
    check_rate(&state, &auth)?;
    check_password(&state, &auth, req.password).await?;
    let code = mfa_crypto::normalize_code(&req.code);
    if !mfa_service::verify_current_code(
        &state.store,
        &state.mfa.key,
        auth.user.id,
        &code,
        now_ms(),
    )
    .await
    .map_err(crypto_failed)?
    {
        return Err(wrong_code());
    }
    state.store.remove_totp(auth.user.id).await?;
    tracing::info!(user = %auth.user.id, "two-step verification disabled");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn recovery_codes(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<MfaCodeRequest>,
) -> AppResult<Json<MfaRecoveryCodes>> {
    check_rate(&state, &auth)?;
    let code = mfa_crypto::normalize_code(&req.code);
    if !mfa_service::verify_current_code(
        &state.store,
        &state.mfa.key,
        auth.user.id,
        &code,
        now_ms(),
    )
    .await
    .map_err(crypto_failed)?
    {
        return Err(wrong_code());
    }
    let (codes, hashes) = new_recovery_codes(auth.user.id)?;
    if !state
        .store
        .replace_recovery_codes(auth.user.id, &hashes)
        .await?
    {
        return Err(AppError::not_found());
    }
    Ok(Json(MfaRecoveryCodes {
        recovery_codes: codes.iter().map(|c| c.to_string()).collect(),
    }))
}
