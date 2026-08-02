//! OAuth PKCE flow for Dropbox authentication.
//!
//! Implements RFC 7636 (PKCE) for secure authorization without embedding secrets.

use serde::Deserialize;

use super::{SyncError, SyncResult};

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

/// Response from token endpoint.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: String,
}

/// Error response from token endpoint.
#[derive(Deserialize, Debug)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

/// Exchange authorization code for tokens.
pub fn exchange_code(code: &str, verifier: &str) -> SyncResult<TokenResponse> {
    let client = reqwest::blocking::Client::new();

    let params = [
        ("code", code),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier),
        ("client_id", CLIENT_ID),
    ];

    let response = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .map_err(|e| SyncError::OAuth(format!("request failed: {}", e)))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|e| SyncError::OAuth(format!("failed to read response: {}", e)))?;

    if !status.is_success() {
        // Try to parse error response
        if let Ok(err) = serde_json::from_str::<TokenError>(&body) {
            let msg = err.error_description.unwrap_or_else(|| err.error.clone());
            return Err(SyncError::OAuth(msg));
        }
        return Err(SyncError::OAuth(format!("HTTP {}: {}", status, body)));
    }

    serde_json::from_str(&body).map_err(|e| SyncError::OAuth(format!("parse response: {}", e)))
}

/// Refresh an expired access token.
pub fn refresh_access_token(refresh_token: &str) -> SyncResult<TokenResponse> {
    let client = reqwest::blocking::Client::new();

    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
    ];

    let response = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .map_err(|e| SyncError::OAuth(format!("refresh request failed: {}", e)))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|e| SyncError::OAuth(format!("failed to read response: {}", e)))?;

    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<TokenError>(&body) {
            let msg = err.error_description.unwrap_or_else(|| err.error.clone());
            return Err(SyncError::OAuth(msg));
        }
        return Err(SyncError::OAuth(format!("HTTP {}: {}", status, body)));
    }

    serde_json::from_str(&body).map_err(|e| SyncError::OAuth(format!("parse response: {}", e)))
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
