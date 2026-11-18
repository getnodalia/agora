-- A device outlives its session: logging out and back in on the same computer keeps the same
-- device, so it stays in its conversations and keeps reading them. A device without a session
-- still counts for `DETACHED_DEVICE_DAYS` after it was last seen, then drops out of
-- conversations; its owner can also remove it at any time.
--
-- SQLite cannot change a foreign key in place, and dropping `devices` would cascade into the
-- tables that point at it, so their rows are kept aside while the table is rebuilt.
CREATE TABLE key_packages_keep AS SELECT * FROM key_packages;
CREATE TABLE welcomes_keep AS SELECT * FROM welcomes;

CREATE TABLE devices_v2 (
    public_key      BLOB    PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id      INTEGER UNIQUE REFERENCES sessions(id) ON DELETE SET NULL,
    created_at      INTEGER NOT NULL,
    last_seen_at    INTEGER NOT NULL
);
INSERT INTO devices_v2 (public_key, user_id, session_id, created_at, last_seen_at)
    SELECT public_key, user_id, session_id, created_at, created_at FROM devices;
DROP TABLE devices;
ALTER TABLE devices_v2 RENAME TO devices;
CREATE INDEX devices_user ON devices(user_id);

INSERT INTO key_packages SELECT * FROM key_packages_keep;
INSERT INTO welcomes SELECT * FROM welcomes_keep;
DROP TABLE key_packages_keep;
DROP TABLE welcomes_keep;
