//! `grans remote init`: choose the remote and authorize grans there.

use std::io::{self, Write};

use anyhow::{Context, Result};

use crate::cli::args::InitProvider;
use crate::pkce::PkceChallenge;
use crate::sync::config::SyncConfig;
use crate::sync::dropbox::oauth::{PKCE_ENTROPY_BYTES, build_auth_url, exchange_code};
use crate::sync::remote::Provider;

pub(super) fn init(target: &InitProvider) -> Result<()> {
    let provider = match target {
        InitProvider::Dropbox => Provider::Dropbox,
    };
    let config = SyncConfig::load()?;

    if config.is_authenticated() && !confirm_replacing(&config, provider)? {
        println!("Keeping the existing connection.");
        return Ok(());
    }

    let refresh_token = match target {
        InitProvider::Dropbox => authorize_dropbox()?,
    };

    config.reconnected(provider, refresh_token).save()?;

    println!("\nConnected to {}.", provider);
    match provider {
        Provider::Dropbox => println!("The shared copy lives in Apps/grans/ in your Dropbox."),
    }

    Ok(())
}

/// Ask before replacing an existing connection.
fn confirm_replacing(config: &SyncConfig, provider: Provider) -> Result<bool> {
    let current = config.provider();

    if current == provider {
        println!("Already connected to {}.", current);
        print!("Authorize again? [y/N] ");
    } else {
        println!(
            "Already connected to {}. Connecting to {} replaces that connection, and grans \
             forgets what it knew about the copy on {}.",
            current, provider, current
        );
        print!("Continue? [y/N] ");
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y"))
}

/// Run Dropbox's authorization in a browser and return the refresh token.
fn authorize_dropbox() -> Result<String> {
    let pkce = PkceChallenge::generate(PKCE_ENTROPY_BYTES);
    let auth_url = build_auth_url(&pkce.challenge);

    println!("\nOpening browser for Dropbox authorization...");
    println!("\nIf the browser doesn't open, visit this URL:");
    println!("{}\n", auth_url);

    if let Err(e) = open::that(&auth_url) {
        eprintln!("Could not open browser: {}", e);
    }

    print!("Enter the authorization code from Dropbox: ");
    io::stdout().flush()?;

    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    let code = code.trim();

    if code.is_empty() {
        anyhow::bail!("No authorization code provided");
    }

    println!("Exchanging code for tokens...");
    let tokens = exchange_code(code, &pkce.verifier)?;

    tokens
        .refresh_token
        .context("Dropbox granted access but returned no refresh token")
}
