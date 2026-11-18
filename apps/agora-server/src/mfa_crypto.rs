//! Two-step verification primitives: sealing TOTP secrets, checking codes and recovery codes.
//! One-time use is enforced atomically in storage, not by these functions alone.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use agora_core::UserId;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use totp_rs::Builder;
use zeroize::Zeroizing;

const VERSION: u8 = 1;
const SECRET_LEN: usize = 20;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const SEALED_LEN: usize = 1 + NONCE_LEN + SECRET_LEN + TAG_LEN;
const PERIOD_SECS: u64 = 30;
const RECOVERY_BYTES: usize = 16;

/// Intentionally does not implement `Debug` or serialization.
pub struct MfaKey(Zeroizing<[u8; 32]>);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MfaError {
    #[error("MFA cryptographic operation failed")]
    Crypto,
    #[error("invalid MFA secret")]
    InvalidSecret,
}

impl MfaKey {
    /// The caller is responsible for securely supplying and backing up this 256-bit key.
    pub fn new(key: Zeroizing<[u8; 32]>) -> Self {
        Self(key)
    }

    /// Seal a 160-bit TOTP secret to an account with AES-256-GCM and a fresh OS-random nonce.
    pub fn seal(&self, user: UserId, secret: &[u8]) -> Result<Vec<u8>, MfaError> {
        if secret.len() != SECRET_LEN {
            return Err(MfaError::InvalidSecret);
        }
        let cipher = Aes256Gcm::new_from_slice(self.0.as_ref()).map_err(|_| MfaError::Crypto)?;
        let mut nonce = [0u8; NONCE_LEN];
        OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| MfaError::Crypto)?;
        let aad = associated_data(user);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: secret,
                    aad: &aad,
                },
            )
            .map_err(|_| MfaError::Crypto)?;
        let mut sealed = Vec::with_capacity(SEALED_LEN);
        sealed.push(VERSION);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    /// Reject malformed or tampered data without disclosing whether a key or account matched.
    pub fn open(&self, user: UserId, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, MfaError> {
        if sealed.len() != SEALED_LEN || sealed.first() != Some(&VERSION) {
            return Err(MfaError::Crypto);
        }
        let cipher = Aes256Gcm::new_from_slice(self.0.as_ref()).map_err(|_| MfaError::Crypto)?;
        let aad = associated_data(user);
        let plain = cipher
            .decrypt(
                Nonce::from_slice(&sealed[1..1 + NONCE_LEN]),
                Payload {
                    msg: &sealed[1 + NONCE_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| MfaError::Crypto)?;
        if plain.len() != SECRET_LEN {
            return Err(MfaError::Crypto);
        }
        Ok(Zeroizing::new(plain))
    }
}

fn associated_data(user: UserId) -> Vec<u8> {
    let mut aad = b"agora:mfa:totp:v1:".to_vec();
    aad.extend_from_slice(&user.0.to_be_bytes());
    aad
}

pub fn new_totp_secret() -> Result<Zeroizing<[u8; SECRET_LEN]>, MfaError> {
    let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
    OsRng
        .try_fill_bytes(secret.as_mut())
        .map_err(|_| MfaError::Crypto)?;
    Ok(secret)
}

/// Validate the standard six-digit TOTP (30 seconds, one step of clock skew). Returns the
/// matched step only if it is newer than `last_used`. The caller must atomically persist and
/// compare the step with session issuance to enforce one-time use under concurrent requests.
pub fn verify_totp(
    secret: &[u8],
    token: &str,
    now_seconds: u64,
    last_used: Option<u64>,
) -> Result<Option<u64>, MfaError> {
    if secret.len() != SECRET_LEN {
        return Err(MfaError::InvalidSecret);
    }
    if token.len() != 6
        || !token.bytes().all(|b| b.is_ascii_digit())
        || now_seconds > u64::MAX - 2 * PERIOD_SECS
    {
        return Ok(None);
    }
    let totp = Builder::new()
        .with_secret(secret.to_vec())
        .with_digits(6)
        .with_step_duration(PERIOD_SECS)
        .with_skew(1)
        .build()
        .map_err(|_| MfaError::InvalidSecret)?;
    Ok(totp
        .check(token, now_seconds)
        .filter(|step| last_used.is_none_or(|last| *step > last)))
}

/// RFC 4648 base32 without padding, as authenticator apps expect.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(bytes.len().saturating_mul(8).div_ceil(5));
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The `otpauth://` URI authenticator apps read from a QR code.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let issuer = percent(issuer);
    format!(
        "otpauth://totp/{issuer}:{}?secret={}&issuer={issuer}&algorithm=SHA1&digits=6&period=30",
        percent(account),
        base32(secret)
    )
}

fn percent(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Recovery codes are typed by people: accept spaces and dashes they may add.
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A recovery token has 128 bits of OS-generated entropy. The caller must display it only once
/// and atomically consume its hash; neither hashing nor generation alone ensures one-time use.
pub fn new_recovery_token() -> Result<Zeroizing<String>, MfaError> {
    let mut bytes = Zeroizing::new([0u8; RECOVERY_BYTES]);
    OsRng
        .try_fill_bytes(bytes.as_mut())
        .map_err(|_| MfaError::Crypto)?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

/// Hash a recovery token with account and purpose separation; never persist the plaintext.
pub fn recovery_hash(user: UserId, token: &str) -> Option<[u8; 32]> {
    if token.len() != 22 || URL_SAFE_NO_PAD.decode(token).ok()?.len() != RECOVERY_BYTES {
        return None;
    }
    let mut h = Sha256::new();
    h.update(b"agora:mfa:recovery:v1:");
    h.update(user.0.to_be_bytes());
    h.update(token.as_bytes());
    Some(h.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_known_vector_skew_and_replay_boundary() {
        let secret = b"12345678901234567890";
        // RFC 6238 SHA-1 vector at 59 seconds, truncated to six digits.
        assert_eq!(verify_totp(secret, "287082", 59, None).unwrap(), Some(1));
        assert_eq!(verify_totp(secret, "287082", 60, None).unwrap(), Some(1));
        assert_eq!(verify_totp(secret, "287082", 60, Some(1)).unwrap(), None);
        assert_eq!(verify_totp(secret, "287082", 90, None).unwrap(), None);
        assert_eq!(verify_totp(secret, "28x082", 59, None).unwrap(), None);
        assert_eq!(verify_totp(secret, "287082", u64::MAX, None).unwrap(), None);
        assert_eq!(
            verify_totp(b"short", "287082", 59, None),
            Err(MfaError::InvalidSecret)
        );
    }

    #[test]
    fn base32_and_uri_match_rfc_4648_and_key_uri_format() {
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(
            base32(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        let uri = otpauth_uri("Ágora", "ana smith", b"12345678901234567890");
        assert!(uri.starts_with("otpauth://totp/%C3%81gora:ana%20smith?secret=GEZDG"));
        assert!(uri.contains("&issuer=%C3%81gora&"));
    }

    #[test]
    fn sealed_secret_is_random_and_bound_to_account_and_key() {
        let key = MfaKey::new(Zeroizing::new([7; 32]));
        let wrong = MfaKey::new(Zeroizing::new([8; 32]));
        let secret = new_totp_secret().unwrap();
        let a = key.seal(UserId(1), secret.as_ref()).unwrap();
        let b = key.seal(UserId(1), secret.as_ref()).unwrap();
        assert_ne!(a, b);
        assert_eq!(key.open(UserId(1), &a).unwrap().as_slice(), secret.as_ref());
        assert!(key.open(UserId(2), &a).is_err());
        assert!(wrong.open(UserId(1), &a).is_err());
        let mut corrupt = a.clone();
        corrupt[15] ^= 1;
        assert!(key.open(UserId(1), &corrupt).is_err());
        assert!(key.open(UserId(1), &a[..a.len() - 1]).is_err());
        assert!(key.seal(UserId(1), b"short").is_err());
    }

    #[test]
    fn recovery_tokens_are_random_account_bound_and_well_formed() {
        let a = new_recovery_token().unwrap();
        let b = new_recovery_token().unwrap();
        assert_ne!(*a, *b);
        assert_ne!(recovery_hash(UserId(1), &a), recovery_hash(UserId(2), &a));
        assert!(recovery_hash(UserId(1), &a).is_some());
        assert!(recovery_hash(UserId(1), "invalid").is_none());
    }
}
