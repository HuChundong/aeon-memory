// port of src/core/record/l1-writer.ts

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

static MEMORY_ID_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// Three memory types (port of MemoryType from l1-writer.ts:32)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MemoryType {
    #[serde(rename = "persona")]
    Persona,
    #[serde(rename = "episodic")]
    Episodic,
    #[serde(rename = "instruction")]
    Instruction,
}

/// A persisted memory record in L1 JSONL files (port of MemoryRecord from l1-writer.ts:49-72)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub content: String,
    pub r#type: String,
    #[serde(serialize_with = "crate::types::serialize_js_number")]
    pub priority: f64,
    pub scene_name: String,
    pub source_message_ids: Vec<String>,
    pub metadata: serde_json::Value,
    pub timestamps: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "sessionKey")]
    pub session_key: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// Extracted memory from LLM output (port of ExtractedMemory)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtractedMemory {
    pub content: String,
    pub r#type: String,
    #[serde(serialize_with = "crate::types::serialize_js_number")]
    pub priority: f64,
    pub source_message_ids: Vec<String>,
    pub metadata: serde_json::Value,
}

/// Dedup decision from LLM (port of DedupDecision)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DedupDecision {
    pub decision: String,
    pub reason: String,
    #[serde(default)]
    pub merged_content: Option<String>,
}

/// Generate a unique memory ID (port of generateMemoryId)
pub fn generate_memory_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // The TypeScript implementation uses a random suffix. A process-local
    // sequence gives the same wire format while also guaranteeing uniqueness
    // for multiple records created inside the same clock tick.
    let suffix = MEMORY_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("m_{}_{:08x}", ms, suffix)
}

/// L1 JSONL file path for today's shard
pub fn l1_jsonl_path(base_dir: &str) -> std::path::PathBuf {
    let date = crate::utils::time::local_date_for_filename();
    Path::new(base_dir)
        .join("records")
        .join(format!("{}.jsonl", date))
}

/// Write a memory record to L1 JSONL (append) + optionally to VectorStore.
/// port of writeMemory() from l1-writer.ts
pub fn write_memory(
    base_dir: &str,
    record: &MemoryRecord,
    vector_store: &mut dyn crate::types::IMemoryStore,
    embedding_service: &dyn crate::types::EmbeddingService,
) -> AeonMemoryResult<()> {
    // TS treats embedding/vector persistence as a best-effort secondary
    // index. JSONL remains the source of truth, and disabled embedding is
    // represented by an empty vector which must not be sent to vec0.
    let embedding = embedding_service
        .embed(&record.content)
        .ok()
        .filter(|value| !value.is_empty());
    let out_dir = Path::new(base_dir).join("records");
    std::fs::create_dir_all(&out_dir).map_err(AeonMemoryCoreError::Io)?;
    let path = out_dir.join(format!(
        "{}.jsonl",
        crate::utils::time::local_date_for_filename()
    ));
    let line = serde_json::to_string(record).map_err(AeonMemoryCoreError::Json)?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(AeonMemoryCoreError::Io)?;
    writeln!(file, "{}", line).map_err(AeonMemoryCoreError::Io)?;

    // Match the TS SQLite adapter: timestamp_str is the first trail entry,
    // while timestamp_start/end are the lexical min/max of the whole trail.
    // Episodic metadata remains metadata and does not replace this trail.
    let timestamp_str = record.timestamps.first().cloned().unwrap_or_default();
    let timestamp_start = record
        .timestamps
        .iter()
        .min()
        .cloned()
        .unwrap_or_else(|| timestamp_str.clone());
    let timestamp_end = record
        .timestamps
        .iter()
        .max()
        .cloned()
        .unwrap_or_else(|| timestamp_str.clone());
    let l1_row = crate::types::L1RecordRow {
        record_id: record.id.clone(),
        content: record.content.clone(),
        r#type: record.r#type.clone(),
        priority: record.priority,
        scene_name: record.scene_name.clone(),
        session_key: record.session_key.clone(),
        session_id: record.session_id.clone(),
        timestamp_str,
        timestamp_start,
        timestamp_end,
        created_time: record.created_at.clone(),
        updated_time: record.updated_at.clone(),
        metadata_json: record.metadata.to_string(),
    };
    let _ = vector_store.upsert_l1(&l1_row, embedding.as_deref());
    Ok(())
}

/// Read all L1 memory records from a session's JSONL files.
/// port of readMemoryRecords() from l1-reader.ts
/// Write an updated memory record (for update/merge dedup actions).
/// port of writeMemoryUpdate() from l1-writer.ts
pub fn write_memory_update(
    base_dir: &str,
    record_id: &str,
    new_content: &str,
    vector_store: &mut dyn crate::types::IMemoryStore,
    embedding_service: &dyn crate::types::EmbeddingService,
) -> AeonMemoryResult<()> {
    // Query existing record to preserve fields
    let existing_rows = vector_store.query_l1_records(&crate::types::L1QueryFilter {
        session_key: None,
        session_id: None,
        updated_after: None,
    })?;
    let existing = existing_rows.into_iter().find(|r| r.record_id == record_id);

    let now = crate::utils::time::now_instant_iso();
    let row = match existing {
        Some(e) => crate::types::L1RecordRow {
            content: new_content.to_string(),
            updated_time: now.clone(),
            ..e
        },
        None => {
            return Err(AeonMemoryCoreError::Store(format!(
                "cannot update missing L1 record {record_id}"
            )));
        }
    };
    let metadata = serde_json::from_str(&row.metadata_json).unwrap_or_default();
    let record = MemoryRecord {
        id: row.record_id.clone(),
        content: row.content.clone(),
        r#type: row.r#type.clone(),
        priority: row.priority,
        scene_name: row.scene_name.clone(),
        source_message_ids: vec![],
        metadata,
        timestamps: vec![row.timestamp_str.clone()],
        created_at: row.created_time.clone(),
        updated_at: row.updated_time.clone(),
        session_key: row.session_key.clone(),
        session_id: row.session_id.clone(),
    };
    let embedding = embedding_service
        .embed(new_content)
        .ok()
        .filter(|value| !value.is_empty());
    let out_dir = Path::new(base_dir).join("records");
    std::fs::create_dir_all(&out_dir).map_err(AeonMemoryCoreError::Io)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(l1_jsonl_path(base_dir))
        .map_err(AeonMemoryCoreError::Io)?;
    use std::io::Write;
    writeln!(file, "{}", serde_json::to_string(&record)?).map_err(AeonMemoryCoreError::Io)?;
    let _ = vector_store.upsert_l1(&row, embedding.as_deref());
    Ok(())
}

pub fn read_memory_records(
    session_key: &str,
    base_dir: &str,
) -> AeonMemoryResult<Vec<MemoryRecord>> {
    let rec_dir = Path::new(base_dir).join("records");
    if !rec_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&rec_dir)
        .map_err(AeonMemoryCoreError::Io)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".jsonl"))
        .collect();
    entries.sort();

    let mut records = Vec::new();
    for file_name in entries {
        let file_path = rec_dir.join(&file_name);
        let raw = match std::fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let rec: MemoryRecord = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if rec.session_key == session_key {
                records.push(rec);
            }
        }
    }
    records.sort_by_key(|r| r.created_at.clone());
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::l1_dedup::{RecordingMockEmbedding, RecordingMockStore};

    #[test]
    fn test_generate_memory_id() {
        let id1 = generate_memory_id();
        let id2 = generate_memory_id();
        assert!(id1.starts_with("m_"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_write_and_read() {
        let dir = std::env::temp_dir()
            .join("aeon-memory-test-l1-writer")
            .join("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("records")).unwrap();

        let rec = MemoryRecord {
            id: "mem_test_1".into(),
            content: "test memory".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "test-scene".into(),
            source_message_ids: vec!["msg1".into()],
            metadata: serde_json::json!({}),
            timestamps: vec![
                "2026-07-13T12:00:00Z".into(),
                "2026-07-12T12:00:00Z".into(),
                "2026-07-14T12:00:00Z".into(),
            ],
            created_at: "2026-07-13T00:00:00Z".into(),
            updated_at: "2026-07-13T00:00:01Z".into(),
            session_key: "sk-1".into(),
            session_id: "".into(),
        };

        let mut store = RecordingMockStore::new();
        let embedding = RecordingMockEmbedding::new(vec![1.0, 0.0]);
        write_memory(dir.to_str().unwrap(), &rec, &mut store, &embedding).unwrap();

        let records = read_memory_records("sk-1", dir.to_str().unwrap()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].content, "test memory");
        assert_eq!(records[0].r#type, "persona");
        let calls = store.upsert_l1_calls.lock().unwrap();
        assert_eq!(calls[0].timestamp_str, "2026-07-13T12:00:00Z");
        assert_eq!(calls[0].timestamp_start, "2026-07-12T12:00:00Z");
        assert_eq!(calls[0].timestamp_end, "2026-07-14T12:00:00Z");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_jsonl_format() {
        let dir = std::env::temp_dir()
            .join("aeon-memory-test-l1-format")
            .join("fmt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("records")).unwrap();

        let rec = MemoryRecord {
            id: "mem_fmt_1".into(),
            content: "format test".into(),
            r#type: "instruction".into(),
            priority: -1.0,
            scene_name: String::new(),
            source_message_ids: vec![],
            metadata: serde_json::json!({"source": "test"}),
            timestamps: vec![],
            created_at: "2026-07-13T00:00:00Z".into(),
            updated_at: "2026-07-13T00:00:01Z".into(),
            session_key: "sk".into(),
            session_id: "".into(),
        };

        let mut store = RecordingMockStore::new();
        let embedding = RecordingMockEmbedding::new(vec![1.0, 0.0]);
        write_memory(dir.to_str().unwrap(), &rec, &mut store, &embedding).unwrap();

        // Read raw JSONL and verify fields
        let date = crate::utils::time::local_date_for_filename();
        let path = dir.join("records").join(format!("{}.jsonl", date));
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(parsed["id"], "mem_fmt_1");
        assert_eq!(parsed["type"], "instruction");
        assert_eq!(parsed["priority"], -1);
        assert_eq!(parsed["metadata"]["source"], "test");
        // TS schema intentionally mixes snake_case memory fields with
        // camelCase lifecycle/session fields.
        assert!(
            parsed.get("sessionKey").is_some(),
            "Should use camelCase sessionKey"
        );
        assert!(
            parsed.get("source_message_ids").is_some(),
            "Should preserve snake_case source_message_ids"
        );
        assert!(parsed.get("scene_name").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
