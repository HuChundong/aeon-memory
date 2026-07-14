use aeon_memory_core::utils::{memory_cleaner::next_run_at_ms, time};
use serde_json::Value;

fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/time_production_oracle.json")).unwrap()
}

#[test]
fn configured_timezone_controls_production_formatting_and_day_boundary() {
    let expected = oracle();
    let instant = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:30:00.000Z")
        .unwrap()
        .to_utc();
    time::init_time_module("Asia/Shanghai");
    assert_eq!(time::active_timezone(), "Asia/Shanghai");
    assert_eq!(
        time::format_local_date(Some(instant)),
        expected["shanghai"]["date"]
    );
    assert_eq!(
        time::format_for_llm("2026-01-01T00:30:00.000Z"),
        expected["shanghai"]["formatted"]
    );
    assert_eq!(
        time::TimeContext::new("Asia/Shanghai").start_of_local_day(instant),
        expected["shanghai"]["dayStart"]
    );
}

#[test]
fn cleaner_uses_calendar_days_across_dst() {
    let expected = oracle();
    for (name, now) in [
        ("spring", "2026-03-07T09:00:00Z"),
        ("fall", "2026-10-31T08:00:00Z"),
    ] {
        let now = chrono::DateTime::parse_from_rfc3339(now)
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            next_run_at_ms("03:00", now, "America/New_York").unwrap(),
            expected["dst"][name]
        );
    }
}
