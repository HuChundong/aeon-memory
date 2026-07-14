use aeon_memory_core::tools::{
    conversation_search::{ConversationSearchResult, format_conversation_search_response},
    memory_search::{MemorySearchResult, format_memory_search_response},
};
use aeon_memory_core::utils::{
    no_think_fetch::{
        apply_no_think_strategy, is_valid_disable_thinking, normalize_disable_thinking,
    },
    sanitize,
    session_filter::{SessionFilter, is_non_interactive_trigger},
    time::TimeContext,
};
use chrono::DateTime;
use serde_json::{Value, json};
fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/utils_oracle.json")).unwrap()
}
#[test]
fn sanitize_vectors_match_ts() {
    for c in oracle()["sanitize"].as_array().unwrap() {
        let s = c["text"].as_str().unwrap();
        assert_eq!(sanitize::sanitize_text(s), c["sanitize"]);
        assert_eq!(sanitize::looks_like_prompt_injection(s), c["injection"]);
        assert_eq!(sanitize::should_capture_l0(s), c["l0"]);
        assert_eq!(sanitize::should_extract_l1(s), c["l1"]);
        assert_eq!(sanitize::strip_code_blocks(s), c["strip"]);
        assert_eq!(sanitize::escape_xml_tags(s), c["xml"]);
    }
}
#[test]
fn no_think_all_strategies_and_validation_match_ts() {
    let o = oracle();
    for c in o["noThink"].as_array().unwrap() {
        let mut body =
            json!({"messages":[],"chat_template_kwargs":{"foo":"bar","enable_thinking":true}});
        apply_no_think_strategy(c["strategy"].as_str().unwrap(), &mut body);
        assert_eq!(body, c["body"])
    }
    for c in o["validation"].as_array().unwrap() {
        let v = if c["value"] == "__undefined__" {
            None
        } else {
            Some(&c["value"])
        };
        assert_eq!(
            v.is_some_and(is_valid_disable_thinking),
            c["valid"].as_bool().unwrap()
        );
        assert_eq!(normalize_disable_thinking(v), c["normalized"]);
    }
}
#[test]
fn session_filter_and_env_match_ts() {
    let o = oracle();
    let f = SessionFilter::new(&["bench-judge-*".into(), " agent:blocked:* ".into()]);
    for c in o["sessions"]["keys"].as_array().unwrap() {
        assert_eq!(f.should_skip(c["key"].as_str().unwrap()), c["skip"])
    }
    for c in o["sessions"]["ctx"].as_array().unwrap() {
        let x = &c["ctx"];
        assert_eq!(
            f.should_skip_ctx(
                x["sessionKey"].as_str(),
                x["sessionId"].as_str(),
                x["trigger"].as_str()
            ),
            c["skip"]
        )
    }
    for c in o["sessions"]["triggers"].as_array().unwrap() {
        assert_eq!(
            is_non_interactive_trigger(c["trigger"].as_str(), c["key"].as_str()),
            c["result"]
        )
    }
    unsafe { std::env::set_var("AEON_MEMORY_UTIL_ORACLE", "value") };
    assert_eq!(
        std::env::var("AEON_MEMORY_UTIL_ORACLE").ok(),
        o["env"]["set"].as_str().map(str::to_owned)
    );
    assert!(std::env::var("AEON_MEMORY_UTIL_MISSING").is_err());
}
#[test]
fn time_boundary_table_matches_ts() {
    for c in oracle()["time"].as_array().unwrap() {
        let ctx = TimeContext::new(c["tz"].as_str().unwrap());
        let dt = DateTime::parse_from_rfc3339(c["iso"].as_str().unwrap())
            .unwrap()
            .to_utc();
        assert_eq!(ctx.format_local_date(dt), c["date"]);
        assert_eq!(ctx.format_local_datetime(dt), c["dateTime"]);
        assert_eq!(ctx.start_of_local_day(dt), c["start"].as_i64().unwrap());
        assert_eq!(ctx.format_for_llm(dt), c["llm"]);
    }
}
#[test]
fn tool_response_formatters_are_byte_exact() {
    let o = oracle();
    for c in o["tools"]["memory"].as_array().unwrap() {
        let input: MemorySearchResult = serde_json::from_value(c["input"].clone()).unwrap();
        assert_eq!(
            format_memory_search_response(&input).as_bytes(),
            c["output"].as_str().unwrap().as_bytes()
        )
    }
    for c in o["tools"]["conversation"].as_array().unwrap() {
        let input: ConversationSearchResult = serde_json::from_value(c["input"].clone()).unwrap();
        assert_eq!(
            format_conversation_search_response(&input).as_bytes(),
            c["output"].as_str().unwrap().as_bytes()
        )
    }
}
