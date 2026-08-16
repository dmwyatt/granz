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
/// never-bound database. A mismatch is a hard error, dry run or not.
pub(super) fn ensure_account_binding(
    conn: &Connection,
    token: &str,
    dry_run: bool,
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
            let current_email = fetch_email(token);
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

    let backfilled = accounts::bind_account(conn, sub, info.id.as_deref(), info.email.as_deref())?;

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
    ApiClient::new(token.to_string())
        .ok()
        .and_then(|client| client.get_user_info().ok())
        .and_then(|info| info.email)
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
