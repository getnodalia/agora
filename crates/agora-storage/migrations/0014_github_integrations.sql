-- GitHub joins Hito and plain webhooks as a kind of integration. SQLite cannot widen a CHECK in
-- place, so both tables are rebuilt with the new list. The notices go first on the way out
-- because they reference the integrations, which keeps the cascade from touching them.
CREATE TABLE integrations_new (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('hito', 'webhook', 'github')),
    name TEXT NOT NULL,
    secret BLOB NOT NULL CHECK(length(secret) = 32),
    created_by INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
INSERT INTO integrations_new (id, channel_id, kind, name, secret, created_by, created_at)
    SELECT id, channel_id, kind, name, secret, created_by, created_at FROM integrations;

CREATE TABLE integration_notices_new (
    id INTEGER PRIMARY KEY,
    integration_id INTEGER NOT NULL REFERENCES integrations_new(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
    title TEXT,
    text TEXT NOT NULL,
    url TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(integration_id, payload_hash)
);
INSERT INTO integration_notices_new
        (id, integration_id, channel_id, payload_hash, title, text, url, created_at)
    SELECT id, integration_id, channel_id, payload_hash, title, text, url, created_at
    FROM integration_notices;

DROP TABLE integration_notices;
DROP TABLE integrations;
ALTER TABLE integrations_new RENAME TO integrations;
ALTER TABLE integration_notices_new RENAME TO integration_notices;
CREATE INDEX integrations_channel ON integrations(channel_id);
CREATE INDEX integration_notices_channel ON integration_notices(channel_id, id DESC);
