use super::token::Tokenizer;
use super::types::OffloadEntry;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FastPathResult {
    pub applied: usize,
    pub deleted: usize,
}

fn is_tracked(ids: &std::collections::HashSet<String>, id: &str) -> bool {
    ids.contains(id) || ids.contains(&normalize_tool_call_id(id))
}

fn message_view(message: &Value) -> &Value {
    message.get("message").unwrap_or(message)
}

fn message_tool_id(message: &Value) -> Option<&str> {
    let message = message_view(message);
    message
        .get("toolCallId")
        .or_else(|| message.get("tool_call_id"))
        .and_then(Value::as_str)
}

fn is_tool_result(message: &Value) -> bool {
    matches!(
        message_view(message).get("role").and_then(Value::as_str),
        Some("tool" | "toolResult")
    )
}

fn content_blocks(message: &Value) -> Option<&Vec<Value>> {
    message_view(message)
        .get("content")
        .and_then(Value::as_array)
}

fn content_blocks_mut(message: &mut Value) -> Option<&mut Vec<Value>> {
    if message.get("message").is_some() {
        message
            .get_mut("message")?
            .get_mut("content")?
            .as_array_mut()
    } else {
        message.get_mut("content")?.as_array_mut()
    }
}

fn assistant_tool_ids(message: &Value) -> Vec<String> {
    content_blocks(message)
        .into_iter()
        .flatten()
        .filter(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("tool_use" | "toolCall")
            )
        })
        .filter_map(|block| block.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn only_tool_use_assistant(message: &Value) -> bool {
    if message_view(message).get("role").and_then(Value::as_str) != Some("assistant") {
        return false;
    }
    let Some(blocks) = content_blocks(message) else {
        return false;
    };
    !blocks.is_empty()
        && blocks.iter().all(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("tool_use" | "toolCall")
            )
        })
}

/// Re-apply confirmed/deleted decisions before considering new compression.
/// Hosts commonly resubmit the pristine transcript on every request; without
/// this phase an earlier L3 decision would silently disappear on the next turn.
pub fn fast_path_reapply(
    messages: &mut Vec<Value>,
    entries: &[OffloadEntry],
    confirmed: &std::collections::HashSet<String>,
    deleted: &std::collections::HashSet<String>,
) -> FastPathResult {
    if confirmed.is_empty() && deleted.is_empty() {
        return FastPathResult::default();
    }
    let entries_by_id = index(entries);
    let mut delete_indices = Vec::new();
    let mut applied = 0;
    for (index, message) in messages.iter_mut().enumerate() {
        let direct_id = message_tool_id(message).map(str::to_owned);
        if direct_id
            .as_deref()
            .is_some_and(|id| is_tracked(deleted, id))
        {
            delete_indices.push(index);
            continue;
        }

        let tool_ids = assistant_tool_ids(message);
        let only_tool_use = only_tool_use_assistant(message);
        if only_tool_use
            && !tool_ids.is_empty()
            && tool_ids.iter().all(|id| is_tracked(deleted, id))
        {
            delete_indices.push(index);
            continue;
        }

        // Mixed text + tool-use assistant messages stay in the transcript, but
        // blocks whose paired result was deleted must be removed to avoid an
        // orphaned tool call at the provider boundary.
        if !only_tool_use
            && !tool_ids.is_empty()
            && let Some(blocks) = content_blocks_mut(message)
        {
            blocks.retain(|block| {
                let is_tool = matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("tool_use" | "toolCall")
                );
                !is_tool
                    || block
                        .get("id")
                        .and_then(Value::as_str)
                        .is_none_or(|id| !is_tracked(deleted, id))
            });
        }

        if message.get("_offloaded").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(id) = direct_id.as_deref()
            && is_tracked(confirmed, id)
            && is_tool_result(message)
            && let Some(entry) = lookup(&entries_by_id, id)
            && replace_tool_result(message, entry)
        {
            message["_offloaded"] = Value::Bool(true);
            applied += 1;
            continue;
        }

        if only_tool_use
            && !tool_ids.is_empty()
            && tool_ids.iter().all(|id| is_tracked(confirmed, id))
        {
            let matched = tool_ids
                .iter()
                .filter_map(|id| lookup(&entries_by_id, id).cloned())
                .collect::<Vec<_>>();
            if matched.len() == tool_ids.len() {
                replace_assistant_tool_uses(message, &matched);
                message["_offloaded"] = Value::Bool(true);
                applied += 1;
            }
        } else if !tool_ids.is_empty() {
            let matched = entries
                .iter()
                .filter(|entry| is_tracked(confirmed, &entry.tool_call_id))
                .cloned()
                .collect::<Vec<_>>();
            replace_assistant_tool_uses(message, &matched);
        }
    }
    for index in delete_indices.iter().rev() {
        messages.remove(*index);
    }
    FastPathResult {
        applied,
        deleted: delete_indices.len(),
    }
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub deleted_count: usize,
    pub remaining_tokens: usize,
    pub deleted_tool_call_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MildResult {
    pub replaced_count: usize,
    pub final_threshold: i32,
    pub replaced_tool_call_ids: Vec<String>,
}

pub fn mild_cascade(
    messages: &mut [Value],
    entries: &[OffloadEntry],
    scan_ratio: f64,
) -> MildResult {
    let map = index(entries);
    let end = ((messages.len() as f64) * scan_ratio).floor() as usize;
    let mut candidates = vec![];
    for (i, m) in messages.iter().take(end).enumerate() {
        if m.get("_offloaded").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = m
            .get("toolCallId")
            .or_else(|| m.get("tool_call_id"))
            .or_else(|| m.get("message").and_then(|x| x.get("toolCallId")))
            .and_then(Value::as_str);
        if let Some((id, e)) = id.and_then(|id| lookup(&map, id).map(|e| (id.to_owned(), e))) {
            candidates.push((i, id, e.score.unwrap_or(5.0)));
        }
    }
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut count = 0;
    let mut ids = vec![];
    let mut final_threshold = 7;
    for threshold in (1..=7).rev() {
        final_threshold = threshold;
        for (i, id, score) in &candidates {
            if *score < threshold as f64
                || messages[*i].get("_offloaded").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            if let Some(e) = lookup(&map, id) {
                let original = messages[*i]
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|s| s.encode_utf16().count())
                    .unwrap_or(0);
                if replace_tool_result(&mut messages[*i], e) {
                    let summary = messages[*i]
                        .get("content")
                        .and_then(Value::as_str)
                        .map(|s| s.encode_utf16().count())
                        .unwrap_or(0);
                    if summary <= original {
                        count += 1;
                        ids.push(id.clone())
                    }
                }
            }
        }
        if count >= 10 {
            break;
        }
    }
    MildResult {
        replaced_count: count,
        final_threshold,
        replaced_tool_call_ids: ids,
    }
}

fn last_user_index(messages: &[Value]) -> Option<usize> {
    messages.iter().rposition(|m| {
        m.get("_mmdContextMessage").is_none()
            && m.get("_mmdInjection").is_none()
            && m.get("role")
                .or_else(|| m.get("message").and_then(|x| x.get("role")))
                .and_then(Value::as_str)
                == Some("user")
    })
}
fn role(m: &Value) -> Option<&str> {
    m.get("role")
        .or_else(|| m.get("message").and_then(|x| x.get("role")))
        .or_else(|| m.get("type"))
        .and_then(Value::as_str)
}
fn active_insert(messages: &[Value]) -> usize {
    if messages.len() <= 2 {
        return 0;
    }
    let half = messages.len() / 2;
    if let Some(u) = last_user_index(messages)
        && u >= half
    {
        return u + 1;
    }
    let mut start = messages.len();
    for i in (0..messages.len()).rev() {
        let m = &messages[i];
        if m.get("_mmdContextMessage").is_some() || m.get("_mmdInjection").is_some() {
            continue;
        }
        if matches!(role(m), Some("toolResult" | "tool" | "assistant")) {
            start = i
        } else {
            break;
        }
    }
    start
        .max(messages.len().saturating_sub(30))
        .min(messages.len() - 1)
}
fn restore_mmd(messages: &mut Vec<Value>, saved: Vec<Value>) {
    for m in saved {
        let idx = if m.get("_mmdContextMessage").and_then(Value::as_str) == Some("history")
            || m.get("_mmdInjection").is_some()
        {
            messages
                .iter()
                .position(|x| x.get("_mmdContextMessage").and_then(Value::as_str) == Some("active"))
                .unwrap_or_else(|| active_insert(messages))
        } else {
            active_insert(messages)
        };
        messages.insert(idx, m)
    }
}
fn extract_mmd(messages: &mut Vec<Value>) -> Vec<Value> {
    let mut out = vec![];
    let mut i = 0;
    while i < messages.len() {
        if messages[i].get("_mmdContextMessage").is_some()
            || messages[i].get("_mmdInjection").is_some()
        {
            out.push(messages.remove(i))
        } else {
            i += 1
        }
    }
    out
}

/// Deterministic TS-compatible aggressive head deletion primitive.
pub fn aggressive_delete(
    messages: &mut Vec<Value>,
    threshold: usize,
    system: &str,
    t: &dyn Tokenizer,
) -> DeleteResult {
    let mut remaining = context_total(messages, system, t);
    if remaining < threshold || messages.len() <= 2 {
        return DeleteResult {
            deleted_count: 0,
            remaining_tokens: remaining,
            deleted_tool_call_ids: vec![],
        };
    }
    let mmd = extract_mmd(messages);
    let need = remaining - threshold;
    let max = messages.len() - 2;
    let mut acc = 0;
    let mut count = 0;
    for m in messages.iter().take(max) {
        acc += message_tokens(m, t);
        count += 1;
        if acc >= need {
            break;
        }
    }
    if let Some(user) = last_user_index(messages) {
        count = count.min(user);
    }
    if count == 0 {
        restore_mmd(messages, mmd);
        return DeleteResult {
            deleted_count: 0,
            remaining_tokens: remaining,
            deleted_tool_call_ids: vec![],
        };
    }
    let removed: Vec<_> = messages.drain(..count).collect();
    let array = Value::Array(removed.clone());
    remaining = remaining.saturating_sub(message_tokens(&array, t));
    let ids = removed.iter().filter_map(first_tool_id).collect();
    restore_mmd(messages, mmd);
    DeleteResult {
        deleted_count: count,
        remaining_tokens: remaining,
        deleted_tool_call_ids: ids,
    }
}

/// Deterministic TS-compatible emergency head deletion for the normal
/// (non-stalled) path, preserving the last two messages.
pub fn emergency_delete(
    messages: &mut Vec<Value>,
    target: usize,
    system: &str,
    t: &dyn Tokenizer,
) -> DeleteResult {
    let mut remaining = context_total(messages, system, t);
    let mmd = extract_mmd(messages);
    let mut deleted = 0;
    let mut ids = vec![];
    while messages.len() > 2 && remaining > target {
        let ratio = ((remaining - target) as f64 / remaining as f64).min(0.5);
        let mut count = ((messages.len() as f64 * ratio).ceil() as usize)
            .max(1)
            .min(messages.len() - 2);
        if let Some(user) = last_user_index(messages) {
            count = count.min(user);
        }
        if count == 0 {
            let (n, tokens, tail_ids) = tail_delete(messages, target, remaining, t);
            deleted += n;
            remaining = remaining.saturating_sub(tokens);
            ids.extend(tail_ids);
            if n == 0 {
                let saved = truncate_largest(messages, t, &mut ids);
                if saved == 0 {
                    break;
                }
                remaining = remaining.saturating_sub(saved);
            }
            continue;
        }
        let removed: Vec<_> = messages.drain(..count).collect();
        remaining = remaining.saturating_sub(message_tokens(&Value::Array(removed.clone()), t));
        deleted += count;
        ids.extend(removed.iter().filter_map(first_tool_id));
    }
    for m in &mmd {
        remaining += message_tokens(m, t);
    }
    restore_mmd(messages, mmd);
    DeleteResult {
        deleted_count: deleted,
        remaining_tokens: remaining,
        deleted_tool_call_ids: ids,
    }
}

fn tool_ids(m: &Value) -> Vec<String> {
    m.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| {
            matches!(
                b.get("type").and_then(Value::as_str),
                Some("tool_use" | "toolCall")
            )
        })
        .filter_map(|b| b.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn first_tool_id(message: &Value) -> Option<String> {
    message_tool_id(message)
        .map(str::to_owned)
        .or_else(|| assistant_tool_ids(message).into_iter().next())
}
fn tail_delete(
    messages: &mut Vec<Value>,
    target: usize,
    current: usize,
    t: &dyn Tokenizer,
) -> (usize, usize, Vec<String>) {
    let mut n = 0;
    let mut tokens = 0;
    let mut ids = vec![];
    while current.saturating_sub(tokens) > target && messages.len() > 2 {
        let last = last_user_index(messages);
        let mut groups: Vec<(Vec<usize>, usize, Vec<String>)> = vec![];
        let mut claimed = std::collections::HashSet::new();
        for i in 1..messages.len() {
            if Some(i) == last || claimed.contains(&i) {
                continue;
            }
            let tu = tool_ids(&messages[i]);
            if !tu.is_empty() && role(&messages[i]) == Some("assistant") {
                let mut ix = vec![i];
                claimed.insert(i);
                for (j, candidate) in messages.iter().enumerate().skip(i + 1) {
                    if claimed.contains(&j) || Some(j) == last {
                        continue;
                    }
                    let id = candidate
                        .get("toolCallId")
                        .or_else(|| messages[j].get("tool_call_id"))
                        .and_then(Value::as_str);
                    if matches!(role(candidate), Some("tool" | "toolResult"))
                        && id.is_some_and(|x| tu.iter().any(|v| v == x))
                    {
                        ix.push(j);
                        claimed.insert(j);
                    }
                }
                let cost = ix.iter().map(|&j| message_tokens(&messages[j], t)).sum();
                groups.push((ix, cost, tu));
            }
        }
        for i in 1..messages.len() {
            if claimed.contains(&i) || Some(i) == last || messages.len() - i <= 1 {
                continue;
            }
            if matches!(
                role(&messages[i]),
                Some("tool" | "toolResult" | "assistant")
            ) {
                let id = messages[i]
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .into_iter()
                    .collect();
                groups.push((vec![i], message_tokens(&messages[i], t), id));
            }
        }
        groups.sort_by_key(|g| std::cmp::Reverse(g.1));
        let Some((ix, cost, gids)) = groups.into_iter().next() else {
            break;
        };
        if messages.len() - ix.len() < 2 {
            break;
        }
        for &i in ix.iter().rev() {
            messages.remove(i);
        }
        n += ix.len();
        tokens += cost;
        ids.extend(gids)
    }
    (n, tokens, ids)
}
fn truncate_largest(messages: &mut [Value], t: &dyn Tokenizer, ids: &mut Vec<String>) -> usize {
    let last = last_user_index(messages);
    let Some((i, before)) = messages
        .iter()
        .enumerate()
        .filter(|(i, m)| {
            Some(*i) != last
                && m.get("_mmdContextMessage").is_none()
                && m.get("_mmdInjection").is_none()
        })
        .map(|(i, m)| (i, message_tokens(m, t)))
        .max_by_key(|x| x.1)
    else {
        return 0;
    };
    if before < 600 {
        return 0;
    }
    let r = role(&messages[i]).unwrap_or("undefined");
    let id = messages[i]
        .get("toolCallId")
        .or_else(|| messages[i].get("tool_call_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let suffix = id
        .as_deref()
        .map(|x| format!(", id={x}"))
        .unwrap_or_default();
    messages[i]["content"] = Value::String(format!(
        "[Tool output truncated for context management. Original ~{before} tokens, role={r}{suffix}]"
    ));
    if let Some(id) = id {
        ids.push(id)
    }
    before.saturating_sub(message_tokens(&messages[i], t))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    None,
    Mild,
    Aggressive,
    Emergency,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionResult {
    pub mode: CompressionMode,
    pub replaced: usize,
    pub deleted: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub replaced_tool_call_ids: Vec<String>,
    pub deleted_tool_call_ids: Vec<String>,
}
fn message_tokens(v: &Value, t: &dyn Tokenizer) -> usize {
    t.count(&serde_json::to_string(v).unwrap_or_default())
}
fn tracker_value(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "_offloaded"
                            | "_mmdContextMessage"
                            | "_mmdInjection"
                            | "_contextOffloadProcessed"
                            | "details"
                    )
                })
                .map(|(k, v)| (k.clone(), tracker_value(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(tracker_value).collect()),
        x => x.clone(),
    }
}
fn context_total(messages: &[Value], system: &str, t: &dyn Tokenizer) -> usize {
    t.count(system)
        + messages
            .iter()
            .map(|m| message_tokens(&tracker_value(m), t))
            .sum::<usize>()
        + (messages.len() as f64 * 0.5).ceil() as usize
}
pub fn compress(
    messages: &mut Vec<Value>,
    entries: &[OffloadEntry],
    _current_nodes: &std::collections::HashSet<String>,
    context_window: usize,
    system: &str,
    cfg: &super::types::OffloadConfig,
    t: &dyn Tokenizer,
) -> CompressionResult {
    let total = |m: &[Value]| {
        t.count(system)
            + m.iter().map(|v| message_tokens(v, t)).sum::<usize>()
            + (m.len() as f64 * 0.5).ceil() as usize
    };
    let before = total(messages);
    let mild = (context_window as f64 * cfg.mild_offload_ratio).floor() as usize;
    let aggressive = (context_window as f64 * cfg.aggressive_compress_ratio).floor() as usize;
    let emergency = (context_window as f64 * cfg.emergency_compress_ratio).floor() as usize;
    let target = (context_window as f64 * cfg.emergency_target_ratio).floor() as usize;
    let mut replaced = 0;
    let mut deleted = 0;
    let mut replaced_tool_call_ids = Vec::new();
    let mut deleted_tool_call_ids = Vec::new();
    let mut mode = CompressionMode::None;
    if before >= aggressive {
        mode = CompressionMode::Aggressive;
        let result = aggressive_delete(messages, aggressive, system, t);
        deleted += result.deleted_count;
        deleted_tool_call_ids.extend(result.deleted_tool_call_ids);
    }
    if total(messages) >= mild {
        if mode == CompressionMode::None {
            mode = CompressionMode::Mild
        }
        let result = mild_cascade(messages, entries, cfg.mild_offload_scan_ratio);
        replaced += result.replaced_count;
        replaced_tool_call_ids.extend(result.replaced_tool_call_ids);
    }
    if total(messages) >= emergency {
        mode = CompressionMode::Emergency;
        let result = emergency_delete(messages, target, system, t);
        deleted += result.deleted_count;
        deleted_tool_call_ids.extend(result.deleted_tool_call_ids);
    }
    CompressionResult {
        mode,
        replaced,
        deleted,
        tokens_before: before,
        tokens_after: total(messages),
        replaced_tool_call_ids,
        deleted_tool_call_ids,
    }
}
pub fn normalize_tool_call_id(id: &str) -> String {
    id.replace('_', "")
}
pub fn lookup<'a>(map: &'a HashMap<String, OffloadEntry>, id: &str) -> Option<&'a OffloadEntry> {
    map.get(id).or_else(|| map.get(&normalize_tool_call_id(id)))
}
pub fn index(entries: &[OffloadEntry]) -> HashMap<String, OffloadEntry> {
    let mut m = HashMap::new();
    for e in entries {
        m.insert(e.tool_call_id.clone(), e.clone());
        m.entry(normalize_tool_call_id(&e.tool_call_id))
            .or_insert_with(|| e.clone());
    }
    m
}
pub fn compact_tool_call(s: &str) -> String {
    if s.chars().count() <= 300 {
        return s.into();
    }
    let mut x = s.chars().take(299).collect::<String>();
    x.push('…');
    x
}
pub fn replace_tool_result(message: &mut Value, entry: &OffloadEntry) -> bool {
    let m = if message.get("message").is_some() {
        &mut message["message"]
    } else {
        &mut *message
    };
    let role = m.get("role").and_then(Value::as_str);
    if !matches!(role, Some("tool") | Some("toolResult")) {
        return false;
    }
    let id = m
        .get("toolCallId")
        .or_else(|| m.get("tool_call_id"))
        .and_then(Value::as_str);
    if id.is_none_or(|x| normalize_tool_call_id(x) != normalize_tool_call_id(&entry.tool_call_id)) {
        return false;
    }
    let summary = format!(
        "[Offloaded Tool Result | node: {}]\nSummary: {}\nresult_ref: {} (read this file for full tool call and raw result)",
        entry.node_id.as_deref().unwrap_or("N/A"),
        entry.summary,
        entry.result_ref
    );
    m["content"] = if m.get("content").is_some_and(Value::is_array) {
        json!([{"type":"text","text":summary}])
    } else {
        Value::String(summary)
    };
    message["_offloaded"] = Value::Bool(true);
    true
}
pub fn replace_assistant_tool_uses(message: &mut Value, entries: &[OffloadEntry]) -> usize {
    let m = if message.get("message").is_some() {
        &mut message["message"]
    } else {
        message
    };
    if m.get("role").and_then(Value::as_str) != Some("assistant") {
        return 0;
    }
    let Some(a) = m.get_mut("content").and_then(Value::as_array_mut) else {
        return 0;
    };
    let map = index(entries);
    let mut n = 0;
    for b in a {
        if !matches!(
            b.get("type").and_then(Value::as_str),
            Some("tool_use") | Some("toolCall")
        ) {
            continue;
        }
        let Some(id) = b.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(e) = lookup(&map, id) else { continue };
        let compact = json!({"_offloaded":true,"node_id":e.node_id.as_deref().unwrap_or("N/A"),"tool_call":compact_tool_call(&e.tool_call)});
        if b.get("arguments").is_some() {
            b["arguments"] = compact
        } else {
            b["input"] = compact
        }
        n += 1
    }
    n
}
