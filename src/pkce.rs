//! PKCE challenge generation (RFC 7636), shared by the OAuth flows.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

/// PKCE verifier/challenge pair for an OAuth flow.
#[derive(Debug)]
pub struct PkceChallenge {
    /// Random verifier string (sent during token exchange)
    pub verifier: String,
    /// SHA256 hash of the verifier (sent during authorization)
    pub challenge: String,
}

impl PkceChallenge {
    /// Generate a pair from `entropy_bytes` of OS randomness.
    ///
    /// RFC 7636 requires a 43-128 character verifier. Base64url encoding
    /// produces 4 characters per 3 bytes with no padding, so 32 bytes yields
    /// the 43-character minimum and 64 bytes yields 86.
    pub fn generate(entropy_bytes: usize) -> Self {
        let mut bytes = vec![0u8; entropy_bytes];
        OsRng.fill_bytes(&mut bytes);

        let verifier = URL_SAFE_NO_PAD.encode(&bytes);

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        Self { verifier, challenge }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verifier_length_from_entropy() {
        // 64 bytes -> 86 chars (Dropbox), 32 bytes -> 43 chars (Granola)
        assert_eq!(PkceChallenge::generate(64).verifier.len(), 86);
        assert_eq!(PkceChallenge::generate(32).verifier.len(), 43);
    }

    #[test]
    fn test_verifier_is_unreserved_charset() {
        // RFC 7636 restricts the verifier to [A-Za-z0-9-._~]
        let verifier = PkceChallenge::generate(32).verifier;
        assert!(verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')));
    }

    #[test]
    fn test_challenge_is_s256_of_verifier() {
        let pkce = PkceChallenge::generate(64);

        // Challenge is base64url of a SHA256 hash = 43 chars
        assert_eq!(pkce.challenge.len(), 43);

        let mut hasher = Sha256::new();
        hasher.update(pkce.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn test_pkce_uniqueness() {
        let first = PkceChallenge::generate(64);
        let second = PkceChallenge::generate(64);

        assert_ne!(first.verifier, second.verifier);
        assert_ne!(first.challenge, second.challenge);
    }
}
