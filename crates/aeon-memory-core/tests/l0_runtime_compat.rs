use aeon_memory_core::record::l0_recorder::{
    RecordConversationParams, read_conversation_records, record_conversation,
};
use serde_json::{Value, json};

fn clean(role: &str, content: &str, timestamp: i64) -> Value {
    json!({"role":role,"content":content,"timestamp":timestamp})
}

#[test]
fn l0_filter_slice_sanitize_and_persistence_match_typescript() {
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/l0_runtime_oracle.json")).unwrap();
    let cases = vec![
        (
            "cursor",
            json!([{"role":"user","content":"old","timestamp":100},{"role":"assistant","content":"new answer","timestamp":201},{"role":"user","content":"new question","timestamp":202}]),
            Some(200),
            None,
            None,
        ),
        (
            "slice_replace",
            json!([{"role":"user","content":"history","timestamp":1},{"role":"assistant","content":"history answer","timestamp":2},{"role":"user","content":"<memory-context>polluted</memory-context> actual","timestamp":300},{"role":"assistant","content":"before\n```js\nsecret()\n```\nafter","timestamp":301}]),
            None,
            Some(2),
            Some("clean user prompt"),
        ),
        (
            "content_parts",
            json!([{"role":"user","content":[{"type":"text","text":"hello"},{"type":"image","data":"data:image/png;base64,AAAA"}],"timestamp":400},{"role":"assistant","content":[{"type":"text","text":"world"}],"timestamp":401}]),
            None,
            None,
            None,
        ),
    ];
    for (index, (name, raw, cursor, count, original)) in cases.into_iter().enumerate() {
        let dir = std::env::temp_dir().join(format!(
            "aeon-memory-l0-compat-{}-{index}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let messages = raw.as_array().unwrap();
        let returned = record_conversation(RecordConversationParams {
            session_key: if name == "content_parts" {
                "parts"
            } else {
                "s"
            },
            session_id: Some(if name == "content_parts" { "p" } else { "sid" }),
            raw_messages: messages,
            base_dir: dir.to_str().unwrap(),
            original_user_text: original,
            after_timestamp: cursor,
            original_user_message_count: count,
        })
        .unwrap();
        let session_key = if name == "content_parts" {
            "parts"
        } else {
            "s"
        };
        let persisted = read_conversation_records(session_key, dir.to_str().unwrap()).unwrap();
        let actual_returned = returned
            .iter()
            .map(|m| clean(&m.role, &m.content, m.timestamp))
            .collect::<Vec<_>>();
        let actual_persisted = persisted
            .iter()
            .map(|m| clean(&m.role, &m.content, m.timestamp))
            .collect::<Vec<_>>();
        assert_eq!(
            json!(actual_returned),
            expected[index]["returned"],
            "{name} returned"
        );
        assert_eq!(
            json!(actual_persisted),
            expected[index]["persisted"],
            "{name} persisted"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
