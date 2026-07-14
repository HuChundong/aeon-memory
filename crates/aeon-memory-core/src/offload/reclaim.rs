use super::storage::read_jsonl_values;
use crate::AeonMemoryResult;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
#[derive(Debug, Clone, Copy)]
pub struct ReclaimConfig {
    pub retention_days: u64,
    pub log_max_size_mb: u64,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReclaimStats {
    pub deleted_jsonl: usize,
    pub deleted_refs: usize,
    pub deleted_mmds: usize,
    pub truncated_logs: usize,
    pub pruned_registry_entries: usize,
}
fn old(p: &Path, cut: SystemTime) -> bool {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t < cut)
}
pub fn reclaim(root: &Path, cfg: ReclaimConfig) -> AeonMemoryResult<ReclaimStats> {
    let mut s = ReclaimStats::default();
    if cfg.retention_days < 3 || !root.exists() {
        return Ok(s);
    }
    let cut = SystemTime::now() - Duration::from_secs(cfg.retention_days * 86400);
    let agents: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    reclaim_jsonl_in(root, cut, &mut s);
    for dir in &agents {
        reclaim_jsonl_in(dir, cut, &mut s);
        let mut refs = HashSet::new();
        for e in fs::read_dir(dir)?.filter_map(Result::ok) {
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("offload-") && n.ends_with(".jsonl") {
                for v in read_jsonl_values(&p)? {
                    if let Some(r) = v
                        .get("result_ref")
                        .and_then(|x| x.as_str())
                        .and_then(|x| Path::new(x).file_name())
                    {
                        refs.insert(r.to_owned());
                    }
                }
            }
        }
        let rd = dir.join("refs");
        if rd.exists() {
            for e in fs::read_dir(rd)?.filter_map(Result::ok) {
                if e.path().extension().is_some_and(|x| x == "md")
                    && !refs.contains(&e.file_name())
                    && old(&e.path(), cut)
                    && fs::remove_file(e.path()).is_ok()
                {
                    s.deleted_refs += 1
                }
            }
        }
        let md = dir.join("mmds");
        if md.exists() {
            let mut files: Vec<_> = fs::read_dir(md)?
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "mmd"))
                .collect();
            files.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            let active = fs::read(dir.join("state.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| {
                    v.get("activeMmdFile")
                        .and_then(|x| x.as_str())
                        .map(str::to_owned)
                });
            let mut remaining = files.len();
            for e in files {
                if remaining <= 15 {
                    break;
                }
                if active.as_deref() != e.file_name().to_str()
                    && old(&e.path(), cut)
                    && fs::remove_file(e.path()).is_ok()
                {
                    s.deleted_mmds += 1;
                    remaining -= 1;
                }
            }
        }
        prune_registry(dir, cut, &mut s);
    }
    if cfg.log_max_size_mb > 0 {
        let max = cfg.log_max_size_mb * 1024 * 1024;
        let mut logs: Vec<_> = fs::read_dir(root)?
            .filter_map(Result::ok)
            .filter(|e| {
                let p = e.path();
                p.extension().is_some_and(|x| x == "log")
                    || (p.extension().is_some_and(|x| x == "jsonl")
                        && !e.file_name().to_string_lossy().starts_with("offload-"))
            })
            .collect();
        let mut total: u64 = logs
            .iter()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        logs.sort_by_key(|e| std::cmp::Reverse(e.metadata().map(|m| m.len()).unwrap_or(0)));
        for e in logs {
            if total <= max {
                break;
            }
            let Ok(len) = e.metadata().map(|m| m.len()) else {
                continue;
            };
            if len == 0 {
                continue;
            }
            if fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(e.path())
                .is_ok()
            {
                total -= len;
                s.truncated_logs += 1
            }
        }
    }
    Ok(s)
}

fn reclaim_jsonl_in(dir: &Path, cut: SystemTime, stats: &mut ReclaimStats) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut deleted = false;
    for e in entries.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("offload-")
            && name.ends_with(".jsonl")
            && old(&e.path(), cut)
            && fs::remove_file(e.path()).is_ok()
        {
            stats.deleted_jsonl += 1;
            deleted = true;
        }
    }
    if deleted {
        sync_registry_files(dir);
    }
}

fn read_registry(path: &Path) -> Option<BTreeMap<String, serde_json::Value>> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn write_registry(path: &Path, value: &BTreeMap<String, serde_json::Value>) {
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    if serde_json::to_vec_pretty(value)
        .ok()
        .is_some_and(|bytes| fs::write(&tmp, bytes).is_ok())
    {
        let _ = fs::rename(tmp, path);
    }
}
fn sync_registry_files(dir: &Path) {
    let path = dir.join("sessions-registry.json");
    let Some(mut registry) = read_registry(&path) else {
        return;
    };
    let before = registry.len();
    registry.retain(|_, v| {
        v.get("offloadFile")
            .and_then(|x| x.as_str())
            .is_none_or(|f| dir.join(f).exists())
    });
    if registry.len() != before {
        write_registry(&path, &registry);
    }
}
fn prune_registry(dir: &Path, cut: SystemTime, stats: &mut ReclaimStats) {
    let path = dir.join("sessions-registry.json");
    let Some(mut registry) = read_registry(&path) else {
        return;
    };
    let before = registry.len();
    registry.retain(|_, v| {
        let Some(s) = v.get("updatedAt").and_then(|x| x.as_str()) else {
            return true;
        };
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .is_none_or(|d| {
                let t: SystemTime = d.with_timezone(&chrono::Utc).into();
                t >= cut
            })
    });
    let removed = before - registry.len();
    if removed > 0 {
        stats.pruned_registry_entries += removed;
        write_registry(&path, &registry);
    }
}
