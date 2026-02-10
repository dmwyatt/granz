//! Platform detection for selecting the correct release asset.

use super::{UpdateError, UpdateResult};

/// Get the expected asset name for the current platform.
pub fn asset_name() -> UpdateResult<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("grans-linux-x86_64"),
        ("macos", "aarch64") => Ok("grans-macos-aarch64"),
        ("macos", "x86_64") => Ok("grans-macos-x86_64"),
        ("windows", "x86_64") => Ok("grans-windows-x86_64.exe"),
        (os, arch) => Err(UpdateError::UnsupportedPlatform {
            os: os.to_string(),
            arch: arch.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_name_returns_result() {
        // Just verify it returns something without panicking
        let result = asset_name();
        // On supported platforms, should succeed; on unsupported, should error
        match result {
            Ok(name) => {
                assert!(!name.is_empty());
                assert!(name.starts_with("grans-"));
            }
            Err(UpdateError::UnsupportedPlatform { os, arch }) => {
                assert!(!os.is_empty());
                assert!(!arch.is_empty());
            }
            Err(_) => panic!("Unexpected error type"),
        }
    }
}
