//! Granola's PKCE login and token refresh.
//!
//! Replicates the desktop client's own OAuth flow so grans can hold its own
//! session rather than scavenging the one Granola stored locally. See
//! [`crate::api::credentials`] for why that scavenging no longer works.
//!
//! The login ends on a `granola.ai/app-redirect?code=...` page that hands off
//! to `granola://login-complete?code=...`, which the operating system routes to
//! Granola.app rather than to grans. So the code is pasted back by hand instead
//! of caught on a loopback listener: Granola's `/v1/auth` rejects a loopback
//! `redirect` parameter with a 403.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use log::debug;
use serde::Deserialize;
use url::Url;

use super::credentials::GranolaCredentials;
use super::identity;

const AUTH_URL: &str = "https://api.granola.ai/v1/auth";
const AUTH_COMPLETE_URL: &str = "https://api.granola.ai/v1/workos-auth-complete";
const REFRESH_URL: &str = "https://api.granola.ai/v1/refresh-access-token";

/// Page the browser lands on when the login succeeds. Shown in error messages
/// so the user knows which URL to copy.
const REDIRECT_PAGE: &str = "https://www.granola.ai/app-redirect";

/// Entropy for the PKCE verifier: 32 bytes is the 43-character verifier
/// Granola's own client sends.
pub const PKCE_ENTROPY_BYTES: usize = 32;

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Identity provider to sign in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Google,
    Microsoft,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Microsoft => "microsoft",
        }
    }
}

/// Tokens as Granola's auth endpoints return them.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    /// Access token lifetime in seconds.
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub session_id: Option<String>,
}

impl TokenSet {
    /// Convert into storable credentials, dating the expiry from `now`.
    ///
    /// `previous_session_id` carries forward the session recorded at login
    /// when a refresh response omits it.
    pub fn into_credentials(
        self,
        now: i64,
        previous_session_id: Option<String>,
    ) -> GranolaCredentials {
        GranolaCredentials {
            expires_at: self.expires_in.map(|lifetime| now + lifetime),
            session_id: self.session_id.or(previous_session_id),
            access_token: Some(self.access_token),
            refresh_token: self.refresh_token,
        }
    }
}

/// Build the URL that starts the login, and the click id that must accompany
/// the later code exchange.
pub fn build_auth_url(challenge: &str, provider: Provider) -> (String, String) {
    let sign_in_click_id = uuid::Uuid::new_v4().to_string();

    let mut url = Url::parse(AUTH_URL).expect("auth URL is a valid constant");
    url.query_pairs_mut()
        .append_pair("dev", "false")
        .append_pair("code_challenge", challenge)
        .append_pair("platform", identity::platform())
        .append_pair("version", &identity::client_version())
        .append_pair("sign_in_click_id", &sign_in_click_id)
        .append_pair("intent", "download")
        .append_pair("provider", provider.as_str());

    (url.into(), sign_in_click_id)
}

/// Extract the authorization code from what the user pasted back.
///
/// The login hands the same code to two URLs: the `app-redirect` page the
/// browser shows, and the `granola://login-complete` deep link it triggers.
/// Either is a valid thing to copy, as is a bare code, so this takes the
/// `code` parameter from any URL rather than insisting on one scheme.
pub fn parse_callback_code(pasted: &str) -> Result<String> {
    let pasted = pasted.trim();
    if pasted.is_empty() {
        bail!("No authorization code provided");
    }

    if !pasted.contains("://") {
        if pasted.split_whitespace().count() > 1 {
            bail!("Expected a single authorization code or a callback URL, got text with spaces");
        }
        return Ok(pasted.to_string());
    }

    let url = Url::parse(pasted).context("Could not parse the pasted callback URL")?;

    url.query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .filter(|code| !code.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "That URL has no 'code' parameter.\n\
                 Copy the URL your browser lands on after signing in; it looks like\n\
                 {}?code=...",
                REDIRECT_PAGE
            )
        })
}

/// Check that Granola will accept our reported client version before sending
/// the user to a browser.
///
/// Granola redirects `/v1/auth` to its upgrade page when the version is stale,
/// which in a browser looks like a marketing page rather than an error. Only
/// that one recognized failure is fatal here; anything else is left to the
/// real flow so a server-side change does not block login.
pub fn check_client_version_accepted(auth_url: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(auth_url)
        .send()
        .context("Failed to reach Granola's auth endpoint")?;

    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    debug!("auth preflight: {} -> {}", response.status(), location);

    if location.contains("upgrade-app") {
        bail!(
            "Granola rejected client version {} as out of date.\n\
             Set {}=<current Granola version> and try again.",
            identity::client_version(),
            identity::VERSION_ENV_VAR
        );
    }

    Ok(())
}

/// Exchange an authorization code for tokens.
///
/// Unauthenticated, which is what makes bootstrapping a session possible.
pub fn exchange_code(code: &str, verifier: &str, sign_in_click_id: &str) -> Result<TokenSet> {
    let body = serde_json::json!({
        "code": code,
        "codeVerifier": verifier,
        "platform": identity::platform(),
        "isDev": false,
        "signInClickId": sign_in_click_id,
    });

    let response = post_json(AUTH_COMPLETE_URL, &body, None)
        .context("Failed to exchange the authorization code")?;

    extract_token_set(&response).context(
        "Granola's response to the code exchange did not contain the expected tokens",
    )
}

/// Exchange a refresh token for a new token set.
///
/// Granola rotates the refresh token on every call, so the response must be
/// persisted or the chain breaks. The access token identifies the caller and
/// may be expired; the refresh token in the body is the grant being validated.
pub fn refresh_tokens(access_token: Option<&str>, refresh_token: &str) -> Result<TokenSet> {
    let body = serde_json::json!({ "refresh_token": refresh_token });

    let response = post_json(REFRESH_URL, &body, access_token)
        .context("Failed to refresh the Granola access token")?;

    extract_token_set(&response)
        .context("Granola's refresh response did not contain the expected tokens")
}

/// POST JSON to a Granola auth endpoint and return the response body.
fn post_json(
    url: &str,
    body: &serde_json::Value,
    bearer: Option<&str>,
) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .build()
        .context("Failed to create HTTP client")?;

    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-Client-Version", identity::client_version())
        .json(body);

    if let Some(token) = bearer {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = request.send().with_context(|| format!("POST {}", url))?;

    let status = response.status();
    let text = response.text().unwrap_or_default();
    debug!("POST {} -> {}", url, status);

    if !status.is_success() {
        bail!("Granola returned HTTP {}: {}", status.as_u16(), text);
    }

    Ok(text)
}

/// Pull the token set out of an auth response.
///
/// Granola nests these under `workos_tokens`, which it sometimes writes as a
/// JSON-encoded string rather than an object, and some responses carry the
/// fields at the top level instead.
fn extract_token_set(body: &str) -> Result<TokenSet> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Response was not valid JSON")?;

    let tokens = match value.get("workos_tokens") {
        Some(serde_json::Value::String(encoded)) => serde_json::from_str(encoded)
            .context("workos_tokens was a string but not valid JSON")?,
        Some(nested) => nested.clone(),
        None => value,
    };

    serde_json::from_value(tokens).context("Response did not match the expected token shape")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_url_carries_required_parameters() {
        let (url, click_id) = build_auth_url("test-challenge", Provider::Google);

        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("code_challenge=test-challenge"));
        assert!(url.contains("provider=google"));
        assert!(url.contains("dev=false"));
        assert!(url.contains("intent=download"));
        // The version gate is the one parameter whose absence silently
        // redirects to an upgrade page.
        assert!(url.contains(&format!("version={}", identity::client_version())));
        assert!(url.contains(&format!("sign_in_click_id={}", click_id)));
    }

    #[test]
    fn test_auth_url_click_id_is_per_call() {
        let (_, first) = build_auth_url("c", Provider::Google);
        let (_, second) = build_auth_url("c", Provider::Google);

        assert_ne!(first, second);
    }

    #[test]
    fn test_auth_url_provider_selects_microsoft() {
        let (url, _) = build_auth_url("c", Provider::Microsoft);

        assert!(url.contains("provider=microsoft"));
    }

    #[test]
    fn test_parse_app_redirect_url() {
        // What the browser actually lands on, and what the address bar shows.
        let pasted = "https://www.granola.ai/app-redirect?code=01KYE1X5RCRJE7VSWQ452PCK3C\
                      &isDev=false&platform=windows&sso=false";

        assert_eq!(
            parse_callback_code(pasted).unwrap(),
            "01KYE1X5RCRJE7VSWQ452PCK3C"
        );
    }

    #[test]
    fn test_parse_callback_url() {
        // The deep link that page hands off to, for anyone who copies it.
        let code = parse_callback_code("granola://login-complete?code=abc123&sso=false").unwrap();

        assert_eq!(code, "abc123");
    }

    #[test]
    fn test_parse_callback_url_percent_decodes() {
        let code = parse_callback_code("granola://login-complete?code=a%2Fb%2Bc").unwrap();

        assert_eq!(code, "a/b+c");
    }

    #[test]
    fn test_parse_callback_url_ignores_other_parameters() {
        let code =
            parse_callback_code("granola://login-complete?signInClickId=x&code=zzz&handoff=1")
                .unwrap();

        assert_eq!(code, "zzz");
    }

    #[test]
    fn test_parse_bare_code() {
        assert_eq!(parse_callback_code("  raw-code-42  ").unwrap(), "raw-code-42");
    }

    #[test]
    fn test_parse_rejects_empty() {
        assert!(parse_callback_code("   ").is_err());
    }

    #[test]
    fn test_parse_rejects_url_without_code() {
        let err = parse_callback_code("granola://login-complete?sso=false").unwrap_err();

        assert!(err.to_string().contains("no 'code' parameter"));
    }

    #[test]
    fn test_parse_rejects_the_starting_auth_url() {
        // Pasting the URL grans printed, rather than where it ends up, is the
        // likeliest mistake. It carries a code_challenge but no code.
        let err = parse_callback_code(
            "https://api.granola.ai/v1/auth?dev=false&code_challenge=abc&provider=google",
        )
        .unwrap_err();

        assert!(err.to_string().contains("no 'code' parameter"));
        assert!(err.to_string().contains("app-redirect"));
    }

    #[test]
    fn test_parse_rejects_prose() {
        assert!(parse_callback_code("I could not find the code").is_err());
    }

    #[test]
    fn test_extract_token_set_from_nested_object() {
        let body = r#"{"workos_tokens": {
            "access_token": "at", "refresh_token": "rt",
            "expires_in": 3600, "session_id": "sess_1"
        }}"#;

        let tokens = extract_token_set(body).unwrap();

        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.refresh_token, "rt");
        assert_eq!(tokens.expires_in, Some(3600));
        assert_eq!(tokens.session_id, Some("sess_1".to_string()));
    }

    #[test]
    fn test_extract_token_set_from_double_encoded_string() {
        // Granola writes workos_tokens as encoded JSON in its own token store,
        // so the API is assumed capable of the same shape.
        let body =
            r#"{"workos_tokens": "{\"access_token\":\"at\",\"refresh_token\":\"rt\"}"}"#;

        let tokens = extract_token_set(body).unwrap();

        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.refresh_token, "rt");
    }

    #[test]
    fn test_extract_token_set_from_top_level() {
        let body = r#"{"access_token": "at", "refresh_token": "rt", "token_type": "Bearer"}"#;

        let tokens = extract_token_set(body).unwrap();

        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.expires_in, None);
    }

    #[test]
    fn test_extract_token_set_rejects_missing_refresh_token() {
        // A response without a refresh token would strand the chain, so it
        // must not be accepted as a successful login.
        let body = r#"{"access_token": "at", "expires_in": 3600}"#;

        assert!(extract_token_set(body).is_err());
    }

    #[test]
    fn test_into_credentials_dates_expiry_from_now() {
        let tokens = TokenSet {
            access_token: "at".to_string(),
            refresh_token: "rt".to_string(),
            expires_in: Some(3600),
            session_id: Some("sess_1".to_string()),
        };

        let creds = tokens.into_credentials(1_000, None);

        assert_eq!(creds.expires_at, Some(4_600));
        assert_eq!(creds.session_id, Some("sess_1".to_string()));
    }

    #[test]
    fn test_into_credentials_keeps_prior_session_id() {
        let tokens = TokenSet {
            access_token: "at".to_string(),
            refresh_token: "rt".to_string(),
            expires_in: None,
            session_id: None,
        };

        let creds = tokens.into_credentials(1_000, Some("sess_login".to_string()));

        assert_eq!(creds.session_id, Some("sess_login".to_string()));
        assert_eq!(creds.expires_at, None);
    }
}
