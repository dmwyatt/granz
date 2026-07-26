//! The Granola desktop client identity grans presents to the Granola API.
//!
//! grans talks to Granola's internal API, which expects the desktop client's
//! version and platform. The `/v1/auth` endpoint is strict about it: omitting
//! the version, or sending one Granola considers too old, redirects to an
//! upgrade page instead of starting a login.

use std::env;

/// Environment variable that overrides the reported client version.
pub const VERSION_ENV_VAR: &str = "GRANS_GRANOLA_VERSION";

/// Version reported when nothing overrides it.
///
/// This will eventually go stale as the desktop app moves on. When it does,
/// `VERSION_ENV_VAR` is the workaround that does not need a grans release.
const DEFAULT_VERSION: &str = "7.441.6";

/// Granola client version to report.
pub fn client_version() -> String {
    resolve_version(env::var(VERSION_ENV_VAR).ok())
}

/// Choose between an override and the built-in default.
///
/// Split from [`client_version`] so the precedence is testable without
/// mutating the environment, which is `unsafe` in edition 2024.
fn resolve_version(override_value: Option<String>) -> String {
    match override_value {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => DEFAULT_VERSION.to_string(),
    }
}

/// Platform name to report.
///
/// `macOS` is what Granola's own client sends and is confirmed against the
/// live endpoint; the other two follow the same capitalized-product-name
/// convention but have not been observed on the wire.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Linux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_defaults_when_unset() {
        assert_eq!(resolve_version(None), DEFAULT_VERSION);
    }

    #[test]
    fn test_version_override_wins() {
        assert_eq!(resolve_version(Some("9.1.2".to_string())), "9.1.2");
    }

    #[test]
    fn test_blank_override_falls_back_to_default() {
        // An exported-but-empty variable should not send an empty version,
        // which the auth endpoint treats as missing.
        assert_eq!(resolve_version(Some("   ".to_string())), DEFAULT_VERSION);
    }

    #[test]
    fn test_override_is_trimmed() {
        assert_eq!(resolve_version(Some(" 9.1.2\n".to_string())), "9.1.2");
    }

    #[test]
    fn test_platform_is_reported_for_this_target() {
        assert!(matches!(platform(), "macOS" | "Windows" | "Linux"));
    }
}
