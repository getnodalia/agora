//! Files shared in conversations. Clients encrypt each file before uploading it and put its key
//! in the end-to-end encrypted message, so the server stores and serves bytes it cannot read.

use agora_core::ChannelId;
use agora_protocol::UploadedFile;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::{channel_access, check_write};
use crate::auth::{now_ms, Auth};
use crate::config::FILE_HARD_MAX_BYTES;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The smallest ciphertext a client can produce: an AES-GCM nonce and tag around nothing.
const MIN_BYTES: usize = 12 + 16;

pub async fn upload(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    bytes: Bytes,
) -> AppResult<(StatusCode, Json<UploadedFile>)> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    if !access.can_post() {
        return Err(AppError::forbidden(if access.channel.archived {
            "this channel is archived"
        } else {
            "you cannot post here"
        }));
    }
    check_write(&state, auth.user.id)?;
    let max = state.config.limits.max_file_bytes.min(FILE_HARD_MAX_BYTES);
    if bytes.len() > max {
        let mut e = AppError::invalid(format!(
            "files can be at most {} MB on this server",
            max / (1024 * 1024)
        ));
        e.body.field = Some("file".into());
        return Err(e);
    }
    if bytes.len() < MIN_BYTES {
        return Err(AppError::invalid("this is not an encrypted file"));
    }
    let file = state
        .store
        .insert_file(access.channel.id, auth.user.id, &bytes, now_ms())
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(UploadedFile {
            id: file,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }),
    ))
}

pub async fn download(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let channel = state
        .store
        .file_channel(id)
        .await?
        .ok_or_else(AppError::not_found)?;
    channel_access(&state, channel, auth.user.id).await?;
    let data = state
        .store
        .file_data(id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        data,
    )
        .into_response())
}
