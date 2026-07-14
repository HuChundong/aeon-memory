// port of src/utils/text-utils.ts

/// Truncate text to max_chars, appending suffix if truncated.
pub fn truncate_text(text: &str, max_chars: usize, suffix: &str) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text
        .chars()
        .take(max_chars.saturating_sub(suffix.len()))
        .collect();
    format!("{}{}", truncated.trim_end(), suffix)
}

/// Count approximate tokens in text (quick heuristic).
/// Port of tiktoken-free estimation.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // CJK: ~1 token per 1.7 chars; English: ~1 token per 4 chars
    let (cjk_count, other_count) = text.chars().fold((0usize, 0usize), |(cjk, other), c| {
        let range = c as u32;
        if (0x4E00..=0x9FFF).contains(&range)
            || (0x3400..=0x4DBF).contains(&range)
            || (0x2E80..=0x2FFF).contains(&range)
        {
            (cjk + 1, other)
        } else if !c.is_whitespace() {
            (cjk, other + 1)
        } else {
            (cjk, other)
        }
    });
    (cjk_count as f64 / 1.7 + other_count as f64 / 4.0).ceil() as usize
}

/// Join non-empty lines with separator.
pub fn join_non_empty(lines: &[String], sep: &str) -> String {
    lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join(sep)
}

/// Normalize whitespace: collapse multiple spaces to one, trim.
pub fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate_text("hello", 10, "..."), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate_text("hello world this is long", 15, "...");
        assert!(result.len() <= 18); // 15 + 3
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_english() {
        let tokens = estimate_tokens("hello world this is a test message");
        assert!(tokens > 0);
        assert!(tokens < 20);
    }

    #[test]
    fn test_normalize_whitespace() {
        assert_eq!(normalize_whitespace("  hello   world  "), "hello world");
        assert_eq!(normalize_whitespace("foo\nbar\tbaz"), "foo bar baz");
    }
}
