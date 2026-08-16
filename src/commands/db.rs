use anyhow::Result;
use std::path::Path;

use crate::cli::args::DbAction;
use crate::db::accounts::{self, account_label};
use crate::db::integrity;

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
        std::fs::remove_file(db_path)?;
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
    for entry in std::fs::read_dir(&data_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("db") {
            std::fs::remove_file(&path)?;
            println!("Cleared: {}", path.display());
            count += 1;
        }
    }

    if count == 0 {
        println!("No database files found");
    } else {
        println!("\nCleared {} database file(s)", count);
    }

    Ok(())
}

fn show_database_info(db_path: &Path) -> Result<()> {
    println!("Database path: {}", db_path.display());

    if db_path.exists() {
        let metadata = std::fs::metadata(db_path)?;
        let size_bytes = metadata.len();
        let size_mb = size_bytes as f64 / 1_048_576.0;

        println!("Database size: {:.2} MB ({} bytes)", size_mb, size_bytes);
        println!("Status: exists");

        // Try to read metadata from the database
        if let Ok(conn) = rusqlite::Connection::open(db_path) {
            // From PRAGMA user_version, which is what the migration system
            // actually tracks. The `metadata` row this used to read is a fossil
            // of the pre-migration scheme: nothing has written it since, so it
            // reported 3 on a database at 14.
            if let Ok(schema_version) = crate::db::migrations::get_schema_version(&conn) {
                println!("Schema version: {}", schema_version);
            }

            // Which accounts this database has synced from.
            match accounts::list_accounts(&conn) {
                Ok(records) if records.is_empty() => println!("Accounts seen: none"),
                Ok(records) => {
                    println!("Accounts seen:");
                    for record in records {
                        println!(
                            "  {} (first seen {})",
                            account_label(&record.email, &record.account_id),
                            record.first_seen_at
                        );
                    }
                }
                Err(e) => println!("Accounts seen: could not be read ({})", e),
            }

            // Show last sync times
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

            // Show document/transcript counts
            if let Ok(doc_count) =
                conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
            {
                println!("Documents: {}", doc_count);
            }

            if let Ok(transcript_count) = conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM transcript_utterances",
                [],
                |row| row.get(0),
            ) {
                println!("Transcript utterances: {}", transcript_count);
            }

            report_search_index_health(&conn);
        }
    } else {
        println!("Status: does not exist (run 'grans sync' to create)");
    }

    Ok(())
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
    fn test_show_database_info_reports_accounts_seen() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = crate::db::migrations::open_and_migrate(&db_path).unwrap();
        crate::db::accounts::record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com")
            .unwrap();
        drop(conn);

        assert!(show_database_info(&db_path).is_ok());
    }

    #[test]
    fn test_show_database_info_existing() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        std::fs::write(&db_path, "mock db data").unwrap();

        assert!(db_path.exists());
        let result = show_database_info(&db_path);
        assert!(result.is_ok());
    }
}
