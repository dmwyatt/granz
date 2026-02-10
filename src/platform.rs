use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

fn dirs_home() -> Option<PathBuf> {
    env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// Get the data directory for grans (for database, embeddings, etc.)
pub fn data_dir() -> Result<PathBuf> {
    let dir = if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg).join("grans")
    } else if let Some(home) = dirs_home() {
        if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support").join("grans")
        } else {
            // Linux (including WSL)
            home.join(".local").join("share").join("grans")
        }
    } else {
        bail!("Cannot determine data directory");
    };

    Ok(dir)
}

/// Copy text to the system clipboard using platform-specific commands.
///
/// Uses `pbcopy` on macOS, `clip.exe` on Windows/WSL, and `xclip` or `xsel` on Linux.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let (program, args) = clipboard_command()?;

    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to launch clipboard command")?;

    use std::io::Write;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(text.as_bytes())
        .context("Failed to write to clipboard command")?;

    let status = child.wait().context("Clipboard command failed")?;
    if !status.success() {
        bail!("Clipboard command exited with status {}", status);
    }

    Ok(())
}

/// Determine the clipboard command for the current platform.
fn clipboard_command() -> Result<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        return Ok(("pbcopy", vec![]));
    }

    if cfg!(target_os = "windows") {
        return Ok(("clip.exe", vec![]));
    }

    // WSL: use clip.exe
    if is_wsl() {
        return Ok(("clip.exe", vec![]));
    }

    if cfg!(target_os = "linux") {
        // Try xclip first, then xsel
        if command_exists("xclip") {
            return Ok(("xclip", vec!["-selection", "clipboard"]));
        }
        if command_exists("xsel") {
            return Ok(("xsel", vec!["--clipboard", "--input"]));
        }
        bail!(
            "No clipboard utility found. Install xclip or xsel:\n  \
             sudo apt install xclip"
        );
    }

    bail!("Clipboard not supported on this platform")
}

fn is_wsl() -> bool {
    if cfg!(target_os = "linux") {
        if let Ok(version) = std::fs::read_to_string("/proc/version") {
            let lower = version.to_lowercase();
            return lower.contains("microsoft") || lower.contains("wsl");
        }
    }
    false
}

fn command_exists(name: &str) -> bool {
    let checker = if cfg!(target_os = "windows") {
        "where.exe"
    } else {
        "which"
    };
    Command::new(checker)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_dir() {
        let dir = data_dir();
        assert!(dir.is_ok());
        let path = dir.unwrap();
        assert!(path.to_string_lossy().contains("grans"));
    }

    #[test]
    fn test_clipboard_command_resolves() {
        // Should resolve to some clipboard command on CI/dev machines,
        // or return an error with a helpful message -- either way, no panic.
        let _ = clipboard_command();
    }

    #[test]
    fn test_command_exists_known() {
        // `which` itself should always exist on unix
        assert!(command_exists("ls"));
    }

    #[test]
    fn test_command_exists_unknown() {
        assert!(!command_exists("definitely_not_a_real_command_xyz"));
    }
}
