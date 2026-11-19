-- Getting back into an account without email: a recovery key the person keeps, and a one-use code
-- the person who runs the server can issue. Only hashes are stored; both are replaced or removed
-- when used.
CREATE TABLE recovery_keys (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    key_hash BLOB NOT NULL CHECK (length(key_hash) = 32),
    created_at INTEGER NOT NULL
);
CREATE TABLE reset_codes (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    code_hash BLOB NOT NULL CHECK (length(code_hash) = 32),
    expires_at INTEGER NOT NULL,
    remove_two_step INTEGER NOT NULL DEFAULT 0
);
