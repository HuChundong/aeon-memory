use aeon_memory_core::offload::{
    engine::OffloadEngine,
    storage::StorageContext,
    types::{OffloadConfig, ToolPair},
};
use serde_json::{Value, json};
fn pair(id: &str, params: Value, result: Value) -> ToolPair {
    ToolPair {
        tool_name: "shell".into(),
        tool_call_id: id.into(),
        params,
        result,
        error: None,
        timestamp: "t".into(),
        duration_ms: None,
    }
}
#[test]
fn hook_buffer_filters_thresholds_and_persistent_state_match_typescript() {
    let o: Value =
        serde_json::from_str(include_str!("fixtures/offload_state_oracle.json")).unwrap();
    let d = std::env::temp_dir().join(format!("aeon-memory-state-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut e = OffloadEngine::load(
        StorageContext::new(&d, "main", "s1"),
        OffloadConfig::default(),
    )
    .unwrap();
    let inputs = [
        pair("a", json!({"cmd":"ls"}), json!("ok")),
        pair("a", json!({"cmd":"ls"}), json!("duplicate")),
        pair("hb", json!({"path":"HEARTBEAT.md"}), json!("x")),
        pair(
            "approval",
            json!({}),
            json!({"details":{"status":"approval-pending"}}),
        ),
        pair("b", json!({"cmd":"pwd"}), json!("ok")),
    ];
    let mut counts = vec![];
    for p in inputs {
        e.buffer_persisted(p).unwrap();
        counts.push(e.pending_count());
    }
    assert_eq!(json!(counts), o["counts"]);
    assert_eq!(
        json!([e.pending_count() >= 3, e.pending_count() >= 2]),
        o["forced"]
    );
    e.state.active_mmd_file = Some("001-task.mmd".into());
    e.state.active_mmd_id = Some("node-1".into());
    e.state.last_offloaded_tool_call_id = Some("a".into());
    e.state.last_l2_trigger_time = Some("2026-01-01T00:00:00Z".into());
    aeon_memory_core::offload::storage::save_state(&e.ctx, &e.state).unwrap();
    let loaded = aeon_memory_core::offload::storage::load_state(&e.ctx).unwrap();
    assert_eq!(
        loaded.active_mmd_file.as_deref(),
        o["state"]["activeMmdFile"].as_str()
    );
    assert_eq!(
        loaded.active_mmd_id.as_deref(),
        o["state"]["activeMmdId"].as_str()
    );
    assert_eq!(
        loaded.last_offloaded_tool_call_id.as_deref(),
        o["state"]["lastOffloadedToolCallId"].as_str()
    );
    assert_eq!(
        loaded.last_l2_trigger_time.as_deref(),
        o["state"]["lastL2TriggerTime"].as_str()
    );
    let _ = std::fs::remove_dir_all(d);
}
