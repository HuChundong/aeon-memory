use aeon_memory_core::utils::memory_cleaner::run_once;
use chrono::TimeZone;
use serde_json::Value;
#[test]
fn filesystem_retention_matches_typescript() {
    let o: Value = serde_json::from_str(include_str!("fixtures/cleaner_oracle.json")).unwrap();
    let d = std::env::temp_dir().join(format!("aeon-memory-cleaner-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for sub in ["conversations", "records"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
        for f in [
            "2026-07-10.jsonl",
            "2026-07-11.json",
            "2026-07-12.jsonl",
            "notes.jsonl",
            "bad.txt",
        ] {
            std::fs::write(d.join(sub).join(f), f).unwrap();
        }
    }
    let s = run_once(
        &d,
        2,
        chrono::Local
            .with_ymd_and_hms(2026, 7, 13, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis(),
        None,
    )
    .unwrap();
    assert_eq!(s.scanned_files, 8);
    assert_eq!(s.changed_files, 4);
    assert_eq!(s.skipped_non_shard_files, 2);
    for sub in ["conversations", "records"] {
        let mut files = std::fs::read_dir(d.join(sub))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(serde_json::to_value(files).unwrap(), o["tree"][sub]);
    }
    let _ = std::fs::remove_dir_all(d);
}
