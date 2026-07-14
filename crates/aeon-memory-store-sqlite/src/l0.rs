// port of src/core/store/sqlite.ts L0 operations (l0_conversations + l0_vec + l0_fts)

use rusqlite::{Connection, params};

use crate::schema;

/// Insert or update an L0 conversation record (metadata + optional embedding).
/// When embedding is Some, also upserts into l0_vec.
pub fn upsert_l0(
    conn: &rusqlite::Connection,
    record: &aeon_memory_core::types::L0Record,
    embedding: Option<&[f32]>,
) -> Result<bool, rusqlite::Error> {
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        let rows = conn.execute(
        "INSERT INTO l0_conversations (record_id, session_key, session_id, role, message_text, recorded_at, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(record_id) DO UPDATE SET
            message_text = excluded.message_text,
            recorded_at = excluded.recorded_at,
            timestamp = excluded.timestamp",
        params![
            record.id,
            record.session_key,
            record.session_id,
            record.role,
            record.message_text,
            record.recorded_at,
            record.timestamp,
        ],
    )?;

        // FTS5 insert
        if schema::is_fts_available(conn) {
            let indexed_text = aeon_memory_core::fts_query::tokenize_for_fts(&record.message_text);
            conn.execute(
            "INSERT OR REPLACE INTO l0_fts (message_text, message_text_original, record_id, session_key, session_id, role, recorded_at, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                indexed_text,
                record.message_text,
                record.id,
                record.session_key,
                record.session_id,
                record.role,
                record.recorded_at,
                record.timestamp,
            ],
        )?;
        }

        // Vec0 insert (when embedding is provided and l0_vec table exists)
        if let Some(emb) = embedding.filter(|embedding| embedding.iter().any(|value| *value != 0.0))
        {
            let table_ok: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='l0_vec'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);
            if table_ok {
                let vec_str = serde_json::to_string(emb)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                // vec0 does not implement SQLite's conflict/upsert clauses.
                // Match the TypeScript store by replacing a vector explicitly.
                conn.execute(
                    "DELETE FROM l0_vec WHERE record_id = ?1",
                    params![record.id],
                )?;
                conn.execute(
                    "INSERT INTO l0_vec (record_id, embedding, recorded_at) VALUES (?1, ?2, ?3)",
                    params![record.id, vec_str, record.recorded_at],
                )?;
            }
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

/// Attach a deferred embedding to an existing L0 row without rewriting its
/// metadata. Returns false when the record no longer exists.
pub fn update_l0_embedding(
    conn: &Connection,
    record_id: &str,
    embedding: &[f32],
) -> Result<bool, rusqlite::Error> {
    if embedding.iter().all(|value| *value == 0.0) {
        return Ok(false);
    }
    let recorded_at = conn.query_row(
        "SELECT recorded_at FROM l0_conversations WHERE record_id = ?1",
        params![record_id],
        |row| row.get::<_, String>(0),
    );
    let recorded_at = match recorded_at {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(false),
        Err(error) => return Err(error),
    };
    let vec_str = serde_json::to_string(embedding)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        conn.execute(
            "DELETE FROM l0_vec WHERE record_id = ?1",
            params![record_id],
        )?;
        conn.execute(
            "INSERT INTO l0_vec (record_id, embedding, recorded_at) VALUES (?1, ?2, ?3)",
            params![record_id, vec_str, recorded_at],
        )?;
        Ok(true)
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

pub fn search_l0_fts(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<aeon_memory_core::types::L0FtsResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT record_id, session_key, session_id, role, message_text_original,
                recorded_at, timestamp, bm25(l0_fts) AS rank
         FROM l0_fts WHERE l0_fts MATCH ?1 ORDER BY rank ASC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![query, limit], |row| {
        let rank: f64 = row.get(7)?;
        let relevance = if rank < 0.0 {
            -rank
        } else {
            1.0 / (1.0 + rank)
        };
        Ok(aeon_memory_core::types::L0FtsResult {
            record_id: row.get(0)?,
            session_key: row.get(1)?,
            session_id: row.get(2)?,
            role: row.get(3)?,
            message_text: row.get(4)?,
            score: relevance / (1.0 + relevance),
            recorded_at: row.get(5)?,
            timestamp: row.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn search_l0_vector(
    conn: &Connection,
    embedding: &[f32],
    limit: i64,
) -> Result<Vec<aeon_memory_core::types::L0SearchResult>, rusqlite::Error> {
    let query = serde_json::to_string(embedding)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let mut stmt = conn.prepare(
        "SELECT v.record_id, c.session_key, c.session_id, c.role, c.message_text,
                v.distance, c.recorded_at, c.timestamp
         FROM l0_vec v JOIN l0_conversations c ON c.record_id = v.record_id
         WHERE v.embedding MATCH ?1 AND k = ?2 ORDER BY v.distance",
    )?;
    let retrieve_count = limit.saturating_add(10);
    let rows = stmt.query_map(params![query, retrieve_count], |row| {
        let distance: Option<f64> = row.get(5)?;
        Ok((
            distance,
            aeon_memory_core::types::L0SearchResult {
                record_id: row.get(0)?,
                session_key: row.get(1)?,
                session_id: row.get(2)?,
                role: row.get(3)?,
                message_text: row.get(4)?,
                score: distance.map_or(0.0, |distance| 1.0 - distance),
                recorded_at: row.get(6)?,
                timestamp: row.get(7)?,
            },
        ))
    })?;
    let mut results = Vec::new();
    for row in rows {
        let (distance, result) = row?;
        if distance.is_some_and(|distance| !distance.is_nan()) {
            results.push(result);
            if results.len() >= limit.max(0) as usize {
                break;
            }
        }
    }
    Ok(results)
}

/// Delete an L0 record by ID.
pub fn delete_l0(conn: &Connection, record_id: &str) -> Result<bool, rusqlite::Error> {
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        let rows = conn.execute(
            "DELETE FROM l0_conversations WHERE record_id = ?1",
            params![record_id],
        )?;
        if schema::table_row_exists(conn, "l0_vec") {
            conn.execute(
                "DELETE FROM l0_vec WHERE record_id = ?1",
                params![record_id],
            )?;
        }
        // FTS is a best-effort secondary index in the TypeScript store.
        let _ = conn.execute(
            "DELETE FROM l0_fts WHERE record_id = ?1",
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

/// Count L0 records.
pub fn count_l0(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("SELECT COUNT(*) FROM l0_conversations", [], |row| {
        row.get(0)
    })
}

pub fn delete_l0_expired(conn: &Connection, cutoff: &str) -> Result<i64, rusqlite::Error> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM l0_conversations", [], |row| {
        row.get(0)
    })?;
    let expired: i64 = conn.query_row(
        "SELECT COUNT(*) FROM l0_conversations WHERE recorded_at != '' AND recorded_at < ?1",
        [cutoff],
        |row| row.get(0),
    )?;
    // TypeScript refuses a cleanup pass that would remove more than 80% of
    // the layer, independently of the cleaner's minimum-count guard.
    if total > 0 && expired.saturating_mul(10) > total.saturating_mul(8) {
        return Ok(0);
    }
    let mut stmt = conn.prepare(
        "SELECT record_id FROM l0_conversations WHERE recorded_at != '' AND recorded_at < ?1",
    )?;
    let ids = stmt
        .query_map([cutoff], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for id in &ids {
        delete_l0(conn, id)?;
    }
    Ok(ids.len() as i64)
}

/// Query L0 messages for a session key (newest first, limited).
/// port of stmtL0QueryAll in sqlite.ts
pub fn query_l0_for_session(
    conn: &Connection,
    session_key: &str,
    limit: i64,
) -> Result<Vec<aeon_memory_core::types::L0QueryRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT record_id, session_key, session_id, role, message_text, recorded_at, timestamp
         FROM l0_conversations
         WHERE session_key = ?1
         ORDER BY recorded_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![session_key, limit], |row| {
        Ok(aeon_memory_core::types::L0QueryRow {
            record_id: row.get(0)?,
            session_key: row.get(1)?,
            session_id: row.get(2)?,
            role: row.get(3)?,
            message_text: row.get(4)?,
            recorded_at: row.get(5)?,
            timestamp: row.get(6)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Query L0 messages after a cursor (recorded_at > cursor).
/// port of stmtL0QueryAfter in sqlite.ts
pub fn query_l0_after(
    conn: &Connection,
    session_key: &str,
    after_recorded_at: &str,
    limit: i64,
) -> Result<Vec<aeon_memory_core::types::L0QueryRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT record_id, session_key, session_id, role, message_text, recorded_at, timestamp
         FROM l0_conversations
         WHERE session_key = ?1 AND recorded_at > ?2
         ORDER BY recorded_at DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![session_key, after_recorded_at, limit], |row| {
        Ok(aeon_memory_core::types::L0QueryRow {
            record_id: row.get(0)?,
            session_key: row.get(1)?,
            session_id: row.get(2)?,
            role: row.get(3)?,
            message_text: row.get(4)?,
            recorded_at: row.get(5)?,
            timestamp: row.get(6)?,
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
        let dir = crate::test_support::unique_dir("aeon-memory-test-l0");
        let path = dir.join(name).to_string_lossy().to_string();
        let store = StoreConnection::open(&path).unwrap();
        // Initialize schema (same as VectorStore.init())
        let c = store.conn.lock().unwrap();
        schema::initialize_schema(&c, None, 0).unwrap();
        drop(c);
        (store, path)
    }

    fn cleanup(path: &str) {
        crate::test_support::cleanup_db(path);
    }

    #[test]
    fn test_insert_and_count() {
        let (store, path) = setup_store("test_l0_insert.db");
        let c = store.conn.lock().unwrap();
        let record = aeon_memory_core::types::L0Record {
            id: "l0_test_1".to_string(),
            session_key: "session-1".to_string(),
            session_id: "".to_string(),
            role: "user".to_string(),
            message_text: "Hello world".to_string(),
            recorded_at: "2026-07-13T00:00:00Z".to_string(),
            timestamp: 1781836800000,
        };
        assert!(upsert_l0(&c, &record, None).unwrap());
        assert_eq!(count_l0(&c).unwrap(), 1);
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_insert_and_query() {
        let (store, path) = setup_store("test_l0_query.db");
        let c = store.conn.lock().unwrap();
        let record = aeon_memory_core::types::L0Record {
            id: "l0_test_2".to_string(),
            session_key: "session-2".to_string(),
            session_id: "".to_string(),
            role: "user".to_string(),
            message_text: "Query test message".to_string(),
            recorded_at: "2026-07-13T00:00:01Z".to_string(),
            timestamp: 1781836800001,
        };
        upsert_l0(&c, &record, None).unwrap();
        let rows = query_l0_for_session(&c, "session-2", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_text, "Query test message");
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn chinese_fts_uses_tokenized_index_and_returns_original_text() {
        let (store, path) = setup_store("test_l0_chinese_fts.db");
        let c = store.conn.lock().unwrap();
        let original = "用户希望优化数据库查询性能";
        let record = aeon_memory_core::types::L0Record {
            id: "l0_zh".to_string(),
            session_key: "session-zh".to_string(),
            session_id: "conversation-zh".to_string(),
            role: "user".to_string(),
            message_text: original.to_string(),
            recorded_at: "2026-07-13T00:00:01Z".to_string(),
            timestamp: 1_781_836_800_001,
        };
        upsert_l0(&c, &record, None).unwrap();
        let query = aeon_memory_core::fts_query::build_fts_query("数据库查询优化").unwrap();
        let rows = search_l0_fts(&c, &query, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_text, original);
        let (indexed, persisted_original): (String, String) = c
            .query_row(
                "SELECT message_text, message_text_original FROM l0_fts WHERE record_id = ?1",
                ["l0_zh"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_ne!(indexed, persisted_original);
        assert_eq!(persisted_original, original);
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_query_after_cursor() {
        let (store, path) = setup_store("test_l0_cursor.db");
        let c = store.conn.lock().unwrap();
        let r1 = aeon_memory_core::types::L0Record {
            id: "l0_c1".to_string(),
            session_key: "s1".to_string(),
            session_id: "".to_string(),
            role: "user".to_string(),
            message_text: "first".to_string(),
            recorded_at: "2026-07-13T00:00:01Z".to_string(),
            timestamp: 1000,
        };
        let r2 = aeon_memory_core::types::L0Record {
            id: "l0_c2".to_string(),
            session_key: "s1".to_string(),
            session_id: "".to_string(),
            role: "assistant".to_string(),
            message_text: "second".to_string(),
            recorded_at: "2026-07-13T00:00:02Z".to_string(),
            timestamp: 2000,
        };
        upsert_l0(&c, &r1, None).unwrap();
        upsert_l0(&c, &r2, None).unwrap();
        let rows = query_l0_after(&c, "s1", "2026-07-13T00:00:01Z", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_text, "second");
        drop(c);
        cleanup(&path);
    }

    #[test]
    fn test_delete() {
        let (store, path) = setup_store("test_l0_delete.db");
        let c = store.conn.lock().unwrap();
        let record = aeon_memory_core::types::L0Record {
            id: "l0_del".to_string(),
            session_key: "s-del".to_string(),
            session_id: "".to_string(),
            role: "user".to_string(),
            message_text: "to delete".to_string(),
            recorded_at: "2026-07-13T00:00:00Z".to_string(),
            timestamp: 0,
        };
        upsert_l0(&c, &record, None).unwrap();
        assert_eq!(count_l0(&c).unwrap(), 1);
        delete_l0(&c, "l0_del").unwrap();
        assert_eq!(count_l0(&c).unwrap(), 0);
        drop(c);
        cleanup(&path);
    }
}
