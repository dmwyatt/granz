//! Where grans keeps its Granola credentials.
//!
//! The refresh token is durable: it mints access tokens until the session is
//! revoked, so it belongs in the platform keychain rather than on disk. Where
//! no keychain is reachable, it falls back to a `0600` TOML file at
//! `data_dir()/auth.toml`, which keeps other local users out but leaves the
//! token readable in a backup or a copy of the disk. Callers tell the user
//! when that fallback is in use.
//!
//! On macOS the stored item is given a permissive ACL, which lets any process
//! running as the user read it without a keychain prompt. That is a real
//! concession, and [`super::keychain_acl`] explains why the alternative is
//! being challenged for a password on every single read.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::debug;

use super::credentials::GranolaCredentials;
#[cfg(target_os = "macos")]
use super::keychain_acl;
use crate::platform::data_dir;

/// Keychain service name grans stores its session under.
const KEYCHAIN_SERVICE: &str = "grans";

/// Keychain account name within that service.
const KEYCHAIN_ACCOUNT: &str = "granola-session";

/// Where grans keeps its Granola credentials.
pub enum CredentialStore {
    /// The platform keychain, which keeps the refresh token out of the
    /// filesystem entirely.
    Keychain(Box<keyring::Entry>),
    /// A `0600` TOML file, for machines with no reachable keychain.
    File(PathBuf),
}

impl CredentialStore {
    /// Open the best available store, moving file-stored credentials into the
    /// keychain the first time one is available.
    pub fn open() -> Result<Self> {
        let store = match reachable_keychain() {
            Some(entry) => Self::Keychain(Box::new(entry)),
            None => Self::File(credentials_path()?),
        };

        store.absorb_credentials_file()?;
        Ok(store)
    }

    /// A store backed by a specific file. Used for the fallback and by tests.
    pub fn file(path: PathBuf) -> Self {
        Self::File(path)
    }

    pub fn load(&self) -> Result<Option<GranolaCredentials>> {
        match self {
            Self::File(path) => read_file(path),
            Self::Keychain(entry) => match entry.get_password() {
                Ok(json) => serde_json::from_str(&json)
                    .map(Some)
                    .context("Failed to parse credentials from the keychain"),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e).context("Failed to read credentials from the keychain"),
            },
        }
    }

    pub fn save(&self, credentials: &GranolaCredentials) -> Result<()> {
        match self {
            Self::File(path) => write_file(credentials, path),
            Self::Keychain(entry) => {
                let json = serde_json::to_string(credentials)
                    .context("Failed to serialize credentials")?;
                entry
                    .set_password(&json)
                    .context("Failed to store credentials in the keychain")?;
                grant_prompt_free_access()
            }
        }
    }

    pub fn delete(&self) -> Result<()> {
        match self {
            Self::File(path) => delete_file(path),
            Self::Keychain(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e).context("Failed to remove credentials from the keychain"),
            },
        }
    }

    /// Whether the refresh token is protected by the platform keychain.
    pub fn is_keychain(&self) -> bool {
        matches!(self, Self::Keychain(_))
    }

    /// Name this store for `grans auth status`.
    pub fn describe(&self) -> String {
        match self {
            Self::Keychain(_) => format!("{} ({})", keychain_name(), KEYCHAIN_SERVICE),
            Self::File(path) => path.display().to_string(),
        }
    }

    /// Move credentials out of the fallback file once a keychain is available.
    ///
    /// The file is removed only after the keychain write succeeds, so a
    /// failure here leaves the existing credentials usable.
    fn absorb_credentials_file(&self) -> Result<()> {
        let Self::Keychain(_) = self else {
            return Ok(());
        };

        let path = credentials_path()?;
        let Some(credentials) = read_file(&path)? else {
            return Ok(());
        };

        debug!("Moving credentials from {} into the keychain", path.display());
        self.save(&credentials)?;
        delete_file(&path)
    }
}

/// The keychain, if one can actually be read from.
///
/// Constructing an entry is not proof: a Linux box with no Secret Service, or
/// a locked keychain, fails only when read. So this reads, and treats "no such
/// entry" as a working keychain that is simply empty.
fn reachable_keychain() -> Option<keyring::Entry> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .inspect_err(|e| debug!("No keychain available: {}", e))
        .ok()?;

    match entry.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Some(entry),
        Err(e) => {
            debug!("Keychain present but unreadable: {}", e);
            None
        }
    }
}

/// Keep the item readable by the next build of grans as well as this one.
///
/// Applied on every save rather than at first login: it is idempotent, and an
/// entry written by an earlier version still needs upgrading. Only macOS ties
/// reads to the caller's code signature, so no other platform has anything to
/// do here.
#[cfg(target_os = "macos")]
fn grant_prompt_free_access() -> Result<()> {
    keychain_acl::allow_any_application(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).context(
        "Stored the credentials, but could not stop the macOS keychain from asking \
         for a password on every read. Run `grans auth logout` then `grans auth login` \
         to rewrite the entry.",
    )
}

#[cfg(not(target_os = "macos"))]
fn grant_prompt_free_access() -> Result<()> {
    Ok(())
}

/// What the platform calls its keychain.
fn keychain_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else {
        "Secret Service"
    }
}

/// Read credentials from `path`, or `None` if the file does not exist.
fn read_file(path: &Path) -> Result<Option<GranolaCredentials>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };

    toml::from_str(&content)
        .map(Some)
        .with_context(|| format!("Failed to parse credentials at {}", path.display()))
}

/// Write credentials to `path`, replacing any existing file atomically.
fn write_file(credentials: &GranolaCredentials, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(credentials).context("Failed to serialize credentials")?;

    // Write to a temp file and rename into place, so a crash mid-write
    // cannot leave a truncated credential file behind.
    let temp_path = path.with_extension("toml.tmp");
    let mut file = create_private_file(&temp_path)
        .with_context(|| format!("Failed to create {}", temp_path.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write {}", temp_path.display()))?;
    drop(file);

    fs::rename(&temp_path, path).with_context(|| format!("Failed to replace {}", path.display()))
}

/// Create a file readable only by its owner.
///
/// The permissions are set at creation rather than tightened afterward:
/// writing the refresh token first and calling `chmod` second leaves a window
/// where another local user can read it. On Windows the file inherits the
/// directory's ACL, which is why the fallback is worth warning about there.
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    // Clear anything an interrupted run left behind: the mode below applies
    // only when the open creates the file, so reusing one would silently keep
    // whatever permissions it already had.
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let mut options = fs::OpenOptions::new();
    // create_new refuses to write through a file, or symlink, that appeared
    // between the remove and the open.
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options.open(path)
}

/// Remove the credential file at `path`. Succeeds if it is already gone.
fn delete_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("Failed to remove {}", path.display())),
    }
}

/// Path to the fallback credential file.
fn credentials_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("auth.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn creds() -> GranolaCredentials {
        GranolaCredentials {
            refresh_token: "refresh-abc".to_string(),
            access_token: Some("access-xyz".to_string()),
            expires_at: Some(1_800_003_600),
            session_id: Some("session_01ABC".to_string()),
        }
    }

    fn file_store(dir: &TempDir) -> CredentialStore {
        CredentialStore::file(dir.path().join("auth.toml"))
    }

    #[test]
    fn test_load_missing_file_is_none() {
        let dir = TempDir::new().unwrap();

        assert_eq!(file_store(&dir).load().unwrap(), None);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);

        store.save(&creds()).unwrap();

        assert_eq!(store.load().unwrap(), Some(creds()));
    }

    #[test]
    fn test_refresh_token_only_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);
        let bare = GranolaCredentials::from_refresh_token("refresh-only".to_string());

        store.save(&bare).unwrap();

        assert_eq!(store.load().unwrap(), Some(bare));
    }

    #[test]
    fn test_save_creates_missing_parent_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("deeper").join("auth.toml");

        CredentialStore::file(path.clone()).save(&creds()).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_save_replaces_existing_and_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        let store = CredentialStore::file(path.clone());

        store.save(&creds()).unwrap();
        let rotated = GranolaCredentials::from_refresh_token("refresh-rotated".to_string());
        store.save(&rotated).unwrap();

        assert_eq!(store.load().unwrap(), Some(rotated));
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn test_load_malformed_file_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        fs::write(&path, "this is not toml {{{").unwrap();

        assert!(CredentialStore::file(path).load().is_err());
    }

    #[test]
    fn test_delete_removes_credentials() {
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);
        store.save(&creds()).unwrap();

        store.delete().unwrap();

        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn test_delete_is_idempotent() {
        let dir = TempDir::new().unwrap();

        assert!(file_store(&dir).delete().is_ok());
    }

    #[test]
    fn test_file_store_is_not_reported_as_keychain() {
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);

        assert!(!store.is_keychain());
        assert!(store.describe().contains("auth.toml"));
    }

    #[test]
    fn test_credentials_survive_json_roundtrip() {
        // The keychain holds one JSON string rather than the TOML the file
        // backend writes, so the same struct has to survive both.
        let json = serde_json::to_string(&creds()).unwrap();

        assert_eq!(
            serde_json::from_str::<GranolaCredentials>(&json).unwrap(),
            creds()
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.toml");
        CredentialStore::file(path.clone()).save(&creds()).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_private_file_is_owner_only_from_creation() {
        // The permissions must come from the open, not from a later chmod:
        // between the two, the refresh token would be readable by anyone who
        // can traverse the directory.
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let file = create_private_file(&dir.path().join("fresh.tmp")).unwrap();

        let mode = file.metadata().unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_private_file_tightens_permissions_on_reuse() {
        // A temp file left behind by an earlier run with loose permissions
        // must not be inherited as-is.
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("stale.tmp");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let file = create_private_file(&path).unwrap();

        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}
