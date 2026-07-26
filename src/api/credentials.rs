//! grans's own Granola session.
//!
//! Granola's desktop app keeps its data-encryption key in the macOS
//! data-protection keychain, gated on its own code signature, so grans cannot
//! read the app's stored session there. Instead grans holds its own refresh
//! token, obtained through the same PKCE login the desktop client uses.
//!
//! Where that lives is [`super::credential_store`]'s problem.

use serde::{Deserialize, Serialize};

/// Treat an access token expiring within this window as already expired, so a
/// long-running command does not start with a token that dies mid-flight.
const EXPIRY_SKEW_SECS: i64 = 60;

/// grans's own Granola session.
///
/// The refresh token is the durable credential and is always present; the
/// access token is a cache that may be absent or stale.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct GranolaCredentials {
    /// Long-lived refresh token, rewritten from every refresh response.
    pub refresh_token: String,

    /// Most recently issued access token, if one has been fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,

    /// Unix timestamp (seconds) at which `access_token` expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,

    /// WorkOS session identifier, for `grans auth status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl GranolaCredentials {
    /// Create credentials holding only a refresh token.
    pub fn from_refresh_token(refresh_token: String) -> Self {
        Self {
            refresh_token,
            access_token: None,
            expires_at: None,
            session_id: None,
        }
    }

    /// Return the access token if it is present and not near expiry.
    ///
    /// `now` is a Unix timestamp in seconds. A token with no recorded expiry
    /// is treated as unusable, since we cannot tell a live one from a dead one.
    pub fn valid_access_token(&self, now: i64) -> Option<&str> {
        let expires_at = self.expires_at?;
        if expires_at - EXPIRY_SKEW_SECS <= now {
            return None;
        }
        self.access_token.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn creds() -> GranolaCredentials {
        GranolaCredentials {
            refresh_token: "refresh-abc".to_string(),
            access_token: Some("access-xyz".to_string()),
            expires_at: Some(NOW + 3600),
            session_id: Some("session_01ABC".to_string()),
        }
    }

    #[test]
    fn test_valid_access_token_when_fresh() {
        assert_eq!(creds().valid_access_token(NOW), Some("access-xyz"));
    }

    #[test]
    fn test_valid_access_token_none_when_expired() {
        let expired = GranolaCredentials {
            expires_at: Some(NOW - 1),
            ..creds()
        };

        assert_eq!(expired.valid_access_token(NOW), None);
    }

    #[test]
    fn test_valid_access_token_none_inside_skew_window() {
        // Expires in 30s, which is inside the 60s skew margin.
        let expiring = GranolaCredentials {
            expires_at: Some(NOW + 30),
            ..creds()
        };

        assert_eq!(expiring.valid_access_token(NOW), None);
    }

    #[test]
    fn test_valid_access_token_none_without_expiry() {
        let no_expiry = GranolaCredentials {
            expires_at: None,
            ..creds()
        };

        assert_eq!(no_expiry.valid_access_token(NOW), None);
    }

    #[test]
    fn test_valid_access_token_none_without_token() {
        let no_token = GranolaCredentials {
            access_token: None,
            ..creds()
        };

        assert_eq!(no_token.valid_access_token(NOW), None);
    }

    #[test]
    fn test_from_refresh_token_has_nothing_else() {
        let bare = GranolaCredentials::from_refresh_token("rt".to_string());

        assert_eq!(bare.refresh_token, "rt");
        assert_eq!(bare.valid_access_token(NOW), None);
    }
}
