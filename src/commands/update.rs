//! Update command implementation.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use crate::update::download::{
    current_binary_hash, download_asset, replace_binary, verify_checksum,
};
pub use crate::update::fetch::UpdateOptions;
use crate::update::fetch::{
    PromptProvider, RealAuthProvider, RealPromptProvider, ReleaseFetch, fetch_release,
};
use crate::update::github::{
    Asset, RealGitHubApi, Release, WorkflowRun, find_asset, format_timestamp,
};
use crate::update::platform::asset_name;
use crate::update::wait::WaitConfig;
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

/// Run the update command.
pub fn run(options: UpdateOptions) -> Result<()> {
    let current_version = env!("GRANS_VERSION");
    println!("Current version: {}", current_version);
    println!();

    let mut token = get_initial_token(options.use_gh_auth);

    let spinner = create_spinner("Checking build status...");

    let ReleaseFetch {
        release,
        token,
        waited_build,
    } = fetch_release(
        &RealGitHubApi,
        &RealAuthProvider,
        &RealPromptProvider,
        &mut token,
        options,
        &WaitConfig::default().with_timeout(options.timeout_secs),
        &spinner,
    )?;

    spinner.finish_and_clear();

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

    if options.check_only {
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

fn display_release_info(release: &Release, asset: &Asset) {
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
        println!("Published: {}", format_timestamp(published).dimmed());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::test_support::{MockPromptProvider, completed_run};

    fn install_options(wait_for_build: bool) -> UpdateOptions {
        UpdateOptions {
            check_only: false,
            use_gh_auth: false,
            wait_for_build,
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

        display_release_info(&release, &asset);
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
}
