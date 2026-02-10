//! Sync configuration management.
//!
//! Stores OAuth tokens and sync state in `~/.local/share/grans/sync.toml`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use super::{SyncError, SyncResult};
use crate::platform::data_dir;

/// Configuration for Dropbox sync.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct SyncConfig {
    /// OAuth refresh token (long-lived)
    pub refresh_token: Option<String>,

    /// Unix timestamp of last successful push
    pub last_push_time: Option<u64>,

    /// Unix timestamp of last successful pull
    pub last_pull_time: Option<u64>,
}

impl SyncConfig {
    /// Load config from disk, returning default if file doesn't exist.
    pub fn load() -> SyncResult<Self> {
        let path = config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content =
            fs::read_to_string(&path).map_err(|e| SyncError::Config(format!("read: {}", e)))?;

        toml::from_str(&content).map_err(|e| SyncError::Config(format!("parse: {}", e)))
    }

    /// Save config to disk with restrictive permissions.
    pub fn save(&self) -> SyncResult<()> {
        let path = config_path()?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content =
            toml::to_string_pretty(self).map_err(|e| SyncError::Config(format!("serialize: {}", e)))?;

        // Write to temp file first for atomic operation
        let temp_path = path.with_extension("toml.tmp");
        fs::write(&temp_path, &content)?;

        // Set permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&temp_path, perms)?;
        }

        // Atomic rename
        fs::rename(&temp_path, &path)?;

        Ok(())
    }

    /// Check if we have authentication credentials.
    pub fn is_authenticated(&self) -> bool {
        self.refresh_token.is_some()
    }

    /// Clear all authentication data.
    pub fn clear_auth(&mut self) {
        self.refresh_token = None;
    }
}

/// Get the path to the sync config file.
pub fn config_path() -> SyncResult<PathBuf> {
    let dir = data_dir().map_err(|e| SyncError::Config(e.to_string()))?;
    Ok(dir.join("sync.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_test_data_dir<F, R>(f: F) -> R
    where
        F: FnOnce(&TempDir) -> R,
    {
        let dir = TempDir::new().unwrap();
        f(&dir)
    }

    #[test]
    fn test_default_config() {
        let config = SyncConfig::default();
        assert!(config.refresh_token.is_none());
        assert!(config.last_push_time.is_none());
        assert!(config.last_pull_time.is_none());
        assert!(!config.is_authenticated());
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = SyncConfig {
            refresh_token: Some("test-token".to_string()),
            last_push_time: Some(1234567890),
            last_pull_time: Some(1234567891),
        };

        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: SyncConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.refresh_token, config.refresh_token);
        assert_eq!(deserialized.last_push_time, config.last_push_time);
        assert_eq!(deserialized.last_pull_time, config.last_pull_time);
    }

    #[test]
    fn test_is_authenticated() {
        let mut config = SyncConfig::default();
        assert!(!config.is_authenticated());

        config.refresh_token = Some("token".to_string());
        assert!(config.is_authenticated());

        config.clear_auth();
        assert!(!config.is_authenticated());
    }

    #[test]
    fn test_roundtrip_file() {
        with_test_data_dir(|dir| {
            let path = dir.path().join("sync.toml");

            let config = SyncConfig {
                refresh_token: Some("my-token".to_string()),
                last_push_time: Some(100),
                last_pull_time: None,
            };

            let content = toml::to_string_pretty(&config).unwrap();
            std::fs::write(&path, &content).unwrap();

            let loaded: SyncConfig =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(loaded.refresh_token, Some("my-token".to_string()));
            assert_eq!(loaded.last_push_time, Some(100));
            assert!(loaded.last_pull_time.is_none());
        });
    }
}
