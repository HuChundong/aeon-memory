use super::{storage::StorageContext, token::Tokenizer};
use crate::AeonMemoryResult;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;

pub const MMD_MARKER: &str = "_mmdContextMessage";
fn role(v: &Value) -> Option<&str> {
    v.get("role")
        .or_else(|| v.get("message")?.get("role"))
        .and_then(Value::as_str)
}
fn is_tool_result(v: &Value) -> bool {
    matches!(role(v), Some("tool") | Some("toolResult"))
}
pub fn active_insertion_point(messages: &[Value]) -> usize {
    if messages.len() <= 2 {
        return 0;
    }
    let half = messages.len() / 2;
    let latest = messages
        .iter()
        .rposition(|m| m.get(MMD_MARKER).is_none() && role(m) == Some("user"));
    let mut idx = if latest.is_some_and(|i| i >= half) {
        latest.unwrap() + 1
    } else {
        let mut s = messages.len();
        for i in (0..messages.len()).rev() {
            if matches!(
                role(&messages[i]),
                Some("toolResult") | Some("tool") | Some("assistant")
            ) {
                s = i
            } else {
                break;
            }
        }
        s.max(messages.len().saturating_sub(30))
            .min(messages.len() - 1)
    };
    while idx > 0 && idx < messages.len() && is_tool_result(&messages[idx]) {
        idx -= 1
    }
    idx
}
pub fn active_block(file: &str, content: &str) -> String {
    let goal = content
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("%%{"))
        .and_then(|l| l.strip_suffix("}%%"))
        .and_then(|j| serde_json::from_str::<Value>(&format!("{{{j}}}")).ok())
        .and_then(|v| v.get("taskGoal").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    ["<current_task_context>".into(),"【当前活跃任务的mermaid流程图】这是你最近正在执行的任务的阶段性记录（此条下方的tool use未被汇总，进程可能有延迟，仅供参考）。".into(),if goal.is_empty(){String::new()}else{format!("**任务目标:** {goal}")},format!("**任务文件:** {file}"),"```mermaid".into(),content.into(),"```".into(),"标记为 \"doing\" 的节点是近期焦点（注：可能有延迟，下方的tool use未被统计，仅供参考），\"done\" 的已完成。请参考此保持方向感，避免重复已完成的工作。".into(),"</current_task_context>".into()].into_iter().filter(|s|!s.is_empty()).collect::<Vec<_>>().join("\n")
}
pub fn inject_active(
    messages: &mut Vec<Value>,
    ctx: &StorageContext,
    file: Option<&str>,
    tokenizer: &dyn Tokenizer,
    context_window: usize,
    max_ratio: f64,
) -> AeonMemoryResult<usize> {
    messages.retain(|m| m.get(MMD_MARKER).is_none());
    let Some(file) = file else { return Ok(0) };
    let path = ctx.mmds_dir.join(file);
    if !path.exists() {
        return Ok(0);
    }
    let text = active_block(file, &fs::read_to_string(path)?);
    let tokens = tokenizer.count(&text);
    if tokens > (context_window as f64 * max_ratio).floor() as usize {
        return Ok(0);
    }
    let idx = active_insertion_point(messages);
    messages.insert(
        idx,
        json!({"role":"user","content":[{"type":"text","text":text}],MMD_MARKER:"active"}),
    );
    Ok(tokens)
}

/// Re-introduce diagrams for tasks whose tool messages were removed by aggressive L3.
/// Newest files win; when a full diagram does not fit, a metadata/node-only view is tried.
pub fn build_history(
    ctx: &StorageContext,
    deleted: &[super::types::OffloadEntry],
    active: Option<&str>,
    tokenizer: &dyn Tokenizer,
    context_window: usize,
    max_ratio: f64,
) -> AeonMemoryResult<(Vec<Value>, usize)> {
    let prefixes: HashSet<_> = deleted
        .iter()
        .filter_map(|e| e.node_id.as_deref()?.split('-').next().map(str::to_owned))
        .collect();
    let budget = (context_window as f64 * max_ratio).floor() as usize;
    let mut files = if ctx.mmds_dir.exists() {
        fs::read_dir(&ctx.mmds_dir)?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|f| {
                f.ends_with(".mmd")
                    && active != Some(f.as_str())
                    && prefixes.contains(f.split('-').next().unwrap_or(""))
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    files.sort();
    files.reverse();
    let mut out = vec![];
    let mut used = 0;
    for file in files {
        let content = fs::read_to_string(ctx.mmds_dir.join(&file))?;
        let full = format!(
            "<historical_task_context>\n**任务文件:** {file}\n```mermaid\n{content}\n```\n</historical_task_context>"
        );
        let compact = format!(
            "<historical_task_context>\n**任务文件:** {file}\n{}\n</historical_task_context>",
            super::mermaid::compact_history_meta(&content)
        );
        let selected = [full, compact]
            .into_iter()
            .find(|s| used + tokenizer.count(s) <= budget);
        if let Some(text) = selected {
            used += tokenizer.count(&text);
            out.push(
                json!({"role":"user","content":[{"type":"text","text":text}],"_mmdInjection":true}),
            );
        }
    }
    Ok((out, used))
}
