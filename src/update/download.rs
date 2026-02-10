//! Download, verify, and replace the binary.

use std::io::{Read, Write};

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use super::github::Asset;
use super::{UpdateError, UpdateResult};

/// Download an asset with a progress bar.
///
/// If `token` is provided, it will be used for authentication and the API URL
/// will be used instead of the browser URL (required for private repos).
pub fn download_asset(asset: &Asset, token: Option<&str>) -> UpdateResult<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("grans/{}", env!("GRANS_VERSION")))
        .build()?;

    // For authenticated requests (private repos), use the API URL with
    // Accept: application/octet-stream header. For public repos without
    // auth, use the browser download URL directly.
    let request = if let Some(token) = token {
        client
            .get(&asset.url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Accept", "application/octet-stream")
    } else {
        client.get(&asset.browser_download_url)
    };

    let response = request.send()?;

    let status = response.status();
    if !status.is_success() {
        return Err(UpdateError::Download(format!(
            "HTTP {}: {}",
            status,
            response.text().unwrap_or_default()
        )));
    }

    let total_size = response.content_length().unwrap_or(asset.size);
    let pb = create_progress_bar(total_size);

    let mut content = Vec::with_capacity(total_size as usize);
    let mut reader = response;

    loop {
        let mut buffer = [0u8; 8192];
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        content.extend_from_slice(&buffer[..bytes_read]);
        pb.set_position(content.len() as u64);
    }

    pb.finish_with_message("Downloaded");
    Ok(content)
}

fn create_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[grans] Downloading {bytes}/{total_bytes} [{bar:30}] {percent}%")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    pb
}

/// Compute SHA256 hash of the currently running binary.
pub fn current_binary_hash() -> UpdateResult<String> {
    let exe_path = std::env::current_exe()?;
    let content = std::fs::read(&exe_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

/// Verify SHA256 checksum of downloaded content.
pub fn verify_checksum(content: &[u8], expected: &str) -> UpdateResult<()> {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let hash = hasher.finalize();
    let actual = format!("{:x}", hash);

    if actual != expected {
        return Err(UpdateError::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        });
    }

    Ok(())
}

/// Replace the current binary with the new one.
pub fn replace_binary(content: &[u8]) -> UpdateResult<()> {
    // Write to temp file first
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("grans_update_binary");

    let mut file = std::fs::File::create(&temp_path)?;
    file.write_all(content)?;
    file.flush()?;
    drop(file);

    // Set executable bit on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_path, perms)?;
    }

    // Replace the binary
    self_replace::self_replace(&temp_path)
        .map_err(|e| UpdateError::Replace(e.to_string()))?;

    // Clean up temp file (may fail on Windows, that's okay)
    let _ = std::fs::remove_file(&temp_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_checksum_success() {
        let content = b"hello world";
        // SHA256 of "hello world"
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_checksum(content, expected).is_ok());
    }

    #[test]
    fn test_verify_checksum_failure() {
        let content = b"hello world";
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = verify_checksum(content, wrong);
        assert!(result.is_err());
        match result {
            Err(UpdateError::ChecksumMismatch { expected, actual }) => {
                assert_eq!(expected, wrong);
                assert_eq!(
                    actual,
                    "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
                );
            }
            _ => panic!("Expected ChecksumMismatch error"),
        }
    }

    #[test]
    fn test_current_binary_hash_returns_valid_sha256() {
        let hash = current_binary_hash().expect("Should be able to hash current binary");
        // SHA256 produces 64 hex characters
        assert_eq!(hash.len(), 64);
        // All characters should be valid hex
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
