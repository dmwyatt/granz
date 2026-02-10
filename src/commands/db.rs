use anyhow::Result;
use std::path::Path;

use crate::cli::args::DbAction;

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
            if let Ok(schema_version) = conn.query_row(
                "SELECT value FROM metadata WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            ) {
                println!("Schema version: {}", schema_version);
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
        }
    } else {
        println!("Status: does not exist (run 'grans sync' to create)");
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
    fn test_show_database_info_existing() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        std::fs::write(&db_path, "mock db data").unwrap();

        assert!(db_path.exists());
        let result = show_database_info(&db_path);
        assert!(result.is_ok());
    }
}
