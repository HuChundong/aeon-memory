use super::types::{OffloadEntry, PluginState};
use crate::{AeonMemoryCoreError, AeonMemoryResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageContext {
    pub data_root: PathBuf,
    pub data_dir: PathBuf,
    pub refs_dir: PathBuf,
    pub mmds_dir: PathBuf,
    pub offload_jsonl: PathBuf,
    pub state_file: PathBuf,
    pub agent_name: String,
    pub session_id: String,
}

fn safe_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .replace("..", "_")
}

pub fn parse_session_key(key: &str) -> Option<(String, String)> {
    let mut p = key.split(':');
    if p.next()? != "agent" {
        return None;
    }
    let mut agent = p.next()?.to_owned();
    if agent.is_empty() {
        return None;
    }
    let session = p.collect::<Vec<_>>().join(":");
    if session.is_empty() {
        return None;
    }
    if let Some(i) = session.find("swebench-w") {
        let n: String = session[i + "swebench-w".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !n.is_empty() {
            agent.push_str("-w");
            agent.push_str(&n);
        }
    }
    Some((safe_component(&agent), safe_component(&session)))
}

impl StorageContext {
    pub fn new(root: impl Into<PathBuf>, agent: &str, session: &str) -> Self {
        let data_root = root.into();
        let agent_name = safe_component(agent);
        let session_id = safe_component(session);
        let data_dir = data_root.join(&agent_name);
        Self {
            refs_dir: data_dir.join("refs"),
            mmds_dir: data_dir.join("mmds"),
            offload_jsonl: data_dir.join(format!("offload-{session_id}.jsonl")),
            state_file: data_dir.join("state.json"),
            data_root,
            data_dir,
            agent_name,
            session_id,
        }
    }
    pub fn ensure_dirs(&self) -> AeonMemoryResult<()> {
        fs::create_dir_all(&self.refs_dir)?;
        fs::create_dir_all(&self.mmds_dir)?;
        Ok(())
    }
}

fn json_err(e: serde_json::Error) -> AeonMemoryCoreError {
    AeonMemoryCoreError::Json(e)
}
pub fn append_entry(ctx: &StorageContext, entry: &OffloadEntry) -> AeonMemoryResult<()> {
    ctx.ensure_dirs()?;
    if entry.tool_call_id.is_empty() {
        return Err(AeonMemoryCoreError::InvalidInput(
            "tool_call_id is required".into(),
        ));
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ctx.offload_jsonl)?;
    serde_json::to_writer(&mut f, entry).map_err(json_err)?;
    f.write_all(b"\n")?;
    Ok(())
}
pub fn read_entries(ctx: &StorageContext) -> AeonMemoryResult<Vec<OffloadEntry>> {
    if !ctx.offload_jsonl.exists() {
        return Ok(vec![]);
    };
    let f = fs::File::open(&ctx.offload_jsonl)?;
    let mut out = vec![];
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<OffloadEntry>(&line)
            && !v.tool_call_id.is_empty()
        {
            out.push(v)
        }
    }
    Ok(out)
}
pub fn rewrite_entries(ctx: &StorageContext, entries: &[OffloadEntry]) -> AeonMemoryResult<()> {
    ctx.ensure_dirs()?;
    let tmp = ctx.offload_jsonl.with_extension("jsonl.tmp");
    let mut f = fs::File::create(&tmp)?;
    for e in entries {
        if e.tool_call_id.is_empty() {
            continue;
        }
        serde_json::to_writer(&mut f, e).map_err(json_err)?;
        f.write_all(b"\n")?;
    }
    fs::rename(tmp, &ctx.offload_jsonl)?;
    Ok(())
}

fn normalized_tool_id(id: &str) -> String {
    id.replace('_', "")
}

/// Persist L3 compression decisions in the per-session offload JSONL, exactly
/// where the TypeScript state manager reconstructs them after a restart.
pub fn mark_offload_status(
    ctx: &StorageContext,
    updates: &HashMap<String, Value>,
) -> AeonMemoryResult<()> {
    if updates.is_empty() || !ctx.offload_jsonl.exists() {
        return Ok(());
    }
    let mut entries = read_entries(ctx)?;
    let mut changed = false;
    for entry in &mut entries {
        let status = updates
            .get(&entry.tool_call_id)
            .or_else(|| updates.get(&normalized_tool_id(&entry.tool_call_id)));
        if let Some(status) = status
            && entry.offloaded.as_ref() != Some(status)
        {
            entry.offloaded = Some(status.clone());
            changed = true;
        }
    }
    if changed {
        rewrite_entries(ctx, &entries)?;
    }
    Ok(())
}

/// Rebuild the state manager's confirmed set from persisted JSONL entries.
pub fn confirmed_offload_ids(entries: &[OffloadEntry]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for entry in entries {
        let confirmed = match entry.offloaded.as_ref() {
            Some(Value::Bool(value)) => *value,
            Some(Value::Null) | None => false,
            Some(Value::String(value)) => !value.is_empty(),
            Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
            Some(Value::Array(value)) => !value.is_empty(),
            Some(Value::Object(value)) => !value.is_empty(),
        };
        if confirmed {
            ids.insert(entry.tool_call_id.clone());
            ids.insert(normalized_tool_id(&entry.tool_call_id));
        }
    }
    ids
}

/// Rebuild the state manager's aggressively deleted set from persisted JSONL.
pub fn deleted_offload_ids(entries: &[OffloadEntry]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for entry in entries {
        if entry.offloaded.as_ref().and_then(Value::as_str) == Some("deleted") {
            ids.insert(entry.tool_call_id.clone());
            ids.insert(normalized_tool_id(&entry.tool_call_id));
        }
    }
    ids
}
pub fn write_ref(ctx: &StorageContext, pair: &super::types::ToolPair) -> AeonMemoryResult<String> {
    ctx.ensure_dirs()?;
    let safe = safe_component(&pair.tool_name);
    let name = format!(
        "{}-{}-{}.md",
        pair.timestamp.replace([':', '+'], "-"),
        safe,
        safe_component(&pair.tool_call_id)
    );
    let rel = format!("refs/{name}");
    let result = if let Some(s) = pair.result.as_str() {
        s.into()
    } else {
        serde_json::to_string_pretty(&pair.result).map_err(json_err)?
    };
    fs::write(
        ctx.data_dir.join(&rel),
        format!(
            "**Tool:** {}\n**Call ID:** {}\n\n**Result:**\n```\n{}\n```",
            pair.tool_name, pair.tool_call_id, result
        ),
    )?;
    Ok(rel)
}
pub fn save_state(ctx: &StorageContext, state: &PluginState) -> AeonMemoryResult<()> {
    ctx.ensure_dirs()?;
    atomic_json(&ctx.state_file, state)
}
pub fn load_state(ctx: &StorageContext) -> AeonMemoryResult<PluginState> {
    if !ctx.state_file.exists() {
        return Ok(PluginState::default());
    };
    serde_json::from_slice(&fs::read(&ctx.state_file)?).map_err(json_err)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    session_id: String,
    offload_file: String,
    updated_at: String,
}
pub fn register_session(ctx: &StorageContext, key: &str, real_id: &str) -> AeonMemoryResult<()> {
    ctx.ensure_dirs()?;
    let path = ctx.data_dir.join("sessions-registry.json");
    let mut map: BTreeMap<String, RegistryEntry> = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?).unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    map.insert(
        key.into(),
        RegistryEntry {
            session_id: real_id.into(),
            offload_file: format!("offload-{real_id}.jsonl"),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    atomic_json(&path, &map)
}
fn atomic_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> AeonMemoryResult<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(json_err)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}
pub fn read_jsonl_values(path: &Path) -> AeonMemoryResult<Vec<Value>> {
    if !path.exists() {
        return Ok(vec![]);
    };
    Ok(BufReader::new(fs::File::open(path)?)
        .lines()
        .map_while(Result::ok)
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect())
}
