use aeon_memory_core::offload::reclaim::{ReclaimConfig, reclaim};
use filetime::{FileTime, set_file_times};
use serde_json::{Value, json};
use std::{fs, path::Path};
fn put(p: &Path, data: &[u8], fresh: bool) {
    fs::write(p, data).unwrap();
    let secs = if fresh { 4_102_444_800 } else { 946_684_800 };
    let t = FileTime::from_unix_time(secs, 0);
    set_file_times(p, t, t).unwrap()
}
fn setup(d: &Path) {
    let a = d.join("agent");
    fs::create_dir_all(a.join("refs")).unwrap();
    fs::create_dir_all(a.join("mmds")).unwrap();
    put(&d.join("offload-root.jsonl"), b"{}\n", false);
    put(&a.join("offload-old.jsonl"), b"{}\n", false);
    put(
        &a.join("offload-live.jsonl"),
        b"{\"result_ref\":\"refs/kept.md\"}\n",
        true,
    );
    put(&a.join("refs/kept.md"), b"keep", false);
    put(&a.join("refs/orphan.md"), b"gone", false);
    put(&a.join("refs/ignored.bin"), b"stay", false);
    for i in 0..17 {
        put(&a.join(format!("mmds/{i:02}.mmd")), b"m", i >= 3)
    }
    fs::write(a.join("state.json"), r#"{"activeMmdFile":"00.mmd"}"#).unwrap();
    fs::write(a.join("sessions-registry.json"),r#"{"missing":{"offloadFile":"offload-old.jsonl","updatedAt":"2100-01-01T00:00:00Z"},"expired":{"updatedAt":"2000-01-01T00:00:00Z"},"fresh":{"updatedAt":"2100-01-01T00:00:00Z"}}"#).unwrap();
    put(&d.join("debug.log"), &vec![b'x'; 900000], true);
    put(&d.join("trace.jsonl"), &vec![b'y'; 300000], true);
    put(&d.join("offload-data.jsonl"), &vec![b'z'; 300000], true)
}
fn walk(root: &Path, p: &Path, out: &mut Vec<Value>) {
    for e in fs::read_dir(p).unwrap().flatten() {
        let q = e.path();
        if q.is_dir() {
            walk(root, &q, out)
        } else {
            let path = q
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push(json!({"path": path, "size": q.metadata().unwrap().len()}))
        }
    }
}
#[test]
fn reclaim_stats_and_persisted_tree_match_typescript() {
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/reclaim_oracle.json")).unwrap();
    let d = std::env::temp_dir().join(format!("aeon-memory-reclaim-oracle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    setup(&d);
    let stats = reclaim(
        &d,
        ReclaimConfig {
            retention_days: 3,
            log_max_size_mb: 1,
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(stats).unwrap(), expected["stats"]);
    assert_eq!(
        serde_json::to_value(
            reclaim(
                &d,
                ReclaimConfig {
                    retention_days: 2,
                    log_max_size_mb: 1
                }
            )
            .unwrap()
        )
        .unwrap(),
        expected["disabled"]
    );
    assert_eq!(
        serde_json::to_value(
            reclaim(
                &d.join("absent"),
                ReclaimConfig {
                    retention_days: 3,
                    log_max_size_mb: 1
                }
            )
            .unwrap()
        )
        .unwrap(),
        expected["missing"]
    );
    let mut files = vec![];
    walk(&d, &d, &mut files);
    files.sort_by_key(|v| v["path"].as_str().unwrap().to_owned());
    assert_eq!(json!(files), expected["files"]);
    let registry: Value =
        serde_json::from_slice(&fs::read(d.join("agent/sessions-registry.json")).unwrap()).unwrap();
    assert_eq!(registry, expected["registry"]);
    let _ = fs::remove_dir_all(d);
}
