-- Files shared in a conversation. `data` is ciphertext: the sender's device encrypts each file
-- with a fresh key that travels inside the end-to-end encrypted message, so the server never
-- holds a key. Deleting the channel deletes its files.
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    uploader_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    size INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    data BLOB NOT NULL
);
CREATE INDEX files_channel ON files(channel_id);
