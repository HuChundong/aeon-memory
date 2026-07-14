use super::parser::ReplaceBlock;
use crate::{AeonMemoryCoreError, AeonMemoryResult};
pub fn apply_replace_blocks(source: &str, blocks: &[ReplaceBlock]) -> AeonMemoryResult<String> {
    let mut lines: Vec<String> = source.lines().map(Into::into).collect();
    let mut sorted = blocks.to_vec();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.start_line));
    for b in sorted {
        if b.start_line == 0
            || b.end_line.saturating_add(1) < b.start_line
            || b.end_line > lines.len()
        {
            return Err(AeonMemoryCoreError::InvalidInput(format!(
                "invalid mermaid line range {}..{}",
                b.start_line, b.end_line
            )));
        }
        lines.splice(
            b.start_line - 1..b.end_line,
            b.content.lines().map(Into::into),
        );
    }
    Ok(lines.join("\n"))
}
pub fn compact_history_meta(mmd: &str) -> String {
    mmd.lines()
        .filter(|l| l.trim_start().starts_with("%%") || l.contains("[\""))
        .collect::<Vec<_>>()
        .join("\n")
}
