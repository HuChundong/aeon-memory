use super::types::OffloadEntry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn extract_json(raw: &str) -> Option<Value> {
    if raw.is_empty() {
        return None;
    }
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str(trimmed) {
        return Some(v);
    }
    if let Some(a) = trimmed.find("```") {
        let rest = &trimmed[a + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        if let Some(b) = rest.find("```") {
            let candidate = rest[..b].trim();
            if let Ok(v) = serde_json::from_str(candidate) {
                return Some(v);
            }
            let fixed = candidate.replace(",}", "}");
            if let Ok(v) = serde_json::from_str(&fixed) {
                return Some(v);
            }
        }
    }
    if let (Some(a), Some(b)) = (trimmed.find('{'), trimmed.rfind('}')) {
        let candidate = &trimmed[a..=b];
        if let Ok(v) = serde_json::from_str(candidate) {
            return Some(v);
        }
        let fixed = candidate.replace(",}", "}");
        if let Ok(v) = serde_json::from_str(&fixed) {
            return Some(v);
        }
    }
    if let (Some(a), Some(b)) = (trimmed.find('['), trimmed.rfind(']')) {
        let candidate = &trimmed[a..=b];
        if let Ok(v) = serde_json::from_str(candidate) {
            return Some(v);
        }
        let fixed = candidate.replace(",}", "}");
        if let Ok(v) = serde_json::from_str(&fixed) {
            return Some(v);
        }
    }
    let fixed = trimmed.replace(",}", "}");
    if let Ok(v) = serde_json::from_str(&fixed) {
        return Some(v);
    }
    None
}
pub fn extract_mermaid(raw: &str) -> Option<String> {
    if let Some(a) = raw.find("```mermaid") {
        let s = &raw[a + 10..];
        if let Some(b) = s.find("```") {
            return Some(s[..b].trim().into());
        }
    }
    if raw.contains("flowchart") || raw.contains("graph") {
        Some(raw.trim().into())
    } else {
        None
    }
}
pub fn parse_l1(raw: &str) -> Vec<OffloadEntry> {
    let Some(Value::Array(items)) = extract_json(raw) else {
        return vec![];
    };
    items
        .into_iter()
        .filter_map(|v| {
            let id = v.get("tool_call_id")?.as_str()?.to_owned();
            if id.is_empty() {
                return None;
            }
            Some(OffloadEntry {
                timestamp: v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                node_id: None,
                tool_call: v
                    .get("tool_call")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                summary: v
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                result_ref: String::new(),
                tool_call_id: id,
                session_key: None,
                score: Some(v.get("score").and_then(Value::as_f64).unwrap_or(5.0)),
                offloaded: v.get("offloaded").cloned(),
            })
        })
        .collect()
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskJudgment {
    pub task_completed: bool,
    pub is_continuation: bool,
    pub is_long_task: bool,
    pub continuation_mmd_file: Option<String>,
    pub new_task_label: Option<String>,
}
pub fn parse_l15(raw: &str) -> Option<TaskJudgment> {
    let v = extract_json(raw)?;
    if ["taskCompleted", "isContinuation", "isLongTask"]
        .iter()
        .all(|k| v.get(k).is_none_or(Value::is_null))
    {
        return None;
    }
    Some(TaskJudgment {
        task_completed: v
            .get("taskCompleted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_continuation: v
            .get("isContinuation")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_long_task: v
            .get("isLongTask")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        continuation_mmd_file: v
            .get("continuationMmdFile")
            .and_then(Value::as_str)
            .map(Into::into),
        new_task_label: v
            .get("newTaskLabel")
            .and_then(Value::as_str)
            .map(Into::into),
    })
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceBlock {
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Response {
    pub replace: bool,
    pub mmd_content: Option<String>,
    pub replace_blocks: Vec<ReplaceBlock>,
    pub node_mapping: BTreeMap<String, String>,
}
pub fn parse_l2(raw: &str) -> Option<L2Response> {
    let Some(v) = extract_json(raw) else {
        return extract_mermaid(raw).map(|m| L2Response {
            replace: false,
            mmd_content: Some(m),
            replace_blocks: vec![],
            node_mapping: BTreeMap::new(),
        });
    };
    let replace = v.get("file_action").and_then(Value::as_str) == Some("replace");
    let mmd_content = if replace {
        None
    } else {
        v.get("mmd_content")
            .and_then(Value::as_str)
            .and_then(|s| extract_mermaid(s).or_else(|| Some(s.into())))
            .or_else(|| extract_mermaid(raw))
    };
    let replace_blocks = v
        .get("replace_blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let number = |v: &Value| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                    .map(|n| n as usize)
            };
            Some(ReplaceBlock {
                start_line: number(b.get("start_line")?)?,
                end_line: number(b.get("end_line")?)?,
                content: b
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|s| extract_mermaid(s).unwrap_or_else(|| s.into()))
                    .unwrap_or_default(),
            })
        })
        .collect();
    let node_mapping = v
        .get("node_mapping")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.into())))
        .collect();
    Some(L2Response {
        replace,
        mmd_content,
        replace_blocks,
        node_mapping,
    })
}
