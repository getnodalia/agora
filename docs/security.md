# Security model

What Ágora protects, how, and what it does not do. Report a vulnerability as
[SECURITY.md](../SECURITY.md) says, never in a public issue.

## What Ágora is not

- **No end-to-end encryption.** The server stores every message in clear text in its database and
  can read it, as in Slack, Teams or Discord. Whoever runs the server, and whoever gets a copy of
  its database or backups, can read the conversations. Do not describe Ágora as end-to-end
  encrypted. If it is ever added, it will use an audited, standard protocol (such as MLS), never
  an algorithm of our own.
- **No transport encryption of its own.** The server speaks plain HTTP and WebSocket and expects a
  TLS reverse proxy in front of it ([deployment.md](deployment.md)). It refuses to listen on a
  non-loopback address unless `trust_proxy` (a TLS proxy is in front) or
  `allow_insecure_public_bind` (an isolated lab network) is set, and the desktop app warns on the
  sign-in screen when the address is `http://` and not on this computer.

## Accounts and sessions

- Passwords are hashed with **Argon2id** (the `argon2` crate's defaults: 19 MiB, 2 passes, a
  random salt each), on Tokio's blocking pool so hashing never stalls the server. Passwords are
  10 to 256 characters.
- A sign-in that names an unknown account still verifies against a dummy hash, so the response
  time does not tell which usernames exist.
- **Session tokens** are 32 random bytes from the operating system (`OsRng`). The database keeps
  only their SHA-256; a stolen database cannot be used to sign in. Sessions expire
  (`session_days`, 30 by default), and **Log out** deletes the session and closes its WebSockets
  at once.
- **Invitation codes** are random too, stored as SHA-256, limited in uses and time.
- The desktop app keeps a session only when "Keep me signed in" is ticked, in `session.json` in
  its config folder, created with mode `0600` on Unix and written atomically. Log out deletes it.

## Authorisation and isolation

- Every HTTP request but registration, sign-in and server info needs `Authorization: Bearer`.
- The WebSocket at `/api/v1/ws` authenticates with its first frame within a few seconds (a web
  client cannot set headers on a WebSocket). Until then nothing is sent; a bad token gets one
  error and close code 4001. The session is checked again every minute.
- Every read and write checks membership: a workspace's channels, members and messages are only
  visible to its members, and a private channel or direct message only to its members. A request
  for something you cannot see answers 404, the same as for something that does not exist.
  Real-time events go only to the people who could read the same thing over HTTP.
- Roles (owner, admin, member, guest) decide who may create channels, invite, archive and delete
  other people's messages. The rules live in one place, `agora_core::roles`, with tests.

## Input and output

- Every input is validated (`agora_core::validate`): names, lengths, Unicode control characters.
  Message bodies are at most 8000 characters, request bodies 64 KiB, WebSocket frames 16 KiB.
- All SQL goes through SQLx with bound parameters; no query is built from strings.
- Messages are stored as the text the person typed. The desktop app renders a small Markdown subset
  itself and never interprets HTML; links open only on a click. Responses carry
  `X-Content-Type-Options: nosniff` and `Cache-Control: no-store`, ready for a web client.
- A message carries a client nonce: a retry after a lost reply returns the stored message instead of
  posting it twice.

## Abuse limits

Per minute: sign-in and registration attempts per address (10), writes per account (120), reads
per account (1200), typing notices (30); at most 16 WebSockets per account. All are in
`agora.toml` (`[limits]`). Behind a proxy, set `trust_proxy = true` so the address comes from
`X-Forwarded-For`; without a proxy that header is ignored.

## Logs

The server logs events with ids (`account created user=2`), never passwords, tokens, invitation
codes or message text. At `debug` level the HTTP trace prints request paths, which include search
terms; keep production at `info` (the default).

## Known limits

- No two-factor sign-in, password reset by email or account deletion yet.
- Rate limits are kept in memory per server process.
- A server administrator can read the database: see "What Ágora is not".
