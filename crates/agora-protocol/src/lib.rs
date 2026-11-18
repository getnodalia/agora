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
    Typing {
        channel_id: ChannelId,
        thread_root_id: Option<MessageId>,
        user: User,
    },
    Presence {
        user_id: UserId,
        online: bool,
    },
    Pong,
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
