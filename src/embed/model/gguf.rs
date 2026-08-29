//! The GGUF weights the llama.cpp embedder loads, fetched on first use.
//!
//! The ONNX path gets its weights through fastembed's hf-hub download; this
//! is the same arrangement for the GGUF: one pinned file, cached in the
//! platform data directory, downloaded once and never checked again once it
//! is there.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use crate::platform;

/// One pinned weights file: where it comes from and what it must hash to.
pub(super) struct GgufSpec {
    pub file_name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// nomic-embed-text-v1.5 in f16. The pin is the LFS object HuggingFace serves
/// for that path, so a changed upstream file fails loudly here rather than
/// silently producing vectors that no longer match the ONNX ones.
pub(super) const NOMIC_F16: GgufSpec = GgufSpec {
    file_name: "nomic-embed-text-v1.5.f16.gguf",
    url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.f16.gguf",
    sha256: "f7af6f66802f4df86eda10fe9bbcfc75c39562bed48ef6ace719a251cf1c2fdb",
    size: 274_290_560,
};

/// What a fetch delivered, for checking against the spec.
pub(super) struct Transfer {
    pub bytes: u64,
    pub sha256: String,
}

/// The cached path of `spec`, downloading it first if the cache lacks it.
pub(super) fn ensure_model_file(spec: &GgufSpec) -> Result<PathBuf> {
    let dir = platform::data_dir()?.join("gguf_cache");
    fs::create_dir_all(&dir)?;
    ensure_in_dir(spec, &dir, |part| download(spec, part))
}

/// Resolve `spec` inside `dir`, calling `fetch` to fill a temporary file when
/// the final one is missing. The final name only appears once the transfer
/// has been verified, so an interrupted or corrupt download can never be
/// mistaken for the model. The temporary file has a unique name, so two
/// processes downloading at once cannot interleave writes into one file, and
/// it is deleted on every path except the rename into place.
pub(super) fn ensure_in_dir(
    spec: &GgufSpec,
    dir: &Path,
    fetch: impl FnOnce(&Path) -> Result<Transfer>,
) -> Result<PathBuf> {
    let path = dir.join(spec.file_name);
    if path.exists() {
        return Ok(path);
    }

    let part = tempfile::Builder::new()
        .prefix(spec.file_name)
        .suffix(".part")
        .tempfile_in(dir)?;
    let transfer = fetch(part.path())?;
    check_transfer(&transfer, spec)?;
    part.persist(&path)?;
    Ok(path)
}

/// Verify a transfer delivered exactly the pinned file.
pub(super) fn check_transfer(transfer: &Transfer, spec: &GgufSpec) -> Result<()> {
    if transfer.bytes != spec.size {
        bail!(
            "{} download delivered {} bytes, expected {}",
            spec.file_name,
            transfer.bytes,
            spec.size
        );
    }
    if transfer.sha256 != spec.sha256 {
        bail!(
            "{} download hashed to {}, expected {}",
            spec.file_name,
            transfer.sha256,
            spec.sha256
        );
    }
    Ok(())
}

/// Stream `spec.url` to `dest`, hashing as it goes.
fn download(spec: &GgufSpec, dest: &Path) -> Result<Transfer> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("grans/{}", env!("GRANS_VERSION")))
        .build()?;
    let mut response = client
        .get(spec.url)
        .send()
        .with_context(|| format!("downloading {}", spec.file_name))?;
    let status = response.status();
    if !status.is_success() {
        bail!("downloading {}: HTTP {}", spec.file_name, status);
    }

    let progress = download_progress(spec, response.content_length().unwrap_or(spec.size));
    let mut writer = BufWriter::new(File::create(dest)?);
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = response.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buffer[..n])?;
        hasher.update(&buffer[..n]);
        bytes += n as u64;
        progress.set_position(bytes);
    }
    writer.flush()?;
    progress.finish_and_clear();

    Ok(Transfer {
        bytes,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn download_progress(spec: &GgufSpec, total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "[grans] Downloading {} {{bytes}}/{{total_bytes}} [{{bar:30}}] {{percent}}%",
                spec.file_name
            ))
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn hello_spec() -> GgufSpec {
        GgufSpec {
            file_name: "hello.gguf",
            url: "unused",
            sha256: HELLO_SHA256,
            size: 5,
        }
    }

    fn write_hello(part: &Path) -> Result<Transfer> {
        fs::write(part, b"hello")?;
        Ok(Transfer {
            bytes: 5,
            sha256: HELLO_SHA256.to_string(),
        })
    }

    #[test]
    fn cached_file_is_returned_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hello_spec();
        fs::write(dir.path().join(spec.file_name), b"hello").unwrap();

        let path = ensure_in_dir(&spec, dir.path(), |_| panic!("fetch must not run")).unwrap();

        assert_eq!(path, dir.path().join(spec.file_name));
    }

    #[test]
    fn verified_download_is_moved_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hello_spec();

        let path = ensure_in_dir(&spec, dir.path(), write_hello).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn mismatched_download_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let spec = GgufSpec {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..hello_spec()
        };

        let err = ensure_in_dir(&spec, dir.path(), write_hello).unwrap_err();

        assert!(err.to_string().contains("hashed to"), "{err}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn failed_fetch_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hello_spec();

        let err = ensure_in_dir(&spec, dir.path(), |part| {
            fs::write(part, b"hel")?;
            bail!("connection reset")
        })
        .unwrap_err();

        assert!(err.to_string().contains("connection reset"), "{err}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_fetches_write_distinct_files() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hello_spec();
        let mut first_part = None;

        ensure_in_dir(&spec, dir.path(), |part| {
            first_part = Some(part.to_path_buf());
            let second = ensure_in_dir(&spec, dir.path(), |inner| {
                assert_ne!(inner, part, "both fetches were handed the same file");
                write_hello(inner)
            })?;
            assert_eq!(fs::read(second)?, b"hello");
            write_hello(part)
        })
        .unwrap();

        assert!(first_part.is_some());
        assert_eq!(fs::read(dir.path().join(spec.file_name)).unwrap(), b"hello");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn check_transfer_reports_size_before_hash() {
        let spec = hello_spec();
        let transfer = Transfer {
            bytes: 4,
            sha256: "wrong".to_string(),
        };

        let err = check_transfer(&transfer, &spec).unwrap_err();

        assert!(err.to_string().contains("4 bytes, expected 5"), "{err}");
    }

    #[test]
    fn check_transfer_accepts_the_pinned_file() {
        let transfer = Transfer {
            bytes: 5,
            sha256: HELLO_SHA256.to_string(),
        };
        check_transfer(&transfer, &hello_spec()).unwrap();
    }
}
