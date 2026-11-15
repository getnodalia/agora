-- End-to-end encryption (MLS, RFC 9420). The server keeps ciphertext, epochs and the latest
-- public GroupInfo of each conversation; it never has a key that opens a message.

-- A device is one signed-in client's MLS signing key. It lives and dies with its session (a
-- device that signs in again after its session expired moves to the new one).
CREATE TABLE devices (
    public_key      BLOB    PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id      INTEGER NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
    created_at      INTEGER NOT NULL
);
CREATE INDEX devices_user ON devices(user_id);

-- One-use MLS key packages a device publishes, so others can add it while it is offline.
CREATE TABLE key_packages (
    id              INTEGER PRIMARY KEY,
    device_key      BLOB    NOT NULL REFERENCES devices(public_key) ON DELETE CASCADE,
    data            BLOB    NOT NULL,
    created_at      INTEGER NOT NULL
);
CREATE INDEX key_packages_device ON key_packages(device_key, id);

-- The Welcome that let a device into a conversation, waiting for it to come online.
CREATE TABLE welcomes (
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    device_key      BLOB    NOT NULL REFERENCES devices(public_key) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    data            BLOB    NOT NULL,
    PRIMARY KEY (channel_id, device_key)
);

-- One MLS group per channel or direct conversation.
CREATE TABLE mls_groups (
    channel_id      INTEGER PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
    epoch           INTEGER NOT NULL,
    group_info      BLOB    NOT NULL,
    updated_at      INTEGER NOT NULL
);

-- Each conversation's encrypted log, in the one order every device must process it: commits
-- (devices joining and leaving) and application messages (new messages and edits).
CREATE TABLE mls_log (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id      INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    epoch           INTEGER NOT NULL,
    kind            TEXT    NOT NULL CHECK (kind IN ('commit', 'message')),
    author_id       INTEGER NOT NULL REFERENCES users(id),
    message_id      INTEGER REFERENCES messages(id) ON DELETE CASCADE,
    data            BLOB    NOT NULL,
    created_at      INTEGER NOT NULL
);
CREATE INDEX mls_log_channel ON mls_log(channel_id, seq);
CREATE INDEX mls_log_message ON mls_log(message_id) WHERE message_id IS NOT NULL;

-- Messages written before encryption keep their body; encrypted ones keep it empty.
ALTER TABLE messages ADD COLUMN encrypted INTEGER NOT NULL DEFAULT 0;
