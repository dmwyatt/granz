//! OAuth PKCE flow for Dropbox authentication.
//!
//! Implements RFC 7636 (PKCE) for secure authorization without embedding secrets.

use crate::sync::SyncResult;
use crate::sync::oauth::{TokenResponse, request_tokens};

/// Dropbox App Key (public, safe to embed)
const CLIENT_ID: &str = "bqkd8myq8v5w7xu";

/// Dropbox OAuth endpoints
const AUTHORIZE_URL: &str = "https://www.dropbox.com/oauth2/authorize";
const TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";

/// Entropy for the PKCE verifier, in bytes.
pub const PKCE_ENTROPY_BYTES: usize = 64;

/// Build the authorization URL for the user to visit.
pub fn build_auth_url(challenge: &str) -> String {
    format!(
        "{}?client_id={}&response_type=code&code_challenge={}&code_challenge_method=S256&token_access_type=offline",
        AUTHORIZE_URL, CLIENT_ID, challenge
    )
}

/// Exchange authorization code for tokens.
pub fn exchange_code(code: &str, verifier: &str) -> SyncResult<TokenResponse> {
    request_tokens(
        TOKEN_URL,
        &[
            ("code", code),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
            ("client_id", CLIENT_ID),
        ],
    )
}

/// Refresh an expired access token.
pub fn refresh_access_token(refresh_token: &str) -> SyncResult<TokenResponse> {
    request_tokens(
        TOKEN_URL,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_url_format() {
        let challenge = "test_challenge_123";
        let url = build_auth_url(challenge);

        assert!(url.starts_with("https://www.dropbox.com/oauth2/authorize"));
        assert!(url.contains("client_id=bqkd8myq8v5w7xu"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(&format!("code_challenge={}", challenge)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("token_access_type=offline"));
    }
}
