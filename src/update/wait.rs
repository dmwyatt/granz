//! Build waiting functionality for checking GitHub Actions workflow status.

use std::thread;
use std::time::{Duration, Instant};

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use super::github::{check_build_status, BuildStatus, WorkflowRun};
use super::{UpdateError, UpdateResult};

/// Configuration for waiting on a build.
#[derive(Debug, Clone)]
pub struct WaitConfig {
    /// How often to poll for status updates (seconds)
    pub poll_interval_secs: u64,
    /// Maximum time to wait (seconds)
    pub timeout_secs: u64,
}

impl Default for WaitConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 15,
            timeout_secs: 600,
        }
    }
}

impl WaitConfig {
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
}

/// Display information about an in-progress build.
pub fn display_build_info(run: &WorkflowRun) {
    println!();
    println!(
        "{}: A release build is in progress",
        "Note".yellow().bold()
    );
    println!(
        "  Workflow: {}",
        run.name.as_deref().unwrap_or("Unknown")
    );
    println!("  Started:  {}", format_timestamp(&run.created_at));
    println!("  URL:      {}", run.html_url);
}

/// Wait for a build to complete.
///
/// Polls the GitHub Actions API at the configured interval until the build
/// completes, fails, or times out.
///
/// Returns `Ok(())` if the build completes successfully.
/// Returns an error if the build fails or times out.
pub fn wait_for_build(token: Option<&str>, config: &WaitConfig) -> UpdateResult<()> {
    let start = Instant::now();
    let poll_duration = Duration::from_secs(config.poll_interval_secs);
    let timeout_duration = Duration::from_secs(config.timeout_secs);

    // Create a spinner that auto-animates
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("[grans] {spinner} Build in progress ({elapsed} elapsed)")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.enable_steady_tick(Duration::from_millis(100));

    loop {
        let elapsed = start.elapsed();

        if elapsed >= timeout_duration {
            pb.finish_and_clear();
            return Err(UpdateError::BuildTimeout {
                elapsed_secs: elapsed.as_secs(),
            });
        }

        // Check current status
        let status = check_build_status(token)?;

        match status {
            BuildStatus::Completed(_) => {
                pb.finish_and_clear();
                println!("{} Build completed!", "✓".green().bold());
                return Ok(());
            }
            BuildStatus::Failed(run) => {
                pb.finish_and_clear();
                let conclusion = run.conclusion.unwrap_or_else(|| "unknown".to_string());
                return Err(UpdateError::BuildFailed { conclusion });
            }
            BuildStatus::InProgress(_) => {
                // Spinner auto-updates, just keep waiting
            }
            BuildStatus::Idle => {
                pb.finish_and_clear();
                println!("No active build found.");
                return Ok(());
            }
        }

        // Wait before next poll
        thread::sleep(poll_duration);
    }
}

/// Format a GitHub timestamp for display in local time.
fn format_timestamp(ts: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    } else {
        ts.replace('T', " ")
            .replace('Z', "")
            .chars()
            .take(19)
            .collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wait_config_default() {
        let config = WaitConfig::default();
        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(config.timeout_secs, 600);
    }

    #[test]
    fn test_wait_config_with_timeout() {
        let config = WaitConfig::default().with_timeout(300);
        assert_eq!(config.timeout_secs, 300);
        assert_eq!(config.poll_interval_secs, 15);
    }

    #[test]
    fn test_format_timestamp() {
        let ts = "2025-01-30T14:23:00Z";
        let formatted = format_timestamp(ts);
        // Output is in local time, so just verify format (YYYY-MM-DD HH:MM:SS)
        assert_eq!(formatted.len(), 19);
        assert!(formatted.contains('-'));
        assert!(formatted.contains(':'));
    }

    #[test]
    fn test_format_timestamp_fallback() {
        let ts = "not-a-timestamp";
        let formatted = format_timestamp(ts);
        // Fallback strips T and Z
        assert!(!formatted.is_empty());
    }
}
