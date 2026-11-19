//! Recovery keys and reset codes: generated here, shown once, stored only as hashes.

use agora_core::UserId;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

use crate::mfa_crypto::base32;

/// What a code is for, so a hash of one cannot be replayed as the other.
#[derive(Clone, Copy)]
pub enum Purpose {
    Key,
    Operator,
}

impl Purpose {
    const fn label(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Operator => "operator",
        }
    }
}

/// 160 random bits as `ABCD-EFGH-…`, easy to read out and to type.
pub fn new_code() -> String {
    let mut bytes = [0u8; 20];
    OsRng.fill_bytes(&mut bytes);
    base32(&bytes)
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Whatever the person typed, without the dashes and spaces, in capitals.
pub fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// The hash stored for a code, bound to the account and the purpose.
pub fn hash(user: UserId, purpose: Purpose, code: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"agora:reset:v1:");
    h.update(purpose.label().as_bytes());
    h.update(b":");
    h.update(user.0.to_be_bytes());
    h.update(b":");
    h.update(normalize(code).as_bytes());
    h.finalize().into()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_random_readable_and_forgiving_to_type() {
        let a = new_code();
        assert_ne!(a, new_code());
        assert_eq!(a.len(), 8 * 4 + 7);
        let typed = format!(" {} ", a.to_lowercase().replace('-', " "));
        assert_eq!(normalize(&typed), normalize(&a));
        let user = UserId(1);
        assert_eq!(
            hash(user, Purpose::Key, &a),
            hash(user, Purpose::Key, &typed)
        );
        assert_ne!(
            hash(user, Purpose::Key, &a),
            hash(user, Purpose::Operator, &a)
        );
        assert_ne!(
            hash(user, Purpose::Key, &a),
            hash(UserId(2), Purpose::Key, &a)
        );
    }
}
