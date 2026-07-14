// port of src/utils/time.ts with chrono-tz IANA timezone support

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::{OffsetComponents, Tz};
use std::sync::{LazyLock, RwLock};

static RESOLVED_TZ: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("UTC".to_owned()));

#[derive(Debug, Clone)]
pub struct TimeContext {
    timezone: String,
}
impl TimeContext {
    pub fn new(timezone: &str) -> Self {
        Self {
            timezone: resolve_timezone(timezone),
        }
    }
    fn offset_at(&self, dt: &DateTime<Utc>) -> FixedOffset {
        offset_at_name(&self.timezone, dt)
    }
    pub fn format_local_date(&self, dt: DateTime<Utc>) -> String {
        dt.with_timezone(&self.offset_at(&dt))
            .format("%Y-%m-%d")
            .to_string()
    }
    pub fn format_local_datetime(&self, dt: DateTime<Utc>) -> String {
        dt.with_timezone(&self.offset_at(&dt))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }
    pub fn format_for_llm(&self, dt: DateTime<Utc>) -> String {
        dt.with_timezone(&self.offset_at(&dt))
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string()
    }
    pub fn start_of_local_day(&self, dt: DateTime<Utc>) -> i64 {
        let local = dt.with_timezone(&self.offset_at(&dt));
        let date = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day()).unwrap();
        let naive = date.and_hms_opt(0, 0, 0).unwrap();
        if let Ok(tz) = self.timezone.parse::<Tz>() {
            tz.from_local_datetime(&naive)
                .earliest()
                .unwrap()
                .timestamp_millis()
        } else {
            self.offset_at(&dt)
                .from_local_datetime(&naive)
                .single()
                .unwrap()
                .timestamp_millis()
        }
    }

    /// Next configured wall-clock time, advancing by a calendar day rather
    /// than a fixed 24 hours so DST transitions preserve `HH:mm`.
    pub fn next_run_at_ms(&self, clean_time: &str, now_ms: i64) -> Option<i64> {
        let (hour, minute) = clean_time.split_once(':')?;
        let (hour, minute) = (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?);
        let now = DateTime::<Utc>::from_timestamp_millis(now_ms)?;
        let local = now.with_timezone(&self.offset_at(&now));
        let mut date = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())?;
        let mut candidate =
            self.resolve_local(date.and_hms_opt(hour.min(23), minute.min(59), 0)?)?;
        if candidate <= now {
            date = date.succ_opt()?;
            candidate = self.resolve_local(date.and_hms_opt(hour.min(23), minute.min(59), 0)?)?;
        }
        Some(candidate.timestamp_millis())
    }

    fn resolve_local(&self, mut value: NaiveDateTime) -> Option<DateTime<Utc>> {
        if let Ok(tz) = self.timezone.parse::<Tz>() {
            // JS Date normalizes a nonexistent spring-forward wall time to
            // the first valid instant and picks the earlier repeated instant.
            for _ in 0..=180 {
                match tz.from_local_datetime(&value) {
                    chrono::LocalResult::Single(dt) => return Some(dt.to_utc()),
                    chrono::LocalResult::Ambiguous(first, _) => return Some(first.to_utc()),
                    chrono::LocalResult::None => value += chrono::Duration::minutes(1),
                }
            }
            None
        } else {
            self.offset_at(&Utc::now())
                .from_local_datetime(&value)
                .single()
                .map(|dt| dt.to_utc())
        }
    }
}
fn offset_at_name(name: &str, dt: &DateTime<Utc>) -> FixedOffset {
    if let Ok(o) = parse_offset_str(name) {
        return o;
    }
    if let Ok(tz) = name.parse::<Tz>() {
        let x = dt.with_timezone(&tz);
        let secs = (x.offset().base_utc_offset() + x.offset().dst_offset()).num_seconds() as i32;
        return FixedOffset::east_opt(secs).unwrap();
    }
    FixedOffset::east_opt(0).unwrap()
}

/// Initialize the time module.
pub fn init_time_module(timezone: &str) {
    let tz = resolve_timezone(timezone);
    if let Ok(mut current) = RESOLVED_TZ.write() {
        *current = tz;
    }
}

/// Get the currently active timezone name.
pub fn active_timezone() -> String {
    RESOLVED_TZ
        .read()
        .map(|value| value.clone())
        .unwrap_or_else(|_| "UTC".to_owned())
}

/// Resolve a timezone string: "system" auto-detect, IANA name, or offset.
fn resolve_timezone(cfg_tz: &str) -> String {
    match cfg_tz {
        "system" => local_tz_name().unwrap_or("UTC".to_string()),
        _ => {
            if cfg_tz == "UTC" || cfg_tz == "utc" {
                return "UTC".to_string();
            }
            if (cfg_tz.contains('/') || cfg_tz.chars().any(|c| c.is_alphabetic()))
                && cfg_tz.parse::<Tz>().is_ok()
            {
                return cfg_tz.to_string();
            }
            // Validate as offset
            if parse_offset_str(cfg_tz).is_ok() {
                return cfg_tz.to_string();
            }
            eprintln!(
                "[aeon-memory-core] Unknown timezone '{}', using UTC",
                cfg_tz
            );
            "UTC".to_string()
        }
    }
}

fn parse_offset_str(tz: &str) -> Result<FixedOffset, String> {
    let bytes = tz.as_bytes();
    if bytes.len() != 6 || (bytes[0] != b'+' && bytes[0] != b'-') || bytes[3] != b':' {
        return Err("not an offset string".to_string());
    }
    let sign = if bytes[0] == b'+' { 1 } else { -1 };
    let hours: i32 = (bytes[1] - b'0') as i32 * 10 + (bytes[2] - b'0') as i32;
    let minutes: i32 = (bytes[4] - b'0') as i32 * 10 + (bytes[5] - b'0') as i32;
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return Err("invalid offset".to_string());
    }
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60)).ok_or_else(|| "invalid".to_string())
}

fn local_tz_name() -> Option<String> {
    let tz = std::env::var("TZ").ok().filter(|s| !s.is_empty());
    if let Some(tz) = tz {
        return Some(tz);
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.rfind("/zoneinfo/") {
            return Some(s[idx + "/zoneinfo/".len()..].to_string());
        }
    }
    let offset = Local::now().offset().local_minus_utc();
    let abs = offset.abs();
    let h = abs / 3600;
    let m = (abs / 60) % 60;
    Some(format!(
        "{}{:02}:{:02}",
        if offset >= 0 { '+' } else { '-' },
        h,
        m
    ))
}

/// Compute effective FixedOffset for configured timezone at current UTC time.
fn effective_offset() -> FixedOffset {
    let tz_name = active_timezone();
    if let Ok(offset) = parse_offset_str(&tz_name) {
        return offset;
    }
    if let Ok(tz) = tz_name.parse::<Tz>() {
        let local_dt: chrono::DateTime<Tz> = Utc::now().with_timezone(&tz);
        let secs = (local_dt.offset().base_utc_offset() + local_dt.offset().dst_offset())
            .num_seconds() as i32;
        if let Some(off) = FixedOffset::east_opt(secs) {
            return off;
        }
    }
    FixedOffset::east_opt(0).unwrap()
}

fn effective_offset_at(utc_dt: &DateTime<Utc>) -> FixedOffset {
    let tz_name = active_timezone();
    if let Ok(offset) = parse_offset_str(&tz_name) {
        return offset;
    }
    if let Ok(tz) = tz_name.parse::<Tz>() {
        let local_dt: chrono::DateTime<Tz> = utc_dt.with_timezone(&tz);
        let secs = (local_dt.offset().base_utc_offset() + local_dt.offset().dst_offset())
            .num_seconds() as i32;
        if let Some(off) = FixedOffset::east_opt(secs) {
            return off;
        }
    }
    FixedOffset::east_opt(0).unwrap()
}

/// Current time as UTC ISO 8601 with "Z" suffix.
pub fn now_instant_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Current epoch milliseconds (UTC).
pub fn now_epoch_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Format local date as YYYY-MM-DD.
pub fn format_local_date(dt: Option<DateTime<Utc>>) -> String {
    let utc_dt = dt.unwrap_or_else(Utc::now);
    let offset = effective_offset_at(&utc_dt);
    let local = utc_dt.with_timezone(&offset);
    format!(
        "{:04}-{:02}-{:02}",
        local.year(),
        local.month(),
        local.day()
    )
}

/// Format timestamp for LLM display.
pub fn format_for_llm(ts: &str) -> String {
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        let utc = dt.to_utc();
        let offset = effective_offset_at(&utc);
        return dt
            .with_timezone(&offset)
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string();
    }
    if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
        let offset = effective_offset_at(&dt);
        return dt
            .with_timezone(&offset)
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string();
    }
    ts.to_string()
}

/// Local date string for JSONL filenames.
pub fn local_date_for_filename() -> String {
    format_local_date(None)
}

/// Describe timezone for prompt injection (port of describeTimeZoneForPrompt).
pub fn describe_timezone_for_prompt() -> String {
    let offset = effective_offset();
    let rendered = format!(
        "{}{:02}:{:02}",
        if offset.local_minus_utc() >= 0 {
            '+'
        } else {
            '-'
        },
        offset.local_minus_utc().unsigned_abs() / 3600,
        (offset.local_minus_utc().unsigned_abs() / 60) % 60,
    );
    format!(
        "All timestamps below are in {} (UTC{}). When reasoning about \"yesterday\", \"last week\", or time differences, use this timezone.",
        active_timezone(),
        rendered
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_instant_iso() {
        let s = now_instant_iso();
        assert!(s.ends_with('Z'));
        assert!(s.len() > 20);
    }

    #[test]
    fn test_format_local_date_specific() {
        let dt = DateTime::parse_from_rfc3339("2026-07-13T12:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(format_local_date(Some(dt)), "2026-07-13");
    }

    #[test]
    fn test_format_for_llm() {
        let f = format_for_llm("2026-07-13T12:00:00Z");
        assert!(f.contains("2026-07-13"));
        assert!(f.contains(':'));
    }

    #[test]
    fn test_active_timezone_default() {
        assert_eq!(active_timezone(), "UTC");
    }

    #[test]
    fn test_parse_offset_str() {
        assert_eq!(
            parse_offset_str("+08:00").unwrap().local_minus_utc(),
            8 * 3600
        );
        assert!(parse_offset_str("invalid").is_err());
    }

    fn tz_offset_secs(tz: &Tz, utc_dt: &DateTime<Utc>) -> i32 {
        let local: chrono::DateTime<Tz> = utc_dt.with_timezone(tz);
        (local.offset().base_utc_offset() + local.offset().dst_offset()).num_seconds() as i32
    }

    #[test]
    fn test_iana_asia_shanghai() {
        let tz: Tz = "Asia/Shanghai".parse().unwrap();
        let dt = parse_dt("2026-07-13T12:00:00Z");
        assert_eq!(tz_offset_secs(&tz, &dt), 8 * 3600);
    }

    #[test]
    fn test_iana_asia_kolkata() {
        let tz: Tz = "Asia/Kolkata".parse().unwrap();
        let dt = Utc::now();
        assert_eq!(tz_offset_secs(&tz, &dt), 5 * 3600 + 30 * 60);
    }

    #[test]
    fn test_iana_us_eastern_dst() {
        let tz: Tz = "America/New_York".parse().unwrap();
        let dt_jul = parse_dt("2026-07-13T12:00:00Z");
        assert_eq!(tz_offset_secs(&tz, &dt_jul), -4 * 3600, "July EDT");
        let dt_jan = parse_dt("2026-01-15T12:00:00Z");
        assert_eq!(tz_offset_secs(&tz, &dt_jan), -5 * 3600, "January EST");
    }

    fn parse_dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    #[test]
    fn test_describe_timezone() {
        // describe_timezone_for_prompt uses the module-level timezone,
        // which defaults to "UTC" if init was never called
        let desc = describe_timezone_for_prompt();
        // It should include a timezone reference
        assert!(!desc.is_empty());
    }
}
