//! Storage for grans's own Granola credentials.
//!
//! Granola's desktop app keeps its data-encryption key in the macOS
//! data-protection keychain, gated on its own code signature, so grans cannot
//! read the app's stored session there. Instead grans holds its own refresh
//! token, obtained through the same PKCE login the desktop client uses.
//!
//! Persisted as TOML at `data_dir()/auth.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::platform::data_dir;

/// Treat an access token expiring within this window as already expired, so a
/// long-running command does not start with a token that dies mid-flight.
const EXPIRY_SKEW_SECS: i64 = 60;

/// grans's own Granola session.
///
/// The refresh token is the durable credential and is always present; the
/// access token is a cache that may be absent or stale.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct GranolaCredentials {
    /// Long-lived refresh token. Granola rotates this on every refresh.
    pub refresh_token: String,

    /// Most recently issued access token, if one has been fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,

    /// Unix timestamp (seconds) at which `access_token` expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,

    /// WorkOS session identifier, for `grans auth status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl GranolaCredentials {
    /// Create credentials holding only a refresh token.
    pub fn from_refresh_token(refresh_token: String) -> Self {
        Self {
            refresh_token,
            access_token: None,
            expires_at: None,
            session_id: None,
        }
    }

    /// Return the access token if it is present and not near expiry.
    ///
    /// `now` is a Unix timestamp in seconds. A token with no recorded expiry
    /// is treated as unusable, since we cannot tell a live one from a dead one.
    pub fn valid_access_token(&self, now: i64) -> Option<&str> {
        let expires_at = self.expires_at?;
        if expires_at - EXPIRY_SKEW_SECS <= now {
            return None;
        }
        self.access_token.as_deref()
    }

    /// Load credentials from `path`, or `None` if the file does not exist.
    pub fn load_from(path: &Path) -> Result<Option<Self>> {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to read {}", path.display()))
            }
        };

        toml::from_str(&content)
            .map(Some)
            .with_context(|| format!("Failed to parse credentials at {}", path.display()))
    }

    /// Write credentials to `path`, replacing any existing file atomically.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize credentials")?;

        // Write to a temp file, tighten permissions, then rename into place so
        // a crash mid-write cannot leave a truncated credential file behind.
        let temp_path = path.with_extension("toml.tmp");
        fs::write(&temp_path, &content)
            .with_context(|| format!("Failed to write {}", temp_path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to set permissions on {}", temp_path.display()))?;
        }

        fs::rename(&temp_path, path)
            .with_context(|| format!("Failed to replace {}", path.display()))
    }

    /// Load credentials from the default location.
    pub fn load() -> Result<Option<Self>> {
        Self::load_from(&credentials_path()?)
    }

    /// Save credentials to the default location.
    pub fn save(&self) -> Result<()> {
        self.save_to(&credentials_path()?)
    }
}

/// Remove the credential file at `path`. Succeeds if it is already gone.
pub fn delete_from(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("Failed to remove {}", path.display())),
    }
}

/// Remove the credential file at the default location.
pub fn delete() -> Result<()> {
    delete_from(&credentials_path()?)
}

/// Path to grans's credential file.
pub fn credentials_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("auth.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const NOW: i64 = 1_800_000_000;

    fn creds() -> GranolaCredentials {
        GranolaCredentials {
            refresh_token: "refresh-abc".to_string(),
            access_token: Some("access-xyz".to_string()),
            expires_at: Some(NOW + 3600),
            session_id: Some("session_01ABC".to_string()),
        }
    }

    #[test]
    fn test_load_from_missing_file_is_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");

        assert_eq!(GranolaCredentials::load_from(&path).unwrap(), None);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");

        creds().save_to(&path).unwrap();

        assert_eq!(GranolaCredentials::load_from(&path).unwrap(), Some(creds()));
    }

    #[test]
    fn test_save_creates_missing_parent_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("deeper").join("auth.toml");

        creds().save_to(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_save_replaces_existing_and_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");

        creds().save_to(&path).unwrap();
        let rotated = GranolaCredentials::from_refresh_token("refresh-rotated".to_string());
        rotated.save_to(&path).unwrap();

        assert_eq!(
            GranolaCredentials::load_from(&path).unwrap(),
            Some(rotated)
        );
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn test_refresh_token_only_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        let bare = GranolaCredentials::from_refresh_token("refresh-only".to_string());

        bare.save_to(&path).unwrap();

        assert_eq!(GranolaCredentials::load_from(&path).unwrap(), Some(bare));
    }

    #[test]
    fn test_load_from_malformed_file_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        fs::write(&path, "this is not toml {{{").unwrap();

        assert!(GranolaCredentials::load_from(&path).is_err());
    }

    #[test]
    fn test_valid_access_token_when_fresh() {
        assert_eq!(creds().valid_access_token(NOW), Some("access-xyz"));
    }

    #[test]
    fn test_valid_access_token_none_when_expired() {
        let expired = GranolaCredentials {
            expires_at: Some(NOW - 1),
            ..creds()
        };

        assert_eq!(expired.valid_access_token(NOW), None);
    }

    #[test]
    fn test_valid_access_token_none_inside_skew_window() {
        // Expires in 30s, which is inside the 60s skew margin.
        let expiring = GranolaCredentials {
            expires_at: Some(NOW + 30),
            ..creds()
        };

        assert_eq!(expiring.valid_access_token(NOW), None);
    }

    #[test]
    fn test_valid_access_token_none_without_expiry() {
        let no_expiry = GranolaCredentials {
            expires_at: None,
            ..creds()
        };

        assert_eq!(no_expiry.valid_access_token(NOW), None);
    }

    #[test]
    fn test_valid_access_token_none_without_token() {
        let no_token = GranolaCredentials {
            access_token: None,
            ..creds()
        };

        assert_eq!(no_token.valid_access_token(NOW), None);
    }

    #[test]
    fn test_delete_removes_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        creds().save_to(&path).unwrap();

        delete_from(&path).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn test_delete_missing_file_succeeds() {
        let dir = TempDir::new().unwrap();

        assert!(delete_from(&dir.path().join("auth.toml")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        creds().save_to(&path).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
