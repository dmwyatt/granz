mod auth;
pub mod client;
pub mod credential_store;
pub mod credentials;
pub mod granola_auth;
pub mod identity;
// Only macOS decides keychain reads by the caller's code signature, so only
// macOS needs the ACL rewritten. See `keychain_acl`.
#[cfg(target_os = "macos")]
pub mod keychain_acl;
// Granola's own token store is readable everywhere but macOS, where its
// encryption key sits behind Granola's code signature. See `auth`.
#[cfg(not(target_os = "macos"))]
pub mod local_store;
#[cfg(not(target_os = "macos"))]
mod token_store;
pub mod types;

pub use auth::{TOKEN_ENV_VAR, resolve_token, token_override};
pub use client::{ApiClient, ApiError, fetch_panels, fetch_transcript};
pub use types::ApiPanel;
