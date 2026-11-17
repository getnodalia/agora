//! GitHub webhook signature validation, independent of HTTP and persistence.
//!
//! A valid HMAC authenticates the raw body only; GitHub's delivery and event headers are not
//! signed. The future receiver must enforce endpoint-specific secrets, repository and event
//! permissions, size limits, replay protection and authorization. This module exposes no route.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Verify the exact request bytes against GitHub's `X-Hub-Signature-256` header.
/// The secret must come from a protected source and must not be shared between integrations.
pub fn verify_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some(hex) = header.strip_prefix("sha256=") else {
        return false;
    };
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let mut signature = [0u8; 32];
    for (slot, pair) in signature.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
        let Some(high) = (pair[0] as char).to_digit(16) else {
            return false;
        };
        let Some(low) = (pair[1] as char).to_digit(16) else {
            return false;
        };
        *slot = ((high << 4) | low) as u8;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::verify_signature;

    #[test]
    fn github_documented_vector_and_untrusted_inputs() {
        let secret = b"It's a Secret to Everybody";
        let body = b"Hello, World!";
        let signature = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_signature(secret, body, signature));
        assert!(!verify_signature(b"wrong", body, signature));
        assert!(!verify_signature(secret, b"Hello, world!", signature));
        assert!(!verify_signature(b"", body, signature));
        assert!(!verify_signature(
            secret,
            body,
            "sha1=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        ));
        assert!(!verify_signature(
            secret,
            body,
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e1z"
        ));
        assert!(!verify_signature(secret, body, "sha256=7571"));
        assert!(!verify_signature(secret, body, &format!("{signature}00")));
    }
}
