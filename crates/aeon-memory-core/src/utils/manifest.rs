// port of src/utils/manifest.ts

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Store binding info — port of ManifestStoreInfo from manifest.ts:22-34
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestStoreInfo {
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<ManifestSqliteInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcvdb: Option<ManifestTcvdbInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSqliteInfo {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestTcvdbInfo {
    pub url: String,
    pub database: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

/// Seed run info — port of ManifestSeedInfo from manifest.ts:36-44
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSeedInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_file: Option<String>,
    pub sessions: u32,
    pub rounds: u32,
    pub messages: u32,
    pub started_at: String,
    pub completed_at: String,
}

/// Full manifest — port of Manifest from manifest.ts:46-55
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub created_at: String,
    pub store: ManifestStoreInfo,
    pub seed: Option<ManifestSeedInfo>,
}

impl Manifest {
    pub fn new(store: ManifestStoreInfo) -> Self {
        Self {
            version: 1,
            created_at: crate::utils::time::now_instant_iso(),
            store,
            seed: None,
        }
    }
}

/// Snapshot of current store config for manifest writing.
/// port of StoreConfigSnapshot from manifest.ts:103-109
#[derive(Clone, Debug)]
pub struct StoreConfigSnapshot {
    pub r#type: String,
    pub sqlite_path: Option<String>,
    pub tcvdb_url: Option<String>,
    pub tcvdb_database: Option<String>,
    pub tcvdb_alias: Option<String>,
}

/// Build ManifestStoreInfo from StoreConfigSnapshot.
/// port of buildStoreInfo() from manifest.ts:114-126
pub fn build_store_info(snapshot: &StoreConfigSnapshot) -> ManifestStoreInfo {
    if snapshot.r#type == "sqlite" {
        ManifestStoreInfo {
            r#type: "sqlite".to_string(),
            sqlite: Some(ManifestSqliteInfo {
                path: snapshot
                    .sqlite_path
                    .clone()
                    .unwrap_or_else(|| "vectors.db".to_string()),
            }),
            tcvdb: None,
        }
    } else {
        ManifestStoreInfo {
            r#type: "tcvdb".to_string(),
            sqlite: None,
            tcvdb: Some(ManifestTcvdbInfo {
                url: snapshot.tcvdb_url.clone().unwrap_or_default(),
                database: snapshot.tcvdb_database.clone().unwrap_or_default(),
                alias: snapshot.tcvdb_alias.clone(),
            }),
        }
    }
}

/// Read manifest from disk. Returns None if not found.
/// port of readManifest() from manifest.ts:75-84
pub fn read_manifest(data_dir: &str) -> Option<Manifest> {
    let path = manifest_path(data_dir);
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write manifest to disk (creates .metadata/ if needed).
/// port of writeManifest() from manifest.ts:89-97
pub fn write_manifest(data_dir: &str, manifest: &Manifest) -> AeonMemoryResult<()> {
    let path = manifest_path(data_dir);
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(AeonMemoryCoreError::Io)?;

    let mut json = serde_json::to_string_pretty(manifest).map_err(AeonMemoryCoreError::Json)?;
    json.push('\n');
    std::fs::write(&path, json.as_bytes()).map_err(AeonMemoryCoreError::Io)?;

    Ok(())
}

/// Compare persisted store binding against current config.
/// Returns list of mismatch descriptions (empty = all good).
/// port of diffStoreBinding() from manifest.ts:132-159
pub fn diff_store_binding(
    persisted: &ManifestStoreInfo,
    current: &ManifestStoreInfo,
) -> Vec<String> {
    let mut diffs = Vec::new();
    if persisted.r#type != current.r#type {
        diffs.push(format!(
            "store type changed: {} → {}",
            persisted.r#type, current.r#type
        ));
        return diffs;
    }

    if persisted.r#type == "sqlite" {
        let old_path = persisted.sqlite.as_ref().map(|s| &s.path);
        let new_path = current.sqlite.as_ref().map(|s| &s.path);
        if old_path != new_path {
            diffs.push(format!(
                "sqlite path changed: {:?} → {:?}",
                old_path, new_path
            ));
        }
    }

    if persisted.r#type == "tcvdb" {
        let old_url = persisted.tcvdb.as_ref().map(|t| &t.url);
        let new_url = current.tcvdb.as_ref().map(|t| &t.url);
        if old_url != new_url {
            diffs.push(format!("tcvdb url changed: {:?} → {:?}", old_url, new_url));
        }
    }

    diffs
}

fn manifest_path(data_dir: &str) -> std::path::PathBuf {
    Path::new(data_dir).join(".metadata").join("manifest.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("aeon-memory-test-manifest")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_read_nonexistent() {
        let dir = setup_dir("nonexistent");
        assert!(read_manifest(dir.to_str().unwrap()).is_none());
    }

    #[test]
    fn test_write_and_read() {
        let dir = setup_dir("write_read");
        let store = ManifestStoreInfo {
            r#type: "sqlite".to_string(),
            sqlite: Some(ManifestSqliteInfo {
                path: "vectors.db".to_string(),
            }),
            tcvdb: None,
        };
        let manifest = Manifest::new(store);
        write_manifest(dir.to_str().unwrap(), &manifest).unwrap();

        let read = read_manifest(dir.to_str().unwrap()).unwrap();
        assert_eq!(read.version, 1);
        assert_eq!(read.store.r#type, "sqlite");
        assert_eq!(read.store.sqlite.unwrap().path, "vectors.db");
    }

    #[test]
    fn test_diff_store_type() {
        let old = ManifestStoreInfo {
            r#type: "sqlite".to_string(),
            sqlite: Some(ManifestSqliteInfo {
                path: "old.db".to_string(),
            }),
            tcvdb: None,
        };
        let new = ManifestStoreInfo {
            r#type: "tcvdb".to_string(),
            sqlite: None,
            tcvdb: Some(ManifestTcvdbInfo {
                url: "http://example.com".to_string(),
                database: "test".to_string(),
                alias: None,
            }),
        };
        let diffs = diff_store_binding(&old, &new);
        assert!(!diffs.is_empty());
        assert!(diffs[0].contains("store type changed"));
    }

    #[test]
    fn test_diff_no_change() {
        let old = ManifestStoreInfo {
            r#type: "sqlite".to_string(),
            sqlite: Some(ManifestSqliteInfo {
                path: "vectors.db".to_string(),
            }),
            tcvdb: None,
        };
        let new = old.clone();
        let diffs = diff_store_binding(&old, &new);
        assert!(diffs.is_empty());
    }

    #[test]
    fn test_build_store_info_sqlite() {
        let snapshot = StoreConfigSnapshot {
            r#type: "sqlite".to_string(),
            sqlite_path: Some("custom/path/vectors.db".to_string()),
            tcvdb_url: None,
            tcvdb_database: None,
            tcvdb_alias: None,
        };
        let info = build_store_info(&snapshot);
        assert_eq!(info.sqlite.unwrap().path, "custom/path/vectors.db");
    }
}
