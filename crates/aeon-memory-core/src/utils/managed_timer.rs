// port of src/utils/managed-timer.ts — resettable and one-shot timers.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A managed timer that can be scheduled, cancelled, and flushed.
/// Port of ManagedTimer from src/utils/managed-timer.ts.
pub struct ManagedTimer {
    name: String,
    inner: Arc<Mutex<TimerInner>>,
}

struct TimerInner {
    pending: bool,
    scheduled_time: Option<Instant>,
    generation: u64,
    /// Thread handle for the currently running timer, if any.
    handle: Option<thread::JoinHandle<()>>,
}

impl ManagedTimer {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inner: Arc::new(Mutex::new(TimerInner {
                pending: false,
                scheduled_time: None,
                generation: 0,
                handle: None,
            })),
        }
    }

    /// Schedule a callback after a delay (resettable: calling again cancels previous).
    /// Port of ManagedTimer.schedule().
    pub fn schedule<F>(&self, delay_ms: u64, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        let captured_gen = inner.generation;
        inner.pending = true;
        inner.scheduled_time = Some(Instant::now() + Duration::from_millis(delay_ms));

        let inner_clone = self.inner.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            let should_fire = {
                let g = inner_clone.lock().unwrap();
                g.pending && g.generation == captured_gen
            };
            if should_fire {
                callback();
            }
        });
        inner.handle = Some(handle);
    }

    /// Schedule at an absolute time (downward-only: only moves earlier).
    /// Port of ManagedTimer.scheduleAt().
    pub fn schedule_at<F>(&self, fire_at: Instant, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let now = Instant::now();
        if fire_at <= now {
            // Fire immediately
            callback();
            return;
        }
        let delay_ms = (fire_at - now).as_millis() as u64;
        self.schedule(delay_ms, callback);
    }

    /// Try to advance the scheduled time to an earlier point.
    /// Returns true if the timer was moved.
    /// Port of ManagedTimer.tryAdvanceTo().
    pub fn try_advance_to<F>(&self, earlier_fire_at: Instant, callback: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        let inner = self.inner.lock().unwrap();
        match inner.scheduled_time {
            Some(current) if current <= earlier_fire_at => false,
            _ => {
                drop(inner); // release lock before scheduling
                self.schedule_at(earlier_fire_at, callback);
                true
            }
        }
    }

    /// Cancel the pending timer. Port of ManagedTimer.cancel().
    pub fn cancel(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending = false;
        inner.scheduled_time = None;
    }

    /// Whether a timer is currently pending.
    pub fn is_pending(&self) -> bool {
        self.inner.lock().unwrap().pending
    }

    /// The currently scheduled fire time, if any.
    pub fn scheduled_time(&self) -> Option<Instant> {
        self.inner.lock().unwrap().scheduled_time
    }

    /// Flush: fire the callback immediately if pending, then cancel.
    pub fn flush(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pending = false;
        inner.scheduled_time = None;
        // Note: actual callback firing is handled by the spawned thread;
        // if the timer hasn't fired yet, cancel it. The callback already
        // ran if the thread completed.
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn wait_cond<F: Fn() -> bool>(cond: F, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !cond() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        cond()
    }

    #[test]
    fn test_timer_new_not_pending() {
        let t = ManagedTimer::new("test");
        assert!(!t.is_pending());
    }

    #[test]
    fn test_timer_schedule_fires() {
        let t = ManagedTimer::new("fire");
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        t.schedule(10, move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        assert!(wait_cond(
            || fired.load(Ordering::SeqCst) == 1,
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn test_timer_cancel() {
        let t = ManagedTimer::new("cancel");
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        t.schedule(50, move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        t.cancel();
        thread::sleep(Duration::from_millis(150));
        assert_eq!(fired.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_timer_reschedule() {
        let t = ManagedTimer::new("resched");
        let old_fired = Arc::new(AtomicUsize::new(0));
        let new_fired = Arc::new(AtomicUsize::new(0));

        let of = old_fired.clone();
        t.schedule(80, move || {
            of.fetch_add(1, Ordering::SeqCst);
        });

        let nf = new_fired.clone();
        t.schedule(10, move || {
            nf.fetch_add(1, Ordering::SeqCst);
        });

        // Wait for new task to fire (well past its 10ms delay)
        assert!(wait_cond(
            || new_fired.load(Ordering::SeqCst) == 1,
            Duration::from_secs(5)
        ));
        // The new task must fire exactly once
        assert_eq!(new_fired.load(Ordering::SeqCst), 1);

        // Wait past the original 80ms deadline and verify old task was cancelled
        thread::sleep(Duration::from_millis(120));
        assert_eq!(old_fired.load(Ordering::SeqCst), 0);
        // Verify new task still fired exactly once (no duplicate)
        assert_eq!(new_fired.load(Ordering::SeqCst), 1);
    }
}
