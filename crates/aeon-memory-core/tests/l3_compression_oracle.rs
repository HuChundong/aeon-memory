use aeon_memory_core::offload::{l3, token::O200kTokenizer, types::OffloadEntry};
use serde_json::{Value, json};
fn expected() -> Value {
    serde_json::from_str(include_str!("fixtures/l3_compression_oracle.json")).unwrap()
}
fn entries() -> Vec<OffloadEntry> {
    (0..12)
        .map(|i| OffloadEntry {
            timestamp: "".into(),
            node_id: Some(format!("N-c{i}")),
            tool_call: "read({})".into(),
            summary: format!("summary-c{i}"),
            result_ref: format!("refs/c{i}.md"),
            tool_call_id: format!("c{i}"),
            session_key: None,
            score: Some(if i < 6 { 9.0 } else { 5.0 }),
            offloaded: None,
        })
        .collect()
}
#[test]
fn mild_mutation_bytes_match_typescript() {
    let e = expected();
    let mut messages = (0..12)
        .map(|i| json!({"role":"tool","toolCallId":format!("c{i}"),"content":"x".repeat(300)}))
        .collect::<Vec<_>>();
    messages.push(json!({"role":"user","content":"latest"}));
    let result = l3::mild_cascade(&mut messages, &entries(), 1.0);
    assert_eq!(result.replaced_count, e["mild"]["result"]["replacedCount"]);
    assert_eq!(
        result.final_threshold,
        e["mild"]["result"]["finalThreshold"]
    );
    assert_eq!(
        json!(result.replaced_tool_call_ids),
        e["mild"]["result"]["replacedToolCallIds"]
    );
    assert_eq!(json!(messages), e["mild"]["messages"])
}
#[test]
fn aggressive_boundary_and_last_user_protection_match_typescript() {
    let e = expected();
    let mut m=(0..8).map(|i|json!({"role":if i==5{"user"}else{"assistant"},"content":format!("m{i}-{}","x".repeat(80))})).collect::<Vec<_>>();
    let r = l3::aggressive_delete(&mut m, 100, "sys", &O200kTokenizer);
    assert_eq!(r.deleted_count, e["aggressive"]["result"]["deletedCount"]);
    assert_eq!(
        r.remaining_tokens,
        e["aggressive"]["result"]["remainingTokens"]
    );
    assert_eq!(json!(m), e["aggressive"]["messages"])
}
#[test]
fn emergency_target_and_minimum_keep_match_typescript() {
    let e = expected();
    let mut m = (0..7)
        .map(|i| json!({"role":"assistant","content":format!("e{i}-{}","y".repeat(100))}))
        .collect::<Vec<_>>();
    let r = l3::emergency_delete(&mut m, 60, "sys", &O200kTokenizer);
    assert_eq!(r.deleted_count, e["emergency"]["result"]["deletedCount"]);
    assert_eq!(
        r.remaining_tokens,
        e["emergency"]["result"]["remainingTokens"]
    );
    assert_eq!(json!(m), e["emergency"]["messages"])
}

#[test]
fn emergency_stalled_head_tail_group_matches_typescript() {
    let e = expected();
    let mut m = vec![
        json!({"role":"user","content":"head"}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"read","input":{"q":"x".repeat(1200)}}]}),
        json!({"role":"tool","toolCallId":"tu1","content":"r".repeat(1600)}),
        json!({"role":"assistant","content":"plain".repeat(300)}),
        json!({"role":"system","content":"keep"}),
    ];
    let r = l3::emergency_delete(&mut m, 60, "", &O200kTokenizer);
    assert_eq!(json!(r.deleted_count), e["tail"]["result"]["deletedCount"]);
    assert_eq!(
        json!(r.deleted_tool_call_ids),
        e["tail"]["result"]["deletedToolCallIds"]
    );
    assert_eq!(
        json!(r.remaining_tokens),
        e["tail"]["result"]["remainingTokens"]
    );
    assert_eq!(json!(m), e["tail"]["messages"]);
}

#[test]
fn emergency_oversized_truncation_matches_typescript() {
    let e = expected();
    let mut m = vec![
        json!({"role":"user","content":"head"}),
        json!({"role":"system","content":"H".repeat(6000)}),
        json!({"role":"system","content":"keep"}),
    ];
    let r = l3::emergency_delete(&mut m, 60, "", &O200kTokenizer);
    assert_eq!(
        json!(r.remaining_tokens),
        e["truncate"]["result"]["remainingTokens"]
    );
    assert_eq!(json!(m), e["truncate"]["messages"]);
}

#[test]
fn aggressive_reinserts_history_injection_and_active_mmd_like_typescript() {
    let e = expected();
    let mut m = vec![
        json!({"role":"assistant","content":"old".repeat(100)}),
        json!({"role":"system","content":"history","_mmdContextMessage":"history"}),
        json!({"role":"assistant","content":"old2".repeat(100)}),
        json!({"role":"system","content":"active","_mmdContextMessage":"active"}),
        json!({"role":"system","content":"inject","_mmdInjection":true}),
        json!({"role":"user","content":"latest"}),
        json!({"role":"assistant","content":"recent"}),
    ];
    let r = l3::aggressive_delete(&mut m, 30, "", &O200kTokenizer);
    assert_eq!(json!(r.deleted_count), e["mmd"]["result"]["deletedCount"]);
    assert_eq!(
        json!(r.remaining_tokens),
        e["mmd"]["result"]["remainingTokens"]
    );
    assert_eq!(json!(m), e["mmd"]["messages"]);
}
