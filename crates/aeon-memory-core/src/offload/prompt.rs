//! Byte-compatible builders for the three local-LLM prompts.
use super::types::{OffloadEntry, ToolPair};
use serde::{Deserialize, Serialize};

// Keeping the canonical text in one place prevents translations from silently drifting.
// The extractor is covered by a golden test against the TypeScript exported template.
const L1_TS: &str = include_str!("../../fixtures/prompts/l1-prompt.txt");
const L15_TS: &str = include_str!("../../fixtures/prompts/l15-prompt.txt");
const L2_TS: &str = include_str!("../../fixtures/prompts/l2-prompt.txt");

fn exported_template(source: &'static str, name: &str) -> String {
    let marker = format!("export const {name} = `");
    let start = source.find(&marker).expect("canonical prompt export") + marker.len();
    let tail = &source[start..];
    let end = tail.find("`;\n").expect("canonical prompt terminator");
    // TypeScript template literals escape embedded backticks in source; the runtime
    // value consumed by the old implementation does not contain those backslashes.
    tail[..end].replace("\\`", "`")
}
pub fn l1_system_prompt() -> &'static str {
    static VALUE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| exported_template(L1_TS, "L1_SYSTEM_PROMPT"))
}
pub fn l15_system_prompt() -> &'static str {
    static VALUE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| exported_template(L15_TS, "L15_SYSTEM_PROMPT"))
}
pub fn l2_system_prompt() -> &'static str {
    static VALUE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| exported_template(L2_TS, "L2_SYSTEM_PROMPT"))
}

fn js_stringify(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}
fn truncate_utf16(s: &str, max: usize) -> String {
    if s.encode_utf16().count() <= max {
        return s.into();
    }
    let units: Vec<_> = s.encode_utf16().take(max).collect();
    format!("{}...", String::from_utf16_lossy(&units))
}
pub fn build_l1_user_prompt(recent: &str, pairs: &[ToolPair]) -> String {
    let mut p = vec![
        "## 最近的对话上下文（用于理解当前任务）：".into(),
        recent.into(),
        "\n## Tool call/result pairs to summarize:".into(),
    ];
    for (i, pair) in pairs.iter().enumerate() {
        let params = js_stringify(&pair.params);
        let result = js_stringify(&pair.result);
        let canonical = format!("{}({})", pair.tool_name, js_stringify(&pair.params));
        p.push(format!("--- Tool Pair {} ---", i + 1));
        p.push(format!("tool_call_id: {}", pair.tool_call_id));
        p.push(format!("timestamp: {}", pair.timestamp));
        p.push(format!(
            "Tool: {}{}",
            pair.tool_name,
            if canonical.encode_utf16().count() > 200 {
                " [NEEDS_COMPRESS]"
            } else {
                ""
            }
        ));
        p.push(format!("Params: {}", truncate_utf16(&params, 500)));
        p.push(format!("Result: {}\n", truncate_utf16(&result, 2000)));
    }
    p.push("Summarize each pair into the JSON array format described.".into());
    p.join("\n")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MmdMeta {
    pub filename: String,
    pub path: String,
    pub task_goal: String,
    pub done_count: usize,
    pub doing_count: usize,
    pub todo_count: usize,
    pub updated_time: Option<String>,
    #[serde(default)]
    pub node_summaries: Vec<NodeSummary>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeSummary {
    pub node_id: String,
    pub status: String,
    pub summary: String,
}
pub fn build_l15_user_prompt(
    recent: &str,
    current: Option<(&str, &str, &str)>,
    metas: &[MmdMeta],
) -> String {
    let mut p = vec![
        "## 1. 最近的对话上下文 (Recent 6 messages):".into(),
        recent.into(),
        "\n## 2. 当前挂载的任务图 (Active Mermaid — 完整内容):".into(),
    ];
    if let Some((file, content, path)) = current {
        p.push(format!("**File:** {file}"));
        if !path.is_empty() {
            p.push(format!("**Path:** `{path}`"));
        }
        p.push(format!("\n```mermaid\n{content}\n```"));
    } else {
        p.push("(none - 当前处于闲置状态，无活跃任务)".into());
    }
    p.push("\n## 3. 历史可用的任务图 (Available Mermaid task files):".into());
    if metas.is_empty() {
        p.push("(none - 暂无历史长任务)".into())
    } else {
        for m in metas {
            p.push(format!("- **{}**", m.filename));
            p.push(format!("  path: `{}`", m.path));
            p.push(format!("  taskGoal: {}", m.task_goal));
            let total = m.done_count + m.doing_count + m.todo_count;
            p.push(format!(
                "  progress: {}/{} done, {} doing, {} todo",
                m.done_count, total, m.doing_count, m.todo_count
            ));
            if let Some(t) = &m.updated_time {
                p.push(format!("  lastUpdated: {t}"));
            }
            if !m.node_summaries.is_empty() {
                p.push("  recentNodes:".into());
                for n in &m.node_summaries {
                    p.push(format!(
                        "    - [{}] ({}) {}",
                        n.node_id, n.status, n.summary
                    ));
                }
            }
            p.push(String::new());
        }
    }
    p.push("请严格根据系统指令的【三步思考链路】进行研判，并输出合法的 JSON 对象。".into());
    p.join("\n")
}

#[allow(clippy::too_many_arguments)]
pub fn build_l2_user_prompt(
    existing: Option<&str>,
    entries: &[OffloadEntry],
    recent: Option<&str>,
    turn: Option<&str>,
    label: &str,
    prefix: &str,
    char_count: usize,
) -> String {
    let mut p = vec![format!(
        "## 近期对话历史：\n{}",
        recent.unwrap_or("(无可用历史)")
    )];
    if let Some(t) = turn {
        p.push(format!("\n## 当前最新一轮：\n{t}"));
    }
    p.push(format!("\n## MMD prefix: {prefix}"));
    p.push(format!(
        "（所有节点 ID 必须以此前缀开头，如 {prefix}-N1, {prefix}-N2...）"
    ));
    p.push(format!("\n## Current task label: {label}"));
    if char_count > 2500 {
        p.push(format!(
            "\n## Current MMD size: {char_count} chars (budget: 4000 chars)"
        ));
        p.push("⚠ 接近上限，请积极合并节点、精简 summary，优先使用 replace 模式微调而非 write 全量重写。".into());
    } else if char_count > 2000 {
        p.push(format!(
            "\n## Current MMD size: {char_count} chars (budget: 4000 chars)"
        ));
        p.push("注意控制增长，合并同类节点。".into());
    }
    p.push("\n## Existing Mermaid content:".into());
    if let Some(m) = existing {
        for (i, l) in m.split('\n').enumerate() {
            p.push(format!("L{}: {l}", i + 1));
        }
    } else {
        p.push("(empty — create new)".into());
    }
    p.push("\n## New offload entries to incorporate:".into());
    for (i, e) in entries.iter().enumerate() {
        p.push(format!(
            "{}. [{}] {} → {} ({})",
            i + 1,
            e.tool_call_id,
            e.tool_call,
            e.summary,
            e.timestamp
        ));
    }
    p.push(
        "\n请根据系统指令生成/更新 Mermaid 流程图，并输出合法的 JSON 对象（含 node_mapping）。"
            .into(),
    );
    p.join("\n")
}
