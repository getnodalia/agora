//! Incoming webhooks: another app (Hito, a script, CI) posts notices to a channel through a
//! signed HTTP request. Notices are server-readable and shown apart from encrypted messages.

use std::net::SocketAddr;

use agora_core::{ChannelId, ChannelKind};
use agora_protocol::{
    CreateIntegrationRequest, CreatedIntegration, Integration, IntegrationNotice, ServerEvent,
};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{channel_access, client_ip, ChannelAccess};
use crate::auth::{now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::integrations::github::verify_signature;
use crate::state::AppState;

/// The largest request an integration can send.
pub const BODY_MAX: usize = 64 * 1024;
const NAME_MAX: usize = 60;
const TITLE_MAX: usize = 200;
const TEXT_MAX: usize = 2_000;
const URL_MAX: usize = 500;
/// Integrations one channel can have.
const PER_CHANNEL: usize = 20;
/// Notices one integration can post per minute.
const NOTICES_PER_MINUTE: u32 = 60;

fn manageable(access: &ChannelAccess) -> AppResult<()> {
    if access.channel.kind == ChannelKind::Direct {
        return Err(AppError::invalid(
            "direct conversations do not take integrations",
        ));
    }
    if !access.role.is_admin() {
        return Err(AppError::forbidden(
            "only workspace admins can manage integrations",
        ));
    }
    Ok(())
}

pub async fn list(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Integration>>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    Ok(Json(state.store.integrations(access.channel.id).await?))
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CreateIntegrationRequest>,
) -> AppResult<(StatusCode, Json<CreatedIntegration>)> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    manageable(&access)?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
        let mut e = AppError::invalid(format!(
            "a name of 1 to {NAME_MAX} characters, without control characters"
        ));
        e.body.field = Some("name".into());
        return Err(e);
    }
    if state.store.integrations(access.channel.id).await?.len() >= PER_CHANNEL {
        return Err(AppError::invalid(format!(
            "a channel can have at most {PER_CHANNEL} integrations"
        )));
    }
    let mut secret = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut secret)
        .map_err(|_| AppError::internal())?;
    let integration = state
        .store
        .create_integration(
            access.channel.id,
            req.kind,
            name,
            &secret,
            auth.user.id,
            now_ms(),
        )
        .await?;
    let path = format!("/api/v1/hooks/{}", integration.id);
    Ok((
        StatusCode::CREATED,
        Json(CreatedIntegration {
            integration,
            path,
            secret: hex(&secret),
        }),
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let (integration, _) = state
        .store
        .integration_secret(id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let access = channel_access(&state, integration.channel_id, auth.user.id).await?;
    manageable(&access)?;
    state.store.delete_integration(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn notices(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<IntegrationNotice>>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    Ok(Json(state.store.notices(access.channel.id, 50).await?))
}

/// What a request may carry. Hito's payload has `project.name` and `server_url`; any other app
/// sends `text` with an optional `title` and `url`.
#[derive(Deserialize)]
struct Payload {
    text: String,
    title: Option<String>,
    url: Option<String>,
    server_url: Option<String>,
    project: Option<Project>,
}

#[derive(Deserialize)]
struct Project {
    name: Option<String>,
}

/// Receive a notice. The signature is HMAC-SHA256 of the raw body, keyed with the integration's
/// secret as the hex text it was shown as, in `X-Agora-Signature` or Hito's `X-Hito-Signature`, as `sha256=<hex>`.
pub async fn receive(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Bytes,
) -> AppResult<StatusCode> {
    let ip = client_ip(&state, peer, &headers);
    if !state
        .limiter
        .allow(&format!("hook:{ip}"), state.config.limits.writes_per_minute)
        || !state
            .limiter
            .allow(&format!("hook-id:{id}"), NOTICES_PER_MINUTE)
    {
        return Err(AppError::rate_limited());
    }
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let signature = header("x-agora-signature")
        .or_else(|| header("x-hito-signature"))
        .unwrap_or("");
    // An unknown integration and a wrong signature answer the same.
    let Some((integration, secret)) = state.store.integration_secret(id).await? else {
        return Err(AppError::unauthenticated());
    };
    // The key is the text the person pasted into the other app, as GitHub's webhooks do.
    if !verify_signature(hex(&secret).as_bytes(), &body, signature) {
        return Err(AppError::unauthenticated());
    }
    let payload: Payload =
        serde_json::from_slice(&body).map_err(|_| AppError::invalid("invalid payload"))?;
    let text = clean(&payload.text, TEXT_MAX);
    if text.is_empty() {
        return Err(AppError::invalid("the notice has no text"));
    }
    let title = payload
        .title
        .or_else(|| payload.project.and_then(|p| p.name))
        .map(|t| clean(&t, TITLE_MAX))
        .filter(|t| !t.is_empty());
    let url = payload
        .url
        .or(payload.server_url)
        .map(|u| u.trim().to_owned())
        .filter(|u| web_link(u));
    let hash: [u8; 32] = Sha256::digest(&body).into();
    let recorded = state
        .store
        .record_notice(agora_storage::integrations::NewNotice {
            integration: integration.id,
            channel: integration.channel_id,
            hash: &hash,
            title: title.as_deref(),
            text: &text,
            url: url.as_deref(),
            now: now_ms(),
        })
        .await?;
    if let Some(notice) = recorded {
        if let Some(channel) = state.store.channel(integration.channel_id).await? {
            let audience = state.store.channel_audience(&channel).await?;
            state
                .hub
                .send(&audience, &ServerEvent::IntegrationNotice { notice });
        }
    }
    Ok(StatusCode::ACCEPTED)
}

/// Text without control characters (new lines kept), trimmed to `max` characters.
fn clean(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A link a person can open: http or https, no spaces, not too long.
fn web_link(url: &str) -> bool {
    url.len() <= URL_MAX
        && (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_are_cleaned_and_links_checked() {
        assert_eq!(clean("  a\u{7}b\nc ", 10), "ab\nc");
        assert_eq!(clean("abcdef", 3), "abc");
        assert!(web_link("https://hito.example/p/1"));
        assert!(!web_link("javascript:alert(1)"));
        assert!(!web_link("https://a b"));
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
    }
}
