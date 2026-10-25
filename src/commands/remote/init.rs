//! `grans remote init`: choose the remote and authorize grans there.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::args::InitProvider;
use crate::pkce::PkceChallenge;
use crate::sync::config::{Authorization, SyncConfig};
use crate::sync::dropbox;
use crate::sync::google_drive::loopback::{RedirectListener, wait_for_response};
use crate::sync::google_drive::oauth::{self as google, GoogleClient};
use crate::sync::remote::Provider;

/// A remote to authorize, with what it needs read before anything is asked.
enum Target {
    Dropbox,
    GoogleDrive(GoogleClient),
}

pub(super) fn init(requested: &InitProvider) -> Result<()> {
    // Read the client file first, so a wrong one fails before any prompt.
    let target = match requested {
        InitProvider::Dropbox => Target::Dropbox,
        InitProvider::GoogleDrive { client_file } => {
            Target::GoogleDrive(read_client_file(client_file)?)
        }
    };
    let provider = match target {
        Target::Dropbox => Provider::Dropbox,
        Target::GoogleDrive(_) => Provider::GoogleDrive,
    };
    let config = SyncConfig::load()?;

    if config.is_authenticated() && !confirm_replacing(&config, provider)? {
        println!("Keeping the existing connection.");
        return Ok(());
    }

    let authorization = match target {
        Target::Dropbox => Authorization::Dropbox {
            refresh_token: authorize_dropbox()?,
        },
        Target::GoogleDrive(client) => Authorization::GoogleDrive {
            refresh_token: authorize_google_drive(&client)?,
            client,
        },
    };

    config.reconnected(authorization).save()?;

    println!("\nConnected to {}.", provider);
    match provider {
        Provider::Dropbox => println!("The shared copy lives in Apps/grans/ in your Dropbox."),
        Provider::GoogleDrive => {
            println!("The shared copy lives in a folder named grans in your Google Drive.")
        }
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
    let pkce = PkceChallenge::generate(dropbox::oauth::PKCE_ENTROPY_BYTES);
    let auth_url = dropbox::oauth::build_auth_url(&pkce.challenge);

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
    let tokens = dropbox::oauth::exchange_code(code, &pkce.verifier)?;

    tokens
        .refresh_token
        .context("Dropbox granted access but returned no refresh token")
}

/// Read the OAuth client JSON the user downloaded from Google.
fn read_client_file(path: &Path) -> Result<GoogleClient> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("reading the client file {}", path.display()))?;
    google::parse_client_file(&json)
        .with_context(|| format!("reading the client file {}", path.display()))
}

/// Run Google's authorization in a browser and return the refresh token.
fn authorize_google_drive(client: &GoogleClient) -> Result<String> {
    let listener = RedirectListener::bind().context("listening for Google's response")?;
    let redirect_uri = listener.redirect_uri();
    let pkce = PkceChallenge::generate(google::PKCE_ENTROPY_BYTES);
    let state = google::random_state();
    let auth_url = google::build_auth_url(client, &redirect_uri, &pkce.challenge, &state);

    println!("\nOpening browser for Google authorization...");
    println!("\nIf the browser doesn't open, visit this URL:");
    println!("{}\n", auth_url);

    if let Err(e) = open::that(&auth_url) {
        eprintln!("Could not open browser: {}", e);
    }

    println!("Waiting for Google to send the browser back to grans.");
    println!(
        "If the browser is on another machine, paste the address it ends up on here, \
         even though that page will not load:"
    );

    let response = wait_for_response(listener).context("waiting for Google's response")?;
    let code = google::parse_callback(&response, &state)?;

    println!("Exchanging code for tokens...");
    Ok(google::exchange_code(
        client,
        &code,
        &pkce.verifier,
        &redirect_uri,
    )?)
}
