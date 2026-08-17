//! `grans auth` — manage grans's own Granola session.

use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{FixedOffset, TimeZone, Utc};
use log::debug;

use crate::api::credential_store::CredentialStore;
use crate::api::credentials::GranolaCredentials;
use crate::api::granola_auth::{self, Provider};
use crate::api::{ApiClient, jwt, resolve_token};
use crate::cli::args::{AuthAction, AuthProvider};
use crate::db::accounts::account_label;
use crate::pkce::PkceChallenge;

impl From<AuthProvider> for Provider {
    fn from(provider: AuthProvider) -> Self {
        match provider {
            AuthProvider::Google => Provider::Google,
            AuthProvider::Microsoft => Provider::Microsoft,
        }
    }
}

/// Run `grans auth` subcommands.
pub fn run(action: &AuthAction, tz: &FixedOffset, db_path: Option<&Path>) -> Result<()> {
    match action {
        AuthAction::Login {
            provider,
            refresh_token_stdin,
        } => login(*provider, *refresh_token_stdin),
        AuthAction::Status => status(tz, db_path),
        AuthAction::Logout => logout(),
    }
}

fn login(provider: AuthProvider, refresh_token_stdin: bool) -> Result<()> {
    let (store, existing) = CredentialStore::open()?;

    if existing.is_some() && !replacing_existing_session()? {
        return Ok(());
    }

    let credentials = if refresh_token_stdin {
        credentials_from_stdin()?
    } else {
        credentials_from_browser_login(provider.into())?
    };

    store.save(&credentials)?;

    println!("\nSigned in. grans now holds its own Granola session.");
    println!("  Credentials: {}", store.describe());
    if !store.is_keychain() {
        print_no_keychain_warning();
    }
    println!("Run `grans sync` to fetch your meetings.");
    Ok(())
}

/// Warn that the refresh token is only as protected as the file holding it.
///
/// The refresh token mints access tokens until the session is revoked, so a
/// copy of that file from a backup or disk image is a live credential.
fn print_no_keychain_warning() {
    eprintln!();
    eprintln!("Warning: no keychain was reachable, so the refresh token is stored");
    eprintln!("unencrypted. File permissions keep other local users out, but anyone");
    eprintln!("who obtains a copy of the file can use the session until it is revoked.");
    eprintln!();
}

/// Ask before replacing a session that already works.
///
/// Returns true when the caller should go ahead and sign in again.
fn replacing_existing_session() -> Result<bool> {
    println!("grans already has a Granola session.");
    print!("Sign in again? [y/N] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim().eq_ignore_ascii_case("y") {
        return Ok(true);
    }

    println!("Keeping the existing session.");
    Ok(false)
}

/// Bootstrap from a refresh token supplied on stdin.
///
/// Reading it from stdin rather than an argument keeps the token out of shell
/// history and out of the process list.
fn credentials_from_stdin() -> Result<GranolaCredentials> {
    let mut token = String::new();
    io::stdin()
        .lock()
        .read_line(&mut token)
        .context("Failed to read the refresh token from stdin")?;

    let token = token.trim();
    if token.is_empty() {
        bail!("No refresh token on stdin");
    }

    Ok(GranolaCredentials::from_refresh_token(token.to_string()))
}

/// Run the browser login and exchange the pasted callback for tokens.
fn credentials_from_browser_login(provider: Provider) -> Result<GranolaCredentials> {
    let pkce = PkceChallenge::generate(granola_auth::PKCE_ENTROPY_BYTES);
    let (auth_url, sign_in_click_id) = granola_auth::build_auth_url(&pkce.challenge, provider);

    granola_auth::check_client_version_accepted(&auth_url)?;

    println!("\nOpening your browser to sign in to Granola...");
    println!("\nIf it doesn't open, visit this URL:");
    println!("{}\n", auth_url);

    if let Err(e) = open::that(&auth_url) {
        eprintln!("Could not open browser: {}", e);
    }

    print_callback_instructions();

    print!("Paste the callback URL: ");
    io::stdout().flush()?;
    let mut pasted = String::new();
    io::stdin()
        .lock()
        .read_line(&mut pasted)
        .context("Failed to read the callback URL from stdin")?;

    let callback = granola_auth::parse_callback(&pasted)?;

    println!("Exchanging the code for tokens...");
    let tokens = granola_auth::exchange_code(&callback, &pkce.verifier, &sign_in_click_id)?;

    Ok(tokens.into_credentials(Utc::now().timestamp(), None))
}

/// Explain how to get the callback URL out of the browser.
///
/// The dialog matters: the authorization code is bound to the PKCE verifier
/// grans generated, so letting Granola.app open the link hands the code to a
/// verifier it does not have. That fails inside Granola and can leave grans
/// with a spent code and no obvious cause.
fn print_callback_instructions() {
    println!("After signing in you land on a granola.ai page, which offers to open");
    println!("the Granola app. Cancel that dialog, then copy the URL from your");
    println!("browser's address bar. It looks like:");
    println!();
    println!("  https://www.granola.ai/app-redirect?code=...");
    println!();
}

fn status(tz: &FixedOffset, db_path: Option<&Path>) -> Result<()> {
    let (store, stored) = CredentialStore::open()?;

    let Some(credentials) = stored else {
        println!("Not signed in.");
        println!("grans falls back to the token Granola's desktop app stored locally,");
        println!("which no longer works on current macOS builds. Run `grans auth login`.");
        return Ok(());
    };

    println!("Signed in.");

    let cached_sub = credentials
        .access_token
        .as_deref()
        .and_then(jwt::decode_sub);
    let account = describe_account(
        cached_sub.as_deref(),
        |sub| stored_account_email(db_path, sub),
        remote_identity,
    );
    println!("  Account:     {}", account);

    // The network fallback refreshes an expired access token and persists
    // the result, so re-read before describing expiry.
    let credentials = store.load()?.unwrap_or(credentials);

    println!("  Credentials: {}", store.describe());

    if let Some(session_id) = &credentials.session_id {
        println!("  Session:     {}", session_id);
    }

    println!("  Access token: {}", describe_expiry(&credentials, tz));

    if !store.is_keychain() {
        print_no_keychain_warning();
    }

    Ok(())
}

/// Identity learned from the network fallback: the `sub` decoded from the
/// resolved token and the email get-user-info reported.
struct RemoteIdentity {
    sub: Option<String>,
    email: Option<String>,
}

/// Describe which Granola account the session belongs to.
///
/// Offline first: the cached access token's JWT `sub` joined against the
/// local accounts log. Only when the email is not known locally does this
/// fall back to `fetch_remote` (one get-user-info call). Every failure
/// degrades to showing what is known rather than erroring: status must
/// still report on a machine that is offline or has never synced.
fn describe_account(
    cached_sub: Option<&str>,
    local_email: impl Fn(&str) -> Option<String>,
    fetch_remote: impl FnOnce() -> Result<RemoteIdentity>,
) -> String {
    if let Some(sub) = cached_sub {
        if let Some(email) = local_email(sub) {
            return account_label(&email, sub);
        }
    }

    match fetch_remote() {
        Ok(remote) => {
            let sub = remote.sub.as_deref().or(cached_sub);
            match (remote.email, sub) {
                (Some(email), Some(sub)) => account_label(&email, sub),
                (Some(email), None) => email,
                (None, Some(sub)) => {
                    format!("{} (email unknown: get-user-info returned none)", sub)
                }
                (None, None) => "unknown (the token carries no identity and get-user-info \
                                 returned no email)"
                    .to_string(),
            }
        }
        Err(e) => match cached_sub {
            Some(sub) => format!("{} (email unknown: {:#})", sub, e),
            None => format!("unknown ({:#})", e),
        },
    }
}

/// Email the local accounts log records for this account, or None when the
/// database, its accounts table, or the account itself is absent. A miss is
/// never an error, it just falls through to the network path: status must
/// still report on a machine that has never synced.
fn stored_account_email(db_path: Option<&Path>, sub: &str) -> Option<String> {
    let path = match db_path {
        Some(path) => path.to_path_buf(),
        None => match crate::db::connection::default_db_path() {
            Ok(path) => path,
            Err(e) => {
                debug!("no database path to read the accounts log from: {}", e);
                return None;
            }
        },
    };

    let conn = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(e) => {
            debug!("accounts log unavailable at {}: {}", path.display(), e);
            return None;
        }
    };

    match crate::db::accounts::email_for(&conn, sub) {
        Ok(email) => email,
        Err(e) => {
            debug!("accounts log unreadable: {}", e);
            None
        }
    }
}

/// Resolve the stored session to a live access token and ask Granola whose
/// it is. Refreshes (and persists) the token when the cached one expired.
fn remote_identity() -> Result<RemoteIdentity> {
    let token = resolve_token(None)?;
    let sub = jwt::decode_sub(&token);
    let info = ApiClient::new(token)?.get_user_info()?;
    Ok(RemoteIdentity {
        sub,
        email: info.email,
    })
}

/// Describe the access token's freshness without revealing any of it.
fn describe_expiry(credentials: &GranolaCredentials, tz: &FixedOffset) -> String {
    let now = Utc::now().timestamp();

    match credentials.expires_at {
        None if credentials.access_token.is_none() => {
            "none stored, will be fetched on next use".to_string()
        }
        None => "stored, expiry unknown; will be refreshed on next use".to_string(),
        Some(expires_at) => {
            let when = tz
                .timestamp_opt(expires_at, 0)
                .single()
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| expires_at.to_string());

            if credentials.valid_access_token(now).is_some() {
                format!("valid until {}", when)
            } else {
                format!("expired at {}, will be refreshed on next use", when)
            }
        }
    }
}

fn logout() -> Result<()> {
    let (store, stored) = CredentialStore::open()?;

    if stored.is_none() {
        println!("Not signed in; nothing to remove.");
        return Ok(());
    }

    store.delete()?;

    println!("Removed grans's stored Granola credentials.");
    println!("The session itself remains active in Granola until you revoke it there.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn with_expiry(expires_at: Option<i64>, access_token: Option<&str>) -> GranolaCredentials {
        GranolaCredentials {
            refresh_token: "rt".to_string(),
            access_token: access_token.map(str::to_string),
            expires_at,
            session_id: None,
        }
    }

    // --- describe_account ---

    fn no_local(_sub: &str) -> Option<String> {
        None
    }

    #[test]
    fn account_known_locally_never_touches_the_network() {
        let described = describe_account(
            Some("user_01AAA"),
            |sub| (sub == "user_01AAA").then(|| "a@example.com".to_string()),
            || panic!("must not fetch remotely when the email is known locally"),
        );

        assert_eq!(described, "a@example.com (user_01AAA)");
    }

    #[test]
    fn local_miss_falls_back_to_the_network() {
        let described = describe_account(Some("user_01AAA"), no_local, || {
            Ok(RemoteIdentity {
                sub: Some("user_01AAA".to_string()),
                email: Some("a@example.com".to_string()),
            })
        });

        assert_eq!(described, "a@example.com (user_01AAA)");
    }

    #[test]
    fn network_failure_still_shows_the_cached_id() {
        let described = describe_account(Some("user_01AAA"), no_local, || {
            Err(anyhow::anyhow!("network unreachable"))
        });

        assert!(described.contains("user_01AAA"));
        assert!(described.contains("network unreachable"));
    }

    #[test]
    fn no_cached_token_resolves_identity_remotely() {
        let described = describe_account(None, no_local, || {
            Ok(RemoteIdentity {
                sub: Some("user_01BBB".to_string()),
                email: Some("b@example.com".to_string()),
            })
        });

        assert_eq!(described, "b@example.com (user_01BBB)");
    }

    #[test]
    fn nothing_resolvable_reports_unknown_with_the_reason() {
        let described =
            describe_account(None, no_local, || Err(anyhow::anyhow!("refresh rejected")));

        assert!(described.starts_with("unknown"));
        assert!(described.contains("refresh rejected"));
    }

    #[test]
    fn emailless_remote_identity_shows_the_id_it_has() {
        let described = describe_account(Some("user_01AAA"), no_local, || {
            Ok(RemoteIdentity {
                sub: Some("user_01AAA".to_string()),
                email: None,
            })
        });

        assert!(described.contains("user_01AAA"));
        assert!(described.contains("email unknown"));
    }

    #[test]
    fn test_describe_expiry_for_valid_token() {
        let future = Utc::now().timestamp() + 3600;
        let described = describe_expiry(&with_expiry(Some(future), Some("at")), &tz());

        assert!(described.starts_with("valid until "));
    }

    #[test]
    fn test_describe_expiry_for_expired_token() {
        let past = Utc::now().timestamp() - 3600;
        let described = describe_expiry(&with_expiry(Some(past), Some("at")), &tz());

        assert!(described.contains("expired at"));
        assert!(described.contains("refreshed on next use"));
    }

    #[test]
    fn test_describe_expiry_without_any_access_token() {
        let described = describe_expiry(&with_expiry(None, None), &tz());

        assert!(described.contains("none stored"));
    }

    #[test]
    fn test_describe_expiry_without_known_expiry() {
        let described = describe_expiry(&with_expiry(None, Some("at")), &tz());

        assert!(described.contains("expiry unknown"));
    }

    #[test]
    fn test_describe_expiry_never_includes_token_material() {
        let future = Utc::now().timestamp() + 3600;
        let secret = "super-secret-access-token";

        let described = describe_expiry(&with_expiry(Some(future), Some(secret)), &tz());

        assert!(!described.contains(secret));
    }
}
