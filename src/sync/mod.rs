//! Dropbox sync module for synchronizing SQLite index and embeddings databases.

pub mod config;
pub mod content_hash;
pub mod dropbox;
pub mod metadata;
pub mod oauth;
pub mod transfer;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Not authenticated. Run 'grans dropbox init' first.")]
    NotAuthenticated,

    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error("Dropbox API error: {0}")]
    DropboxApi(String),

    #[error("Dropbox {operation} failed: {hint}")]
    Transport {
        operation: String,
        hint: String,
        #[source]
        source: reqwest::Error,
    },

    #[error(
        "Incomplete transfer: {what} delivered {actual} bytes but Dropbox reported {expected}. \
         The local file was left untouched; retry the transfer."
    )]
    IncompleteTransfer {
        what: String,
        actual: u64,
        expected: u64,
    },

    #[error(
        "Corrupt transfer: the {what} received does not match the content hash Dropbox reported \
         (expected {expected}, computed {actual}). The local file was left untouched; \
         retry the transfer."
    )]
    ContentMismatch {
        what: String,
        expected: String,
        actual: String,
    },

    #[error(
        "Dropbox reported no content hash for {path}, so the download could not be verified \
         against the stored file."
    )]
    MissingContentHash { path: String },

    #[error("Config error: {0}")]
    Config(String),

    #[error("File conflict: {what} on Dropbox is newer ({remote_time}) than local ({local_time}). Use --force to overwrite.")]
    ConflictRemoteNewer {
        what: String,
        remote_time: String,
        local_time: String,
    },

    #[error("File conflict: {what} locally is newer ({local_time}) than on Dropbox ({remote_time}). Use --force to overwrite.")]
    ConflictLocalNewer {
        what: String,
        local_time: String,
        remote_time: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type SyncResult<T> = Result<T, SyncError>;
