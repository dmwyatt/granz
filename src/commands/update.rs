//! Update command implementation.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use crate::update::download::{
    current_binary_hash, download_asset, replace_binary, verify_checksum,
};
use crate::update::github::{
    BuildStatus, GitHubApi, RealGitHubApi, Release, WorkflowRun, find_asset,
};
use crate::update::platform::asset_name;
use crate::update::wait::{WaitConfig, display_build_info, wait_for_build};
use crate::update::{UpdateError, get_github_token_from_env, get_github_token_from_gh_cli};

/// Create a spinner with a consistent style for the update command.
fn create_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("[grans] {spinner} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message(message.to_string());
    pb
}

/// How `grans update` was invoked, as far as the build-status path cares.
#[derive(Debug, Clone, Copy)]
pub struct UpdateOptions {
    /// Report what is available, install nothing.
    pub check_only: bool,
    /// Wait out an in-progress build without asking (`--wait`).
    pub wait_for_build: bool,
    /// Give up on an in-progress build after this long.
    pub timeout_secs: u64,
}

/// The latest release, plus what it took to get it.
pub struct ReleaseFetch {
    pub release: Release,
    /// The token the successful calls used, if any.
    pub token: Option<String>,
    /// The build we sat through to reach this release, if we waited at all.
    pub waited_build: Option<WorkflowRun>,
}

/// Run the update command.
pub fn run(
    check_only: bool,
    use_gh_auth: bool,
    wait_for_build_flag: bool,
    timeout_secs: u64,
) -> Result<()> {
    let current_version = env!("GRANS_VERSION");
    println!("Current version: {}", current_version);
    println!();

    let options = UpdateOptions {
        check_only,
        wait_for_build: wait_for_build_flag,
        timeout_secs,
    };

    // Get token from environment if available, or from --use-gh-auth flag
    let mut token = get_initial_token(use_gh_auth);

    let spinner = create_spinner("Checking build status...");

    // Try to check build status and fetch release, prompting for auth if needed
    let ReleaseFetch {
        release,
        token,
        waited_build,
    } = fetch_with_auth_fallback(&mut token, options, &spinner)?;

    spinner.finish_and_clear();

    // Everything below reads this release: whether we are already up to date,
    // and which binary to install. If we waited for a build, a release that
    // predates it answers both questions about the wrong release.
    if let Some(ref run) = waited_build {
        ensure_release_came_from_build(&release, run)?;
    }

    // Find asset for this platform
    let expected_asset = asset_name()?;
    let asset = find_asset(&release, expected_asset).ok_or(UpdateError::AssetNotFound)?;

    // Check if we have a checksum
    let expected_sha256 = match asset.sha256() {
        Some(hash) => hash,
        None => {
            println!(
                "\n{}: Release asset has no checksum. Cannot determine update status.",
                "Warning".yellow()
            );
            display_release_info(&release, asset);
            return Ok(());
        }
    };

    // Compare current binary hash against release hash
    let current_hash = current_binary_hash()?;
    if current_hash == expected_sha256 {
        println!(
            "{} You are already running the latest version ({}).",
            "Up to date!".green().bold(),
            release.tag_name
        );
        return Ok(());
    }

    // Update available - display release info
    display_release_info(&release, asset);

    if check_only {
        return Ok(());
    }

    if !confirm_install(&RealPromptProvider, options, waited_build.as_ref())? {
        println!("Update cancelled.");
        return Ok(());
    }

    // Download
    println!();
    let content = download_asset(asset, token.as_deref())?;

    // Verify checksum
    print!("Verifying checksum... ");
    io::stdout().flush()?;
    verify_checksum(&content, expected_sha256)?;
    println!("{}", "OK".green());

    // Replace binary
    print!("Installing... ");
    io::stdout().flush()?;
    replace_binary(&content)?;
    println!("{}", "OK".green());

    println!(
        "\n{} Updated to {}",
        "Success!".green().bold(),
        release.tag_name
    );
    println!("Run 'grans --version' to verify.");

    Ok(())
}

/// Ask before installing, and return whether to go ahead.
///
/// Two things answer the question before it is asked. Waiting out an
/// in-progress build is the user's own go-ahead: they asked for that release
/// and sat through the build for it. So is `--wait`, whether or not a build
/// turned out to be running, because it is documented for scripts, and a
/// prompt there reads EOF and cancels the update the script asked for.
fn confirm_install<P: PromptProvider>(
    prompt_provider: &P,
    options: UpdateOptions,
    waited_build: Option<&WorkflowRun>,
) -> Result<bool> {
    if waited_build.is_some() {
        println!("\nInstalling the build you waited for.");
        return Ok(true);
    }

    if options.wait_for_build {
        return Ok(true);
    }

    prompt_provider.prompt_yes_no("\nDownload and install? [y/N] ")
}

/// Check that `release` is the one the build we waited for published.
///
/// The release workflow creates the release before the run reports completion,
/// so anything published before that run even started is a stale answer from
/// the GitHub API, not the binary that was just built. Nobody watches a wait to
/// its end, so that case is an error rather than a prompt: quietly installing
/// some other binary, or reporting "up to date" off a stale release, is the
/// outcome worth avoiding.
fn ensure_release_came_from_build(release: &Release, run: &WorkflowRun) -> Result<()> {
    let published_at = release.published_at.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Release {} has no publish time, so grans cannot tell whether it came from the build it waited for.",
            release.tag_name
        )
    })?;

    let published = parse_github_timestamp(published_at)?;
    let build_started = parse_github_timestamp(&run.created_at)?;

    if published < build_started {
        return Err(anyhow::anyhow!(
            "The latest release ({}, published {}) predates the build that just finished, so it is not the build you waited for. GitHub may not have published it yet; run 'grans update' again in a moment.",
            release.tag_name,
            published_at
        ));
    }

    Ok(())
}

fn parse_github_timestamp(ts: &str) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map_err(|e| anyhow::anyhow!("Could not read GitHub timestamp {:?}: {}", ts, e))
}

/// Get initial token from environment or --use-gh-auth flag.
///
/// Does NOT prompt the user - that happens later if needed.
fn get_initial_token(use_gh_auth: bool) -> Option<String> {
    // First, check if user has env var set
    if let Some(token) = get_github_token_from_env() {
        return Some(token);
    }

    // If --use-gh-auth flag was passed, use gh CLI automatically
    if use_gh_auth {
        if let Some(token) = get_github_token_from_gh_cli() {
            println!("Using gh CLI authentication...");
            return Some(token);
        }
    }

    None
}

/// Try to fetch build status and release, falling back to auth if needed.
///
/// First tries without auth (works for public repos). If that fails with 404,
/// prompts for gh CLI auth and retries.
fn fetch_with_auth_fallback(
    token: &mut Option<String>,
    options: UpdateOptions,
    spinner: &ProgressBar,
) -> Result<ReleaseFetch> {
    fetch_with_auth_fallback_impl(
        &RealGitHubApi,
        &RealAuthProvider,
        &RealPromptProvider,
        token,
        options,
        spinner,
    )
}

/// Handle the result of a build status check.
///
/// Returns the in-progress build we waited for, if we waited for one.
fn handle_build_status_result<G: GitHubApi, P: PromptProvider>(
    github: &G,
    prompt_provider: &P,
    result: Result<BuildStatus, UpdateError>,
    token: Option<&str>,
    options: UpdateOptions,
) -> Result<Option<WorkflowRun>> {
    match result {
        Ok(BuildStatus::InProgress(ref run)) => {
            return handle_in_progress_build(github, prompt_provider, run, options, token);
        }
        Ok(BuildStatus::Completed(_) | BuildStatus::Idle) => {
            // No active build, continue normally
        }
        Ok(BuildStatus::Failed(ref run)) => {
            let conclusion = run
                .conclusion
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            println!(
                "{}: Recent build failed with: {}",
                "Note".yellow(),
                conclusion
            );
        }
        Err(e) => {
            if options.wait_for_build {
                println!(
                    "{}: Could not check build status: {}",
                    "Warning".yellow(),
                    e
                );
                println!("The --wait flag may not work without authentication.");
            }
        }
    }
    Ok(None)
}

/// Handle an in-progress build: display info and optionally wait.
///
/// Returns the build we waited for, or `None` if we did not wait.
fn handle_in_progress_build<G: GitHubApi, P: PromptProvider>(
    github: &G,
    prompt_provider: &P,
    run: &WorkflowRun,
    options: UpdateOptions,
    token: Option<&str>,
) -> Result<Option<WorkflowRun>> {
    display_build_info(run);

    if options.check_only {
        // Just report, don't wait or prompt
        return Ok(None);
    }

    let waiting = if options.wait_for_build {
        // Auto-wait (for scripts)
        println!();
        true
    } else {
        prompt_provider.prompt_yes_no("\nWould you like to wait for it to complete? [y/N] ")?
    };

    if !waiting {
        println!("Continuing without waiting...");
        return Ok(None);
    }

    let config = WaitConfig::default().with_timeout(options.timeout_secs);
    wait_for_build(github, token, &config).map_err(Into::into)
}

fn display_release_info(release: &Release, asset: &crate::update::github::Asset) {
    println!();
    println!("{}", "Latest Release".bold());
    println!("{}", "--------------".dimmed());
    println!("Version:  {}", release.tag_name);

    if let Some(ref name) = release.name {
        if name != &release.tag_name {
            println!("Name:     {}", name);
        }
    }

    if let Some(ref published) = release.published_at {
        // Format: "2025-01-27T10:30:00Z" -> "2025-01-27 10:30 UTC"
        let formatted = published
            .replace('T', " ")
            .replace('Z', " UTC")
            .chars()
            .take(20)
            .collect::<String>()
            + " UTC";
        println!("Published: {}", formatted.dimmed());
    }

    println!("Size:     {:.2} MB", asset.size as f64 / 1_048_576.0);

    if asset.sha256().is_some() {
        println!("Checksum: {}", "SHA256 available".dimmed());
    } else {
        println!("Checksum: {}", "Not available".yellow());
    }

    if let Some(ref body) = release.body {
        if !body.is_empty() {
            println!();
            println!("{}", "Release Notes".bold());
            println!("{}", "-------------".dimmed());
            // Limit to first 500 chars for display
            let truncated = if body.len() > 500 {
                format!("{}...", &body[..500])
            } else {
                body.clone()
            };
            println!("{}", truncated);
        }
    }
}

/// Trait for auth token providers, allowing for mocking in tests.
pub trait AuthProvider {
    /// Get token from gh CLI if available.
    fn get_gh_cli_token(&self) -> Option<String>;
}

/// Real auth provider that uses the gh CLI.
pub struct RealAuthProvider;

impl AuthProvider for RealAuthProvider {
    fn get_gh_cli_token(&self) -> Option<String> {
        get_github_token_from_gh_cli()
    }
}

/// Trait for user prompts, allowing for mocking in tests.
pub trait PromptProvider {
    /// Ask user yes/no question, returns true if yes.
    fn prompt_yes_no(&self, message: &str) -> Result<bool>;
}

/// Real prompt provider that uses stdin/stdout.
pub struct RealPromptProvider;

impl PromptProvider for RealPromptProvider {
    fn prompt_yes_no(&self, message: &str) -> Result<bool> {
        print!("{}", message);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        Ok(input.trim().eq_ignore_ascii_case("y"))
    }
}

/// Core auth flow logic, extracted for testability.
///
/// Tries API calls without auth first (for public repos).
/// If 404, prompts for auth and retries.
pub fn fetch_with_auth_fallback_impl<G: GitHubApi, A: AuthProvider, P: PromptProvider>(
    github: &G,
    auth_provider: &A,
    prompt_provider: &P,
    token: &mut Option<String>,
    options: UpdateOptions,
    spinner: &ProgressBar,
) -> Result<ReleaseFetch> {
    // Try build status check first
    let build_status_result = github.check_build_status(token.as_deref());

    // Check if we need auth (404 means private repo or auth required)
    let needs_auth =
        matches!(&build_status_result, Err(UpdateError::GitHubApi(msg)) if msg.contains("404"));

    let waited_build = if needs_auth && token.is_none() {
        // Suspend spinner for interactive auth prompt
        let new_token =
            spinner.suspend(|| prompt_for_gh_auth_impl(auth_provider, prompt_provider))?;
        if let Some(new_token) = new_token {
            *token = Some(new_token);
            // Retry build status with auth
            let retry_result = github.check_build_status(token.as_deref());
            // Suspend spinner for build status handling (may prompt to wait)
            spinner.suspend(|| {
                handle_build_status_result(
                    github,
                    prompt_provider,
                    retry_result,
                    token.as_deref(),
                    options,
                )
            })?
        } else {
            // User declined auth, continue without (will likely fail on release fetch)
            if options.wait_for_build {
                spinner.suspend(|| {
                    println!(
                        "{}: Could not check build status without authentication.",
                        "Warning".yellow()
                    );
                    println!("The --wait flag may not work.");
                });
            }
            None
        }
    } else {
        // No auth needed or we already have a token - handle the result
        spinner.suspend(|| {
            handle_build_status_result(
                github,
                prompt_provider,
                build_status_result,
                token.as_deref(),
                options,
            )
        })?
    };

    // Now fetch release
    spinner.set_message("Checking for updates...");
    match github.fetch_latest_release(token.as_deref()) {
        Ok(release) => Ok(ReleaseFetch {
            release,
            token: token.clone(),
            waited_build,
        }),
        Err(UpdateError::NotFound { has_token: false }) if token.is_none() => {
            // Suspend spinner for interactive auth prompt
            let new_token =
                spinner.suspend(|| prompt_for_gh_auth_impl(auth_provider, prompt_provider))?;
            if let Some(new_token) = new_token {
                *token = Some(new_token);
                let release = github.fetch_latest_release(token.as_deref())?;
                Ok(ReleaseFetch {
                    release,
                    token: token.clone(),
                    waited_build,
                })
            } else {
                Err(anyhow::anyhow!(
                    "Release not found. If this is a private repository, either:\n  \
                     - Install and authenticate the gh CLI: gh auth login\n  \
                     - Set GH_TOKEN or GITHUB_TOKEN environment variable"
                ))
            }
        }
        Err(e) => Err(e.into()),
    }
}

/// Prompt user for gh CLI authentication using providers.
fn prompt_for_gh_auth_impl<A: AuthProvider, P: PromptProvider>(
    auth_provider: &A,
    prompt_provider: &P,
) -> Result<Option<String>> {
    if let Some(gh_token) = auth_provider.get_gh_cli_token() {
        println!();
        println!(
            "{}: Authentication required. The gh CLI is available.",
            "Note".yellow()
        );
        if prompt_provider.prompt_yes_no("Use gh auth token? [y/N] ")? {
            return Ok(Some(gh_token));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::github::{Asset, BuildStatus, Release};
    use crate::update::test_support::{
        MockGitHubApi, completed_run, in_progress_run, not_found_error, test_release,
    };
    use indicatif::ProgressBar;
    use std::cell::{Cell, RefCell};

    /// The options a plain `grans update --check` runs with.
    fn check_only_options() -> UpdateOptions {
        UpdateOptions {
            check_only: true,
            wait_for_build: false,
            timeout_secs: 600,
        }
    }

    #[test]
    fn test_display_release_info_does_not_panic() {
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            name: Some("Version 1.0.0".to_string()),
            body: Some("Release notes here".to_string()),
            published_at: Some("2025-01-27T10:30:00Z".to_string()),
            assets: vec![],
        };

        let asset = Asset {
            name: "grans-linux-x86_64".to_string(),
            size: 10_000_000,
            url: "https://api.github.com/repos/test/test/releases/assets/123".to_string(),
            browser_download_url: "https://example.com".to_string(),
            digest: Some("sha256:abc123".to_string()),
        };

        // Just verify it doesn't panic
        display_release_info(&release, &asset);
    }

    // Mock implementations for testing

    struct MockAuthProvider {
        token: Option<String>,
    }

    impl AuthProvider for MockAuthProvider {
        fn get_gh_cli_token(&self) -> Option<String> {
            self.token.clone()
        }
    }

    struct MockPromptProvider {
        responses: RefCell<Vec<bool>>,
        call_count: Cell<usize>,
    }

    impl MockPromptProvider {
        fn new(responses: Vec<bool>) -> Self {
            Self {
                responses: RefCell::new(responses),
                call_count: Cell::new(0),
            }
        }
    }

    impl PromptProvider for MockPromptProvider {
        fn prompt_yes_no(&self, _message: &str) -> Result<bool> {
            let idx = self.call_count.get();
            self.call_count.set(idx + 1);
            let responses = self.responses.borrow();
            if idx < responses.len() {
                Ok(responses[idx])
            } else {
                panic!("Unexpected prompt call (call #{})", idx + 1);
            }
        }
    }

    #[test]
    fn test_public_repo_no_auth_needed() {
        // Public repo: both build status and release work without auth
        let github = MockGitHubApi::new(vec![Ok(BuildStatus::Idle)], vec![Ok(test_release())]);
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            check_only_options(),
            &ProgressBar::hidden(),
        );

        assert!(result.is_ok());
        assert!(token.is_none()); // No auth was needed
        assert_eq!(github.build_status_calls.get(), 1);
        assert_eq!(github.release_calls.get(), 1);
    }

    #[test]
    fn test_private_repo_auth_required_user_accepts() {
        // Private repo: 404 without auth, succeeds with auth
        let github = MockGitHubApi::new(
            vec![Err(not_found_error()), Ok(BuildStatus::Idle)],
            vec![Ok(test_release())],
        );
        let auth = MockAuthProvider {
            token: Some("test_token".to_string()),
        };
        let prompt = MockPromptProvider::new(vec![true]); // User says yes

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            check_only_options(),
            &ProgressBar::hidden(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string())); // Auth was obtained
        assert_eq!(github.build_status_calls.get(), 2); // Called twice (retry)
        assert_eq!(github.release_calls.get(), 1);
    }

    #[test]
    fn test_private_repo_auth_required_user_declines() {
        // Private repo: 404 without auth, user declines auth
        let github = MockGitHubApi::new(
            vec![Err(not_found_error())],
            vec![Err(UpdateError::NotFound { has_token: false })],
        );
        let auth = MockAuthProvider {
            token: Some("test_token".to_string()),
        };
        let prompt = MockPromptProvider::new(vec![false, false]); // User says no twice

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            check_only_options(),
            &ProgressBar::hidden(),
        );

        assert!(result.is_err()); // Should fail
        assert!(token.is_none()); // No auth was obtained
    }

    #[test]
    fn test_with_preexisting_token() {
        // User already has a token (from env or --use-gh-auth)
        let github = MockGitHubApi::new(vec![Ok(BuildStatus::Idle)], vec![Ok(test_release())]);
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]); // Should not be called

        let mut token = Some("existing_token".to_string());
        let result = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            check_only_options(),
            &ProgressBar::hidden(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("existing_token".to_string()));
        // No prompts should have been made
        assert_eq!(prompt.call_count.get(), 0);
    }

    #[test]
    fn test_build_in_progress_detected_with_auth() {
        // Build is in progress, auth required to detect it
        let github = MockGitHubApi::new(
            vec![
                Err(not_found_error()),
                Ok(BuildStatus::InProgress(in_progress_run())),
            ],
            vec![Ok(test_release())],
        );
        let auth = MockAuthProvider {
            token: Some("test_token".to_string()),
        };
        let prompt = MockPromptProvider::new(vec![true]); // Accept auth

        let mut token = None;
        // check_only=true so we don't try to wait
        let result = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            check_only_options(),
            &ProgressBar::hidden(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.build_status_calls.get(), 2); // Retried with auth
    }

    /// A release published while `in_progress_run()` was running.
    fn release_published_at(published_at: &str) -> Release {
        Release {
            published_at: Some(published_at.to_string()),
            ..test_release()
        }
    }

    #[test]
    fn test_install_is_not_confirmed_again_after_waiting() {
        let prompt = MockPromptProvider::new(vec![]); // Panics if asked

        let approved = confirm_install(&prompt, install_options(false), Some(&completed_run()))
            .expect("should proceed");

        assert!(approved);
        assert_eq!(prompt.call_count.get(), 0);
    }

    #[test]
    fn test_the_wait_flag_installs_even_when_no_build_was_running() {
        // Nothing was building, so there was nothing to wait for, but --wait
        // still says nobody is at the terminal to answer a prompt.
        let prompt = MockPromptProvider::new(vec![]); // Panics if asked

        let approved =
            confirm_install(&prompt, install_options(true), None).expect("should proceed");

        assert!(approved);
        assert_eq!(prompt.call_count.get(), 0);
    }

    #[test]
    fn test_install_is_confirmed_when_we_did_not_wait() {
        let accepts = MockPromptProvider::new(vec![true]);
        assert!(confirm_install(&accepts, install_options(false), None).expect("should proceed"));
        assert_eq!(accepts.call_count.get(), 1);

        let declines = MockPromptProvider::new(vec![false]);
        assert!(
            !confirm_install(&declines, install_options(false), None).expect("should not proceed")
        );
    }

    #[test]
    fn test_a_release_published_during_the_build_is_accepted() {
        let release = release_published_at("2025-01-30T12:08:00Z");

        ensure_release_came_from_build(&release, &in_progress_run())
            .expect("release came from the build");
    }

    #[test]
    fn test_a_release_older_than_the_build_we_waited_for_is_refused() {
        // GitHub served a release that predates the run we waited for, so it
        // cannot be the build's output. Installing it, or reporting "up to
        // date" from it, would answer for a release the user never asked about.
        let release = release_published_at("2025-01-29T09:00:00Z");

        let err = ensure_release_came_from_build(&release, &in_progress_run())
            .expect_err("should refuse the stale release");

        assert!(err.to_string().contains("v2025.1.30"), "{}", err);
    }

    #[test]
    fn test_a_release_with_no_publish_time_is_refused_after_waiting() {
        let release = Release {
            published_at: None,
            ..test_release()
        };

        let err = ensure_release_came_from_build(&release, &in_progress_run())
            .expect_err("should refuse the unverifiable release");

        assert!(err.to_string().contains("publish"), "{}", err);
    }

    /// A build that finishes on the wait loop's first poll, so no sleeping.
    fn build_finishes_while_waiting() -> MockGitHubApi {
        MockGitHubApi::new(
            vec![
                Ok(BuildStatus::InProgress(in_progress_run())),
                Ok(BuildStatus::Completed(completed_run())),
            ],
            vec![Ok(test_release())],
        )
    }

    fn install_options(wait_for_build: bool) -> UpdateOptions {
        UpdateOptions {
            check_only: false,
            wait_for_build,
            timeout_secs: 600,
        }
    }

    #[test]
    fn test_wait_flag_reports_that_we_waited() {
        // Regression: --wait used to fall through to the install confirmation,
        // which reads EOF in a script and cancels the update it just waited for.
        let github = build_finishes_while_waiting();
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]); // --wait asks nothing

        let mut token = Some("existing_token".to_string());
        let fetched = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            install_options(true),
            &ProgressBar::hidden(),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_some());
        assert_eq!(prompt.call_count.get(), 0);
    }

    #[test]
    fn test_accepting_the_wait_prompt_reports_that_we_waited() {
        let github = build_finishes_while_waiting();
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![true]); // Yes, wait for it

        let mut token = Some("existing_token".to_string());
        let fetched = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            install_options(false),
            &ProgressBar::hidden(),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_some());
    }

    #[test]
    fn test_declining_the_wait_prompt_reports_that_we_did_not_wait() {
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::InProgress(in_progress_run()))],
            vec![Ok(test_release())],
        );
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![false]); // No, don't wait

        let mut token = Some("existing_token".to_string());
        let fetched = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            install_options(false),
            &ProgressBar::hidden(),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_none());
    }

    #[test]
    fn test_no_build_in_progress_reports_that_we_did_not_wait() {
        let github = MockGitHubApi::new(vec![Ok(BuildStatus::Idle)], vec![Ok(test_release())]);
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let fetched = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            install_options(false),
            &ProgressBar::hidden(),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_none());
    }

    #[test]
    fn test_check_only_never_waits_even_with_the_wait_flag() {
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::InProgress(in_progress_run()))],
            vec![Ok(test_release())],
        );
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let options = UpdateOptions {
            check_only: true,
            wait_for_build: true,
            timeout_secs: 600,
        };
        let fetched = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            options,
            &ProgressBar::hidden(),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_none());
        assert_eq!(github.build_status_calls.get(), 1); // No polling loop
    }

    #[test]
    fn test_release_fetch_triggers_auth_if_build_status_succeeded() {
        // Build status works without auth, but release fetch requires auth
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::Idle)],
            vec![
                Err(UpdateError::NotFound { has_token: false }),
                Ok(test_release()),
            ],
        );
        let auth = MockAuthProvider {
            token: Some("test_token".to_string()),
        };
        let prompt = MockPromptProvider::new(vec![true]); // Accept auth for release

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github,
            &auth,
            &prompt,
            &mut token,
            check_only_options(),
            &ProgressBar::hidden(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.release_calls.get(), 2); // Retried with auth
    }
}
