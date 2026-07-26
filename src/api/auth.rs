//! Deciding which token grans authenticates with.
//!
//! The sources themselves live elsewhere: [`super::credential_store`] holds
//! grans's own session and [`super::local_store`] reads the one Granola's desktop
//! app stored. This module is the order they are tried in.

use anyhow::{bail, Result};
use chrono::Utc;
use log::debug;

use super::credential_store::CredentialStore;
use super::credentials::GranolaCredentials;
use super::{granola_auth, local_store};

/// Environment variable supplying a token when `--token` is absent.
pub const TOKEN_ENV_VAR: &str = "GRANS_TOKEN";

/// Pick the token override from the `--token` flag or the environment.
///
/// Read at the CLI boundary and threaded downward as a value so nothing below
/// touches process-global state. An exported-but-empty environment variable
/// counts as absent; an empty `--token` is an explicit mistake and is left to
/// [`resolve_token`] to reject.
pub fn token_override(flag: Option<&str>, env_value: Option<String>) -> Option<String> {
    if let Some(flag) = flag {
        return Some(flag.to_string());
    }

    env_value.filter(|value| !value.trim().is_empty())
}

/// Resolve the authentication token.
///
/// In order: the caller's override (`--token` or `GRANS_TOKEN`), then grans's
/// own stored credentials, refreshing them when the access token has expired,
/// then the token Granola's desktop app stored locally.
///
/// That last step no longer works on current macOS builds, where Granola's
/// data-encryption key sits behind its own code signature, but it is still the
/// only source on Windows for anyone who has not run `grans auth login`.
pub fn resolve_token(override_token: Option<&str>) -> Result<String> {
    match override_token {
        Some(token) if token.is_empty() => {
            bail!("Provided --token value is empty")
        }
        Some(token) => {
            debug!("Using provided token override ({} chars)", token.len());
            Ok(token.to_string())
        }
        None => stored_or_local_token(),
    }
}

/// Use grans's own credentials if it has any, otherwise Granola's local store.
fn stored_or_local_token() -> Result<String> {
    let store = CredentialStore::open()?;

    match store.load()? {
        Some(credentials) => {
            debug!("Using grans's own stored credentials");
            token_from_credentials(credentials, &store)
        }
        None => {
            debug!("No stored credentials; reading Granola's local token store");
            local_store::get_auth_token()
        }
    }
}

/// Return the stored access token, refreshing it first if it has expired.
fn token_from_credentials(
    credentials: GranolaCredentials,
    store: &CredentialStore,
) -> Result<String> {
    let now = Utc::now().timestamp();

    if let Some(token) = credentials.valid_access_token(now) {
        return Ok(token.to_string());
    }

    debug!("Stored access token is missing or expired; refreshing");
    refresh_and_persist(credentials, now, store, live_refresh)
}

/// Ask Granola for a new token set.
fn live_refresh(credentials: &GranolaCredentials) -> Result<granola_auth::TokenSet> {
    granola_auth::refresh_tokens(
        credentials.access_token.as_deref(),
        &credentials.refresh_token,
    )
}

/// Refresh the access token, tolerating a concurrent invocation that rotated
/// the refresh token first.
fn refresh_and_persist(
    credentials: GranolaCredentials,
    now: i64,
    store: &CredentialStore,
    refresh: impl Fn(&GranolaCredentials) -> Result<granola_auth::TokenSet>,
) -> Result<String> {
    let error = match refresh_once(&credentials, now, store, &refresh) {
        Ok(token) => return Ok(token),
        Err(error) => error,
    };

    // Granola currently returns the refresh token unchanged, but if it ever
    // rotates, a concurrent grans invocation refreshing at the same moment
    // would consume the token we just sent. If what is on disk has moved on,
    // that invocation succeeded and its result is usable; only a chain that
    // has genuinely stalled is an error.
    if let Some(current) = store.load()? {
        if current.refresh_token != credentials.refresh_token {
            debug!("Refresh token was rotated concurrently; using the newer credentials");
            if let Some(token) = current.valid_access_token(now) {
                return Ok(token.to_string());
            }
            return refresh_once(&current, now, store, &refresh);
        }
    }

    Err(error.context(
        "Could not refresh grans's Granola session. Run `grans auth login` to sign in again.",
    ))
}

/// Exchange the refresh token for a new access token and persist the result.
fn refresh_once(
    credentials: &GranolaCredentials,
    now: i64,
    store: &CredentialStore,
    refresh: impl Fn(&GranolaCredentials) -> Result<granola_auth::TokenSet>,
) -> Result<String> {
    let tokens = refresh(credentials)?;
    let access_token = tokens.access_token.clone();

    // Persist before returning. The refresh token comes back unchanged today,
    // but if Granola starts rotating it, handing back an access token without
    // saving what replaced it would strand the chain if the process died here.
    store.save(&tokens.into_credentials(now, credentials.session_id.clone()))?;

    Ok(access_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_token_uses_override() {
        let token = resolve_token(Some("my-override-token")).unwrap();
        assert_eq!(token, "my-override-token");
    }

    #[test]
    fn test_resolve_token_rejects_empty_override() {
        let result = resolve_token(Some(""));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_token_override_prefers_flag() {
        let chosen = token_override(Some("from-flag"), Some("from-env".to_string()));

        assert_eq!(chosen, Some("from-flag".to_string()));
    }

    #[test]
    fn test_token_override_uses_env_without_flag() {
        let chosen = token_override(None, Some("from-env".to_string()));

        assert_eq!(chosen, Some("from-env".to_string()));
    }

    #[test]
    fn test_token_override_treats_blank_env_as_absent() {
        // An exported-but-empty GRANS_TOKEN should fall through to the
        // credential chain rather than send an empty bearer token.
        assert_eq!(token_override(None, Some("  ".to_string())), None);
    }

    #[test]
    fn test_token_override_keeps_empty_flag_for_rejection() {
        // `--token ""` is a mistake worth reporting, so it must survive to
        // resolve_token rather than silently falling through.
        assert_eq!(token_override(Some(""), None), Some(String::new()));
        assert!(resolve_token(Some("")).is_err());
    }

    #[test]
    fn test_token_override_absent_without_flag_or_env() {
        assert_eq!(token_override(None, None), None);
    }

    // --- refresh chain ---

    fn stored(refresh_token: &str, access_token: Option<&str>, expires_at: Option<i64>) -> GranolaCredentials {
        GranolaCredentials {
            refresh_token: refresh_token.to_string(),
            access_token: access_token.map(str::to_string),
            expires_at,
            session_id: Some("sess_original".to_string()),
        }
    }

    fn token_set(access: &str, refresh: &str) -> granola_auth::TokenSet {
        granola_auth::TokenSet {
            access_token: access.to_string(),
            refresh_token: refresh.to_string(),
            expires_in: Some(3600),
            session_id: None,
        }
    }

    #[test]
    fn test_refresh_persists_rotated_token_before_returning() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::file(dir.path().join("auth.toml"));
        let old = stored("refresh-old", None, None);

        let token = refresh_and_persist(old, 1_000, &store, |_| {
            Ok(token_set("access-new", "refresh-new"))
        })
        .unwrap();

        assert_eq!(token, "access-new");
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.refresh_token, "refresh-new");
        assert_eq!(saved.access_token, Some("access-new".to_string()));
        assert_eq!(saved.expires_at, Some(4_600));
    }

    #[test]
    fn test_refresh_carries_session_id_forward() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::file(dir.path().join("auth.toml"));

        refresh_and_persist(stored("refresh-old", None, None), 1_000, &store, |_| {
            Ok(token_set("access-new", "refresh-new"))
        })
        .unwrap();

        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.session_id, Some("sess_original".to_string()));
    }

    #[test]
    fn test_refresh_failure_reports_relogin() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::file(dir.path().join("auth.toml"));
        let creds = stored("refresh-dead", None, None);
        store.save(&creds).unwrap();

        let err = refresh_and_persist(creds, 1_000, &store, |_| bail!("token expired"))
            .unwrap_err();

        assert!(err.to_string().contains("grans auth login"));
    }

    #[test]
    fn test_refresh_uses_token_a_concurrent_run_already_fetched() {
        // Another grans consumed our refresh token and wrote a fresh access
        // token. Ours fails, but the chain is intact and its result is usable.
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::file(dir.path().join("auth.toml"));
        let ours = stored("refresh-old", None, None);
        store
            .save(&stored("refresh-rotated", Some("access-from-other-run"), Some(9_000)))
            .unwrap();

        let token = refresh_and_persist(ours, 1_000, &store, |_| bail!("token already used"))
            .unwrap();

        assert_eq!(token, "access-from-other-run");
    }

    #[test]
    fn test_refresh_retries_with_rotated_token_when_other_run_left_none_usable() {
        // The concurrent run rotated the refresh token but its access token is
        // already expired, so we refresh again with what it left behind.
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::file(dir.path().join("auth.toml"));
        let ours = stored("refresh-old", None, None);
        store
            .save(&stored("refresh-rotated", Some("stale"), Some(0)))
            .unwrap();

        let token = refresh_and_persist(ours, 1_000, &store, |credentials| {
            if credentials.refresh_token == "refresh-rotated" {
                Ok(token_set("access-second-try", "refresh-newest"))
            } else {
                bail!("token already used")
            }
        })
        .unwrap();

        assert_eq!(token, "access-second-try");
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.refresh_token, "refresh-newest");
    }

    #[test]
    fn test_refresh_does_not_retry_when_stored_token_is_unchanged() {
        // Nothing rotated it, so the failure is real and must not be retried
        // against the same dead token.
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::file(dir.path().join("auth.toml"));
        let creds = stored("refresh-same", None, None);
        store.save(&creds).unwrap();
        let attempts = std::cell::Cell::new(0);

        let result = refresh_and_persist(creds, 1_000, &store, |_| {
            attempts.set(attempts.get() + 1);
            bail!("refresh rejected")
        });

        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
    }

}
