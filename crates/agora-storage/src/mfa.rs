//! Atomic MFA persistence primitives. No route calls them yet; they do not enforce MFA alone.

use crate::{Result, Store, StoreError};
use agora_core::UserId;

const SEALED_SECRET_LEN: usize = 49;
const RECOVERY_COUNT: usize = 10;
const SESSION_HASH_LEN: usize = 32;

impl Store {
    /// Store an already-verified factor and recovery hashes in one transaction. The caller must
    /// verify TOTP and encrypt the secret before calling; plaintext and keys never belong here.
    /// Existing factors are never overwritten. This does not revoke or restrict old sessions.
    pub async fn enroll_totp(
        &self,
        user: UserId,
        sealed_secret: &[u8],
        verified_step: i64,
        recovery_hashes: &[[u8; 32]],
        now: i64,
    ) -> Result<()> {
        if sealed_secret.len() != SEALED_SECRET_LEN
            || sealed_secret.first() != Some(&1)
            || verified_step < 0
            || recovery_hashes.len() != RECOVERY_COUNT
        {
            return Err(StoreError::InvalidMfa);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO mfa_totp (user_id, sealed_secret, last_step, enrolled_at) VALUES (?, ?, ?, ?)")
            .bind(user.0).bind(sealed_secret).bind(verified_step).bind(now).execute(&mut *tx).await.map_err(crate::conflict)?;
        for hash in recovery_hashes {
            sqlx::query("INSERT INTO mfa_recovery_codes (user_id, code_hash) VALUES (?, ?)")
                .bind(user.0)
                .bind(hash.as_slice())
                .execute(&mut *tx)
                .await
                .map_err(crate::conflict)?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Return the encrypted factor and last accepted step, never the plaintext secret.
    pub async fn totp_factor(&self, user: UserId) -> Result<Option<(Vec<u8>, i64)>> {
        Ok(
            sqlx::query_as("SELECT sealed_secret, last_step FROM mfa_totp WHERE user_id = ?")
                .bind(user.0)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Consume a verified step and issue the session in the same transaction. The caller must
    /// authenticate the password, verify the TOTP against this user's factor, apply rate limits
    /// and ensure the presented step is legitimate before calling.
    pub async fn consume_totp_and_create_session(
        &self,
        user: UserId,
        step: i64,
        session_hash: &[u8],
        now: i64,
        expires_at: i64,
    ) -> Result<Option<i64>> {
        if step < 0 || session_hash.len() != SESSION_HASH_LEN || expires_at <= now {
            return Err(StoreError::InvalidMfa);
        }
        let mut tx = self.pool.begin().await?;
        let changed =
            sqlx::query("UPDATE mfa_totp SET last_step = ? WHERE user_id = ? AND last_step < ?")
                .bind(step)
                .bind(user.0)
                .bind(step)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        let id = sqlx::query_scalar("INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at) VALUES (?, ?, ?, ?, ?) RETURNING id")
            .bind(session_hash).bind(user.0).bind(now).bind(expires_at).bind(now).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(id))
    }

    /// Consume a recovery hash and issue a session atomically; an invalid or already consumed
    /// code returns `None`. The caller must authenticate the password and apply rate limits.
    pub async fn consume_recovery_and_create_session(
        &self,
        user: UserId,
        recovery_hash: &[u8; 32],
        session_hash: &[u8],
        now: i64,
        expires_at: i64,
    ) -> Result<Option<i64>> {
        if session_hash.len() != SESSION_HASH_LEN || expires_at <= now {
            return Err(StoreError::InvalidMfa);
        }
        let mut tx = self.pool.begin().await?;
        let changed =
            sqlx::query("DELETE FROM mfa_recovery_codes WHERE user_id = ? AND code_hash = ?")
                .bind(user.0)
                .bind(recovery_hash.as_slice())
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        let id = sqlx::query_scalar("INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at) VALUES (?, ?, ?, ?, ?) RETURNING id")
            .bind(session_hash).bind(user.0).bind(now).bind(expires_at).bind(now).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(id))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, Store, UserId) {
        let dir = tempfile::tempdir().unwrap();
        let db = format!("sqlite://{}", dir.path().join("mfa.db").display());
        let store = Store::open(&db).await.unwrap();
        let user = store.create_user("ana", "Ana", "hash", 1).await.unwrap();
        (dir, store, user.id)
    }
    fn recovery_hashes() -> Vec<[u8; 32]> {
        (0..10).map(|n| [n; 32]).collect()
    }
    fn sealed() -> [u8; 49] {
        [1; 49]
    }

    #[tokio::test]
    async fn upgrade_preserves_existing_account_and_session() {
        let dir = tempfile::tempdir().unwrap();
        let migrations = dir.path().join("old-migrations");
        std::fs::create_dir(&migrations).unwrap();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        for name in [
            "0001_initial.sql",
            "0002_end_to_end.sql",
            "0003_server_initialization.sql",
            "0004_tasks.sql",
            "0005_organizations.sql",
        ] {
            std::fs::copy(source.join(name), migrations.join(name)).unwrap();
        }
        let db = format!("sqlite://{}", dir.path().join("old.db").display());
        let options = db
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .unwrap()
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate::Migrator::new(migrations.as_path())
            .await
            .unwrap()
            .run(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES (1, 'ana', 'Ana', 'hash', 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at) VALUES (?, 1, 1, 200, 1)")
            .bind([9u8; 32].as_slice()).execute(&pool).await.unwrap();
        pool.close().await;

        let store = Store::open(&db).await.unwrap();
        assert!(store.totp_factor(UserId(1)).await.unwrap().is_none());
        let session_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE user_id = 1 AND token_hash = ?",
        )
        .bind([9u8; 32].as_slice())
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(session_count, 1);
        store.close().await;
    }

    #[tokio::test]
    async fn enrollment_is_atomic_and_does_not_replace_existing_factors() {
        let (_dir, store, user) = fixture().await;
        let duplicates = [[7u8; 32]; 10];
        assert!(store
            .enroll_totp(user, &sealed(), 10, &duplicates, 1)
            .await
            .is_err());
        assert!(store.totp_factor(user).await.unwrap().is_none());
        assert!(matches!(
            store
                .enroll_totp(user, &[1; 48], 10, &recovery_hashes(), 1)
                .await,
            Err(StoreError::InvalidMfa)
        ));
        store
            .enroll_totp(user, &sealed(), 10, &recovery_hashes(), 1)
            .await
            .unwrap();
        assert!(store
            .enroll_totp(user, &sealed(), 20, &recovery_hashes(), 2)
            .await
            .is_err());
        assert_eq!(
            store.totp_factor(user).await.unwrap(),
            Some((sealed().to_vec(), 10))
        );
        store.close().await;
    }

    #[tokio::test]
    async fn totp_step_is_consumed_once_even_for_concurrent_sessions() {
        let (_dir, store, user) = fixture().await;
        store
            .enroll_totp(user, &sealed(), 10, &recovery_hashes(), 1)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            store.consume_totp_and_create_session(user, 11, &[41; 32], 100, 200),
            store.consume_totp_and_create_session(user, 11, &[42; 32], 100, 200)
        );
        assert_ne!(a.unwrap().is_some(), b.unwrap().is_some());
        assert_eq!(store.totp_factor(user).await.unwrap().unwrap().1, 11);
        assert!(store
            .consume_totp_and_create_session(user, 10, &[43; 32], 100, 200)
            .await
            .unwrap()
            .is_none());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = ?")
            .bind(user.0)
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        store.close().await;
    }

    #[tokio::test]
    async fn totp_step_is_not_consumed_if_session_insertion_fails() {
        let (_dir, store, user) = fixture().await;
        store
            .enroll_totp(user, &sealed(), 10, &recovery_hashes(), 1)
            .await
            .unwrap();
        store.create_session(&[9; 32], user, 1, 200).await.unwrap();
        assert!(store
            .consume_totp_and_create_session(user, 11, &[9; 32], 100, 200)
            .await
            .is_err());
        assert_eq!(store.totp_factor(user).await.unwrap().unwrap().1, 10);
        assert!(store
            .consume_totp_and_create_session(user, 11, &[10; 32], 100, 200)
            .await
            .unwrap()
            .is_some());
        store.close().await;
    }

    #[tokio::test]
    async fn recovery_consumption_and_session_creation_roll_back_together() {
        let (_dir, store, user) = fixture().await;
        store
            .enroll_totp(user, &sealed(), 10, &recovery_hashes(), 1)
            .await
            .unwrap();
        store.create_session(&[9; 32], user, 1, 200).await.unwrap();
        // A duplicate session token must not consume the recovery code.
        assert!(store
            .consume_recovery_and_create_session(user, &[3; 32], &[9; 32], 100, 200)
            .await
            .is_err());
        let (a, b) = tokio::join!(
            store.consume_recovery_and_create_session(user, &[3; 32], &[10; 32], 100, 200),
            store.consume_recovery_and_create_session(user, &[3; 32], &[11; 32], 100, 200)
        );
        assert_ne!(a.unwrap().is_some(), b.unwrap().is_some());
        assert!(store
            .consume_recovery_and_create_session(UserId(user.0 + 1), &[4; 32], &[12; 32], 100, 200)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .consume_recovery_and_create_session(user, &[4; 32], &[13; 32], 100, 200)
            .await
            .unwrap()
            .is_some());
        store.close().await;
    }
}
