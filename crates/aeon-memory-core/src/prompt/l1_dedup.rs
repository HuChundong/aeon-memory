// EXACT prompt from TS source, embedded at compile time via generated fixture.
// Regenerate with: node tests/fixtures/gen-golden-fixtures.mjs
pub const L1_DEDUP_SYSTEM_PROMPT: &str = include_str!("../../tests/fixtures/prompt_l1_dedup.txt");

pub fn format_dedup_prompt(existing_content: &str, new_content: &str) -> String {
    format!(
        "【现有记忆】\n{existing}\n\n【待添加的新记忆】\n{new_content}",
        existing = existing_content,
        new_content = new_content,
    )
}
