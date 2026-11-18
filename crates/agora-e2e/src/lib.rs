//! Ágora's end-to-end encryption.
//!
//! Every channel and direct conversation is one MLS group (Messaging Layer Security, RFC 9420),
//! run by [OpenMLS](https://github.com/openmls/openmls). Each signed-in client is one MLS member,
//! a *device*, with its own signing key. Message bodies are encrypted for the devices in the
//! group at that moment, so the server stores and relays only ciphertext. Nothing in this crate
//! is home-made cryptography: it chooses a standard ciphersuite and keeps OpenMLS's state.
//!
//! This crate does no I/O. [`Device`] holds one device's keys, groups and the plaintext it has
//! already decrypted, and turns into bytes for the front end to store ([`Device::to_bytes`]).
//! [`inspect`] and [`inspect_group_info`] read the unencrypted header of MLS messages, so the
//! server can check that what it relays belongs to the right conversation and epoch without
//! being able to read it.
//!
//! The one key the server never has is a device's group secret. What the server does decide is
//! who is in a channel, so a dishonest server could add a device of its own. Every client shows
//! the devices in a conversation and their fingerprints, which is how people catch that.

use std::collections::{BTreeMap, BTreeSet};

use agora_core::{ChannelId, MessageId, UserId};
use base64::Engine as _;
use openmls::messages::group_info::VerifiableGroupInfo;
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::{MemoryStorage, RustCrypto};
use openmls_traits::OpenMlsProvider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// X25519 key agreement, AES-128-GCM, SHA-256 and Ed25519 signatures: MLS's mandatory suite.
pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// The version of [`Device::to_bytes`]'s format.
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("this device's saved keys are damaged or from another version")]
    Corrupt,
    #[error("this device has no keys for that conversation yet")]
    NoGroup,
    #[error("the conversation has a commit waiting for the server's answer")]
    Pending,
    #[error("an MLS message is malformed")]
    Malformed,
    #[error("the message belongs to another conversation")]
    WrongGroup,
    #[error("encryption failed: {0}")]
    Mls(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod call;

fn mls<E: std::fmt::Debug>(e: E) -> Error {
    Error::Mls(format!("{e:?}"))
}

/// The MLS group id of a conversation. The server checks it on everything it stores.
pub fn group_id(channel: ChannelId) -> Vec<u8> {
    format!("agora:channel:{}", channel.0).into_bytes()
}

/// What an MLS message says about itself without being decrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub group_id: Vec<u8>,
    pub epoch: u64,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Application,
    Commit,
    Proposal,
}

/// Read the plaintext header of an MLS handshake or application message.
pub fn inspect(bytes: &[u8]) -> Result<Header> {
    let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|_| Error::Malformed)?;
    let message = message
        .try_into_protocol_message()
        .map_err(|_| Error::Malformed)?;
    let kind = match message.content_type() {
        ContentType::Application => Kind::Application,
        ContentType::Commit => Kind::Commit,
        ContentType::Proposal => Kind::Proposal,
    };
    Ok(Header {
        group_id: message.group_id().as_slice().to_vec(),
        epoch: message.epoch().as_u64(),
        kind,
    })
}

/// Read the group id and epoch of a GroupInfo, without verifying its signature.
pub fn inspect_group_info(bytes: &[u8]) -> Result<(Vec<u8>, u64)> {
    let info = group_info(bytes)?;
    Ok((info.group_id().as_slice().to_vec(), info.epoch().as_u64()))
}

fn group_info(bytes: &[u8]) -> Result<VerifiableGroupInfo> {
    match MlsMessageIn::tls_deserialize_exact(bytes)
        .map_err(|_| Error::Malformed)?
        .extract()
    {
        MlsMessageBodyIn::GroupInfo(info) => Ok(info),
        _ => Err(Error::Malformed),
    }
}

/// The person and device a key package belongs to, after checking its signature.
pub fn inspect_key_package(bytes: &[u8]) -> Result<Leaf> {
    let kp = KeyPackageIn::tls_deserialize_exact(bytes).map_err(|_| Error::Malformed)?;
    let kp = kp
        .validate(&RustCrypto::default(), ProtocolVersion::Mls10)
        .map_err(|_| Error::Malformed)?;
    if kp.ciphersuite() != CIPHERSUITE {
        return Err(Error::Malformed);
    }
    let node = kp.leaf_node();
    Ok(Leaf {
        user: user_of(node.credential()).ok_or(Error::Malformed)?,
        public_key: node.signature_key().as_slice().to_vec(),
    })
}

/// A short, readable digest of a device's signing key, for people to compare.
pub fn fingerprint(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    let hex: Vec<String> = digest.iter().take(12).map(|b| format!("{b:02X}")).collect();
    hex.chunks(2)
        .map(|pair| pair.concat())
        .collect::<Vec<_>>()
        .join(" ")
}

fn identity(user: UserId) -> Vec<u8> {
    format!("agora:user:{}", user.0).into_bytes()
}

fn user_of(credential: &Credential) -> Option<UserId> {
    let basic = BasicCredential::try_from(credential.clone()).ok()?;
    let text = std::str::from_utf8(basic.identity()).ok()?;
    text.strip_prefix("agora:user:")?.parse().ok().map(UserId)
}

/// A member of a conversation's group: one device of one person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub user: UserId,
    /// The device's signing key, as the server lists it.
    pub public_key: Vec<u8>,
}

impl Leaf {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_key)
    }

    fn from_member(member: &Member) -> Option<Leaf> {
        Some(Leaf {
            user: user_of(&member.credential)?,
            public_key: member.signature_key.clone(),
        })
    }
}

/// One entry of a conversation's encrypted log, as the server hands it out.
#[derive(Clone, Debug)]
pub struct Entry {
    pub seq: i64,
    pub author: UserId,
    /// The message this entry creates or edits, for application messages.
    pub message_id: Option<MessageId>,
    /// Empty when the message was deleted.
    pub data: Vec<u8>,
}

/// What processing an [`Entry`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A message body, now readable through [`Device::plaintext`].
    Message { message_id: MessageId, body: String },
    /// The group changed: devices joined or left.
    Changed { joined: Vec<Leaf>, left: Vec<Leaf> },
    /// This device was removed from the conversation; it must join again to read on.
    Removed,
    /// Nothing to do: this device's own entry, or a deleted message.
    Skipped,
    /// The entry could not be decrypted or did not match its metadata.
    Unreadable,
}

/// The contents of an application message.
#[derive(Serialize, Deserialize)]
struct Payload {
    v: u32,
    body: String,
    /// For an edit, the message it replaces, so an old edit cannot be replayed onto another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edits: Option<i64>,
}

/// A handshake waiting for the server: send `commit` and `group_info`, then call
/// [`Device::confirm`] with the entry's sequence number, or [`Device::abort`] if refused.
#[derive(Clone, Debug)]
pub struct Pending {
    pub commit: Vec<u8>,
    pub group_info: Vec<u8>,
    pub epoch: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct ChannelState {
    /// The last log entry this device has processed.
    last_seq: i64,
    /// Entries this device wrote itself; MLS cannot decrypt its own messages.
    own: BTreeSet<i64>,
    /// A commit sent but not yet answered.
    #[serde(default)]
    pending: bool,
    /// An external join sent but not yet answered.
    #[serde(default)]
    joining: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    channels: BTreeMap<i64, ChannelState>,
    /// What this device has decrypted, by message id. MLS deletes each message key once it is
    /// used, so this is the only copy a device will ever have.
    plaintext: BTreeMap<i64, String>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    user: i64,
    public_key: String,
    storage: Vec<(String, String)>,
    state: State,
}

#[derive(Default)]
struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &MemoryStorage {
        &self.storage
    }
    fn crypto(&self) -> &RustCrypto {
        &self.crypto
    }
    fn rand(&self) -> &RustCrypto {
        &self.crypto
    }
}

/// One signed-in client's keys, groups and decrypted messages.
pub struct Device {
    user: UserId,
    provider: Provider,
    signer: SignatureKeyPair,
    state: State,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("user", &self.user)
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

impl Device {
    /// A new device for `user`, with a fresh signing key.
    pub fn new(user: UserId) -> Result<Device> {
        let provider = Provider::default();
        let signer = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm()).map_err(mls)?;
        signer.store(provider.storage()).map_err(mls)?;
        Ok(Device {
            user,
            provider,
            signer,
            state: State::default(),
        })
    }

    /// Everything this device knows, to store on disk (owner-only: it holds private keys and
    /// decrypted messages).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let values = self
            .provider
            .storage
            .values
            .read()
            .map_err(|_| Error::Corrupt)?;
        let storage = values
            .iter()
            .map(|(k, v)| (b64().encode(k), b64().encode(v)))
            .collect();
        drop(values);
        let saved = SavedRef {
            version: FORMAT_VERSION,
            user: self.user.0,
            public_key: b64().encode(self.signer.public()),
            storage,
            state: &self.state,
        };
        serde_json::to_vec(&saved).map_err(|_| Error::Corrupt)
    }

    /// A device saved with [`Device::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Device> {
        let saved: Saved = serde_json::from_slice(bytes).map_err(|_| Error::Corrupt)?;
        if saved.version != FORMAT_VERSION {
            return Err(Error::Corrupt);
        }
        let provider = Provider::default();
        {
            let mut values = provider
                .storage
                .values
                .write()
                .map_err(|_| Error::Corrupt)?;
            for (k, v) in saved.storage {
                let k = b64().decode(k).map_err(|_| Error::Corrupt)?;
                let v = b64().decode(v).map_err(|_| Error::Corrupt)?;
                values.insert(k, v);
            }
        }
        let public = b64().decode(saved.public_key).map_err(|_| Error::Corrupt)?;
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public,
            CIPHERSUITE.signature_algorithm(),
        )
        .ok_or(Error::Corrupt)?;
        Ok(Device {
            user: UserId(saved.user),
            provider,
            signer,
            state: saved.state,
        })
    }

    pub fn user(&self) -> UserId {
        self.user
    }

    /// This device's signing key, which the server lists beside the session.
    pub fn public_key(&self) -> Vec<u8> {
        self.signer.public().to_vec()
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(self.signer.public())
    }

    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(identity(self.user)).into(),
            signature_key: self.signer.public().into(),
        }
    }

    fn join_config() -> MlsGroupJoinConfig {
        MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            // Keys of past epochs are dropped at once: a removed device's old keys open nothing.
            .max_past_epochs(0)
            .build()
    }

    fn load(&self, channel: ChannelId) -> Result<Option<MlsGroup>> {
        MlsGroup::load(
            self.provider.storage(),
            &GroupId::from_slice(&group_id(channel)),
        )
        .map_err(mls)
    }

    fn group(&self, channel: ChannelId) -> Result<MlsGroup> {
        self.load(channel)?.ok_or(Error::NoGroup)
    }

    fn channel(&mut self, channel: ChannelId) -> &mut ChannelState {
        self.state.channels.entry(channel.0).or_default()
    }

    /// Whether this device can read and write `channel` now.
    pub fn is_member(&self, channel: ChannelId) -> bool {
        matches!(self.load(channel), Ok(Some(g)) if g.is_active())
    }

    /// The group's epoch, if this device is in it.
    pub fn epoch(&self, channel: ChannelId) -> Option<u64> {
        match self.load(channel) {
            Ok(Some(g)) if g.is_active() => Some(g.epoch().as_u64()),
            _ => None,
        }
    }

    /// The key that seals this conversation's call at the group's current epoch: an MLS
    /// exported secret (RFC 9420 §8.5), so only the conversation's devices can derive it and a
    /// device removed from it cannot follow the next epoch's calls.
    pub fn call_key(&self, channel: ChannelId) -> Result<call::CallKey> {
        let group = self.group(channel)?;
        if !group.is_active() {
            return Err(Error::NoGroup);
        }
        let secret = group
            .export_secret(
                self.provider.crypto(),
                call::EXPORTER_LABEL,
                &channel.0.to_be_bytes(),
                32,
            )
            .map_err(mls)?;
        let key: [u8; 32] = secret.try_into().map_err(|_| Error::Malformed)?;
        Ok(call::CallKey {
            epoch: group.epoch().as_u64(),
            key,
        })
    }

    /// The last log entry processed for `channel`.
    pub fn last_seq(&self, channel: ChannelId) -> i64 {
        self.state
            .channels
            .get(&channel.0)
            .map_or(0, |c| c.last_seq)
    }

    /// The devices in `channel`'s group.
    pub fn members(&self, channel: ChannelId) -> Vec<Leaf> {
        match self.load(channel) {
            Ok(Some(g)) if g.is_active() => {
                g.members().filter_map(|m| Leaf::from_member(&m)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Start `channel`'s group with this device alone. Returns its GroupInfo for the server;
    /// call [`Device::abort`] if the server already has a group.
    pub fn create(&mut self, channel: ChannelId, from_seq: i64) -> Result<Vec<u8>> {
        self.forget(channel)?;
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .max_past_epochs(0)
            .build();
        let group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(&group_id(channel)),
            self.credential(),
        )
        .map_err(mls)?;
        let info = group
            .export_group_info(self.provider.crypto(), &self.signer, true)
            .map_err(mls)?;
        let state = self.channel(channel);
        state.last_seq = from_seq;
        state.joining = true;
        info.tls_serialize_detached().map_err(mls)
    }

    /// Join `channel`'s group from the server's latest GroupInfo, by an external commit.
    /// Messages sent before this device joined stay unreadable to it.
    pub fn join(
        &mut self,
        channel: ChannelId,
        group_info_bytes: &[u8],
        from_seq: i64,
    ) -> Result<Pending> {
        let info = group_info(group_info_bytes)?;
        if info.group_id().as_slice() != group_id(channel).as_slice() {
            return Err(Error::WrongGroup);
        }
        self.forget(channel)?;
        let (group, bundle) = MlsGroup::external_commit_builder()
            .with_config(Self::join_config())
            .build_group(&self.provider, info, self.credential())
            .map_err(mls)?
            .load_psks(self.provider.storage())
            .map_err(mls)?
            .build(
                self.provider.rand(),
                self.provider.crypto(),
                &self.signer,
                |_| true,
            )
            .map_err(mls)?
            .finalize(&self.provider)
            .map_err(mls)?;
        let (commit, _, info) = bundle.into_contents();
        let info = info.ok_or(Error::Mls("no GroupInfo after joining".into()))?;
        let pending = Pending {
            commit: commit.tls_serialize_detached().map_err(mls)?,
            group_info: MlsMessageOut::from(info)
                .tls_serialize_detached()
                .map_err(mls)?,
            epoch: group.epoch().as_u64().saturating_sub(1),
        };
        let state = self.channel(channel);
        state.last_seq = from_seq;
        state.joining = true;
        state.pending = false;
        Ok(pending)
    }

    /// `count` fresh key packages: what others need to add this device to a conversation while it
    /// is offline. Each is used once; the private halves stay in this device's storage.
    pub fn key_packages(&mut self, count: usize) -> Result<Vec<Vec<u8>>> {
        (0..count)
            .map(|_| {
                let bundle = KeyPackage::builder()
                    .build(CIPHERSUITE, &self.provider, &self.signer, self.credential())
                    .map_err(mls)?;
                bundle.key_package().tls_serialize_detached().map_err(mls)
            })
            .collect()
    }

    /// Add other people's devices from their key packages. Returns the commit for the server
    /// and the Welcome that lets the new devices in; `None` if no key package was usable.
    /// Each key package must belong to the user and signing key the server listed it under.
    pub fn add(
        &mut self,
        channel: ChannelId,
        key_packages: &[(Leaf, Vec<u8>)],
    ) -> Result<Option<(Pending, Vec<u8>)>> {
        let mut group = self.group(channel)?;
        if group.pending_commit().is_some() {
            return Err(Error::Pending);
        }
        let present: Vec<Leaf> = group
            .members()
            .filter_map(|m| Leaf::from_member(&m))
            .collect();
        let mut valid: Vec<KeyPackage> = Vec::new();
        for (leaf, bytes) in key_packages {
            let Ok(kp) = KeyPackageIn::tls_deserialize_exact(bytes.as_slice()) else {
                continue;
            };
            let Ok(kp) = kp.validate(self.provider.crypto(), ProtocolVersion::Mls10) else {
                continue;
            };
            let node = kp.leaf_node();
            let claimed = Leaf {
                user: match user_of(node.credential()) {
                    Some(u) => u,
                    None => continue,
                },
                public_key: node.signature_key().as_slice().to_vec(),
            };
            if claimed != *leaf || present.contains(&claimed) || kp.ciphersuite() != CIPHERSUITE {
                continue;
            }
            if valid
                .iter()
                .any(|v| v.leaf_node().signature_key() == node.signature_key())
            {
                continue;
            }
            valid.push(kp);
        }
        if valid.is_empty() {
            return Ok(None);
        }
        let epoch = group.epoch().as_u64();
        let (commit, welcome, info) = group
            .add_members(&self.provider, &self.signer, &valid)
            .map_err(mls)?;
        let info = info.ok_or(Error::Mls("no GroupInfo after adding".into()))?;
        self.channel(channel).pending = true;
        Ok(Some((
            Pending {
                commit: commit.tls_serialize_detached().map_err(mls)?,
                group_info: MlsMessageOut::from(info)
                    .tls_serialize_detached()
                    .map_err(mls)?,
                epoch,
            },
            welcome.tls_serialize_detached().map_err(mls)?,
        )))
    }

    /// Enter `channel`'s group through a Welcome that another member sent with the commit at
    /// log entry `seq`. Everything after that entry is readable.
    pub fn welcome(&mut self, channel: ChannelId, bytes: &[u8], seq: i64) -> Result<()> {
        let MlsMessageBodyIn::Welcome(welcome) = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|_| Error::Malformed)?
            .extract()
        else {
            return Err(Error::Malformed);
        };
        self.forget(channel)?;
        let group =
            StagedWelcome::new_from_welcome(&self.provider, &Self::join_config(), welcome, None)
                .map_err(mls)?
                .into_group(&self.provider)
                .map_err(mls)?;
        if group.group_id().as_slice() != group_id(channel).as_slice() {
            let mut group = group;
            group.delete(self.provider.storage()).map_err(mls)?;
            return Err(Error::WrongGroup);
        }
        self.channel(channel).last_seq = seq;
        Ok(())
    }

    /// Remove every device for which `keep` answers false (people who left the channel, devices
    /// that signed out). `None` when there is nobody to remove.
    pub fn remove(
        &mut self,
        channel: ChannelId,
        keep: impl Fn(&Leaf) -> bool,
    ) -> Result<Option<Pending>> {
        let mut group = self.group(channel)?;
        if group.pending_commit().is_some() {
            return Err(Error::Pending);
        }
        let own = group.own_leaf_index();
        let gone: Vec<LeafNodeIndex> = group
            .members()
            .filter(|m| m.index != own)
            .filter(|m| Leaf::from_member(m).is_none_or(|leaf| !keep(&leaf)))
            .map(|m| m.index)
            .collect();
        if gone.is_empty() {
            return Ok(None);
        }
        let epoch = group.epoch().as_u64();
        let (commit, _, info) = group
            .remove_members(&self.provider, &self.signer, &gone)
            .map_err(mls)?;
        let info = info.ok_or(Error::Mls("no GroupInfo after removing".into()))?;
        self.channel(channel).pending = true;
        Ok(Some(Pending {
            commit: commit.tls_serialize_detached().map_err(mls)?,
            group_info: MlsMessageOut::from(info)
                .tls_serialize_detached()
                .map_err(mls)?,
            epoch,
        }))
    }

    /// The server accepted this device's commit (or group) as entry `seq`.
    pub fn confirm(&mut self, channel: ChannelId, seq: Option<i64>) -> Result<()> {
        let mut group = self.group(channel)?;
        if group.pending_commit().is_some() {
            group.merge_pending_commit(&self.provider).map_err(mls)?;
        }
        let state = self.channel(channel);
        state.pending = false;
        state.joining = false;
        if let Some(seq) = seq {
            state.own.insert(seq);
        }
        Ok(())
    }

    /// The server refused this device's commit: drop it (and, for a join, the whole group).
    pub fn abort(&mut self, channel: ChannelId) -> Result<()> {
        let joining = self.channel(channel).joining;
        if joining {
            return self.forget(channel);
        }
        if let Some(mut group) = self.load(channel)? {
            group
                .clear_pending_commit(self.provider.storage())
                .map_err(mls)?;
        }
        self.channel(channel).pending = false;
        Ok(())
    }

    /// Drop `channel`'s group and position (not the messages already decrypted).
    pub fn forget(&mut self, channel: ChannelId) -> Result<()> {
        if let Some(mut group) = self.load(channel)? {
            group.delete(self.provider.storage()).map_err(mls)?;
        }
        self.state.channels.remove(&channel.0);
        Ok(())
    }

    /// Encrypt a message body (or, with `edits`, a new body for that message).
    pub fn encrypt(
        &mut self,
        channel: ChannelId,
        body: &str,
        edits: Option<MessageId>,
    ) -> Result<(Vec<u8>, u64)> {
        let mut group = self.group(channel)?;
        if group.pending_commit().is_some() {
            return Err(Error::Pending);
        }
        let payload = serde_json::to_vec(&Payload {
            v: 1,
            body: body.to_owned(),
            edits: edits.map(|m| m.0),
        })
        .map_err(|_| Error::Malformed)?;
        let message = group
            .create_message(&self.provider, &self.signer, &payload)
            .map_err(mls)?;
        Ok((
            message.tls_serialize_detached().map_err(mls)?,
            group.epoch().as_u64(),
        ))
    }

    /// Record a message this device sent: its plaintext and its log entry.
    pub fn sent(
        &mut self,
        channel: ChannelId,
        message_id: MessageId,
        seq: Option<i64>,
        body: &str,
    ) {
        self.state.plaintext.insert(message_id.0, body.to_owned());
        if let Some(seq) = seq {
            self.channel(channel).own.insert(seq);
        }
    }

    /// Process the next entry of `channel`'s log. Entries must come in order.
    pub fn process(&mut self, channel: ChannelId, entry: &Entry) -> Result<Outcome> {
        let state = self.channel(channel);
        if entry.seq <= state.last_seq {
            return Ok(Outcome::Skipped);
        }
        state.last_seq = entry.seq;
        if state.own.remove(&entry.seq) || entry.data.is_empty() {
            return Ok(Outcome::Skipped);
        }
        let Some(mut group) = self.load(channel)? else {
            return Ok(Outcome::Unreadable);
        };
        if !group.is_active() {
            return Ok(Outcome::Unreadable);
        }
        let Ok(message) = MlsMessageIn::tls_deserialize_exact(entry.data.as_slice()) else {
            return Ok(Outcome::Unreadable);
        };
        let Ok(message) = message.try_into_protocol_message() else {
            return Ok(Outcome::Unreadable);
        };
        if message.group_id().as_slice() != group_id(channel).as_slice() {
            return Ok(Outcome::Unreadable);
        }
        let before: Vec<Member> = group.members().collect();
        let processed = match group.process_message(&self.provider, message) {
            Ok(p) => p,
            // A message from an epoch this device never had keys for, or tampered with.
            Err(_) => return Ok(Outcome::Unreadable),
        };
        let sender = user_of(processed.credential());
        let new_member = matches!(processed.sender(), Sender::NewMemberCommit);
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => {
                let (Some(message_id), Some(sender)) = (entry.message_id, sender) else {
                    return Ok(Outcome::Unreadable);
                };
                if sender != entry.author {
                    return Ok(Outcome::Unreadable);
                }
                let Ok(payload) = serde_json::from_slice::<Payload>(&app.into_bytes()) else {
                    return Ok(Outcome::Unreadable);
                };
                if payload.edits.is_some_and(|m| m != message_id.0) {
                    return Ok(Outcome::Unreadable);
                }
                // A sender's client is not trusted to have checked the text either.
                let Ok(body) = agora_core::validate::message_body(&payload.body) else {
                    return Ok(Outcome::Unreadable);
                };
                let payload = Payload { body, ..payload };
                self.state
                    .plaintext
                    .insert(message_id.0, payload.body.clone());
                Ok(Outcome::Message {
                    message_id,
                    body: payload.body,
                })
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                if staged.self_removed() {
                    group
                        .merge_staged_commit(&self.provider, *staged)
                        .map_err(mls)?;
                    group.delete(self.provider.storage()).map_err(mls)?;
                    return Ok(Outcome::Removed);
                }
                let left: Vec<Leaf> = staged
                    .remove_proposals()
                    .filter_map(|p| {
                        let index = p.remove_proposal().removed();
                        before.iter().find(|m| m.index == index)
                    })
                    .filter_map(Leaf::from_member)
                    .collect();
                let mut joined: Vec<Leaf> = staged
                    .add_proposals()
                    .filter_map(|p| {
                        let leaf = p.add_proposal().key_package().leaf_node();
                        Some(Leaf {
                            user: user_of(leaf.credential())?,
                            public_key: leaf.signature_key().as_slice().to_vec(),
                        })
                    })
                    .collect();
                if new_member {
                    if let Some(leaf) = staged.update_path_leaf_node() {
                        if let Some(user) = user_of(leaf.credential()) {
                            joined.push(Leaf {
                                user,
                                public_key: leaf.signature_key().as_slice().to_vec(),
                            });
                        }
                    }
                }
                group
                    .merge_staged_commit(&self.provider, *staged)
                    .map_err(mls)?;
                Ok(Outcome::Changed { joined, left })
            }
            // Proposals are not used by Ágora's clients; ignore stray ones.
            _ => Ok(Outcome::Skipped),
        }
    }

    /// Drop a deleted message's text.
    pub fn forget_message(&mut self, message: MessageId) {
        self.state.plaintext.remove(&message.0);
    }

    /// The body of a message this device has read or sent.
    pub fn plaintext(&self, message: MessageId) -> Option<&str> {
        self.state.plaintext.get(&message.0).map(String::as_str)
    }

    /// Messages this device has decrypted whose body contains `query` (case-insensitive),
    /// newest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<MessageId> {
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.state
            .plaintext
            .iter()
            .rev()
            .filter(|(_, body)| body.to_lowercase().contains(&needle))
            .take(limit)
            .map(|(id, _)| MessageId(*id))
            .collect()
    }
}

#[derive(Serialize)]
struct SavedRef<'a> {
    version: u32,
    user: i64,
    public_key: String,
    storage: Vec<(String, String)>,
    state: &'a State,
}

#[cfg(test)]
mod tests;
