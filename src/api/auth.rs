use std::env;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use log::debug;
use serde::Deserialize;

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

/// Get the authentication token from Granola's supabase.json file
pub fn get_auth_token() -> Result<String> {
    let config_path = find_supabase_json()?;

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;

    let config: SupabaseConfig = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;

    let token = config
        .workos_tokens
        .and_then(|t| t.access_token)
        .ok_or_else(|| anyhow::anyhow!(
            "No access token found in {}. Please ensure you are logged into Granola.",
            config_path.display()
        ))?;

    if token.is_empty() {
        bail!("Access token is empty in {}. Please re-login to Granola.", config_path.display());
    }

    debug!("Loaded auth token from {} ({} chars)", config_path.display(), token.len());
    Ok(token)
}

/// Find the supabase.json file in platform-specific locations
fn find_supabase_json() -> Result<PathBuf> {
    let candidates = supabase_json_candidates();
    debug!("Searching for supabase.json in {} locations", candidates.len());

    for candidate in &candidates {
        debug!("  checking: {}", candidate.display());
        if candidate.exists() {
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

/// Get Windows-side supabase.json path candidates when running on WSL
fn wsl_windows_supabase_candidates() -> Option<Vec<PathBuf>> {
    let username = wsl_windows_username()?;

    let mut candidates = Vec::new();

    // Windows AppData Roaming path via WSL mount
    let roaming_path = PathBuf::from(format!(
        "/mnt/c/Users/{}/AppData/Roaming/Granola/supabase.json",
        username
    ));
    candidates.push(roaming_path);

    // Also check Local AppData as a fallback
    let local_path = PathBuf::from(format!(
        "/mnt/c/Users/{}/AppData/Local/Granola/supabase.json",
        username
    ));
    candidates.push(local_path);

    Some(candidates)
}

fn supabase_json_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // WSL: Check Windows paths first (higher priority)
    if is_wsl() {
        if let Some(windows_paths) = wsl_windows_supabase_candidates() {
            candidates.extend(windows_paths);
        }
    }

    if let Some(home) = dirs_home() {
        // macOS
        candidates.push(
            home.join("Library/Application Support/Granola/supabase.json"),
        );

        // Linux / WSL fallback
        candidates.push(home.join(".config/Granola/supabase.json"));

        // XDG
        if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
            candidates.push(PathBuf::from(xdg).join("Granola/supabase.json"));
        }
    }

    // Windows (native)
    if let Ok(appdata) = env::var("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("Granola/supabase.json"));
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
    fn test_wsl_windows_supabase_candidates() {
        // Test that the function returns paths in the expected format
        // Even if we can't determine the username, it should not panic
        if let Some(candidates) = wsl_windows_supabase_candidates() {
            for path in &candidates {
                let path_str = path.to_string_lossy();
                assert!(
                    path_str.contains("/mnt/c/Users/")
                        && path_str.contains("Granola/supabase.json")
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
    fn test_supabase_json_candidates_no_panic() {
        // Ensure supabase_json_candidates doesn't panic
        let candidates = supabase_json_candidates();
        // Should return at least some candidates
        assert!(!candidates.is_empty() || cfg!(target_os = "unknown"));
    }
}
