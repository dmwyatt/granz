use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use log::debug;
use serde::Deserialize;

use super::token_store;

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

/// Resolve the authentication token: use the provided override, or fall back
/// to reading from Granola's supabase.json.
pub fn resolve_token(override_token: Option<&str>) -> Result<String> {
    match override_token {
        Some(token) if token.is_empty() => {
            bail!("Provided --token value is empty")
        }
        Some(token) => {
            debug!("Using provided --token override ({} chars)", token.len());
            Ok(token.to_string())
        }
        None => get_auth_token(),
    }
}

/// Get the authentication token from Granola's local config.
///
/// Prefers the encrypted `supabase.json.enc` store used by recent Granola
/// versions, falling back to the legacy plaintext `supabase.json`.
pub fn get_auth_token() -> Result<String> {
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
    fn test_resolve_token_none_falls_back() {
        // When no override is provided, resolve_token falls back to get_auth_token.
        // On CI / machines without Granola this will error, but it should not panic.
        let _ = resolve_token(None);
    }

    #[test]
    fn test_granola_dir_candidates_no_panic() {
        // Ensure granola_dir_candidates doesn't panic
        let candidates = granola_dir_candidates();
        // Should return at least some candidates
        assert!(!candidates.is_empty() || cfg!(target_os = "unknown"));
    }
}
