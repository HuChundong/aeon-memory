// port of src/utils/no-think-fetch.ts
// Provider-specific strategies for disabling LLM thinking/reasoning.

use serde_json::Value;

pub fn is_valid_disable_thinking(value: &Value) -> bool {
    value == &Value::Bool(false)
        || value.as_str().is_some_and(|s| {
            matches!(
                s,
                "vllm" | "deepseek" | "dashscope" | "openai" | "anthropic" | "kimi" | "gemini"
            )
        })
}
pub fn normalize_disable_thinking(value: Option<&Value>) -> Value {
    match value {
        None | Some(Value::Bool(false)) => Value::Bool(false),
        Some(Value::Bool(true)) => Value::String("vllm".into()),
        Some(v) if is_valid_disable_thinking(v) => v.clone(),
        _ => Value::Bool(false),
    }
}

/// Apply the no-think strategy to a chat completion request body.
/// Port of STRATEGY_TRANSFORMERS in no-think-fetch.ts.
/// Only modifies bodies with a "messages" array (chat completions).
pub fn apply_no_think_strategy(strategy: &str, body: &mut Value) {
    if !body.is_object() {
        return;
    }
    if body.get("messages").and_then(|m| m.as_array()).is_none() {
        return;
    }
    match strategy {
        "vllm" => apply_vllm(body),
        "deepseek" => apply_deepseek(body),
        "dashscope" => apply_dashscope(body),
        "openai" => apply_openai(body),
        "anthropic" | "kimi" => apply_anthropic(body),
        "gemini" => apply_gemini(body),
        _ => {} // unknown strategy: passthrough
    }
}

fn apply_vllm(body: &mut Value) {
    let existing = body
        .get("chat_template_kwargs")
        .and_then(|v| v.as_object())
        .cloned();
    let mut kwargs = existing.unwrap_or_default();
    kwargs.insert("enable_thinking".to_string(), Value::Bool(false));
    body.as_object_mut()
        .unwrap()
        .insert("chat_template_kwargs".to_string(), Value::Object(kwargs));
}

fn apply_deepseek(body: &mut Value) {
    body.as_object_mut()
        .unwrap()
        .insert("enable_thinking".to_string(), Value::Bool(false));
}

fn apply_dashscope(body: &mut Value) {
    body.as_object_mut()
        .unwrap()
        .insert("enable_thinking".to_string(), Value::Bool(false));
}

fn apply_openai(body: &mut Value) {
    body.as_object_mut().unwrap().insert(
        "reasoning_effort".to_string(),
        Value::String("low".to_string()),
    );
}

fn apply_anthropic(body: &mut Value) {
    body.as_object_mut().unwrap().insert(
        "thinking".to_string(),
        serde_json::json!({"type": "disabled"}),
    );
}

fn apply_gemini(body: &mut Value) {
    body.as_object_mut().unwrap().insert(
        "thinking_config".to_string(),
        serde_json::json!({"thinking_budget": 0}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_vllm() {
        let mut body = json!({"messages": [{"role": "user", "content": "hi"}]});
        apply_no_think_strategy("vllm", &mut body);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn test_deepseek() {
        let mut body = json!({"messages": [{"role": "user", "content": "hi"}]});
        apply_no_think_strategy("deepseek", &mut body);
        assert_eq!(body["enable_thinking"], false);
    }

    #[test]
    fn test_anthropic() {
        let mut body = json!({"messages": [{"role": "user", "content": "hi"}]});
        apply_no_think_strategy("anthropic", &mut body);
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn test_non_chat_passthrough() {
        let mut body = json!({"input": "test", "model": "test"});
        apply_no_think_strategy("vllm", &mut body);
        // Should have no chat_template_kwargs added (no messages array)
        assert!(body.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn test_unknown_strategy() {
        let mut body = json!({"messages": [{"role": "user", "content": "hi"}]});
        let original = body.clone();
        apply_no_think_strategy("unknown", &mut body);
        assert_eq!(body, original);
    }
}
