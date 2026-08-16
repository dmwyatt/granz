//! Recording which account a sync ran as.
//!
//! Every sync entry point calls [`record_source_account`] right after
//! resolving its token. The token's JWT `sub` identifies the account; an
//! unseen account is appended to the accounts log (capturing its email via
//! get-user-info), and the sub is returned so upserts can stamp rows with
//! the account they arrived under. Nothing is enforced: multiple accounts'
//! data coexisting in one database is supported by design.

use anyhow::{Result, anyhow};
use log::debug;
use rusqlite::Connection;

use crate::api::types::UserInfoResponse;
use crate::api::{ApiClient, jwt};
use crate::db::accounts;

/// Identify the account the resolved token belongs to, recording it in the
/// accounts log the first time it is seen.
///
/// Returns the account id that upserts should stamp, or None when the token
/// carries no decodable JWT identity (arbitrary `--token` values; the API
/// rejects fake tokens on its own), in which case rows from this sync get no
/// source account. A dry run identifies but never writes the log.
pub(super) fn record_source_account(
    conn: &Connection,
    token: &str,
    dry_run: bool,
) -> Result<Option<String>> {
    record_source_account_with(conn, token, dry_run, fetch_user_info)
}

/// [`record_source_account`] with the get-user-info fetch injected, so the
/// recording logic is testable without live HTTP.
fn record_source_account_with(
    conn: &Connection,
    token: &str,
    dry_run: bool,
    fetch_info: impl Fn(&str) -> Result<UserInfoResponse>,
) -> Result<Option<String>> {
    let Some(sub) = jwt::decode_sub(token) else {
        debug!("token has no decodable JWT identity; rows from this sync get no source account");
        return Ok(None);
    };

    if accounts::account_seen(conn, &sub)? || dry_run {
        return Ok(Some(sub));
    }

    let info = fetch_info(token)?;
    let email = require_email(&info)?;
    let backfilled = accounts::record_account(conn, &sub, info.id.as_deref(), &email)?;

    eprintln!(
        "[grans] Recording new account {}",
        accounts::account_label(&email, &sub)
    );
    if backfilled > 0 {
        eprintln!(
            "[grans] First account seen: stamped {} pre-existing row(s) as synced from it",
            backfilled
        );
    }
    Ok(Some(sub))
}

/// The identity lookup for a not-yet-recorded account. Failure fails the
/// sync: the log entry is what makes provenance legible later, so a sync
/// must not write rows for an account it could not record.
fn fetch_user_info(token: &str) -> Result<UserInfoResponse> {
    let client = ApiClient::new(token.to_string())?;
    client.get_user_info().map_err(|e| {
        anyhow!(
            "Cannot record the current Granola account: get-user-info failed: {}",
            e
        )
    })
}

/// The email get-user-info returned, or an error refusing to record without
/// one. An email-less log entry would leave every later "which account is
/// this?" question with only an opaque WorkOS id.
fn require_email(info: &UserInfoResponse) -> Result<String> {
    info.email.clone().ok_or_else(|| {
        anyhow!(
            "get-user-info returned no email for this account; refusing to record an \
             email-less account, because the email is what makes the log legible later"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    /// Build an unsigned JWT whose payload carries the given `sub`.
    fn make_jwt(sub: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(json!({ "sub": sub }).to_string());
        format!("{}.{}.fake-signature", header, payload)
    }

    fn test_db() -> rusqlite::Connection {
        build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "First"}
            }
        }))
    }

    fn info(email: Option<&str>) -> UserInfoResponse {
        UserInfoResponse {
            id: Some("uuid-1".to_string()),
            email: email.map(str::to_string),
        }
    }

    fn account_count(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn undecodable_token_records_nothing_and_returns_none() {
        let conn = test_db();
        let stamped = record_source_account_with(&conn, "not-a-jwt", false, |_| {
            panic!("must not fetch user info for an undecodable token")
        })
        .unwrap();
        assert_eq!(stamped, None);
        assert_eq!(account_count(&conn), 0);
    }

    #[test]
    fn unseen_account_is_recorded_once_with_its_email() {
        let conn = test_db();
        let token = make_jwt("user_01AAA");

        let stamped =
            record_source_account_with(&conn, &token, false, |_| Ok(info(Some("a@example.com"))))
                .unwrap();
        assert_eq!(stamped.as_deref(), Some("user_01AAA"));
        assert_eq!(account_count(&conn), 1);

        let records = accounts::list_accounts(&conn).unwrap();
        assert_eq!(records[0].email, "a@example.com");
    }

    #[test]
    fn seen_account_is_not_duplicated_and_needs_no_lookup() {
        let conn = test_db();
        let token = make_jwt("user_01AAA");
        record_source_account_with(&conn, &token, false, |_| Ok(info(Some("a@example.com"))))
            .unwrap();

        let stamped = record_source_account_with(&conn, &token, false, |_| {
            panic!("must not fetch user info for an already-recorded account")
        })
        .unwrap();
        assert_eq!(stamped.as_deref(), Some("user_01AAA"));
        assert_eq!(account_count(&conn), 1);
    }

    #[test]
    fn dry_run_identifies_but_never_writes_the_log() {
        let conn = test_db();
        let token = make_jwt("user_01AAA");

        let stamped = record_source_account_with(&conn, &token, true, |_| {
            panic!("a dry run must not fetch user info")
        })
        .unwrap();
        assert_eq!(stamped.as_deref(), Some("user_01AAA"));
        assert_eq!(account_count(&conn), 0);
    }

    #[test]
    fn failed_identity_lookup_fails_the_sync() {
        let conn = test_db();
        let token = make_jwt("user_01AAA");

        let err = record_source_account_with(&conn, &token, false, |_| {
            Err(anyhow!("get-user-info failed: 401"))
        })
        .unwrap_err();
        assert!(err.to_string().contains("get-user-info failed"));
        assert_eq!(account_count(&conn), 0);
    }

    #[test]
    fn emailless_identity_is_refused() {
        let conn = test_db();
        let token = make_jwt("user_01AAA");

        let err = record_source_account_with(&conn, &token, false, |_| Ok(info(None))).unwrap_err();
        assert!(err.to_string().contains("no email"));
        assert_eq!(account_count(&conn), 0);
    }
}
