// port of src/core/store/sqlite.ts initSchema() method
//
// DDL from sqlite.ts:489-829.
// vec0 extension loading mirrors sqlite.ts:452-465 (load → catch → degrade).

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Result of schema initialization.
#[derive(Debug, Clone)]
pub struct SchemaInitResult {
    pub needs_reindex: bool,
    pub reason: Option<String>,
    pub vec_available: bool,
    pub fts_available: bool,
}

/// Embedded provider meta (serialized as key-value in embedding_meta table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    pub provider: String,
    pub model: String,
    pub dimensions: u32,
}

/// Search paths for the sqlite-vec loadable extension binary.
/// Priority:
///   1. AEON_MEMORY_VEC0_PATH env var (explicit path, highest priority)
///   2. Directory containing the running aeon-memory/aeon-memory-server executable
///   3. CARGO_MANIFEST_DIR/tests/fixtures/  (test helper)
///   4. LD_LIBRARY_PATH / DYLD_LIBRARY_PATH / PATH
///   5. System library paths
fn vec_extension_search_paths() -> Vec<PathBuf> {
    vec_extension_search_paths_with(
        std::env::var_os("AEON_MEMORY_VEC0_PATH").map(PathBuf::from),
        std::env::current_exe().ok(),
    )
}

fn vec_extension_search_paths_with(
    explicit: Option<PathBuf>,
    executable: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Explicit env var (highest priority)
    if let Some(explicit) = explicit {
        paths.push(explicit);
    }

    // 2. Native release packages place vec0 next to both executables. This
    // works for aeon-memory/aeon-memory-server on Linux, macOS and Windows and does not
    // depend on the process working directory.
    if let Some(executable_dir) = executable.as_deref().and_then(Path::parent) {
        paths.push(executable_dir.to_path_buf());
    }

    // 3. CARGO_MANIFEST_DIR for test fixtures
    paths.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures"),
    );

    // 4. Dynamic library paths from environment. split_paths handles ':' on
    // Unix and ';' plus drive letters on Windows correctly.
    for var in &[
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
        "PATH",
    ] {
        if let Some(value) = std::env::var_os(var) {
            paths.extend(std::env::split_paths(&value).filter(|path| !path.as_os_str().is_empty()));
        }
    }

    // 5. Common Unix system paths. Windows system directories are already
    // represented by PATH and executable-sibling discovery above.
    paths.push(PathBuf::from("/usr/local/lib"));
    paths.push(PathBuf::from("/opt/homebrew/lib"));
    paths.push(PathBuf::from("/usr/lib"));
    paths.push(PathBuf::from("/lib"));

    paths
}

/// Find and return the path to the vec0 loadable extension.
/// Returns None if not found on any search path.
fn find_vec_extension() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    const EXT_NAME: &str = "vec0.dylib";
    #[cfg(target_os = "linux")]
    const EXT_NAME: &str = "vec0.so";
    #[cfg(target_os = "windows")]
    const EXT_NAME: &str = "vec0.dll";

    find_vec_extension_in_paths(&vec_extension_search_paths(), EXT_NAME)
}

fn find_vec_extension_in_paths(paths: &[PathBuf], extension_name: &str) -> Option<PathBuf> {
    for dir in paths {
        if !dir.exists() {
            continue;
        }
        if dir.is_file() && dir.file_name().is_some_and(|name| name == extension_name) {
            return Some(dir.clone());
        }

        // Search recursively up to 2 levels for the extension file
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Look one level deeper
                    if let Ok(sub) = std::fs::read_dir(&path) {
                        for sub_entry in sub.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path
                                .file_name()
                                .is_some_and(|name| name == extension_name)
                            {
                                return Some(sub_path);
                            }
                        }
                    }
                } else if path.file_name().is_some_and(|name| name == extension_name) {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Try to load the sqlite-vec extension. Mirrors TS behavior at sqlite.ts:452-465.
/// Returns `true` if the extension was loaded successfully (vec0 tables available).
pub fn try_load_vec_extension(conn: &Connection) -> bool {
    let ext_path = find_vec_extension();
    try_load_vec_extension_path(conn, ext_path.as_deref())
}

fn try_load_vec_extension_path(conn: &Connection, ext_path: Option<&Path>) -> bool {
    let ext_path = match ext_path {
        Some(path) => path,
        None => return false,
    };

    // Safety: the vec0 extension is a trusted first-party extension from the
    // official sqlite-vec project (https://github.com/asg017/sqlite-vec).
    #[allow(clippy::let_unit_value)]
    unsafe {
        let guard = conn.load_extension_enable();
        if guard.is_err() {
            return false;
        }
        let _guard_lifetime = guard.unwrap();
        let result = conn.load_extension(ext_path.to_str().unwrap_or(""), None);
        // _guard_lifetime dropped here → load_extension_disable() called
        result.is_ok()
    }
}

/// Create all core tables IF NOT EXISTS.
/// Always executes regardless of sqlite-vec availability.
/// Vec0 tables are created only if [try_load_vec_extension] was called first
/// and the extension loaded successfully.
pub fn initialize_schema(
    conn: &Connection,
    provider_info: Option<&aeon_memory_core::types::EmbeddingProviderInfo>,
    dimensions: u32,
) -> Result<SchemaInitResult, rusqlite::Error> {
    // Detect vec0 availability
    let vec_available = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'l1_vec'",
            [],
            |row| row.get::<_, String>(0),
        )
        .is_ok();

    // Check FTS availability
    let fts_available = conn
        .query_row("SELECT COUNT(*) FROM l1_fts", [], |row| {
            row.get::<_, i64>(0)
        })
        .is_ok();

    // ── embedding_meta table ──
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embedding_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )?;

    // ── Reindex detection ──
    let mut result = SchemaInitResult {
        needs_reindex: false,
        reason: None,
        vec_available,
        fts_available,
    };

    if let Some(info) = provider_info {
        let saved = read_embedding_meta(conn)?;
        if let Some(saved) = saved {
            let mut reasons = Vec::new();
            if saved.provider != info.provider {
                reasons.push(format!("provider: {} → {}", saved.provider, info.provider));
            }
            if saved.model != info.model {
                reasons.push(format!("model: {} → {}", saved.model, info.model));
            }
            if saved.dimensions != dimensions {
                reasons.push(format!("dimensions: {} → {}", saved.dimensions, dimensions));
            }
            if !reasons.is_empty() {
                drop_vec_tables(conn)?;
                result.needs_reindex = true;
                result.reason = Some(reasons.join(", "));
            }
        } else {
            let l1 = table_row_count(conn, "l1_records");
            let l0 = table_row_count(conn, "l0_conversations");
            if l1 > 0 || l0 > 0 {
                drop_vec_tables(conn)?;
                result.needs_reindex = true;
                result.reason = Some(
                    "legacy DB without embedding_meta — cannot verify vector compatibility"
                        .to_string(),
                );
            } else if get_vec_table_dimensions(conn).is_some_and(|saved| saved != dimensions) {
                // No records exist, so table recreation is required but there
                // is nothing to re-embed and needs_reindex remains false.
                drop_vec_tables(conn)?;
            }
        }
    }

    // ── L1 schema ──
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS l1_records (
            record_id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            type TEXT DEFAULT '',
            priority INTEGER DEFAULT 50,
            scene_name TEXT DEFAULT '',
            session_key TEXT DEFAULT '',
            session_id TEXT DEFAULT '',
            timestamp_str TEXT DEFAULT '',
            timestamp_start TEXT DEFAULT '',
            timestamp_end TEXT DEFAULT '',
            created_time TEXT DEFAULT '',
            updated_time TEXT DEFAULT '',
            metadata_json TEXT DEFAULT '{}'
        )",
    )?;

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_l1_type ON l1_records(type);
         CREATE INDEX IF NOT EXISTS idx_l1_session_key ON l1_records(session_key);
         CREATE INDEX IF NOT EXISTS idx_l1_session_id ON l1_records(session_id);
         CREATE INDEX IF NOT EXISTS idx_l1_scene ON l1_records(scene_name);
         CREATE INDEX IF NOT EXISTS idx_l1_ts_start ON l1_records(timestamp_start);
         CREATE INDEX IF NOT EXISTS idx_l1_ts_end ON l1_records(timestamp_end);
         CREATE INDEX IF NOT EXISTS idx_l1_session_updated ON l1_records(session_id, updated_time);
         CREATE INDEX IF NOT EXISTS idx_l1_sessionkey_updated ON l1_records(session_key, updated_time)",
    )?;

    // ── L0 schema ──
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS l0_conversations (
            record_id TEXT PRIMARY KEY,
            session_key TEXT NOT NULL,
            session_id TEXT DEFAULT '',
            role TEXT NOT NULL DEFAULT '',
            message_text TEXT NOT NULL,
            recorded_at TEXT DEFAULT '',
            timestamp INTEGER DEFAULT 0
        )",
    )?;

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_l0_session ON l0_conversations(session_key);
         CREATE INDEX IF NOT EXISTS idx_l0_session_id ON l0_conversations(session_id);
         CREATE INDEX IF NOT EXISTS idx_l0_recorded ON l0_conversations(recorded_at);
         CREATE INDEX IF NOT EXISTS idx_l0_timestamp ON l0_conversations(timestamp)",
    )?;

    // ── FTS5 tables ──
    if create_fts_tables(conn).is_ok() {
        result.fts_available = true;
    }

    Ok(result)
}

/// Create vec0 tables. Requires sqlite-vec extension to be loaded first.
pub fn create_vec_tables(conn: &Connection, dimensions: u32) -> Result<(), rusqlite::Error> {
    let l1 = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS l1_vec USING vec0(
            record_id TEXT PRIMARY KEY,
            embedding float[{dims}] distance_metric=cosine,
            updated_time TEXT DEFAULT ''
        )",
        dims = dimensions
    );
    conn.execute_batch(&l1)?;

    let l0 = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS l0_vec USING vec0(
            record_id TEXT PRIMARY KEY,
            embedding float[{dims}] distance_metric=cosine,
            recorded_at TEXT DEFAULT ''
        )",
        dims = dimensions
    );
    conn.execute_batch(&l0)?;
    Ok(())
}

pub(crate) fn table_row_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// Drop vec0 tables (for reindex).
fn drop_vec_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    let _ = conn.execute_batch("DROP TABLE IF EXISTS l1_vec");
    let _ = conn.execute_batch("DROP TABLE IF EXISTS l0_vec");
    Ok(())
}

fn get_vec_table_dimensions(conn: &Connection) -> Option<u32> {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='l1_vec'",
            [],
            |row| row.get(0),
        )
        .ok()?;
    let marker = "float[";
    let start = sql.find(marker)? + marker.len();
    let end = sql[start..].find(']')? + start;
    sql[start..end].parse().ok()
}

/// Create FTS5 virtual tables. Returns Err if FTS5 is not available.
fn create_fts_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS l1_fts USING fts5(
            content, content_original UNINDEXED,
            record_id UNINDEXED, type UNINDEXED, priority UNINDEXED,
            scene_name UNINDEXED, session_key UNINDEXED, session_id UNINDEXED,
            timestamp_str UNINDEXED, timestamp_start UNINDEXED,
            timestamp_end UNINDEXED, metadata_json UNINDEXED
        )",
    )?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS l0_fts USING fts5(
            message_text, message_text_original UNINDEXED,
            record_id UNINDEXED, session_key UNINDEXED, session_id UNINDEXED,
            role UNINDEXED, recorded_at UNINDEXED, timestamp UNINDEXED
        )",
    )?;
    Ok(())
}

// ── Meta helpers ──

pub fn write_embedding_meta(
    conn: &Connection,
    meta: &EmbeddingMeta,
) -> Result<(), rusqlite::Error> {
    // Match the TypeScript database format exactly. Readers still accept the
    // historical Rust three-key representation below.
    let value = serde_json::to_string(meta)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    conn.execute(
        "INSERT INTO embedding_meta (key, value) VALUES ('embedding_provider_info', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [&value],
    )?;
    Ok(())
}

pub fn read_embedding_meta(conn: &Connection) -> Result<Option<EmbeddingMeta>, rusqlite::Error> {
    // Prefer the canonical TypeScript single-key JSON representation. A
    // malformed value is a database compatibility error, not "missing" meta.
    let canonical: Option<String> = conn
        .query_row(
            "SELECT value FROM embedding_meta WHERE key = 'embedding_provider_info'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(value) = canonical {
        return serde_json::from_str(&value).map(Some).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        });
    }

    // Backward compatibility for databases written by earlier Rust builds.
    let p: Option<String> = conn
        .query_row(
            "SELECT value FROM embedding_meta WHERE key = 'provider'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let m: Option<String> = conn
        .query_row(
            "SELECT value FROM embedding_meta WHERE key = 'model'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let d: Option<String> = conn
        .query_row(
            "SELECT value FROM embedding_meta WHERE key = 'dimensions'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match (p, m, d) {
        (Some(provider), Some(model), Some(dimensions)) => {
            let dimensions = dimensions.parse().map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(Some(EmbeddingMeta {
                provider,
                model,
                dimensions,
            }))
        }
        _ => Ok(None),
    }
}

pub fn table_row_count(conn: &Connection, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {}", table);
    conn.query_row(&sql, [], |row| row.get(0)).unwrap_or(0)
}

pub fn is_fts_available(conn: &Connection) -> bool {
    conn.query_row("SELECT COUNT(*) FROM l1_fts", [], |row| {
        row.get::<_, i64>(0)
    })
    .is_ok()
}

// ── Vec0 extension loading test ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::StoreConnection;

    fn setup(name: &str) -> (StoreConnection, String) {
        let dir = crate::test_support::unique_dir("aeon-memory-test-vec");
        let path = dir.join(name).to_string_lossy().to_string();
        let store = StoreConnection::open(&path).unwrap();
        (store, path)
    }

    fn cleanup(path: &str) {
        crate::test_support::cleanup_db(path);
    }

    #[test]
    fn vec0_needs_explicit_load_call() {
        // vec0 table creation requires the extension to be loaded explicitly
        // via try_load_vec_extension(). Without that call, vec0 CREATE fails.
        let (store, path) = setup("vec0_no_load.db");
        let conn = store.conn.lock().unwrap();
        let r = create_vec_tables(&conn, 1536);
        assert!(
            r.is_err(),
            "vec0 CREATE should fail before extension is loaded"
        );
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn explicit_vec0_path_has_highest_priority_and_accepts_a_file() {
        let dir =
            std::env::temp_dir().join(format!("aeon-memory-vec-explicit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join(NATIVE_VEC0_NAME);
        std::fs::write(&fixture, b"discovery only").unwrap();
        let paths = vec_extension_search_paths_with(Some(fixture.clone()), None);
        let found = find_vec_extension_in_paths(&paths[..1], NATIVE_VEC0_NAME);
        assert_eq!(found.as_deref(), Some(fixture.as_path()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    const NATIVE_VEC0_NAME: &str = "vec0.dylib";
    #[cfg(target_os = "linux")]
    const NATIVE_VEC0_NAME: &str = "vec0.so";
    #[cfg(target_os = "windows")]
    const NATIVE_VEC0_NAME: &str = "vec0.dll";

    #[test]
    fn executable_sibling_is_discovered_after_explicit_override() {
        let root =
            std::env::temp_dir().join(format!("aeon-memory-vec-sibling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        let override_dir = root.join("override");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&override_dir).unwrap();
        let executable = bin.join(if cfg!(windows) {
            "aeon-memory-server.exe"
        } else {
            "aeon-memory-server"
        });
        std::fs::write(&executable, b"test executable").unwrap();
        let sibling = bin.join(NATIVE_VEC0_NAME);
        let explicit = override_dir.join(NATIVE_VEC0_NAME);
        std::fs::write(&sibling, b"sibling").unwrap();
        std::fs::write(&explicit, b"explicit").unwrap();

        let paths = vec_extension_search_paths_with(Some(explicit.clone()), Some(executable));
        assert_eq!(paths.first(), Some(&explicit));
        assert_eq!(paths.get(1), Some(&bin));
        assert_eq!(
            find_vec_extension_in_paths(&paths[..2], NATIVE_VEC0_NAME),
            Some(explicit)
        );
        assert_eq!(
            find_vec_extension_in_paths(&paths[1..2], NATIVE_VEC0_NAME),
            Some(sibling)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_or_invalid_sibling_fails_soft_without_accepting_wrong_files() {
        let root =
            std::env::temp_dir().join(format!("aeon-memory-vec-invalid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("not-vec0.dll"), b"wrong name").unwrap();
        assert_eq!(
            find_vec_extension_in_paths(std::slice::from_ref(&root), NATIVE_VEC0_NAME),
            None
        );

        let invalid = root.join(NATIVE_VEC0_NAME);
        std::fs::write(&invalid, b"not a loadable library").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        assert!(!try_load_vec_extension_path(&conn, Some(&invalid)));
        assert!(!try_load_vec_extension_path(&conn, None));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn vec0_available_with_extension() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        assert!(
            fixtures.join("vec0.dylib").exists(),
            "required vec0.dylib fixture missing at {:?}",
            fixtures.join("vec0.dylib")
        );

        let (store, path) = setup("vec0_with_ext.db");
        let conn = store.conn.lock().unwrap();

        // First initialize core schema (creates l1_records, etc.)
        initialize_schema(&conn, None, 0).unwrap();

        assert!(
            try_load_vec_extension(&conn),
            "vec0.dylib exists but failed to load"
        );
        create_vec_tables(&conn, 4).unwrap();

        conn.execute(
            "INSERT INTO l1_records (record_id, content, type, priority, scene_name, session_key, session_id, created_time, updated_time, metadata_json)
             VALUES ('vec_test_1', 'vector test one', 'episodic', 50, 'vec-scene', 'vec-session', '', '2026-07-13T00:00:00Z', '2026-07-13T00:00:00Z', '{}')",
            [],
        ).unwrap();

        // vec0 accepts JSON array text format for float vectors
        let insert = conn.execute(
            "INSERT INTO l1_vec (record_id, embedding, updated_time) VALUES ('vec_test_1', '[0.1,0.2,0.3,0.4]', '2026-07-13T00:00:00Z')",
            [],
        );
        assert!(insert.is_ok(), "vec0 insert: {:?}", insert.err());

        // KNN search
        let mut stmt = conn.prepare(
            "SELECT record_id, distance FROM l1_vec WHERE embedding MATCH ?1 AND k = 5 ORDER BY distance"
        ).unwrap();
        let results: Vec<(String, f64)> = stmt
            .query_map(["[0.1,0.2,0.3,0.4]"], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert_eq!(results.len(), 1, "KNN search should return 1 vector");
        assert_eq!(results[0].0, "vec_test_1");
        assert!(
            results[0].1.abs() < 0.001,
            "distance should be near 0 for identical vector"
        );

        drop(stmt);
        drop(conn);
        cleanup(&path);
    }
}
