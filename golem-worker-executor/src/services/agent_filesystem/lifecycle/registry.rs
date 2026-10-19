// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::*;

impl FilesystemInvocation {
    pub(super) fn admit(&self) -> Result<InvocationLease, AccessError> {
        let mut state = self.activity.state.lock().unwrap();
        if state.closed {
            return Err(AccessError::Revoked);
        }
        state.active += 1;
        Ok(InvocationLease(self.clone()))
    }

    /// Call after stopping the body and destroying its resource table. Previously admitted calls
    /// retain their leases when abandoned, and can still create and close descriptors.
    pub(crate) async fn close_and_drain(&self) {
        self.activity.state.lock().unwrap().closed = true;
        loop {
            let changed = self.activity.changed.notified();
            if self.activity.state.lock().unwrap().active == 0 {
                return;
            }
            changed.await;
        }
    }
}

impl InvocationLease {
    pub(super) fn retain(&self) -> Self {
        self.0.activity.state.lock().unwrap().active += 1;
        Self(self.0.clone())
    }
}

impl Drop for InvocationLease {
    fn drop(&mut self) {
        self.0.activity.state.lock().unwrap().active -= 1;
        self.0.activity.changed.notify_waiters();
    }
}

/// Returns a weak activity observer for a `Resident` filesystem generation.
///
/// Worker lifecycle code uses it to monitor terminal invalidation and in-flight effects without
/// keeping the filesystem resident. A dropped generation is reported as inactive, not failed.
pub(crate) fn filesystem_activity<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ResidentFilesystem<Adapter>,
) -> ResidentFilesystemActivity {
    ResidentFilesystemActivity {
        registry: Arc::downgrade(
            &filesystem
                .generation
                .as_ref()
                .expect("agent filesystem generation already consumed")
                .registry,
        ),
    }
}

impl ResidentFilesystemActivity {
    /// Reports whether the observed resident generation has been terminally invalidated.
    ///
    /// Worker supervision polls this after residence begins. It returns `false` if the generation
    /// has been dropped, because the observer does not keep the generation alive.
    pub(crate) fn has_terminal_failure(&self) -> bool {
        self.registry
            .upgrade()
            .is_some_and(|registry| registry.is_invalidated())
    }

    /// Waits until the observed resident generation is terminally invalidated.
    ///
    /// Worker supervision uses this as an invalidation signal. The wait does not complete for a
    /// normal seal or drop; if the weak reference is already gone, it remains pending.
    pub(crate) async fn wait_for_terminal_failure(&self) {
        let Some(registry) = self.registry.upgrade() else {
            std::future::pending::<()>().await;
            return;
        };
        registry.wait_for_invalidation().await;
    }

    /// Reports whether filesystem calls are still in flight.
    ///
    /// Unload heuristics use this for a resident generation. Open node handles alone do not count
    /// as active calls, and a dropped generation returns `false`.
    pub(crate) fn has_active_effects(&self) -> bool {
        self.registry.upgrade().is_some_and(|registry| {
            let state = registry.state.lock().unwrap();
            state.calls != 0
        })
    }

    /// Returns the Unix timestamp in milliseconds of the latest completed filesystem call.
    ///
    /// Idle tracking reads this for a resident generation. It returns zero before any call
    /// completes and after the observed generation has been dropped.
    pub(crate) fn last_effect_completion_millis(&self) -> u64 {
        self.registry.upgrade().map_or(0, |registry| {
            registry
                .last_effect_completion_millis
                .load(std::sync::atomic::Ordering::Acquire)
        })
    }
}

impl CallEffect {
    /// What the lease of a call with this effect counts. A call that decides after a check counts
    /// its change itself, so its lease counts nothing.
    pub(super) fn counted(self) -> Option<Counted> {
        match self {
            Self::Read | Self::Decides => None,
            Self::Changes => Some(Counted::Fresh),
            Self::ChoosesTimes => Some(Counted::Chosen),
            Self::Installs => Some(Counted::Installed),
        }
    }
}

impl TreeCounters {
    /// The counters after one more change of the kind `counted`.
    pub(super) fn after(self, counted: Counted) -> Self {
        let (carried, chosen) = match counted {
            Counted::Fresh => (0, 0),
            Counted::Installed | Counted::Replayed => (1, 0),
            Counted::Chosen => (1, 1),
        };
        Self {
            changes: self.changes + 1,
            carried: self.carried + carried,
            chosen: self.chosen + chosen,
            ..self
        }
    }

    /// The counters after a restore of the baseline that holds `times`. A tree with the
    /// modification times of a save holds times that a start without the restore does not give,
    /// so its restore counts as a chosen time. A tree with the times of an install of initial
    /// files holds what a start without the restore gives, so its restore counts as an install.
    pub(super) fn after_restore(self, times: RestoredTimes) -> Self {
        match times {
            RestoredTimes::Saved { kept } => Self {
                restored_times: kept,
                ..self.after(Counted::Chosen)
            },
            RestoredTimes::Installed => self.after(Counted::Installed),
        }
    }

    /// The mark of the tree of the generation `generation` at these counters, for a tree that no
    /// capture copied.
    pub(super) fn mark(self, generation: u64) -> TreeMark {
        TreeMark {
            generation,
            changes: self.changes,
            carried: self.carried,
            saved_times: self.restored_times,
        }
    }

    /// The mark of the tree of the generation `generation` at these counters, for a tree that a
    /// capture copies.
    pub(super) fn captured_mark(self, generation: u64) -> TreeMark {
        TreeMark {
            saved_times: true,
            ..self.mark(generation)
        }
    }

    /// Whether a change put a chosen modification time at a path outside an install.
    pub(super) fn has_chosen_times(self) -> bool {
        self.chosen != 0
    }
}

impl TreeMark {
    /// Whether `self` and `other` are marks of one generation.
    pub(crate) fn same_generation(&self, other: &TreeMark) -> bool {
        self.generation == other.generation
    }
}

/// Gives a mark of a new generation and a later mark of the same generation, for tests of the
/// callers of the lifecycle.
#[cfg(test)]
pub(crate) fn test_tree_marks() -> (TreeMark, TreeMark) {
    let generation = NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mark = |changes| TreeMark {
        generation,
        changes,
        carried: 0,
        saved_times: true,
    };
    (mark(0), mark(1))
}

impl GenerationRegistry {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                admission: AdmissionState::Open,
                replay_access: false,
                calls: 0,
                nodes: 0,
                generation: NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                counters: TreeCounters::default(),
            }),
            changed: tokio::sync::Notify::new(),
            last_effect_completion_millis: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Admits a call, and counts its effect on the tree. A capture reads the counters only when no
    /// call holds a lease, so it sees each change whose effect can be on disk.
    pub(super) fn lease_call(
        self: &Arc<Self>,
        effect: CallEffect,
    ) -> Result<CallLease, AccessError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            AdmissionState::Open => {}
            AdmissionState::Transitioning => return Err(AccessError::Transitioning),
            AdmissionState::Sealed | AdmissionState::Invalidated => {
                return Err(AccessError::Revoked);
            }
        }
        state.calls = state
            .calls
            .checked_add(1)
            .expect("filesystem call count overflowed");
        if let Some(counted) = effect.counted() {
            state.counters = state.counters.after(counted);
        }
        Ok(CallLease {
            registry: Arc::clone(self),
            invocation: None,
        })
    }

    /// Counts a change that a call with [`CallEffect::Decides`] makes. The call records it while
    /// it holds its lease, before its first change.
    pub(super) fn record(&self, counted: Counted) {
        let mut state = self.state.lock().unwrap();
        state.counters = state.counters.after(counted);
    }

    /// Records that the baseline is a restored tree with `times`, as
    /// [`TreeCounters::after_restore`] tells.
    pub(super) fn record_restore(&self, times: RestoredTimes) {
        let mut state = self.state.lock().unwrap();
        state.counters = state.counters.after_restore(times);
    }

    /// The number of the generation and its counters now.
    pub(super) fn counters(&self) -> (u64, TreeCounters) {
        let state = self.state.lock().unwrap();
        (state.generation, state.counters)
    }

    /// The mark of the tree now, for a tree that no capture copied.
    pub(super) fn baseline_mark(&self) -> TreeMark {
        let (generation, counters) = self.counters();
        counters.mark(generation)
    }

    pub(super) fn lease_internal_call(self: &Arc<Self>) -> CallLease {
        let mut state = self.state.lock().unwrap();
        state.calls = state
            .calls
            .checked_add(1)
            .expect("filesystem call count overflowed");
        CallLease {
            registry: Arc::clone(self),
            invocation: None,
        }
    }

    pub(super) fn register_node(self: &Arc<Self>) -> (NodeLease, bool) {
        let mut state = self.state.lock().unwrap();
        state.nodes = state
            .nodes
            .checked_add(1)
            .expect("filesystem node count overflowed");
        (
            NodeLease {
                registry: Arc::clone(self),
            },
            matches!(
                state.admission,
                AdmissionState::Open | AdmissionState::Transitioning
            ),
        )
    }

    pub(super) fn begin_transition(&self) -> Result<(), AccessError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            AdmissionState::Open => {
                state.admission = AdmissionState::Transitioning;
                Ok(())
            }
            AdmissionState::Transitioning => Err(AccessError::Transitioning),
            AdmissionState::Sealed | AdmissionState::Invalidated => Err(AccessError::Revoked),
        }
    }

    pub(super) fn enable_replay_access(&self) {
        let mut state = self.state.lock().unwrap();
        if state.admission == AdmissionState::Open {
            state.replay_access = true;
        }
    }

    pub(super) fn replay_access_allowed(&self) -> bool {
        self.state.lock().unwrap().replay_access
    }

    pub(super) fn begin_replay_drain(&self) -> Result<(), AccessError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            AdmissionState::Open if state.replay_access => {
                state.replay_access = false;
                state.admission = AdmissionState::Transitioning;
                Ok(())
            }
            AdmissionState::Open | AdmissionState::Transitioning => Err(AccessError::Transitioning),
            AdmissionState::Sealed | AdmissionState::Invalidated => Err(AccessError::Revoked),
        }
    }

    pub(super) fn finish_transition(&self) {
        let mut state = self.state.lock().unwrap();
        if state.admission == AdmissionState::Transitioning {
            state.admission = AdmissionState::Open;
        }
        self.changed.notify_waiters();
    }

    pub(super) fn seal(&self) {
        let mut state = self.state.lock().unwrap();
        state.replay_access = false;
        if state.admission != AdmissionState::Invalidated {
            state.admission = AdmissionState::Sealed;
        }
        self.changed.notify_waiters();
    }

    pub(super) fn invalidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.replay_access = false;
        state.admission = AdmissionState::Invalidated;
        drop(state);
        self.changed.notify_waiters();
    }

    pub(super) fn is_invalidated(&self) -> bool {
        self.state.lock().unwrap().admission == AdmissionState::Invalidated
    }

    pub(super) async fn wait_for_invalidation(&self) {
        loop {
            let notified = self.changed.notified();
            if self.is_invalidated() {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn wait_for_calls(&self) {
        loop {
            let notified = self.changed.notified();
            if self.state.lock().unwrap().calls == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn wait_for_drain(&self) {
        loop {
            let notified = self.changed.notified();
            let drained = {
                let state = self.state.lock().unwrap();
                state.calls == 0 && state.nodes == 0
            };
            if drained {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for CallLease {
    fn drop(&mut self) {
        let mut state = self.registry.state.lock().unwrap();
        debug_assert!(state.calls != 0);
        state.calls -= 1;
        drop(state);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        self.registry
            .last_effect_completion_millis
            .store(now, std::sync::atomic::Ordering::Release);
        self.registry.changed.notify_waiters();
    }
}

impl Drop for NodeLease {
    fn drop(&mut self) {
        let mut state = self.registry.state.lock().unwrap();
        debug_assert!(state.nodes != 0);
        state.nodes -= 1;
        drop(state);
        self.registry.changed.notify_waiters();
    }
}
