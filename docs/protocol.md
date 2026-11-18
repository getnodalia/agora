# Protocol (API v1)

JSON over HTTP under `/api/v1`, and a WebSocket at `/api/v1/ws`. The types are in
`crates/agora-protocol`; fields are only ever added (with defaults) within a version.

Authenticated requests carry `Authorization: Bearer <token>`. Errors are
`{"code": "...", "message": "...", "field": "..."}` with `code` one of `invalid`,
`unauthenticated`, `forbidden`, `not_found`, `conflict`, `rate_limited`, `registration_closed`,
`internal`, `stale_epoch`. A channel the caller may not read answers `404`, not `403`, so ids reveal nothing.

## Endpoints

| Method and path | What |
|---|---|
| `GET /server` | Name, version, API version, registration policy (no auth) |
| `POST /auth/register` | `{username, display_name, password, invite_code?}` → session |
| `POST /auth/login` | `{username, password, totp?, recovery_code?}` → `{token, expires_at, user}`; `mfa_required` when the account has two-step verification and no valid code was sent |
| `GET /auth/mfa` | `{enabled, recovery_codes_left}` for the caller |
| `POST /auth/mfa/setup` | `{password}` → `{secret, uri}`: a new authenticator secret, pending for 10 minutes |
| `POST /auth/mfa/enable` | `{code}` → `{recovery_codes}` (shown once); other sessions are signed out |
| `POST /auth/mfa/disable` | `{password, code}` |
| `POST /auth/mfa/recovery-codes` | `{code}` → `{recovery_codes}`: replaces the old ones |
| `POST /auth/logout` | Ends this session and closes its sockets |
| `GET /me`, `PATCH /me` | The signed-in user; change the display name |
| `GET /workspaces`, `POST /workspaces` | Workspaces the caller belongs to; create one (with `#general`) |
| `POST /workspaces/join` | `{code}`: redeem an invitation |
| `GET /workspaces/{id}` | One workspace |
| `GET /workspaces/{id}/members` | Members, with whether they are online |
| `POST /workspaces/{id}/invites` | `{max_uses?, expires_in_hours?, role?}` → the code, shown once (admins) |
| `GET /workspaces/{id}/channels`, `POST …` | Channels the caller can see, with unread and mention counts; create one |
| `POST /workspaces/{id}/direct` | `{user_id}`: open (or find) the direct conversation |
| `GET /workspaces/{id}/search?q=` | Messages matching `q` in channels the caller can read (only unencrypted messages from before end-to-end encryption; clients search their own decrypted copy) |
| `POST /workspaces/{id}/messages/lookup` | `{ids}`: the messages with these ids the caller can read (what a local search found) |
| `GET /channels/{id}`, `PATCH …` | One channel; rename, set the topic, archive |
| `POST /channels/{id}/join`, `/leave` | Join a public channel; leave a channel |
| `POST /channels/{id}/members` | `{user_id}`: add someone to a private channel |
| `POST /channels/{id}/read` | `{message_id}`: move the read marker (reading counts as received) |
| `POST /channels/{id}/delivered` | `{message_id}`: one of the caller's devices received the messages up to this one |
| `GET /channels/{id}/receipts` | `[{user_id, delivered_id, read_id}]`: how far each member has received and read the channel |
| `POST /channels/{id}/files` | Raw body: a file the client already encrypted → `201 {id, size}`. At most `max_file_bytes` (100 MB by default) |
| `GET /files/{id}` | The encrypted bytes of a file, for anyone who can read its channel |
| `GET /channels/{id}/messages?before=&after=&limit=` | History, oldest first, top-level messages only |
| `POST /channels/{id}/messages` | `{ciphertext, mentions, client_nonce, thread_root_id?}`, `body` empty; a repeated nonce returns the stored message |
| `PATCH /messages/{id}`, `DELETE …` | Edit (author, `{ciphertext, mentions}`) or delete (author or moderator) |
| `GET /messages/{id}/thread` | A root message and its replies |
| `POST /messages/{id}/reactions` | `{emoji}`: toggle the caller's reaction |
| `GET /channels/{id}/integrations`, `POST …` | The apps that post to a channel; add one (`{kind, name}`, admins) → `{integration, path, secret}`, the secret shown once |
| `DELETE /integrations/{id}` | Remove one (admins) |
| `GET /channels/{id}/notices` | The newest 50 notices integrations posted, oldest first |
| `POST /hooks/{id}` | No session: a notice from another app, signed with the integration's key (see [integrations.md](integrations.md)) |

### End-to-end encryption

| Method and path | What |
|---|---|
| `GET /devices` | The caller's devices: `[{public_key, created_at, last_seen_at, this_device, signed_in}]` |
| `POST /devices/remove` | `{public_key}`: remove one of the caller's devices and sign it out |
| `PUT /devices/me` | `{public_key}`: this session's device (its MLS signature key) → `{key_packages}` left |
| `POST /devices/me/key_packages` | `{key_packages}`: publish MLS key packages of this device |
| `GET /channels/{id}/mls?after=` | The conversation's epoch, latest GroupInfo, the devices that may read it, the log after `after`, and a Welcome waiting for this device |
| `POST /channels/{id}/mls/group` | `{group_info}`: start the conversation's group at epoch 0 (`409` if it exists) |
| `POST /channels/{id}/mls/commit` | `{commit, group_info, welcome?, welcome_to}` made at the current epoch → `{seq, epoch}`; `stale_epoch` if another commit came first |
| `POST /channels/{id}/mls/key_packages` | `{devices}`: claim one key package of each device that may read the conversation |

Binary fields are base64. The server checks group ids, epochs and that a key package belongs to
the device publishing it, which it can do without any key; it never decrypts. A message or edit
whose ciphertext is not an MLS application message of this conversation's group at its current
epoch is refused.

A file travels in two parts. The client encrypts it with a fresh AES-256-GCM key, uploads the
result, and then sends an ordinary end-to-end encrypted message whose text names the upload,
the file name, size and type, and the key. A message can also carry a copy of earlier messages
for people who joined after them. The formats of both are in `agora_core::content`.

`GET /healthz` (outside the prefix) answers `ok` for load balancers.

## WebSocket

Client frames: `auth {token}` (first, within 10 s), `typing {channel_id, thread_root_id?}`, `ping`,
and for calls `call_join {channel_id, audio, video}`, `call_update {channel_id, audio, video}` and
`call_leave {channel_id}`.

Server events: `ready {user}`, `error {code, message}`, `message_created`, `message_updated`,
`message_deleted`, `reactions_changed`, `channel_changed`, `channel_removed`, `member_joined`,
`read_marker`, `receipt` (a member received or read more of a conversation), `integration_notice`, `typing`, `presence`, `mls_changed` (a conversation's group moved: catch up),
`call_changed {channel_id, participants}` (who is in a channel's call; empty means it ended; sent
to everyone who can read the channel, and after `ready` for every call going on) and `call_left
{channel_id, reason}` (`other_device`, `full` or `no_access`: this connection is not in the call),
`pong`.

### Calls

Only channel members who can post there may join its call, and a connection is in at most one
call; joining from a second device moves the person there. Audio and video travel as binary
WebSocket frames (`agora_protocol::media`): a client sends `[1][kind][channel id, i64 big-endian]
[payload]`, with kind `1` audio and `2` video, and the server relays
`[1][kind][channel id][sender user id, i64 big-endian][payload]` to the others in the call. A
payload is at most 60 KiB, and each connection may send `call_bytes_per_second` (1 MiB by
default); frames over that, malformed, or for a call the connection is not in are dropped
without an answer. The server never reads payloads: clients seal them end to end
(`agora_e2e::call`), with AES-256-GCM under a key exported from the conversation's MLS group
(label `agora call v1`, context the channel id), as `[epoch, u64][nonce, 12 bytes][ciphertext of
sequence, u64, and media]`, the channel, sender, kind and epoch as associated data.

What the media is is up to the clients; the server never looks. The desktop app sends
audio as 16 kHz mono µ-law in 20 ms frames (the frame's kind says audio) and video as one JPEG per frame, 320 pixels wide at about 10 frames per second, so a
frame fits well under the payload limit.

Close codes: `4001` the session is invalid (do not retry), `1011` server error and `1012` server
restart (retry with backoff). After reconnecting, a client fetches history `after` the last
message it has, which is how nothing is lost while it was away.

### Organizations (experimental API)

`POST /organizations` creates an organization and makes its creator its owner; `GET /organizations`
lists those the caller belongs to. The owner alone may list organization members, assign an existing
account a non-owner organization role with `PUT /organizations/{id}/members/{user}`, and create a
workspace with `POST /organizations/{id}/workspaces`. The owner role cannot be reassigned through
this API. `GET /organizations/{id}/workspaces` returns only workspaces the caller already belongs to;
organization membership alone never grants access to a workspace or its channels. Organization and
workspace roles are independent: changing one does not change the other or revoke existing workspace
access. For an organization workspace, only the organization owner can issue workspace invitations;
redeeming one adds the invitee to the organization as a member or guest without overwriting an
existing organization role. Existing standalone workspaces remain standalone after migration.

The desktop Organizations dialog can create organizations and their workspaces, select an accessible
workspace and change the organization role of an existing organization member. Invitations to an
organization workspace add the invitee to the organization as described above. Workspace role
management, member removal, ownership transfer and moving existing workspaces into an organization
are not implemented.

In this initial implementation only the owner has organization-management privileges; admin, member
and guest organization roles do not yet differ in their organization-level permissions.

### Workspace tasks

The previous experimental server-readable task endpoints are disabled while an end-to-end encrypted
replacement is developed. They return 404. Existing task rows are preserved in the database and
remain server-readable; disabling the API does not encrypt or delete those rows. There is no task
API or task UI in this version. No automatic conversion of earlier data is performed.
