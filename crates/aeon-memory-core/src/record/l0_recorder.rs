// port of src/core/conversation/l0-recorder.ts

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::utils::sanitize;
use crate::utils::time;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Flat JSONL message record — one per line in daily shard file.
/// port of L0MessageRecord from l0-recorder.ts:46-54
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L0MessageRecord {
    pub session_key: String,
    pub session_id: String,
    pub recorded_at: String,
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

/// Filtered conversation message returned to caller.
/// port of ConversationMessage from l0-recorder.ts:28-34
#[derive(Clone, Debug)]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

/// Parameters for record_conversation().
/// port of recordConversation params from l0-recorder.ts:89-106
pub struct RecordConversationParams<'a> {
    pub session_key: &'a str,
    pub session_id: Option<&'a str>,
    pub raw_messages: &'a [serde_json::Value],
    pub base_dir: &'a str,
    pub original_user_text: Option<&'a str>,
    pub after_timestamp: Option<i64>,
    pub original_user_message_count: Option<u32>,
}

/// Record a conversation round to the L0 JSONL file.
/// Only records **incremental** messages since the last cursor.
/// port of recordConversation() from l0-recorder.ts:89-296
pub fn record_conversation(
    params: RecordConversationParams<'_>,
) -> AeonMemoryResult<Vec<ConversationMessage>> {
    let session_key = params.session_key;
    let base_dir = params.base_dir;

    // Step 1: Position slice + extract user/assistant messages
    let use_position_slice = params
        .original_user_message_count
        .map(|c| c > 0 && (c as usize) <= params.raw_messages.len())
        .unwrap_or(false);

    let sliced_messages: &[serde_json::Value] = if use_position_slice {
        let start = params.original_user_message_count.unwrap() as usize;
        &params.raw_messages[start..]
    } else {
        params.raw_messages
    };

    let all_extracted = extract_user_assistant_messages(sliced_messages, session_key);

    // Step 1.5: Incremental filter — strict greater-than cursor
    let cursor = params.after_timestamp.unwrap_or(0);
    let mut extracted: Vec<ConversationMessage> = if cursor != 0 {
        all_extracted
            .into_iter()
            .filter(|m| m.timestamp > cursor)
            .collect()
    } else {
        all_extracted
    };

    if extracted.is_empty() {
        return Ok(Vec::new());
    }

    // Step 2: Replace polluted user messages with cached original prompt
    if let Some(original_text) = params.original_user_text {
        let target_raw = if use_position_slice {
            sliced_messages.first()
        } else {
            params
                .original_user_message_count
                .and_then(|c| params.raw_messages.get(c as usize))
        };

        if let Some(raw) = target_raw {
            let target_ts = raw.get("timestamp").and_then(|v| v.as_i64());
            if let Some(ts) = target_ts {
                for msg in extracted.iter_mut() {
                    if msg.role == "user" && msg.timestamp == ts {
                        msg.content = original_text.to_string();
                        break;
                    }
                }
            }
        }
    }

    // Step 3: Sanitize and filter
    let filtered: Vec<ConversationMessage> = extracted
        .into_iter()
        .map(|mut m| {
            let mut content = sanitize::sanitize_text(&m.content);
            if m.role == "assistant" {
                content = sanitize::strip_code_blocks(&content);
            }
            m.content = content;
            m
        })
        .filter(|m| sanitize::should_capture(&m.content))
        .collect();

    if filtered.is_empty() {
        return Ok(Vec::new());
    }

    // Step 4: Write to JSONL — one message per line
    let now = time::now_instant_iso();
    let shard_date = time::local_date_for_filename();
    let out_dir = Path::new(base_dir).join("conversations");
    let out_path = out_dir.join(format!("{}.jsonl", shard_date));

    let mut lines = Vec::with_capacity(filtered.len());
    for msg in &filtered {
        let record = L0MessageRecord {
            session_key: session_key.to_string(),
            session_id: params.session_id.unwrap_or("").to_string(),
            recorded_at: now.clone(),
            id: msg.id.clone(),
            role: msg.role.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
        };
        let line = serde_json::to_string(&record).map_err(AeonMemoryCoreError::Json)?;
        lines.push(line);
    }

    // Write atomically: append to file
    std::fs::create_dir_all(&out_dir).map_err(AeonMemoryCoreError::Io)?;

    let mut content = lines.join("\n");
    content.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(content.as_bytes())
        })
        .map_err(AeonMemoryCoreError::Io)?;

    Ok(filtered)
}

/// Extract user and assistant messages from raw hook message array.
/// port of extractUserAssistantMessages() from l0-recorder.ts:521-567
fn extract_user_assistant_messages(
    messages: &[serde_json::Value],
    _session_key: &str,
) -> Vec<ConversationMessage> {
    let mut result = Vec::new();
    for msg in messages {
        let obj = match msg.as_object() {
            Some(o) => o,
            None => continue,
        };
        let role = match obj.get("role").and_then(|v| v.as_str()) {
            Some("user") | Some("assistant") => obj["role"].as_str().unwrap().to_string(),
            _ => continue,
        };

        let content = extract_text_content(msg);
        let content = match content {
            Some(c) => c,
            None => continue,
        };

        // Strip inline base64 image data URIs
        let content = strip_base64_images(&content);

        if content.trim().is_empty() {
            continue;
        }

        let ts = msg
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(now_epoch_ms_fallback);
        let id = msg
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(generate_message_id);

        result.push(ConversationMessage {
            id,
            role,
            content: content.trim().to_string(),
            timestamp: ts,
        });
    }
    result
}

fn extract_text_content(msg: &serde_json::Value) -> Option<String> {
    if let Some(s) = msg.get("content").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    part.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    None
}

fn strip_base64_images(text: &str) -> String {
    // regex-free: find and replace "data:image/...;base64,..." patterns
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"data:image/") {
            // Find the end of the base64 data (look for ", closing quote, or whitespace)
            if let Some(end) = text[i..].find(['"', ')', ' ', '\n']) {
                result.push_str("[image]");
                i += end;
                continue;
            }
        }
        result.push(text[i..].chars().next().unwrap_or(' '));
        i += text[i..].chars().next().unwrap_or(' ').len_utf8();
    }
    result
}

fn generate_message_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Simple random bytes
    let rand = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        & 0xFFFFFF) as u32;
    format!("msg_{}_{:06x}", ms, rand)
}

fn now_epoch_ms_fallback() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Read L0 message records from daily JSONL files for a given session.
/// port of readConversationRecords() from l0-recorder.ts:305-391
pub fn read_conversation_records(
    session_key: &str,
    base_dir: &str,
) -> AeonMemoryResult<Vec<L0MessageRecord>> {
    let conv_dir = Path::new(base_dir).join("conversations");
    if !conv_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&conv_dir)
        .map_err(AeonMemoryCoreError::Io)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".jsonl"))
        .collect();

    entries.sort();

    let mut records = Vec::new();
    for file_name in entries {
        let file_path = conv_dir.join(&file_name);
        let raw = match std::fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<L0MessageRecord>(line) {
                Ok(rec) if rec.session_key == session_key => records.push(rec),
                _ => continue,
            }
        }
    }

    // Sort by recorded_at, then timestamp
    records.sort_by_key(|r| r.timestamp);
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("aeon-memory-test-l0-recorder")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_msg(id: &str, role: &str, content: &str, ts: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "role": role,
            "content": content,
            "timestamp": ts,
        })
    }

    #[test]
    fn test_extract_user_assistant() {
        let msgs = vec![
            make_msg("1", "user", "hello", 1000),
            make_msg("2", "assistant", "hi there", 1001),
            make_msg("3", "tool", "some result", 1002),
            make_msg("4", "user", "", 1003), // empty content
        ];
        let extracted = extract_user_assistant_messages(&msgs, "test-session");
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].content, "hello");
        assert_eq!(extracted[1].content, "hi there");
    }

    #[test]
    fn test_record_and_read() {
        let dir = setup_dir("roundtrip");

        let msgs = vec![
            make_msg("1", "user", "first message", 1000),
            make_msg("2", "assistant", "first reply", 1001),
        ];

        let result = record_conversation(RecordConversationParams {
            session_key: "test-session",
            session_id: None,
            raw_messages: &msgs,
            base_dir: dir.to_str().unwrap(),
            original_user_text: None,
            after_timestamp: None,
            original_user_message_count: None,
        })
        .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "first message");

        // Read back
        let records = read_conversation_records("test-session", dir.to_str().unwrap()).unwrap();
        let conv_dir = dir.join("conversations");
        let files: Vec<_> = if conv_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&conv_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            entries
                .iter()
                .map(|e| {
                    format!(
                        "name={} is_file={}",
                        e.file_name().to_string_lossy(),
                        e.file_type().map(|t| t.is_file()).unwrap_or(false)
                    )
                })
                .collect()
        } else {
            vec![]
        };
        eprintln!(
            "DEBUG: conv_dir exists={}, files={:?}",
            conv_dir.exists(),
            files
        );
        assert!(!files.is_empty(), "No JSONL files found in {:?}", conv_dir);
        assert_eq!(
            records.len(),
            2,
            "Expected 2 records, got {}. Files: {:?}",
            records.len(),
            files
        );
        assert_eq!(records[0].content, "first message");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_incremental_cursor() {
        let dir = setup_dir("incremental");

        // First batch
        let msgs1 = vec![make_msg("1", "user", "first", 1000)];
        record_conversation(RecordConversationParams {
            session_key: "sk",
            session_id: None,
            raw_messages: &msgs1,
            base_dir: dir.to_str().unwrap(),
            original_user_text: None,
            after_timestamp: None,
            original_user_message_count: None,
        })
        .unwrap();

        // Second batch with cursor — should only capture new messages
        let msgs2 = vec![
            make_msg("1", "user", "first", 1000),
            make_msg("2", "assistant", "second", 2000),
        ];
        let result = record_conversation(RecordConversationParams {
            session_key: "sk",
            session_id: None,
            raw_messages: &msgs2,
            base_dir: dir.to_str().unwrap(),
            original_user_text: None,
            after_timestamp: Some(1000),
            original_user_message_count: None,
        })
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "second");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_original_user_text_replacement() {
        let dir = setup_dir("original_text");

        // Simulate a user message that has been polluted with prependContext
        let msgs = vec![
            make_msg("1", "assistant", "previous reply", 500),
            serde_json::json!({
                "id": "2",
                "role": "user",
                "content": "<relevant-memories>old memory</relevant-memories> actual user query",
                "timestamp": 1000,
            }),
        ];

        let result = record_conversation(RecordConversationParams {
            session_key: "sk",
            session_id: None,
            raw_messages: &msgs,
            base_dir: dir.to_str().unwrap(),
            original_user_text: Some("actual user query"),
            after_timestamp: None,
            original_user_message_count: Some(1), // slice from index 1 → only user msg
        })
        .unwrap();

        assert_eq!(result.len(), 1);
        // The content should be replaced with the original clean text, THEN sanitized
        assert_eq!(result[0].content, "actual user query");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_base64_images_stripped() {
        let msgs = vec![serde_json::json!({
            "id": "1", "role": "user",
            "content": "Look at this: data:image/png;base64,iVBORw0KGgo= and more text",
            "timestamp": 1000,
        })];
        let extracted = extract_user_assistant_messages(&msgs, "sk");
        assert_eq!(extracted.len(), 1);
        assert!(extracted[0].content.contains("[image]"));
        assert!(!extracted[0].content.contains("iVBORw0KGgo="));
    }

    #[test]
    fn test_empty_messages_filtered() {
        let msgs = vec![
            make_msg("1", "user", "", 1000),
            make_msg("2", "assistant", "   ", 1001),
            make_msg("3", "user", "real msg", 1002),
        ];
        let extracted = extract_user_assistant_messages(&msgs, "sk");
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].content, "real msg");
    }

    #[test]
    fn test_strip_code_blocks_from_assistant() {
        let dir = setup_dir("strip_code");

        let msgs = vec![make_msg(
            "1",
            "assistant",
            "Here is code:\n```rust\nfn main() {}\n```\nEnd.",
            1000,
        )];
        let result = record_conversation(RecordConversationParams {
            session_key: "sk",
            session_id: None,
            raw_messages: &msgs,
            base_dir: dir.to_str().unwrap(),
            original_user_text: None,
            after_timestamp: None,
            original_user_message_count: None,
        })
        .unwrap();

        assert_eq!(
            result.len(),
            1,
            "expected 1 result, got {}: {:?}",
            result.len(),
            result.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
        // The code block should be stripped from assistant messages
        assert!(
            result[0].content.contains("Here is code:"),
            "content: {}",
            result[0].content
        );
        assert!(
            !result[0].content.contains("```"),
            "content still contains code fence: {}",
            result[0].content
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
