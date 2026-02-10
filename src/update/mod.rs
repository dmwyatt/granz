//! Self-update module for downloading and installing new releases from GitHub.

pub mod download;
pub mod github;
pub mod platform;
pub mod wait;

use thiserror::Error;

/// Get GitHub token from environment if available.
///
/// Checks `GH_TOKEN` first (used by gh CLI), then `GITHUB_TOKEN`.
pub(crate) fn get_github_token_from_env() -> Option<String> {
    std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok()
}

/// Check if the gh CLI is available and authenticated.
///
/// Returns the token from `gh auth token` if successful, None otherwise.
pub(crate) fn get_github_token_from_gh_cli() -> Option<String> {
    std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|token| !token.is_empty())
}

#[derive(Error, Debug)]
pub enum UpdateError {
    #[error("Unsupported platform: {os}/{arch}")]
    UnsupportedPlatform { os: String, arch: String },

    #[error("No release asset found for this platform")]
    AssetNotFound,

    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    #[error("Release not found")]
    NotFound {
        /// Whether a token was used in the request
        has_token: bool,
    },

    #[error("Download failed: {0}")]
    Download(String),

    #[error("Binary replacement failed: {0}")]
    Replace(String),

    #[error("Build timed out after {elapsed_secs} seconds")]
    BuildTimeout { elapsed_secs: u64 },

    #[error("Build failed with conclusion: {conclusion}")]
    BuildFailed { conclusion: String },

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type UpdateResult<T> = Result<T, UpdateError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // Helper to safely set/remove env vars in tests
    // SAFETY: These tests run single-threaded (cargo test runs each test in isolation)
    unsafe fn set_env(key: &str, value: &str) {
        unsafe { env::set_var(key, value) };
    }

    unsafe fn remove_env(key: &str) {
        unsafe { env::remove_var(key) };
    }

    unsafe fn restore_env(key: &str, orig: Option<String>) {
        match orig {
            Some(v) => unsafe { env::set_var(key, v) },
            None => unsafe { env::remove_var(key) },
        }
    }

    #[test]
    fn test_get_github_token_from_env_prefers_gh_token() {
        // Save original values
        let orig_gh = env::var("GH_TOKEN").ok();
        let orig_github = env::var("GITHUB_TOKEN").ok();

        // SAFETY: Test runs single-threaded
        unsafe {
            set_env("GH_TOKEN", "gh_token_value");
            set_env("GITHUB_TOKEN", "github_token_value");
        }

        let result = get_github_token_from_env();
        assert_eq!(result, Some("gh_token_value".to_string()));

        // Restore
        unsafe {
            restore_env("GH_TOKEN", orig_gh);
            restore_env("GITHUB_TOKEN", orig_github);
        }
    }

    #[test]
    fn test_get_github_token_from_env_falls_back_to_github_token() {
        // Save original values
        let orig_gh = env::var("GH_TOKEN").ok();
        let orig_github = env::var("GITHUB_TOKEN").ok();

        // SAFETY: Test runs single-threaded
        unsafe {
            remove_env("GH_TOKEN");
            set_env("GITHUB_TOKEN", "github_token_value");
        }

        let result = get_github_token_from_env();
        assert_eq!(result, Some("github_token_value".to_string()));

        // Restore
        unsafe {
            restore_env("GH_TOKEN", orig_gh);
            restore_env("GITHUB_TOKEN", orig_github);
        }
    }

    #[test]
    fn test_get_github_token_from_env_returns_none_when_unset() {
        // Save original values
        let orig_gh = env::var("GH_TOKEN").ok();
        let orig_github = env::var("GITHUB_TOKEN").ok();

        // SAFETY: Test runs single-threaded
        unsafe {
            remove_env("GH_TOKEN");
            remove_env("GITHUB_TOKEN");
        }

        let result = get_github_token_from_env();
        assert_eq!(result, None);

        // Restore
        unsafe {
            restore_env("GH_TOKEN", orig_gh);
            restore_env("GITHUB_TOKEN", orig_github);
        }
    }
}
