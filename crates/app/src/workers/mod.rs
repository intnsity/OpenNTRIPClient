//! Worker threads and the plumbing they share.

pub mod ntrip;
pub mod serial;
pub mod tls;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Bounded corrections queue between the NTRIP worker (producer) and the
/// serial worker (consumer). On overflow the OLDEST block is dropped and a
/// visible overrun counter ticks - stale corrections are worthless and the
/// drop itself is a diagnostic ("your receiver link is slower than the
/// stream"). A plain mpsc sync_channel cannot drop-oldest from the sender
/// side, hence this hand-rolled Mutex+Condvar deque.
pub struct CorrQueue {
    q: Mutex<VecDeque<Vec<u8>>>,
    cv: Condvar,
    cap: usize,
    /// True only while a serial worker is draining. Pushes while inactive
    /// are discarded silently: with no receiver attached there is no overrun
    /// to report, and corrections must not pile up.
    active: AtomicBool,
    overruns: AtomicU64,
}

pub enum PushOutcome {
    /// No serial worker attached; bytes discarded by design.
    Inactive,
    Queued,
    /// Queue was full: the oldest block was dropped. Carries the new
    /// cumulative overrun count.
    DroppedOldest(u64),
}

impl CorrQueue {
    pub fn new(cap: usize) -> Self {
        CorrQueue {
            q: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            cap: cap.max(1),
            active: AtomicBool::new(false),
            overruns: AtomicU64::new(0),
        }
    }

    /// Serial worker attach/detach. Detaching clears queued blocks so a
    /// later session never replays a stale backlog.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
        if !active && let Ok(mut q) = self.q.lock() {
            q.clear();
        }
    }

    pub fn push(&self, bytes: Vec<u8>) -> PushOutcome {
        let Ok(mut q) = self.q.lock() else {
            return PushOutcome::Inactive;
        };
        // Checked UNDER the queue lock: set_active(false) stores the flag and
        // then clears while holding this same lock, so a push observing
        // `active` here either lands before that clear (and is removed by
        // it) or sees false. Checking before locking left a window where one
        // stale block could slip in after the clear and replay into the next
        // serial session.
        if !self.active.load(Ordering::SeqCst) {
            return PushOutcome::Inactive;
        }
        let mut outcome = PushOutcome::Queued;
        if q.len() >= self.cap {
            q.pop_front();
            let n = self.overruns.fetch_add(1, Ordering::SeqCst) + 1;
            outcome = PushOutcome::DroppedOldest(n);
        }
        q.push_back(bytes);
        drop(q);
        self.cv.notify_one();
        outcome
    }

    /// Non-blocking pop for the serial worker's drain-everything step.
    pub fn try_pop(&self) -> Option<Vec<u8>> {
        self.q.lock().ok()?.pop_front()
    }

    /// Blocking pop with timeout; the serial worker's idle wait.
    pub fn pop_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        let q = self.q.lock().ok()?;
        let (mut q, _) = self
            .cv
            .wait_timeout_while(q, timeout, |q| q.is_empty())
            .ok()?;
        q.pop_front()
    }

    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::SeqCst)
    }
}

/// Join a worker with a deadline. Returns false if the thread is still
/// running when the timeout expires (the thread is then abandoned - the
/// process is exiting anyway, and blocking exit forever would be worse).
pub fn join_timeout(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.join().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_queue_discards() {
        let q = CorrQueue::new(4);
        assert!(matches!(q.push(vec![1]), PushOutcome::Inactive));
        assert!(q.try_pop().is_none());
        assert_eq!(q.overruns(), 0);
    }

    #[test]
    fn overflow_drops_oldest_and_counts() {
        let q = CorrQueue::new(2);
        q.set_active(true);
        assert!(matches!(q.push(vec![1]), PushOutcome::Queued));
        assert!(matches!(q.push(vec![2]), PushOutcome::Queued));
        let PushOutcome::DroppedOldest(n) = q.push(vec![3]) else {
            panic!("expected overflow");
        };
        assert_eq!(n, 1);
        assert_eq!(q.try_pop(), Some(vec![2]), "oldest (1) was dropped");
        assert_eq!(q.try_pop(), Some(vec![3]));
        assert_eq!(q.overruns(), 1);
    }

    #[test]
    fn deactivate_clears_backlog() {
        let q = CorrQueue::new(4);
        q.set_active(true);
        q.push(vec![1]);
        q.set_active(false);
        q.set_active(true);
        assert!(q.try_pop().is_none(), "stale backlog must not survive");
    }

    /// The no-stale-replay invariant under contention: once set_active(false)
    /// returns, the queue is empty and stays empty, no matter how a
    /// concurrent producer's push interleaved with the deactivation. Guards
    /// the check-active-under-the-lock ordering in push().
    #[test]
    fn concurrent_push_never_survives_deactivation() {
        let q = std::sync::Arc::new(CorrQueue::new(8));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let producer = {
            let q = q.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    q.push(vec![0u8; 8]);
                }
            })
        };
        for _ in 0..2_000 {
            q.set_active(true);
            std::hint::spin_loop();
            q.set_active(false);
            assert!(
                q.try_pop().is_none(),
                "a push slipped past set_active(false)'s clear"
            );
        }
        stop.store(true, Ordering::Relaxed);
        producer.join().unwrap();
    }

    #[test]
    fn pop_timeout_wakes_on_push() {
        let q = std::sync::Arc::new(CorrQueue::new(4));
        q.set_active(true);
        let q2 = q.clone();
        let t = std::thread::spawn(move || q2.pop_timeout(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(30));
        q.push(vec![9]);
        assert_eq!(t.join().unwrap(), Some(vec![9]));
    }
}
