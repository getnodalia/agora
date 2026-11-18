//! GitHub webhook inbox. External metadata is not a chat message and is not E2EE.
use super::{client_ip, workspace_role};
use crate::{
    auth::{now_ms, Auth},
    error::{AppError, AppResult},
    integrations::github::verify_signature,
    state::AppState,
};
use agora_core::WorkspaceId;
use agora_protocol::GitHubEvent;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;

#[derive(Deserialize)]
struct Payload {
    repository: Repository,
    action: Option<String>,
    number: Option<i64>,
    issue: Option<Issue>,
}
#[derive(Deserialize)]
struct Issue {
    number: Option<i64>,
}
#[derive(Deserialize)]
struct Repository {
    id: u64,
}

pub async fn receive(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<StatusCode> {
    let config = state
        .config
        .github_webhook
        .as_ref()
        .ok_or_else(AppError::not_found)?;
    let ip = client_ip(&state, peer, &headers);
    if !state.limiter.allow(
        &format!("github-webhook:{ip}"),
        state.config.limits.writes_per_minute,
    ) {
        return Err(AppError::rate_limited());
    }
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let signature = header("x-hub-signature-256").unwrap_or("");
    if !verify_signature(config.secret.as_bytes(), &body, signature) {
        return Err(AppError::unauthenticated());
    }
    let event = header("x-github-event").ok_or_else(|| AppError::invalid("missing event"))?;
    if !matches!(
        event,
        "issues"
            | "issue_comment"
            | "pull_request"
            | "pull_request_review"
            | "pull_request_review_comment"
            | "workflow_run"
            | "check_run"
            | "check_suite"
            | "release"
            | "push"
            | "create"
            | "delete"
            | "dependabot_alert"
            | "code_scanning_alert"
            | "secret_scanning_alert"
            | "ping"
    ) {
        return Err(AppError::invalid("unsupported event"));
    }
    let delivery =
        header("x-github-delivery").ok_or_else(|| AppError::invalid("missing delivery id"))?;
    if delivery.len() != 36
        || !delivery.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(AppError::invalid("invalid delivery id"));
    }
    let payload: Payload =
        serde_json::from_slice(&body).map_err(|_| AppError::invalid("invalid payload"))?;
    if payload.repository.id != config.repository {
        return Err(AppError::not_found());
    }
    if payload
        .action
        .as_ref()
        .is_some_and(|s| s.len() > 64 || !s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
    {
        return Err(AppError::invalid("invalid action"));
    }
    let hash: [u8; 32] = Sha256::digest(&body).into();
    let object_number = payload
        .number
        .or_else(|| payload.issue.and_then(|i| i.number))
        .filter(|n| *n > 0);
    state
        .store
        .record_github_event(agora_storage::github_events::NewGitHubEvent {
            workspace: WorkspaceId(config.workspace),
            delivery,
            hash: &hash,
            event,
            action: payload.action.as_deref(),
            object_number,
            now: now_ms(),
        })
        .await?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn events(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<GitHubEvent>>> {
    let ws = WorkspaceId(id);
    workspace_role(&state, ws, auth.user.id).await?;
    Ok(Json(state.store.github_events(ws).await?))
}
