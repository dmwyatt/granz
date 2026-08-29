//! Reaching the latest release: the build-status check, waiting on a build,
//! attributing the release to it, and falling back to gh CLI auth on the way.

use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;
use indicatif::ProgressBar;

use super::github::{BuildStatus, GitHubApi, Release, WorkflowRun};
use super::wait::{
    WaitConfig, display_build_info, release_is_from_build, wait_for_build, wait_for_release,
};
use super::{UpdateError, get_github_token_from_gh_cli};

/// How `grans update` was invoked.
#[derive(Debug, Clone, Copy)]
pub struct UpdateOptions {
    /// Report what is available, install nothing.
    pub check_only: bool,
    /// Take the gh CLI's token without asking (`--use-gh-auth`).
    pub use_gh_auth: bool,
    /// Wait out an in-progress build without asking (`--wait`).
    pub wait_for_build: bool,
    /// Give up on an in-progress build after this long.
    pub timeout_secs: u64,
}

/// The latest release, plus what it took to get it.
#[derive(Debug)]
pub struct ReleaseFetch {
    pub release: Release,
    /// The token the successful calls used, if any.
    pub token: Option<String>,
    /// The build we sat through to reach this release, if we waited at all.
    /// When set, `release` has been checked to be that build's output.
    pub waited_build: Option<WorkflowRun>,
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

/// Fetch the latest release, waiting for an in-progress build first if asked.
///
/// Tries API calls without auth first (for public repos). If 404, prompts for
/// auth and retries. `token` is updated with any token obtained on the way.
pub fn fetch_release<G: GitHubApi, A: AuthProvider, P: PromptProvider>(
    github: &G,
    auth_provider: &A,
    prompt_provider: &P,
    token: &mut Option<String>,
    options: UpdateOptions,
    wait_config: &WaitConfig,
    spinner: &ProgressBar,
) -> Result<ReleaseFetch> {
    let waited_build = check_build(
        github,
        auth_provider,
        prompt_provider,
        token,
        options,
        wait_config,
        spinner,
    )?;

    spinner.set_message("Checking for updates...");
    let mut release =
        fetch_latest_release_with_auth(github, auth_provider, prompt_provider, token, spinner)?;

    if let Some(run) = &waited_build
        && !release_is_from_build(&release, run)
    {
        release =
            spinner.suspend(|| wait_for_release(github, token.as_deref(), run, wait_config))?;
    }

    Ok(ReleaseFetch {
        release,
        token: token.clone(),
        waited_build,
    })
}

/// Check build status, obtaining auth if the check needs it, and wait for an
/// in-progress build when the options or the user say to.
///
/// Returns the build we waited for, if we waited for one.
fn check_build<G: GitHubApi, A: AuthProvider, P: PromptProvider>(
    github: &G,
    auth_provider: &A,
    prompt_provider: &P,
    token: &mut Option<String>,
    options: UpdateOptions,
    wait_config: &WaitConfig,
    spinner: &ProgressBar,
) -> Result<Option<WorkflowRun>> {
    let build_status_result = github.check_build_status(token.as_deref());

    // 404 means private repo or auth required
    let needs_auth =
        matches!(&build_status_result, Err(UpdateError::GitHubApi(msg)) if msg.contains("404"));

    if !(needs_auth && token.is_none()) {
        return spinner.suspend(|| {
            handle_build_status_result(
                github,
                prompt_provider,
                build_status_result,
                token.as_deref(),
                options,
                wait_config,
            )
        });
    }

    let new_token = spinner.suspend(|| prompt_for_gh_auth(auth_provider, prompt_provider))?;
    let Some(new_token) = new_token else {
        return spinner.suspend(|| build_status_unavailable_without_auth(options));
    };

    *token = Some(new_token);
    let retry_result = github.check_build_status(token.as_deref());
    spinner.suspend(|| {
        handle_build_status_result(
            github,
            prompt_provider,
            retry_result,
            token.as_deref(),
            options,
            wait_config,
        )
    })
}

/// The user declined auth, so the build status is unknown.
///
/// Interactively that is fine: the release fetch will likely fail on its own,
/// and if it does not, the user still confirms the install. Under `--wait`
/// nobody confirms anything, and the flag's whole promise is to install the
/// build that is running, so not knowing whether one is running is fatal.
fn build_status_unavailable_without_auth(options: UpdateOptions) -> Result<Option<WorkflowRun>> {
    if options.wait_for_build {
        return Err(anyhow::anyhow!(
            "Could not check build status without authentication, so --wait cannot tell whether a build is running. Pass --use-gh-auth or set GH_TOKEN."
        ));
    }
    Ok(None)
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
    wait_config: &WaitConfig,
) -> Result<Option<WorkflowRun>> {
    match result {
        Ok(BuildStatus::InProgress(ref run)) => {
            handle_in_progress_build(github, prompt_provider, run, options, token, wait_config)
        }
        Ok(BuildStatus::Completed(_) | BuildStatus::Idle) => Ok(None),
        Ok(BuildStatus::Failed(ref run)) => {
            let conclusion = run.conclusion.as_deref().unwrap_or("unknown");
            println!(
                "{}: Recent build failed with: {}",
                "Note".yellow(),
                conclusion
            );
            Ok(None)
        }
        // Same reasoning as build_status_unavailable_without_auth: --wait
        // installs unattended, so it must not proceed blind.
        Err(e) if options.wait_for_build => Err(anyhow::anyhow!(
            "Could not check build status: {}\n--wait cannot tell whether a build is running, so it stops here rather than install unattended.",
            e
        )),
        Err(_) => Ok(None),
    }
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
    wait_config: &WaitConfig,
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

    Ok(Some(wait_for_build(github, token, run, wait_config)?))
}

/// Fetch the latest release, prompting for gh CLI auth if the unauthenticated
/// fetch comes back 404.
fn fetch_latest_release_with_auth<G: GitHubApi, A: AuthProvider, P: PromptProvider>(
    github: &G,
    auth_provider: &A,
    prompt_provider: &P,
    token: &mut Option<String>,
    spinner: &ProgressBar,
) -> Result<Release> {
    match github.fetch_latest_release(token.as_deref()) {
        Ok(release) => Ok(release),
        Err(UpdateError::NotFound { has_token: false }) if token.is_none() => {
            let new_token =
                spinner.suspend(|| prompt_for_gh_auth(auth_provider, prompt_provider))?;
            let Some(new_token) = new_token else {
                return Err(anyhow::anyhow!(
                    "Release not found. If this is a private repository, either:\n  \
                     - Install and authenticate the gh CLI: gh auth login\n  \
                     - Set GH_TOKEN or GITHUB_TOKEN environment variable"
                ));
            };
            *token = Some(new_token);
            Ok(github.fetch_latest_release(token.as_deref())?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Offer the gh CLI's token, if it has one.
fn prompt_for_gh_auth<A: AuthProvider, P: PromptProvider>(
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
    use crate::update::test_support::{
        MockAuthProvider, MockGitHubApi, MockPromptProvider, completed_run, in_progress_run,
        not_found_error, stale_release, test_release,
    };

    /// The options a plain `grans update --check` runs with.
    fn check_only_options() -> UpdateOptions {
        UpdateOptions {
            check_only: true,
            use_gh_auth: false,
            wait_for_build: false,
            timeout_secs: 600,
        }
    }

    fn install_options(wait_for_build: bool) -> UpdateOptions {
        UpdateOptions {
            check_only: false,
            use_gh_auth: false,
            wait_for_build,
            timeout_secs: 600,
        }
    }

    /// Poll without sleeping, so a multi-poll test runs instantly.
    fn immediate_polling() -> WaitConfig {
        WaitConfig {
            poll_interval_secs: 0,
            ..WaitConfig::default()
        }
    }

    fn fetch<A: AuthProvider, P: PromptProvider>(
        github: &MockGitHubApi,
        auth: &A,
        prompt: &P,
        token: &mut Option<String>,
        options: UpdateOptions,
    ) -> Result<ReleaseFetch> {
        fetch_release(
            github,
            auth,
            prompt,
            token,
            options,
            &immediate_polling(),
            &ProgressBar::hidden(),
        )
    }

    fn no_gh_cli() -> MockAuthProvider {
        MockAuthProvider { token: None }
    }

    fn gh_cli_with(token: &str) -> MockAuthProvider {
        MockAuthProvider {
            token: Some(token.to_string()),
        }
    }

    /// A build in progress that finishes on the wait loop's first poll.
    fn build_finishes_while_waiting() -> MockGitHubApi {
        MockGitHubApi::new(
            vec![Ok(BuildStatus::InProgress(in_progress_run()))],
            vec![Ok(test_release())],
        )
        .with_runs(vec![Ok(completed_run())])
    }

    #[test]
    fn test_public_repo_no_auth_needed() {
        let github = MockGitHubApi::new(vec![Ok(BuildStatus::Idle)], vec![Ok(test_release())]);
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = None;
        let result = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            check_only_options(),
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
        let prompt = MockPromptProvider::new(vec![true]); // User says yes

        let mut token = None;
        let result = fetch(
            &github,
            &gh_cli_with("test_token"),
            &prompt,
            &mut token,
            check_only_options(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.build_status_calls.get(), 2); // Retried with auth
        assert_eq!(github.release_calls.get(), 1);
    }

    #[test]
    fn test_private_repo_auth_required_user_declines() {
        let github = MockGitHubApi::new(
            vec![Err(not_found_error())],
            vec![Err(UpdateError::NotFound { has_token: false })],
        );
        let prompt = MockPromptProvider::new(vec![false, false]); // No, twice

        let mut token = None;
        let result = fetch(
            &github,
            &gh_cli_with("test_token"),
            &prompt,
            &mut token,
            check_only_options(),
        );

        assert!(result.is_err());
        assert!(token.is_none());
    }

    #[test]
    fn test_with_preexisting_token() {
        let github = MockGitHubApi::new(vec![Ok(BuildStatus::Idle)], vec![Ok(test_release())]);
        let prompt = MockPromptProvider::new(vec![]); // Should not be called

        let mut token = Some("existing_token".to_string());
        let result = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            check_only_options(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("existing_token".to_string()));
        assert_eq!(prompt.call_count.get(), 0);
    }

    #[test]
    fn test_build_in_progress_detected_with_auth() {
        let github = MockGitHubApi::new(
            vec![
                Err(not_found_error()),
                Ok(BuildStatus::InProgress(in_progress_run())),
            ],
            vec![Ok(test_release())],
        );
        let prompt = MockPromptProvider::new(vec![true]); // Accept auth

        let mut token = None;
        // check_only, so the build is reported but not waited for
        let result = fetch(
            &github,
            &gh_cli_with("test_token"),
            &prompt,
            &mut token,
            check_only_options(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.build_status_calls.get(), 2);
    }

    #[test]
    fn test_wait_flag_reports_that_we_waited() {
        // Regression: --wait used to fall through to the install confirmation,
        // which reads EOF in a script and cancels the update it just waited for.
        let github = build_finishes_while_waiting();
        let prompt = MockPromptProvider::new(vec![]); // --wait asks nothing

        let mut token = Some("existing_token".to_string());
        let fetched = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(true),
        )
        .expect("fetch should succeed");

        assert_eq!(fetched.waited_build, Some(completed_run()));
        assert_eq!(prompt.call_count.get(), 0);
    }

    #[test]
    fn test_accepting_the_wait_prompt_reports_that_we_waited() {
        let github = build_finishes_while_waiting();
        let prompt = MockPromptProvider::new(vec![true]); // Yes, wait for it

        let mut token = Some("existing_token".to_string());
        let fetched = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(false),
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
        let prompt = MockPromptProvider::new(vec![false]); // No, don't wait

        let mut token = Some("existing_token".to_string());
        let fetched = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(false),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_none());
    }

    #[test]
    fn test_no_build_in_progress_reports_that_we_did_not_wait() {
        let github = MockGitHubApi::new(vec![Ok(BuildStatus::Idle)], vec![Ok(test_release())]);
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let fetched = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(false),
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
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let options = UpdateOptions {
            wait_for_build: true,
            ..check_only_options()
        };
        let fetched = fetch(&github, &no_gh_cli(), &prompt, &mut token, options)
            .expect("fetch should succeed");

        assert!(fetched.waited_build.is_none());
        assert_eq!(github.run_calls.get(), 0); // No polling loop
    }

    #[test]
    fn test_waited_release_is_polled_past_a_stale_one() {
        // Regression: a stale /releases/latest after the build used to be a
        // hard error, which under --wait meant waiting out the whole build and
        // then being told to run the command again.
        let github = MockGitHubApi::new(
            vec![Ok(BuildStatus::InProgress(in_progress_run()))],
            vec![Ok(stale_release()), Ok(test_release())],
        )
        .with_runs(vec![Ok(completed_run())]);
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let fetched = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(true),
        )
        .expect("fetch should succeed");

        assert_eq!(fetched.release.tag_name, test_release().tag_name);
        assert_eq!(github.release_calls.get(), 2);
    }

    #[test]
    fn test_wait_flag_fails_when_the_build_status_cannot_be_checked() {
        // Regression: a failed status check under --wait used to print a
        // warning and go on to install whatever release was current.
        let github = MockGitHubApi::new(
            vec![Err(UpdateError::GitHubApi(
                "HTTP 403: rate limited".to_string(),
            ))],
            vec![],
        );
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let err = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(true),
        )
        .expect_err("--wait must not proceed blind");

        assert!(err.to_string().contains("--wait"), "{}", err);
        assert_eq!(github.release_calls.get(), 0);
    }

    #[test]
    fn test_wait_flag_fails_when_auth_for_the_status_check_is_declined() {
        let github = MockGitHubApi::new(vec![Err(not_found_error())], vec![]);
        let prompt = MockPromptProvider::new(vec![false]); // Decline gh auth

        let mut token = None;
        let err = fetch(
            &github,
            &gh_cli_with("test_token"),
            &prompt,
            &mut token,
            install_options(true),
        )
        .expect_err("--wait must not proceed blind");

        assert!(err.to_string().contains("--wait"), "{}", err);
        assert_eq!(github.release_calls.get(), 0);
    }

    #[test]
    fn test_status_check_failure_is_not_fatal_without_the_wait_flag() {
        // Interactively the user still confirms the install, so an unknown
        // build status is worth no more than a warning.
        let github = MockGitHubApi::new(
            vec![Err(UpdateError::GitHubApi("HTTP 500".to_string()))],
            vec![Ok(test_release())],
        );
        let prompt = MockPromptProvider::new(vec![]);

        let mut token = Some("existing_token".to_string());
        let fetched = fetch(
            &github,
            &no_gh_cli(),
            &prompt,
            &mut token,
            install_options(false),
        )
        .expect("fetch should succeed");

        assert!(fetched.waited_build.is_none());
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
        let prompt = MockPromptProvider::new(vec![true]); // Accept auth for release

        let mut token = None;
        let result = fetch(
            &github,
            &gh_cli_with("test_token"),
            &prompt,
            &mut token,
            check_only_options(),
        );

        assert!(result.is_ok());
        assert_eq!(token, Some("test_token".to_string()));
        assert_eq!(github.release_calls.get(), 2); // Retried with auth
    }
}
