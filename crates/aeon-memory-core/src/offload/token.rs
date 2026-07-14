use serde::{Deserialize, Serialize};
use serde_json::Value;
pub trait Tokenizer: Send + Sync {
    fn encoding(&self) -> &str;
    fn count(&self, text: &str) -> usize;
}

#[derive(Debug, Default)]
pub struct HeuristicTokenizer;
impl Tokenizer for HeuristicTokenizer {
    fn encoding(&self) -> &str {
        "heuristic-cjk-1.7-other-4"
    }
    fn count(&self, text: &str) -> usize {
        estimate_tokens(text)
    }
}

/// Exact counterpart of the TypeScript tracker's default `o200k_base` BPE.
#[derive(Debug, Default)]
pub struct O200kTokenizer;
impl Tokenizer for O200kTokenizer {
    fn encoding(&self) -> &str {
        "o200k_base"
    }
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            0
        } else if text.contains("<|endoftext|>") || text.contains("<|endofprompt|>") {
            // js-tiktoken rejects disallowed special tokens and the TS tracker
            // catches that exception, falling back to ceil(JS UTF-16 length/4).
            text.encode_utf16().count().div_ceil(4)
        } else {
            tiktoken_rs::o200k_base_singleton()
                .encode(text, &std::collections::HashSet::new())
                .0
                .len()
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    pub timestamp: String,
    pub stage: String,
    pub encoding: String,
    pub total_tokens: usize,
    pub system_tokens: usize,
    pub messages_tokens: usize,
    pub user_prompt_tokens: usize,
    pub message_count: usize,
}
/// Host-neutral fallback matching the original non-tiktoken heuristic: CJK/1.7, other/4.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let (mut cjk, mut other) = (0, 0);
    for c in text.chars() {
        if matches!(c as u32,0x3400..=0x4dbf|0x4e00..=0x9fff) {
            cjk += 1
        } else {
            other += 1
        }
    }
    ((cjk as f64 / 1.7) + (other as f64 / 4.0)).ceil() as usize
}
fn filtered(v: &Value) -> Value {
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
                .map(|(k, v)| (k.clone(), filtered(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(filtered).collect()),
        x => x.clone(),
    }
}
fn last_user(messages: &[Value]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
        let m = m.get("message").unwrap_or(m);
        if m.get("role")?.as_str()? != "user" {
            return None;
        }
        match m.get("content")? {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => Some(
                a.iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        }
    })
}
pub fn snapshot(
    stage: &str,
    messages: &[Value],
    system: Option<&str>,
    user: Option<&str>,
) -> ContextSnapshot {
    snapshot_with(&O200kTokenizer, stage, messages, system, user)
}
pub fn snapshot_with(
    tokenizer: &dyn Tokenizer,
    stage: &str,
    messages: &[Value],
    system: Option<&str>,
    user: Option<&str>,
) -> ContextSnapshot {
    let system_tokens = tokenizer.count(system.unwrap_or(""));
    let messages_tokens = messages
        .iter()
        .map(|m| tokenizer.count(&serde_json::to_string(&filtered(m)).unwrap_or_default()))
        .sum::<usize>()
        + (messages.len() as f64 * 0.5).ceil() as usize;
    let user_prompt_tokens = match user.filter(|s| !s.trim().is_empty()) {
        Some(u) if last_user(messages).is_none_or(|x| x.trim() != u.trim()) => tokenizer.count(u),
        _ => 0,
    };
    ContextSnapshot {
        timestamp: chrono::Utc::now().to_rfc3339(),
        stage: stage.into(),
        encoding: tokenizer.encoding().into(),
        total_tokens: system_tokens + messages_tokens + user_prompt_tokens,
        system_tokens,
        messages_tokens,
        user_prompt_tokens,
        message_count: messages.len(),
    }
}
