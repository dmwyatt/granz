//! The service holding the shared copy of the database, whichever one it is.
//!
//! Push, pull and status only need four things from a service: what it holds
//! under a name, and a way to upload, download, and hash in its algorithm. Each
//! service's client provides those through [`Remote`], so the conflict checks
//! and download verification are written once.

use std::fmt;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::config::SyncConfig;
use super::content_hash::HashKind;
use super::dropbox::{self, DropboxClient};
use super::transfer::{ProgressFn, verify_content_hash};
use super::{SyncError, SyncResult};

/// A service grans can keep the shared copy on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Dropbox,
}

impl Provider {
    /// The algorithm this service reports content hashes in.
    pub fn hash_kind(self) -> HashKind {
        match self {
            Self::Dropbox => HashKind::Dropbox,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dropbox => "Dropbox",
        })
    }
}

/// What a remote reports about a file it stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub size: u64,
    /// The service's hash of the stored bytes, in its provider's [`HashKind`].
    ///
    /// Optional so that a response without one still describes the file;
    /// callers that require verification reject its absence themselves.
    pub content_hash: Option<String>,
    /// When the stored copy last changed, as a Unix timestamp.
    pub modified: Option<u64>,
}

/// Files kept on a service, named relative to grans' place there.
pub trait Remote {
    fn provider(&self) -> Provider;

    /// Describe the file stored under `name`, or `None` if there is none.
    fn stat(&self, name: &str) -> SyncResult<Option<RemoteFile>>;

    /// Store `local_path` under `name`, replacing any file already there.
    ///
    /// Returns what the service reports it stored. Use [`upload_verified`] to
    /// prove that matches the local file.
    fn upload(
        &self,
        local_path: &Path,
        name: &str,
        on_progress: ProgressFn,
    ) -> SyncResult<RemoteFile>;

    /// Stream the file stored under `name` into `writer`, reporting the running
    /// byte count.
    ///
    /// Returns the number of bytes written.
    fn download_to_writer(
        &self,
        name: &str,
        writer: &mut dyn Write,
        on_progress: &mut dyn FnMut(u64),
    ) -> SyncResult<u64>;

    /// Download a small file into memory.
    ///
    /// Only for files whose size is known to be modest, such as sync metadata.
    fn download(&self, name: &str) -> SyncResult<Vec<u8>> {
        let mut buffer = Vec::new();
        self.download_to_writer(name, &mut buffer, &mut |_| {})?;
        Ok(buffer)
    }
}

/// Connect to the remote the sync record names, refreshing its access token.
pub fn connect(config: &SyncConfig) -> SyncResult<Box<dyn Remote>> {
    let refresh_token = config
        .refresh_token
        .as_deref()
        .ok_or(SyncError::NotAuthenticated)?;

    match config.provider() {
        Provider::Dropbox => {
            let tokens = dropbox::oauth::refresh_access_token(refresh_token)?;
            Ok(Box::new(DropboxClient::new(tokens.access_token)?))
        }
    }
}

/// Upload a file and prove the remote stored the bytes hashed as `local_hash`.
///
/// The service reports what it stored, so an upload can prove it arrived intact
/// instead of leaving a corrupt copy to be discovered on the next pull.
pub fn upload_verified(
    remote: &dyn Remote,
    local_path: &Path,
    name: &str,
    local_hash: &str,
    on_progress: ProgressFn,
) -> SyncResult<RemoteFile> {
    let provider = remote.provider();
    let stored = remote.upload(local_path, name, on_progress)?;

    let stored_hash =
        stored
            .content_hash
            .as_deref()
            .ok_or_else(|| SyncError::MissingContentHash {
                provider,
                path: name.to_string(),
            })?;
    verify_content_hash(stored_hash, local_hash, provider, "uploaded file")?;

    Ok(stored)
}

/// An in-memory remote, for testing what the commands do with one.
#[cfg(test)]
pub mod fake {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::io::Write;
    use std::path::Path;

    use super::{Provider, Remote, RemoteFile};
    use crate::sync::SyncResult;
    use crate::sync::content_hash::ContentHasher;
    use crate::sync::transfer::ProgressFn;

    /// Stores files in a map and reports their true hashes.
    #[derive(Default)]
    pub struct FakeRemote {
        files: RefCell<HashMap<String, Vec<u8>>>,
        uploads: Cell<usize>,
    }

    impl FakeRemote {
        pub fn with_file(name: &str, contents: &[u8]) -> Self {
            let remote = Self::default();
            remote
                .files
                .borrow_mut()
                .insert(name.to_string(), contents.to_vec());
            remote
        }

        /// How many uploads have reached the remote.
        pub fn uploads(&self) -> usize {
            self.uploads.get()
        }

        pub fn contents(&self, name: &str) -> Option<Vec<u8>> {
            self.files.borrow().get(name).cloned()
        }

        fn describe(&self, contents: &[u8]) -> RemoteFile {
            let mut hasher = ContentHasher::new(self.provider().hash_kind());
            hasher.update(contents);
            RemoteFile {
                size: contents.len() as u64,
                content_hash: Some(hasher.finish()),
                modified: None,
            }
        }
    }

    impl Remote for FakeRemote {
        fn provider(&self) -> Provider {
            Provider::Dropbox
        }

        fn stat(&self, name: &str) -> SyncResult<Option<RemoteFile>> {
            Ok(self.files.borrow().get(name).map(|c| self.describe(c)))
        }

        fn upload(
            &self,
            local_path: &Path,
            name: &str,
            _on_progress: ProgressFn,
        ) -> SyncResult<RemoteFile> {
            let contents = std::fs::read(local_path)?;
            let stored = self.describe(&contents);
            self.files.borrow_mut().insert(name.to_string(), contents);
            self.uploads.set(self.uploads.get() + 1);
            Ok(stored)
        }

        fn download_to_writer(
            &self,
            name: &str,
            writer: &mut dyn Write,
            on_progress: &mut dyn FnMut(u64),
        ) -> SyncResult<u64> {
            let contents = self
                .contents(name)
                .expect("download of a file never stored");
            writer.write_all(&contents)?;
            on_progress(contents.len() as u64);
            Ok(contents.len() as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRemote;
    use super::*;
    use crate::sync::content_hash::hash_file;

    /// Stores something other than what it was sent, as a transfer corrupted
    /// in flight would.
    struct GarblingRemote(FakeRemote);

    impl Remote for GarblingRemote {
        fn provider(&self) -> Provider {
            self.0.provider()
        }

        fn stat(&self, name: &str) -> SyncResult<Option<RemoteFile>> {
            self.0.stat(name)
        }

        fn upload(
            &self,
            local_path: &Path,
            name: &str,
            on_progress: ProgressFn,
        ) -> SyncResult<RemoteFile> {
            let garbled = local_path.with_extension("garbled");
            std::fs::write(&garbled, b"not what was sent")?;
            self.0.upload(&garbled, name, on_progress)
        }

        fn download_to_writer(
            &self,
            name: &str,
            writer: &mut dyn Write,
            on_progress: &mut dyn FnMut(u64),
        ) -> SyncResult<u64> {
            self.0.download_to_writer(name, writer, on_progress)
        }
    }

    fn local_file(dir: &tempfile::TempDir, contents: &[u8]) -> (std::path::PathBuf, String) {
        let path = dir.path().join("grans.db");
        std::fs::write(&path, contents).unwrap();
        let hash = hash_file(Provider::Dropbox.hash_kind(), &path).unwrap();
        (path, hash)
    }

    #[test]
    fn an_upload_stored_intact_passes_verification() {
        let dir = tempfile::TempDir::new().unwrap();
        let (path, hash) = local_file(&dir, b"meetings");
        let remote = FakeRemote::default();

        let stored = upload_verified(&remote, &path, "grans.db", &hash, Box::new(|_| {})).unwrap();

        assert_eq!(stored.content_hash, Some(hash));
    }

    #[test]
    fn an_upload_stored_differently_fails_verification() {
        let dir = tempfile::TempDir::new().unwrap();
        let (path, hash) = local_file(&dir, b"meetings");
        let remote = GarblingRemote(FakeRemote::default());

        let err = upload_verified(&remote, &path, "grans.db", &hash, Box::new(|_| {})).unwrap_err();

        assert!(matches!(err, SyncError::ContentMismatch { .. }), "{}", err);
    }
}
