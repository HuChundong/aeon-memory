use aeon_memory_core::utils::session_filter::{SessionFilter, is_non_interactive_trigger};
use serde_json::Value;

#[test]
fn session_filter_all_rules_match_typescript() {
    let o: Value =
        serde_json::from_str(include_str!("fixtures/session_filter_oracle.json")).unwrap();
    let patterns = o["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let f = SessionFilter::new(&patterns);
    for c in o["keys"].as_array().unwrap() {
        assert_eq!(
            f.should_skip(c["key"].as_str().unwrap()),
            c["skip"].as_bool().unwrap(),
            "key={}",
            c["key"]
        );
    }
    for c in o["triggers"].as_array().unwrap() {
        assert_eq!(
            is_non_interactive_trigger(c["trigger"].as_str(), Some("agent:a:normal")),
            c["skip"].as_bool().unwrap()
        );
    }
    for c in o["keyTriggers"].as_array().unwrap() {
        assert_eq!(
            is_non_interactive_trigger(None, c["key"].as_str()),
            c["skip"].as_bool().unwrap()
        );
    }
    for c in o["contexts"].as_array().unwrap() {
        let x = &c["ctx"];
        assert_eq!(
            f.should_skip_ctx(
                x["sessionKey"].as_str(),
                x["sessionId"].as_str(),
                x["trigger"].as_str()
            ),
            c["skip"].as_bool().unwrap()
        );
    }
}
