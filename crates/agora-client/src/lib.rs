//! Talking to an Ágora server, from any front end.
//!
//! [`Client`] wraps the HTTP API; [`realtime::connect`] keeps a WebSocket open, reconnecting with
//! backoff and reporting what happens through a callback. Neither touches a UI toolkit: the
//! desktop app, tests and future web or mobile clients share them.

pub mod files;
pub mod realtime;
pub mod secure;

use agora_core::{ChannelId, MessageId, OrganizationId, UserId, WorkspaceId};
use agora_protocol::*;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

pub use realtime::{connect, MediaSender, Realtime, RealtimeEvent};
pub use secure::Secure;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server answered with an error.
    #[error("{}", .error.message)]
    Api { status: u16, error: ApiError },
    /// The server could not be reached, or the connection broke.
    #[error("cannot reach the server: {0}")]
    Network(String),
    #[error("the server address is not valid: {0}")]
    InvalidUrl(String),
    #[error("unexpected answer from the server: {0}")]
    Decode(String),
    /// End-to-end encryption failed on this device (keys, storage, a damaged message).
    #[error("encryption failed: {0}")]
    Encryption(String),
    /// Something on this computer failed: a file could not be read or written.
    #[error("{0}")]
    Local(String),
}

impl ClientError {
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            ClientError::Api { error, .. } => Some(error.code),
            _ => None,
        }
    }
    /// The session is gone: the person must log in again.
    pub fn is_unauthenticated(&self) -> bool {
        self.code() == Some(ErrorCode::Unauthenticated)
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Parse what a person typed as a server address: `chat.example.com`, `localhost:7878`,
/// `https://chat.example.com/`. Without a scheme, loopback hosts get `http`, others `https`.
pub fn parse_server_url(input: &str) -> Result<Url> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        return Err(ClientError::InvalidUrl("empty".into()));
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        let host = input.split([':', '/']).next().unwrap_or_default();
        let local =
            matches!(host, "localhost" | "127.0.0.1" | "[::1]") || host.ends_with(".localhost");
        format!("{}://{input}", if local { "http" } else { "https" })
    };
    let url = Url::parse(&with_scheme).map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ClientError::InvalidUrl(
            "use an http or https address".into(),
        ));
    }
    Ok(url)
}

/// Whether credentials sent to `url` would cross a network unencrypted.
pub fn is_insecure(url: &Url) -> bool {
    url.scheme() == "http"
        && !matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        )
}

/// An HTTP client for one server, with or without a session.
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
}

impl Client {
    pub fn new(base: Url) -> Result<Client> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Agora/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ClientError::Network(e.to_string()))?;
        Ok(Client {
            http,
            base,
            token: None,
        })
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Client {
        self.token = Some(token.into());
        self
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    /// The WebSocket address of this server.
    pub fn ws_url(&self) -> Result<Url> {
        let mut url = self.base.clone();
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|()| ClientError::InvalidUrl("cannot derive the WebSocket address".into()))?;
        url.set_path(WS_PATH);
        Ok(url)
    }

    fn url(&self, path: &str) -> Result<Url> {
        let mut url = self.base.clone();
        url.set_path(&format!("{API_PREFIX}{path}"));
        Ok(url)
    }

    async fn send<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<&B>,
    ) -> Result<T> {
        let mut req = self.http.request(method, url);
        if let Some(body) = body {
            req = req.json(body);
        }
        let bytes = self.execute(req).await?;
        let bytes = if bytes.is_empty() {
            &b"null"[..]
        } else {
            &bytes[..]
        };
        serde_json::from_slice(bytes).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Send a request with this session's token; the body of a successful answer, or the
    /// server's error.
    async fn execute(&self, mut req: reqwest::RequestBuilder) -> Result<Vec<u8>> {
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let res = req
            .send()
            .await
            .map_err(|e| ClientError::Network(e.without_url().to_string()))?;
        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .map_err(|e| ClientError::Network(e.without_url().to_string()))?;
        if !status.is_success() {
            let error = serde_json::from_slice::<ApiError>(&bytes).unwrap_or(ApiError {
                code: if status == StatusCode::UNAUTHORIZED {
                    ErrorCode::Unauthenticated
                } else {
                    ErrorCode::Internal
                },
                message: format!("the server answered {status}"),
                field: None,
            });
            return Err(ClientError::Api {
                status: status.as_u16(),
                error,
            });
        }
        Ok(bytes.to_vec())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send::<(), T>(Method::GET, self.url(path)?, None).await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send(Method::POST, self.url(path)?, Some(body)).await
    }

    // Accounts --------------------------------------------------------------------------------

    pub async fn server_info(&self) -> Result<ServerInfo> {
        self.get("/server").await
    }

    /// Create an account. The returned client carries its session.
    pub async fn register(&self, req: &RegisterRequest) -> Result<(Client, SessionResponse)> {
        let session: SessionResponse = self.post("/auth/register", req).await?;
        Ok((self.clone().with_token(session.token.clone()), session))
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<(Client, SessionResponse)> {
        self.login_with(&LoginRequest {
            username: username.to_owned(),
            password: password.to_owned(),
            totp: None,
            recovery_code: None,
        })
        .await
    }

    /// Log in with a second factor. A missing one fails with `ErrorCode::MfaRequired`.
    pub async fn login_with(&self, req: &LoginRequest) -> Result<(Client, SessionResponse)> {
        let session: SessionResponse = self.post("/auth/login", &req).await?;
        Ok((self.clone().with_token(session.token.clone()), session))
    }

    /// Choose a new password with the recovery key (or a code from whoever runs the server).
    /// Signs in, and returns the replacement recovery key.
    pub async fn reset_password(
        &self,
        req: &ResetPasswordRequest,
    ) -> Result<(Client, SessionResponse, String)> {
        let reply: ResetPasswordResponse = self.post("/auth/password-reset", req).await?;
        let client = self.clone().with_token(reply.session.token.clone());
        Ok((client, reply.session, reply.recovery_key))
    }

    /// End the session on the server. Its WebSockets close.
    pub async fn logout(&self) -> Result<()> {
        self.post::<_, ()>("/auth/logout", &()).await
    }

    /// Reauthenticate and sign out every other session belonging to this account.
    pub async fn revoke_other_sessions(&self, password: &str) -> Result<()> {
        let req = RevokeOtherSessionsRequest {
            password: password.to_owned(),
        };
        self.post("/auth/sessions/revoke-others", &req).await
    }

    // Two-step verification ------------------------------------------------------------------

    pub async fn mfa_status(&self) -> Result<MfaStatus> {
        self.get("/auth/mfa").await
    }

    /// A new authenticator secret; it becomes active with [`Client::mfa_enable`].
    pub async fn mfa_setup(&self, password: &str) -> Result<MfaSetup> {
        self.post(
            "/auth/mfa/setup",
            &MfaSetupRequest {
                password: password.to_owned(),
            },
        )
        .await
    }

    /// Confirm the pending secret. Other sessions are signed out; the codes are shown once.
    pub async fn mfa_enable(&self, code: &str) -> Result<MfaRecoveryCodes> {
        self.post(
            "/auth/mfa/enable",
            &MfaCodeRequest {
                code: code.to_owned(),
            },
        )
        .await
    }

    pub async fn mfa_disable(&self, password: &str, code: &str) -> Result<()> {
        self.post(
            "/auth/mfa/disable",
            &MfaDisableRequest {
                password: password.to_owned(),
                code: code.to_owned(),
            },
        )
        .await
    }

    /// Whether the account has a recovery key.
    pub async fn recovery_status(&self) -> Result<RecoveryStatus> {
        self.get("/auth/recovery-key").await
    }

    /// A new recovery key (the old one stops working). Shown once.
    pub async fn new_recovery_key(&self, password: &str) -> Result<RecoveryKey> {
        self.post(
            "/auth/recovery-key",
            &RecoveryKeyRequest {
                password: password.to_owned(),
            },
        )
        .await
    }

    /// Change the password; every other session is signed out.
    pub async fn change_password(&self, password: &str, new_password: &str) -> Result<()> {
        self.post(
            "/auth/password",
            &ChangePasswordRequest {
                password: password.to_owned(),
                new_password: new_password.to_owned(),
            },
        )
        .await
    }

    /// Replace every recovery code; the old ones stop working.
    pub async fn mfa_new_recovery_codes(&self, code: &str) -> Result<MfaRecoveryCodes> {
        self.post(
            "/auth/mfa/recovery-codes",
            &MfaCodeRequest {
                code: code.to_owned(),
            },
        )
        .await
    }

    pub async fn me(&self) -> Result<User> {
        self.get("/me").await
    }

    pub async fn update_profile(&self, display_name: &str) -> Result<User> {
        let req = UpdateProfileRequest {
            display_name: display_name.to_owned(),
        };
        self.send(Method::PATCH, self.url("/me")?, Some(&req)).await
    }

    // Organizations --------------------------------------------------------------------------

    pub async fn organizations(&self) -> Result<Vec<Organization>> {
        self.get("/organizations").await
    }
    pub async fn create_organization(&self, name: &str) -> Result<Organization> {
        self.post(
            "/organizations",
            &CreateOrganizationRequest {
                name: name.to_owned(),
            },
        )
        .await
    }
    pub async fn organization_workspaces(&self, org: OrganizationId) -> Result<Vec<Workspace>> {
        self.get(&format!("/organizations/{org}/workspaces")).await
    }
    pub async fn create_organization_workspace(
        &self,
        org: OrganizationId,
        name: &str,
    ) -> Result<Workspace> {
        self.post(
            &format!("/organizations/{org}/workspaces"),
            &CreateWorkspaceRequest {
                name: name.to_owned(),
            },
        )
        .await
    }
    pub async fn organization_members(&self, org: OrganizationId) -> Result<Vec<Member>> {
        self.get(&format!("/organizations/{org}/members")).await
    }
    pub async fn set_organization_member(
        &self,
        org: OrganizationId,
        user: UserId,
        role: agora_core::Role,
    ) -> Result<()> {
        self.send(
            Method::PUT,
            self.url(&format!("/organizations/{org}/members/{user}"))?,
            Some(&SetOrganizationMemberRequest { role }),
        )
        .await
    }

    // Workspaces ------------------------------------------------------------------------------

    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        self.get("/workspaces").await
    }

    /// The workspace's server-readable GitHub inbox, separate from encrypted chat messages.
    pub async fn github_events(&self, ws: WorkspaceId) -> Result<Vec<GitHubEvent>> {
        self.get(&format!("/workspaces/{ws}/integrations/github/events"))
            .await
    }

    pub async fn create_workspace(&self, name: &str) -> Result<Workspace> {
        self.post(
            "/workspaces",
            &CreateWorkspaceRequest {
                name: name.to_owned(),
            },
        )
        .await
    }

    pub async fn members(&self, ws: WorkspaceId) -> Result<Vec<Member>> {
        self.get(&format!("/workspaces/{ws}/members")).await
    }

    pub async fn create_invite(
        &self,
        ws: WorkspaceId,
        req: &CreateInviteRequest,
    ) -> Result<Invite> {
        self.post(&format!("/workspaces/{ws}/invites"), req).await
    }

    pub async fn join_workspace(&self, code: &str) -> Result<Workspace> {
        self.post(
            "/workspaces/join",
            &JoinWorkspaceRequest {
                code: code.to_owned(),
            },
        )
        .await
    }

    // Channels --------------------------------------------------------------------------------

    pub async fn channels(&self, ws: WorkspaceId) -> Result<Vec<Channel>> {
        self.get(&format!("/workspaces/{ws}/channels")).await
    }

    pub async fn channel(&self, id: ChannelId) -> Result<Channel> {
        self.get(&format!("/channels/{id}")).await
    }

    pub async fn create_channel(
        &self,
        ws: WorkspaceId,
        req: &CreateChannelRequest,
    ) -> Result<Channel> {
        self.post(&format!("/workspaces/{ws}/channels"), req).await
    }

    pub async fn update_channel(
        &self,
        id: ChannelId,
        req: &UpdateChannelRequest,
    ) -> Result<Channel> {
        self.send(
            Method::PATCH,
            self.url(&format!("/channels/{id}"))?,
            Some(req),
        )
        .await
    }

    pub async fn join_channel(&self, id: ChannelId) -> Result<Channel> {
        self.post(&format!("/channels/{id}/join"), &()).await
    }

    pub async fn leave_channel(&self, id: ChannelId) -> Result<()> {
        self.post(&format!("/channels/{id}/leave"), &()).await
    }

    pub async fn add_channel_member(&self, id: ChannelId, user: UserId) -> Result<Channel> {
        self.post(
            &format!("/channels/{id}/members"),
            &AddChannelMemberRequest { user_id: user },
        )
        .await
    }

    pub async fn open_direct(&self, ws: WorkspaceId, user: UserId) -> Result<Channel> {
        self.post(
            &format!("/workspaces/{ws}/direct"),
            &OpenDirectRequest { user_id: user },
        )
        .await
    }

    pub async fn mark_read(&self, id: ChannelId, message: MessageId) -> Result<()> {
        self.post(
            &format!("/channels/{id}/read"),
            &MarkReadRequest {
                message_id: message,
            },
        )
        .await
    }

    /// One of this person's devices received the channel's messages up to `message`.
    pub async fn mark_delivered(&self, id: ChannelId, message: MessageId) -> Result<()> {
        self.post(
            &format!("/channels/{id}/delivered"),
            &MarkReadRequest {
                message_id: message,
            },
        )
        .await
    }

    /// Store an encrypted file for a channel. `ciphertext` must already be encrypted: use
    /// [`Secure::send_file`] to send a file in a message.
    pub async fn upload_file(
        &self,
        channel: ChannelId,
        ciphertext: Vec<u8>,
    ) -> Result<UploadedFile> {
        let req = self
            .http
            .post(self.url(&format!("/channels/{channel}/files"))?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(ciphertext);
        let bytes = self.execute(req).await?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// An encrypted file's bytes, as stored.
    pub async fn download_file(&self, id: i64) -> Result<Vec<u8>> {
        self.execute(self.http.get(self.url(&format!("/files/{id}"))?))
            .await
    }

    /// How far each member has received and read a channel.
    pub async fn receipts(&self, id: ChannelId) -> Result<Vec<Receipt>> {
        self.get(&format!("/channels/{id}/receipts")).await
    }

    // Integrations ----------------------------------------------------------------------------

    /// The incoming webhooks that post to a channel.
    pub async fn integrations(&self, id: ChannelId) -> Result<Vec<Integration>> {
        self.get(&format!("/channels/{id}/integrations")).await
    }

    /// Add an incoming webhook to a channel (workspace admins). The secret comes back once.
    pub async fn create_integration(
        &self,
        id: ChannelId,
        req: &CreateIntegrationRequest,
    ) -> Result<CreatedIntegration> {
        self.post(&format!("/channels/{id}/integrations"), req)
            .await
    }

    /// Delete a workspace and everything in it. Only its owner may, and `name` must be the
    /// workspace's exact name: the server refuses a mistyped confirmation.
    pub async fn delete_workspace(&self, id: WorkspaceId, name: &str) -> Result<()> {
        let mut url = self.url(&format!("/workspaces/{id}"))?;
        url.query_pairs_mut().append_pair("confirm", name);
        self.send::<(), _>(Method::DELETE, url, None).await
    }

    pub async fn delete_integration(&self, id: i64) -> Result<()> {
        self.send::<(), _>(
            Method::DELETE,
            self.url(&format!("/integrations/{id}"))?,
            None,
        )
        .await
    }

    /// Post a test notice through an integration, to check the connection from this side.
    pub async fn test_integration(&self, id: i64) -> Result<()> {
        self.post(&format!("/integrations/{id}/test"), &()).await
    }

    /// The newest notices integrations posted to a channel, oldest first.
    pub async fn notices(&self, id: ChannelId) -> Result<Vec<IntegrationNotice>> {
        self.get(&format!("/channels/{id}/notices")).await
    }

    /// The full address another app sends notices to.
    pub fn hook_url(&self, created: &CreatedIntegration) -> String {
        let mut url = self.base.clone();
        url.set_path(&created.path);
        url.to_string()
    }

    // Messages --------------------------------------------------------------------------------

    /// History of a channel: the latest page, the page `before` a message, or what came `after`
    /// one.
    pub async fn history(
        &self,
        id: ChannelId,
        before: Option<MessageId>,
        after: Option<MessageId>,
        limit: Option<u32>,
    ) -> Result<MessagePage> {
        let mut url = self.url(&format!("/channels/{id}/messages"))?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(b) = before {
                q.append_pair("before", &b.to_string());
            }
            if let Some(a) = after {
                q.append_pair("after", &a.to_string());
            }
            if let Some(l) = limit {
                q.append_pair("limit", &l.to_string());
            }
        }
        self.send::<(), _>(Method::GET, url, None).await
    }

    pub async fn post_message(&self, id: ChannelId, req: &PostMessageRequest) -> Result<Message> {
        self.post(&format!("/channels/{id}/messages"), req).await
    }

    /// Replace a message's text. Messages are encrypted: [`Secure::edit`] builds the request.
    pub async fn edit_message(&self, id: MessageId, req: &EditMessageRequest) -> Result<Message> {
        self.send(
            Method::PATCH,
            self.url(&format!("/messages/{id}"))?,
            Some(req),
        )
        .await
    }

    /// Messages by id that the caller may read in a workspace, newest first.
    pub async fn lookup_messages(
        &self,
        ws: WorkspaceId,
        ids: &[MessageId],
    ) -> Result<SearchResults> {
        self.post(
            &format!("/workspaces/{ws}/messages/lookup"),
            &LookupMessagesRequest { ids: ids.to_vec() },
        )
        .await
    }

    // End-to-end encryption -------------------------------------------------------------------

    /// Tie this session to a device's MLS signing key (base64).
    pub async fn register_device(&self, public_key: &str) -> Result<DeviceStatus> {
        self.send(
            Method::PUT,
            self.url("/devices/me")?,
            Some(&RegisterDeviceRequest {
                public_key: public_key.to_owned(),
            }),
        )
        .await
    }

    /// Publish key packages for this session's device (base64).
    /// Your devices: signed in, or signed out recently and still in your conversations.
    pub async fn own_devices(&self) -> Result<Vec<OwnDevice>> {
        self.get("/devices").await
    }

    /// Remove a device (base64 key). Removing this one signs this session out.
    pub async fn remove_device(&self, public_key: &str) -> Result<()> {
        self.post(
            "/devices/remove",
            &RemoveDeviceRequest {
                public_key: public_key.to_owned(),
            },
        )
        .await
    }

    pub async fn publish_key_packages(&self, key_packages: Vec<String>) -> Result<DeviceStatus> {
        self.post(
            "/devices/me/key_packages",
            &PublishKeyPackagesRequest { key_packages },
        )
        .await
    }

    /// Take a key package for each listed device (base64 signing keys) to add it to a
    /// conversation. Devices without one left are missing from the answer.
    pub async fn claim_key_packages(
        &self,
        id: ChannelId,
        devices: Vec<String>,
    ) -> Result<Vec<ClaimedKeyPackage>> {
        self.post(
            &format!("/channels/{id}/mls/key_packages"),
            &ClaimKeyPackagesRequest { devices },
        )
        .await
    }

    /// A conversation's encryption state, with the log entries after `after` when it is set.
    pub async fn mls_state(&self, id: ChannelId, after: Option<i64>) -> Result<MlsState> {
        let mut url = self.url(&format!("/channels/{id}/mls"))?;
        if let Some(after) = after {
            url.query_pairs_mut()
                .append_pair("after", &after.to_string());
        }
        self.send::<(), _>(Method::GET, url, None).await
    }

    pub async fn mls_create_group(
        &self,
        id: ChannelId,
        group_info: &str,
    ) -> Result<CommitResponse> {
        self.post(
            &format!("/channels/{id}/mls/group"),
            &CreateGroupRequest {
                group_info: group_info.to_owned(),
            },
        )
        .await
    }

    pub async fn mls_commit(&self, id: ChannelId, req: &CommitRequest) -> Result<CommitResponse> {
        self.post(&format!("/channels/{id}/mls/commit"), req).await
    }

    pub async fn delete_message(&self, id: MessageId) -> Result<()> {
        self.send::<(), _>(Method::DELETE, self.url(&format!("/messages/{id}"))?, None)
            .await
    }

    pub async fn thread(&self, id: MessageId) -> Result<Thread> {
        self.get(&format!("/messages/{id}/thread")).await
    }

    pub async fn react(&self, id: MessageId, emoji: &str) -> Result<Vec<Reaction>> {
        self.post(
            &format!("/messages/{id}/reactions"),
            &ReactRequest {
                emoji: emoji.to_owned(),
            },
        )
        .await
    }

    pub async fn search(&self, ws: WorkspaceId, query: &str) -> Result<SearchResults> {
        let mut url = self.url(&format!("/workspaces/{ws}/search"))?;
        url.query_pairs_mut().append_pair("q", query);
        self.send::<(), _>(Method::GET, url, None).await
    }
}

/// A fresh idempotency key for a message.
pub fn new_nonce() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("{:016x}{:016x}", rng.gen::<u64>(), rng.gen::<u64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses() {
        assert_eq!(
            parse_server_url("localhost:7878").unwrap().as_str(),
            "http://localhost:7878/"
        );
        assert_eq!(
            parse_server_url("chat.example.com").unwrap().as_str(),
            "https://chat.example.com/"
        );
        assert_eq!(
            parse_server_url("http://10.0.0.2:7878/").unwrap().port(),
            Some(7878)
        );
        assert!(parse_server_url("ftp://x").is_err());
        assert!(parse_server_url("").is_err());
    }

    #[test]
    fn plain_http_is_only_fine_on_this_machine() {
        assert!(!is_insecure(&parse_server_url("localhost:7878").unwrap()));
        assert!(is_insecure(
            &parse_server_url("http://10.0.0.2:7878").unwrap()
        ));
        assert!(!is_insecure(&parse_server_url("chat.example.com").unwrap()));
    }

    #[test]
    fn websocket_address_follows_the_scheme() {
        let c = Client::new(parse_server_url("chat.example.com").unwrap()).unwrap();
        assert_eq!(
            c.ws_url().unwrap().as_str(),
            "wss://chat.example.com/api/v1/ws"
        );
    }

    #[test]
    fn nonces_are_valid() {
        let n = new_nonce();
        assert!(agora_core::validate::nonce(&n).is_ok());
        assert_ne!(n, new_nonce());
    }
}
