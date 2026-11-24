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
//! (`store.spawn(async move |accessor| ...)`): HTTP body recorders, TCP send/receive
//! drivers, filesystem/stdio stream consumers, blobstore/keyvalue value
//! consumers. `run_concurrent` returns as soon as the invocation's root
//! future resolves, leaving these tasks suspended wherever their last poll
//! left them. The invocation completion path must keep the store's event
//! loop running until none of them is still *active* — otherwise a task's
//! durable `Start`/`End` entries could land after the invocation that counts
//! the task settled (`AgentInvocationSettled` for a result recorded at the
//! export's return, `AgentInvocationFinished` otherwise), breaking positional
//! replay, or never be appended at all.
//!
//! A task that resumes from a park counts on the invocation the loop runs then,
//! which drives it from then on: its entries land before that invocation
//! settles, and a `Start` it records names that invocation as its `driver`. A
//! parked task whose live work needs a driving invocation waits for one
//! ([`TailActivity::park_until_driven`]).
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
//!   unobserved delivery, and invocation settlement may own the remaining tail;
//! * a destructor's passive wait for sibling history or the natural replay tail.
//!   Its cursor transactions stay active, and it cannot perform a missing live
//!   effect until the normal replay-to-live transition authorizes continuation;
//! * passive replay-progress notification for a recorded delivery marker while an
//!   invocation boundary (a terminal, an `AgentInvocationSettled` or an
//!   `AgentInvocationStarted`) lies between the cursor and the marker. A reader
//!   retained across invocations is consumed in a later invocation, which cannot
//!   replay until this one settles.
//!
//! Waits whose completion is *not* guest-driven — durable oplog appends,
//! live network / file I/O, ordinary replay-resolver waits, queued or running cursor
//! transactions, and waits for a delivery marker recorded in the current
//! invocation — keep the task active:
//! the completion path waits for them (bounded by `tail_work_settle_timeout` on
//! the settled path; the drain after a result recorded at the export's return is
//! unbounded).

use golem_common::model::oplog::OplogIndex;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// The invocation the loop runs: its `AgentInvocationStarted` index and its activity counter.
#[derive(Clone, Debug)]
struct CurrentInvocation {
    started_index: OplogIndex,
    active: Arc<AtomicUsize>,
}

/// Per-worker counter of Golem-spawned store tasks that are currently
/// active (not finished and not parked at a safe park point). See the
/// module documentation.
///
/// A tracker can also count on the activity of one invocation: the invocation
/// attributed when the tracker was obtained. Every activity created from it counts
/// on that invocation. When a parked task resumes while the loop runs a later
/// invocation, its activity counts on that later invocation, the one that drives it
/// from then on. The invocation loop settles on the per-worker count only.
#[derive(Clone, Debug)]
pub struct TailWorkTracker {
    active: Arc<AtomicUsize>,
    invocation: Option<Arc<AtomicUsize>>,
    current: Arc<watch::Sender<Option<CurrentInvocation>>>,
}

impl Default for TailWorkTracker {
    fn default() -> Self {
        Self {
            active: Arc::default(),
            invocation: None,
            current: Arc::new(watch::Sender::new(None)),
        }
    }
}

impl TailWorkTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// This per-worker tracker, also counting on the activity of `invocation`.
    pub(crate) fn counting_on(&self, invocation: &TailWorkTracker) -> Self {
        Self {
            active: self.active.clone(),
            invocation: Some(invocation.active.clone()),
            current: self.current.clone(),
        }
    }

    /// Records that the loop runs the invocation started at `started_index`, whose activity
    /// `invocation` counts. Resumed tasks count on it from then on.
    pub(crate) fn begin_invocation(&self, started_index: OplogIndex, invocation: &TailWorkTracker) {
        self.current.send_replace(Some(CurrentInvocation {
            started_index,
            active: invocation.active.clone(),
        }));
    }

    /// Records that the loop runs no open invocation: it begins the next one, or the current one
    /// closed.
    pub(crate) fn end_invocation(&self) {
        self.current.send_replace(None);
    }

    /// The `AgentInvocationStarted` index of the invocation the loop runs.
    pub(crate) fn current_invocation(&self) -> Option<OplogIndex> {
        self.current
            .borrow()
            .as_ref()
            .map(|current| current.started_index)
    }

    fn current_counter(&self) -> Option<Arc<AtomicUsize>> {
        self.current
            .borrow()
            .as_ref()
            .map(|current| current.active.clone())
    }

    /// Registers one spawned task as active, returning its RAII activity
    /// token. Create the token *before* spawning the task and move it into
    /// the task, so the task is counted from the instant it is spawned
    /// (a spawned task is not polled until the next event-loop scope).
    pub fn activity(&self) -> TailActivity {
        TailActivity::new(self.clone())
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
    tracker: TailWorkTracker,
    /// The invocation counter the activity counts on now.
    counted: Mutex<Option<Arc<AtomicUsize>>>,
}

impl TailActivity {
    fn new(tracker: TailWorkTracker) -> Self {
        let counted = tracker.invocation.clone();
        let activity = Self {
            tracker,
            counted: Mutex::new(counted),
        };
        activity.add();
        activity
    }

    /// A tracker for the activities this task creates: they count on the invocation this
    /// activity counts on now.
    pub fn tracker(&self) -> TailWorkTracker {
        TailWorkTracker {
            active: self.tracker.active.clone(),
            invocation: self.counted.lock().unwrap().clone(),
            current: self.tracker.current.clone(),
        }
    }

    /// Awaits `fut` with this task marked inactive: use it around — and only
    /// around — safe park points (see the module documentation). The task is
    /// re-marked active before this returns, and stays balanced if the task
    /// future is dropped mid-park. It counts on the invocation the loop runs
    /// when it resumes.
    pub async fn park<F: Future>(&self, fut: F) -> F::Output {
        let _parked = ParkGuard::new(self);
        fut.await
    }

    /// Parks until the loop runs an invocation that `drives` accepts, given its
    /// `AgentInvocationStarted` index, and counts on that invocation from then on. A task that
    /// performs live work after its spawning invocation closed waits here for the invocation that
    /// drives that work.
    pub async fn park_until_driven(&self, drives: impl Fn(OplogIndex) -> bool) {
        loop {
            let mut current = self.tracker.current.subscribe();
            let driving = current
                .borrow_and_update()
                .as_ref()
                .filter(|current| drives(current.started_index))
                .map(|current| current.active.clone());
            if let Some(driving) = driving {
                self.count_on(driving);
                return;
            }
            // The tracker holds the sender for as long as this activity exists.
            let _ = self.park(current.changed()).await;
        }
    }

    fn add(&self) {
        self.tracker.active.fetch_add(1, Ordering::AcqRel);
        if let Some(invocation) = &*self.counted.lock().unwrap() {
            invocation.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn sub(&self) {
        self.tracker.active.fetch_sub(1, Ordering::AcqRel);
        if let Some(invocation) = &*self.counted.lock().unwrap() {
            invocation.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Moves the count of this active activity to `invocation`.
    fn count_on(&self, invocation: Arc<AtomicUsize>) {
        let mut counted = self.counted.lock().unwrap();
        if counted
            .as_ref()
            .is_some_and(|counted| Arc::ptr_eq(counted, &invocation))
        {
            return;
        }
        invocation.fetch_add(1, Ordering::AcqRel);
        if let Some(previous) = counted.replace(invocation) {
            previous.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Counts the resumed activity on the invocation the loop runs, if it runs one.
    fn resume(&self) {
        if let Some(current) = self.tracker.current_counter() {
            *self.counted.lock().unwrap() = Some(current);
        }
        self.add();
    }
}

impl Drop for TailActivity {
    fn drop(&mut self) {
        self.sub();
    }
}

/// Inverse guard used by [`TailActivity::park`]: decrements the active count
/// while it exists and restores it on drop, so a task dropped while parked
/// nets out correctly against [`TailActivity`]'s own drop.
struct ParkGuard<'a> {
    activity: &'a TailActivity,
}

impl<'a> ParkGuard<'a> {
    fn new(activity: &'a TailActivity) -> Self {
        activity.sub();
        Self { activity }
    }
}

impl Drop for ParkGuard<'_> {
    fn drop(&mut self) {
        self.activity.resume();
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
    fn an_invocation_counts_the_activity_of_trackers_obtained_for_it() {
        let worker = TailWorkTracker::default();
        let first = TailWorkTracker::default();
        let second = TailWorkTracker::default();

        let of_first = worker.counting_on(&first).activity();
        let child_of_first = of_first.tracker().activity();
        let of_second = worker.counting_on(&second).activity();
        assert_eq!(worker.active_count(), 3);
        assert_eq!(first.active_count(), 2);
        assert_eq!(second.active_count(), 1);

        futures::executor::block_on(of_first.park(async {
            assert_eq!(worker.active_count(), 2);
            assert_eq!(first.active_count(), 1);
            assert_eq!(second.active_count(), 1);
        }));
        drop(of_second);
        assert!(!second.has_active());
        assert_eq!(worker.active_count(), 2);
        drop(child_of_first);
        drop(of_first);
        assert!(!first.has_active());
        assert!(!worker.has_active());
    }

    #[test]
    fn resumed_parked_task_counts_on_the_driving_invocation() {
        let worker = TailWorkTracker::default();
        let spawner = TailWorkTracker::default();
        let driver = TailWorkTracker::default();
        worker.begin_invocation(OplogIndex::from_u64(10), &spawner);

        let activity = worker.counting_on(&spawner).activity();
        assert_eq!(spawner.active_count(), 1);

        let mut parked = Box::pin(activity.park(std::future::pending::<()>()));
        futures::executor::block_on(async {
            assert!(futures::poll!(parked.as_mut()).is_pending());
        });
        assert!(!spawner.has_active());

        worker.end_invocation();
        worker.begin_invocation(OplogIndex::from_u64(21), &driver);
        assert_eq!(worker.current_invocation(), Some(OplogIndex::from_u64(21)));
        drop(parked);

        assert!(!spawner.has_active());
        assert_eq!(driver.active_count(), 1);
        let child = activity.tracker().activity();
        assert_eq!(driver.active_count(), 2);
        drop(child);
        drop(activity);
        assert!(!driver.has_active());
        assert!(!worker.has_active());
    }

    #[test]
    fn park_until_driven_waits_for_an_accepted_invocation() {
        let worker = TailWorkTracker::default();
        let closed = TailWorkTracker::default();
        let driver = TailWorkTracker::default();
        let closed_start = OplogIndex::from_u64(10);
        worker.begin_invocation(closed_start, &closed);

        let activity = worker.counting_on(&closed).activity();
        let mut waiting = Box::pin(activity.park_until_driven(|started| started != closed_start));
        futures::executor::block_on(async {
            assert!(futures::poll!(waiting.as_mut()).is_pending());
        });
        assert!(!worker.has_active());

        worker.end_invocation();
        futures::executor::block_on(async {
            assert!(futures::poll!(waiting.as_mut()).is_pending());
        });
        assert!(!worker.has_active());

        worker.begin_invocation(OplogIndex::from_u64(21), &driver);
        futures::executor::block_on(async {
            assert!(futures::poll!(waiting.as_mut()).is_ready());
        });
        assert!(!closed.has_active());
        assert_eq!(driver.active_count(), 1);
        drop(waiting);
        drop(activity);
        assert!(!worker.has_active());
    }

    /// The activity counts on the accepted invocation from then on, also when that invocation
    /// already runs when the task asks.
    #[test]
    fn park_until_driven_counts_on_an_already_running_driver() {
        let worker = TailWorkTracker::default();
        let spawner = TailWorkTracker::default();
        let driver = TailWorkTracker::default();
        worker.begin_invocation(OplogIndex::from_u64(10), &spawner);
        let activity = worker.counting_on(&spawner).activity();
        worker.end_invocation();
        worker.begin_invocation(OplogIndex::from_u64(21), &driver);

        futures::executor::block_on(
            activity.park_until_driven(|started| started == OplogIndex::from_u64(21)),
        );
        assert_eq!(
            driver.active_count(),
            1,
            "the activity counts on the driving invocation"
        );
        assert!(
            !spawner.has_active(),
            "the activity no longer counts on its spawner"
        );
        drop(activity);
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
            if let Some(initiations) = &self.host_initiations {
                initiations.close();
            }
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

        /// Disarms the deadline unless it fired first. `false` means that it fired: its
        /// interrupt is sent, or is about to be sent.
        pub fn disarm(self) -> bool {
            !self.claim.swap(true, Ordering::AcqRel)
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
            self.schema_producers.revoke();
            let tasks = self.schema_output_tasks.lock().unwrap().take();
            drop(tasks);
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
            let claim = Arc::new(AtomicBool::new(false));
            let timer = duration.map(|duration| {
                let latch = latch.clone();
                let claim = claim.clone();
                let execution_status = self.execution_status.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    if claim.swap(true, Ordering::AcqRel) {
                        return;
                    }
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
                claim,
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
