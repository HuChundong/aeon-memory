// port of src/core/store/sqlite.ts lines 407-429 (VectorStore constructor)

use rusqlite::{Connection, OpenFlags};
use std::sync::Mutex;

/// Thread-safe wrapper around rusqlite Connection.
/// The TS code uses DatabaseSync (not thread-safe) but protects via WAL mode
/// and sequential event loop. Here we use Mutex for safe concurrent access.
pub struct StoreConnection {
    pub conn: Mutex<Connection>,
    pub db_path: String,
}

impl StoreConnection {
    /// Open or create the SQLite database with WAL mode and performance pragmas.
    pub fn open(db_path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        conn.execute_batch("PRAGMA busy_timeout = 5000")?;
        conn.execute_batch("PRAGMA journal_mode = WAL")?;
        conn.execute_batch("PRAGMA cache_size = -65536")?;
        conn.execute_batch("PRAGMA mmap_size = 134217728")?;
        conn.execute_batch("PRAGMA wal_autocheckpoint = 1000")?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_string(),
        })
    }

    /// Open in read-only mode (for compatibility testing).
    pub fn open_readonly(db_path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> String {
        crate::test_support::unique_dir("aeon-memory-test-connection")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn test_open_creates_db() {
        let path = test_path("open_creates.db");
        let store = StoreConnection::open(&path).unwrap();
        assert!(std::path::Path::new(&path).exists());
        drop(store);
        crate::test_support::cleanup_db(&path);
    }

    #[test]
    fn test_wal_mode() {
        let path = test_path("wal_mode.db");
        let store = StoreConnection::open(&path).unwrap();
        let conn = store.conn.lock().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let timeout: i32 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
        drop(conn);
        crate::test_support::cleanup_db(&path);
    }
}
