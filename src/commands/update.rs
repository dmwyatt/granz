//! Update command implementation.

use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;

use crate::update::download::{current_binary_hash, download_asset, replace_binary, verify_checksum};
use crate::update::github::{find_asset, BuildStatus, Release};
use crate::update::platform::asset_name;
use crate::update::wait::{display_build_info, wait_for_build, WaitConfig};
use crate::update::{get_github_token_from_env, get_github_token_from_gh_cli, UpdateError};

/// Run the update command.
pub fn run(check_only: bool, use_gh_auth: bool, wait_for_build_flag: bool, timeout_secs: u64) -> Result<()> {
    let current_version = env!("GRANS_VERSION");
    println!("Current version: {}", current_version);
    println!();

    // Get token from environment if available, or from --use-gh-auth flag
    let mut token = get_initial_token(use_gh_auth);

    // Try to check build status and fetch release, prompting for auth if needed
    let (release, token) = fetch_with_auth_fallback(&mut token, check_only, wait_for_build_flag, timeout_secs)?;

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

    // Prompt for confirmation
    print!("\nDownload and install? [y/N] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if !input.trim().eq_ignore_ascii_case("y") {
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
    check_only: bool,
    wait_for_build_flag: bool,
    timeout_secs: u64,
) -> Result<(Release, Option<String>)> {
    use crate::update::github::RealGitHubApi;
    fetch_with_auth_fallback_impl(
        &RealGitHubApi,
        &RealAuthProvider,
        &RealPromptProvider,
        token,
        check_only,
        wait_for_build_flag,
        timeout_secs,
    )
}


/// Handle the result of a build status check.
fn handle_build_status_result(
    result: Result<BuildStatus, UpdateError>,
    token: Option<&str>,
    check_only: bool,
    wait_flag: bool,
    timeout_secs: u64,
) -> Result<()> {
    match result {
        Ok(BuildStatus::InProgress(ref run)) => {
            handle_in_progress_build(run, check_only, wait_flag, timeout_secs, token)?;
        }
        Ok(BuildStatus::Completed(_) | BuildStatus::Idle) => {
            // No active build, continue normally
        }
        Ok(BuildStatus::Failed(ref run)) => {
            let conclusion = run.conclusion.clone().unwrap_or_else(|| "unknown".to_string());
            println!(
                "{}: Recent build failed with: {}",
                "Note".yellow(),
                conclusion
            );
        }
        Err(e) => {
            if wait_flag {
                println!(
                    "{}: Could not check build status: {}",
                    "Warning".yellow(),
                    e
                );
                println!("The --wait flag may not work without authentication.");
            }
        }
    }
    Ok(())
}


/// Handle an in-progress build: display info and optionally wait.
fn handle_in_progress_build(
    run: &crate::update::github::WorkflowRun,
    check_only: bool,
    wait_flag: bool,
    timeout_secs: u64,
    token: Option<&str>,
) -> Result<()> {
    display_build_info(run);

    if check_only {
        // Just report, don't wait or prompt
        return Ok(());
    }

    if wait_flag {
        // Auto-wait (for scripts)
        println!();
        let config = WaitConfig::default().with_timeout(timeout_secs);
        wait_for_build(token, &config)?;
    } else {
        // Interactive: prompt user
        print!("\nWould you like to wait for it to complete? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().eq_ignore_ascii_case("y") {
            let config = WaitConfig::default().with_timeout(timeout_secs);
            wait_for_build(token, &config)?;
        } else {
            println!("Continuing without waiting...");
        }
    }

    Ok(())
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

    println!(
        "Size:     {:.2} MB",
        asset.size as f64 / 1_048_576.0
    );

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

use crate::update::github::GitHubApi;

/// Core auth flow logic, extracted for testability.
///
/// Tries API calls without auth first (for public repos).
/// If 404, prompts for auth and retries.
pub fn fetch_with_auth_fallback_impl<G: GitHubApi, A: AuthProvider, P: PromptProvider>(
    github: &G,
    auth_provider: &A,
    prompt_provider: &P,
    token: &mut Option<String>,
    check_only: bool,
    wait_for_build_flag: bool,
    timeout_secs: u64,
) -> Result<(Release, Option<String>)> {
    // Try build status check first
    let build_status_result = github.check_build_status(token.as_deref());

    // Check if we need auth (404 means private repo or auth required)
    let needs_auth = matches!(&build_status_result, Err(UpdateError::GitHubApi(msg)) if msg.contains("404"));

    if needs_auth && token.is_none() {
        // Try to get auth from gh CLI
        if let Some(new_token) = prompt_for_gh_auth_impl(auth_provider, prompt_provider)? {
            *token = Some(new_token);
            // Retry build status with auth
            let retry_result = github.check_build_status(token.as_deref());
            handle_build_status_result(retry_result, token.as_deref(), check_only, wait_for_build_flag, timeout_secs)?;
        } else {
            // User declined auth, continue without (will likely fail on release fetch)
            if wait_for_build_flag {
                println!(
                    "{}: Could not check build status without authentication.",
                    "Warning".yellow()
                );
                println!("The --wait flag may not work.");
            }
        }
    } else {
        // No auth needed or we already have a token - handle the result
        handle_build_status_result(build_status_result, token.as_deref(), check_only, wait_for_build_flag, timeout_secs)?;
    }

    // Now fetch release
    println!("Checking for updates...");
    match github.fetch_latest_release(token.as_deref()) {
        Ok(release) => Ok((release, token.clone())),
        Err(UpdateError::NotFound { has_token: false }) if token.is_none() => {
            // Try to get auth from gh CLI (if we haven't already)
            if let Some(new_token) = prompt_for_gh_auth_impl(auth_provider, prompt_provider)? {
                *token = Some(new_token);
                let release = github.fetch_latest_release(token.as_deref())?;
                Ok((release, token.clone()))
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
    use crate::update::github::{Asset, BuildStatus, GitHubApi, Release, WorkflowRun};
    use std::cell::{Cell, RefCell};

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

    use std::collections::VecDeque;

    struct MockGitHubApi {
        build_status_responses: RefCell<VecDeque<Result<BuildStatus, UpdateError>>>,
        release_responses: RefCell<VecDeque<Result<Release, UpdateError>>>,
        build_status_calls: Cell<usize>,
        release_calls: Cell<usize>,
    }

    impl MockGitHubApi {
        fn new(
            build_status_responses: Vec<Result<BuildStatus, UpdateError>>,
            release_responses: Vec<Result<Release, UpdateError>>,
        ) -> Self {
            Self {
                build_status_responses: RefCell::new(build_status_responses.into()),
                release_responses: RefCell::new(release_responses.into()),
                build_status_calls: Cell::new(0),
                release_calls: Cell::new(0),
            }
        }
    }

    impl GitHubApi for MockGitHubApi {
        fn fetch_latest_release(&self, _token: Option<&str>) -> crate::update::UpdateResult<Release> {
            self.release_calls.set(self.release_calls.get() + 1);
            self.release_responses
                .borrow_mut()
                .pop_front()
                .expect("Unexpected call to fetch_latest_release")
        }

        fn check_build_status(&self, _token: Option<&str>) -> crate::update::UpdateResult<BuildStatus> {
            self.build_status_calls.set(self.build_status_calls.get() + 1);
            self.build_status_responses
                .borrow_mut()
                .pop_front()
                .expect("Unexpected call to check_build_status")
        }
    }

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

    fn make_test_release() -> Release {
        Release {
            tag_name: "v2025.1.30".to_string(),
            name: Some("2025.1.30 (abc1234)".to_string()),
            body: None,
            published_at: Some("2025-01-30T12:00:00Z".to_string()),
            assets: vec![],
        }
    }

    fn make_in_progress_run() -> WorkflowRun {
        WorkflowRun {
            id: 12345,
            name: Some("Release".to_string()),
            status: "in_progress".to_string(),
            conclusion: None,
            html_url: "https://github.com/test/test/actions/runs/12345".to_string(),
            created_at: "2025-01-30T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_public_repo_no_auth_needed() {
        // Public repo: both build status and release work without auth
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::Idle)],
            vec![Ok(make_test_release())],
        );
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github, &auth, &prompt,
            &mut token, true, false, 600,
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
            vec![
                Err(UpdateError::GitHubApi("HTTP 404: Not Found".to_string())),
                Ok(BuildStatus::Idle),
            ],
            vec![Ok(make_test_release())],
        );
        let auth = MockAuthProvider { token: Some("test_token".to_string()) };
        let prompt = MockPromptProvider::new(vec![true]); // User says yes

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github, &auth, &prompt,
            &mut token, true, false, 600,
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
            vec![Err(UpdateError::GitHubApi("HTTP 404: Not Found".to_string()))],
            vec![Err(UpdateError::NotFound { has_token: false })],
        );
        let auth = MockAuthProvider { token: Some("test_token".to_string()) };
        let prompt = MockPromptProvider::new(vec![false, false]); // User says no twice

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github, &auth, &prompt,
            &mut token, true, false, 600,
        );

        assert!(result.is_err()); // Should fail
        assert!(token.is_none()); // No auth was obtained
    }

    #[test]
    fn test_with_preexisting_token() {
        // User already has a token (from env or --use-gh-auth)
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::Idle)],
            vec![Ok(make_test_release())],
        );
        let auth = MockAuthProvider { token: None };
        let prompt = MockPromptProvider::new(vec![]); // Should not be called

        let mut token = Some("existing_token".to_string());
        let result = fetch_with_auth_fallback_impl(
            &github, &auth, &prompt,
            &mut token, true, false, 600,
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
                Err(UpdateError::GitHubApi("HTTP 404: Not Found".to_string())),
                Ok(BuildStatus::InProgress(make_in_progress_run())),
            ],
            vec![Ok(make_test_release())],
        );
        let auth = MockAuthProvider { token: Some("test_token".to_string()) };
        let prompt = MockPromptProvider::new(vec![true]); // Accept auth

        let mut token = None;
        // check_only=true so we don't try to wait
        let result = fetch_with_auth_fallback_impl(
            &github, &auth, &prompt,
            &mut token, true, true, 600,
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.build_status_calls.get(), 2); // Retried with auth
    }

    #[test]
    fn test_release_fetch_triggers_auth_if_build_status_succeeded() {
        // Build status works without auth, but release fetch requires auth
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::Idle)],
            vec![
                Err(UpdateError::NotFound { has_token: false }),
                Ok(make_test_release()),
            ],
        );
        let auth = MockAuthProvider { token: Some("test_token".to_string()) };
        let prompt = MockPromptProvider::new(vec![true]); // Accept auth for release

        let mut token = None;
        let result = fetch_with_auth_fallback_impl(
            &github, &auth, &prompt,
            &mut token, true, false, 600,
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.release_calls.get(), 2); // Retried with auth
    }
}
