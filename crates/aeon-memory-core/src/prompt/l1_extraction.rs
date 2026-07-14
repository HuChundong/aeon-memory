// EXACT prompt from TS source, embedded at compile time via generated fixture.
// Regenerate with: node tests/fixtures/gen-golden-fixtures.mjs
// The trailing newline from include_str! is intentional to match TS source.
pub const EXTRACT_MEMORIES_SYSTEM_PROMPT: &str = {
    const S: &str = include_str!("../../tests/fixtures/prompt_l1_extraction.txt");
    S
};

/// Format the extraction user prompt with the given messages and context.
/// Port of formatExtractionPrompt() from l1-extraction.ts:115-145
pub fn format_extraction_prompt(
    new_messages: &[crate::record::l0_recorder::ConversationMessage],
    background_messages: &[crate::record::l0_recorder::ConversationMessage],
    previous_scene_name: &str,
    timezone_desc: &str,
) -> String {
    let format_timestamp = |timestamp: i64| {
        chrono::DateTime::from_timestamp_millis(timestamp).map_or_else(
            || timestamp.to_string(),
            |value| crate::utils::time::format_for_llm(&value.to_rfc3339()),
        )
    };
    let bg_text = if background_messages.is_empty() {
        "无".to_string()
    } else {
        background_messages
            .iter()
            .map(|m| {
                format!(
                    "[{}] [{}] [{}]: {}",
                    m.id,
                    m.role,
                    format_timestamp(m.timestamp),
                    m.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let new_text = new_messages
        .iter()
        .map(|m| {
            format!(
                "[{}] [{}] [{}]: {}",
                m.id,
                m.role,
                format_timestamp(m.timestamp),
                m.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        r#"**{tz}**

**输出语言**：根据下方"待提取的新消息"中 user 发言的主导语言书写 `scene_name` 和 memory `content`。

【上一个情境】：{prev}

【背景对话】（仅供理解上下文推断关系/时间，严禁从中提取记忆）：
{bg}

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

【待提取的新消息】（务必结合 timestamp 推算时间，只从这里提取记忆！）：
{new}"#,
        tz = timezone_desc,
        prev = previous_scene_name,
        bg = bg_text,
        new = new_text,
    )
}
