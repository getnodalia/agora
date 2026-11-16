-- Existing installations with users are already initialized. Never rewrite older migrations.
CREATE TABLE server_initialization (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    initialized INTEGER NOT NULL CHECK (initialized IN (0, 1)),
    token_hash BLOB,
    expires_at INTEGER
);
INSERT INTO server_initialization (id, initialized)
SELECT 1, CASE WHEN EXISTS (SELECT 1 FROM users) THEN 1 ELSE 0 END;
