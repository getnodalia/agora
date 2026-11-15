-- Ágora's first schema. Migrations are append-only: never edit one that has shipped.
-- Timestamps are milliseconds since the Unix epoch (UTC).

CREATE TABLE users (
    id              INTEGER PRIMARY KEY,
    username        TEXT    NOT NULL UNIQUE,
    display_name    TEXT    NOT NULL,
    password_hash   TEXT    NOT NULL,
    created_at      INTEGER NOT NULL
);

-- Only a SHA-256 of each session token is stored, so a copy of the database cannot be replayed.
CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY,
    token_hash      BLOB    NOT NULL UNIQUE,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    last_seen_at    INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);

CREATE TABLE workspaces (
    id              INTEGER PRIMARY KEY,
    name            TEXT    NOT NULL,
    created_by      INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at      INTEGER NOT NULL
);

CREATE TABLE workspace_members (
    workspace_id    INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT    NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'guest')),
    joined_at       INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, user_id)
);
CREATE INDEX workspace_members_user ON workspace_members(user_id);

CREATE TABLE invites (
    id              INTEGER PRIMARY KEY,
    workspace_id    INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    code_hash       BLOB    NOT NULL UNIQUE,
    role            TEXT    NOT NULL CHECK (role IN ('member', 'guest')),
    created_by      INTEGER REFERENCES users(id) ON DELETE SET NULL,
    max_uses        INTEGER NOT NULL,
    uses            INTEGER NOT NULL DEFAULT 0,
    expires_at      INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
);

CREATE TABLE channels (
    id              INTEGER PRIMARY KEY,
    workspace_id    INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT    NOT NULL,
    topic           TEXT    NOT NULL DEFAULT '',
    kind            TEXT    NOT NULL CHECK (kind IN ('public', 'private', 'direct')),
    archived        INTEGER NOT NULL DEFAULT 0,
    -- New members of the workspace join default channels automatically (#general).
    is_default      INTEGER NOT NULL DEFAULT 0,
    -- For a direct conversation: "<smaller user id>:<larger user id>", unique per workspace.
    direct_key      TEXT,
    created_by      INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX channels_name ON channels(workspace_id, name) WHERE kind <> 'direct';
CREATE UNIQUE INDEX channels_direct ON channels(workspace_id, direct_key) WHERE kind = 'direct';

CREATE TABLE channel_members (
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined_at       INTEGER NOT NULL,
    last_read_id    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, user_id)
);
CREATE INDEX channel_members_user ON channel_members(user_id);

CREATE TABLE messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    author_id       INTEGER NOT NULL REFERENCES users(id),
    thread_root_id  INTEGER REFERENCES messages(id) ON DELETE CASCADE,
    body            TEXT    NOT NULL,
    client_nonce    TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    edited_at       INTEGER,
    deleted_at      INTEGER,
    reply_count     INTEGER NOT NULL DEFAULT 0,
    last_reply_at   INTEGER
);
-- Retrying a send with the same nonce must never create a second message.
CREATE UNIQUE INDEX messages_nonce ON messages(author_id, client_nonce);
CREATE INDEX messages_channel ON messages(channel_id, thread_root_id, id);
CREATE INDEX messages_thread ON messages(thread_root_id, id) WHERE thread_root_id IS NOT NULL;

CREATE TABLE mentions (
    message_id      INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, user_id)
);
CREATE INDEX mentions_user ON mentions(user_id, channel_id, message_id);

CREATE TABLE reactions (
    message_id      INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji           TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (message_id, user_id, emoji)
);
