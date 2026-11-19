//! Ágora's wire contract.
//!
//! The HTTP API lives under [`API_PREFIX`] and speaks JSON; the real-time channel is a WebSocket
//! at [`WS_PATH`] that carries [`ClientFrame`]s up and [`ServerEvent`]s down. Every type here is
//! what crosses the network, so changing a field is a protocol change: add fields with
//! `#[serde(default)]`, never rename or remove them within a version. `docs/protocol.md`
//! describes each endpoint.

use agora_core::{
    ChannelId, ChannelKind, InviteId, MessageId, OrganizationId, Role, UserId, WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// The API version this crate describes.
pub const API_VERSION: u32 = 1;
/// Every HTTP endpoint starts with this.
pub const API_PREFIX: &str = "/api/v1";
/// The WebSocket endpoint.
pub const WS_PATH: &str = "/api/v1/ws";
/// Seconds a WebSocket has, after opening, to send its [`ClientFrame::Auth`].
pub const WS_AUTH_TIMEOUT_SECS: u64 = 10;

/// Milliseconds since the Unix epoch, UTC.
pub type Timestamp = i64;

// ---------------------------------------------------------------------------------------------
// Errors

/// What went wrong, as a stable machine-readable code. The `message` beside it is for people.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request body or a parameter is malformed or out of range.
    Invalid,
    /// No valid session: log in again.
    Unauthenticated,
    /// Logged in, but not allowed to do this.
    Forbidden,
    /// It does not exist, or you are not allowed to know it exists.
    NotFound,
    /// It already exists (a username, a channel name).
    Conflict,
    /// Too many requests; try again later.
    RateLimited,
    /// Registration is closed or needs an invitation.
    RegistrationClosed,
    /// The conversation's encryption moved on (a device joined or left) since the request was
    /// prepared: process the new log entries, then encrypt again.
    StaleEpoch,
    /// The password was right and the account has two-step verification: send the login again
    /// with `totp` or `recovery_code`.
    MfaRequired,
    /// The server failed; the details are in its log.
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// The input field at fault, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Server

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub api_version: u32,
    /// Whether anyone may create an account, or only people with an invitation.
    pub registration: Registration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    Open,
    InviteOnly,
    Closed,
}

// ---------------------------------------------------------------------------------------------
// Accounts

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub display_name: String,
    pub password: String,
    /// Required when the server's registration is invite-only; joins that workspace.
    #[serde(default)]
    pub invite_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// The six-digit code from an authenticator app, when the account has two-step verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
    /// One of the account's single-use recovery codes, instead of `totp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<String>,
}

/// Two-step verification (TOTP) for the signed-in account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MfaStatus {
    pub enabled: bool,
    /// Unused recovery codes; 0 when disabled.
    pub recovery_codes_left: u32,
}

/// Start enabling two-step verification. The password is asked again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaSetupRequest {
    pub password: String,
}

/// A new secret for the authenticator app. It is not active until confirmed with a code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaSetup {
    /// Base32, for typing into an authenticator app.
    pub secret: String,
    /// `otpauth://totp/...`, for a QR code.
    pub uri: String,
}

/// Confirm the pending secret with a current code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaCodeRequest {
    pub code: String,
}

/// Turn two-step verification off: the password and a current code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaDisableRequest {
    pub password: String,
    pub code: String,
}

/// Recovery codes, shown once. Each signs in one time instead of a code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaRecoveryCodes {
    pub recovery_codes: Vec<String>,
}

/// Reauthenticate before revoking all other sessions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevokeOtherSessionsRequest {
    pub password: String,
}

/// Change the password of the signed-in account. The old one is asked again; every other
/// session is signed out.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub password: String,
    pub new_password: String,
}

/// Create (or replace) the account's recovery key. The password is asked again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryKeyRequest {
    pub password: String,
}

/// A recovery key, shown once. The server keeps only a hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryKey {
    pub recovery_key: String,
}

/// Whether the account has a recovery key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatus {
    pub has_key: bool,
}

/// Set a new password without the old one, with the account's recovery key or a one-use code
/// from whoever runs the server. Accounts with two-step verification also send a code from
/// their authenticator app (or one of their recovery codes) when using the recovery key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResetPasswordRequest {
    pub username: String,
    pub code: String,
    pub new_password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<String>,
}

/// The new session, and the replacement recovery key (shown once).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResetPasswordResponse {
    pub session: SessionResponse,
    pub recovery_key: String,
}

/// A new session. The token is shown once; the server keeps only its hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub token: String,
    pub expires_at: Timestamp,
    pub user: User,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    pub display_name: String,
}

// ---------------------------------------------------------------------------------------------
// Organizations

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub id: OrganizationId,
    pub name: String,
    pub role: Role,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateOrganizationRequest {
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetOrganizationMemberRequest {
    pub role: Role,
}

// ---------------------------------------------------------------------------------------------
// Workspace tasks (server-readable, separate from encrypted messages)

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub assignee_id: Option<UserId>,
    pub due_at: Option<Timestamp>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub revision: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    pub priority: TaskPriority,
    pub assignee_id: Option<UserId>,
    pub due_at: Option<Timestamp>,
    /// Must be true: task data is readable by the server and workspace members.
    pub acknowledge_server_readable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub assignee_id: Option<UserId>,
    pub due_at: Option<Timestamp>,
    /// The revision read by the client; concurrent edits otherwise return conflict.
    pub revision: i64,
}

// ---------------------------------------------------------------------------------------------
// Workspaces

/// A GitHub event's server-readable metadata, separate from end-to-end encrypted messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubEvent {
    pub id: i64,
    pub event: String,
    pub action: Option<String>,
    pub object_number: Option<i64>,
    pub received_at: Timestamp,
}

/// What sends notices through an integration. Both are incoming webhooks; the kind names the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationKind {
    /// Hito, Nodalia's tasks and milestones app.
    Hito,
    /// Any app or script that can send a signed HTTP request.
    Webhook,
}

/// An incoming webhook that posts notices to a channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integration {
    pub id: i64,
    pub channel_id: ChannelId,
    pub kind: IntegrationKind,
    pub name: String,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateIntegrationRequest {
    pub kind: IntegrationKind,
    pub name: String,
}

/// A new integration, with what the other app needs. The secret is shown this once.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatedIntegration {
    pub integration: Integration,
    /// Where the other app sends requests, from the server's root (`/api/v1/hooks/7`).
    pub path: String,
    /// The key that signs each request body (HMAC-SHA256, keyed with this text as it is).
    pub secret: String,
}

/// A notice an integration posted to a channel. It is not end-to-end encrypted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationNotice {
    pub id: i64,
    pub integration_id: i64,
    pub channel_id: ChannelId,
    pub integration_name: String,
    pub kind: IntegrationKind,
    pub title: Option<String>,
    pub text: String,
    pub url: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// The caller's role in it.
    pub role: Role,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub user: User,
    pub role: Role,
    pub joined_at: Timestamp,
    /// Whether the person has a live connection right now.
    #[serde(default)]
    pub online: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    /// How many people may use it. 1 to 1000; default 1.
    #[serde(default)]
    pub max_uses: Option<u32>,
    /// Hours until it expires. 1 to 720 (30 days); default 168 (7 days).
    #[serde(default)]
    pub expires_in_hours: Option<u32>,
    /// Role given to whoever joins with it: `member` (default) or `guest`.
    #[serde(default)]
    pub role: Option<Role>,
}

/// An invitation. `code` is shown once, at creation; the server keeps only its hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    pub id: InviteId,
    pub workspace_id: WorkspaceId,
    pub code: String,
    pub max_uses: u32,
    pub expires_at: Timestamp,
    pub role: Role,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinWorkspaceRequest {
    pub code: String,
}

// ---------------------------------------------------------------------------------------------
// Channels

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    /// For a direct conversation, the other person's username.
    pub name: String,
    pub topic: String,
    pub kind: ChannelKind,
    pub archived: bool,
    pub created_by: Option<UserId>,
    pub created_at: Timestamp,
    /// Whether the caller is a member (has it in their sidebar).
    pub joined: bool,
    /// Messages after the caller's read marker (top-level and replies), when joined.
    pub unread: u32,
    /// Unread messages that mention the caller.
    #[serde(default)]
    pub mentions: u32,
    pub last_message_id: Option<MessageId>,
    pub member_count: u32,
    /// For a direct conversation, the other person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<User>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub private: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateChannelRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddChannelMemberRequest {
    pub user_id: UserId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenDirectRequest {
    pub user_id: UserId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkReadRequest {
    pub message_id: MessageId,
}

/// A file stored for a conversation: ciphertext only, its key travels in the encrypted message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadedFile {
    pub id: i64,
    pub size: u64,
}

/// How far one member of a conversation has received and read it: every message up to
/// `delivered_id` reached one of their devices, and they have seen every one up to `read_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub user_id: UserId,
    pub delivered_id: MessageId,
    pub read_id: MessageId,
}

// ---------------------------------------------------------------------------------------------
// Messages

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    /// Who reacted, up to a few dozen; enough to show "you" and names on hover.
    pub users: Vec<UserId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub channel_id: ChannelId,
    pub author: User,
    /// Set when this message is a reply in a thread.
    pub thread_root_id: Option<MessageId>,
    /// Empty when deleted.
    pub body: String,
    pub created_at: Timestamp,
    pub edited_at: Option<Timestamp>,
    pub deleted: bool,
    /// For a thread root: how many replies it has.
    pub reply_count: u32,
    pub last_reply_at: Option<Timestamp>,
    pub reactions: Vec<Reaction>,
    /// The key the author's client sent with it, so the author can match its optimistic copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_nonce: Option<String>,
    /// End-to-end encrypted: `body` is empty here and the text is in the conversation's
    /// encrypted log, which only its devices can read. Messages from before encryption are not.
    #[serde(default)]
    pub encrypted: bool,
    /// The log entry holding this message's current text (its creation or latest edit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_seq: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostMessageRequest {
    /// Always empty: the server refuses unencrypted messages.
    #[serde(default)]
    pub body: String,
    /// The MLS application message holding the text, base64.
    #[serde(default)]
    pub ciphertext: Option<String>,
    /// The people the text mentions. The server cannot read the text, so the client says.
    #[serde(default)]
    pub mentions: Vec<UserId>,
    /// A unique key per message chosen by the client. Sending the same key again returns the
    /// message already stored instead of creating a second one.
    pub client_nonce: String,
    #[serde(default)]
    pub thread_root_id: Option<MessageId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditMessageRequest {
    /// Always empty, as for a new message.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub ciphertext: Option<String>,
    #[serde(default)]
    pub mentions: Vec<UserId>,
}

/// Messages by id, for showing results a client found in its own decrypted history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LookupMessagesRequest {
    pub ids: Vec<MessageId>,
}

// ---------------------------------------------------------------------------------------------
// End-to-end encryption

/// This session's device: its MLS signing key, base64. One per session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterDeviceRequest {
    pub public_key: String,
}

/// This session's device as the server sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceStatus {
    /// Key packages left for others to add this device with. Publish more when it runs low.
    pub key_packages: u32,
}

/// One of your devices: a computer that can read your conversations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnDevice {
    /// The MLS signing key, base64. Compare its fingerprint with what others see.
    pub public_key: String,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    /// The device making this request.
    pub this_device: bool,
    /// Signed in now. A signed-out device keeps its place in conversations for a while, so
    /// signing in again on the same computer shows everything.
    pub signed_in: bool,
}

/// Remove one of your devices: it is signed out and leaves every conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveDeviceRequest {
    pub public_key: String,
}

/// One-use key packages for this session's device, base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishKeyPackagesRequest {
    pub key_packages: Vec<String>,
}

/// Claim one key package for each listed device (base64 signing keys) to add it to a
/// conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimKeyPackagesRequest {
    pub devices: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimedKeyPackage {
    pub device: Device,
    pub key_package: String,
}

/// A Welcome waiting for the caller's device: it was added at log entry `seq`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MlsWelcome {
    pub seq: i64,
    pub data: String,
}

/// A signed-in device of someone who can read a conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub user_id: UserId,
    /// Base64 MLS signing key.
    pub public_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MlsKind {
    /// Devices joined or left: the conversation moves to the next epoch.
    Commit,
    /// A message's text, new or edited.
    Message,
}

/// One entry of a conversation's encrypted log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MlsEntry {
    pub seq: i64,
    pub epoch: u64,
    pub kind: MlsKind,
    pub author_id: UserId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    /// The MLS message, base64. Empty for a deleted message.
    pub data: String,
}

/// A conversation's encryption state, and (with `after`) the log entries after a point.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MlsState {
    pub channel_id: ChannelId,
    /// `None` until the first device starts the group.
    pub epoch: Option<u64>,
    /// The latest GroupInfo (public: it lets a member's new device join), base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_info: Option<String>,
    /// The newest entry in the log, 0 for none.
    pub last_seq: i64,
    /// Signed-in devices of everyone who may read the conversation now.
    pub devices: Vec<Device>,
    pub entries: Vec<MlsEntry>,
    /// More entries follow the last one returned.
    pub has_more: bool,
    /// A Welcome for the caller's device, if someone added it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<MlsWelcome>,
}

/// Start a conversation's group (first device only).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateGroupRequest {
    pub group_info: String,
}

/// A commit for the current epoch and the GroupInfo of the epoch it leads to, both base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitRequest {
    pub commit: String,
    pub group_info: String,
    /// For a commit that adds devices: their Welcome, and the devices (signing keys) it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub welcome_to: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitResponse {
    pub seq: i64,
    pub epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactRequest {
    pub emoji: String,
}

/// A page of history, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    /// Whether older messages exist before the first one returned.
    pub has_more: bool,
}

/// A thread: its root and its replies, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub root: Message,
    pub replies: Vec<Message>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResults {
    pub messages: Vec<Message>,
}

// ---------------------------------------------------------------------------------------------
// Real time

/// What a client sends on the WebSocket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    /// Must be the first frame, within [`WS_AUTH_TIMEOUT_SECS`].
    Auth {
        token: String,
    },
    /// The person is typing in a channel (or a thread of it). Send at most every few seconds.
    Typing {
        channel_id: ChannelId,
        #[serde(default)]
        thread_root_id: Option<MessageId>,
    },
    Ping,
    /// Join the call in a channel (starting it if nobody is in it). A connection is in at most
    /// one call; joining another leaves the first, and joining from a second device moves the
    /// person there.
    CallJoin {
        channel_id: ChannelId,
        #[serde(default)]
        audio: bool,
        #[serde(default)]
        video: bool,
    },
    /// Say whether the microphone and camera are on.
    CallUpdate {
        channel_id: ChannelId,
        audio: bool,
        video: bool,
    },
    CallLeave {
        channel_id: ChannelId,
    },
}

/// What the server sends on the WebSocket. Clients ignore event types they do not know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Authentication succeeded; events follow.
    Ready {
        user: User,
    },
    /// A fatal error; the server closes the socket after it.
    Error {
        code: ErrorCode,
        message: String,
    },
    MessageCreated {
        message: Message,
    },
    MessageUpdated {
        message: Message,
    },
    MessageDeleted {
        channel_id: ChannelId,
        message_id: MessageId,
        thread_root_id: Option<MessageId>,
    },
    ReactionsChanged {
        channel_id: ChannelId,
        message_id: MessageId,
        reactions: Vec<Reaction>,
    },
    /// A channel was created, renamed, archived or gained members: refetch it.
    ChannelChanged {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// A conversation's encrypted log has a new commit: devices joined or left.
    MlsChanged {
        channel_id: ChannelId,
    },
    /// The caller can no longer see this channel.
    ChannelRemoved {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// A workspace the caller belonged to was deleted by its owner.
    WorkspaceRemoved {
        workspace_id: WorkspaceId,
    },
    /// Someone joined a workspace the caller belongs to.
    MemberJoined {
        workspace_id: WorkspaceId,
        member: Member,
    },
    /// The caller's read marker moved (from another of their devices).
    ReadMarker {
        channel_id: ChannelId,
        message_id: MessageId,
    },
    /// An integration posted a notice to a channel the caller can read.
    IntegrationNotice {
        notice: IntegrationNotice,
    },
    /// A member of a conversation the caller can read received or read more of it.
    Receipt {
        channel_id: ChannelId,
        receipt: Receipt,
    },
    Typing {
        channel_id: ChannelId,
        thread_root_id: Option<MessageId>,
        user: User,
    },
    Presence {
        user_id: UserId,
        online: bool,
    },
    /// Who is in a channel's call now. Sent to everyone who can read the channel when it
    /// changes, and after [`ServerEvent::Ready`] for every call going on. No participants means
    /// the call ended.
    CallChanged {
        channel_id: ChannelId,
        participants: Vec<CallParticipant>,
    },
    /// This connection is no longer in the call: the person joined from another device, lost
    /// access to the channel, or the call was full.
    CallLeft {
        channel_id: ChannelId,
        reason: CallLeftReason,
    },
    Pong,
}

/// Someone in a call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallParticipant {
    pub user_id: UserId,
    pub audio: bool,
    pub video: bool,
    pub joined_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallLeftReason {
    /// The person joined the call from another device.
    OtherDevice,
    /// The call has as many people as the server allows.
    Full,
    /// The person can no longer take part in the channel.
    NoAccess,
}

/// Binary WebSocket frames carry a call's audio and video, end-to-end encrypted by the clients.
///
/// Up, a client sends `[version][kind][channel id: i64 BE][payload]`; down, the server relays
/// `[version][kind][channel id: i64 BE][sender user id: i64 BE][payload]` to the other people in
/// the call. The server reads only the header and never the payload.
pub mod media {
    use agora_core::{ChannelId, UserId};

    /// The frame layout version.
    pub const VERSION: u8 = 1;
    /// The largest payload a frame may carry.
    pub const MAX_PAYLOAD: usize = 60 * 1024;
    const UP_HEADER: usize = 10;
    const DOWN_HEADER: usize = 18;
    /// The largest binary frame either side sends.
    pub const MAX_FRAME: usize = MAX_PAYLOAD + DOWN_HEADER;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Kind {
        Audio,
        Video,
    }

    impl Kind {
        fn byte(self) -> u8 {
            match self {
                Kind::Audio => 1,
                Kind::Video => 2,
            }
        }

        fn from_byte(b: u8) -> Option<Kind> {
            match b {
                1 => Some(Kind::Audio),
                2 => Some(Kind::Video),
                _ => None,
            }
        }
    }

    /// A frame a client sends.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Up<'a> {
        pub kind: Kind,
        pub channel: ChannelId,
        pub payload: &'a [u8],
    }

    /// A frame the server relays.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Down<'a> {
        pub kind: Kind,
        pub channel: ChannelId,
        pub sender: UserId,
        pub payload: &'a [u8],
    }

    fn i64_at(bytes: &[u8], at: usize) -> Option<i64> {
        let raw: [u8; 8] = bytes.get(at..at.checked_add(8)?)?.try_into().ok()?;
        Some(i64::from_be_bytes(raw))
    }

    impl Up<'_> {
        pub fn encode(&self) -> Vec<u8> {
            let mut out = Vec::with_capacity(UP_HEADER + self.payload.len());
            out.push(VERSION);
            out.push(self.kind.byte());
            out.extend_from_slice(&self.channel.0.to_be_bytes());
            out.extend_from_slice(self.payload);
            out
        }

        /// `None` for a frame of another version, an unknown kind, or a payload too large.
        pub fn parse(bytes: &[u8]) -> Option<Up<'_>> {
            if *bytes.first()? != VERSION {
                return None;
            }
            let kind = Kind::from_byte(*bytes.get(1)?)?;
            let channel = ChannelId(i64_at(bytes, 2)?);
            let payload = bytes.get(UP_HEADER..)?;
            (payload.len() <= MAX_PAYLOAD).then_some(Up {
                kind,
                channel,
                payload,
            })
        }

        /// The frame the server relays for it.
        pub fn relay(&self, sender: UserId) -> Vec<u8> {
            Down {
                kind: self.kind,
                channel: self.channel,
                sender,
                payload: self.payload,
            }
            .encode()
        }
    }

    impl Down<'_> {
        pub fn encode(&self) -> Vec<u8> {
            let mut out = Vec::with_capacity(DOWN_HEADER + self.payload.len());
            out.push(VERSION);
            out.push(self.kind.byte());
            out.extend_from_slice(&self.channel.0.to_be_bytes());
            out.extend_from_slice(&self.sender.0.to_be_bytes());
            out.extend_from_slice(self.payload);
            out
        }

        pub fn parse(bytes: &[u8]) -> Option<Down<'_>> {
            if *bytes.first()? != VERSION {
                return None;
            }
            let kind = Kind::from_byte(*bytes.get(1)?)?;
            let channel = ChannelId(i64_at(bytes, 2)?);
            let sender = UserId(i64_at(bytes, 10)?);
            let payload = bytes.get(DOWN_HEADER..)?;
            (payload.len() <= MAX_PAYLOAD).then_some(Down {
                kind,
                channel,
                sender,
                payload,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn frames_round_trip_and_hostile_bytes_are_refused() {
            let up = Up {
                kind: Kind::Video,
                channel: ChannelId(42),
                payload: b"sealed",
            };
            let bytes = up.encode();
            assert_eq!(Up::parse(&bytes), Some(up));
            let relayed = up.relay(UserId(7));
            let down = Down::parse(&relayed).unwrap();
            assert_eq!(down.sender, UserId(7));
            assert_eq!(down.payload, b"sealed");
            assert_eq!(down.kind, Kind::Video);

            for bad in [
                &b""[..],
                &[1],
                &[2, 1, 0, 0, 0, 0, 0, 0, 0, 1],
                &[1, 9, 0, 0, 0, 0, 0, 0, 0, 1],
            ] {
                assert_eq!(Up::parse(bad), None);
            }
            assert_eq!(Up::parse(&bytes[..9]), None);
            assert_eq!(Down::parse(&relayed[..17]), None);
            let mut big = Up { payload: &[], ..up }.encode();
            big.resize(10 + MAX_PAYLOAD + 1, 0);
            assert_eq!(Up::parse(&big), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_tagged_json() {
        let f = ClientFrame::Auth { token: "t".into() };
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"type":"auth","token":"t"}"#
        );
        let typing: ClientFrame =
            serde_json::from_str(r#"{"type":"typing","channel_id":3}"#).unwrap();
        assert_eq!(
            typing,
            ClientFrame::Typing {
                channel_id: ChannelId(3),
                thread_root_id: None
            }
        );
    }

    #[test]
    fn events_round_trip() {
        let e = ServerEvent::Presence {
            user_id: UserId(7),
            online: true,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"type":"presence","user_id":7,"online":true}"#);
        assert_eq!(serde_json::from_str::<ServerEvent>(&json).unwrap(), e);
    }

    #[test]
    fn errors_have_stable_codes() {
        let e = ApiError {
            code: ErrorCode::RateLimited,
            message: "slow down".into(),
            field: None,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"code":"rate_limited","message":"slow down"}"#
        );
    }
}
