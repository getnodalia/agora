//! Second-factor verification for sign-in, and the server's sealing key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use agora_core::UserId;
use agora_storage::Store;
use rand::{rngs::OsRng, RngCore};
use zeroize::Zeroizing;

use crate::config::Config;
use crate::mfa_crypto::{self, MfaError, MfaKey};

/// How long a new secret waits for its first code.
const PENDING_MS: i64 = 10 * 60 * 1000;

type Pending = HashMap<UserId, (Zeroizing<[u8; 20]>, i64)>;

/// The sealing key and secrets shown to a person but not yet confirmed with a code.
pub struct Mfa {
    pub key: MfaKey,
    pending: Mutex<Pending>,
}

impl Mfa {
    /// Read the key file, or create it (owner-only) on first start. An in-memory database gets
    /// an in-memory key, which is right for tests and nothing else.
    pub fn load(config: &Config) -> std::io::Result<Self> {
        let path = config
            .mfa_key_file
            .clone()
            .or_else(|| default_key_path(&config.database));
        let key = match path {
            Some(path) => read_or_create(&path)?,
            None => random_key()?,
        };
        Ok(Self::with_key(MfaKey::new(key)))
    }

    pub fn with_key(key: MfaKey) -> Self {
        Self {
            key,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_pending(&self, user: UserId, secret: Zeroizing<[u8; 20]>, now: i64) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.retain(|_, (_, at)| now.saturating_sub(*at) < PENDING_MS);
        pending.insert(user, (secret, now));
    }

    /// The pending secret if it is still fresh. It stays until enrollment succeeds.
    pub fn pending(&self, user: UserId, now: i64) -> Option<Zeroizing<[u8; 20]>> {
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending
            .get(&user)
            .filter(|(_, at)| now.saturating_sub(*at) < PENDING_MS)
            .map(|(secret, _)| secret.clone())
    }

    pub fn clear_pending(&self, user: UserId) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&user);
    }
}

fn default_key_path(database: &str) -> Option<PathBuf> {
    let rest = database
        .strip_prefix("sqlite://")
        .or_else(|| database.strip_prefix("sqlite:"))?;
    let file = rest.split('?').next()?;
    if file.is_empty() || file.starts_with(':') {
        return None;
    }
    Some(PathBuf::from(format!("{file}.mfa-key")))
}

fn random_key() -> std::io::Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    OsRng
        .try_fill_bytes(key.as_mut())
        .map_err(std::io::Error::other)?;
    Ok(key)
}

fn read_or_create(path: &Path) -> std::io::Result<Zeroizing<[u8; 32]>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{} must hold exactly 32 bytes", path.display()),
                )
            })?;
            Ok(Zeroizing::new(key))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = random_key()?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            std::io::Write::write_all(&mut file, key.as_ref())?;
            file.sync_all()?;
            tracing::info!(path = %path.display(), "created the two-step verification key");
            Ok(key)
        }
        Err(e) => Err(e),
    }
}

/// Check a current code for a signed-in account (turning the factor off, new recovery codes).
/// Each code is accepted once.
pub async fn verify_current_code(
    store: &Store,
    key: &MfaKey,
    user: UserId,
    token: &str,
    now_ms: i64,
) -> Result<bool, Error> {
    let Ok(now_secs) = u64::try_from(now_ms / 1000) else {
        return Ok(false);
    };
    let Some((sealed, last_step)) = store.totp_factor(user).await? else {
        return Ok(false);
    };
    let secret = key.open(user, &sealed)?;
    let Some(step) =
        mfa_crypto::verify_totp(&secret, token, now_secs, u64::try_from(last_step).ok())?
    else {
        return Ok(false);
    };
    let Ok(step) = i64::try_from(step) else {
        return Ok(false);
    };
    Ok(store.consume_totp_step(user, step).await?)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MFA verification failed")]
    Crypto(#[from] MfaError),
    #[error("MFA storage failed")]
    Storage(#[from] agora_storage::StoreError),
}

/// The password must already have been verified and attempts rate-limited by the caller.
/// The caller must supply a freshly generated session-token hash and a trusted server time.
pub async fn verify_totp_and_create_session(
    store: &Store,
    key: &MfaKey,
    user: UserId,
    token: &str,
    session_hash: &[u8],
    now_ms: i64,
    expires_at: i64,
) -> Result<Option<i64>, Error> {
    let Ok(now_secs) = u64::try_from(now_ms / 1000) else {
        return Ok(None);
    };
    if now_ms < 0 {
        return Ok(None);
    }
    let Some((sealed, last_step)) = store.totp_factor(user).await? else {
        return Ok(None);
    };
    let secret = key.open(user, &sealed)?;
    let Some(step) =
        mfa_crypto::verify_totp(&secret, token, now_secs, u64::try_from(last_step).ok())?
    else {
        return Ok(None);
    };
    let Ok(step) = i64::try_from(step) else {
        return Ok(None);
    };
    Ok(store
        .consume_totp_and_create_session(user, step, session_hash, now_ms, expires_at)
        .await?)
}

/// Like TOTP verification, this requires a verified password and rate limiting from the caller.
/// A recovery token can create at most one session, even under concurrent requests.
pub async fn verify_recovery_and_create_session(
    store: &Store,
    user: UserId,
    token: &str,
    session_hash: &[u8],
    now_ms: i64,
    expires_at: i64,
) -> Result<Option<i64>, Error> {
    let Some(hash) = mfa_crypto::recovery_hash(user, token) else {
        return Ok(None);
    };
    Ok(store
        .consume_recovery_and_create_session(user, &hash, session_hash, now_ms, expires_at)
        .await?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn key_file_sits_next_to_the_database() {
        assert_eq!(
            default_key_path("sqlite://agora.db"),
            Some(PathBuf::from("agora.db.mfa-key"))
        );
        assert_eq!(
            default_key_path("sqlite:///var/lib/agora/agora.db?mode=rwc"),
            Some(PathBuf::from("/var/lib/agora/agora.db.mfa-key"))
        );
        assert_eq!(default_key_path("sqlite::memory:"), None);
    }

    #[test]
    fn key_file_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k");
        let a = read_or_create(&path).unwrap();
        let b = read_or_create(&path).unwrap();
        assert_eq!(*a, *b);
        std::fs::write(&path, b"short").unwrap();
        assert!(read_or_create(&path).is_err());
    }

    #[tokio::test]
    async fn verified_factor_is_bound_to_user_and_consumed_before_session() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        let ana = store.create_user("ana", "Ana", "hash", 1).await.unwrap();
        let bob = store.create_user("bob", "Bob", "hash", 1).await.unwrap();
        let key = MfaKey::new(Zeroizing::new([7; 32]));
        let secret = b"12345678901234567890";
        let sealed = key.seal(ana.id, secret).unwrap();
        let recovery: Vec<_> = (0..10).map(|n| [n; 32]).collect();
        store
            .enroll_totp(ana.id, &sealed, 0, &recovery, 1)
            .await
            .unwrap();
        assert!(verify_totp_and_create_session(
            &store, &key, bob.id, "287082", &[1; 32], 59000, 100000
        )
        .await
        .unwrap()
        .is_none());
        assert!(verify_totp_and_create_session(
            &store, &key, ana.id, "000000", &[1; 32], 59000, 100000
        )
        .await
        .unwrap()
        .is_none());
        let (a, b) = tokio::join!(
            verify_totp_and_create_session(&store, &key, ana.id, "287082", &[2; 32], 59000, 100000),
            verify_totp_and_create_session(&store, &key, ana.id, "287082", &[3; 32], 59000, 100000)
        );
        assert_ne!(a.unwrap().is_some(), b.unwrap().is_some());
        assert!(store.session(&[1; 32], 59000).await.unwrap().is_none());
        assert_eq!(
            store.session(&[2; 32], 59000).await.unwrap().is_some() as u8
                + store.session(&[3; 32], 59000).await.unwrap().is_some() as u8,
            1
        );
        store.close().await;
    }

    #[tokio::test]
    async fn recovery_token_is_account_bound_and_single_use() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        let ana = store.create_user("ana", "Ana", "hash", 1).await.unwrap();
        let bob = store.create_user("bob", "Bob", "hash", 1).await.unwrap();
        let key = MfaKey::new(Zeroizing::new([7; 32]));
        let token = mfa_crypto::new_recovery_token().unwrap();
        let mut hashes: Vec<_> = (0..9).map(|n| [n; 32]).collect();
        hashes.push(mfa_crypto::recovery_hash(ana.id, &token).unwrap());
        let sealed = key.seal(ana.id, b"12345678901234567890").unwrap();
        store
            .enroll_totp(ana.id, &sealed, 0, &hashes, 1)
            .await
            .unwrap();
        assert!(
            verify_recovery_and_create_session(&store, bob.id, &token, &[1; 32], 10, 100)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            verify_recovery_and_create_session(&store, ana.id, &token, &[2; 32], 10, 100)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            verify_recovery_and_create_session(&store, ana.id, &token, &[3; 32], 10, 100)
                .await
                .unwrap()
                .is_none()
        );
        store.close().await;
    }
}
