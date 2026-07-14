//! Stateful, host-neutral L1/L1.5/L2 coordinator.
use super::{
    mermaid,
    parser::{self, TaskJudgment},
    prompt,
    storage::{self, StorageContext},
    types::{BoundaryResult, L15Boundary, OffloadConfig, OffloadEntry, PluginState, ToolPair},
};
use crate::{
    AeonMemoryCoreError, AeonMemoryResult,
    types::{LlmRunParams, LlmRunner},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
};

fn truncate_chars(value: &str, limit: usize, suffix: &str) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    value
        .chars()
        .take(limit.saturating_sub(suffix.chars().count()))
        .collect::<String>()
        + suffix
}

fn degraded_entry(pair: &ToolPair, result_ref: Option<&String>) -> OffloadEntry {
    let result = pair
        .result
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(&pair.result).unwrap_or_default());
    let params = pair
        .params
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(&pair.params).unwrap_or_default());
    OffloadEntry {
        timestamp: pair.timestamp.clone(),
        node_id: None,
        tool_call: format!(
            "{}({})",
            pair.tool_name,
            truncate_chars(&params, 200, "...")
        ),
        summary: format!(
            "[L1 degraded] {}: {}",
            pair.tool_name,
            truncate_chars(&result, 300, "...")
        ),
        result_ref: result_ref.cloned().unwrap_or_default(),
        tool_call_id: pair.tool_call_id.clone(),
        session_key: None,
        score: Some(0.0),
        offloaded: None,
    }
}

pub struct OffloadEngine {
    pub ctx: StorageContext,
    pub config: OffloadConfig,
    pub state: PluginState,
    pending: Vec<ToolPair>,
    processed: HashSet<String>,
    l1_chunk_fail_counts: HashMap<String, u8>,
}
impl OffloadEngine {
    pub fn load(ctx: StorageContext, config: OffloadConfig) -> AeonMemoryResult<Self> {
        let state = storage::load_state(&ctx)?;
        // Match the TS host adapter: pending tool pairs and retry counters are
        // process-local only. A restart must not replay an interrupted turn.
        let pending = Vec::new();
        let processed = storage::read_entries(&ctx)?
            .into_iter()
            .flat_map(|e| [e.tool_call_id.clone(), e.tool_call_id.replace('_', "")])
            .collect();
        Ok(Self {
            ctx,
            config,
            state,
            pending,
            processed,
            l1_chunk_fail_counts: HashMap::new(),
        })
    }
    pub fn buffer(&mut self, pair: ToolPair) -> bool {
        let heartbeat =
            serde_json::to_string(&pair.params).is_ok_and(|raw| raw.contains("HEARTBEAT.md"));
        let approval_pending = pair
            .result
            .pointer("/details/status")
            .and_then(serde_json::Value::as_str)
            == Some("approval-pending");
        if pair.tool_call_id.is_empty()
            || heartbeat
            || approval_pending
            || self.processed.contains(&pair.tool_call_id)
        {
            false
        } else {
            self.pending.push(pair);
            true
        }
    }
    pub fn buffer_persisted(&mut self, pair: ToolPair) -> AeonMemoryResult<bool> {
        Ok(self.buffer(pair))
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    fn run_params(system: &str, prompt: String, task: &str) -> LlmRunParams {
        LlmRunParams {
            prompt,
            system_prompt: Some(system.into()),
            task_id: task.into(),
            timeout_ms: Some(120_000),
            max_tokens: None,
            workspace_dir: None,
            file_tool_policy: None,
            instance_id: None,
        }
    }
    pub async fn flush_l1(
        &mut self,
        runner: &dyn LlmRunner,
        recent: &str,
        force: bool,
    ) -> AeonMemoryResult<Vec<OffloadEntry>> {
        if self.pending.is_empty()
            || (!force && self.pending.len() < self.config.force_trigger_threshold)
        {
            return Ok(vec![]);
        }
        // A flush selects up to max_pairs_per_batch, then drains that complete
        // selection through TS-compatible LLM requests of at most five pairs.
        let selected_count = self.config.max_pairs_per_batch.min(self.pending.len());
        let selected = self.pending[..selected_count].to_vec();
        let mut committed = HashSet::new();
        let mut all_entries = Vec::new();
        for pairs in selected.chunks(5) {
            let pairs = pairs.to_vec();
            let chunk_key = pairs[0].tool_call_id.clone();
            let mut refs = BTreeMap::new();
            for pair in &pairs {
                refs.insert(
                    pair.tool_call_id.clone(),
                    storage::write_ref(&self.ctx, pair)?,
                );
            }
            let attempt: AeonMemoryResult<Vec<OffloadEntry>> = async {
                let prompt = prompt::build_l1_user_prompt(recent, &pairs);
                let raw = runner
                    .run_offload_l1(
                        Self::run_params(prompt::l1_system_prompt(), prompt, "offload-l1"),
                        recent,
                        &pairs,
                    )
                    .await?;
                let mut entries = parser::parse_l1(&raw);
                let expected: HashSet<_> = pairs
                    .iter()
                    .map(|pair| pair.tool_call_id.as_str())
                    .collect();
                entries.retain(|entry| expected.contains(entry.tool_call_id.as_str()));
                let returned: HashSet<_> = entries
                    .iter()
                    .map(|entry| entry.tool_call_id.as_str())
                    .collect();
                if entries.len() != pairs.len() || returned != expected {
                    return Err(AeonMemoryCoreError::InvalidInput(format!(
                        "L1 returned {} entries for {} pairs",
                        entries.len(),
                        pairs.len()
                    )));
                }
                Ok(entries)
            }
            .await;
            let mut entries = match attempt {
                Ok(entries) => {
                    self.l1_chunk_fail_counts.remove(&chunk_key);
                    entries
                }
                Err(_) => {
                    let failures = self
                        .l1_chunk_fail_counts
                        .entry(chunk_key.clone())
                        .and_modify(|count| *count += 1)
                        .or_insert(1);
                    if *failures < 3 {
                        continue;
                    }
                    self.l1_chunk_fail_counts.remove(&chunk_key);
                    pairs
                        .iter()
                        .map(|pair| degraded_entry(pair, refs.get(&pair.tool_call_id)))
                        .collect()
                }
            };
            for entry in &mut entries {
                if entry.result_ref.is_empty() {
                    entry.result_ref = refs.get(&entry.tool_call_id).cloned().unwrap_or_default();
                }
                storage::append_entry(&self.ctx, entry)?;
                self.processed.insert(entry.tool_call_id.clone());
                committed.insert(entry.tool_call_id.clone());
            }
            all_entries.extend(entries);
        }
        if !committed.is_empty() {
            self.pending
                .retain(|pair| !committed.contains(&pair.tool_call_id));
            self.state.entry_counter += all_entries.len();
            storage::save_state(&self.ctx, &self.state)?;
        }
        Ok(all_entries)
    }
    pub async fn judge_l15(
        &mut self,
        runner: &dyn LlmRunner,
        recent: &str,
        current: Option<(&str, &str, &str)>,
        metas: &[prompt::MmdMeta],
    ) -> AeonMemoryResult<TaskJudgment> {
        let start = self.state.entry_counter;
        let raw = runner
            .run_offload_l15(
                Self::run_params(
                    prompt::l15_system_prompt(),
                    prompt::build_l15_user_prompt(recent, current, metas),
                    "offload-l15",
                ),
                recent,
                current,
                metas,
            )
            .await?;
        let j = parser::parse_l15(&raw).ok_or_else(|| {
            AeonMemoryCoreError::InvalidInput("L1.5 response parsing failed".into())
        })?;
        let target = if j.is_long_task {
            if j.is_continuation {
                j.continuation_mmd_file.clone()
            } else {
                let n = self.state.mmd_counter + 1;
                self.state.mmd_counter = n;
                Some(format!(
                    "{n:03}-{}.mmd",
                    sanitize_label(j.new_task_label.as_deref().unwrap_or("unnamed-task"))
                ))
            }
        } else {
            None
        };
        self.state.active_mmd_file = target.clone();
        self.state.active_mmd_id = target
            .as_deref()
            .and_then(|f| f.strip_suffix(".mmd"))
            .map(str::to_owned);
        let b = L15Boundary {
            start_index: start,
            result: if target.is_some() {
                BoundaryResult::Long
            } else {
                BoundaryResult::Short
            },
            target_mmd: target,
        };
        if self
            .state
            .l15_boundaries
            .last()
            .is_some_and(|x| x.start_index == start)
        {
            self.state.l15_boundaries.pop();
        }
        self.state.l15_boundaries.push(b);
        storage::save_state(&self.ctx, &self.state)?;
        Ok(j)
    }
    /// TS-compatible L1.5 policy: one retry, then persist a short boundary fail-safe.
    pub async fn judge_l15_with_retry(
        &mut self,
        runner: &dyn LlmRunner,
        recent: &str,
        current: Option<(&str, &str, &str)>,
        metas: &[prompt::MmdMeta],
    ) -> AeonMemoryResult<TaskJudgment> {
        let first = self.judge_l15(runner, recent, current, metas).await;
        if first.is_ok() {
            return first;
        }
        let second = self.judge_l15(runner, recent, current, metas).await;
        if second.is_ok() {
            return second;
        }
        let start = self.state.entry_counter;
        self.state.active_mmd_file = None;
        self.state.active_mmd_id = None;
        if self
            .state
            .l15_boundaries
            .last()
            .is_some_and(|b| b.start_index == start)
        {
            self.state.l15_boundaries.pop();
        }
        self.state.l15_boundaries.push(L15Boundary {
            start_index: start,
            result: BoundaryResult::Short,
            target_mmd: None,
        });
        storage::save_state(&self.ctx, &self.state)?;
        second
    }
    pub fn l2_candidates(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AeonMemoryResult<BTreeMap<String, Vec<OffloadEntry>>> {
        let all = storage::read_entries(&self.ctx)?;
        let mut out: BTreeMap<String, Vec<OffloadEntry>> = BTreeMap::new();
        for (i, e) in all.into_iter().enumerate() {
            if e.node_id.as_deref().is_some_and(|n| n != "wait")
                || e.tool_call.contains("HEARTBEAT.md")
            {
                continue;
            }
            if e.node_id.as_deref() == Some("wait")
                && let Ok(t) = chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                && (now - t.with_timezone(&chrono::Utc)).num_seconds()
                    < self.config.l2_wait_retry_seconds as i64
            {
                continue;
            }
            let Some(b) = self
                .state
                .l15_boundaries
                .iter()
                .rev()
                .find(|b| b.start_index <= i)
            else {
                continue;
            };
            if b.result != BoundaryResult::Long {
                continue;
            }
            if let Some(file) = &b.target_mmd {
                out.entry(file.clone()).or_default().push(e)
            }
        }
        Ok(out)
    }
    pub fn should_run_l2(
        &self,
        candidates: &BTreeMap<String, Vec<OffloadEntry>>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let nulls = candidates
            .values()
            .flatten()
            .filter(|e| e.node_id.is_none())
            .count();
        if nulls >= self.config.l2_null_threshold {
            return true;
        }
        let last = self
            .state
            .last_l2_trigger_time
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&chrono::Utc));
        if last.is_some_and(|t| {
            (now - t).num_seconds() >= self.config.l2_timeout_seconds as i64
                && (!self.config.l2_time_trigger_requires_new_offload
                    || candidates.values().flatten().any(|e| {
                        chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                            .map_or(true, |x| x.with_timezone(&chrono::Utc) > t)
                    }))
        }) {
            return true;
        }
        if last.is_none() {
            if candidates
                .values()
                .flatten()
                .any(|e| e.node_id.as_deref() == Some("wait"))
            {
                return true;
            }
            return candidates
                .values()
                .flatten()
                .filter_map(|e| chrono::DateTime::parse_from_rfc3339(&e.timestamp).ok())
                .map(|t| t.with_timezone(&chrono::Utc))
                .min()
                .is_some_and(|oldest| {
                    (now - oldest).num_seconds() >= self.config.l2_timeout_seconds as i64
                });
        }
        false
    }
    pub async fn run_l2(
        &mut self,
        runner: &dyn LlmRunner,
        recent: Option<&str>,
        turn: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AeonMemoryResult<usize> {
        let groups = self.l2_candidates(now)?;
        if !self.should_run_l2(&groups, now) {
            return Ok(0);
        }
        let mut updated = 0;
        for (file, entries) in groups {
            let path = self.ctx.mmds_dir.join(&file);
            let existing = fs::read_to_string(&path).ok();
            let prefix = file.split('-').next().unwrap_or("000");
            let label = file
                .strip_suffix(".mmd")
                .unwrap_or(&file)
                .split_once('-')
                .map_or("unnamed-task", |x| x.1);
            let char_count = existing.as_ref().map_or(0, |s| s.encode_utf16().count());
            let raw = runner
                .run_offload_l2(
                    Self::run_params(
                        prompt::l2_system_prompt(),
                        prompt::build_l2_user_prompt(
                            existing.as_deref(),
                            &entries,
                            recent,
                            turn,
                            label,
                            prefix,
                            char_count,
                        ),
                        "offload-l2",
                    ),
                    existing.as_deref(),
                    &entries,
                    recent,
                    turn,
                    label,
                    prefix,
                    char_count,
                )
                .await?;
            let parsed = parser::parse_l2(&raw).ok_or_else(|| {
                AeonMemoryCoreError::InvalidInput("L2 response parsing failed".into())
            })?;
            let content = if parsed.replace {
                mermaid::apply_replace_blocks(
                    existing.as_deref().ok_or_else(|| {
                        AeonMemoryCoreError::InvalidInput("L2 replace without existing MMD".into())
                    })?,
                    &parsed.replace_blocks,
                )?
            } else {
                parsed.mmd_content.ok_or_else(|| {
                    AeonMemoryCoreError::InvalidInput("L2 write missing MMD".into())
                })?
            };
            fs::create_dir_all(&self.ctx.mmds_dir)?;
            fs::write(path, &content)?;
            let mut all = storage::read_entries(&self.ctx)?;
            let ids: HashSet<_> = entries.iter().map(|e| e.tool_call_id.as_str()).collect();
            for e in &mut all {
                if ids.contains(e.tool_call_id.as_str()) {
                    if let Some(n) = parsed.node_mapping.get(&e.tool_call_id) {
                        e.node_id = Some(n.clone());
                        updated += 1
                    } else {
                        e.node_id = Some("wait".into())
                    }
                }
            }
            storage::rewrite_entries(&self.ctx, &all)?;
        }
        self.state.last_l2_trigger_time = Some(now.to_rfc3339());
        storage::save_state(&self.ctx, &self.state)?;
        Ok(updated)
    }
}
fn sanitize_label(s: &str) -> String {
    let x = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    x.trim_matches('-').chars().take(30).collect::<String>()
}
