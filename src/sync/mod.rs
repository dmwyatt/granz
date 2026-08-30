//! Dropbox sync module for synchronizing SQLite index and embeddings databases.

pub mod config;
pub mod content_hash;
pub mod dropbox;
pub mod journal;
pub mod metadata;
pub mod oauth;
pub mod reconcile;
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

    #[error(
        "The {what} on Dropbox has changed since this machine last synced, so pushing would \
         discard those changes. Run 'grans dropbox pull' first, or push --force to overwrite."
    )]
    ConflictRemoteChanged { what: String },

    #[error(
        "The local {what} has changed since this machine last synced, so pulling would discard \
         those changes. Run 'grans dropbox push' first, or pull --force to overwrite."
    )]
    ConflictLocalChanged { what: String },

    #[error(
        "The {what} on Dropbox differs from the local copy, and grans has no record of syncing \
         with it, so neither can be shown to supersede the other. Compare them with \
         'grans dropbox status', then push --force or pull --force to choose."
    )]
    ConflictNoSyncRecord { what: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type SyncResult<T> = Result<T, SyncError>;
