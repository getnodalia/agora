//! Unwired second-factor verification. This alone is not an authentication flow.

use agora_core::UserId;
use agora_storage::Store;

use crate::mfa_crypto::{self, MfaError, MfaKey};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MFA verification failed")]
    Crypto(#[from] MfaError),
    #[error("MFA storage failed")]
    Storage(#[from] agora_storage::StoreError),
}

/// The password must already have been verified and attempts rate-limited by the caller.
/// The caller must supply a freshly generated session-token hash and a trusted server time.
/// This function is not connected to sign-in, enrollment, or any HTTP endpoint.
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
    use zeroize::Zeroizing;

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
