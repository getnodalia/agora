-- Storage foundation only. No session restrictions or MFA sign-in are enabled by this migration.
CREATE TABLE mfa_totp (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    sealed_secret BLOB NOT NULL CHECK (length(sealed_secret) = 49),
    last_step INTEGER NOT NULL CHECK (last_step >= 0),
    enrolled_at INTEGER NOT NULL
);
CREATE TABLE mfa_recovery_codes (
    user_id INTEGER NOT NULL REFERENCES mfa_totp(user_id) ON DELETE CASCADE,
    code_hash BLOB NOT NULL CHECK (length(code_hash) = 32),
    PRIMARY KEY (user_id, code_hash)
);
