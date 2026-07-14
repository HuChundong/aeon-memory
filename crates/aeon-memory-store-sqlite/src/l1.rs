// port of src/core/store/sqlite.ts L1 operations (l1_records + l1_vec + l1_fts)

use rusqlite::{Connection, params};

use crate::schema;

/// Insert or update an L1 memory record.
/// port of VectorStore.upsertL1Metadata() + upsertL1Vector() in sqlite.ts
pub fn upsert_l1(
    conn: &Connection,
    record: &aeon_memory_core::types::L1RecordRow,
    embedding: Option<&[f32]>,
) -> Result<bool, rusqlite::Error> {
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        let rows = conn.execute(
        "INSERT INTO l1_records (record_id, content, type, priority, scene_name, session_key, session_id,
            timestamp_str, timestamp_start, timestamp_end, created_time, updated_time, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(record_id) DO UPDATE SET
            content=excluded.content,
            type=excluded.type,
            priority=excluded.priority,
            scene_name=excluded.scene_name,
            timestamp_str=excluded.timestamp_str,
            timestamp_start=excluded.timestamp_start,
            timestamp_end=excluded.timestamp_end,
            updated_time=excluded.updated_time,
            metadata_json=excluded.metadata_json",
        params![
            record.record_id,
            record.content,
            record.r#type,
            record.priority,
            record.scene_name,
            record.session_key,
            record.session_id,
            record.timestamp_str,
            record.timestamp_start,
            record.timestamp_end,
            record.created_time,
            record.updated_time,
            record.metadata_json,
        ],
    )?;

        // FTS5 insert
        if schema::is_fts_available(conn) {
            let indexed_content = aeon_memory_core::fts_query::tokenize_for_fts(&record.content);
            conn.execute(
            "INSERT OR REPLACE INTO l1_fts (content, content_original, record_id, type, priority, scene_name,
                session_key, session_id, timestamp_str, timestamp_start, timestamp_end, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                indexed_content,
                record.content,
                record.record_id,
                record.r#type,
                record.priority,
                record.scene_name,
                record.session_key,
                record.session_id,
                record.timestamp_str,
                record.timestamp_start,
                record.timestamp_end,
                record.metadata_json,
            ],
        )?;
        }

        if let Some(embedding) =
            embedding.filter(|embedding| embedding.iter().any(|value| *value != 0.0))
        {
            let vec_str = serde_json::to_string(embedding)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            // vec0 does not support INSERT OR REPLACE / ON CONFLICT.  Keep the
            // metadata and vector replacement atomic, exactly like sqlite.ts.
            conn.execute(
                "DELETE FROM l1_vec WHERE record_id = ?1",
                params![record.record_id],
            )?;
            conn.execute(
                "INSERT INTO l1_vec (record_id, embedding, updated_time) VALUES (?1, ?2, ?3)",
                params![record.record_id, vec_str, record.updated_time],
            )?;
        }

        Ok(rows > 0)
    })();
    match result {
        Ok(written) => {
            conn.execute_batch("COMMIT")?;
            Ok(written)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Delete an L1 record by ID.
pub fn delete_l1(conn: &Connection, record_id: &str) -> Result<bool, rusqlite::Error> {
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        let rows = conn.execute(
            "DELETE FROM l1_records WHERE record_id = ?1",
            params![record_id],
        )?;
        if schema::table_row_exists(conn, "l1_vec") {
            conn.execute(
                "DELETE FROM l1_vec WHERE record_id = ?1",
                params![record_id],
            )?;
        }
        let _ = conn.execute(
            "DELETE FROM l1_fts WHERE record_id = ?1",
            params![record_id],
        );
        Ok(rows > 0)
    })();
    match result {
        Ok(deleted) => {
            conn.execute_batch("COMMIT")?;
            Ok(deleted)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Count L1 records.
pub fn count_l1(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("SELECT COUNT(*) FROM l1_records", [], |row| row.get(0))
}

pub fn delete_l1_expired(conn: &Connection, cutoff: &str) -> Result<i64, rusqlite::Error> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM l1_records", [], |row| row.get(0))?;
    let expired: i64 = conn.query_row(
        "SELECT COUNT(*) FROM l1_records WHERE updated_time != '' AND updated_time < ?1",
        [cutoff],
        |row| row.get(0),
    )?;
    if total > 0 && expired.saturating_mul(10) > total.saturating_mul(8) {
        return Ok(0);
    }
    let mut stmt = conn.prepare(
        "SELECT record_id FROM l1_records WHERE updated_time != '' AND updated_time < ?1",
    )?;
    let ids = stmt
        .query_map([cutoff], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for id in &ids {
        delete_l1(conn, id)?;
    }
    Ok(ids.len() as i64)
}

/// Search L1 records by FTS5 query.
/// port of VectorStore.searchL1Fts() in sqlite.ts
pub fn search_l1_fts(
    conn: &Connection,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<aeon_memory_core::types::L1FtsResult>, rusqlite::Error> {
    // Check if FTS is available
    if !schema::is_fts_available(conn) {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT record_id, content_original AS content, type, priority, scene_name,
                session_key, session_id, timestamp_str, timestamp_start, timestamp_end,
                metadata_json,
                bm25(l1_fts) AS rank
         FROM l1_fts
         WHERE l1_fts MATCH ?1
         ORDER BY rank ASC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(params![fts_query, limit], |row| {
        // BM25 rank: negative = more relevant. Convert to 0-1 score.
        let rank: f64 = row.get(11)?;
        let score = if rank < 0.0 {
            let relevance = -rank;
            relevance / (1.0 + relevance)
        } else {
            1.0 / (1.0 + rank)
        };

        Ok(aeon_memory_core::types::L1FtsResult {
            record_id: row.get(0)?,
            content: row.get(1)?,
            r#type: row.get(2)?,
            priority: row.get(3)?,
            scene_name: row.get(4)?,
            score,
            session_key: row.get(5)?,
            session_id: row.get(6)?,
            timestamp_str: row.get(7)?,
            timestamp_start: row.get(8)?,
            timestamp_end: row.get(9)?,
            metadata_json: row.get(10)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Search L1 records by vector similarity (KNN).
/// Requires sqlite-vec extension to be loaded.
pub fn search_l1_vector(
    conn: &Connection,
    query_embedding: &[f32],
    top_k: i64,
) -> Result<Vec<aeon_memory_core::types::L1SearchResult>, rusqlite::Error> {
    // Check if l1_vec table exists
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='l1_vec'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !table_exists {
        return Ok(Vec::new());
    }

    // Vec0 accepts query as JSON array string
    let query_str: String =
        serde_json::to_string(&query_embedding).unwrap_or_else(|_| "[]".to_string());

    let mut stmt = conn.prepare(
        "SELECT l1_vec.record_id, l1_vec.distance, l1_records.content, l1_records.type,
                l1_records.priority, l1_records.scene_name, l1_records.session_key, l1_records.session_id,
                l1_records.timestamp_str, l1_records.timestamp_start, l1_records.timestamp_end,
                l1_records.metadata_json
         FROM l1_vec
         JOIN l1_records ON l1_vec.record_id = l1_records.record_id
         WHERE embedding MATCH ?1 AND k = ?2
         ORDER BY distance
         "
    )?;

    let retrieve_count = top_k.saturating_add(10);
    let rows = stmt.query_map(params![query_str, retrieve_count], |row| {
        let distance: Option<f64> = row.get(1)?;
        let score = distance.map_or(0.0, |distance| 1.0 - distance);
        Ok((
            distance,
            aeon_memory_core::types::L1SearchResult {
                record_id: row.get(0)?,
                content: row.get(2)?,
                r#type: row.get(3)?,
                priority: row.get(4)?,
                scene_name: row.get(5)?,
                score,
                session_key: row.get(6)?,
                session_id: row.get(7)?,
                timestamp_str: row.get(8)?,
                timestamp_start: row.get(9)?,
                timestamp_end: row.get(10)?,
                metadata_json: row.get(11)?,
            },
        ))
    })?;

    let mut result = Vec::new();
    for row in rows {
        let (distance, row) = row?;
        if distance.is_some_and(|distance| !distance.is_nan()) {
            result.push(row);
            if result.len() >= top_k.max(0) as usize {
                break;
            }
        }
    }
    Ok(result)
}

/// Query L1 records with optional filters.
/// port of VectorStore.queryMemoryRecords() in sqlite.ts
pub fn query_l1_records(
    conn: &Connection,
    filter: &aeon_memory_core::types::L1QueryFilter,
) -> Result<Vec<aeon_memory_core::types::L1RecordRow>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT record_id, content, type, priority, scene_name, session_key, session_id,
                timestamp_str, timestamp_start, timestamp_end, created_time, updated_time, metadata_json
         FROM l1_records WHERE 1=1",
    );
    let mut param_values: Vec<String> = Vec::new();

    if let Some(ref sk) = filter.session_key {
        sql.push_str(" AND session_key = ?");
        param_values.push(sk.clone());
    }
    if let Some(ref sid) = filter.session_id {
        sql.push_str(" AND session_id = ?");
        param_values.push(sid.clone());
    }
    if let Some(ref ua) = filter.updated_after {
        sql.push_str(" AND updated_time > ?");
        param_values.push(ua.clone());
    }

    sql.push_str(" ORDER BY updated_time DESC");

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(|v| v as &dyn rusqlite::types::ToSql)
        .collect();

    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(aeon_memory_core::types::L1RecordRow {
            record_id: row.get(0)?,
            content: row.get(1)?,
            r#type: row.get(2)?,
            priority: row.get(3)?,
            scene_name: row.get(4)?,
            session_key: row.get(5)?,
            session_id: row.get(6)?,
            timestamp_str: row.get(7)?,
            timestamp_start: row.get(8)?,
            timestamp_end: row.get(9)?,
            created_time: row.get(10)?,
            updated_time: row.get(11)?,
            metadata_json: row.get(12)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::StoreConnection;

    fn setup_store(name: &str) -> (StoreConnection, String) {
        let dir = crate::test_support::unique_dir("aeon-memory-test-l1");
        let path = dir.join(name).to_string_lossy().to_string();
        let store = StoreConnection::open(&path).unwrap();
        let c = store.conn.lock().unwrap();
        schema::initialize_schema(&c, None, 0).unwrap();
        drop(c);
        (store, path)
    }

    fn cleanup(path: &str) {
        crate::test_support::cleanup_db(path);
    }

    fn test_record(id: &str) -> aeon_memory_core::types::L1RecordRow {
        aeon_memory_core::types::L1RecordRow {
            record_id: id.to_string(),
            content: format!("Test memory {}", id),
            r#type: "persona".to_string(),
            priority: 50.0,
            scene_name: "test-scene".to_string(),
            session_key: "session-test".to_string(),
            session_id: "".to_string(),
            timestamp_str: "2026-07-13".to_string(),
            timestamp_start: "".to_string(),
            timestamp_end: "".to_string(),
            created_time: "2026-07-13T00:00:00Z".to_string(),
            updated_time: "2026-07-13T00:00:00Z".to_string(),
            metadata_json: "{}".to_string(),
        }
    }

    #[test]
    fn test_insert_and_count_l1() {
        let (store, path) = setup_store("test_l1_insert.db");
        let c = store.conn.lock().unwrap();
        let record = test_record("l1_test_1");
        assert!(upsert_l1(&c, &record, None).unwrap());
        assert_eq!(count_l1(&c).unwrap(), 1);
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn fractional_priority_round_trips_without_loss() {
        let (store, path) = setup_store("test_l1_fractional_priority.db");
        let c = store.conn.lock().unwrap();
        let mut record = test_record("l1_fractional_priority");
        record.priority = 70.5;
        assert!(upsert_l1(&c, &record, None).unwrap());

        let rows = query_l1_records(
            &c,
            &aeon_memory_core::types::L1QueryFilter {
                session_key: Some("session-test".to_string()),
                session_id: None,
                updated_after: None,
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].priority, 70.5);

        let (stored_priority, storage_class): (f64, String) = c
            .query_row(
                "SELECT priority, typeof(priority) FROM l1_records WHERE record_id = ?1",
                ["l1_fractional_priority"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_priority, 70.5);
        assert_eq!(storage_class, "real");

        if schema::is_fts_available(&c) {
            let fts_priority: f64 = c
                .query_row(
                    "SELECT priority FROM l1_fts WHERE record_id = ?1",
                    ["l1_fractional_priority"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(fts_priority, 70.5);
        }
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn chinese_fts_uses_tokenized_index_and_returns_original_content() {
        let (store, path) = setup_store("test_l1_chinese_fts.db");
        let c = store.conn.lock().unwrap();
        let original = "用户希望优化数据库查询性能";
        let mut record = test_record("l1_zh");
        record.content = original.to_string();
        upsert_l1(&c, &record, None).unwrap();
        let query = aeon_memory_core::fts_query::build_fts_query("数据库查询优化").unwrap();
        let rows = search_l1_fts(&c, &query, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, original);
        let (indexed, persisted_original): (String, String) = c
            .query_row(
                "SELECT content, content_original FROM l1_fts WHERE record_id = ?1",
                ["l1_zh"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_ne!(indexed, persisted_original);
        assert_eq!(persisted_original, original);
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_upsert_updates_existing() {
        let (store, path) = setup_store("test_l1_upsert.db");
        let c = store.conn.lock().unwrap();
        let mut record = test_record("l1_upsert");
        upsert_l1(&c, &record, None).unwrap();
        assert_eq!(count_l1(&c).unwrap(), 1);
        record.content = "Updated content".to_string();
        record.updated_time = "2026-07-13T01:00:00Z".to_string();
        upsert_l1(&c, &record, None).unwrap();
        assert_eq!(count_l1(&c).unwrap(), 1);
        let filter = aeon_memory_core::types::L1QueryFilter {
            session_key: Some("session-test".to_string()),
            session_id: None,
            updated_after: None,
        };
        let rows = query_l1_records(&c, &filter).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "Updated content");
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_delete_l1() {
        let (store, path) = setup_store("test_l1_delete.db");
        let c = store.conn.lock().unwrap();
        let record = test_record("l1_del");
        upsert_l1(&c, &record, None).unwrap();
        assert_eq!(count_l1(&c).unwrap(), 1);
        delete_l1(&c, "l1_del").unwrap();
        assert_eq!(count_l1(&c).unwrap(), 0);
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_query_by_session_key() {
        let (store, path) = setup_store("test_l1_query_sk.db");
        let c = store.conn.lock().unwrap();
        let mut r1 = test_record("l1_q_1");
        r1.session_key = "sk-a".to_string();
        let mut r2 = test_record("l1_q_2");
        r2.session_key = "sk-a".to_string();
        r2.updated_time = "2026-07-13T01:00:00Z".to_string();
        let mut r3 = test_record("l1_q_3");
        r3.session_key = "sk-b".to_string();
        upsert_l1(&c, &r1, None).unwrap();
        upsert_l1(&c, &r2, None).unwrap();
        upsert_l1(&c, &r3, None).unwrap();
        let filter = aeon_memory_core::types::L1QueryFilter {
            session_key: Some("sk-a".to_string()),
            session_id: None,
            updated_after: None,
        };
        let rows = query_l1_records(&c, &filter).unwrap();
        assert_eq!(rows.len(), 2);
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_query_updated_after() {
        let (store, path) = setup_store("test_l1_updated_after.db");
        let c = store.conn.lock().unwrap();
        let mut r1 = test_record("l1_ua_1");
        r1.session_key = "sk".to_string();
        r1.updated_time = "2026-07-13T00:00:00Z".to_string();
        let mut r2 = test_record("l1_ua_2");
        r2.session_key = "sk".to_string();
        r2.updated_time = "2026-07-13T02:00:00Z".to_string();
        upsert_l1(&c, &r1, None).unwrap();
        upsert_l1(&c, &r2, None).unwrap();
        let filter = aeon_memory_core::types::L1QueryFilter {
            session_key: Some("sk".to_string()),
            session_id: None,
            updated_after: Some("2026-07-13T01:00:00Z".to_string()),
        };
        let rows = query_l1_records(&c, &filter).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].record_id, "l1_ua_2");
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_fts_search() {
        let (store, path) = setup_store("test_l1_fts.db");
        let c = store.conn.lock().unwrap();
        let mut r1 = test_record("l1_fts_1");
        r1.content = "user likes programming and machine learning".to_string();
        r1.session_key = "sk".to_string();
        let mut r2 = test_record("l1_fts_2");
        r2.content = "user likes travel and photography".to_string();
        r2.session_key = "sk".to_string();
        upsert_l1(&c, &r1, None).unwrap();
        upsert_l1(&c, &r2, None).unwrap();
        let results = search_l1_fts(&c, "\"programming\"", 10).unwrap();
        assert_eq!(results.len(), 1, "Should find exactly 1 FTS result");
        assert_eq!(results[0].record_id, "l1_fts_1");
        let results = search_l1_fts(&c, "\"user\"", 10).unwrap();
        assert_eq!(results.len(), 2, "Should find 2 FTS results for 'user'");
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_multiple_types() {
        let (store, path) = setup_store("test_l1_types.db");
        let c = store.conn.lock().unwrap();
        for i in 0..3 {
            let types = ["persona", "episodic", "instruction"];
            let mut r = test_record(&format!("l1_type_{}", i));
            r.r#type = types[i].to_string();
            r.session_key = "sk".to_string();
            r.updated_time = format!("2026-07-13T00:00:0{}Z", i);
            upsert_l1(&c, &r, None).unwrap();
        }
        assert_eq!(count_l1(&c).unwrap(), 3);
        drop(c);
        cleanup(&path);
    }
}
