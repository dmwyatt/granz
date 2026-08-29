//! Waiting on a GitHub Actions release build, and on the release it publishes.

use std::thread;
use std::time::{Duration, Instant};

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use super::github::{BuildStatus, GitHubApi, Release, WorkflowRun, format_timestamp};
use super::{UpdateError, UpdateResult};

/// Configuration for waiting on a build.
#[derive(Debug, Clone)]
pub struct WaitConfig {
    /// How often to poll for status updates (seconds)
    pub poll_interval_secs: u64,
    /// Maximum time to wait for the build (seconds)
    pub timeout_secs: u64,
    /// Maximum time to wait, after the build, for GitHub to serve its release (seconds)
    pub release_timeout_secs: u64,
}

impl Default for WaitConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 15,
            timeout_secs: 600,
            release_timeout_secs: 120,
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
    println!("{}: A release build is in progress", "Note".yellow().bold());
    println!("  Workflow: {}", run.name.as_deref().unwrap_or("Unknown"));
    println!("  Started:  {}", format_timestamp(&run.created_at));
    println!("  URL:      {}", run.html_url);
}

fn spinner(template: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template(template)
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Wait for `run` to complete.
///
/// Polls that run by id, so a newer push that cancels and supersedes it is
/// reported as a failure rather than quietly waited on instead. The run the
/// user consented to is the run whose outcome they get.
///
/// Returns the completed run. Returns an error if the build fails, times out,
/// or can no longer be fetched.
pub fn wait_for_build<G: GitHubApi>(
    github: &G,
    token: Option<&str>,
    run: &WorkflowRun,
    config: &WaitConfig,
) -> UpdateResult<WorkflowRun> {
    let start = Instant::now();
    let poll_duration = Duration::from_secs(config.poll_interval_secs);
    let timeout_duration = Duration::from_secs(config.timeout_secs);

    let pb = spinner("[grans] {spinner} Build in progress ({elapsed} elapsed)");

    loop {
        let elapsed = start.elapsed();

        if elapsed >= timeout_duration {
            pb.finish_and_clear();
            return Err(UpdateError::BuildTimeout {
                elapsed_secs: elapsed.as_secs(),
            });
        }

        let latest = github.fetch_workflow_run(run.id, token)?;

        match BuildStatus::from_run(latest) {
            BuildStatus::Completed(run) => {
                pb.finish_and_clear();
                println!("{} Build completed!", "✓".green().bold());
                return Ok(run);
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
                return Err(UpdateError::GitHubApi(format!(
                    "Build {} reported a status grans does not recognize",
                    run.id
                )));
            }
        }

        thread::sleep(poll_duration);
    }
}

/// Whether `release` is what `run` published.
///
/// The release workflow titles each release `<calver> (<short sha>)` with the
/// commit it built, and the run carries that commit as `head_sha`. Matching
/// the two attributes the release by provenance, which a timestamp cannot do
/// when GitHub is serving a cached `/releases/latest`.
pub fn release_is_from_build(release: &Release, run: &WorkflowRun) -> bool {
    release_sha(release).is_some_and(|sha| !sha.is_empty() && run.head_sha.starts_with(sha))
}

/// The short sha in a release title of the form `<calver> (<sha>)`.
fn release_sha(release: &Release) -> Option<&str> {
    let (_, tail) = release.name.as_deref()?.rsplit_once('(')?;
    tail.strip_suffix(')')
}

/// Fetch the release that `run` published.
///
/// Right after a build finishes GitHub can still serve the previous release
/// from `/releases/latest`; same-day rebuilds delete and recreate the release,
/// so this is the expected case rather than a rare one. Keep asking at the
/// polling cadence for a bounded window before giving up.
pub fn wait_for_release<G: GitHubApi>(
    github: &G,
    token: Option<&str>,
    run: &WorkflowRun,
    config: &WaitConfig,
) -> UpdateResult<Release> {
    let start = Instant::now();
    let poll_duration = Duration::from_secs(config.poll_interval_secs);
    let timeout_duration = Duration::from_secs(config.release_timeout_secs);

    let pb =
        spinner("[grans] {spinner} Waiting for GitHub to publish the release ({elapsed} elapsed)");

    loop {
        let release = github.fetch_latest_release(token)?;

        if release_is_from_build(&release, run) {
            pb.finish_and_clear();
            return Ok(release);
        }

        if start.elapsed() >= timeout_duration {
            pb.finish_and_clear();
            return Err(UpdateError::ReleaseNotFromBuild {
                tag: release.tag_name,
                sha: run.head_sha.chars().take(7).collect(),
            });
        }

        thread::sleep(poll_duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::test_support::{
        MockGitHubApi, completed_run, failed_run, in_progress_run, stale_release, test_release,
    };

    /// Poll without sleeping, so a multi-poll test runs instantly.
    fn immediate_polling() -> WaitConfig {
        WaitConfig {
            poll_interval_secs: 0,
            timeout_secs: 600,
            release_timeout_secs: 600,
        }
    }

    #[test]
    fn test_wait_config_default() {
        let config = WaitConfig::default();
        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(config.timeout_secs, 600);
        assert_eq!(config.release_timeout_secs, 120);
    }

    #[test]
    fn test_wait_config_with_timeout() {
        let config = WaitConfig::default().with_timeout(300);
        assert_eq!(config.timeout_secs, 300);
        assert_eq!(config.poll_interval_secs, 15);
    }

    #[test]
    fn test_wait_for_build_returns_when_the_build_finishes() {
        let github = MockGitHubApi::runs(vec![Ok(in_progress_run()), Ok(completed_run())]);

        let completed = wait_for_build(&github, None, &in_progress_run(), &immediate_polling())
            .expect("wait should succeed");

        assert_eq!(completed, completed_run());
        assert_eq!(github.run_calls.get(), 2);
    }

    #[test]
    fn test_wait_for_build_polls_the_run_it_was_given() {
        // Regression: polling "the newest Release run" followed a superseding
        // run when a later push cancelled the one the user consented to.
        let github = MockGitHubApi::runs(vec![Ok(completed_run())]);

        wait_for_build(&github, None, &in_progress_run(), &immediate_polling())
            .expect("wait should succeed");

        assert_eq!(
            *github.run_ids_requested.borrow(),
            vec![in_progress_run().id]
        );
    }

    #[test]
    fn test_wait_for_build_reports_a_cancelled_build() {
        // A newer push cancels the run in progress; the user waited for this
        // one, so its cancellation is the answer, not a reason to follow the
        // replacement.
        let github = MockGitHubApi::runs(vec![Ok(failed_run("cancelled"))]);

        let err =
            wait_for_build(&github, None, &in_progress_run(), &immediate_polling()).unwrap_err();

        assert!(matches!(
            err,
            UpdateError::BuildFailed { ref conclusion } if conclusion == "cancelled"
        ));
    }

    #[test]
    fn test_wait_for_build_fails_when_the_run_cannot_be_fetched() {
        // Regression: a run that vanished from the listing used to end the
        // wait with "no build" and fall through to an unverified install.
        let github = MockGitHubApi::runs(vec![Err(UpdateError::GitHubApi(
            "HTTP 404: Not Found".to_string(),
        ))]);

        let err =
            wait_for_build(&github, None, &in_progress_run(), &immediate_polling()).unwrap_err();

        assert!(matches!(err, UpdateError::GitHubApi(_)));
    }

    #[test]
    fn test_wait_for_build_reports_a_failed_build() {
        let github = MockGitHubApi::runs(vec![Ok(failed_run("failure"))]);

        let err =
            wait_for_build(&github, None, &in_progress_run(), &immediate_polling()).unwrap_err();

        assert!(matches!(
            err,
            UpdateError::BuildFailed { ref conclusion } if conclusion == "failure"
        ));
    }

    #[test]
    fn test_wait_for_build_times_out() {
        let github = MockGitHubApi::runs(vec![]);

        let config = WaitConfig {
            timeout_secs: 0,
            ..immediate_polling()
        };
        let err = wait_for_build(&github, None, &in_progress_run(), &config).unwrap_err();

        assert!(matches!(err, UpdateError::BuildTimeout { .. }));
        assert_eq!(github.run_calls.get(), 0);
    }

    #[test]
    fn test_release_is_from_build_matches_the_title_sha_against_head_sha() {
        assert!(release_is_from_build(&test_release(), &completed_run()));
        assert!(!release_is_from_build(&stale_release(), &completed_run()));
    }

    #[test]
    fn test_release_is_from_build_needs_a_sha_in_the_title() {
        let unnamed = Release {
            name: None,
            ..test_release()
        };
        assert!(!release_is_from_build(&unnamed, &completed_run()));

        let empty_parens = Release {
            name: Some("2025.1.30 ()".to_string()),
            ..test_release()
        };
        assert!(!release_is_from_build(&empty_parens, &completed_run()));
    }

    #[test]
    fn test_wait_for_release_returns_the_release_the_build_published() {
        let github = MockGitHubApi::releases(vec![Ok(test_release())]);

        let release = wait_for_release(&github, None, &completed_run(), &immediate_polling())
            .expect("release should match");

        assert_eq!(release.tag_name, test_release().tag_name);
        assert_eq!(github.release_calls.get(), 1);
    }

    #[test]
    fn test_wait_for_release_polls_past_a_stale_release() {
        // GitHub served the previous release right after the build finished.
        let github = MockGitHubApi::releases(vec![Ok(stale_release()), Ok(test_release())]);

        let release = wait_for_release(&github, None, &completed_run(), &immediate_polling())
            .expect("release should match on the second poll");

        assert_eq!(release.tag_name, test_release().tag_name);
        assert_eq!(github.release_calls.get(), 2);
    }

    #[test]
    fn test_wait_for_release_gives_up_after_the_release_timeout() {
        let github = MockGitHubApi::releases(vec![Ok(stale_release())]);

        let config = WaitConfig {
            release_timeout_secs: 0,
            ..immediate_polling()
        };
        let err = wait_for_release(&github, None, &completed_run(), &config).unwrap_err();

        assert!(matches!(
            err,
            UpdateError::ReleaseNotFromBuild { ref tag, ref sha }
                if tag == &stale_release().tag_name && sha == "abc1234"
        ));
    }
}
