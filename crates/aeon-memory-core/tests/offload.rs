use aeon_memory_core::offload::{
    l3, mermaid, parser,
    storage::{self, StorageContext},
    token,
    types::{OffloadConfig, OffloadEntry},
};
use serde_json::json;
use std::fs;
fn entry(id: &str) -> OffloadEntry {
    OffloadEntry {
        timestamp: "2026-01-01T00:00:00Z".into(),
        node_id: Some("N1".into()),
        tool_call: "read({})".into(),
        summary: "read file".into(),
        result_ref: "refs/a.md".into(),
        tool_call_id: id.into(),
        session_key: None,
        score: Some(8.0),
        offloaded: None,
    }
}
#[test]
fn disabled_and_storage_roundtrip() {
    assert!(!OffloadConfig::default().enabled);
    let root = std::env::temp_dir().join(format!("aeon-memory-offload-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let c = StorageContext::new(&root, "main", "s1");
    storage::append_entry(&c, &entry("tool_u_1")).unwrap();
    assert_eq!(storage::read_entries(&c).unwrap(), vec![entry("tool_u_1")]);
    fs::write(&c.offload_jsonl, "bad\n{\"tool_call_id\":\"\"}\n").unwrap();
    assert!(storage::read_entries(&c).unwrap().is_empty());
    let _ = fs::remove_dir_all(root);
}
#[test]
fn parsers_preserve_ts_fallbacks() {
    let l1 = parser::parse_l1("```json\n[{\"tool_call_id\":\"x\",\"summary\":\"ok\",}]\n```");
    assert_eq!(l1[0].score, Some(5.0));
    assert!(
        parser::parse_l15("{\"taskCompleted\":null,\"isContinuation\":null,\"isLongTask\":null}")
            .is_none()
    );
    let l2 = parser::parse_l2("```mermaid\nflowchart TD\n A-->B\n```").unwrap();
    assert_eq!(l2.mmd_content.unwrap(), "flowchart TD\n A-->B");
}
#[test]
fn l3_matches_normalized_ids_and_mutates_messages() {
    let e = entry("tool_u_1");
    let map = l3::index(std::slice::from_ref(&e));
    assert!(l3::lookup(&map, "toolu1").is_some());
    let mut tool = json!({"role":"tool","toolCallId":"toolu1","content":"large"});
    assert!(l3::replace_tool_result(&mut tool, &e));
    assert!(tool["_offloaded"].as_bool().unwrap());
    let mut assistant = json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu1","input":{"path":"x"}}]});
    assert_eq!(l3::replace_assistant_tool_uses(&mut assistant, &[e]), 1);
    assert_eq!(assistant["content"][0]["input"]["node_id"], "N1");
}
#[test]
fn snapshot_dedupes_user_and_strips_metadata() {
    let msgs = vec![json!({"role":"user","content":"hello","_offloaded":true})];
    let s = token::snapshot("before", &msgs, Some("sys"), Some("hello"));
    assert_eq!(s.user_prompt_tokens, 0);
    assert_eq!(s.message_count, 1);
}
#[test]
fn mermaid_replacements_are_reverse_ordered() {
    let out = mermaid::apply_replace_blocks(
        "a\nb\nc\nd",
        &[
            parser::ReplaceBlock {
                start_line: 2,
                end_line: 2,
                content: "B".into(),
            },
            parser::ReplaceBlock {
                start_line: 4,
                end_line: 4,
                content: "D".into(),
            },
        ],
    )
    .unwrap();
    assert_eq!(out, "a\nB\nc\nD");
}
