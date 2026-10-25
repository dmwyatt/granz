//! Google's OAuth for installed apps, with a client the user registered.
//!
//! grans ships no Google client of its own. Each user creates a Desktop app
//! OAuth client in their own Google Cloud project and hands grans the JSON file
//! Google offers for download, so no app identity has to outlive the project's
//! maintainer.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::sync::oauth::{TokenResponse, request_tokens};
use crate::sync::{SyncError, SyncResult};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Lets grans see and change the files it creates in Drive, and nothing else.
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// Entropy for the PKCE verifier, in bytes: 86 characters once encoded, inside
/// the 43 to 128 that Google accepts.
pub const PKCE_ENTROPY_BYTES: usize = 64;

/// The OAuth client a user registered in their own Google Cloud project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleClient {
    pub client_id: String,
    /// Google issues desktop clients a secret but does not treat it as
    /// confidential, and documents it as optional for installed apps.
    pub client_secret: Option<String>,
}

/// The client file as Google's console downloads it: the client sits under a
/// key naming its application type.
#[derive(Deserialize)]
struct ClientFile {
    installed: Option<GoogleClient>,
    web: Option<serde_json::Value>,
}

/// Read the client JSON downloaded from the Google Cloud console.
pub fn parse_client_file(json: &str) -> SyncResult<GoogleClient> {
    let file: ClientFile = serde_json::from_str(json)
        .map_err(|e| SyncError::Config(format!("not a Google OAuth client file: {}", e)))?;

    match file {
        ClientFile {
            installed: Some(client),
            ..
        } => Ok(client),
        ClientFile { web: Some(_), .. } => Err(SyncError::Config(
            "this is a Web application client. grans needs a Desktop app client: create \
             one under Clients in the Google Cloud console and download its JSON"
                .to_string(),
        )),
        _ => Err(SyncError::Config(
            "not a Google OAuth client file: it holds no Desktop app (\"installed\") client"
                .to_string(),
        )),
    }
}

/// An unguessable value tying Google's response to this login.
pub fn random_state() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the URL that starts the login in a browser.
pub fn build_auth_url(
    client: &GoogleClient,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", client.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", DRIVE_FILE_SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            // A refresh token comes only with offline access, and only on a
            // consent screen; without forcing one, a second authorization of
            // the same client gets an access token alone.
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", state),
        ],
    )
    .expect("the authorization endpoint is a valid URL")
    .to_string()
}

/// Read the authorization code from Google's response.
///
/// Accepts the request target the browser sent to the loopback listener, or
/// the whole address pasted from a browser on another machine.
pub fn parse_callback(response: &str, expected_state: &str) -> SyncResult<String> {
    let response = response.trim();
    let url = if response.starts_with('/') {
        Url::parse(&format!("http://127.0.0.1{}", response))
    } else {
        Url::parse(response)
    }
    .map_err(|e| SyncError::OAuth(format!("could not read Google's response: {}", e)))?;

    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };

    if param("state").as_deref() != Some(expected_state) {
        return Err(SyncError::OAuth(
            "Google's response belongs to a different login attempt".to_string(),
        ));
    }
    if let Some(error) = param("error") {
        return Err(SyncError::OAuth(format!(
            "Google did not grant access: {}",
            error
        )));
    }

    param("code").ok_or_else(|| {
        SyncError::OAuth("Google's response carried no authorization code".to_string())
    })
}

/// Fail unless the grant includes Drive file access.
pub fn check_granted_scope(granted: Option<&str>) -> SyncResult<()> {
    let has_drive_file = granted
        .unwrap_or_default()
        .split_whitespace()
        .any(|scope| scope == DRIVE_FILE_SCOPE);

    if has_drive_file {
        return Ok(());
    }

    Err(SyncError::OAuth(
        "Google Drive access was not granted. Run the login again and leave the Drive \
         permission ticked on the consent screen."
            .to_string(),
    ))
}

/// Exchange the authorization code for a refresh token.
pub fn exchange_code(
    client: &GoogleClient,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> SyncResult<String> {
    let mut params = vec![
        ("client_id", client.client_id.as_str()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = client.client_secret.as_deref() {
        params.push(("client_secret", secret));
    }

    let tokens = request_tokens(TOKEN_URL, &params)?;
    check_granted_scope(tokens.scope.as_deref())?;

    tokens.refresh_token.ok_or_else(|| {
        SyncError::OAuth("Google granted access but returned no refresh token".to_string())
    })
}

/// Trade the stored refresh token for an access token.
pub fn refresh_access_token(
    client: &GoogleClient,
    refresh_token: &str,
) -> SyncResult<TokenResponse> {
    let mut params = vec![
        ("client_id", client.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    if let Some(secret) = client.client_secret.as_deref() {
        params.push(("client_secret", secret));
    }

    request_tokens(TOKEN_URL, &params).map_err(explain_refresh_failure)
}

/// Say what to do about a stored login Google no longer accepts.
fn explain_refresh_failure(err: SyncError) -> SyncError {
    match err {
        SyncError::OAuthRejected { code, message } if code == "invalid_grant" => {
            SyncError::OAuthRejected {
                message: format!(
                    "{} Google drops a login after 7 days while the OAuth app's publishing \
                     status is Testing (set it to In production), when access is revoked, or \
                     after six months unused. Run 'grans remote init google-drive \
                     <client-file>' again.",
                    message
                ),
                code,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP_CLIENT: &str = r#"{
        "installed": {
            "client_id": "123-abc.apps.googleusercontent.com",
            "project_id": "grans-sync",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token",
            "client_secret": "not-really-secret",
            "redirect_uris": ["http://localhost"]
        }
    }"#;

    const REDIRECT_URI: &str = "http://127.0.0.1:53123";

    fn client() -> GoogleClient {
        GoogleClient {
            client_id: "123-abc.apps.googleusercontent.com".to_string(),
            client_secret: Some("not-really-secret".to_string()),
        }
    }

    mod client_file {
        use super::*;

        #[test]
        fn a_desktop_client_file_yields_its_id_and_secret() {
            assert_eq!(parse_client_file(DESKTOP_CLIENT).unwrap(), client());
        }

        /// Google may stop showing a secret after the client is created, and
        /// installed apps do not need one.
        #[test]
        fn a_desktop_client_file_without_a_secret_is_accepted() {
            let json = r#"{"installed": {"client_id": "123-abc.apps.googleusercontent.com"}}"#;

            assert_eq!(parse_client_file(json).unwrap().client_secret, None);
        }

        /// A Web application client cannot use a loopback redirect, so the
        /// login would fail in the browser with a confusing error.
        #[test]
        fn a_web_application_client_is_refused_by_name() {
            let json = r#"{"web": {"client_id": "x", "client_secret": "y"}}"#;

            let err = parse_client_file(json).unwrap_err().to_string();

            assert!(err.contains("Desktop app"), "{}", err);
        }

        #[test]
        fn a_file_that_is_not_a_client_is_refused() {
            assert!(parse_client_file(r#"{"type": "service_account"}"#).is_err());
        }
    }

    mod auth_url {
        use super::*;

        fn query(url: &str) -> std::collections::HashMap<String, String> {
            url::Url::parse(url)
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect()
        }

        #[test]
        fn asks_for_drive_file_access_only() {
            let url = build_auth_url(&client(), REDIRECT_URI, "challenge", "state");

            assert_eq!(
                query(&url).get("scope").map(String::as_str),
                Some("https://www.googleapis.com/auth/drive.file")
            );
        }

        /// Without offline access and a forced consent screen, Google issues no
        /// refresh token on a second authorization, and grans could not reach
        /// Drive again once the first access token expired.
        #[test]
        fn asks_for_a_refresh_token_every_time() {
            let q = query(&build_auth_url(
                &client(),
                REDIRECT_URI,
                "challenge",
                "state",
            ));

            assert_eq!(
                (
                    q.get("access_type").map(String::as_str),
                    q.get("prompt").map(String::as_str)
                ),
                (Some("offline"), Some("consent"))
            );
        }

        #[test]
        fn carries_the_redirect_challenge_and_state() {
            let q = query(&build_auth_url(
                &client(),
                REDIRECT_URI,
                "the-challenge",
                "the-state",
            ));

            assert_eq!(
                (
                    q.get("redirect_uri").map(String::as_str),
                    q.get("code_challenge").map(String::as_str),
                    q.get("code_challenge_method").map(String::as_str),
                    q.get("state").map(String::as_str),
                    q.get("client_id").map(String::as_str),
                ),
                (
                    Some(REDIRECT_URI),
                    Some("the-challenge"),
                    Some("S256"),
                    Some("the-state"),
                    Some("123-abc.apps.googleusercontent.com"),
                )
            );
        }
    }

    mod callback {
        use super::*;

        #[test]
        fn the_request_the_browser_makes_yields_the_code() {
            let code = parse_callback("/?state=s1&code=4%2F0Abc&scope=x", "s1").unwrap();

            assert_eq!(code, "4/0Abc");
        }

        /// On a machine without a browser, the address the browser ended up on
        /// elsewhere is pasted instead.
        #[test]
        fn a_pasted_address_yields_the_code() {
            let code =
                parse_callback("  http://127.0.0.1:53123/?state=s1&code=abc\n", "s1").unwrap();

            assert_eq!(code, "abc");
        }

        #[test]
        fn a_response_for_another_login_is_refused() {
            assert!(parse_callback("/?state=other&code=abc", "s1").is_err());
        }

        #[test]
        fn a_refusal_in_the_browser_is_reported() {
            let err = parse_callback("/?state=s1&error=access_denied", "s1")
                .unwrap_err()
                .to_string();

            assert!(err.contains("access_denied"), "{}", err);
        }

        #[test]
        fn a_response_without_a_code_is_refused() {
            assert!(parse_callback("/?state=s1", "s1").is_err());
        }
    }

    mod granted_scope {
        use super::*;

        #[test]
        fn drive_file_among_the_granted_scopes_is_enough() {
            let granted = "openid https://www.googleapis.com/auth/drive.file";

            assert!(check_granted_scope(Some(granted)).is_ok());
        }

        /// Google's consent screen lets the user untick a scope; grans would
        /// then fail on its first Drive call instead of here.
        #[test]
        fn a_grant_without_drive_file_is_refused() {
            assert!(check_granted_scope(Some("openid")).is_err());
        }

        #[test]
        fn a_grant_that_names_no_scopes_is_refused() {
            assert!(check_granted_scope(None).is_err());
        }
    }

    mod refresh_failure {
        use super::*;

        /// Google refuses a stored login for reasons the user can fix, and the
        /// bare "invalid_grant" names none of them.
        #[test]
        fn an_expired_grant_says_what_to_do() {
            let err = explain_refresh_failure(SyncError::OAuthRejected {
                code: "invalid_grant".to_string(),
                message: "Token has been expired or revoked.".to_string(),
            })
            .to_string();

            assert!(
                err.contains("In production") && err.contains("grans remote init google-drive"),
                "{}",
                err
            );
        }

        #[test]
        fn other_failures_pass_through_unchanged() {
            let err = explain_refresh_failure(SyncError::OAuth("request failed".to_string()));

            assert_eq!(err.to_string(), "OAuth error: request failed");
        }
    }
}
