use aeon_memory_core::offload::{l3, storage, types::OffloadEntry};
use serde_json::{Value, json};

const BASELINE: &str = "4339e63650920871eb0e8888083a1779d114e3ae";

fn entry(id: &str, node: &str, offloaded: Value) -> OffloadEntry {
    OffloadEntry {
        timestamp: "2026-07-13T00:00:00Z".into(),
        node_id: Some(node.into()),
        tool_call: "read({})".into(),
        summary: "summary".into(),
        result_ref: "refs/a.md".into(),
        tool_call_id: id.into(),
        session_key: None,
        score: Some(9.0),
        offloaded: Some(offloaded),
    }
}

#[test]
fn before_prompt_fast_path_matches_pinned_typescript() {
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/offload_fast_path_oracle.json")).unwrap();
    assert_eq!(oracle["baseline"], BASELINE);
    let entries = vec![
        entry("tool_u_1", "N1", Value::Bool(true)),
        entry("gone_2", "N2", Value::String("deleted".into())),
    ];
    let confirmed = storage::confirmed_offload_ids(&entries);
    let deleted = storage::deleted_offload_ids(&entries);
    let mut messages = vec![
        json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu1","input":{"path":"raw"}}]}),
        json!({"role":"tool","toolCallId":"toolu1","content":"raw confirmed result"}),
        json!({"role":"assistant","content":[
            {"type":"text","text":"keep this text"},
            {"type":"tool_use","id":"gone2","input":{"path":"delete"}},
            {"type":"tool_use","id":"toolu1","input":{"path":"compress"}}
        ]}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"gone2","input":{}}]}),
        json!({"role":"tool","toolCallId":"gone2","content":"raw deleted result"}),
        json!({"role":"user","content":"latest request"}),
    ];
    l3::fast_path_reapply(&mut messages, &entries, &confirmed, &deleted);
    let mut confirmed = confirmed.into_iter().collect::<Vec<_>>();
    confirmed.sort();
    let mut deleted = deleted.into_iter().collect::<Vec<_>>();
    deleted.sort();
    assert_eq!(json!(confirmed), oracle["confirmed"]);
    assert_eq!(json!(deleted), oracle["deleted"]);
    assert_eq!(json!(messages), oracle["messages"]);
}
