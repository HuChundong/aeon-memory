//! L2 scene files. This is a direct, host-neutral port of `src/core/scene/*`.

use crate::pipeline::checkpoint::{mutate_checkpoint, read_checkpoint};
use crate::types::{FileToolPolicy, LlmRunParams, LlmRunner};
use crate::{AeonMemoryCoreError, AeonMemoryResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const META_START: &str = "-----META-START-----";
const META_END: &str = "-----META-END-----";
pub const NAV_HEADER: &str = "---\n## 🗺️ Scene Navigation (Scene Index)";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneBlockMeta {
    pub created: String,
    pub updated: String,
    pub summary: String,
    pub heat: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneBlock {
    pub filename: String,
    pub meta: SceneBlockMeta,
    pub content: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneIndexEntry {
    pub filename: String,
    pub summary: String,
    pub heat: i64,
    pub created: String,
    pub updated: String,
}

pub fn parse_scene_block(raw: &str, filename: &str) -> SceneBlock {
    let (Some(start), Some(end)) = (raw.find(META_START), raw.find(META_END)) else {
        return SceneBlock {
            filename: filename.into(),
            meta: SceneBlockMeta::default(),
            content: raw.trim().into(),
        };
    };
    let meta = raw[start + META_START.len()..end].trim();
    let field = |name: &str| {
        meta.lines()
            .find_map(|line| line.strip_prefix(&format!("{name}:")))
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    SceneBlock {
        filename: filename.into(),
        meta: SceneBlockMeta {
            created: field("created"),
            updated: field("updated"),
            summary: field("summary"),
            heat: field("heat").parse().unwrap_or(0),
        },
        content: raw[end + META_END.len()..].trim().into(),
    }
}

pub fn format_meta(meta: &SceneBlockMeta) -> String {
    format!(
        "{META_START}\ncreated: {}\nupdated: {}\nsummary: {}\nheat: {}\n{META_END}",
        meta.created, meta.updated, meta.summary, meta.heat
    )
}
pub fn format_scene_block(meta: &SceneBlockMeta, content: &str) -> String {
    format!("{}\n\n{}", format_meta(meta), content)
}

pub fn read_scene_index(data_dir: &Path) -> Vec<SceneIndexEntry> {
    let Ok(raw) = std::fs::read_to_string(data_dir.join(".metadata/scene_index.json")) else {
        return vec![];
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return vec![];
    };
    let Some(items) = value.as_array() else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .filter(|e: &SceneIndexEntry| !e.filename.is_empty())
        .collect()
}
pub fn write_scene_index(data_dir: &Path, entries: &[SceneIndexEntry]) -> AeonMemoryResult<()> {
    let path = data_dir.join(".metadata/scene_index.json");
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(entries)?)?;
    Ok(())
}
pub fn sync_scene_index(data_dir: &Path) -> AeonMemoryResult<Vec<SceneIndexEntry>> {
    let mut entries = vec![];
    if let Ok(files) = std::fs::read_dir(data_dir.join("scene_blocks")) {
        for item in files.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".md") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(item.path()) else {
                continue;
            };
            let b = parse_scene_block(&raw, &name);
            entries.push(SceneIndexEntry {
                filename: name,
                summary: b.meta.summary,
                heat: b.meta.heat,
                created: b.meta.created,
                updated: b.meta.updated,
            });
        }
    }
    write_scene_index(data_dir, &entries)?;
    Ok(entries)
}

fn heat_emoji(heat: i64) -> &'static str {
    match heat {
        1000.. => " 🔥🔥🔥🔥🔥",
        500.. => " 🔥🔥🔥🔥",
        200.. => " 🔥🔥🔥",
        100.. => " 🔥🔥",
        50.. => " 🔥",
        _ => "",
    }
}
pub fn generate_scene_navigation(entries: &[SceneIndexEntry], data_dir: Option<&Path>) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.heat));
    let blocks = sorted
        .iter()
        .map(|e| {
            let p = data_dir
                .map(|d| {
                    d.join("scene_blocks")
                        .join(&e.filename)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .unwrap_or_else(|| format!("scene_blocks/{}", e.filename));
            format!(
                "### Path: {p}\n**热度**: {}{}{}\nSummary: {}",
                e.heat,
                heat_emoji(e.heat),
                if e.updated.is_empty() {
                    String::new()
                } else {
                    format!(" | **更新**: {}", e.updated)
                },
                e.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "{NAV_HEADER}\n*以下是当前场景记忆的索引，可根据需要 read_file 读取详细内容。*\n\n{blocks}\n\n📌 使用说明：\n- Path 是 scene block 的绝对路径，可直接使用 read_file 读取完整内容\n- 热度：该场景被记忆命中的累计次数，越高越重要\n- Summary：场景的核心要点摘要"
    )
}
pub fn strip_scene_navigation(s: &str) -> &str {
    s.find(NAV_HEADER).map_or(s, |i| s[..i].trim_end())
}

pub fn normalize_scene_filename(name: &str) -> String {
    if name.is_empty() {
        return "scene.md".into();
    }
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = if base.to_ascii_lowercase().ends_with(".md") {
        &base[..base.len() - 3]
    } else {
        base
    };
    let forbidden = "()[]{}<>'\"`,;:!?*|/\\=&%$#@^~+";
    let mut out = String::new();
    let mut last_sep = None;
    for c in stem.chars() {
        let c = if c.is_whitespace() || c == '\u{a0}' || c == '\u{3000}' {
            '-'
        } else {
            c
        };
        if forbidden.contains(c) {
            continue;
        }
        if matches!(c, '-' | '_' | '.') {
            if last_sep == Some(c) {
                continue;
            }
            last_sep = Some(c);
        } else {
            last_sep = None;
        }
        out.push(c);
    }
    let safe = out.trim_matches(['-', '_', '.']);
    format!("{}.md", if safe.is_empty() { "scene" } else { safe })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NormalizeRenameResult {
    pub renamed: usize,
    pub skipped: usize,
    pub renames: Vec<(String, String)>,
}
pub fn resolve_unique_scene_path(
    dir: &Path,
    desired: &str,
    exclude: Option<&Path>,
) -> AeonMemoryResult<PathBuf> {
    let target = dir.join(desired);
    if !target.exists() || exclude == Some(target.as_path()) {
        return Ok(target);
    }
    let stem = desired.strip_suffix(".md").unwrap_or(desired);
    for i in 2..1000 {
        let p = dir.join(format!("{stem}-{i}.md"));
        if !p.exists() || exclude == Some(p.as_path()) {
            return Ok(p);
        }
    }
    Err(AeonMemoryCoreError::InvalidInput(format!(
        "could not find a free slot for {desired} after 1000 attempts"
    )))
}
pub fn normalize_scene_filenames(dir: &Path) -> NormalizeRenameResult {
    let mut r = NormalizeRenameResult::default();
    let Ok(items) = std::fs::read_dir(dir) else {
        return r;
    };
    for item in items.flatten() {
        let old = item.file_name().to_string_lossy().into_owned();
        if !old.ends_with(".md") {
            continue;
        }
        let new = normalize_scene_filename(&old);
        if new == old {
            r.skipped += 1;
            continue;
        }
        let Ok(to) = resolve_unique_scene_path(dir, &new, Some(&item.path())) else {
            r.skipped += 1;
            continue;
        };
        if std::fs::rename(item.path(), &to).is_ok() {
            r.renamed += 1;
            r.renames
                .push((old, to.file_name().unwrap().to_string_lossy().into_owned()));
        }
    }
    r
}

pub fn cleanup_scene_files(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(items) = std::fs::read_dir(dir) else {
        return n;
    };
    for item in items.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") {
            continue;
        }
        if let Ok(raw) = std::fs::read_to_string(item.path()) {
            let b = parse_scene_block(&raw, &name);
            if (raw.trim().is_empty() || raw.trim() == "[DELETED]" || b.content.trim().is_empty())
                && std::fs::remove_file(item.path()).is_ok()
            {
                n += 1
            }
        }
    }
    n
}

pub fn parse_persona_update_signal(text: &str) -> Option<String> {
    if let Some(start) = text.find("[PERSONA_UPDATE_REQUEST]") {
        let rest = &text[start + 24..];
        if let Some(end) = rest.find("[/PERSONA_UPDATE_REQUEST]") {
            return Some(
                rest[..end]
                    .trim()
                    .strip_prefix("reason:")
                    .unwrap_or(rest[..end].trim())
                    .trim()
                    .into(),
            );
        }
    }
    text.lines().find_map(|l| {
        l.split_once("PERSONA_UPDATE_REQUEST:")
            .map(|(_, r)| r.trim().to_string())
    })
}

#[derive(Clone, Debug)]
pub struct SceneMemory {
    pub content: String,
    pub created_at: String,
    pub id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractionResult {
    pub memories_processed: usize,
    pub success: bool,
    pub error: Option<String>,
}
pub struct SceneExtractor<'a> {
    pub data_dir: PathBuf,
    pub runner: &'a dyn LlmRunner,
    pub max_scenes: usize,
    pub backup_count: usize,
    pub timeout_ms: u64,
}
impl SceneExtractor<'_> {
    pub async fn extract(&self, memories: &[SceneMemory]) -> ExtractionResult {
        if memories.is_empty() {
            return ExtractionResult {
                memories_processed: 0,
                success: true,
                error: None,
            };
        }
        let blocks = self.data_dir.join("scene_blocks");
        if let Err(e) = std::fs::create_dir_all(&blocks) {
            return failed(e);
        }
        let _ = std::fs::create_dir_all(self.data_dir.join(".metadata"));
        let cp = read_checkpoint(self.data_dir.to_str().unwrap()).unwrap_or_default();
        let _ = backup_scene_dir(
            &blocks,
            &self.data_dir.join(".backup"),
            cp.total_processed,
            self.backup_count,
        )
        .ok()
        .flatten();
        let index = read_scene_index(&self.data_dir);
        let summaries = build_scene_summaries(&index, self.max_scenes);
        let warning = scene_count_warning(index.len(), self.max_scenes);
        let memories_json=serde_json::to_string_pretty(&memories.iter().map(|m|serde_json::json!({"content":m.content,"created_at":m.created_at,"id":m.id.as_deref().unwrap_or("")})).collect::<Vec<_>>()).unwrap();
        let (system, prompt) = build_scene_extraction_prompt(
            &memories_json,
            if summaries.is_empty() {
                "(无已有场景)"
            } else {
                &summaries
            },
            warning.as_deref(),
            &index.iter().map(|e| e.filename.clone()).collect::<Vec<_>>(),
            self.max_scenes,
        );
        let output = self
            .runner
            .run(LlmRunParams {
                prompt,
                system_prompt: Some(system),
                task_id: format!("scene-extract-{}", chrono::Utc::now().timestamp_millis()),
                timeout_ms: Some(self.timeout_ms),
                max_tokens: None,
                workspace_dir: Some(blocks.to_string_lossy().into_owned()),
                file_tool_policy: Some(FileToolPolicy::Scene {
                    readable_files: index.iter().map(|entry| entry.filename.clone()).collect(),
                }),
                instance_id: None,
            })
            .await;
        let output = match output {
            Ok(v) => v,
            Err(e) => {
                // BackupManager.restoreLatestDirectory() is invoked even when
                // this run's source directory was empty (and therefore made
                // no new backup), so an older last-known-good snapshot may be
                // restored.
                let _ = restore_latest_scene_dir(&self.data_dir.join(".backup"), &blocks);
                return ExtractionResult {
                    memories_processed: 0,
                    success: false,
                    error: Some(match e {
                        AeonMemoryCoreError::Config(s)
                        | AeonMemoryCoreError::Store(s)
                        | AeonMemoryCoreError::Llm(s)
                        | AeonMemoryCoreError::Http(s)
                        | AeonMemoryCoreError::NotFound(s)
                        | AeonMemoryCoreError::InvalidInput(s) => s,
                        other => other.to_string(),
                    }),
                };
            }
        };
        cleanup_scene_files(&blocks);
        normalize_scene_filenames(&blocks);
        if let Err(e) = sync_scene_index(&self.data_dir) {
            return failed(e);
        }
        let _ = refresh_persona_navigation(&self.data_dir);
        if let Some(reason) = parse_persona_update_signal(&output) {
            let _ = mutate_checkpoint(self.data_dir.to_str().unwrap(), |c| {
                c.request_persona_update = true;
                c.persona_update_reason = reason;
            });
        }
        ExtractionResult {
            memories_processed: memories.len(),
            success: true,
            error: None,
        }
    }
}
fn failed(e: impl std::fmt::Display) -> ExtractionResult {
    ExtractionResult {
        memories_processed: 0,
        success: false,
        error: Some(e.to_string()),
    }
}
fn scene_count_warning(n: usize, max: usize) -> Option<String> {
    if n >= max {
        Some(format!(
            "当前场景数量为 **{n} 个**，已达到或超过 {max} 个上限！\n**你必须先执行 MERGE 操作**，将最相似的 2-4 个场景合并为 1 个，然后再处理新记忆。\n参考合并对象：热度最低或主题高度重叠的场景。"
        ))
    } else if n == max.saturating_sub(1) {
        Some(format!(
            "当前场景数量为 **{n} 个**，距离上限只差 1 个！\n本次处理**只能 UPDATE 现有场景，不能 CREATE 新场景**。"
        ))
    } else if n >= max.saturating_sub(3) {
        Some(format!(
            "当前场景数量为 **{n} 个**，建议优先考虑 UPDATE 或主动 MERGE 相似场景。"
        ))
    } else {
        None
    }
}
fn build_scene_summaries(index: &[SceneIndexEntry], max: usize) -> String {
    if index.is_empty() {
        return String::new();
    }
    let mut s = format!("**当前场景总数：{} / {max}**\n\n", index.len());
    for e in index {
        s.push_str(&format!(
            "### {}\n**热度**: {} | **更新**: {}\n**summary**: {}\n\n",
            e.filename, e.heat, e.updated, e.summary
        ))
    }
    s
}
pub fn build_scene_extraction_prompt(
    memories_json: &str,
    summaries: &str,
    warning: Option<&str>,
    files: &[String],
    max: usize,
) -> (String, String) {
    let system = include_str!("prompt/resources/scene_extraction_template.txt")
        .replace("987654", &max.to_string())
        .replace("987653", &max.saturating_sub(1).to_string());
    let warning = warning
        .map(|w| format!("\n## ⚠️ 场景数量预警\n{w}\n"))
        .unwrap_or_default();
    let user = format!(
        "## Current Timestamp\n{}\n\n## Existing Scene Blocks Summary\n{}\n{}\n## 已有场景文件清单（只允许读取这些文件）\n{}\n\n## New Memories\n```json\n{}\n```",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
        summaries,
        warning,
        if files.is_empty() {
            "(无)".into()
        } else {
            files
                .iter()
                .map(|f| format!("- `{f}`"))
                .collect::<Vec<_>>()
                .join("\n")
        },
        memories_json
    );
    (system, user)
}
fn backup_scene_dir(
    src: &Path,
    root: &Path,
    offset: u64,
    keep: usize,
) -> AeonMemoryResult<Option<PathBuf>> {
    if !src.exists() {
        return Ok(None);
    }
    // TypeScript's BackupManager skips empty directories and stores backups
    // under a category directory.  Keep the same shallow-file semantics.
    let has_files = std::fs::read_dir(src)?
        .flatten()
        .any(|e| e.path().is_file());
    if !has_files {
        return Ok(None);
    }
    let category = root.join("scene_blocks");
    std::fs::create_dir_all(&category)?;
    let p = category.join(format!(
        "scene_blocks_{}_offset{offset}",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ));
    copy_dir(src, &p)?;
    let mut v = std::fs::read_dir(&category)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect::<Vec<_>>();
    v.sort_by_key(|e| e.file_name());
    if keep > 0 {
        while v.len() > keep {
            std::fs::remove_dir_all(v.remove(0).path())?
        }
    }
    Ok(Some(p))
}
fn copy_dir(src: &Path, dst: &Path) -> AeonMemoryResult<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        if e.path().is_file() {
            std::fs::copy(e.path(), dst.join(e.file_name()))?;
        }
    }
    Ok(())
}
fn restore_scene_dir(src: &Path, dst: &Path) -> AeonMemoryResult<()> {
    let _ = std::fs::remove_dir_all(dst);
    copy_dir(src, dst)
}
fn restore_latest_scene_dir(root: &Path, dst: &Path) -> AeonMemoryResult<bool> {
    let category = root.join("scene_blocks");
    let mut backups = match std::fs::read_dir(category) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect::<Vec<_>>(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    backups.sort_by_key(|e| e.file_name());
    let Some(latest) = backups.last() else {
        return Ok(false);
    };
    restore_scene_dir(&latest.path(), dst)?;
    Ok(true)
}
pub fn refresh_persona_navigation(data: &Path) -> AeonMemoryResult<()> {
    let p = data.join("persona.md");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Ok(());
    };
    let body = strip_scene_navigation(&raw).trim_end();
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn format_golden() {
        let m = SceneBlockMeta {
            created: "a".into(),
            updated: "b".into(),
            summary: "s".into(),
            heat: 7,
        };
        let s = format_scene_block(&m, "# body");
        assert_eq!(
            s,
            "-----META-START-----\ncreated: a\nupdated: b\nsummary: s\nheat: 7\n-----META-END-----\n\n# body"
        );
        assert_eq!(parse_scene_block(&s, "x.md").meta, m)
    }
    #[test]
    fn names_exact() {
        for (a, b) in [
            ("Daily Rhythm in Shanghai.md", "Daily-Rhythm-in-Shanghai.md"),
            ("日常生活 健康管理.md", "日常生活-健康管理.md"),
            ("Coffee (Yirgacheffe).md", "Coffee-Yirgacheffe.md"),
            ("  spaced  .md", "spaced.md"),
            (".MD", "scene.md"),
            ("已经规范.md", "已经规范.md"),
        ] {
            assert_eq!(normalize_scene_filename(a), b)
        }
    }
    #[test]
    fn signals() {
        assert_eq!(
            parse_persona_update_signal(
                "[PERSONA_UPDATE_REQUEST]\nreason: changed\n[/PERSONA_UPDATE_REQUEST]"
            )
            .as_deref(),
            Some("changed")
        );
        assert_eq!(
            parse_persona_update_signal("PERSONA_UPDATE_REQUEST: inline\nx").as_deref(),
            Some("inline")
        )
    }

    struct FileWritingMock;
    #[async_trait::async_trait]
    impl LlmRunner for FileWritingMock {
        async fn run(&self, p: LlmRunParams) -> AeonMemoryResult<String> {
            assert_eq!(p.timeout_ms, Some(321));
            assert_eq!(
                p.system_prompt.as_deref(),
                Some(include_str!("../tests/fixtures/prompt_scene_system_15.txt"))
            );
            let dir = PathBuf::from(p.workspace_dir.unwrap());
            std::fs::write(
                dir.join("Daily Rhythm!.md"),
                "-----META-START-----\ncreated: c\nupdated: u\nsummary: exact summary\nheat: 1\n-----META-END-----\n\n# Narrative",
            )?;
            Ok("PERSONA_UPDATE_REQUEST: cross-scene insight".into())
        }
    }

    #[tokio::test]
    async fn extractor_file_golden() {
        let dir = std::env::temp_dir().join("aeon-memory-scene-extractor-golden");
        let _ = std::fs::remove_dir_all(&dir);
        let extractor = SceneExtractor {
            data_dir: dir.clone(),
            runner: &FileWritingMock,
            max_scenes: 15,
            backup_count: 10,
            timeout_ms: 321,
        };
        let result = extractor
            .extract(&[SceneMemory {
                content: "m".into(),
                created_at: "t".into(),
                id: Some("i".into()),
            }])
            .await;
        assert_eq!(
            result,
            ExtractionResult {
                memories_processed: 1,
                success: true,
                error: None
            }
        );
        let expected = "-----META-START-----\ncreated: c\nupdated: u\nsummary: exact summary\nheat: 1\n-----META-END-----\n\n# Narrative";
        assert_eq!(
            std::fs::read_to_string(dir.join("scene_blocks/Daily-Rhythm.md")).unwrap(),
            expected
        );
        assert_eq!(
            read_scene_index(&dir),
            vec![SceneIndexEntry {
                filename: "Daily-Rhythm.md".into(),
                summary: "exact summary".into(),
                heat: 1,
                created: "c".into(),
                updated: "u".into()
            }]
        );
        let cp = read_checkpoint(dir.to_str().unwrap()).unwrap();
        assert!(cp.request_persona_update);
        assert_eq!(cp.persona_update_reason, "cross-scene insight");
    }

    #[test]
    fn backup_layout_pruning_and_unlimited_match_backup_manager() {
        let dir = std::env::temp_dir().join(format!(
            "aeon-memory-scene-backup-manager-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("scene_blocks");
        let root = dir.join(".backup");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.md"), "a").unwrap();
        for name in [
            "scene_blocks_20260101_000000_offset0",
            "scene_blocks_20260101_000001_offset1",
        ] {
            let p = root.join("scene_blocks").join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("a.md"), "old").unwrap();
        }
        let made = backup_scene_dir(&src, &root, 2, 2).unwrap().unwrap();
        assert!(made.starts_with(root.join("scene_blocks")));
        assert_eq!(
            std::fs::read_dir(root.join("scene_blocks"))
                .unwrap()
                .count(),
            2
        );
        backup_scene_dir(&src, &root, 3, 0).unwrap();
        assert_eq!(
            std::fs::read_dir(root.join("scene_blocks"))
                .unwrap()
                .count(),
            3
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
