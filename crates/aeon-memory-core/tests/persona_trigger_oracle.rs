use aeon_memory_core::{
    persona::PersonaTrigger,
    pipeline::checkpoint::{Checkpoint, write_checkpoint},
};
use serde_json::Value;
#[test]
fn persona_trigger_priority_and_filesystem_branches_match_typescript() {
    let o: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/persona_trigger_oracle.json")).unwrap();
    for (i, v) in o.into_iter().enumerate() {
        let d = std::env::temp_dir().join(format!(
            "aeon-memory-trigger-oracle-{}-{i}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        let c = &v["case"];
        let p = &c["cp"];
        let cp = Checkpoint {
            request_persona_update: p["request_persona_update"].as_bool().unwrap_or(false),
            persona_update_reason: p["persona_update_reason"].as_str().unwrap_or("").into(),
            scenes_processed: p["scenes_processed"].as_u64().unwrap_or(0),
            last_persona_at: p["last_persona_at"].as_i64().unwrap_or(0),
            memories_since_last_persona: p["memories_since_last_persona"].as_u64().unwrap_or(0),
            ..Default::default()
        };
        write_checkpoint(d.to_str().unwrap(), &cp).unwrap();
        if c["scene"].as_bool().unwrap() {
            std::fs::create_dir_all(d.join("scene_blocks")).unwrap();
            std::fs::write(d.join("scene_blocks/a.md"), "body").unwrap();
        }
        if let Some(x) = c["persona"].as_str() {
            std::fs::write(d.join("persona.md"), x).unwrap();
        }
        let got = PersonaTrigger {
            data_dir: d.clone(),
            interval: 5,
        }
        .should_generate()
        .unwrap();
        assert_eq!(got.should, v["result"]["should"]);
        assert_eq!(got.reason, v["result"]["reason"]);
        let _ = std::fs::remove_dir_all(d);
    }
}
