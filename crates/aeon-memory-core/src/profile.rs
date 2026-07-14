//! Host-neutral L2/L3 profile synchronization (`profile-sync.ts`).
use crate::AeonMemoryResult;
use crate::scene::{
    generate_scene_navigation, read_scene_index, strip_scene_navigation, sync_scene_index,
};
use crate::types::Log;
use crate::types::{ProfileRecord, ProfileSyncRecord, ProfileType};
use md5::{Digest as _, Md5};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

const PROFILE_SCOPE: &str = "global";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileBaseline {
    pub version: i32,
    pub content_md5: String,
    pub created_at_ms: i64,
}
pub trait ProfileStore {
    fn pull_profiles(&self) -> AeonMemoryResult<Vec<ProfileRecord>>;
    fn sync_profiles(&mut self, records: &[ProfileSyncRecord]) -> AeonMemoryResult<()>;
    fn delete_profiles(&mut self, ids: &[String]) -> AeonMemoryResult<()>;
}
pub fn build_profile_stable_id(scope: &str, kind: ProfileType, filename: &str) -> String {
    let t = match kind {
        ProfileType::L2 => "l2",
        ProfileType::L3 => "l3",
    };
    let mut h = Sha256::new();
    h.update(format!("{scope}\0{t}\0{filename}"));
    format!("profile:v1:{:x}", h.finalize())
}
fn md5(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s);
    format!("{:x}", h.finalize())
}
fn stat_times(p: &Path) -> (i64, i64) {
    let now = chrono::Utc::now().timestamp_millis();
    std::fs::metadata(p)
        .map(|m| {
            let c = m
                .created()
                .or_else(|_| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(now);
            let u = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(now);
            (c, u)
        })
        .unwrap_or((now, now))
}
pub fn list_local_profiles(data_dir: &Path) -> Vec<ProfileRecord> {
    let mut out = vec![];
    if let Ok(items) = std::fs::read_dir(data_dir.join("scene_blocks")) {
        let mut v = items
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
            .collect::<Vec<_>>();
        v.sort_by_key(|e| e.file_name());
        for e in v {
            let filename = e.file_name().to_string_lossy().into_owned();
            if let Ok(content) = std::fs::read_to_string(e.path()) {
                let (c, u) = stat_times(&e.path());
                out.push(ProfileRecord {
                    id: build_profile_stable_id(PROFILE_SCOPE, ProfileType::L2, &filename),
                    r#type: ProfileType::L2,
                    filename,
                    content_md5: md5(&content),
                    content,
                    agent_id: None,
                    version: 0,
                    created_at_ms: c,
                    updated_at_ms: u,
                })
            }
        }
    }
    let p = data_dir.join("persona.md");
    if let Ok(raw) = std::fs::read_to_string(&p) {
        let body = strip_scene_navigation(&raw).trim();
        if !body.is_empty() {
            let (c, u) = stat_times(&p);
            out.push(ProfileRecord {
                id: build_profile_stable_id(PROFILE_SCOPE, ProfileType::L3, "persona.md"),
                r#type: ProfileType::L3,
                filename: "persona.md".into(),
                content: body.into(),
                content_md5: md5(body),
                agent_id: None,
                version: 0,
                created_at_ms: c,
                updated_at_ms: u,
            })
        }
    }
    out
}
fn temp_dir(data: &Path) -> AeonMemoryResult<PathBuf> {
    for i in 0..100 {
        let p = data.join(format!(".profiles-pull-{}-{i}", std::process::id()));
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(crate::AeonMemoryCoreError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "temp dir exhausted",
    )))
}
fn refresh_navigation(data: &Path) -> AeonMemoryResult<()> {
    let p = data.join("persona.md");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Ok(());
    };
    let body = strip_scene_navigation(&raw).trim();
    if body.is_empty() {
        return Ok(());
    }
    let nav = generate_scene_navigation(&read_scene_index(data), None);
    std::fs::write(
        p,
        if nav.is_empty() {
            format!("{body}\n")
        } else {
            format!("{body}\n\n{nav}\n")
        },
    )?;
    Ok(())
}
pub fn pull_profiles_to_local(
    data: &Path,
    store: &dyn ProfileStore,
    logger: &dyn Log,
) -> AeonMemoryResult<HashMap<String, ProfileBaseline>> {
    let records = store.pull_profiles()?;
    let mut baseline = HashMap::new();
    let tmp = temp_dir(data)?;
    let blocks = tmp.join("scene_blocks");
    std::fs::create_dir_all(&blocks)?;
    let result = (|| {
        for r in &records {
            baseline.insert(
                r.id.clone(),
                ProfileBaseline {
                    version: r.version,
                    content_md5: r.content_md5.clone(),
                    created_at_ms: r.created_at_ms,
                },
            );
            let target = match r.r#type {
                ProfileType::L2 => blocks.join(&r.filename),
                ProfileType::L3 => tmp.join("persona.md"),
            };
            let content = if matches!(r.r#type, ProfileType::L3) {
                strip_scene_navigation(&r.content).trim()
            } else {
                &r.content
            };
            std::fs::write(&target, content)?;
            if md5(content) != r.content_md5 {
                std::fs::remove_file(target)?;
                logger.debug(&format!(
                    "[aeon-memory][profile-sync] MD5 mismatch for {} (will re-pull on next sync)",
                    r.filename
                ));
            }
        }
        let local = data.join("scene_blocks");
        std::fs::remove_dir_all(&local).or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        })?;
        if !rename_scene_blocks(&blocks, &local, logger)? {
            return Ok(baseline.clone());
        }
        let tp = tmp.join("persona.md");
        let lp = data.join("persona.md");
        if tp.exists() {
            let _ = std::fs::remove_file(&lp);
            rename_persona(&tp, &lp, logger)?;
        } else {
            let _ = std::fs::remove_file(lp);
        }
        sync_scene_index(data)?;
        refresh_navigation(data)?;
        logger.debug(&format!(
            "[aeon-memory][profile-sync] Pulled {} profile(s) to local cache",
            records.len()
        ));
        Ok(baseline.clone())
    })();
    let _ = std::fs::remove_dir_all(tmp);
    result
}

/// Returns `false` when another concurrent pull installed its equivalent snapshot first.
fn rename_scene_blocks(from: &Path, to: &Path, logger: &dyn Log) -> AeonMemoryResult<bool> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(true),
        Err(e) if is_rename_race_error(&e) => {
            logger.debug(&format!(
                "[aeon-memory][profile-sync] scene_blocks rename lost race ({}), using existing",
                rename_error_code(&e)
            ));
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}

fn rename_persona(from: &Path, to: &Path, logger: &dyn Log) -> AeonMemoryResult<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if is_rename_race_error(&e) => {
            logger.debug("[aeon-memory][profile-sync] persona.md rename lost race, using existing");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn is_rename_race_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
    )
}

fn rename_error_code(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::AlreadyExists => "EEXIST".into(),
        std::io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY".into(),
        _ => error
            .raw_os_error()
            .map_or_else(|| "UNKNOWN".into(), |code| code.to_string()),
    }
}
pub fn sync_local_profiles_to_store(
    data: &Path,
    store: &mut dyn ProfileStore,
    baseline: &HashMap<String, ProfileBaseline>,
) -> AeonMemoryResult<()> {
    let local = list_local_profiles(data);
    let ids = local.iter().map(|p| p.id.clone()).collect::<HashSet<_>>();
    let changed = local
        .into_iter()
        .filter(|p| {
            baseline.get(&p.id).map(|b| b.content_md5.as_str()) != Some(p.content_md5.as_str())
        })
        .map(|p| {
            let v = baseline.get(&p.id).map(|b| b.version);
            ProfileSyncRecord {
                profile: p,
                baseline_version: v,
            }
        })
        .collect::<Vec<_>>();
    if !changed.is_empty() {
        store.sync_profiles(&changed)?
    }
    let deleted = baseline
        .keys()
        .filter(|id| !ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !deleted.is_empty() {
        store.delete_profiles(&deleted)?
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{SceneBlockMeta, format_scene_block};
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestLog(Mutex<Vec<String>>);
    impl Log for TestLog {
        fn debug(&self, msg: &str) {
            self.0.lock().unwrap().push(msg.into());
        }
        fn info(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn error(&self, _msg: &str) {}
    }
    #[derive(Default)]
    struct Store {
        pull: Vec<ProfileRecord>,
        synced: Vec<ProfileSyncRecord>,
        deleted: Vec<String>,
    }
    impl ProfileStore for Store {
        fn pull_profiles(&self) -> AeonMemoryResult<Vec<ProfileRecord>> {
            Ok(self.pull.clone())
        }
        fn sync_profiles(&mut self, r: &[ProfileSyncRecord]) -> AeonMemoryResult<()> {
            self.synced = r.to_vec();
            Ok(())
        }
        fn delete_profiles(&mut self, r: &[String]) -> AeonMemoryResult<()> {
            self.deleted = r.to_vec();
            Ok(())
        }
    }
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aeon-memory-profile-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("scene_blocks")).unwrap();
        d
    }
    #[test]
    fn stable_id_golden() {
        assert_eq!(
            build_profile_stable_id("global", ProfileType::L2, "a.md"),
            "profile:v1:adbaf39681cdbd3ff4ea974c4f12857f4c0969cb20b6fd735406cb7a5b1bfb4f"
        )
    }
    #[test]
    fn local_files_are_exact() {
        let d = dir("local-files");
        let c = format_scene_block(
            &SceneBlockMeta {
                created: "c".into(),
                updated: "u".into(),
                summary: "s".into(),
                heat: 1,
            },
            "BODY",
        );
        std::fs::write(d.join("scene_blocks/a.md"), &c).unwrap();
        std::fs::write(
            d.join("persona.md"),
            format!(
                "PERSONA\n\n{}",
                generate_scene_navigation(
                    &[crate::scene::SceneIndexEntry {
                        filename: "a.md".into(),
                        summary: "s".into(),
                        heat: 1,
                        created: "c".into(),
                        updated: "u".into()
                    }],
                    None
                )
            ),
        )
        .unwrap();
        let p = list_local_profiles(&d);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].content, c);
        assert_eq!(p[1].content, "PERSONA")
    }
    #[test]
    fn push_changed_and_deleted() {
        let d = dir("push");
        std::fs::write(d.join("persona.md"), "NEW").unwrap();
        let current = list_local_profiles(&d).pop().unwrap();
        let mut b = HashMap::new();
        b.insert(
            current.id.clone(),
            ProfileBaseline {
                version: 7,
                content_md5: "old".into(),
                created_at_ms: 1,
            },
        );
        b.insert(
            "gone".into(),
            ProfileBaseline {
                version: 2,
                content_md5: "x".into(),
                created_at_ms: 1,
            },
        );
        let mut s = Store::default();
        sync_local_profiles_to_store(&d, &mut s, &b).unwrap();
        assert_eq!(s.synced.len(), 1);
        assert_eq!(s.synced[0].baseline_version, Some(7));
        assert_eq!(s.deleted, vec!["gone"])
    }

    #[test]
    fn scene_blocks_rename_race_is_fail_soft_and_logged() {
        let d = dir("scene-rename-race");
        let staged = d.join("staged");
        let winner = d.join("winner");
        std::fs::create_dir(&staged).unwrap();
        std::fs::write(staged.join("ours.md"), "ours").unwrap();
        std::fs::create_dir(&winner).unwrap();
        std::fs::write(winner.join("theirs.md"), "theirs").unwrap();
        let log = TestLog::default();

        assert!(!rename_scene_blocks(&staged, &winner, &log).unwrap());
        assert_eq!(
            std::fs::read_to_string(winner.join("theirs.md")).unwrap(),
            "theirs"
        );
        let messages = log.0.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("scene_blocks rename lost race"));
        assert!(messages[0].contains("using existing"));
    }
}
