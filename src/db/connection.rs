use anyhow::Result;
use log::debug;
use rusqlite::Connection;

use crate::db::migrations;

/// Get the default database path.
/// All data is stored in a single database file.
pub fn default_db_path() -> Result<std::path::PathBuf> {
    let data_dir = crate::platform::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir.join("grans.db"))
}

/// Open or create the grans database.
/// Uses the migration system to ensure the schema is up-to-date.
/// Backs up the database before applying any pending migrations.
pub fn open_or_create_db() -> Result<Connection> {
    let db_path = default_db_path()?;
    debug!("Opening database at {}", db_path.display());
    let conn = migrations::open_and_migrate(&db_path)?;
    let version = migrations::get_schema_version(&conn).unwrap_or(0);
    debug!("Database opened (schema version {})", version);
    Ok(conn)
}

/// Open a database at a specific path.
/// Uses the migration system to ensure the schema is up-to-date.
pub fn open_db_at_path(path: &std::path::Path) -> Result<Connection> {
    debug!("Opening database at {} (custom path)", path.display());
    let conn = migrations::open_and_migrate(path)?;
    let version = migrations::get_schema_version(&conn).unwrap_or(0);
    debug!("Database opened (schema version {})", version);
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_open_or_create_db_creates_schema() {
        // This test uses the real default_db_path which we can't easily override,
        // so we test the underlying migration function directly
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = migrations::open_and_migrate(&db_path).unwrap();

        // Verify key tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_schema_version_tracked() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = migrations::open_and_migrate(&db_path).unwrap();
        let version = migrations::get_schema_version(&conn).unwrap();

        // After applying all migrations, version should be 13
        assert_eq!(version, 13);
    }
}
