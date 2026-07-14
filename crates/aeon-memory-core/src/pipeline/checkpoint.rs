// port of src/utils/checkpoint.ts

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static CHECKPOINT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Per-session state managed by L0/L1 runners.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunnerSessionState {
    pub last_captured_timestamp: i64,
    pub last_l1_cursor: i64,
    pub last_scene_name: String,
}

/// Per-session state managed by PipelineManager.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PipelineSessionState {
    pub conversation_count: u32,
    pub last_extraction_time: String,
    pub last_extraction_updated_time: String,
    pub last_active_time: i64,
    pub l2_pending_l1_count: u32,
    pub warmup_threshold: u32,
    pub l2_last_extraction_time: String,
}

/// Full checkpoint structure — port of Checkpoint from checkpoint.ts:81-109
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Checkpoint {
    pub last_captured_timestamp: i64,
    pub total_processed: u64,
    pub last_persona_at: i64,
    pub last_persona_time: String,
    pub request_persona_update: bool,
    pub persona_update_reason: String,
    pub memories_since_last_persona: u64,
    pub scenes_processed: u64,
    pub l0_conversations_count: u64,
    pub total_memories_extracted: u64,

    #[serde(default)]
    pub runner_states: std::collections::HashMap<String, RunnerSessionState>,

    #[serde(default)]
    pub pipeline_states: std::collections::HashMap<String, PipelineSessionState>,
}

/// Read checkpoint from file. Returns default if not found.
/// port of CheckpointManager.readRaw() from checkpoint.ts:194-243
pub fn read_checkpoint(data_dir: &str) -> AeonMemoryResult<Checkpoint> {
    let _guard = checkpoint_lock()?;
    read_checkpoint_unlocked(data_dir)
}

fn read_checkpoint_unlocked(data_dir: &str) -> AeonMemoryResult<Checkpoint> {
    let path = checkpoint_path(data_dir);
    if !path.exists() {
        return Ok(Checkpoint::default());
    }
    let raw = std::fs::read_to_string(&path).map_err(AeonMemoryCoreError::Io)?;
    let cp: Checkpoint = serde_json::from_str(&raw).unwrap_or_default();
    Ok(cp)
}

/// Write checkpoint atomically (tmp file + rename).
/// port of CheckpointManager.writeRaw() from checkpoint.ts:246-252
pub fn write_checkpoint(data_dir: &str, cp: &Checkpoint) -> AeonMemoryResult<()> {
    let _guard = checkpoint_lock()?;
    write_checkpoint_unlocked(data_dir, cp)
}

fn write_checkpoint_unlocked(data_dir: &str, cp: &Checkpoint) -> AeonMemoryResult<()> {
    let path = checkpoint_path(data_dir);
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(AeonMemoryCoreError::Io)?;

    let tmp_name = format!(".recall_checkpoint.tmp.{}", rand_hex(4));
    let tmp_path = dir.join(&tmp_name);
    let json = serde_json::to_string_pretty(cp).map_err(AeonMemoryCoreError::Json)?;
    std::fs::write(&tmp_path, json.as_bytes()).map_err(AeonMemoryCoreError::Io)?;
    std::fs::rename(&tmp_path, &path).map_err(AeonMemoryCoreError::Io)?;

    Ok(())
}

/// Update checkpoint via mutating function: read, modify, write (no file lock).
/// The TS version uses a per-file async lock; in this sync Rust version,
/// the caller is responsible for serialization (typically via SerialQueue).
pub fn mutate_checkpoint<F>(data_dir: &str, f: F) -> AeonMemoryResult<Checkpoint>
where
    F: FnOnce(&mut Checkpoint),
{
    mutate_checkpoint_result(data_dir, |cp| {
        f(cp);
        Ok(cp.clone())
    })
}

/// Atomically read, mutate and persist a checkpoint while returning an
/// arbitrary result. This is the Rust equivalent of the TS per-file async
/// checkpoint lock and prevents background pipeline persistence from
/// overwriting a concurrent L0 cursor update.
pub fn mutate_checkpoint_result<T, F>(data_dir: &str, f: F) -> AeonMemoryResult<T>
where
    F: FnOnce(&mut Checkpoint) -> AeonMemoryResult<T>,
{
    checkpoint_transaction(data_dir, |cp| f(cp).map(|result| (result, true)))
}

/// Execute an atomic checkpoint transaction and optionally skip the physical
/// write when the operation made no change.
pub fn checkpoint_transaction<T, F>(data_dir: &str, f: F) -> AeonMemoryResult<T>
where
    F: FnOnce(&mut Checkpoint) -> AeonMemoryResult<(T, bool)>,
{
    let _guard = checkpoint_lock()?;
    let mut cp = read_checkpoint_unlocked(data_dir)?;
    let (result, changed) = f(&mut cp)?;
    if changed {
        write_checkpoint_unlocked(data_dir, &cp)?;
    }
    Ok(result)
}

fn checkpoint_lock() -> AeonMemoryResult<std::sync::MutexGuard<'static, ()>> {
    CHECKPOINT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| AeonMemoryCoreError::Store("checkpoint lock poisoned".into()))
}

fn checkpoint_path(data_dir: &str) -> std::path::PathBuf {
    Path::new(data_dir)
        .join(".metadata")
        .join("recall_checkpoint.json")
}

fn rand_hex(bytes: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:01$x}", seed, bytes * 2)
}

/// Per-session helper: get or create runner state.
pub fn get_runner_state<'a>(
    cp: &'a mut Checkpoint,
    session_key: &str,
) -> &'a mut RunnerSessionState {
    cp.runner_states.entry(session_key.to_string()).or_default()
}

/// Per-session helper: get or create pipeline state.
pub fn get_pipeline_state<'a>(
    cp: &'a mut Checkpoint,
    session_key: &str,
) -> &'a mut PipelineSessionState {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    cp.pipeline_states
        .entry(session_key.to_string())
        .or_insert_with(|| PipelineSessionState {
            last_active_time: now_ms,
            ..PipelineSessionState::default()
        })
}

/// Mark L1 extraction complete in checkpoint.
/// port of markL1ExtractionComplete() from checkpoint.ts:410-432
pub fn mark_l1_extraction_complete(
    data_dir: &str,
    session_key: &str,
    memories_extracted: u64,
    cursor_recorded_at_ms: Option<i64>,
    last_scene_name: Option<&str>,
) -> AeonMemoryResult<Checkpoint> {
    mutate_checkpoint(data_dir, |cp| {
        let state = get_runner_state(cp, session_key);
        if let Some(cursor) = cursor_recorded_at_ms {
            state.last_l1_cursor = cursor;
        }
        if let Some(name) = last_scene_name {
            state.last_scene_name = name.to_string();
        }
        cp.total_memories_extracted += memories_extracted;
        cp.memories_since_last_persona += memories_extracted;
    })
}

/// Merge pipeline states into checkpoint.
/// port of mergePipelineStates() from checkpoint.ts:387-397
pub fn merge_pipeline_states(
    data_dir: &str,
    states: &std::collections::HashMap<String, PipelineSessionState>,
) -> AeonMemoryResult<()> {
    mutate_checkpoint(data_dir, |cp| {
        for (key, p_state) in states {
            cp.pipeline_states.insert(key.clone(), p_state.clone());
        }
    })?;
    Ok(())
}

/// Atomic capture: read cursor, capture, advance.
/// port of captureAtomically() from checkpoint.ts:459-486
pub fn capture_atomically<F>(
    data_dir: &str,
    session_key: &str,
    plugin_start_timestamp: Option<i64>,
    f: F,
) -> AeonMemoryResult<Option<(i64, u32)>>
where
    F: FnOnce(i64) -> Option<(i64, u32)>,
{
    checkpoint_transaction(data_dir, |cp| {
        let state = get_runner_state(cp, session_key);
        let after_ts = if state.last_captured_timestamp == 0 {
            plugin_start_timestamp.unwrap_or(0)
        } else {
            state.last_captured_timestamp
        };

        let result = f(after_ts);
        if let Some((max_ts, count)) = result {
            state.last_captured_timestamp = max_ts;
            cp.last_captured_timestamp = cp.last_captured_timestamp.max(max_ts);
            cp.total_processed += count as u64;
            cp.l0_conversations_count += 1;
        }
        let changed = result.is_some();
        Ok((result, changed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("aeon-memory-test-checkpoint")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".metadata")).unwrap();
        dir
    }

    #[test]
    fn test_read_default() {
        let dir = setup_dir("default");
        let cp = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert_eq!(cp.total_processed, 0);
        assert!(!cp.request_persona_update);
    }

    #[test]
    fn test_write_and_read() {
        let dir = setup_dir("write_read");
        let cp = Checkpoint {
            total_processed: 42,
            scenes_processed: 3,
            ..Checkpoint::default()
        };
        write_checkpoint(dir.to_str().unwrap(), &cp).unwrap();

        let read = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert_eq!(read.total_processed, 42);
        assert_eq!(read.scenes_processed, 3);
    }

    #[test]
    fn test_atomic_write_doesnt_corrupt() {
        let dir = setup_dir("atomic");
        let cp = Checkpoint {
            total_processed: 100,
            ..Checkpoint::default()
        };
        write_checkpoint(dir.to_str().unwrap(), &cp).unwrap();
        let read = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert_eq!(read.total_processed, 100);
    }

    #[test]
    fn test_mutate() {
        let dir = setup_dir("mutate");
        let result = mutate_checkpoint(dir.to_str().unwrap(), |cp| {
            cp.total_processed += 5;
        })
        .unwrap();
        assert_eq!(result.total_processed, 5);

        // Verify persistence
        let read = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert_eq!(read.total_processed, 5);
    }

    #[test]
    fn test_capture_atomically() {
        let dir = setup_dir("capture_atom");

        let result = capture_atomically(
            dir.to_str().unwrap(),
            "session-1",
            Some(1000),
            |_after_ts| Some((3000, 3)),
        )
        .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), (3000, 3));

        // Verify cursor advanced
        let cp = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert_eq!(cp.runner_states["session-1"].last_captured_timestamp, 3000);
        assert_eq!(cp.total_processed, 3);
    }

    #[test]
    fn concurrent_l0_and_pipeline_transactions_preserve_both_fields() {
        let dir = setup_dir("concurrent_transactions");
        let left = dir.clone();
        let right = dir.clone();
        let l0 = std::thread::spawn(move || {
            for value in 1..=50 {
                mutate_checkpoint_result(left.to_str().unwrap(), |cp| {
                    cp.total_processed = value;
                    cp.runner_states
                        .entry("s".into())
                        .or_default()
                        .last_captured_timestamp = value as i64;
                    Ok(())
                })
                .unwrap();
            }
        });
        let pipeline = std::thread::spawn(move || {
            for value in 1..=50 {
                mutate_checkpoint_result(right.to_str().unwrap(), |cp| {
                    cp.pipeline_states
                        .entry("s".into())
                        .or_default()
                        .conversation_count = value;
                    Ok(())
                })
                .unwrap();
            }
        });
        l0.join().unwrap();
        pipeline.join().unwrap();

        let checkpoint = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert_eq!(checkpoint.total_processed, 50);
        assert_eq!(checkpoint.runner_states["s"].last_captured_timestamp, 50);
        assert_eq!(checkpoint.pipeline_states["s"].conversation_count, 50);
    }

    #[test]
    fn test_runner_state_isolation() {
        let _dir = setup_dir("isolation");
        let mut cp = Checkpoint::default();
        let s1 = get_runner_state(&mut cp, "session-a");
        s1.last_captured_timestamp = 100;
        let s2 = get_runner_state(&mut cp, "session-b");
        s2.last_captured_timestamp = 200;

        assert_eq!(cp.runner_states["session-a"].last_captured_timestamp, 100);
        assert_eq!(cp.runner_states["session-b"].last_captured_timestamp, 200);
    }
}
