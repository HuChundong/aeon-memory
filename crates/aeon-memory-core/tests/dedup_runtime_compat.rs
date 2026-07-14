use serde_json::Value;

#[test]
fn pinned_ts_dedup_runtime_decision_and_call_contract() {
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/dedup_runtime_oracle.json")).unwrap();
    let actions = oracle["decisions"].as_array().unwrap();
    assert_eq!(
        actions
            .iter()
            .map(|d| d["action"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["store", "skip", "update"]
    );
    assert_eq!(actions[2]["target_ids"], serde_json::json!(["old-1"]));
    assert_eq!(actions[2]["merged_content"], "merged");
    assert_eq!(oracle["failed"][0]["action"], "store");
    assert_eq!(oracle["noRecall"][0]["action"], "store");
    assert_eq!(
        oracle["trace"]["embeds"][0][0],
        serde_json::json!(["store memory", "skip memory", "update memory"])
    );
    assert_eq!(oracle["trace"]["embeds"][0][1]["timeoutMs"], 321);
    assert_eq!(oracle["trace"]["llm"][0]["taskId"], "l1-conflict-detection");
    assert_eq!(oracle["trace"]["llm"][0]["timeoutMs"], 180_000);
}
