//! File encryption: each file gets its own AES-256-GCM key, which travels inside the end-to-end
//! encrypted message that announces it. The server stores `nonce ‖ ciphertext ‖ tag`.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::{ClientError, Result};

const NONCE_LEN: usize = 12;

/// Encrypt `plain` under a fresh key. Returns the bytes to upload and the key, base64.
pub fn seal(plain: &[u8]) -> Result<(Vec<u8>, String)> {
    let key = Aes256Gcm::generate_key(OsRng);
    let cipher = Aes256Gcm::new(&key);
    let nonce = Aes256Gcm::generate_nonce(OsRng);
    let sealed = cipher
        .encrypt(&nonce, plain)
        .map_err(|_| ClientError::Encryption("could not encrypt the file".into()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok((out, STANDARD.encode(key)))
}

/// Decrypt what [`seal`] produced with the key from the message.
pub fn open(data: &[u8], key: &str) -> Result<Vec<u8>> {
    let bad = || {
        ClientError::Encryption(
            "this file cannot be opened: it was changed or the key is wrong".into(),
        )
    };
    let key = STANDARD.decode(key).map_err(|_| bad())?;
    if key.len() != 32 || data.len() < NONCE_LEN {
        return Err(bad());
    }
    let (nonce, sealed) = data.split_at(NONCE_LEN);
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key))
        .decrypt(Nonce::from_slice(nonce), sealed)
        .map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_file_opens_only_with_its_key_and_unchanged() {
        let (data, key) = seal(b"presupuesto").unwrap();
        assert_eq!(open(&data, &key).unwrap(), b"presupuesto");
        let (_, other) = seal(b"x").unwrap();
        assert!(open(&data, &other).is_err());
        let mut changed = data.clone();
        if let Some(b) = changed.last_mut() {
            *b ^= 1;
        }
        assert!(open(&changed, &key).is_err());
        assert!(open(&data[..5], &key).is_err());
        assert!(open(&data, "not base64!").is_err());
    }
}
