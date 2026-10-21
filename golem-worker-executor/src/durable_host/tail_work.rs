// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Activity tracking for Golem-spawned store background tasks ("tail work").
//!
//! Durable host operations spawn background tasks onto the wasmtime store
//! (`store.spawn(AccessorTask)`): HTTP body recorders, TCP send/receive
//! drivers, filesystem/stdio stream consumers, blobstore/keyvalue value
//! consumers. `run_concurrent` returns as soon as the invocation's root
//! future resolves, leaving these tasks suspended wherever their last poll
//! left them. The invocation completion path must keep the store's event
//! loop running until none of them is still *active* — otherwise a task's
//! durable `Start`/`End` entries could land after `AgentInvocationFinished`
//! (breaking positional replay), or never be appended at all.
//!
//! A spawned task counts as **active** from the moment it is spawned until
//! it finishes, *except* while it is parked at a designated **safe park
//! point**: a wait that may legitimately span invocations because it cannot
//! produce durable work until a future guest action or replay-tail progress.
//! Safe park points are:
//!
//! * awaiting guest demand (e.g. a consume-body / TCP-receive demand
//!   channel, or the demand-gating oneshot of the request-body transmission
//!   recorder);
//! * awaiting guest-produced stream data (stdio capture, filesystem write
//!   chunks, TCP send bytes, replayed request-body frames);
//! * awaiting a fresh guest block after async input preparation is complete;
//!   only guest progress can admit this delivery, and no durable effect remains;
//! * passive replay-progress notification for a markerless completion whose
//!   durable finalization is finished. No recorded entry depends on that
//!   unobserved delivery, and invocation settlement may own the remaining tail.
//!
//! Waits whose completion is *not* guest-driven — durable oplog appends,
//! live network / file I/O, replay-resolver waits, queued or running cursor
//! transactions, and recorded delivery-marker waits — keep the task active:
//! the completion path waits for them (bounded by the invocation tail-drain
//! timeout).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Per-worker counter of Golem-spawned store tasks that are currently
/// active (not finished and not parked at a safe park point). See the
/// module documentation.
#[derive(Clone, Debug, Default)]
pub struct TailWorkTracker {
    active: Arc<AtomicUsize>,
}

impl TailWorkTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one spawned task as active, returning its RAII activity
    /// token. Create the token *before* spawning the task and move it into
    /// the task, so the task is counted from the instant it is spawned
    /// (a spawned task is not polled until the next event-loop scope).
    pub fn activity(&self) -> TailActivity {
        TailActivity::new(self.active.clone())
    }

    /// Whether any spawned task is currently active. `false` means every
    /// Golem-spawned store task has either finished or is parked at a safe
    /// park point.
    pub fn has_active(&self) -> bool {
        self.active.load(Ordering::Acquire) > 0
    }

    /// Number of currently active spawned tasks (diagnostics only).
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

/// RAII activity token owned by one spawned store task: the task counts as
/// active while the token exists, except while parked via [`Self::park`].
/// Dropping the token (task finished, or its future was dropped) removes it
/// from the count.
#[derive(Debug)]
pub struct TailActivity {
    counter: Arc<AtomicUsize>,
}

impl TailActivity {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }

    pub fn tracker(&self) -> TailWorkTracker {
        TailWorkTracker {
            active: self.counter.clone(),
        }
    }

    /// Awaits `fut` with this task marked inactive: use it around — and only
    /// around — safe park points (see the module documentation). The task is
    /// re-marked active before this returns, and stays balanced if the task
    /// future is dropped mid-park.
    pub async fn park<F: Future>(&self, fut: F) -> F::Output {
        let _parked = ParkGuard::new(&self.counter);
        fut.await
    }
}

impl Drop for TailActivity {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Inverse guard used by [`TailActivity::park`]: decrements the active count
/// while it exists and restores it on drop, so a task dropped while parked
/// nets out correctly against [`TailActivity`]'s own drop.
struct ParkGuard<'a> {
    counter: &'a Arc<AtomicUsize>,
}

impl<'a> ParkGuard<'a> {
    fn new(counter: &'a Arc<AtomicUsize>) -> Self {
        counter.fetch_sub(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for ParkGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn activity_counts_from_creation_to_drop() {
        let tracker = TailWorkTracker::default();
        assert!(!tracker.has_active());
        let activity = tracker.activity();
        assert!(tracker.has_active());
        assert_eq!(tracker.active_count(), 1);
        drop(activity);
        assert!(!tracker.has_active());
    }

    #[test]
    fn park_releases_and_restores_activity() {
        let tracker = TailWorkTracker::default();
        let activity = tracker.activity();

        futures::executor::block_on(activity.park(async {
            assert!(!tracker.has_active());
        }));
        assert!(tracker.has_active());
        drop(activity);
        assert!(!tracker.has_active());
    }

    #[test]
    fn dropping_task_while_parked_stays_balanced() {
        let tracker = TailWorkTracker::default();
        let activity = tracker.activity();

        let mut parked = Box::pin(activity.park(std::future::pending::<()>()));
        // Poll once so the park guard is created.
        futures::executor::block_on(async {
            let poll = futures::poll!(parked.as_mut());
            assert!(poll.is_pending());
        });
        assert!(!tracker.has_active());

        // Dropping the parked future and then the activity token must net to zero.
        drop(parked);
        assert!(tracker.has_active());
        drop(activity);
        assert!(!tracker.has_active());
    }
}

mod context {
    use crate::durable_host::*;

    impl<Ctx: WorkerCtx> Drop for DurableWorkerCtx<Ctx> {
        fn drop(&mut self) {
            self.begin_stream_runtime_teardown();
            self.linear_memory.clear_limit_exceeded_callback();
        }
    }

    impl InvocationDeadline {
        /// Whether the deadline fired during this invocation.
        pub fn exceeded(&self) -> bool {
            self.latch.load(Ordering::Acquire)
        }

        /// The configured maximum invocation duration, if any.
        pub fn duration(&self) -> Option<Duration> {
            self.duration
        }
    }

    impl Drop for InvocationDeadline {
        fn drop(&mut self) {
            if let Some(timer) = self.timer.take() {
                timer.abort();
            }
            self.latch.store(false, Ordering::Release);
        }
    }

    impl TailWorkDeadline {
        pub(crate) fn exceeded(&self) -> bool {
            self.latch.load(Ordering::Acquire)
        }

        pub(crate) fn duration(&self) -> Duration {
            self.duration
        }
    }

    impl Drop for TailWorkDeadline {
        fn drop(&mut self) {
            if let Some(timer) = self.timer.take() {
                timer.abort();
            }
            self.latch.store(false, Ordering::Release);
        }
    }

    impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
        /// Activity tracker for Golem-spawned store background tasks; see
        /// [`tail_work::TailWorkTracker`].
        pub fn tail_work_tracker(&self) -> tail_work::TailWorkTracker {
            self.state.tail_work_tracker()
        }

        pub(crate) fn stream_runtime_teardown_probe(
            &self,
        ) -> Arc<dyn Fn() -> bool + Send + Sync + 'static> {
            let stream_runtime_teardown = self.stream_runtime_teardown.clone();
            // Owner failure fences entity Stores immediately. The primary signals teardown
            // separately when its interrupt is delivered, not when it is merely queued.
            let fenced_entity_operations = matches!(self.runtime, OwnerRuntime::Entity(_))
                .then(|| self.owner_execution.tool_operations());
            let invocation_loops = self
                .public_state
                .worker()
                .active_agents()
                .invocation_loops();
            Arc::new(move || {
                stream_runtime_teardown.load(Ordering::Acquire)
                    || invocation_loops.is_shut_down()
                    || fenced_entity_operations
                        .as_ref()
                        .is_some_and(|operations| operations.selected_owner_failure().is_some())
            })
        }

        pub(crate) fn begin_stream_runtime_teardown(&self) {
            self.stream_runtime_teardown.store(true, Ordering::Release);
        }

        /// Arms the optional per-invocation wall-clock deadline (`limits.max_invocation_duration`)
        /// and returns its guard. Called at the start of every guest invocation.
        ///
        /// When configured, a timer task is spawned that, once the deadline elapses, latches the
        /// shared `invocation_deadline_exceeded` flag and broadcasts a *synthetic* interrupt on the
        /// running execution status's interrupt-signal channel — without changing the execution
        /// status itself, so the worker is never externally observed as `Interrupted`. The broadcast
        /// wakes every cooperative host park point already racing the interrupt signal; the latched
        /// flag makes `check_interrupt` (epoch callback, CPU-bound wasm) and every *subsequently
        /// created* interrupt signal observe the deadline too. The invocation boundary converts the
        /// resulting synthetic interrupt unwind into a typed timeout failure (see
        /// `apply_invocation_deadline` in `worker::invocation`).
        ///
        /// Dropping the guard (at the invocation boundary) aborts the timer and clears the latch;
        /// arming also clears it first, so a stale latch from a lost abort race cannot leak into the
        /// next invocation.
        pub fn arm_invocation_deadline(&self) -> InvocationDeadline {
            let latch = self.state.invocation_deadline_exceeded.clone();
            latch.store(false, Ordering::Release);
            let duration = self.state.config.limits.max_invocation_duration;
            let timer = duration.map(|duration| {
                let latch = latch.clone();
                let execution_status = self.execution_status.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    latch.store(true, Ordering::Release);
                    let interrupt_signal = {
                        let status = execution_status.read().unwrap();
                        match &*status {
                            ExecutionStatus::Running {
                                interrupt_signal, ..
                            } => Some(interrupt_signal.clone()),
                            _ => None,
                        }
                    };
                    if let Some(interrupt_signal) = interrupt_signal {
                        let _ =
                            interrupt_signal.send(InterruptKind::Interrupt(Timestamp::now_utc()));
                    }
                })
            });
            InvocationDeadline {
                latch,
                duration,
                timer,
            }
        }

        /// Arms a deadline that starts when `drain_started` is notified after the guest export has
        /// returned. When it fires, the shared latch and interrupt broadcast make both existing and
        /// subsequently-created cooperative park points unwind their durable call handles safely.
        pub(crate) fn arm_tail_work_deadline(
            &self,
            drain_started: Arc<tokio::sync::Notify>,
        ) -> TailWorkDeadline {
            let latch = self.state.tail_work_deadline_exceeded.clone();
            latch.store(false, Ordering::Release);
            let timer_latch = latch.clone();
            let execution_status = self.execution_status.clone();
            let duration = self.state.config.limits.tail_work_settle_timeout;
            let timer = tokio::spawn(async move {
                drain_started.notified().await;
                tokio::time::sleep(duration).await;
                timer_latch.store(true, Ordering::Release);
                let interrupt_signal = {
                    let status = execution_status.read().unwrap();
                    match &*status {
                        ExecutionStatus::Running {
                            interrupt_signal, ..
                        } => Some(interrupt_signal.clone()),
                        _ => None,
                    }
                };
                if let Some(interrupt_signal) = interrupt_signal {
                    let _ = interrupt_signal.send(InterruptKind::Interrupt(Timestamp::now_utc()));
                }
            });
            TailWorkDeadline {
                latch,
                duration,
                timer: Some(timer),
            }
        }

        pub(crate) fn invocation_deadline_exceeded(&self) -> bool {
            self.state
                .invocation_deadline_exceeded
                .load(Ordering::Acquire)
        }
    }
}
