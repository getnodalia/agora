-- External events are server-readable metadata, separate from encrypted conversation messages.
CREATE TABLE github_events (
    id INTEGER PRIMARY KEY,
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    delivery TEXT NOT NULL,
    payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
    event TEXT NOT NULL,
    action TEXT,
    object_number INTEGER,
    received_at INTEGER NOT NULL,
    UNIQUE(workspace_id, delivery),
    UNIQUE(workspace_id, payload_hash)
);
CREATE INDEX github_events_workspace ON github_events(workspace_id, id DESC);

-- Keep replay markers when old display records are pruned. These grow with authenticated
-- deliveries until the workspace is deleted; they contain no raw payload or secret.
CREATE TABLE github_event_seen (
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    delivery TEXT NOT NULL,
    payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
    PRIMARY KEY(workspace_id, delivery),
    UNIQUE(workspace_id, payload_hash)
);
