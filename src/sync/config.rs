//! Sync configuration management.
//!
//! Stores OAuth tokens and sync state in `~/.local/share/grans/sync.toml`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use super::remote::Provider;
use super::{SyncError, SyncResult};
use crate::platform::data_dir;

/// The sync record: which remote holds the shared copy, the authorization to
/// reach it, and what both copies held at the last transfer.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct SyncConfig {
    /// The service holding the shared copy.
    ///
    /// Absent from records written before grans supported more than Dropbox;
    /// [`SyncConfig::provider`] reads those as Dropbox.
    #[serde(default)]
    pub provider: Option<Provider>,

    /// OAuth refresh token (long-lived)
    pub refresh_token: Option<String>,

    /// Unix timestamp of last successful push
    pub last_push_time: Option<u64>,

    /// Unix timestamp of last successful pull
    pub last_pull_time: Option<u64>,

    /// Content hash both copies held at the last successful sync, in the
    /// provider's algorithm.
    ///
    /// The reference point for deciding whether the remote copy has changed.
    /// Absent until the first push or pull completes under a version that
    /// records it.
    pub last_synced_hash: Option<String>,

    /// Content hash the local database file held when it last agreed with the
    /// remote copy.
    ///
    /// Normally the same as `last_synced_hash`. They differ when something
    /// rewrites the local file without changing what it holds, which is what
    /// the one-time journal mode conversion does: two header bytes change, so
    /// the file no longer hashes to what the remote has even though the two hold
    /// the same meetings. Keeping the two references apart lets a pull see an
    /// untouched local database and a push see an untouched remote one.
    #[serde(default)]
    pub last_local_hash: Option<String>,
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

        let content = toml::to_string_pretty(self)
            .map_err(|e| SyncError::Config(format!("serialize: {}", e)))?;

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

    /// The service holding the shared copy.
    pub fn provider(&self) -> Provider {
        self.provider.unwrap_or(Provider::Dropbox)
    }

    /// The record after authorizing grans with `provider`.
    ///
    /// Re-authorizing the same remote keeps what the record knows about the
    /// copy there. Switching remotes starts afresh, since hashes recorded
    /// against one service say nothing about the copy on another.
    pub fn reconnected(self, provider: Provider, refresh_token: String) -> Self {
        let kept = if self.provider() == provider {
            self
        } else {
            Self::default()
        };

        Self {
            provider: Some(provider),
            refresh_token: Some(refresh_token),
            ..kept
        }
    }

    /// What the local database file held when it last agreed with the remote.
    ///
    /// Falls back to the shared hash for a record written before the two were
    /// tracked separately.
    pub fn local_reference(&self) -> Option<&str> {
        self.last_local_hash
            .as_deref()
            .or(self.last_synced_hash.as_deref())
    }

    /// Record that both copies now hold `hash`.
    pub fn record_sync(&mut self, hash: String) {
        self.last_synced_hash = Some(hash.clone());
        self.last_local_hash = Some(hash);
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
            provider: Some(Provider::Dropbox),
            refresh_token: Some("test-token".to_string()),
            last_push_time: Some(1234567890),
            last_pull_time: Some(1234567891),
            last_synced_hash: Some("abc123".to_string()),
            last_local_hash: Some("abc123".to_string()),
        };

        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: SyncConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.provider, config.provider);
        assert_eq!(deserialized.refresh_token, config.refresh_token);
        assert_eq!(deserialized.last_push_time, config.last_push_time);
        assert_eq!(deserialized.last_pull_time, config.last_pull_time);
        assert_eq!(deserialized.last_synced_hash, config.last_synced_hash);
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
                provider: None,
                refresh_token: Some("my-token".to_string()),
                last_push_time: Some(100),
                last_pull_time: None,
                last_synced_hash: None,
                last_local_hash: None,
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

    /// Records written before grans supported more than one remote name none,
    /// and every one of them was written for Dropbox.
    #[test]
    fn a_record_that_names_no_provider_is_read_as_dropbox() {
        let legacy = "refresh_token = \"token\"\nlast_synced_hash = \"abc\"\n";

        let config: SyncConfig = toml::from_str(legacy).unwrap();

        assert_eq!(config.provider(), Provider::Dropbox);
    }

    #[test]
    fn reauthorizing_the_same_remote_keeps_the_sync_record() {
        let config = SyncConfig {
            provider: Some(Provider::Dropbox),
            refresh_token: Some("old".to_string()),
            last_synced_hash: Some("abc".to_string()),
            ..Default::default()
        };

        let config = config.reconnected(Provider::Dropbox, "new".to_string());

        assert_eq!(
            (
                config.refresh_token.as_deref(),
                config.last_synced_hash.as_deref()
            ),
            (Some("new"), Some("abc"))
        );
    }
}
