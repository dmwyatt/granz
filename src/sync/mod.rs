//! Sharing the database between machines through a remote service.

pub mod config;
pub mod content_hash;
pub mod dropbox;
pub mod journal;
pub mod metadata;
pub mod oauth;
pub mod reconcile;
pub mod remote;
pub mod transfer;

use thiserror::Error;

use remote::Provider;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("No remote is set up. Run 'grans remote init dropbox' first.")]
    NotAuthenticated,

    #[error("OAuth error: {0}")]
    OAuth(String),

    /// The token endpoint refused the request, with the service's error code.
    #[error("OAuth error: {message}")]
    OAuthRejected { code: String, message: String },

    #[error("{provider} API error: {message}")]
    Api { provider: Provider, message: String },

    #[error("{operation} failed: {hint}")]
    Transport {
        operation: String,
        hint: String,
        #[source]
        source: reqwest::Error,
    },

    #[error(
        "Incomplete transfer: {what} delivered {actual} bytes but {provider} reported {expected}. \
         The local file was left untouched; retry the transfer."
    )]
    IncompleteTransfer {
        provider: Provider,
        what: String,
        actual: u64,
        expected: u64,
    },

    #[error(
        "Corrupt transfer: the {what} received does not match the content hash {provider} \
         reported (expected {expected}, computed {actual}). The local file was left untouched; \
         retry the transfer."
    )]
    ContentMismatch {
        provider: Provider,
        what: String,
        expected: String,
        actual: String,
    },

    #[error(
        "{provider} reported no content hash for {path}, so the transfer could not be verified \
         against the stored file."
    )]
    MissingContentHash { provider: Provider, path: String },

    #[error("Config error: {0}")]
    Config(String),

    #[error(
        "The {what} on {provider} has changed since this machine last synced, so pushing would \
         discard those changes. Run 'grans remote pull' first, or push --force to overwrite."
    )]
    ConflictRemoteChanged { provider: Provider, what: String },

    #[error(
        "The local {what} has changed since this machine last synced, so pulling would discard \
         those changes. Run 'grans remote push' first, or pull --force to overwrite."
    )]
    ConflictLocalChanged { what: String },

    #[error(
        "The {what} on {provider} differs from the local copy, and grans has no record of \
         syncing with it, so neither can be shown to supersede the other. Compare them with \
         'grans remote status', then push --force or pull --force to choose."
    )]
    ConflictNoSyncRecord { provider: Provider, what: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type SyncResult<T> = Result<T, SyncError>;
