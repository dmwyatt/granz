use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use log::debug;
use serde::Deserialize;

use super::credentials::{self, GranolaCredentials};
use super::{granola_auth, token_store};

/// Environment variable supplying a token when `--token` is absent.
pub const TOKEN_ENV_VAR: &str = "GRANS_TOKEN";

/// Structure representing the relevant parts of Granola's supabase.json
#[derive(Debug, Deserialize)]
struct SupabaseConfig {
    #[serde(default, deserialize_with = "deserialize_double_encoded_workos_tokens")]
    workos_tokens: Option<WorkosTokens>,
}

#[derive(Debug, Deserialize)]
struct WorkosTokens {
    #[serde(default)]
    access_token: Option<String>,
}

/// Deserialize workos_tokens which may be either:
/// - A JSON object (WorkosTokens directly)
/// - A double-encoded JSON string containing WorkosTokens
fn deserialize_double_encoded_workos_tokens<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<WorkosTokens>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    // First, try to deserialize as an untagged enum that accepts either
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrObject {
        String(String),
        Object(WorkosTokens),
    }

    match Option::<StringOrObject>::deserialize(deserializer)? {
        None => Ok(None),
        Some(StringOrObject::Object(tokens)) => Ok(Some(tokens)),
        Some(StringOrObject::String(s)) => {
            // Double-encoded: parse the string as JSON
            serde_json::from_str(&s).map(Some).map_err(D::Error::custom)
        }
    }
}

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
    match GranolaCredentials::load()? {
        Some(credentials) => {
            debug!("Using grans's own stored credentials");
            token_from_credentials(credentials)
        }
        None => {
            debug!("No stored credentials; reading Granola's local token store");
            get_auth_token()
        }
    }
}

/// Return the stored access token, refreshing it first if it has expired.
fn token_from_credentials(credentials: GranolaCredentials) -> Result<String> {
    let now = Utc::now().timestamp();

    if let Some(token) = credentials.valid_access_token(now) {
        return Ok(token.to_string());
    }

    debug!("Stored access token is missing or expired; refreshing");
    let path = credentials::credentials_path()?;
    refresh_and_persist(credentials, now, &path, live_refresh)
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
    path: &Path,
    refresh: impl Fn(&GranolaCredentials) -> Result<granola_auth::TokenSet>,
) -> Result<String> {
    let error = match refresh_once(&credentials, now, path, &refresh) {
        Ok(token) => return Ok(token),
        Err(error) => error,
    };

    // Granola rotates the refresh token on every call, so another grans
    // invocation refreshing at the same moment consumes the one we just sent.
    // If what is on disk has moved on, that invocation succeeded and its
    // result is usable; only a chain that has genuinely stalled is an error.
    if let Some(current) = GranolaCredentials::load_from(path)? {
        if current.refresh_token != credentials.refresh_token {
            debug!("Refresh token was rotated concurrently; using the newer credentials");
            if let Some(token) = current.valid_access_token(now) {
                return Ok(token.to_string());
            }
            return refresh_once(&current, now, path, &refresh);
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
    path: &Path,
    refresh: impl Fn(&GranolaCredentials) -> Result<granola_auth::TokenSet>,
) -> Result<String> {
    let tokens = refresh(credentials)?;
    let access_token = tokens.access_token.clone();

    // Persist before returning: Granola has already retired the old refresh
    // token, so handing back an access token without saving the rotation
    // would strand the chain if the process died here.
    tokens
        .into_credentials(now, credentials.session_id.clone())
        .save_to(path)?;

    Ok(access_token)
}

/// Get the authentication token from Granola's local config.
///
/// Prefers the encrypted `supabase.json.enc` store used by recent Granola
/// versions, falling back to the legacy plaintext `supabase.json`. Reached
/// only when grans has no credentials of its own.
fn get_auth_token() -> Result<String> {
    let dir = find_granola_dir()?;
    let json = read_token_json(&dir)?;
    extract_access_token(&json, &dir)
}

/// Read the raw token JSON from a Granola config directory, preferring the
/// encrypted store and falling back to the legacy plaintext file.
fn read_token_json(dir: &Path) -> Result<String> {
    let encrypted = dir.join("supabase.json.enc");
    if encrypted.exists() {
        debug!("Reading encrypted token store at {}", encrypted.display());
        return token_store::decrypt_token_json(dir).with_context(|| {
            format!("Failed to read encrypted Granola token store in {}", dir.display())
        });
    }

    let plaintext = dir.join("supabase.json");
    debug!("Reading plaintext token store at {}", plaintext.display());
    std::fs::read_to_string(&plaintext)
        .with_context(|| format!("Failed to read {}", plaintext.display()))
}

/// Parse the token JSON and extract a non-empty access token.
fn extract_access_token(json: &str, dir: &Path) -> Result<String> {
    let config: SupabaseConfig = serde_json::from_str(json)
        .with_context(|| format!("Failed to parse Granola token JSON from {}", dir.display()))?;

    let token = config
        .workos_tokens
        .and_then(|t| t.access_token)
        .ok_or_else(|| anyhow::anyhow!(
            "No access token found in Granola config at {}. Please ensure you are logged into Granola.",
            dir.display()
        ))?;

    if token.is_empty() {
        bail!("Access token is empty in Granola config at {}. Please re-login to Granola.", dir.display());
    }

    debug!("Loaded auth token ({} chars)", token.len());
    Ok(token)
}

/// Find the Granola config directory in platform-specific locations. A
/// directory qualifies if it contains either token store file.
fn find_granola_dir() -> Result<PathBuf> {
    let candidates = granola_dir_candidates();
    debug!("Searching for Granola config in {} locations", candidates.len());

    for candidate in &candidates {
        debug!("  checking: {}", candidate.display());
        if candidate.join("supabase.json.enc").exists()
            || candidate.join("supabase.json").exists()
        {
            debug!("  found: {}", candidate.display());
            return Ok(candidate.clone());
        }
    }

    bail!(
        "Could not find Granola auth config. Searched:\n{}\n\n\
         Please ensure Granola is installed and you are logged in.",
        candidates
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Detect if running on WSL by checking /proc/version for Microsoft/WSL markers
fn is_wsl() -> bool {
    if cfg!(target_os = "linux") {
        if let Ok(version) = std::fs::read_to_string("/proc/version") {
            let version_lower = version.to_lowercase();
            return version_lower.contains("microsoft") || version_lower.contains("wsl");
        }
    }
    false
}

/// Get Windows username from WSL environment
fn wsl_windows_username() -> Option<String> {
    // Try to get Windows username via cmd.exe
    if let Ok(output) = std::process::Command::new("cmd.exe")
        .args(["/c", "echo %USERNAME%"])
        .output()
    {
        if let Ok(username) = String::from_utf8(output.stdout) {
            let username = username.trim();
            if !username.is_empty() && username != "%USERNAME%" {
                return Some(username.to_string());
            }
        }
    }

    // Fallback to WSL username
    if let Ok(user) = env::var("USER") {
        return Some(user);
    }

    None
}

/// Get Windows-side Granola config directory candidates when running on WSL
fn wsl_windows_granola_dirs() -> Option<Vec<PathBuf>> {
    let username = wsl_windows_username()?;

    Some(vec![
        // Windows AppData Roaming path via WSL mount
        PathBuf::from(format!("/mnt/c/Users/{}/AppData/Roaming/Granola", username)),
        // Also check Local AppData as a fallback
        PathBuf::from(format!("/mnt/c/Users/{}/AppData/Local/Granola", username)),
    ])
}

fn granola_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // WSL: Check Windows paths first (higher priority)
    if is_wsl() {
        if let Some(windows_paths) = wsl_windows_granola_dirs() {
            candidates.extend(windows_paths);
        }
    }

    if let Some(home) = dirs_home() {
        // macOS
        candidates.push(home.join("Library/Application Support/Granola"));

        // Linux / WSL fallback
        candidates.push(home.join(".config/Granola"));

        // XDG
        if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
            candidates.push(PathBuf::from(xdg).join("Granola"));
        }
    }

    // Windows (native)
    if let Ok(appdata) = env::var("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("Granola"));
    }

    candidates
}

fn dirs_home() -> Option<PathBuf> {
    env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| env::var("USERPROFILE").ok().map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_supabase_config_with_token() {
        let json = r#"{
            "workos_tokens": {
                "access_token": "test-token-123"
            }
        }"#;

        let config: SupabaseConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.workos_tokens.unwrap().access_token,
            Some("test-token-123".to_string())
        );
    }

    #[test]
    fn test_parse_supabase_config_empty() {
        let json = r#"{}"#;
        let config: SupabaseConfig = serde_json::from_str(json).unwrap();
        assert!(config.workos_tokens.is_none());
    }

    #[test]
    fn test_parse_supabase_config_no_token() {
        let json = r#"{"workos_tokens": {}}"#;
        let config: SupabaseConfig = serde_json::from_str(json).unwrap();
        assert!(config.workos_tokens.unwrap().access_token.is_none());
    }

    #[test]
    fn test_parse_supabase_config_double_encoded() {
        // workos_tokens is a JSON string containing JSON (double-encoded)
        let json = r#"{"workos_tokens": "{\"access_token\":\"double-encoded-token\",\"expires_in\":21599}"}"#;
        let config: SupabaseConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.workos_tokens.unwrap().access_token,
            Some("double-encoded-token".to_string())
        );
    }

    #[test]
    fn test_get_auth_token_from_file() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("supabase.json");

        std::fs::write(&config_path, r#"{"workos_tokens": {"access_token": "my-secret-token"}}"#).unwrap();

        // We can't easily test get_auth_token() directly since it uses platform paths,
        // but we can test the parsing logic
        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: SupabaseConfig = serde_json::from_str(&content).unwrap();
        let token = config.workos_tokens.unwrap().access_token.unwrap();
        assert_eq!(token, "my-secret-token");
    }

    #[test]
    fn test_is_wsl() {
        // We can't guarantee the test environment, but we can verify
        // the function doesn't panic
        let _ = is_wsl();
    }

    #[test]
    fn test_wsl_windows_granola_dirs() {
        // Test that the function returns paths in the expected format
        // Even if we can't determine the username, it should not panic
        if let Some(candidates) = wsl_windows_granola_dirs() {
            for path in &candidates {
                let path_str = path.to_string_lossy();
                assert!(
                    path_str.contains("/mnt/c/Users/") && path_str.ends_with("Granola")
                );
            }
        }
    }

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
        let path = dir.path().join("auth.toml");
        let old = stored("refresh-old", None, None);

        let token = refresh_and_persist(old, 1_000, &path, |_| {
            Ok(token_set("access-new", "refresh-new"))
        })
        .unwrap();

        assert_eq!(token, "access-new");
        let saved = GranolaCredentials::load_from(&path).unwrap().unwrap();
        assert_eq!(saved.refresh_token, "refresh-new");
        assert_eq!(saved.access_token, Some("access-new".to_string()));
        assert_eq!(saved.expires_at, Some(4_600));
    }

    #[test]
    fn test_refresh_carries_session_id_forward() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");

        refresh_and_persist(stored("refresh-old", None, None), 1_000, &path, |_| {
            Ok(token_set("access-new", "refresh-new"))
        })
        .unwrap();

        let saved = GranolaCredentials::load_from(&path).unwrap().unwrap();
        assert_eq!(saved.session_id, Some("sess_original".to_string()));
    }

    #[test]
    fn test_refresh_failure_reports_relogin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        let creds = stored("refresh-dead", None, None);
        creds.save_to(&path).unwrap();

        let err = refresh_and_persist(creds, 1_000, &path, |_| bail!("token expired"))
            .unwrap_err();

        assert!(err.to_string().contains("grans auth login"));
    }

    #[test]
    fn test_refresh_uses_token_a_concurrent_run_already_fetched() {
        // Another grans consumed our refresh token and wrote a fresh access
        // token. Ours fails, but the chain is intact and its result is usable.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        let ours = stored("refresh-old", None, None);
        stored("refresh-rotated", Some("access-from-other-run"), Some(9_000))
            .save_to(&path)
            .unwrap();

        let token = refresh_and_persist(ours, 1_000, &path, |_| bail!("token already used"))
            .unwrap();

        assert_eq!(token, "access-from-other-run");
    }

    #[test]
    fn test_refresh_retries_with_rotated_token_when_other_run_left_none_usable() {
        // The concurrent run rotated the refresh token but its access token is
        // already expired, so we refresh again with what it left behind.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        let ours = stored("refresh-old", None, None);
        stored("refresh-rotated", Some("stale"), Some(0))
            .save_to(&path)
            .unwrap();

        let token = refresh_and_persist(ours, 1_000, &path, |credentials| {
            if credentials.refresh_token == "refresh-rotated" {
                Ok(token_set("access-second-try", "refresh-newest"))
            } else {
                bail!("token already used")
            }
        })
        .unwrap();

        assert_eq!(token, "access-second-try");
        let saved = GranolaCredentials::load_from(&path).unwrap().unwrap();
        assert_eq!(saved.refresh_token, "refresh-newest");
    }

    #[test]
    fn test_refresh_does_not_retry_when_stored_token_is_unchanged() {
        // Nothing rotated it, so the failure is real and must not be retried
        // against the same dead token.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        let creds = stored("refresh-same", None, None);
        creds.save_to(&path).unwrap();
        let attempts = std::cell::Cell::new(0);

        let result = refresh_and_persist(creds, 1_000, &path, |_| {
            attempts.set(attempts.get() + 1);
            bail!("refresh rejected")
        });

        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn test_granola_dir_candidates_no_panic() {
        // Ensure granola_dir_candidates doesn't panic
        let candidates = granola_dir_candidates();
        // Should return at least some candidates
        assert!(!candidates.is_empty() || cfg!(target_os = "unknown"));
    }
}
