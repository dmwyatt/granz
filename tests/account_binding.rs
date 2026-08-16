//! Every sync entry point must refuse to run when the token belongs to a
//! different account than the database is bound to. This spawns grans per
//! subcommand, so it also proves the enforcement call is actually wired into
//! each entry point: deleting the ensure_account_binding line from any of
//! them turns this red.

mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::TestEnv;

/// Build an unsigned JWT whose payload carries the given `sub`. The binding
/// check decodes the payload locally and never verifies the signature, so
/// this is enough to present as a different account.
fn jwt_for(sub: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"{}"}}"#, sub));
    format!("{}.{}.fake-signature", header, payload)
}

#[test]
fn every_sync_entry_point_refuses_a_mismatched_account() {
    // A document with no transcript and no panels, so the transcript and
    // panel syncs have work to do and reach their token resolution instead
    // of early-returning on an empty work list.
    let env = TestEnv::with_state(
        r#"{"documents": {"doc-1": {"id": "doc-1", "title": "Seed Meeting"}}}"#,
    );

    // Bind the database to one account...
    let conn = rusqlite::Connection::open(&env.db_path).unwrap();
    conn.execute(
        "INSERT INTO accounts (account_id, granola_user_id, email, first_seen_at)
         VALUES ('user_01BOUND', 'uuid-1', 'bound@example.com', '2026-08-01T00:00:00Z')",
        [],
    )
    .unwrap();
    drop(conn);

    // ...and sync with a token that belongs to another.
    let token = jwt_for("user_01OTHER");

    let entry_points: &[&[&str]] = &[
        &["sync"],
        &["sync", "documents"],
        &["sync", "people"],
        &["sync", "calendars"],
        &["sync", "templates"],
        &["sync", "recipes"],
        &["sync", "transcripts"],
        &["sync", "panels"],
    ];

    for args in entry_points {
        let output = env
            .cmd()
            .args(*args)
            .arg("--token")
            .arg(&token)
            .output()
            .unwrap();

        assert!(
            !output.status.success(),
            "{:?} should refuse a mismatched account",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("bound@example.com (user_01BOUND)"),
            "{:?} error must name the bound account, got: {}",
            args,
            stderr
        );
        assert!(
            stderr.contains("user_01OTHER"),
            "{:?} error must name the current token's account, got: {}",
            args,
            stderr
        );
        assert!(
            stderr.contains("grans admin db rebind"),
            "{:?} error must point at the rebind remedy, got: {}",
            args,
            stderr
        );
    }
}
