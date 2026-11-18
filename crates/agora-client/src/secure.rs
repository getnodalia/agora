//! End-to-end encrypted conversations on top of [`Client`].
//!
//! [`Secure`] owns this device's [`agora_e2e::Device`] and does the work every front end needs:
//! it registers the device with the session, keeps each conversation's MLS group in step with the
//! server's log ([`Secure::sync`]), encrypts what is sent and edited, fills decrypted text into
//! the [`Message`]s the server returns (whose `body` is empty), and searches the text this
//! device has read. All MLS work for one device runs under one lock, so a sync never interleaves
//! with a send, and the device file is rewritten (owner-only, atomically) after each change.
//!
//! When the server answers `stale_epoch` (someone else's commit got in first), the operation
//! catches up and tries again.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agora_core::{ChannelId, MessageId, UserId, WorkspaceId};
use agora_e2e::{Device, Entry, Leaf, Outcome};
use agora_protocol::{
    CommitRequest, EditMessageRequest, ErrorCode, Message, MlsState, PostMessageRequest,
    SearchResults,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use tokio::sync::Mutex;

use crate::{Client, ClientError, Result};

/// Attempts at an operation that keeps losing the race to other devices' commits.
const ATTEMPTS: usize = 6;
/// Publish more key packages when fewer than this are left on the server...
const KEY_PACKAGES_LOW: u32 = 20;
/// ...this many at a time.
const KEY_PACKAGES_BATCH: usize = 50;

fn crypto(e: agora_e2e::Error) -> ClientError {
    ClientError::Encryption(e.to_string())
}

/// Check a body the way the server used to, before it is encrypted and out of the server's sight.
fn checked(body: &str) -> Result<String> {
    agora_core::validate::message_body(body).map_err(|e| ClientError::Api {
        status: 400,
        error: agora_protocol::ApiError {
            code: ErrorCode::Invalid,
            message: e.to_string(),
            field: Some(e.field().to_owned()),
        },
    })
}

fn stale(e: &ClientError) -> bool {
    matches!(e.code(), Some(ErrorCode::StaleEpoch | ErrorCode::Conflict))
}

/// Something a sync learned about a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A message's text was decrypted (new or edited).
    Text { message_id: MessageId, body: String },
    /// Devices joined or left the conversation.
    Devices { joined: Vec<Leaf>, left: Vec<Leaf> },
}

struct Inner {
    device: Device,
    path: Option<PathBuf>,
}

impl Inner {
    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = self.device.to_bytes().map_err(crypto)?;
        write_private(path, &bytes).map_err(|e| ClientError::Encryption(e.to_string()))
    }
}

/// Write `bytes` to `path` readable by its owner only, replacing it atomically. The folder is
/// created when missing: on a first launch nothing has made the settings folder yet.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Keep enough key packages on the server for others to add this device while it is away.
async fn replenish(client: &Client, inner: &mut Inner, left: u32) -> Result<()> {
    if left >= KEY_PACKAGES_LOW {
        return Ok(());
    }
    let packages = inner
        .device
        .key_packages(KEY_PACKAGES_BATCH)
        .map_err(crypto)?;
    // Save the private halves before the public ones are out.
    inner.save()?;
    client
        .publish_key_packages(packages.iter().map(|p| STANDARD.encode(p)).collect())
        .await?;
    Ok(())
}

/// This device's end-to-end encryption, for one signed-in session.
#[derive(Clone)]
pub struct Secure {
    client: Client,
    inner: Arc<Mutex<Inner>>,
    fingerprint: String,
    public_key: String,
    user: UserId,
}

impl std::fmt::Debug for Secure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secure")
            .field("user", &self.user)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl Secure {
    /// Load this device from `path` (or create it), and register it with `client`'s session.
    /// Without a path the device lives in memory only, and a restart makes a new one.
    pub async fn open(client: &Client, user: UserId, path: Option<PathBuf>) -> Result<Secure> {
        let saved = path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| Device::from_bytes(&bytes).ok())
            .filter(|d| d.user() == user);
        let device = match saved {
            Some(d) => d,
            None => Device::new(user).map_err(crypto)?,
        };
        let fingerprint = device.fingerprint();
        let public_key = STANDARD.encode(device.public_key());
        let mut inner = Inner { device, path };
        let status = client
            .register_device(&STANDARD.encode(inner.device.public_key()))
            .await?;
        replenish(client, &mut inner, status.key_packages).await?;
        inner.save()?;
        Ok(Secure {
            client: client.clone(),
            inner: Arc::new(Mutex::new(inner)),
            fingerprint,
            public_key,
            user,
        })
    }

    /// This device's signing key, base64, as the server lists it.
    pub fn public_key_base64(&self) -> String {
        self.public_key.clone()
    }

    /// Delete a device file: its keys and the messages it decrypted. Logging out keeps it, so
    /// signing in again on the same computer is the same device; this is for "forget this
    /// computer".
    pub fn wipe(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// This device's fingerprint, for people to compare.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The devices in a conversation's group, as this device sees them.
    pub async fn members(&self, channel: ChannelId) -> Vec<Leaf> {
        self.inner.lock().await.device.members(channel)
    }

    /// The key that seals `channel`'s call now, after catching up with the conversation so it
    /// matches the other devices'. Call again when the conversation's devices change.
    pub async fn call_key(&self, channel: ChannelId) -> Result<agora_e2e::call::CallKey> {
        let mut inner = self.inner.lock().await;
        self.sync_locked(&mut inner, channel).await?;
        inner.save()?;
        inner.device.call_key(channel).map_err(crypto)
    }

    /// Bring this device up to date with a conversation: join its group if needed and decrypt
    /// every new log entry, in order.
    pub async fn sync(&self, channel: ChannelId) -> Result<Vec<Change>> {
        let mut inner = self.inner.lock().await;
        let (changes, _) = self.sync_locked(&mut inner, channel).await?;
        inner.save()?;
        Ok(changes)
    }

    async fn sync_locked(
        &self,
        inner: &mut Inner,
        channel: ChannelId,
    ) -> Result<(Vec<Change>, MlsState)> {
        let mut changes = Vec::new();
        for _ in 0..ATTEMPTS {
            if !inner.device.is_member(channel) {
                self.enter_locked(inner, channel).await?;
            }
            loop {
                let after = inner.device.last_seq(channel);
                let state = self.client.mls_state(channel, Some(after)).await?;
                let mut removed = false;
                for e in &state.entries {
                    let entry = Entry {
                        seq: e.seq,
                        author: e.author_id,
                        message_id: e.message_id,
                        data: STANDARD.decode(&e.data).unwrap_or_default(),
                    };
                    match inner.device.process(channel, &entry).map_err(crypto)? {
                        Outcome::Message { message_id, body } => {
                            changes.push(Change::Text { message_id, body });
                        }
                        Outcome::Changed { joined, left } => {
                            changes.push(Change::Devices { joined, left });
                        }
                        Outcome::Removed => {
                            removed = true;
                            break;
                        }
                        Outcome::Skipped | Outcome::Unreadable => {}
                    }
                }
                if removed {
                    // Removed from the group (left and came back, or a stale device): the
                    // server still lets us read, so join again from here on.
                    inner.device.forget(channel).map_err(crypto)?;
                    break;
                }
                if !state.has_more {
                    return Ok((changes, state));
                }
            }
        }
        Err(ClientError::Encryption(
            "the conversation keeps changing; try again".into(),
        ))
    }

    /// Enter `channel`'s group: through a Welcome someone left for this device (it then reads
    /// everything since it was added), or else by joining on its own.
    async fn enter_locked(&self, inner: &mut Inner, channel: ChannelId) -> Result<()> {
        let state = self.client.mls_state(channel, None).await?;
        if let Some(welcome) = &state.welcome {
            if let Ok(data) = STANDARD.decode(&welcome.data) {
                if inner.device.welcome(channel, &data, welcome.seq).is_ok() {
                    // That used one of this device's key packages.
                    let status = self
                        .client
                        .register_device(&STANDARD.encode(inner.device.public_key()))
                        .await?;
                    return replenish(&self.client, inner, status.key_packages).await;
                }
            }
        }
        self.join_locked(inner, channel).await
    }

    /// Start or join `channel`'s group.
    async fn join_locked(&self, inner: &mut Inner, channel: ChannelId) -> Result<()> {
        for _ in 0..ATTEMPTS {
            let state = self.client.mls_state(channel, None).await?;
            match &state.group_info {
                None => {
                    let info = inner
                        .device
                        .create(channel, state.last_seq)
                        .map_err(crypto)?;
                    match self
                        .client
                        .mls_create_group(channel, &STANDARD.encode(info))
                        .await
                    {
                        Ok(_) => {
                            inner.device.confirm(channel, None).map_err(crypto)?;
                            return Ok(());
                        }
                        Err(e) if stale(&e) => inner.device.abort(channel).map_err(crypto)?,
                        Err(e) => {
                            inner.device.abort(channel).map_err(crypto)?;
                            return Err(e);
                        }
                    }
                }
                Some(info) => {
                    let info = STANDARD
                        .decode(info)
                        .map_err(|e| ClientError::Decode(e.to_string()))?;
                    let pending = inner
                        .device
                        .join(channel, &info, state.last_seq)
                        .map_err(crypto)?;
                    match self.commit(channel, &pending).await {
                        Ok(seq) => {
                            inner.device.confirm(channel, Some(seq)).map_err(crypto)?;
                            return Ok(());
                        }
                        Err(e) if stale(&e) => inner.device.abort(channel).map_err(crypto)?,
                        Err(e) => {
                            inner.device.abort(channel).map_err(crypto)?;
                            return Err(e);
                        }
                    }
                }
            }
        }
        Err(ClientError::Encryption(
            "could not join the conversation's encryption; try again".into(),
        ))
    }

    async fn commit(&self, channel: ChannelId, pending: &agora_e2e::Pending) -> Result<i64> {
        let res = self
            .client
            .mls_commit(
                channel,
                &CommitRequest {
                    commit: STANDARD.encode(&pending.commit),
                    group_info: STANDARD.encode(&pending.group_info),
                    welcome: None,
                    welcome_to: Vec::new(),
                },
            )
            .await?;
        Ok(res.seq)
    }

    /// Catch up, remove devices that may no longer read the conversation and add the ones that
    /// may but are not in it yet. After this the group holds exactly the devices the server
    /// lists for people who can read it (less any that have no key package left; they join on
    /// their own when they next come online).
    async fn prepare_locked(&self, inner: &mut Inner, channel: ChannelId) -> Result<()> {
        for _ in 0..ATTEMPTS {
            if self.remove_locked(inner, channel).await? && self.add_locked(inner, channel).await? {
                return Ok(());
            }
        }
        Err(ClientError::Encryption(
            "the conversation keeps changing; try again".into(),
        ))
    }

    /// Add listed devices missing from the group. `false` if another commit got in first.
    async fn add_locked(&self, inner: &mut Inner, channel: ChannelId) -> Result<bool> {
        let (_, state) = self.sync_locked(inner, channel).await?;
        let present: HashSet<Vec<u8>> = inner
            .device
            .members(channel)
            .into_iter()
            .map(|l| l.public_key)
            .collect();
        let missing: Vec<String> = state
            .devices
            .iter()
            .filter(|d| {
                STANDARD
                    .decode(&d.public_key)
                    .is_ok_and(|k| !present.contains(&k))
            })
            .map(|d| d.public_key.clone())
            .collect();
        if missing.is_empty() {
            return Ok(true);
        }
        let claimed = self.client.claim_key_packages(channel, missing).await?;
        let packages: Vec<(Leaf, Vec<u8>)> = claimed
            .iter()
            .filter_map(|c| {
                Some((
                    Leaf {
                        user: c.device.user_id,
                        public_key: STANDARD.decode(&c.device.public_key).ok()?,
                    },
                    STANDARD.decode(&c.key_package).ok()?,
                ))
            })
            .collect();
        let Some((pending, welcome)) = inner.device.add(channel, &packages).map_err(crypto)? else {
            return Ok(true);
        };
        let req = CommitRequest {
            commit: STANDARD.encode(&pending.commit),
            group_info: STANDARD.encode(&pending.group_info),
            welcome: Some(STANDARD.encode(welcome)),
            welcome_to: claimed
                .iter()
                .map(|c| c.device.public_key.clone())
                .collect(),
        };
        match self.client.mls_commit(channel, &req).await {
            Ok(res) => {
                inner
                    .device
                    .confirm(channel, Some(res.seq))
                    .map_err(crypto)?;
                Ok(true)
            }
            Err(e) if stale(&e) => {
                inner.device.abort(channel).map_err(crypto)?;
                Ok(false)
            }
            Err(e) => {
                inner.device.abort(channel).map_err(crypto)?;
                Err(e)
            }
        }
    }

    /// Remove devices that may no longer read. `false` if another commit got in first.
    async fn remove_locked(&self, inner: &mut Inner, channel: ChannelId) -> Result<bool> {
        {
            let (_, state) = self.sync_locked(inner, channel).await?;
            let allowed: HashSet<(UserId, Vec<u8>)> = state
                .devices
                .iter()
                .filter_map(|d| Some((d.user_id, STANDARD.decode(&d.public_key).ok()?)))
                .collect();
            let pending = inner
                .device
                .remove(channel, |leaf| {
                    allowed.contains(&(leaf.user, leaf.public_key.clone()))
                })
                .map_err(crypto)?;
            let Some(pending) = pending else {
                return Ok(true);
            };
            match self.commit(channel, &pending).await {
                Ok(seq) => {
                    inner.device.confirm(channel, Some(seq)).map_err(crypto)?;
                    Ok(true)
                }
                Err(e) if stale(&e) => {
                    inner.device.abort(channel).map_err(crypto)?;
                    Ok(false)
                }
                Err(e) => {
                    inner.device.abort(channel).map_err(crypto)?;
                    Err(e)
                }
            }
        }
    }

    /// Encrypt and send a message. `mentions` are the people it names (the server cannot read
    /// the text to find them).
    pub async fn post(
        &self,
        channel: ChannelId,
        body: &str,
        nonce: &str,
        thread_root_id: Option<MessageId>,
        mentions: Vec<UserId>,
    ) -> Result<Message> {
        let body = &checked(body)?;
        let mut inner = self.inner.lock().await;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            self.prepare_locked(&mut inner, channel).await?;
            let (data, _) = inner.device.encrypt(channel, body, None).map_err(crypto)?;
            let req = PostMessageRequest {
                body: String::new(),
                ciphertext: Some(STANDARD.encode(data)),
                mentions: mentions.clone(),
                client_nonce: nonce.to_owned(),
                thread_root_id,
            };
            match self.client.post_message(channel, &req).await {
                Ok(mut message) => {
                    inner
                        .device
                        .sent(channel, message.id, message.content_seq, body);
                    inner.save()?;
                    message.body = body.to_owned();
                    return Ok(message);
                }
                Err(e) if e.code() == Some(ErrorCode::StaleEpoch) => last = Some(e),
                Err(e) => {
                    inner.save()?;
                    return Err(e);
                }
            }
        }
        inner.save()?;
        Err(last.unwrap_or(ClientError::Encryption("could not send".into())))
    }

    /// Encrypt a file, upload it and send a message that carries its key, with an optional
    /// caption. The server holds the file's ciphertext and never its key.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_file(
        &self,
        channel: ChannelId,
        name: &str,
        mime: &str,
        data: &[u8],
        caption: &str,
        nonce: &str,
        thread_root_id: Option<MessageId>,
    ) -> Result<Message> {
        let (sealed, key) = crate::files::seal(data)?;
        let uploaded = self.client.upload_file(channel, sealed).await?;
        let attachment = agora_core::content::Attachment {
            file: uploaded.id,
            name: agora_core::content::safe_file_name(name),
            size: u64::try_from(data.len()).unwrap_or(u64::MAX),
            mime: mime.chars().take(100).collect(),
            key,
        };
        let body = agora_core::content::file_message(&attachment, caption);
        self.post(channel, &body, nonce, thread_root_id, Vec::new())
            .await
    }

    /// Download and decrypt a file a message carries.
    pub async fn open_file(&self, attachment: &agora_core::content::Attachment) -> Result<Vec<u8>> {
        let data = self.client.download_file(attachment.file).await?;
        crate::files::open(&data, &attachment.key)
    }

    /// Encrypt and send a new text for one of this person's messages.
    pub async fn edit(
        &self,
        message: &Message,
        body: &str,
        mentions: Vec<UserId>,
    ) -> Result<Message> {
        let body = &checked(body)?;
        let channel = message.channel_id;
        let mut inner = self.inner.lock().await;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            self.prepare_locked(&mut inner, channel).await?;
            let (data, _) = inner
                .device
                .encrypt(channel, body, Some(message.id))
                .map_err(crypto)?;
            let req = EditMessageRequest {
                body: String::new(),
                ciphertext: Some(STANDARD.encode(data)),
                mentions: mentions.clone(),
            };
            match self.client.edit_message(message.id, &req).await {
                Ok(mut edited) => {
                    inner
                        .device
                        .sent(channel, edited.id, edited.content_seq, body);
                    inner.save()?;
                    edited.body = body.to_owned();
                    return Ok(edited);
                }
                Err(e) if e.code() == Some(ErrorCode::StaleEpoch) => last = Some(e),
                Err(e) => {
                    inner.save()?;
                    return Err(e);
                }
            }
        }
        inner.save()?;
        Err(last.unwrap_or(ClientError::Encryption("could not edit".into())))
    }

    /// Put the decrypted text into messages from the server. An encrypted message this device
    /// cannot read (sent before it joined) keeps an empty body.
    pub async fn fill(&self, messages: &mut [Message]) {
        let inner = self.inner.lock().await;
        for m in messages.iter_mut().filter(|m| m.encrypted && !m.deleted) {
            m.body = inner.device.plaintext(m.id).unwrap_or_default().to_owned();
        }
    }

    pub async fn fill_one(&self, message: &mut Message) {
        self.fill(std::slice::from_mut(message)).await;
    }

    /// Forget a deleted message's text.
    pub async fn forget_message(&self, id: MessageId) {
        let mut inner = self.inner.lock().await;
        inner.device.forget_message(id);
        let _ = inner.save();
    }

    /// Search: the text this device has decrypted, plus the server's search of messages from
    /// before encryption. Newest first.
    pub async fn search(&self, ws: WorkspaceId, query: &str) -> Result<SearchResults> {
        let ids = self.inner.lock().await.device.search(query, 100);
        let mut found = if ids.is_empty() {
            Vec::new()
        } else {
            self.client.lookup_messages(ws, &ids).await?.messages
        };
        let legacy = self.client.search(ws, query).await?.messages;
        found.extend(legacy.into_iter().filter(|m| !m.encrypted));
        found.sort_by_key(|m| std::cmp::Reverse(m.id));
        found.dedup_by_key(|m| m.id);
        found.truncate(50);
        self.fill(&mut found).await;
        Ok(SearchResults { messages: found })
    }
}

#[cfg(test)]
mod tests {
    use super::write_private;

    #[test]
    fn the_device_file_is_written_into_a_folder_that_does_not_exist_yet() {
        let root = std::env::temp_dir().join(format!("agora-secure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("first-launch").join("device.json");
        write_private(&path, b"keys").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"keys");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
