-- Incoming webhooks: other apps (Hito, scripts, CI) post notices to a channel. Notices are
-- server-readable and kept apart from end-to-end encrypted messages.
CREATE TABLE integrations (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('hito', 'webhook')),
    name TEXT NOT NULL,
    -- The shared secret that signs requests. The server needs it to check signatures.
    secret BLOB NOT NULL CHECK(length(secret) = 32),
    created_by INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
CREATE INDEX integrations_channel ON integrations(channel_id);

CREATE TABLE integration_notices (
    id INTEGER PRIMARY KEY,
    integration_id INTEGER NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    -- A replayed request has the same body: it is recorded once.
    payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
    title TEXT,
    text TEXT NOT NULL,
    url TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(integration_id, payload_hash)
);
CREATE INDEX integration_notices_channel ON integration_notices(channel_id, id DESC);
