use crate::{AeonMemoryResult, types::IMemoryStore};
use std::path::Path;

pub const MIN_RETAIN_L0: i64 = 50;
pub const MIN_RETAIN_L1: i64 = 20;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CleanupStats {
    pub scanned_files: usize,
    pub changed_files: usize,
    pub skipped_non_shard_files: usize,
    pub delete_failed_files: usize,
    pub removed_l0: i64,
    pub removed_l1: i64,
    pub skipped_l0: bool,
    pub skipped_l1: bool,
}

pub fn cutoff_ms_by_local_day(now_ms: i64, retention_days: u32) -> Option<i64> {
    cutoff_ms_by_timezone(
        now_ms,
        retention_days,
        &crate::utils::time::active_timezone(),
    )
}

pub fn cutoff_ms_by_timezone(now_ms: i64, retention_days: u32, timezone: &str) -> Option<i64> {
    if retention_days == 0 {
        return None;
    }
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)?;
    let context = crate::utils::time::TimeContext::new(timezone);
    let start = context.start_of_local_day(now);
    let cutoff = start - i64::from(retention_days - 1) * 86_400_000;
    (cutoff < now_ms).then_some(cutoff)
}

pub fn next_run_at_ms(clean_time: &str, now_ms: i64, timezone: &str) -> Option<i64> {
    crate::utils::time::TimeContext::new(timezone).next_run_at_ms(clean_time, now_ms)
}

pub fn run_once(
    base: &Path,
    retention_days: u32,
    now_ms: i64,
    mut store: Option<&mut dyn IMemoryStore>,
) -> AeonMemoryResult<CleanupStats> {
    let Some(cutoff) = cutoff_ms_by_local_day(now_ms, retention_days) else {
        return Ok(CleanupStats::default());
    };
    let cutoff_date = crate::utils::time::TimeContext::new(&crate::utils::time::active_timezone())
        .format_local_date(
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(cutoff).ok_or_else(|| {
                crate::AeonMemoryCoreError::InvalidInput("invalid cleanup cutoff".into())
            })?,
        );
    let cutoff_date = chrono::NaiveDate::parse_from_str(&cutoff_date, "%Y-%m-%d")
        .map_err(|error| crate::AeonMemoryCoreError::InvalidInput(error.to_string()))?;
    let mut stats = CleanupStats::default();
    for name in ["conversations", "records"] {
        let Ok(entries) = std::fs::read_dir(base.join(name)) else {
            continue;
        };
        for entry in entries.flatten().filter(|e| e.path().is_file()) {
            let filename = entry.file_name().to_string_lossy().into_owned();
            if !(filename.ends_with(".jsonl") || filename.ends_with(".json")) {
                continue;
            }
            stats.scanned_files += 1;
            let Some(date) = filename
                .strip_suffix(".jsonl")
                .or_else(|| filename.strip_suffix(".json"))
            else {
                continue;
            };
            let Ok(day) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
                stats.skipped_non_shard_files += 1;
                continue;
            };
            if day < cutoff_date {
                match std::fs::remove_file(entry.path()) {
                    Ok(()) => stats.changed_files += 1,
                    Err(_) => stats.delete_failed_files += 1,
                }
            }
        }
    }
    if let Some(s) = store.as_mut() {
        let cutoff_iso = chrono::DateTime::from_timestamp_millis(cutoff)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let l0 = s.count_l0()?;
        if l0 <= MIN_RETAIN_L0 {
            stats.skipped_l0 = true;
        } else {
            stats.removed_l0 = s.delete_l0_expired(&cutoff_iso)?;
        }
        let l1 = s.count_l1()?;
        if l1 <= MIN_RETAIN_L1 {
            stats.skipped_l1 = true;
        } else {
            stats.removed_l1 = s.delete_l1_expired(&cutoff_iso)?;
        }
        if stats.removed_l0 > 0 || stats.removed_l1 > 0 {
            stats.changed_files += 1;
        }
    }
    Ok(stats)
}
