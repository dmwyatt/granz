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

/// What the login callback hands back.
///
/// Beyond the code, the callback restates the terms the login ran under.
/// Echoing those back at the exchange keeps grans consistent with what
/// Granola's own client sends, rather than with what grans guessed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Callback {
    pub code: String,
    pub platform: Option<String>,
    pub is_dev: Option<bool>,
    pub sso: Option<bool>,
    pub sign_in_click_id: Option<String>,
}

/// Read the callback the user pasted back.
///
/// The login hands the same code to two URLs: the `app-redirect` page the
/// browser shows, and the `granola://login-complete` deep link it triggers.
/// Either is a valid thing to copy, as is a bare code, so this takes the
/// parameters from any URL rather than insisting on one scheme.
pub fn parse_callback(pasted: &str) -> Result<Callback> {
    let pasted = pasted.trim();
    if pasted.is_empty() {
        bail!("No authorization code provided");
    }

    if !pasted.contains("://") {
        if pasted.split_whitespace().count() > 1 {
            bail!("Expected a single authorization code or a callback URL, got text with spaces");
        }
        return Ok(Callback {
            code: pasted.to_string(),
            ..Callback::default()
        });
    }

    let url = Url::parse(pasted).context("Could not parse the pasted callback URL")?;
    let parameter = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
    };

    let code = parameter("code").ok_or_else(|| {
        anyhow!(
            "That URL has no 'code' parameter.\n\
             Copy the URL your browser lands on after signing in; it looks like\n\
             {}?code=...",
            REDIRECT_PAGE
        )
    })?;

    Ok(Callback {
        code,
        platform: parameter("platform"),
        is_dev: parameter("isDev").and_then(|value| value.parse().ok()),
        sso: parameter("sso").and_then(|value| value.parse().ok()),
        sign_in_click_id: parameter("signInClickId"),
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
/// `started_with` is the click id grans sent when it built the auth URL, used
/// only when the callback did not restate it.
pub fn exchange_code(
    callback: &Callback,
    verifier: &str,
    started_with: &str,
) -> Result<TokenSet> {
    let body = serde_json::json!({
        "code": callback.code,
        "codeVerifier": verifier,
        "platform": callback.platform.clone().unwrap_or_else(|| identity::platform().to_string()),
        "isDev": callback.is_dev.unwrap_or(false),
        "sso": callback.sso.unwrap_or(false),
        "signInClickId": callback.sign_in_click_id.as_deref().unwrap_or(started_with),
    });

    let response = post_json(AUTH_COMPLETE_URL, &body, None)
        .context("Failed to exchange the authorization code")?;

    extract_token_set(&response).context(
        "Granola's response to the code exchange did not contain the expected tokens",
    )
}

/// Exchange a refresh token for a new token set.
///
/// The response carries a refresh token, which Granola has been observed
/// returning unchanged rather than rotating. Callers persist whatever comes
/// back regardless, so a future rotation cannot strand the chain.
///
/// The access token identifies the caller and may be expired; the refresh
/// token in the body is the grant being validated.
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

/// How deep to look for the token object, and to describe a response.
const MAX_JSON_DEPTH: usize = 5;

/// Pull the token set out of an auth response.
///
/// Granola's endpoints do not agree on where the tokens sit: some nest them
/// under `workos_tokens`, which is sometimes JSON-encoded text rather than an
/// object, and some return them at the top level. Rather than encode one path
/// per endpoint, find the object carrying the pair.
fn extract_token_set(body: &str) -> Result<TokenSet> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Response was not valid JSON")?;

    let tokens = locate_tokens(&value, MAX_JSON_DEPTH).ok_or_else(|| {
        anyhow!(
            "No access_token/refresh_token pair in the response. Its shape was:\n{}",
            describe_json_shape(&value, MAX_JSON_DEPTH)
        )
    })?;

    serde_json::from_value(tokens.clone()).with_context(|| {
        format!(
            "Token object did not match the expected fields. Its shape was:\n{}",
            describe_json_shape(&tokens, MAX_JSON_DEPTH)
        )
    })
}

/// Find the object holding both tokens, descending into JSON-encoded strings.
///
/// Keys naming WorkOS are searched first: a response may carry the identity
/// provider's tokens alongside Granola's, and those are not interchangeable.
fn locate_tokens(value: &serde_json::Value, depth: usize) -> Option<serde_json::Value> {
    if let Some(decoded) = decode_json_string(value) {
        return locate_tokens(&decoded, depth.checked_sub(1)?);
    }

    let object = value.as_object()?;
    if object.contains_key("access_token") && object.contains_key("refresh_token") {
        return Some(value.clone());
    }

    let depth = depth.checked_sub(1)?;
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort_by_key(|key| !key.to_lowercase().contains("workos"));

    keys.into_iter()
        .find_map(|key| locate_tokens(&object[key], depth))
}

/// Parse a string value that itself contains JSON, as Granola writes
/// `workos_tokens` in its own token store.
fn decode_json_string(value: &serde_json::Value) -> Option<serde_json::Value> {
    serde_json::from_str(value.as_str()?).ok()
}

/// Render a value's keys and types, never its values.
///
/// Auth responses carry token material, so a shape is the most that can be
/// safely put in an error message.
fn describe_json_shape(value: &serde_json::Value, depth: usize) -> String {
    match value {
        serde_json::Value::Object(map) if depth == 0 => format!("{{ {} keys }}", map.len()),
        serde_json::Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(key, nested)| format!("{}: {}", key, describe_json_shape(nested, depth - 1)))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
        serde_json::Value::Array(items) => match items.first() {
            Some(_) if depth == 0 => format!("[{} items]", items.len()),
            Some(first) => format!("[{}; {}]", describe_json_shape(first, depth - 1), items.len()),
            None => "[]".to_string(),
        },
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Number(_) => "number".to_string(),
        serde_json::Value::Bool(_) => "bool".to_string(),
        serde_json::Value::Null => "null".to_string(),
    }
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
        let pasted = concat!(
            "https://www.granola.ai/app-redirect?code=01KYE1X5RCRJE7VSWQ452PCK3C",
            "&isDev=false&platform=windows&sso=false"
        );

        let callback = parse_callback(pasted).unwrap();

        assert_eq!(callback.code, "01KYE1X5RCRJE7VSWQ452PCK3C");
        assert_eq!(callback.platform, Some("windows".to_string()));
        assert_eq!(callback.is_dev, Some(false));
        assert_eq!(callback.sso, Some(false));
    }

    #[test]
    fn test_parse_callback_url() {
        // The deep link that page hands off to, for anyone who copies it.
        let callback = parse_callback("granola://login-complete?code=abc123&sso=false").unwrap();

        assert_eq!(callback.code, "abc123");
    }

    #[test]
    fn test_parse_callback_url_percent_decodes() {
        let callback = parse_callback("granola://login-complete?code=a%2Fb%2Bc").unwrap();

        assert_eq!(callback.code, "a/b+c");
    }

    #[test]
    fn test_parse_callback_keeps_the_click_id_it_was_given() {
        let callback =
            parse_callback("granola://login-complete?signInClickId=click-7&code=zzz&handoff=1")
                .unwrap();

        assert_eq!(callback.code, "zzz");
        assert_eq!(callback.sign_in_click_id, Some("click-7".to_string()));
    }

    #[test]
    fn test_parse_bare_code_leaves_terms_unstated() {
        let callback = parse_callback("  raw-code-42  ").unwrap();

        assert_eq!(callback.code, "raw-code-42");
        assert_eq!(callback.platform, None);
        assert_eq!(callback.sign_in_click_id, None);
    }

    #[test]
    fn test_parse_ignores_unparseable_flags() {
        // A malformed boolean should not fail the login; the exchange falls
        // back to grans's own value.
        let callback = parse_callback("granola://login-complete?code=c&sso=maybe").unwrap();

        assert_eq!(callback.sso, None);
    }

    #[test]
    fn test_parse_rejects_empty() {
        assert!(parse_callback("   ").is_err());
    }

    #[test]
    fn test_parse_rejects_url_without_code() {
        let err = parse_callback("granola://login-complete?sso=false").unwrap_err();

        assert!(err.to_string().contains("no 'code' parameter"));
    }

    #[test]
    fn test_parse_rejects_the_starting_auth_url() {
        // Pasting the URL grans printed, rather than where it ends up, is the
        // likeliest mistake. It carries a code_challenge but no code.
        let err = parse_callback(
            "https://api.granola.ai/v1/auth?dev=false&code_challenge=abc&provider=google",
        )
        .unwrap_err();

        assert!(err.to_string().contains("no 'code' parameter"));
        assert!(err.to_string().contains("app-redirect"));
    }

    #[test]
    fn test_parse_rejects_prose() {
        assert!(parse_callback("I could not find the code").is_err());
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
    fn test_extract_token_set_from_an_unexpected_key() {
        let body = r#"{"user": {"id": "u1"},
                       "session": {"tokens": {"access_token": "at", "refresh_token": "rt"}}}"#;

        let tokens = extract_token_set(body).unwrap();

        assert_eq!(tokens.access_token, "at");
    }

    #[test]
    fn test_extract_token_set_prefers_workos_over_provider_tokens() {
        // Google's tokens can ride along in the same response and are not
        // interchangeable with Granola's. Alphabetically google sorts first,
        // so this would pick the wrong pair without the preference.
        let body = r#"{
            "google_tokens": {"access_token": "google-at", "refresh_token": "google-rt"},
            "workos_tokens": {"access_token": "workos-at", "refresh_token": "workos-rt"}
        }"#;

        let tokens = extract_token_set(body).unwrap();

        assert_eq!(tokens.access_token, "workos-at");
    }

    #[test]
    fn test_missing_tokens_error_describes_shape_without_values() {
        let body = r#"{"user": {"email": "someone@example.com"}, "count": 2}"#;

        let message = extract_token_set(body).unwrap_err().to_string();

        assert!(message.contains("user: {email: string}"));
        assert!(message.contains("count: number"));
        assert!(!message.contains("someone@example.com"));
    }

    #[test]
    fn test_describe_json_shape_stops_at_max_depth() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"a": {"b": {"c": "deep"}}}"#).unwrap();

        let described = describe_json_shape(&value, 1);

        assert!(described.contains("a: { 1 keys }"));
        assert!(!described.contains("deep"));
    }

    #[test]
    fn test_locate_tokens_gives_up_rather_than_recursing_forever() {
        // Deeply nested junk must not blow the stack looking for tokens.
        let mut body = String::new();
        for _ in 0..50 {
            body.push_str(r#"{"nested": "#);
        }
        body.push_str("null");
        body.push_str(&"}".repeat(50));
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(locate_tokens(&value, MAX_JSON_DEPTH).is_none());
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
