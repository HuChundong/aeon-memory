// port of src/utils/serial-queue.ts — single-consumer serial task queue.

use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct QueueState {
    pending: Vec<Box<dyn FnOnce() + Send>>,
    running: bool,
    destroyed: bool,
}

/// A single-consumer serial queue: tasks execute one at a time in FIFO order.
/// Port of SerialQueue from src/utils/serial-queue.ts.
pub struct SerialQueue {
    name: String,
    state: Arc<(Mutex<QueueState>, Condvar)>,
}

impl SerialQueue {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: Arc::new((
                Mutex::new(QueueState {
                    pending: Vec::new(),
                    running: false,
                    destroyed: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Add a task to the queue. If no task is currently running, starts
    /// processing in a new thread.
    pub fn add<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let state = self.state.clone();
        let mut guard = state.0.lock().unwrap();
        guard.pending.push(Box::new(task));

        if !guard.running && !guard.destroyed {
            guard.running = true;
            let worker_state = state.clone();
            thread::spawn(move || {
                loop {
                    let task = {
                        let mut g = worker_state.0.lock().unwrap();
                        if g.destroyed {
                            g.running = false;
                            worker_state.1.notify_all();
                            return;
                        }
                        if g.pending.is_empty() {
                            g.running = false;
                            worker_state.1.notify_all();
                            return;
                        }
                        g.pending.remove(0)
                    };
                    task();
                }
            });
        }
    }

    /// Whether the queue has no pending tasks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of pending tasks.
    pub fn len(&self) -> usize {
        self.state.0.lock().unwrap().pending.len()
    }

    /// Whether the queue is idle (no pending or running tasks).
    pub fn is_idle(&self) -> bool {
        let g = self.state.0.lock().unwrap();
        !g.running && g.pending.is_empty()
    }

    /// Wait until all pending tasks complete and the queue becomes idle.
    pub fn wait_for_idle(&self) {
        let mut guard = self.state.0.lock().unwrap();
        while guard.running || !guard.pending.is_empty() {
            guard = self.state.1.wait(guard).unwrap();
        }
    }

    /// Destroy the queue, preventing new tasks from being processed.
    pub fn destroy(&self) {
        let mut guard = self.state.0.lock().unwrap();
        guard.destroyed = true;
        guard.pending.clear();
        self.state.1.notify_all();
    }
}

impl Drop for SerialQueue {
    fn drop(&mut self) {
        self.destroy();
    }
}

unsafe impl Send for SerialQueue {}
unsafe impl Sync for SerialQueue {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_empty_queue() {
        let q = SerialQueue::new("test");
        assert!(q.is_idle());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn test_single_task() {
        let q = SerialQueue::new("single");
        let flag = Arc::new(AtomicUsize::new(0));
        let f = flag.clone();
        q.add(move || {
            f.store(1, Ordering::SeqCst);
        });
        q.wait_for_idle();
        assert_eq!(flag.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_ordered_execution() {
        let q = SerialQueue::new("ordered");
        let results = Arc::new(Mutex::new(Vec::new()));

        for i in 0..5 {
            let r = results.clone();
            q.add(move || {
                std::thread::sleep(std::time::Duration::from_millis(5));
                r.lock().unwrap().push(i);
            });
        }
        q.wait_for_idle();

        let final_results = results.lock().unwrap().clone();
        assert_eq!(final_results, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_multiple_queues() {
        let q1 = SerialQueue::new("q1");
        let q2 = SerialQueue::new("q2");
        let r1 = Arc::new(AtomicUsize::new(0));
        let r2 = Arc::new(AtomicUsize::new(0));

        let a = r1.clone();
        q1.add(move || {
            a.store(1, Ordering::SeqCst);
        });
        let b = r2.clone();
        q2.add(move || {
            b.store(2, Ordering::SeqCst);
        });

        q1.wait_for_idle();
        q2.wait_for_idle();
        assert_eq!(r1.load(Ordering::SeqCst), 1);
        assert_eq!(r2.load(Ordering::SeqCst), 2);
    }
}
