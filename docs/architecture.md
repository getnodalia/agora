# Architecture

Ágora is a Cargo workspace. Dependencies point one way: the apps depend on the crates, and the
crates below never depend on a user interface.

```text
agora-core        ids, roles and permissions, limits, validation, the message text format
   ↑
agora-protocol    the wire contract: JSON types for HTTP and the WebSocket (API v1)
   ↑            ↑
agora-storage   agora-client    SQLite storage (SQLx, migrations) · HTTP + WebSocket client
   ↑            ↑
agora-server    agora-desktop   Tokio + Axum server · egui + nodalia-theme desktop app
```

## Server (`apps/agora-server`)

- **HTTP API** under `/api/v1` (Axum). Every handler validates its input with `agora-core`,
  checks the caller's role and channel membership, then calls `agora-storage`.
- **Real time**: one WebSocket per client at `/api/v1/ws`. The first frame must authenticate
  (`{"type":"auth","token":…}`) within 10 seconds. A hub keeps, per user, the open connections and
  their bounded send queues; a slow client is disconnected rather than allowed to grow memory.
  Events are sent only to the people who may read the channel they concern.
- **Storage**: SQLite in WAL mode through SQLx with embedded migrations
  (`crates/agora-storage/migrations`). All queries are parameterised. The schema avoids
  SQLite-only features where it can so a PostgreSQL backend can follow.
- **Limits**: token-bucket rate limits per address (sign-in) and per account (reads, writes,
  typing notices), body and frame size limits, connection caps per account.
- **Shutdown** closes every WebSocket with "service restart" so clients reconnect to the next run.

## Client library (`crates/agora-client`)

`Client` wraps every endpoint as an async method returning protocol types. `connect` runs the
WebSocket on the current Tokio runtime: it authenticates, sends heartbeats, reconnects with
exponential backoff and jitter, and stops only when the server rejects the session.

## Desktop app (`apps/agora-desktop`)

- `net.rs` runs requests on a two-thread Tokio runtime; each finished request becomes a `Reply`
  in a channel, and the window repaints. The window thread never waits on the network.
- `model.rs` is the signed-in state and a reducer for server events (pure, unit-tested): messages
  land once and in order, the optimistic copy of a sent message is replaced by the server's,
  unread counts only grow for what nobody has seen, threads and reactions update every copy.
- `view.rs`, `signin.rs`, `dialogs.rs` draw with egui and `nodalia_theme` (palette, creator-suite
  islands, buttons, fields, icons, window controls) and return `Action`s that `app.rs` carries
  out after the frame.
- Settings and the optional saved session live in the config folder; the session file is
  written owner-only.

## Message text

Messages are stored exactly as typed. Clients parse a small Markdown subset (`agora_core::format`):
`**bold**`, `*italic*`, `~~strike~~`, `` `code` ``, fenced code blocks, `> quotes`, `@mentions` and
`http`/`https`/`mailto` links. There is no HTML anywhere, so there is nothing to sanitise.
