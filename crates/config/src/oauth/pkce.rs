//! PKCE (RFC 7636, S256) and random URL-safe values for OAuth flows.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// A PKCE verifier with its S256 challenge.
#[derive(Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// 32 random bytes as a 43 character verifier (pi's `pkce.js`).
pub fn generate_pkce() -> Pkce {
    let verifier = random_base64url(32);
    let challenge = challenge_for(&verifier);
    Pkce { verifier, challenge }
}

/// `base64url(SHA-256(verifier))` without padding.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `byte_count` random bytes as base64url without padding (state, nonce).
pub fn random_base64url(byte_count: usize) -> String {
    let mut bytes = vec![0u8; byte_count];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_rfc_7636_example() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_values_are_url_safe_and_unique() {
        let first = generate_pkce();
        let second = generate_pkce();
        assert_eq!(first.verifier.len(), 43);
        assert_eq!(first.challenge, challenge_for(&first.verifier));
        assert_ne!(first.verifier, second.verifier);
        let state = random_base64url(32);
        assert_eq!(state.len(), 43);
        assert!(state.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(state, random_base64url(32));
    }
}
