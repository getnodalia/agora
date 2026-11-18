//! End-to-end encryption: devices, each conversation's MLS group and its encrypted log.
//!
//! The server cannot decrypt anything here. What it does check is everything it can see without
//! keys: that the caller may read the conversation, that an MLS message belongs to this
//! conversation's group (by its group id) and to its current epoch, and that commits arrive one
//! epoch at a time. The order it assigns (`seq`) is the order every device processes.

use agora_core::{ChannelId, UserId};
use agora_e2e::Kind;
use agora_protocol::{
    ClaimKeyPackagesRequest, ClaimedKeyPackage, CommitRequest, CommitResponse, CreateGroupRequest,
    Device, DeviceStatus, MlsEntry, MlsState, MlsWelcome, PublishKeyPackagesRequest,
    RegisterDeviceRequest, ServerEvent,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Deserialize;

use super::{channel_access, check_write};
use crate::auth::{now_ms, Auth};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Log entries returned per request.
const LOG_PAGE_MAX: u32 = 500;
/// Ed25519 public keys are 32 bytes; leave room for other suites later.
const PUBLIC_KEY_MAX: usize = 256;
/// Unused key packages one device may keep on the server.
const KEY_PACKAGES_MAX: i64 = 200;
/// A key package is a few hundred bytes.
const KEY_PACKAGE_BYTES_MAX: usize = 4_096;
/// Devices one request may claim key packages for (and one Welcome may address).
const DEVICES_PER_REQUEST_MAX: usize = 500;

pub(crate) fn decode(field: &str, value: &str) -> AppResult<Vec<u8>> {
    let bytes = STANDARD.decode(value).map_err(|_| {
        let mut e = AppError::invalid("not valid base64");
        e.body.field = Some(field.to_owned());
        e
    })?;
    if bytes.is_empty() {
        let mut e = AppError::invalid("must not be empty");
        e.body.field = Some(field.to_owned());
        return Err(e);
    }
    Ok(bytes)
}

/// Check an MLS message is of `kind` and belongs to `channel`'s group; return its epoch.
pub(crate) fn check_mls(channel: ChannelId, bytes: &[u8], kind: Kind) -> AppResult<u64> {
    let header = agora_e2e::inspect(bytes).map_err(|_| AppError::invalid("not an MLS message"))?;
    if header.group_id != agora_e2e::group_id(channel) {
        return Err(AppError::invalid(
            "the MLS message belongs to another conversation",
        ));
    }
    if header.kind != kind {
        return Err(AppError::invalid("unexpected kind of MLS message"));
    }
    Ok(header.epoch)
}

fn check_group_info(channel: ChannelId, bytes: &[u8]) -> AppResult<u64> {
    let (group, epoch) = agora_e2e::inspect_group_info(bytes)
        .map_err(|_| AppError::invalid("not an MLS GroupInfo"))?;
    if group != agora_e2e::group_id(channel) {
        return Err(AppError::invalid(
            "the GroupInfo belongs to another conversation",
        ));
    }
    Ok(epoch)
}

async fn device_status(state: &AppState, key: &[u8]) -> AppResult<Json<DeviceStatus>> {
    let count = state.store.key_package_count(key).await?;
    Ok(Json(DeviceStatus {
        key_packages: u32::try_from(count).unwrap_or(0),
    }))
}

/// `PUT /devices/me`: the caller's session's MLS signing key.
pub async fn register_device(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<RegisterDeviceRequest>,
) -> AppResult<Json<DeviceStatus>> {
    let key = decode("public_key", &req.public_key)?;
    if key.len() > PUBLIC_KEY_MAX {
        return Err(AppError::invalid("public_key is too long"));
    }
    state
        .store
        .register_device(auth.session_id, auth.user.id, &key, now_ms())
        .await
        .map_err(|e| match e {
            agora_storage::StoreError::Conflict => {
                AppError::conflict("this key belongs to another account")
            }
            other => other.into(),
        })?;
    device_status(&state, &key).await
}

/// `GET /devices`: the caller's devices.
pub async fn own_devices(
    State(state): State<AppState>,
    auth: Auth,
) -> AppResult<Json<Vec<agora_protocol::OwnDevice>>> {
    let rows = state.store.own_devices(auth.user.id, now_ms()).await?;
    Ok(Json(
        rows.into_iter()
            .map(|d| agora_protocol::OwnDevice {
                public_key: STANDARD.encode(&d.public_key),
                created_at: d.created_at,
                last_seen_at: d.last_seen_at,
                this_device: d.session_id == Some(auth.session_id),
                signed_in: d.session_id.is_some(),
            })
            .collect(),
    ))
}

/// `POST /devices/remove`: remove one of the caller's devices and sign it out. The others take
/// it out of each conversation the next time they write there.
pub async fn remove_device(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<agora_protocol::RemoveDeviceRequest>,
) -> AppResult<StatusCode> {
    let key = decode("public_key", &req.public_key)?;
    match state.store.remove_device(auth.user.id, &key).await? {
        None => Err(AppError::not_found()),
        Some(session) => {
            if let Some(id) = session {
                state.hub.close_session(id);
            }
            tracing::info!(user = %auth.user.id, "device removed");
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

/// The caller's registered device, or an error telling the client to register it first.
async fn own_device(state: &AppState, auth: &Auth) -> AppResult<Vec<u8>> {
    state
        .store
        .session_device(auth.session_id)
        .await?
        .ok_or_else(|| AppError::invalid("register this session's device first"))
}

/// `POST /devices/me/key_packages`: publish key packages for the caller's device. Each must be
/// signed by that device for that account.
pub async fn publish_key_packages(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<PublishKeyPackagesRequest>,
) -> AppResult<Json<DeviceStatus>> {
    let key = own_device(&state, &auth).await?;
    let have = state.store.key_package_count(&key).await?;
    let room = usize::try_from(KEY_PACKAGES_MAX.saturating_sub(have)).unwrap_or(0);
    if req.key_packages.len() > room {
        return Err(AppError::invalid("too many key packages; publish fewer"));
    }
    let mut packages = Vec::with_capacity(req.key_packages.len());
    for kp in &req.key_packages {
        if kp.len() > KEY_PACKAGE_BYTES_MAX * 2 {
            return Err(AppError::invalid("a key package is too large"));
        }
        let bytes = decode("key_packages", kp)?;
        let leaf = agora_e2e::inspect_key_package(&bytes)
            .map_err(|_| AppError::invalid("not a valid key package"))?;
        if leaf.user != auth.user.id || leaf.public_key != key {
            return Err(AppError::invalid("a key package belongs to another device"));
        }
        packages.push(bytes);
    }
    state
        .store
        .add_key_packages(&key, &packages, now_ms())
        .await?;
    device_status(&state, &key).await
}

/// The devices of people who may read a conversation, as (user, key).
async fn allowed_devices(
    state: &AppState,
    channel: &agora_storage::ChannelRow,
) -> AppResult<Vec<(UserId, Vec<u8>)>> {
    let audience = state.store.channel_audience(channel).await?;
    Ok(state.store.devices(&audience, now_ms()).await?)
}

/// `POST /channels/{id}/mls/key_packages`: take one key package for each listed device of
/// someone who may read the conversation, to add those devices to its group.
pub async fn claim_key_packages(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<ClaimKeyPackagesRequest>,
) -> AppResult<Json<Vec<ClaimedKeyPackage>>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    check_write(&state, auth.user.id)?;
    if req.devices.len() > DEVICES_PER_REQUEST_MAX {
        return Err(AppError::invalid("too many devices"));
    }
    let allowed = allowed_devices(&state, &access.channel).await?;
    let mut out = Vec::new();
    for wanted in &req.devices {
        let key = decode("devices", wanted)?;
        let Some((user, _)) = allowed.iter().find(|(_, k)| *k == key) else {
            continue;
        };
        if let Some(kp) = state.store.claim_key_package(&key).await? {
            out.push(ClaimedKeyPackage {
                device: Device {
                    user_id: *user,
                    public_key: wanted.clone(),
                },
                key_package: STANDARD.encode(kp),
            });
        }
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    after: Option<i64>,
    limit: Option<u32>,
}

/// `GET /channels/{id}/mls`: the group's state, the devices that may be in it, and with
/// `after`, the log entries that follow.
pub async fn state(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Query(q): Query<LogQuery>,
) -> AppResult<Json<MlsState>> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    let channel = access.channel.id;
    let group = state.store.mls_group(channel).await?;
    let welcome = match state.store.session_device(auth.session_id).await? {
        Some(key) => state.store.welcome(channel, &key).await?,
        None => None,
    };
    let devices = allowed_devices(&state, &access.channel)
        .await?
        .into_iter()
        .map(|(user_id, key)| Device {
            user_id,
            public_key: STANDARD.encode(key),
        })
        .collect();
    let last_seq = state.store.mls_last_seq(channel).await?;
    let (entries, has_more) = match q.after {
        Some(after) => {
            let limit = q.limit.unwrap_or(LOG_PAGE_MAX).clamp(1, LOG_PAGE_MAX);
            state.store.mls_log(channel, after.max(0), limit).await?
        }
        None => (Vec::new(), false),
    };
    let entries = entries
        .into_iter()
        .map(|e| MlsEntry {
            seq: e.seq,
            epoch: e.epoch,
            kind: e.kind,
            author_id: e.author,
            message_id: e.message,
            data: STANDARD.encode(e.data),
        })
        .collect();
    Ok(Json(MlsState {
        channel_id: channel,
        epoch: group.as_ref().map(|g| g.0),
        group_info: group.map(|g| STANDARD.encode(g.1)),
        last_seq,
        devices,
        entries,
        has_more,
        welcome: welcome.map(|(seq, data)| MlsWelcome {
            seq,
            data: STANDARD.encode(data),
        }),
    }))
}

async fn announce(state: &AppState, channel: &agora_storage::ChannelRow) -> AppResult<()> {
    let audience: Vec<UserId> = state.store.channel_audience(channel).await?;
    state.hub.send(
        &audience,
        &ServerEvent::MlsChanged {
            channel_id: channel.id,
        },
    );
    Ok(())
}

/// `POST /channels/{id}/mls/group`: start the conversation's group. Only the first device to
/// ask gets to; the others join it.
pub async fn create_group(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CreateGroupRequest>,
) -> AppResult<(StatusCode, Json<CommitResponse>)> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    check_write(&state, auth.user.id)?;
    let info = decode("group_info", &req.group_info)?;
    let epoch = check_group_info(access.channel.id, &info)?;
    if epoch != 0 {
        return Err(AppError::invalid("a new group starts at epoch 0"));
    }
    state
        .store
        .create_mls_group(access.channel.id, epoch, &info, now_ms())
        .await
        .map_err(|e| match e {
            agora_storage::StoreError::Conflict => {
                AppError::conflict("this conversation already has a group; join it")
            }
            other => other.into(),
        })?;
    announce(&state, &access.channel).await?;
    let seq = state.store.mls_last_seq(access.channel.id).await?;
    Ok((StatusCode::CREATED, Json(CommitResponse { seq, epoch })))
}

/// `POST /channels/{id}/mls/commit`: a commit for the current epoch (a device joining, or
/// someone removing devices that may no longer read).
pub async fn commit(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CommitRequest>,
) -> AppResult<(StatusCode, Json<CommitResponse>)> {
    let access = channel_access(&state, ChannelId(id), auth.user.id).await?;
    check_write(&state, auth.user.id)?;
    let channel = access.channel.id;
    let commit = decode("commit", &req.commit)?;
    let info = decode("group_info", &req.group_info)?;
    let epoch = check_mls(channel, &commit, Kind::Commit)?;
    let next = check_group_info(channel, &info)?;
    if Some(next) != epoch.checked_add(1) {
        return Err(AppError::invalid(
            "the GroupInfo must be for the epoch after the commit",
        ));
    }
    let welcome = match &req.welcome {
        Some(w) => Some(decode("welcome", w)?),
        None => None,
    };
    if req.welcome_to.len() > DEVICES_PER_REQUEST_MAX {
        return Err(AppError::invalid("too many devices"));
    }
    let mut welcome_to = Vec::new();
    if welcome.is_some() {
        let allowed = allowed_devices(&state, &access.channel).await?;
        for d in &req.welcome_to {
            let key = decode("welcome_to", d)?;
            if !allowed.iter().any(|(_, k)| *k == key) {
                return Err(AppError::invalid(
                    "a Welcome is addressed to a device that cannot read here",
                ));
            }
            welcome_to.push(key);
        }
    }
    let seq = state
        .store
        .mls_commit(
            channel,
            epoch,
            &commit,
            &info,
            welcome.as_deref().map(|w| (w, welcome_to.as_slice())),
            auth.user.id,
            now_ms(),
        )
        .await?;
    announce(&state, &access.channel).await?;
    Ok((
        StatusCode::CREATED,
        Json(CommitResponse { seq, epoch: next }),
    ))
}
