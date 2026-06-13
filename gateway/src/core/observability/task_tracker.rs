//! D-G4: per-session task tracking + bounded dangling-task audit.
//!
//! Pipecat warns about leaked tasks at pipeline end (`_print_dangling_tasks`,
//! `worker.py:1290`). WaaV spawns several **session-lifetime** tasks — the
//! outgoing-message sender, the LiveKit audio forwarder, the DAG output drain —
//! that run until the connection drops. This tracker records them by label and
//! cleans them up at teardown.
//!
//! Teardown follows Pipecat's model: **cancel** the tracked tasks, then warn
//! about + count only the ones that *resist* cancellation
//! (`waav_session_dangling_tasks_total`). A session-lifetime loop blocked on
//! `recv().await` (its sender still held by `ConnectionState` until the
//! connection drops) cancels instantly at that await point and is clean — it
//! must NOT be counted, or the metric would fire on every session. The real
//! "dangling" signal is a task stuck in a non-yielding / blocking section (e.g.
//! a `spawn_blocking` closure) that an abort cannot interrupt.
//!
//! The tracker is provider-agnostic — any subsystem that spawns a session-scoped
//! task registers it through the same [`track`](SessionTaskTracker::track) API.

use std::borrow::Cow;
use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::warn;

/// Grace window after aborting tracked tasks before declaring survivors
/// "dangling". Cancellation is a yield point, so a `recv`-blocked loop finishes
/// well within this; only a wedged blocking section outlives it.
pub const DEFAULT_TEARDOWN_GRACE: Duration = Duration::from_millis(150);

/// A single tracked session-lifetime task.
struct TrackedTask {
    label: Cow<'static, str>,
    handle: JoinHandle<()>,
}

/// Records session-scoped [`JoinHandle`]s and audits them at teardown.
///
/// Interior mutability ([`Mutex`]) so registration takes `&self` — the tracker
/// is shared (`Arc`) across the handler and the per-session setup code that
/// spawns driver loops, all behind the connection's `RwLock` read guard. The
/// mutex is held only for `push`/`take`, never across an `.await`.
pub struct SessionTaskTracker {
    tasks: Mutex<Vec<TrackedTask>>,
}

impl SessionTaskTracker {
    /// A fresh, empty tracker.
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// Register a session-lifetime task so it is cancelled (and audited) at
    /// teardown. Cheap: one push under an uncontended mutex.
    pub fn track(&self, label: impl Into<Cow<'static, str>>, handle: JoinHandle<()>) {
        self.lock().push(TrackedTask {
            label: label.into(),
            handle,
        });
    }

    /// Number of tasks currently tracked (inspection/test aid).
    pub fn tracked_count(&self) -> usize {
        self.lock().len()
    }

    /// Teardown audit. **Aborts** every tracked task (the intended cancellation
    /// of session-lifetime loops), waits `grace` for the cancellations to land,
    /// then warns about + counts (`waav_session_dangling_tasks_total`) only the
    /// tasks that *still* haven't finished — i.e. resisted cancellation. Drains
    /// the tracker (so the [`Drop`] backstop is a no-op afterward) and returns
    /// the number of genuinely-dangling tasks.
    pub async fn abort_and_audit(&self, grace: Duration) -> usize {
        let drained: Vec<TrackedTask> = std::mem::take(&mut *self.lock());
        if drained.is_empty() {
            return 0;
        }
        // Cancel everything first — this is how session-lifetime loops (blocked
        // on a recv whose sender ConnectionState still holds) are stopped.
        for task in &drained {
            task.handle.abort();
        }
        // One shared grace window: a cancellable task yields at its next await
        // point and finishes almost immediately; this bounds the wait at a
        // single `grace` regardless of task count.
        tokio::time::sleep(grace).await;
        let mut dangling = 0;
        for task in drained {
            if task.handle.is_finished() {
                continue;
            }
            dangling += 1;
            warn!(
                task = %task.label,
                grace_ms = grace.as_millis() as u64,
                "session task resisted cancellation at teardown — leaked (D-G4)"
            );
            crate::core::metrics::bridge::record_session_dangling_task();
        }
        dangling
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TrackedTask>> {
        // Poisoning here only means a prior panic touched the Vec; the data is
        // still structurally sound, so recover rather than propagate the panic
        // into teardown (which must always make progress).
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for SessionTaskTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionTaskTracker {
    fn drop(&mut self) {
        // Backstop for the panic / early-return path where `abort_and_audit`
        // was never called: never leave a tracked task running past the session
        // (mirrors `WebSocketTtsClient`'s Drop abort).
        let tasks = self.tasks.get_mut().unwrap_or_else(|e| e.into_inner());
        for task in tasks.drain(..) {
            task.handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    const TEST_GRACE: Duration = Duration::from_millis(100);

    /// An async task whose `Drop` flips a flag — lets a test prove an abort
    /// actually cancelled it. Blocked on `sleep().await`, it cancels cleanly at
    /// that await point (a normal session loop), so it is NOT "dangling".
    fn cancellable_task(aborted: Arc<AtomicBool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            struct Guard(Arc<AtomicBool>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _g = Guard(aborted);
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        })
    }

    /// A blocking task that an `abort` cannot interrupt — `spawn_blocking`
    /// closures run to natural completion. This is the genuine "dangling /
    /// resisted cancellation" signal. It blocks on a channel `recv`, so the test
    /// controls exactly when it exits (by dropping the returned sender) — making
    /// the assertion independent of wall-clock timing / machine load.
    ///
    /// Returns a `started` flag too: a `spawn_blocking` task aborted *before* it
    /// is scheduled onto the blocking pool is cancelled pre-start (and would look
    /// finished), so a test must wait for `started` before auditing.
    fn resists_cancellation() -> (JoinHandle<()>, std::sync::mpsc::Sender<()>, Arc<AtomicBool>) {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let started = Arc::new(AtomicBool::new(false));
        let started_in = started.clone();
        let handle = tokio::task::spawn_blocking(move || {
            started_in.store(true, Ordering::SeqCst);
            // Returns only once the test drops `release_tx`; abort() cannot
            // interrupt a blocking recv on a spawn_blocking thread.
            let _ = release_rx.recv();
        });
        (handle, release_tx, started)
    }

    /// Spin (async) until a flag is set, bounded so a stuck test still ends.
    async fn await_flag(flag: &AtomicBool) -> bool {
        for _ in 0..200 {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        flag.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn tracks_and_counts() {
        let tracker = SessionTaskTracker::new();
        tracker.track("a", cancellable_task(Arc::new(AtomicBool::new(false))));
        tracker.track("b", cancellable_task(Arc::new(AtomicBool::new(false))));
        assert_eq!(tracker.tracked_count(), 2);
        let _ = tracker.abort_and_audit(TEST_GRACE).await;
    }

    #[tokio::test]
    async fn finished_task_is_not_dangling() {
        let tracker = SessionTaskTracker::new();
        let mut h = tokio::spawn(async {});
        while !h.is_finished() {
            tokio::task::yield_now().await;
            let _ = futures::poll!(&mut h);
        }
        tracker.track("done", h);
        assert_eq!(tracker.abort_and_audit(TEST_GRACE).await, 0);
        assert_eq!(tracker.tracked_count(), 0, "audit drains the tracker");
    }

    #[tokio::test]
    async fn cancellable_loop_is_aborted_but_not_dangling() {
        // The common case: a session-lifetime loop blocked on recv/sleep. It
        // cancels cleanly at its await point, so it is aborted (drop guard runs)
        // yet reported as ZERO dangling — the metric must not fire every session.
        let tracker = SessionTaskTracker::new();
        let aborted = Arc::new(AtomicBool::new(false));
        tracker.track("recv-loop", cancellable_task(aborted.clone()));
        tokio::task::yield_now().await;

        assert_eq!(
            tracker.abort_and_audit(TEST_GRACE).await,
            0,
            "a cancellable loop is not a dangling task"
        );
        assert_eq!(tracker.tracked_count(), 0, "audit drains the tracker");
        assert!(
            await_flag(&aborted).await,
            "the loop was actually cancelled (its drop guard ran)"
        );
    }

    #[tokio::test]
    async fn resisting_task_is_reported_dangling() {
        // A task that ignores abort (blocking section) outlives the grace and
        // IS the dangling signal.
        let tracker = SessionTaskTracker::new();
        let (handle, release_tx, started) = resists_cancellation();
        tracker.track("blocking", handle);
        // Wait until the closure is actually executing on the blocking pool, so
        // the abort can't cancel it pre-start.
        assert!(await_flag(&started).await, "blocking closure must start");
        assert_eq!(
            tracker.abort_and_audit(TEST_GRACE).await,
            1,
            "a task that resists cancellation is reported dangling"
        );
        assert_eq!(tracker.tracked_count(), 0, "audit drains the tracker");
        drop(release_tx); // let the blocking thread exit cleanly
    }

    #[tokio::test]
    async fn mixed_finished_cancellable_and_resisting() {
        let tracker = SessionTaskTracker::new();
        // finished
        let mut done = tokio::spawn(async {});
        while !done.is_finished() {
            tokio::task::yield_now().await;
            let _ = futures::poll!(&mut done);
        }
        tracker.track("done", done);
        // cancellable
        tracker.track("recv-loop", cancellable_task(Arc::new(AtomicBool::new(false))));
        // resisting
        let (handle, release_tx, started) = resists_cancellation();
        tracker.track("blocking", handle);
        assert!(await_flag(&started).await, "blocking closure must start");
        assert_eq!(tracker.tracked_count(), 3);
        assert_eq!(
            tracker.abort_and_audit(TEST_GRACE).await,
            1,
            "only the cancellation-resisting task counts as dangling"
        );
        drop(release_tx); // let the blocking thread exit cleanly
    }

    #[tokio::test]
    async fn recv_loop_with_live_sender_is_not_dangling() {
        // Mirrors the real DAG-drain / LiveKit-forwarder teardown: the loop is
        // blocked on `recv().await` while its SENDER is still alive elsewhere
        // (in production, `output_tx` in `state.dag_context` / `frame_tx` in the
        // audio callback). The audit must still cancel it cleanly at the recv
        // await point and report ZERO dangling — the false positive the brutal
        // review caught against the old immediate-`is_finished` design.
        let tracker = SessionTaskTracker::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(4);
        let handle = tokio::spawn(async move {
            while rx.recv().await.is_some() {} // parked on recv; sender held below
        });
        tracker.track("drain-like", handle);
        tokio::task::yield_now().await;

        // The sender `tx` is deliberately still held here — exactly the
        // production condition where ConnectionState keeps `output_tx` alive.
        assert_eq!(
            tracker.abort_and_audit(TEST_GRACE).await,
            0,
            "a recv-blocked loop with a live sender is cancelled cleanly, not flagged dangling"
        );
        assert_eq!(tracker.tracked_count(), 0, "audit drains the tracker");
        drop(tx);
    }

    #[tokio::test]
    async fn empty_audit_is_zero() {
        let tracker = SessionTaskTracker::new();
        assert_eq!(tracker.abort_and_audit(TEST_GRACE).await, 0);
    }

    #[tokio::test]
    async fn drop_backstop_aborts_unaudited_tasks() {
        let aborted = Arc::new(AtomicBool::new(false));
        {
            let tracker = SessionTaskTracker::new();
            tracker.track("recv-loop", cancellable_task(aborted.clone()));
            tokio::task::yield_now().await;
            // Drop WITHOUT auditing (panic / early-return path).
        }
        assert!(
            await_flag(&aborted).await,
            "dropping the tracker aborts tasks that were never audited"
        );
    }
}
