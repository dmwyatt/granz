//! The token endpoint half of OAuth 2.0 (RFC 6749), which every remote shares.
//!
//! Exchanging an authorization code and refreshing an access token are both a
//! form POST that answers with tokens or with an `error` and its description.
//! Only the parameters differ between services.

use serde::Deserialize;

use super::{SyncError, SyncResult};

/// A token endpoint's answer to a successful request.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: String,
    /// The scopes actually granted, space-separated, when the service reports
    /// them. A user can grant fewer than were asked for.
    pub scope: Option<String>,
}

/// A token endpoint's answer to a rejected request.
#[derive(Deserialize, Debug)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

/// POST `params` to a token endpoint and return the tokens it grants.
pub fn request_tokens(url: &str, params: &[(&str, &str)]) -> SyncResult<TokenResponse> {
    let client = reqwest::blocking::Client::new();

    let response = client
        .post(url)
        .form(params)
        .send()
        .map_err(|e| SyncError::OAuth(format!("request failed: {}", e)))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|e| SyncError::OAuth(format!("failed to read response: {}", e)))?;

    if !status.is_success() {
        return Err(parse_rejection(status, &body));
    }

    serde_json::from_str(&body).map_err(|e| SyncError::OAuth(format!("parse response: {}", e)))
}

/// Describe a rejected token request, keeping the service's error code so a
/// caller can explain the cases it knows.
fn parse_rejection(status: reqwest::StatusCode, body: &str) -> SyncError {
    match serde_json::from_str::<TokenError>(body) {
        Ok(err) => SyncError::OAuthRejected {
            message: err.error_description.unwrap_or_else(|| err.error.clone()),
            code: err.error,
        },
        Err(_) => SyncError::OAuth(format!("HTTP {}: {}", status, body)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejection_keeps_the_error_code_and_shows_the_description() {
        let body = r#"{"error": "invalid_grant", "error_description": "Token has been expired or revoked."}"#;

        let err = parse_rejection(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(
            matches!(
                &err,
                SyncError::OAuthRejected { code, message }
                    if code == "invalid_grant" && message == "Token has been expired or revoked."
            ),
            "{:?}",
            err
        );
    }

    #[test]
    fn a_rejection_without_a_description_shows_the_code() {
        let err = parse_rejection(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error": "invalid_client"}"#,
        );

        assert_eq!(err.to_string(), "OAuth error: invalid_client");
    }

    #[test]
    fn a_rejection_that_is_not_json_shows_the_status_and_body() {
        let err = parse_rejection(reqwest::StatusCode::BAD_GATEWAY, "upstream down");

        assert_eq!(
            err.to_string(),
            "OAuth error: HTTP 502 Bad Gateway: upstream down"
        );
    }
}
