use aeon_memory_core::offload::token::{O200kTokenizer, Tokenizer, snapshot};
use serde_json::Value;

fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/l3_token_oracle.json")).unwrap()
}

#[test]
fn o200k_fixed_seed_property_corpus_matches_typescript() {
    let tokenizer = O200kTokenizer;
    for case in oracle()["strings"].as_array().unwrap() {
        assert_eq!(
            tokenizer.count(case["text"].as_str().unwrap()),
            case["count"].as_u64().unwrap() as usize,
            "{case}"
        );
    }
}

#[test]
fn token_snapshots_match_typescript_field_by_field() {
    for case in oracle()["snapshots"].as_array().unwrap() {
        let input = &case["input"];
        let messages = input["messages"].as_array().unwrap();
        let actual = snapshot(
            input["stage"].as_str().unwrap(),
            messages,
            input["system"].as_str(),
            input["user"].as_str(),
        );
        let expected = &case["output"];
        assert_eq!(actual.stage, expected["stage"]);
        assert_eq!(actual.encoding, expected["encoding"]);
        assert_eq!(
            actual.total_tokens,
            expected["totalTokens"].as_u64().unwrap() as usize
        );
        assert_eq!(
            actual.system_tokens,
            expected["systemTokens"].as_u64().unwrap() as usize
        );
        assert_eq!(
            actual.messages_tokens,
            expected["messagesTokens"].as_u64().unwrap() as usize
        );
        assert_eq!(
            actual.user_prompt_tokens,
            expected["userPromptTokens"].as_u64().unwrap() as usize
        );
        assert_eq!(
            actual.message_count,
            expected["messageCount"].as_u64().unwrap() as usize
        );
    }
}
