// Cross-compatibility tests: Rust opens a vectors.db generated through the real
// TypeScript VectorStore runtime and verifies schema + data match.

use aeon_memory_core::types::IMemoryStore;
use aeon_memory_store_sqlite::{connection::StoreConnection, l0, l1, schema};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    if name == "vectors-ts.db"
        && let Some(path) = std::env::var_os("AEON_MEMORY_TS_DB_FIXTURE")
    {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn cross_compat_l1_schema_readable() {
    let path = fixture_path("vectors-ts.db");
    assert!(path.exists(), "Run: node tests/fixtures/gen-ts-db.mjs");

    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();
    assert!(
        schema::try_load_vec_extension(&conn),
        "Rust must load sqlite-vec to read TS vec0 fixture rows"
    );

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM l1_records", [], |r| r.get(0))
        .unwrap();
    assert!(count > 0, "l1_records should have data");

    // Verify columns match TS DDL
    let mut stmt = conn.prepare("PRAGMA table_info(l1_records)").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(cols.len(), 13);
    for c in &[
        "record_id",
        "content",
        "type",
        "priority",
        "scene_name",
        "session_key",
        "session_id",
        "timestamp_str",
        "timestamp_start",
        "timestamp_end",
        "created_time",
        "updated_time",
        "metadata_json",
    ] {
        assert!(cols.contains(&c.to_string()), "missing L1 col: {}", c);
    }
}

#[test]
fn cross_compat_l0_schema_readable() {
    let path = fixture_path("vectors-ts.db");
    assert!(path.exists());

    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();

    let mut stmt = conn.prepare("PRAGMA table_info(l0_conversations)").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(cols.len(), 7);
    for c in &[
        "record_id",
        "session_key",
        "session_id",
        "role",
        "message_text",
        "recorded_at",
        "timestamp",
    ] {
        assert!(cols.contains(&c.to_string()), "missing L0 col: {}", c);
    }
}

#[test]
fn cross_compat_indexes_match_ts() {
    let path = fixture_path("vectors-ts.db");
    assert!(path.exists());

    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();

    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%' ORDER BY name"
    ).unwrap();
    let indexes: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    for name in &[
        "idx_l1_type",
        "idx_l1_session_key",
        "idx_l1_scene",
        "idx_l1_session_updated",
        "idx_l1_sessionkey_updated",
        "idx_l0_session",
        "idx_l0_recorded",
        "idx_l0_timestamp",
    ] {
        assert!(
            indexes.contains(&name.to_string()),
            "Missing index: {}",
            name
        );
    }
}

#[test]
fn cross_compat_embedding_meta() {
    let path = fixture_path("vectors-ts.db");
    assert!(path.exists());

    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let meta = schema::read_embedding_meta(&store.conn.lock().unwrap())
        .expect("read TS embedding metadata")
        .expect("Rust must understand TS embedding_provider_info metadata key");
    assert_eq!(meta.provider, "test");
    assert_eq!(meta.model, "test-model");
    assert_eq!(meta.dimensions, 4);
}

#[test]
fn rust_embedding_meta_writes_ts_single_key_and_reads_it_back() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE embedding_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .unwrap();
    let expected = schema::EmbeddingMeta {
        provider: "openai".into(),
        model: "text-embedding-3-small".into(),
        dimensions: 1536,
    };
    schema::write_embedding_meta(&conn, &expected).unwrap();

    let value: String = conn
        .query_row(
            "SELECT value FROM embedding_meta WHERE key = 'embedding_provider_info'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&value).unwrap(),
        serde_json::json!({
            "provider": "openai",
            "model": "text-embedding-3-small",
            "dimensions": 1536
        })
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM embedding_meta", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let actual = schema::read_embedding_meta(&conn).unwrap().unwrap();
    assert_eq!(actual.provider, expected.provider);
    assert_eq!(actual.model, expected.model);
    assert_eq!(actual.dimensions, expected.dimensions);
}

#[test]
fn embedding_meta_reads_legacy_rust_three_key_format() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE embedding_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO embedding_meta VALUES ('provider', 'legacy');
         INSERT INTO embedding_meta VALUES ('model', 'legacy-model');
         INSERT INTO embedding_meta VALUES ('dimensions', '768');",
    )
    .unwrap();
    let actual = schema::read_embedding_meta(&conn).unwrap().unwrap();
    assert_eq!(actual.provider, "legacy");
    assert_eq!(actual.model, "legacy-model");
    assert_eq!(actual.dimensions, 768);
}

#[test]
fn embedding_meta_prefers_ts_single_key_over_legacy_keys() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"CREATE TABLE embedding_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
           INSERT INTO embedding_meta VALUES
             ('embedding_provider_info', '{"provider":"canonical","model":"canonical-model","dimensions":1024}'),
             ('provider', 'legacy'),
             ('model', 'legacy-model'),
             ('dimensions', '768');"#,
    )
    .unwrap();
    let actual = schema::read_embedding_meta(&conn).unwrap().unwrap();
    assert_eq!(actual.provider, "canonical");
    assert_eq!(actual.model, "canonical-model");
    assert_eq!(actual.dimensions, 1024);
}

#[test]
fn embedding_meta_does_not_swallow_malformed_values() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE embedding_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO embedding_meta VALUES ('embedding_provider_info', '{bad json}');",
    )
    .unwrap();
    assert!(schema::read_embedding_meta(&conn).is_err());

    conn.execute(
        "DELETE FROM embedding_meta WHERE key = 'embedding_provider_info'",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO embedding_meta VALUES ('provider', 'legacy');
         INSERT INTO embedding_meta VALUES ('model', 'legacy-model');
         INSERT INTO embedding_meta VALUES ('dimensions', 'not-a-number');",
    )
    .unwrap();
    assert!(schema::read_embedding_meta(&conn).is_err());
}

#[test]
fn cross_compat_ts_vec0_rows_are_present() {
    let path = fixture_path("vectors-ts.db");
    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();
    assert!(
        schema::try_load_vec_extension(&conn),
        "Rust must load sqlite-vec to read TS vec0 fixture rows"
    );
    let l0: i64 = conn
        .query_row("SELECT count(*) FROM l0_vec", [], |row| row.get(0))
        .expect("TS runtime fixture must contain l0_vec; sqlite-vec absence is a hard failure");
    let l1: i64 = conn
        .query_row("SELECT count(*) FROM l1_vec", [], |row| row.get(0))
        .expect("TS runtime fixture must contain l1_vec; sqlite-vec absence is a hard failure");
    assert_eq!((l0, l1), (2, 1));
}

#[test]
fn cross_compat_ts_vec0_embeddings_are_searchable_by_rust() {
    let path = fixture_path("vectors-ts.db");
    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();
    assert!(schema::try_load_vec_extension(&conn));

    let l0_rows = l0::search_l0_vector(&conn, &[1.0, 0.0, 0.0, 0.0], 2).unwrap();
    assert_eq!(
        l0_rows
            .iter()
            .map(|row| row.record_id.as_str())
            .collect::<Vec<_>>(),
        ["l0_compat_001", "l0_compat_002"]
    );
    assert!((l0_rows[0].score - 1.0).abs() < 1e-6);

    let l1_rows = l1::search_l1_vector(&conn, &[0.5, 0.5, 0.5, 0.5], 1).unwrap();
    assert_eq!(l1_rows.len(), 1);
    assert_eq!(l1_rows[0].record_id, "l1_compat_001");
    assert!((l1_rows[0].score - 1.0).abs() < 1e-6);
}

#[test]
fn cross_compat_l0_data_roundtrip() {
    let path = fixture_path("vectors-ts.db");
    assert!(path.exists());

    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();

    let rows = l0::query_l0_for_session(&conn, "session-compat", 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].message_text, "Hello from TS generator");
    assert_eq!(rows[1].role, "user");
    assert_eq!(rows[0].role, "assistant");
}

#[test]
fn cross_compat_l1_data_roundtrip() {
    let path = fixture_path("vectors-ts.db");
    assert!(path.exists());

    let store = StoreConnection::open_readonly(path.to_str().unwrap()).unwrap();
    let conn = store.conn.lock().unwrap();

    let rows = l1::query_l1_records(
        &conn,
        &aeon_memory_core::types::L1QueryFilter {
            session_key: Some("session-compat".to_string()),
            session_id: None,
            updated_after: None,
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].record_id, "l1_compat_001");
    assert_eq!(
        rows[0].content,
        "test user memory for cross-compat verification"
    );
    assert_eq!(rows[0].r#type, "persona");
}

#[test]
fn cross_compat_rust_writes_match_ts_schema() {
    // Verify Rust-written data is schema-compatible with TS DDL
    let dir = std::env::temp_dir().join("aeon-memory-cross-compat-rust");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rust-written.db").to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);

    let store = StoreConnection::open(&path).unwrap();
    let conn = store.conn.lock().unwrap();
    schema::initialize_schema(&conn, None, 0).unwrap();
    drop(conn);

    let mut store2 = aeon_memory_store_sqlite::VectorStore::new(&path, 0);
    store2.init(None).unwrap();

    // Write via IMemoryStore trait
    let l0_rec = aeon_memory_core::types::L0Record {
        id: "rust_compat_l0".to_string(),
        session_key: "rust-sk".to_string(),
        session_id: "".to_string(),
        role: "user".to_string(),
        message_text: "Rust written L0 for compat".to_string(),
        recorded_at: "2026-07-13T12:00:00Z".to_string(),
        timestamp: 1000,
    };
    assert!(store2.upsert_l0(&l0_rec, None).unwrap());

    let l1_rec = aeon_memory_core::types::L1RecordRow {
        record_id: "rust_compat_l1".to_string(),
        content: "Rust written L1 for compat".to_string(),
        r#type: "instruction".to_string(),
        priority: 80.0,
        scene_name: "rust-scene".to_string(),
        session_key: "rust-sk".to_string(),
        session_id: "".to_string(),
        timestamp_str: "2026-07-13".to_string(),
        timestamp_start: "".to_string(),
        timestamp_end: "".to_string(),
        created_time: "2026-07-13T12:00:00Z".to_string(),
        updated_time: "2026-07-13T12:00:01Z".to_string(),
        metadata_json: r#"{"source":"rust"}"#.to_string(),
    };
    assert!(store2.upsert_l1(&l1_rec, None).unwrap());

    // Read back via free functions (same path TS consumers would use)
    let conn = store.conn.lock().unwrap();
    let l0_rows = l0::query_l0_for_session(&conn, "rust-sk", 10).unwrap();
    assert_eq!(l0_rows.len(), 1);
    assert_eq!(l0_rows[0].message_text, "Rust written L0 for compat");

    let l1_rows = l1::query_l1_records(
        &conn,
        &aeon_memory_core::types::L1QueryFilter {
            session_key: Some("rust-sk".to_string()),
            session_id: None,
            updated_after: None,
        },
    )
    .unwrap();
    assert_eq!(l1_rows.len(), 1);
    assert_eq!(l1_rows[0].content, "Rust written L1 for compat");
    assert_eq!(l1_rows[0].metadata_json, r#"{"source":"rust"}"#);
    drop(conn);

    store2.close();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cross_compat_idempotent_schema() {
    // Multiple initialize_schema calls should be safe (IF NOT EXISTS)
    let dir = std::env::temp_dir().join("aeon-memory-cross-compat-idemp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("idemp.db").to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);

    let store = StoreConnection::open(&path).unwrap();
    let conn = store.conn.lock().unwrap();
    schema::initialize_schema(&conn, None, 0).unwrap();
    schema::initialize_schema(&conn, None, 0).unwrap();
    schema::initialize_schema(&conn, None, 0).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM l1_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cleanup_blocks_ninety_percent_and_allows_exactly_eighty_while_ignoring_empty_time() {
    let dir =
        std::env::temp_dir().join(format!("aeon-memory-cleanup-ratio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cleanup.db").to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);
    let mut store = aeon_memory_store_sqlite::VectorStore::new(&path, 0);
    store.init(None).unwrap();
    for index in 0..100 {
        let time = if index < 90 {
            "2020-01-01T00:00:00Z"
        } else if index == 99 {
            ""
        } else {
            "2030-01-01T00:00:00Z"
        };
        store
            .upsert_l0(
                &aeon_memory_core::types::L0Record {
                    id: format!("l0-{index:03}"),
                    session_key: "cleanup".into(),
                    session_id: "round".into(),
                    role: "user".into(),
                    message_text: format!("l0 {index}"),
                    recorded_at: time.into(),
                    timestamp: index,
                },
                None,
            )
            .unwrap();
        store
            .upsert_l1(
                &aeon_memory_core::types::L1RecordRow {
                    record_id: format!("l1-{index:03}"),
                    content: format!("l1 {index}"),
                    r#type: "episodic".into(),
                    priority: 50.0,
                    scene_name: "cleanup".into(),
                    session_key: "cleanup".into(),
                    session_id: "round".into(),
                    timestamp_str: time.into(),
                    timestamp_start: time.into(),
                    timestamp_end: time.into(),
                    created_time: time.into(),
                    updated_time: time.into(),
                    metadata_json: "{}".into(),
                },
                None,
            )
            .unwrap();
    }
    assert_eq!(store.delete_l0_expired("2026-01-01T00:00:00Z").unwrap(), 0);
    assert_eq!(store.delete_l1_expired("2026-01-01T00:00:00Z").unwrap(), 0);
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE l0_conversations SET recorded_at='2030-01-01T00:00:00Z' WHERE record_id IN (SELECT record_id FROM l0_conversations WHERE recorded_at != '' ORDER BY record_id LIMIT 10)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE l1_records SET updated_time='2030-01-01T00:00:00Z' WHERE record_id IN (SELECT record_id FROM l1_records WHERE updated_time != '' ORDER BY record_id LIMIT 10)",
            [],
        )
        .unwrap();
    }
    assert_eq!(store.delete_l0_expired("2026-01-01T00:00:00Z").unwrap(), 80);
    assert_eq!(store.delete_l1_expired("2026-01-01T00:00:00Z").unwrap(), 80);
    assert_eq!(store.count_l0().unwrap(), 20);
    assert_eq!(store.count_l1().unwrap(), 20);
    store.close();
    let _ = std::fs::remove_dir_all(dir);
}
