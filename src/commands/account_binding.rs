//! Account binding enforcement for sync.
//!
//! Every sync entry point calls [`ensure_account_binding`] right after
//! resolving its token, before any writes. The check compares the token's
//! JWT `sub` against the database's active binding: first sync binds the
//! database to the account, a match proceeds, and a mismatch refuses to sync
//! so one account's data is never silently upserted over another's.

use anyhow::{Result, anyhow};
use log::debug;
use rusqlite::Connection;

use crate::api::{ApiClient, jwt};
use crate::db::accounts::{self, ActiveBinding};

/// What the binding check concluded, before any API call is made.
#[derive(Debug, PartialEq, Eq)]
enum BindingDecision {
    /// Token carries no decodable JWT identity; skip the check entirely.
    Skip,
    /// Token matches the active binding; sync may stamp this account id.
    Proceed(String),
    /// Database has never been bound and this run writes; bind it first.
    NeedsFirstBind(String),
    /// Database has never been bound but this is a dry run; write nothing.
    ProceedUnbound,
    /// Token belongs to a different account than the database is bound to.
    Mismatch { binding: ActiveBinding, sub: String },
}

/// Pure decision: token identity vs. active binding.
fn decide(binding: Option<ActiveBinding>, sub: Option<String>, dry_run: bool) -> BindingDecision {
    let Some(sub) = sub else {
        return BindingDecision::Skip;
    };
    match binding {
        None if dry_run => BindingDecision::ProceedUnbound,
        None => BindingDecision::NeedsFirstBind(sub),
        Some(binding) if binding.account_id == sub => BindingDecision::Proceed(sub),
        Some(binding) => BindingDecision::Mismatch { binding, sub },
    }
}

/// Check the resolved token's identity against the database's account
/// binding, establishing the binding on the first non-dry-run sync.
///
/// Returns the account id that document upserts should stamp, or None when
/// the token carries no decodable identity (arbitrary `--token` values; the
/// API rejects fake tokens on its own) or when a dry run proceeds against a
/// never-bound database. A mismatch is a hard error wherever a token is
/// resolved; for `sync` and the documents/people/calendars/templates/recipes
/// subcommands that includes `--dry-run`, because they resolve the token
/// before branching on it. Bulk transcript and panel dry runs are local-db
/// previews that never resolve a token, so their check first applies on the
/// real run.
pub(super) fn ensure_account_binding(
    conn: &Connection,
    token: &str,
    dry_run: bool,
) -> Result<Option<String>> {
    ensure_account_binding_with(conn, token, dry_run, fetch_email)
}

/// [`ensure_account_binding`] with the mismatch-path email lookup injected,
/// so the enforcement logic is testable without live HTTP.
fn ensure_account_binding_with(
    conn: &Connection,
    token: &str,
    dry_run: bool,
    lookup_email: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>> {
    let sub = jwt::decode_sub(token);
    if sub.is_none() {
        debug!("token has no decodable JWT identity; skipping account binding check");
    }

    match decide(accounts::get_active_binding(conn)?, sub, dry_run) {
        BindingDecision::Skip | BindingDecision::ProceedUnbound => Ok(None),
        BindingDecision::Proceed(sub) => Ok(Some(sub)),
        BindingDecision::NeedsFirstBind(sub) => first_bind(conn, token, &sub).map(Some),
        BindingDecision::Mismatch { binding, sub } => {
            let current_email = lookup_email(token);
            Err(anyhow!(mismatch_message(
                &binding,
                &sub,
                current_email.as_deref()
            )))
        }
    }
}

/// Bind a never-bound database to the token's account, backfilling
/// provenance on all pre-existing documents.
///
/// get-user-info failing fails the sync: binding is required before writes,
/// and a binding without email would leave the mismatch error (and every
/// later "which account is this?" question) with only an opaque WorkOS id.
fn first_bind(conn: &Connection, token: &str, sub: &str) -> Result<String> {
    let client = ApiClient::new(token.to_string())?;
    let info = client.get_user_info().map_err(|e| {
        anyhow!(
            "Cannot bind this database to the current Granola account: get-user-info failed: {}",
            e
        )
    })?;

    let backfilled =
        accounts::bind_account_with_backfill(conn, sub, info.id.as_deref(), info.email.as_deref())?;

    eprintln!(
        "[grans] Database is now bound to Granola account {}",
        accounts::account_label(info.email.as_deref(), sub)
    );
    if backfilled > 0 {
        eprintln!(
            "[grans] Recorded {} pre-existing document(s) as synced from this account",
            backfilled
        );
    }
    Ok(sub.to_string())
}

/// Best-effort email lookup for the mismatch error message. The sync is
/// failing either way; a lookup failure only degrades the message to the
/// raw WorkOS id.
fn fetch_email(token: &str) -> Option<String> {
    let client = match ApiClient::new(token.to_string()) {
        Ok(client) => client,
        Err(e) => {
            debug!("mismatch email lookup: could not build API client: {}", e);
            return None;
        }
    };
    match client.get_user_info() {
        Ok(info) => info.email,
        Err(e) => {
            debug!("mismatch email lookup: get-user-info failed: {}", e);
            None
        }
    }
}

/// Format the hard error for a token that belongs to a different account
/// than the database is bound to.
fn mismatch_message(binding: &ActiveBinding, sub: &str, current_email: Option<&str>) -> String {
    let bound = accounts::account_label(binding.email.as_deref(), &binding.account_id);
    let current = accounts::account_label(current_email, sub);
    format!(
        "This database is bound to Granola account {}, but the current token belongs to {}. \
         Refusing to sync across accounts. If this is intentional, run \
         `grans admin db rebind` to bind the database to the current account.",
        bound, current
    )
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

    fn bound_db(account_id: &str, email: &str) -> rusqlite::Connection {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "First"}
            }
        }));
        conn.execute(
            "INSERT INTO accounts (account_id, granola_user_id, email, bound_at)
             VALUES (?1, 'uuid-1', ?2, '2026-08-01T00:00:00Z')",
            rusqlite::params![account_id, email],
        )
        .unwrap();
        conn
    }

    #[test]
    fn mismatched_token_is_a_hard_error_naming_both_accounts() {
        let conn = bound_db("user_01AAA", "old@example.com");
        let token = make_jwt("user_01BBB");

        let err = ensure_account_binding_with(&conn, &token, false, |_| {
            Some("new@example.com".to_string())
        })
        .unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("old@example.com (user_01AAA)"));
        assert!(msg.contains("new@example.com (user_01BBB)"));
        assert!(msg.contains("grans admin db rebind"));
    }

    #[test]
    fn mismatched_token_errors_under_dry_run_too() {
        let conn = bound_db("user_01AAA", "old@example.com");
        let token = make_jwt("user_01BBB");

        let err = ensure_account_binding_with(&conn, &token, true, |_| None).unwrap_err();
        assert!(err.to_string().contains("user_01BBB"));
    }

    #[test]
    fn matching_token_proceeds_without_an_email_lookup() {
        let conn = bound_db("user_01AAA", "old@example.com");
        let token = make_jwt("user_01AAA");

        let stamped = ensure_account_binding_with(&conn, &token, false, |_| {
            panic!("email lookup must not run on a match")
        })
        .unwrap();
        assert_eq!(stamped.as_deref(), Some("user_01AAA"));
    }

    fn binding(account_id: &str, email: Option<&str>) -> ActiveBinding {
        ActiveBinding {
            account_id: account_id.to_string(),
            granola_user_id: Some("uuid-1".to_string()),
            email: email.map(str::to_string),
            bound_at: "2026-08-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn decide_skips_when_token_has_no_identity() {
        assert_eq!(decide(None, None, false), BindingDecision::Skip);
        assert_eq!(
            decide(Some(binding("user_01AAA", None)), None, false),
            BindingDecision::Skip
        );
    }

    #[test]
    fn decide_binds_on_first_writing_sync() {
        assert_eq!(
            decide(None, Some("user_01AAA".to_string()), false),
            BindingDecision::NeedsFirstBind("user_01AAA".to_string())
        );
    }

    #[test]
    fn decide_leaves_dry_run_unbound() {
        assert_eq!(
            decide(None, Some("user_01AAA".to_string()), true),
            BindingDecision::ProceedUnbound
        );
    }

    #[test]
    fn decide_proceeds_on_match() {
        assert_eq!(
            decide(
                Some(binding("user_01AAA", Some("a@example.com"))),
                Some("user_01AAA".to_string()),
                false
            ),
            BindingDecision::Proceed("user_01AAA".to_string())
        );
    }

    #[test]
    fn decide_flags_mismatch_even_under_dry_run() {
        let b = binding("user_01AAA", Some("a@example.com"));
        assert_eq!(
            decide(Some(b.clone()), Some("user_01BBB".to_string()), true),
            BindingDecision::Mismatch {
                binding: b,
                sub: "user_01BBB".to_string()
            }
        );
    }

    #[test]
    fn mismatch_message_names_both_accounts_and_the_fix() {
        let msg = mismatch_message(
            &binding("user_01AAA", Some("old@example.com")),
            "user_01BBB",
            Some("new@example.com"),
        );
        assert!(msg.contains("old@example.com (user_01AAA)"));
        assert!(msg.contains("new@example.com (user_01BBB)"));
        assert!(msg.contains("grans admin db rebind"));
    }

    #[test]
    fn mismatch_message_falls_back_to_raw_ids() {
        let msg = mismatch_message(&binding("user_01AAA", None), "user_01BBB", None);
        assert!(msg.contains("user_01AAA"));
        assert!(msg.contains("user_01BBB"));
    }
}
