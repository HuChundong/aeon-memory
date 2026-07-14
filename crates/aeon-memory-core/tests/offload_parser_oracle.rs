use aeon_memory_core::offload::parser::{
    extract_json, extract_mermaid, parse_l1, parse_l2, parse_l15,
};
use serde_json::{Value, json};

fn oracle() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/offload_parser_oracle.json")).unwrap()
}

fn normalize_l2(raw: &str) -> Value {
    let Some(v) = parse_l2(raw) else {
        return Value::Null;
    };
    let mut out = serde_json::Map::from_iter([
        (
            "fileAction".into(),
            Value::String(if v.replace { "replace" } else { "write" }.into()),
        ),
        (
            "nodeMapping".into(),
            serde_json::to_value(v.node_mapping).unwrap(),
        ),
    ]);
    if let Some(content) = v.mmd_content {
        out.insert("mmdContent".into(), Value::String(content));
    }
    if v.replace {
        out.insert(
            "replaceBlocks".into(),
            Value::Array(
                v.replace_blocks
                    .into_iter()
                    .map(|b| json!({"startLine":b.start_line,"endLine":b.end_line,"content":b.content}))
                    .collect(),
            ),
        );
    }
    Value::Object(out)
}

fn normalize_l1(raw: &str) -> Value {
    Value::Array(
        parse_l1(raw)
            .into_iter()
            .map(|v| {
                json!({
                    "tool_call_id": v.tool_call_id,
                    "tool_call": v.tool_call,
                    "summary": v.summary,
                    "timestamp": v.timestamp,
                    "score": v.score,
                    "node_id": v.node_id,
                })
            })
            .collect(),
    )
}

#[test]
fn every_parser_branch_matches_typescript() {
    for case in oracle() {
        let raw = case["raw"].as_str().unwrap();
        assert_eq!(
            extract_json(raw).unwrap_or(Value::Null),
            case["json"],
            "json: {raw}"
        );
        assert_eq!(
            extract_mermaid(raw).map_or(Value::Null, Value::String),
            case["mermaid"],
            "mermaid: {raw}"
        );
        let mut expected_l1 = case["l1"].clone();
        for item in expected_l1.as_array_mut().unwrap() {
            if let Some(score) = item["score"].as_f64() {
                item["score"] = serde_json::to_value(score).unwrap();
            }
        }
        assert_eq!(normalize_l1(raw), expected_l1, "l1: {raw}");
        assert_eq!(
            parse_l15(raw).map_or(Value::Null, |v| serde_json::to_value(v).unwrap()),
            case["l15"],
            "l15: {raw}"
        );
        assert_eq!(normalize_l2(raw), case["l2"], "l2: {raw}");
    }
}
