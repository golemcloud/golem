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
//! produce durable work until a future guest action. Safe park points are
//! exclusively waits on guest-driven events:
//!
//! * awaiting guest demand (e.g. a consume-body / TCP-receive demand
//!   channel, or the demand-gating oneshot of the request-body transmission
//!   recorder);
//! * awaiting guest-produced stream data (stdio capture, filesystem write
//!   chunks, TCP send bytes, replayed request-body frames).
//!
//! Waits whose completion is *not* guest-driven — durable oplog appends,
//! live network / file I/O, replay-resolver waits — keep the task active:
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
