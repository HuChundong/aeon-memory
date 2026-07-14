//! Deterministic port of `src/utils/pipeline-manager.ts`.
//!
//! The manager owns policy and deadlines, while the embedding runtime owns the
//! actual timer (call [`PipelineManager::run_due`] when the deadline returned by
//! [`PipelineManager::next_deadline_ms`] is reached).  Keeping time outside the
//! manager makes all scheduling rules testable without sleeps.

use super::checkpoint::PipelineSessionState;
use crate::utils::session_filter::SessionFilter;
use chrono::{SecondsFormat, TimeZone, Utc};
use std::collections::HashMap;

const L1_RETRY_DELAY_MS: i64 = 30_000;
const L1_MAX_RETRIES: u8 = 5;
const GC_EVERY_NOTIFICATIONS: u32 = 50;
const GC_INACTIVE_MULTIPLIER: i64 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub every_n_conversations: u32,
    pub enable_warmup: bool,
    pub l1_idle_timeout_ms: i64,
    pub l2_delay_after_l1_ms: i64,
    pub l2_min_interval_ms: i64,
    pub l2_max_interval_ms: i64,
    pub session_active_window_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct L2Result {
    pub latest_cursor: Option<String>,
    pub skipped: bool,
}

/// Injectable wall clock. Values are Unix epoch milliseconds.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// Runtime wake-up adapter. It must arrange for the owner to call `run_due`
/// at (or after) the supplied deadline; `None` cancels the wake-up.
pub trait TimerDriver: Send {
    fn arm(&mut self, deadline_ms: Option<i64>);
}

#[derive(Default)]
struct NoopTimerDriver;
impl TimerDriver for NoopTimerDriver {
    fn arm(&mut self, _: Option<i64>) {}
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }
}

/// Side effects invoked by the scheduler. An error means that layer failed.
pub trait PipelineRunner: Send {
    fn run_l1(&mut self, session: &str, messages: &[CapturedMessage]) -> Result<(), String>;
    fn run_l2(&mut self, session: &str, cursor: Option<&str>) -> Result<L2Result, String>;
    fn run_l3(&mut self) -> Result<(), String>;
}

pub trait StatePersister: Send {
    fn persist(&mut self, states: &HashMap<String, PipelineSessionState>) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum L2Source {
    DelayAfterL1,
    MaxInterval,
}

#[derive(Clone, Debug, Default)]
struct Timers {
    l1_deadline: Option<i64>,
    l2_deadline: Option<(i64, L2Source)>,
    l1_queued: bool,
    l2_queued: bool,
    l1_retry_count: u8,
}

pub struct PipelineManager {
    config: PipelineConfig,
    clock: Box<dyn Clock>,
    timer_driver: Box<dyn TimerDriver>,
    runner: Box<dyn PipelineRunner>,
    persister: Option<Box<dyn StatePersister>>,
    states: HashMap<String, PipelineSessionState>,
    session_order: Vec<String>,
    timers: HashMap<String, Timers>,
    buffers: HashMap<String, Vec<CapturedMessage>>,
    l2_last_run_ms: HashMap<String, i64>,
    notify_counter: u32,
    l3_running: bool,
    l3_pending: bool,
    destroyed: bool,
    session_filter: SessionFilter,
}

impl PipelineManager {
    pub fn new(
        config: PipelineConfig,
        clock: Box<dyn Clock>,
        runner: Box<dyn PipelineRunner>,
    ) -> Self {
        assert!(config.every_n_conversations > 0);
        Self {
            config,
            clock,
            timer_driver: Box::new(NoopTimerDriver),
            runner,
            persister: None,
            states: HashMap::new(),
            session_order: Vec::new(),
            timers: HashMap::new(),
            buffers: HashMap::new(),
            l2_last_run_ms: HashMap::new(),
            notify_counter: 0,
            l3_running: false,
            l3_pending: false,
            destroyed: false,
            session_filter: SessionFilter::new(&[]),
        }
    }

    pub fn with_timer_driver(mut self, timer_driver: Box<dyn TimerDriver>) -> Self {
        self.timer_driver = timer_driver;
        self.refresh_timer_driver();
        self
    }

    pub fn with_persister(mut self, persister: Box<dyn StatePersister>) -> Self {
        self.persister = Some(persister);
        self
    }

    pub fn with_session_filter(mut self, session_filter: SessionFilter) -> Self {
        self.session_filter = session_filter;
        self
    }

    /// Restore checkpoint state and recover pending work as delayed L2 work.
    pub fn start(&mut self, restored: HashMap<String, PipelineSessionState>) {
        if self.destroyed {
            return;
        }
        for (key, mut state) in restored {
            if self.session_filter.should_skip(&key) {
                continue;
            }
            if !self.session_order.contains(&key) {
                self.session_order.push(key.clone());
            }
            // serde defaults old checkpoints to zero: zero means graduated.
            if state.conversation_count != 0 || state.l2_pending_l1_count != 0 {
                state.l2_pending_l1_count = state.l2_pending_l1_count.max(state.conversation_count);
                state.conversation_count = 0;
                self.states.insert(key.clone(), state);
                self.advance_l2_timer(&key);
            } else {
                self.states.insert(key, state);
            }
        }
        self.refresh_timer_driver();
    }

    pub fn notify_conversation(&mut self, session: &str, messages: Vec<CapturedMessage>) {
        if self.destroyed || self.session_filter.should_skip(session) {
            return;
        }
        let now = self.clock.now_ms();
        if !self.states.contains_key(session) {
            self.session_order.push(session.to_owned());
        }
        let enable_warmup = self.config.enable_warmup;
        let state = self
            .states
            .entry(session.to_owned())
            .or_insert_with(|| PipelineSessionState {
                last_active_time: now,
                warmup_threshold: if enable_warmup { 1 } else { 0 },
                ..PipelineSessionState::default()
            });
        state.conversation_count += 1;
        state.last_active_time = now;
        self.buffers
            .entry(session.to_owned())
            .or_default()
            .extend(messages);
        let threshold = if self.config.enable_warmup && state.warmup_threshold > 0 {
            state
                .warmup_threshold
                .min(self.config.every_n_conversations)
        } else {
            self.config.every_n_conversations
        };
        self.timers
            .entry(session.to_owned())
            .or_default()
            .l1_retry_count = 0;
        let reached_threshold = state.conversation_count >= threshold;
        self.persist_states();
        if reached_threshold {
            self.run_l1(session);
        } else {
            self.timers.get_mut(session).unwrap().l1_deadline =
                Some(now + self.config.l1_idle_timeout_ms);
            self.notify_counter += 1;
            if self.notify_counter >= GC_EVERY_NOTIFICATIONS {
                self.notify_counter = 0;
                self.gc_stale_sessions();
            }
        }
        self.refresh_timer_driver();
    }

    /// Execute every timer due at the current clock value. Newly-created due
    /// timers are also drained before returning.
    pub fn run_due(&mut self) {
        if self.destroyed {
            return;
        }
        loop {
            let now = self.clock.now_ms();
            let due_l1 = self
                .timers
                .iter()
                .find(|(_, t)| t.l1_deadline.is_some_and(|at| at <= now))
                .map(|(k, _)| k.clone());
            if let Some(key) = due_l1 {
                self.timers.get_mut(&key).unwrap().l1_deadline = None;
                self.run_l1(&key);
                continue;
            }
            let due_l2 = self.timers.iter().find_map(|(key, timers)| {
                timers
                    .l2_deadline
                    .filter(|(at, _)| *at <= now)
                    .map(|(_, source)| (key.clone(), source))
            });
            if let Some((key, source)) = due_l2 {
                self.timers.get_mut(&key).unwrap().l2_deadline = None;
                self.on_l2_timer(&key, source);
                continue;
            }
            break;
        }
        self.refresh_timer_driver();
    }

    pub fn next_deadline_ms(&self) -> Option<i64> {
        self.timers
            .values()
            .flat_map(|t| {
                [t.l1_deadline, t.l2_deadline.map(|(at, _)| at)]
                    .into_iter()
                    .flatten()
            })
            .min()
    }

    /// End one session without disturbing any other session.
    pub fn flush_session(&mut self, session: &str) {
        if self.destroyed || self.session_filter.should_skip(session) {
            return;
        }
        if let Some(t) = self.timers.get_mut(session) {
            t.l1_deadline = None;
        }
        if self
            .states
            .get(session)
            .is_some_and(|state| state.conversation_count > 0)
            || self.buffers.get(session).is_some_and(|b| !b.is_empty())
        {
            self.run_l1(session);
        }
        self.refresh_timer_driver();
    }

    /// Graceful process shutdown: L1 buffers, then all scheduled L2 work,
    /// then persistence. Once called, new notifications are rejected.
    pub fn shutdown(&mut self) {
        if self.destroyed {
            return;
        }
        // TS marks destroyed before its private queues drain. Internal run_l1/
        // run_l2 remain callable, while trigger_l3 is suppressed during
        // shutdown and no new public notification is accepted.
        self.destroyed = true;
        let sessions = self.session_order.clone();
        for key in &sessions {
            if self
                .states
                .get(key)
                .is_some_and(|state| state.conversation_count > 0)
                || self.buffers.get(key).is_some_and(|b| !b.is_empty())
            {
                self.run_l1(key);
            }
        }
        for key in sessions {
            if self
                .timers
                .get(&key)
                .is_some_and(|t| t.l2_deadline.is_some())
            {
                self.timers.get_mut(&key).unwrap().l2_deadline = None;
                self.run_l2(&key);
            }
        }
        self.persist_states();
        self.timer_driver.arm(None);
    }

    pub fn session_state(&self, session: &str) -> Option<PipelineSessionState> {
        self.states.get(session).cloned()
    }

    pub fn buffered_message_count(&self, session: &str) -> usize {
        self.buffers.get(session).map_or(0, Vec::len)
    }

    pub fn session_keys(&self) -> Vec<String> {
        self.session_order
            .iter()
            .filter(|key| self.states.contains_key(*key))
            .cloned()
            .collect()
    }

    pub fn is_destroyed(&self) -> bool {
        self.destroyed
    }

    /// Request global persona regeneration. Multiple requests while a run is
    /// active collapse into exactly one follow-up run.
    pub fn request_l3(&mut self) {
        self.trigger_l3();
    }

    fn run_l1(&mut self, session: &str) {
        let Some(state) = self.states.get(session) else {
            return;
        };
        if self
            .timers
            .get(session)
            .is_some_and(|timers| timers.l1_queued)
        {
            return;
        }
        if state.conversation_count == 0 && self.buffers.get(session).is_none_or(|b| b.is_empty()) {
            return;
        }
        let timers = self.timers.entry(session.to_owned()).or_default();
        timers.l1_deadline = None;
        timers.l1_queued = true;
        let messages = self.buffers.remove(session).unwrap_or_default();
        match self.runner.run_l1(session, &messages) {
            Ok(()) => {
                let state = self.states.get_mut(session).unwrap();
                state.l2_pending_l1_count = state.conversation_count;
                state.conversation_count = 0;
                if self.config.enable_warmup && state.warmup_threshold > 0 {
                    let next = state.warmup_threshold.saturating_mul(2);
                    state.warmup_threshold = if next >= self.config.every_n_conversations {
                        0
                    } else {
                        next
                    };
                }
                let timers = self.timers.get_mut(session).unwrap();
                timers.l1_retry_count = 0;
                timers.l1_queued = false;
                self.persist_states();
                self.advance_l2_timer(session);
            }
            Err(_) => {
                let current = self.buffers.remove(session).unwrap_or_default();
                let mut restored = messages;
                restored.extend(current);
                self.buffers.insert(session.to_owned(), restored);
                let timers = self.timers.get_mut(session).unwrap();
                timers.l1_queued = false;
                timers.l1_retry_count += 1;
                if timers.l1_retry_count <= L1_MAX_RETRIES {
                    timers.l1_deadline = Some(self.clock.now_ms() + L1_RETRY_DELAY_MS);
                }
            }
        }
    }

    fn advance_l2_timer(&mut self, session: &str) {
        if self.destroyed {
            return;
        }
        let now = self.clock.now_ms();
        let floor = self
            .l2_last_run_ms
            .get(session)
            .map_or(0, |last| last + self.config.l2_min_interval_ms);
        let desired = (now + self.config.l2_delay_after_l1_ms).max(floor);
        let timer = &mut self
            .timers
            .entry(session.to_owned())
            .or_default()
            .l2_deadline;
        if timer.is_none_or(|(current, _)| desired < current) {
            *timer = Some((desired, L2Source::DelayAfterL1));
        }
    }

    fn on_l2_timer(&mut self, session: &str, source: L2Source) {
        let Some(state) = self.states.get(session) else {
            return;
        };
        if source == L2Source::MaxInterval
            && self.clock.now_ms() - state.last_active_time >= self.config.session_active_window_ms
        {
            return;
        }
        self.run_l2(session);
    }

    fn run_l2(&mut self, session: &str) {
        if self
            .timers
            .get(session)
            .is_some_and(|timers| timers.l2_queued)
        {
            return;
        }
        self.timers.entry(session.to_owned()).or_default().l2_queued = true;
        let cursor = self.states.get(session).and_then(|s| {
            (!s.last_extraction_updated_time.is_empty())
                .then_some(s.last_extraction_updated_time.clone())
        });
        let result = self.runner.run_l2(session, cursor.as_deref());
        self.timers.get_mut(session).unwrap().l2_queued = false;
        match result {
            Err(_) => self.arm_l2_max(session),
            Ok(result) => {
                let first = !self.l2_last_run_ms.contains_key(session);
                if first && result.skipped {
                    self.arm_l2_max(session);
                    self.persist_states();
                    return;
                }
                let now = self.clock.now_ms();
                let iso = iso_time(now);
                let state = self.states.get_mut(session).unwrap();
                state.l2_pending_l1_count = 0;
                state.last_extraction_time.clone_from(&iso);
                state.l2_last_extraction_time = iso.clone();
                state.last_extraction_updated_time = result.latest_cursor.unwrap_or_else(|| {
                    if state.last_extraction_updated_time.is_empty() {
                        iso
                    } else {
                        state.last_extraction_updated_time.clone()
                    }
                });
                self.l2_last_run_ms.insert(session.to_owned(), now);
                self.persist_states();
                self.arm_l2_max(session);
                self.trigger_l3();
            }
        }
    }

    fn arm_l2_max(&mut self, session: &str) {
        self.timers
            .entry(session.to_owned())
            .or_default()
            .l2_deadline = Some((
            self.clock.now_ms() + self.config.l2_max_interval_ms,
            L2Source::MaxInterval,
        ));
    }

    fn trigger_l3(&mut self) {
        if self.destroyed {
            return;
        }
        if self.l3_running {
            self.l3_pending = true;
            return;
        }
        loop {
            self.l3_running = true;
            self.l3_pending = false;
            let _ = self.runner.run_l3();
            self.l3_running = false;
            if !self.l3_pending || self.destroyed {
                break;
            }
        }
    }

    fn gc_stale_sessions(&mut self) {
        let now = self.clock.now_ms();
        let max_inactive = self.config.session_active_window_ms * GC_INACTIVE_MULTIPLIER;
        let stale: Vec<_> = self
            .states
            .iter()
            .filter(|(key, state)| {
                now - state.last_active_time >= max_inactive
                    && self
                        .timers
                        .get(*key)
                        .is_none_or(|t| !t.l1_queued && !t.l2_queued)
                    && self.buffers.get(*key).is_none_or(Vec::is_empty)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            self.states.remove(&key);
            self.timers.remove(&key);
            self.buffers.remove(&key);
            self.l2_last_run_ms.remove(&key);
        }
        self.session_order
            .retain(|key| self.states.contains_key(key));
    }

    fn persist_states(&mut self) {
        if let Some(persister) = &mut self.persister {
            let _ = persister.persist(&self.states);
        }
    }

    fn refresh_timer_driver(&mut self) {
        let deadline = self.next_deadline_ms();
        self.timer_driver.arm(deadline);
    }
}

fn iso_time(epoch_ms: i64) -> String {
    Utc.timestamp_millis_opt(epoch_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeClock(Arc<Mutex<i64>>);
    impl FakeClock {
        fn new(now: i64) -> (Self, Arc<Mutex<i64>>) {
            let value = Arc::new(Mutex::new(now));
            (Self(value.clone()), value)
        }
    }
    impl Clock for FakeClock {
        fn now_ms(&self) -> i64 {
            *self.0.lock().unwrap()
        }
    }

    #[derive(Default)]
    struct Calls {
        l1: Vec<(String, usize)>,
        l2: Vec<String>,
        l3: usize,
        fail_l1: usize,
        skip_l2: bool,
    }
    struct FakeRunner(Arc<Mutex<Calls>>);
    impl PipelineRunner for FakeRunner {
        fn run_l1(&mut self, s: &str, m: &[CapturedMessage]) -> Result<(), String> {
            let mut c = self.0.lock().unwrap();
            c.l1.push((s.into(), m.len()));
            if c.fail_l1 > 0 {
                c.fail_l1 -= 1;
                Err("fail".into())
            } else {
                Ok(())
            }
        }
        fn run_l2(&mut self, s: &str, _: Option<&str>) -> Result<L2Result, String> {
            let mut c = self.0.lock().unwrap();
            c.l2.push(s.into());
            Ok(L2Result {
                latest_cursor: Some("cursor".into()),
                skipped: c.skip_l2,
            })
        }
        fn run_l3(&mut self) -> Result<(), String> {
            self.0.lock().unwrap().l3 += 1;
            Ok(())
        }
    }
    struct FakeTimer(Arc<Mutex<Vec<Option<i64>>>>);
    impl TimerDriver for FakeTimer {
        fn arm(&mut self, deadline: Option<i64>) {
            self.0.lock().unwrap().push(deadline);
        }
    }
    fn config() -> PipelineConfig {
        PipelineConfig {
            every_n_conversations: 5,
            enable_warmup: true,
            l1_idle_timeout_ms: 60,
            l2_delay_after_l1_ms: 90,
            l2_min_interval_ms: 900,
            l2_max_interval_ms: 3600,
            session_active_window_ms: 2400,
        }
    }
    fn msg() -> CapturedMessage {
        CapturedMessage {
            role: "user".into(),
            content: "x".into(),
            timestamp: "t".into(),
        }
    }
    fn manager() -> (PipelineManager, Arc<Mutex<i64>>, Arc<Mutex<Calls>>) {
        let (clock, now) = FakeClock::new(1000);
        let calls = Arc::new(Mutex::new(Calls::default()));
        (
            PipelineManager::new(
                config(),
                Box::new(clock),
                Box::new(FakeRunner(calls.clone())),
            ),
            now,
            calls,
        )
    }

    #[test]
    fn warmup_is_one_two_four_then_steady_state() {
        let (mut m, _, calls) = manager();
        m.notify_conversation("s", vec![msg()]);
        assert_eq!(m.session_state("s").unwrap().warmup_threshold, 2);
        m.notify_conversation("s", vec![msg()]);
        m.notify_conversation("s", vec![msg()]);
        assert_eq!(m.session_state("s").unwrap().warmup_threshold, 4);
        for _ in 0..4 {
            m.notify_conversation("s", vec![msg()]);
        }
        assert_eq!(m.session_state("s").unwrap().warmup_threshold, 0);
        assert_eq!(calls.lock().unwrap().l1.len(), 3);
    }

    #[test]
    fn session_filter_keeps_l0_owned_sessions_out_of_l1_l2_pipeline() {
        let (mut m, _, calls) = manager();
        let oracle: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/session_filter_oracle.json"
        ))
        .unwrap();
        let patterns = oracle["patterns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        m = m.with_session_filter(SessionFilter::new(&patterns));
        m.start(HashMap::from([
            (
                "agent:a:subagent:worker".into(),
                PipelineSessionState {
                    conversation_count: 3,
                    ..Default::default()
                },
            ),
            ("agent:a:normal".into(), PipelineSessionState::default()),
        ]));
        assert_eq!(m.session_keys(), ["agent:a:normal"]);

        let mut expected_l1 = 0;
        for case in oracle["keys"].as_array().unwrap() {
            let key = case["key"].as_str().unwrap();
            let skip = case["skip"].as_bool().unwrap();
            m.notify_conversation(key, vec![msg()]);
            m.flush_session(key);
            if skip {
                assert!(m.session_state(key).is_none(), "filtered key={key}");
            } else {
                expected_l1 += 1;
                assert!(m.session_state(key).is_some(), "allowed key={key}");
            }
        }
        assert_eq!(calls.lock().unwrap().l1.len(), expected_l1);
    }

    #[test]
    fn idle_timer_is_resettable_and_retries_preserve_buffer() {
        let (mut m, now, calls) = manager();
        m.config.enable_warmup = false;
        calls.lock().unwrap().fail_l1 = 1;
        m.notify_conversation("s", vec![msg()]);
        *now.lock().unwrap() += 30;
        m.notify_conversation("s", vec![msg()]);
        *now.lock().unwrap() += 59;
        m.run_due();
        assert_eq!(calls.lock().unwrap().l1.len(), 0);
        *now.lock().unwrap() += 1;
        m.run_due();
        assert_eq!(m.buffered_message_count("s"), 2);
        *now.lock().unwrap() += L1_RETRY_DELAY_MS;
        m.run_due();
        assert_eq!(m.buffered_message_count("s"), 0);
        assert_eq!(calls.lock().unwrap().l1.len(), 2);
    }

    #[test]
    fn l2_deadline_only_moves_down_and_respects_minimum() {
        let (mut m, now, calls) = manager();
        m.notify_conversation("s", vec![msg()]);
        assert_eq!(m.next_deadline_ms(), Some(1090));
        *now.lock().unwrap() = 1090;
        m.run_due();
        assert_eq!(calls.lock().unwrap().l2.len(), 1);
        assert_eq!(m.next_deadline_ms(), Some(4690));
        *now.lock().unwrap() = 1200;
        m.config.enable_warmup = false;
        for _ in 0..5 {
            m.notify_conversation("s", vec![msg()]);
        }
        assert_eq!(m.next_deadline_ms(), Some(1990)); // last L2 1090 + min 900
        // A later L1 must not postpone an already earlier deadline.
        for _ in 0..5 {
            m.notify_conversation("s", vec![msg()]);
        }
        assert_eq!(m.next_deadline_ms(), Some(1990));
    }

    #[test]
    fn cold_periodic_session_stops_but_delay_after_l1_is_exempt() {
        let (mut m, now, calls) = manager();
        m.notify_conversation("s", vec![msg()]);
        *now.lock().unwrap() = 1090;
        m.run_due();
        *now.lock().unwrap() = 4690;
        m.run_due();
        assert_eq!(calls.lock().unwrap().l2.len(), 1);
        assert_eq!(m.next_deadline_ms(), None);
    }

    #[test]
    fn first_skipped_l2_does_not_apply_minimum_floor_or_trigger_l3() {
        let (mut m, now, calls) = manager();
        calls.lock().unwrap().skip_l2 = true;
        m.notify_conversation("s", vec![msg()]);
        *now.lock().unwrap() = 1090;
        m.run_due();
        assert_eq!(calls.lock().unwrap().l3, 0);
        calls.lock().unwrap().skip_l2 = false;
        *now.lock().unwrap() = 1100;
        m.config.enable_warmup = false;
        for _ in 0..5 {
            m.notify_conversation("s", vec![msg()]);
        }
        assert_eq!(m.next_deadline_ms(), Some(1190));
    }

    #[test]
    fn flush_session_is_scoped_and_shutdown_drains_all_layers() {
        let (mut m, _, calls) = manager();
        m.config.enable_warmup = false;
        m.notify_conversation("a", vec![msg()]);
        m.notify_conversation("b", vec![msg()]);
        m.flush_session("a");
        assert_eq!(calls.lock().unwrap().l1.len(), 1);
        assert_eq!(m.buffered_message_count("b"), 1);
        m.shutdown();
        let c = calls.lock().unwrap();
        assert_eq!(c.l1.len(), 2);
        assert_eq!(c.l2.len(), 1);
        // The TypeScript manager marks itself destroyed before draining its
        // queues, so shutdown-triggered L2 completions do not enqueue L3.
        assert_eq!(c.l3, 0);
        assert!(m.is_destroyed());
    }

    #[test]
    fn gc_evicts_only_cold_idle_sessions() {
        let (mut m, now, _) = manager();
        m.config.enable_warmup = false;
        m.notify_conversation("cold", vec![]);
        m.flush_session("cold");
        *now.lock().unwrap() += 7201;
        for i in 0..50 {
            m.notify_conversation(&format!("live-{i}"), vec![]);
        }
        assert!(!m.session_keys().contains(&"cold".to_string()));
    }

    #[test]
    fn recovery_converts_unrecoverable_l1_count_into_delayed_l2() {
        let (mut m, now, calls) = manager();
        let mut restored = HashMap::new();
        restored.insert(
            "s".into(),
            PipelineSessionState {
                conversation_count: 3,
                l2_pending_l1_count: 1,
                last_active_time: 1000,
                ..PipelineSessionState::default()
            },
        );
        m.start(restored);
        let state = m.session_state("s").unwrap();
        assert_eq!(state.conversation_count, 0);
        assert_eq!(state.l2_pending_l1_count, 3);
        assert_eq!(m.next_deadline_ms(), Some(1090));
        *now.lock().unwrap() = 1090;
        m.run_due();
        assert_eq!(calls.lock().unwrap().l2, vec!["s"]);
    }

    #[test]
    fn l3_request_while_running_sets_only_one_pending_run() {
        let (mut m, _, calls) = manager();
        // Simulate requests arriving from L2 completions while global L3 owns
        // its mutex. Unit tests are inside the module so no test-only API is
        // exposed to production callers.
        m.l3_running = true;
        m.request_l3();
        m.request_l3();
        assert!(m.l3_pending);
        assert_eq!(calls.lock().unwrap().l3, 0);
        m.l3_running = false;
        m.request_l3();
        assert_eq!(calls.lock().unwrap().l3, 1);
        assert!(!m.l3_pending);
    }

    #[test]
    fn automatic_l1_retry_stops_after_five_retries() {
        let (mut m, now, calls) = manager();
        m.config.enable_warmup = false;
        calls.lock().unwrap().fail_l1 = 20;
        m.notify_conversation("s", vec![msg()]);
        *now.lock().unwrap() += 60;
        m.run_due();
        for _ in 0..5 {
            *now.lock().unwrap() += L1_RETRY_DELAY_MS;
            m.run_due();
        }
        assert_eq!(calls.lock().unwrap().l1.len(), 6);
        assert_eq!(m.next_deadline_ms(), None);
        assert_eq!(m.buffered_message_count("s"), 1);
    }

    #[test]
    fn timer_driver_observes_reset_and_shutdown_cancel() {
        let (mut m, now, _) = manager();
        m.config.enable_warmup = false;
        let arms = Arc::new(Mutex::new(Vec::new()));
        m = m.with_timer_driver(Box::new(FakeTimer(arms.clone())));
        m.notify_conversation("s", vec![msg()]);
        assert_eq!(arms.lock().unwrap().last(), Some(&Some(1060)));
        *now.lock().unwrap() = 1030;
        m.notify_conversation("s", vec![msg()]);
        assert_eq!(arms.lock().unwrap().last(), Some(&Some(1090)));
        m.shutdown();
        assert_eq!(arms.lock().unwrap().last(), Some(&None));
    }
}
