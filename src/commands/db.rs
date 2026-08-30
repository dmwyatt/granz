use anyhow::{Context, Result, bail};
use std::path::Path;

use crate::cli::args::DbAction;
use crate::db::accounts::{self, account_label};
use crate::db::integrity;
use crate::db::wal::remove_database;

pub fn run_with_path(action: &DbAction, db_path: &Path) -> Result<()> {
    match action {
        DbAction::Clear { all } => {
            if *all {
                clear_all_databases()?;
            } else {
                clear_database(db_path)?;
            }
        }
        DbAction::Info => {
            show_database_info(db_path)?;
        }
        DbAction::List => {
            list_all_databases()?;
        }
        DbAction::RebuildFts => {
            rebuild_search_indexes(db_path)?;
        }
    }
    Ok(())
}

fn clear_database(db_path: &Path) -> Result<()> {
    if db_path.exists() {
        remove_database(db_path)?;
        println!("Cleared database: {}", db_path.display());
    } else {
        println!("No database found at {}", db_path.display());
    }

    Ok(())
}

fn clear_all_databases() -> Result<()> {
    let data_dir = crate::platform::data_dir()?;

    if !data_dir.exists() {
        println!("No databases found (data directory doesn't exist)");
        return Ok(());
    }

    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&data_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("db") {
            match remove_database(&path) {
                Ok(()) => {
                    println!("Cleared: {}", path.display());
                    count += 1;
                }
                // One database another process still holds should not decide
                // whether the rest are dealt with.
                Err(e) => failures.push(format!("{}: {:#}", path.display(), e)),
            }
        }
    }

    if count == 0 && failures.is_empty() {
        println!("No database files found");
    } else if count > 0 {
        println!("\nCleared {} database file(s)", count);
    }

    if failures.is_empty() {
        return Ok(());
    }

    bail!("could not clear:\n  {}", failures.join("\n  "))
}

fn show_database_info(db_path: &Path) -> Result<()> {
    println!("Database path: {}", db_path.display());

    if !db_path.exists() {
        println!("Status: does not exist (run 'grans sync' to create)");
        return Ok(());
    }

    print_database_size(db_path)?;
    println!("Status: exists");

    // Read-write, because the index health check is spelled as an INSERT and
    // SQLite refuses that on a read-only connection. Nothing here writes: the
    // journal mode is converted by the migration path, not by opening.
    //
    // A database that will not open is reported as the failure it is. Skipping
    // silently to the next line left the command exiting 0 with nothing but a
    // path and a size, which reads as a healthy empty database.
    let conn = crate::db::connection::open_existing(db_path)
        .with_context(|| format!("reading the database at {}", db_path.display()))?;

    // The first read of the file, and the one that has to be believed: SQLite
    // opens lazily, so a file that is not a database opens without complaint
    // and only fails when something reads a page. Everything below it is
    // best-effort, because an old schema can legitimately be missing a table.
    let schema_version = crate::db::migrations::get_schema_version(&conn)
        .with_context(|| format!("reading the database at {}", db_path.display()))?;
    println!("Schema version: {}", schema_version);

    report_database_contents(&conn);

    Ok(())
}

/// Report what the database occupies on disk.
///
/// The write-ahead log is part of the database, so a figure counting only the
/// main file understates it, and anything that copies the database has to carry
/// both.
fn print_database_size(db_path: &Path) -> Result<()> {
    let size_bytes = std::fs::metadata(db_path)?.len();
    println!(
        "Database size: {:.2} MB ({} bytes)",
        size_bytes as f64 / 1_048_576.0,
        size_bytes
    );

    let log = crate::db::wal::log_path(db_path);
    if let Ok(log_bytes) = std::fs::metadata(&log).map(|m| m.len())
        && log_bytes > 0
    {
        println!(
            "Write-ahead log: {:.2} MB ({} bytes)",
            log_bytes as f64 / 1_048_576.0,
            log_bytes
        );
    }

    Ok(())
}

/// Report what an open database holds.
fn report_database_contents(conn: &rusqlite::Connection) {
    print_accounts_seen(conn);
    print_last_sync_times(conn);
    print_row_counts(conn);
    report_search_index_health(conn);
}

/// Print when each entity type was last synced from the Granola API.
fn print_last_sync_times(conn: &rusqlite::Connection) {
    let sync_keys = [
        "documents",
        "transcripts",
        "people",
        "calendars",
        "templates",
        "recipes",
    ];

    for key in sync_keys {
        let sync_key = format!("last_sync_{}", key);
        if let Ok(last_sync) = conn.query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            [&sync_key],
            |row| row.get::<_, String>(0),
        ) {
            println!("Last {} sync: {}", key, last_sync);
        }
    }
}

/// Print how much the database holds.
fn print_row_counts(conn: &rusqlite::Connection) {
    if let Ok(doc_count) =
        conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
    {
        println!("Documents: {}", doc_count);
    }

    if let Ok(transcript_count) =
        conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM transcript_utterances", [], |row| {
            row.get(0)
        })
    {
        println!("Transcript utterances: {}", transcript_count);
    }
}

/// Print which Granola accounts this database has synced from.
fn print_accounts_seen(conn: &rusqlite::Connection) {
    for line in accounts_seen_lines(conn) {
        println!("{}", line);
    }
}

/// The "Accounts seen" report, one line per entry. A pre-v017 database has
/// no accounts table; that reads as "could not be read", not as empty.
fn accounts_seen_lines(conn: &rusqlite::Connection) -> Vec<String> {
    match accounts::list_accounts(conn) {
        Ok(records) if records.is_empty() => vec!["Accounts seen: none".to_string()],
        Ok(records) => {
            let mut lines = vec!["Accounts seen:".to_string()];
            lines.extend(records.into_iter().map(|record| {
                format!(
                    "  {} (first seen {})",
                    account_label(&record.email, &record.account_id),
                    record.first_seen_at
                )
            }));
            lines
        }
        Err(e) => vec![format!("Accounts seen: could not be read ({})", e)],
    }
}

/// Print whether each full-text index still agrees with the table it indexes.
///
/// This is where FTS drift surfaces. The pull-time check in `db::integrity`
/// cannot do it: FTS5 spells its check as an `INSERT`, and SQLite refuses that
/// on the read-only connection that check deliberately opens.
fn report_search_index_health(conn: &rusqlite::Connection) {
    let reports = match integrity::check_fts_indexes(conn) {
        Ok(reports) => reports,
        Err(err) => {
            println!("Search indexes: could not be checked ({})", err);
            return;
        }
    };

    for report in reports {
        match report.state {
            integrity::FtsIndexState::Consistent => {
                println!("Search index {}: consistent", report.table);
            }
            integrity::FtsIndexState::Drifted(detail) => {
                println!(
                    "Search index {}: DRIFTED ({}) -- search under-reports; \
                     run 'grans admin db rebuild-fts' to repair",
                    report.table, detail
                );
            }
        }
    }
}

fn rebuild_search_indexes(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        println!("No database found at {}", db_path.display());
        return Ok(());
    }

    let conn = crate::db::connection::open_db_at_path(db_path)?;
    for table in integrity::rebuild_fts_indexes(&conn)? {
        println!("Rebuilt {}", table);
    }

    Ok(())
}

fn list_all_databases() -> Result<()> {
    let data_dir = crate::platform::data_dir()?;

    println!("Database directory: {}", data_dir.display());

    if !data_dir.exists() {
        println!("\nNo databases found (data directory doesn't exist)");
        return Ok(());
    }

    let mut databases = Vec::new();
    for entry in std::fs::read_dir(&data_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("db") {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let size_bytes = metadata.len();
                let size_mb = size_bytes as f64 / 1_048_576.0;
                databases.push((path, size_mb));
            }
        }
    }

    if databases.is_empty() {
        println!("\nNo database files found");
    } else {
        println!("\nFound {} database file(s):\n", databases.len());

        // Sort by filename for consistent output
        databases.sort_by(|a, b| a.0.cmp(&b.0));

        for (path, size_mb) in databases {
            println!("  {} ({:.2} MB)", path.display(), size_mb);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    /// A report that cannot read the database says so. Skipping to the next
    /// line left the command exiting 0 after printing a path and a size, which
    /// reads as a healthy, empty database.
    #[test]
    fn a_database_that_cannot_be_read_is_reported_as_a_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let corrupt = dir.path().join("grans.db");
        std::fs::write(&corrupt, b"this is not a SQLite database").unwrap();

        let result = super::show_database_info(&corrupt);

        assert!(
            result.is_err(),
            "a corrupt database must not report as fine"
        );
    }

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_clear_database() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        std::fs::write(&db_path, "test data").unwrap();

        assert!(db_path.exists());
        clear_database(&db_path).unwrap();
        assert!(!db_path.exists());
    }

    #[test]
    fn test_clear_nonexistent_database() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("nonexistent.db");

        // Should not error when clearing a non-existent database
        let result = clear_database(&db_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_database_info_nonexistent() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("nonexistent.db");

        // Should not error when showing info for non-existent database
        let result = show_database_info(&db_path);
        assert!(result.is_ok());
    }

    #[test]
    fn accounts_seen_lines_report_each_account() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = crate::db::migrations::open_and_migrate(&db_path).unwrap();

        assert_eq!(
            accounts_seen_lines(&conn),
            vec!["Accounts seen: none".to_string()]
        );

        crate::db::accounts::record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com")
            .unwrap();
        crate::db::accounts::record_account(&conn, "user_01BBB", Some("uuid-2"), "b@example.com")
            .unwrap();

        let lines = accounts_seen_lines(&conn);
        assert_eq!(lines[0], "Accounts seen:");
        assert!(lines[1].contains("a@example.com (user_01AAA)"));
        assert!(lines[1].contains("first seen "));
        assert!(lines[2].contains("b@example.com (user_01BBB)"));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn accounts_seen_lines_report_an_unreadable_log() {
        // A database without the accounts table (pre-v017) reports why the
        // log is unavailable instead of pretending it is empty.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let lines = accounts_seen_lines(&conn);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("Accounts seen: could not be read"));
    }

    #[test]
    fn test_show_database_info_existing() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        drop(crate::db::connection::open_db_at_path(&db_path).unwrap());

        assert!(db_path.exists());
        let result = show_database_info(&db_path);
        assert!(result.is_ok());
    }
}
