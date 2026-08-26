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

use crate::services::active_workers::MemoryGrant;
use crate::services::agent_memory_meter::AgentMemoryMeter;
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub(crate) const SHARED_LINEAR_MEMORY_ERROR: &str =
    "Shared linear memories require WebAssembly threads, which are not supported";

#[derive(Clone, Debug)]
pub struct LinearMemoryTracker {
    inner: Arc<Inner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnsharedMemoryGrowth {
    pub admission_delta: u64,
    pub protected_total: u64,
}

#[derive(Debug)]
struct Inner {
    bytes: AtomicU64,
    persisted_startup_bytes: u64,
    initially_reserved_bytes: u64,
    startup_bytes_remaining: AtomicU64,
    pending_growth_prepaid: AtomicU64,
    growth_has_pending_grant: AtomicBool,
    pending_growth_grants: Mutex<Vec<PendingGrowthGrant>>,
    transient_growth_grants: Mutex<Vec<MemoryGrant>>,
    retained_growth_grant: Arc<Mutex<MemoryGrant>>,
    reconciling: AtomicBool,
    replaying: AtomicBool,
    transitions: Mutex<()>,
    resource_entry: Arc<AtomicResourceEntry>,
    meter: AgentMemoryMeter,
}

#[derive(Debug)]
struct PendingGrowthGrant {
    grant: MemoryGrant,
    transient: bool,
}

impl LinearMemoryTracker {
    pub(crate) fn new(
        bytes: u64,
        initially_reserved_bytes: u64,
        mode: AgentMode,
        replaying: bool,
        resource_entry: Arc<AtomicResourceEntry>,
        retained_growth_grant: Arc<Mutex<MemoryGrant>>,
        now: Instant,
    ) -> Self {
        let tracker = Self {
            inner: Arc::new(Inner {
                bytes: AtomicU64::new(bytes),
                persisted_startup_bytes: bytes,
                initially_reserved_bytes,
                startup_bytes_remaining: AtomicU64::new(initially_reserved_bytes),
                pending_growth_prepaid: AtomicU64::new(0),
                growth_has_pending_grant: AtomicBool::new(false),
                pending_growth_grants: Mutex::new(Vec::new()),
                transient_growth_grants: Mutex::new(Vec::new()),
                retained_growth_grant,
                reconciling: AtomicBool::new(true),
                replaying: AtomicBool::new(replaying),
                transitions: Mutex::new(()),
                resource_entry: resource_entry.clone(),
                meter: AgentMemoryMeter::new(mode, bytes, true, resource_entry, now),
            }),
        };
        tracker
            .inner
            .meter
            .set_protected_bytes(initially_reserved_bytes);
        tracker
    }

    pub fn current_bytes(&self) -> u64 {
        self.inner.bytes.load(Ordering::Acquire)
    }

    pub(crate) fn set_limit_exceeded_callback(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        self.inner.meter.set_limit_exceeded_callback(callback);
    }

    pub fn initially_reserved_bytes(&self) -> u64 {
        self.inner.initially_reserved_bytes
    }

    pub fn switch_to_live(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.replaying.store(false, Ordering::Release);
    }

    pub fn reconciliation_grant_bytes(&self, bytes: u64) -> u64 {
        if self.inner.replaying.load(Ordering::Acquire) {
            bytes.max(self.inner.initially_reserved_bytes)
        } else {
            bytes
        }
    }

    pub fn reconcile(&self, bytes: u64, now: Instant) -> u64 {
        let _transition = self.inner.transitions.lock().unwrap();
        let live_growth = if self.inner.replaying.load(Ordering::Acquire) {
            0
        } else {
            bytes.saturating_sub(self.inner.persisted_startup_bytes)
        };
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        self.inner
            .meter
            .set_protected_bytes(self.reconciliation_grant_bytes(bytes));
        self.inner
            .meter
            .enforce_limit(self.inner.resource_entry.max_memory_limit() as u64);
        let startup_bytes_remaining = if self.inner.replaying.load(Ordering::Acquire) {
            self.inner.initially_reserved_bytes.saturating_sub(bytes)
        } else {
            0
        };
        self.inner
            .startup_bytes_remaining
            .store(startup_bytes_remaining, Ordering::Release);
        self.inner
            .pending_growth_prepaid
            .store(0, Ordering::Release);
        let pending_grants = std::mem::take(&mut *self.inner.pending_growth_grants.lock().unwrap());
        let mut retained_grant = self.inner.retained_growth_grant.lock().unwrap();
        for pending in pending_grants {
            if pending.transient {
                self.inner
                    .transient_growth_grants
                    .lock()
                    .unwrap()
                    .push(pending.grant);
            } else {
                retained_grant.merge(pending.grant);
            }
        }
        self.inner
            .growth_has_pending_grant
            .store(false, Ordering::Release);
        self.inner.reconciling.store(false, Ordering::Release);
        live_growth
    }

    pub fn grow(&self, delta: u64, now: Instant) -> (u64, bool) {
        let _transition = self.inner.transitions.lock().unwrap();
        if self
            .inner
            .growth_has_pending_grant
            .swap(false, Ordering::AcqRel)
            && let Some(pending) = self.inner.pending_growth_grants.lock().unwrap().pop()
        {
            if pending.transient {
                self.inner
                    .transient_growth_grants
                    .lock()
                    .unwrap()
                    .push(pending.grant);
            } else {
                self.inner
                    .retained_growth_grant
                    .lock()
                    .unwrap()
                    .merge(pending.grant);
            }
        }
        let reconciling = self.inner.reconciling.load(Ordering::Acquire);
        let prepaid = self
            .inner
            .pending_growth_prepaid
            .swap(0, Ordering::AcqRel)
            .min(delta);
        let tracked_delta = if reconciling { delta - prepaid } else { delta };
        let bytes = self.current_bytes().saturating_add(tracked_delta);
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        self.inner
            .meter
            .set_protected_bytes(self.reconciliation_grant_bytes(bytes));
        self.inner
            .meter
            .enforce_limit(self.inner.resource_entry.max_memory_limit() as u64);
        (bytes, reconciling)
    }

    pub fn memory_grow_failed(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        if self
            .inner
            .growth_has_pending_grant
            .swap(false, Ordering::AcqRel)
        {
            self.inner.pending_growth_grants.lock().unwrap().pop();
        }
        let prepaid = self.inner.pending_growth_prepaid.swap(0, Ordering::AcqRel);
        self.inner
            .startup_bytes_remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                Some(remaining.saturating_add(prepaid))
            })
            .ok();
    }

    pub(crate) fn prepare_unshared_growth(
        &self,
        current_memory: usize,
        desired_memory: usize,
    ) -> Option<UnsharedMemoryGrowth> {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner
            .growth_has_pending_grant
            .store(false, Ordering::Release);
        let delta = desired_memory.saturating_sub(current_memory) as u64;
        let reconciling = self.inner.reconciling.load(Ordering::Acquire);
        let prepaid = if (reconciling && current_memory == 0)
            || self.inner.replaying.load(Ordering::Acquire)
        {
            let remaining = self.inner.startup_bytes_remaining.load(Ordering::Acquire);
            let prepaid = delta.min(remaining);
            self.inner
                .startup_bytes_remaining
                .store(remaining - prepaid, Ordering::Release);
            prepaid
        } else {
            0
        };
        self.inner
            .pending_growth_prepaid
            .store(prepaid, Ordering::Release);
        let admission_delta = delta - prepaid;
        let protected_base = if reconciling {
            self.inner.initially_reserved_bytes
        } else {
            self.current_bytes()
        };
        let protected_delta = if reconciling { admission_delta } else { delta };
        let protected_total = protected_base.checked_add(protected_delta)?;
        Some(UnsharedMemoryGrowth {
            admission_delta,
            protected_total,
        })
    }

    pub(crate) fn retain_growth_grant(&self, grant: MemoryGrant) {
        self.retain_pending_growth_grant(grant, false);
    }

    fn retain_pending_growth_grant(&self, grant: MemoryGrant, transient: bool) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner
            .pending_growth_grants
            .lock()
            .unwrap()
            .push(PendingGrowthGrant { grant, transient });
        self.inner
            .growth_has_pending_grant
            .store(true, Ordering::Release);
    }

    pub fn resume(&self, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.resume(self.current_bytes(), now);
    }

    pub fn pause(&self, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.pause(now);
    }

    pub fn stop(&self, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.stop(now);
    }

    pub fn meter(&self) -> &AgentMemoryMeter {
        &self.inner.meter
    }
}

pub fn desired_total_after_growth(
    current_total: u64,
    current_memory: usize,
    desired_memory: usize,
) -> Option<(u64, u64)> {
    let delta = desired_memory.saturating_sub(current_memory) as u64;
    current_total.checked_add(delta).map(|total| (delta, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::active_workers::admission::{
        AdmissionController, AdmissionPolicy, NoEvictionSource,
    };
    use crate::services::active_workers::memory_probe::FixedProbe;
    use crate::services::resource_limits::AtomicResourceEntry;
    use golem_common::model::agent::AgentMode;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    use test_r::test;

    fn grant() -> Arc<Mutex<MemoryGrant>> {
        Arc::new(Mutex::new(MemoryGrant::inert(0)))
    }

    #[test]
    fn independent_memory_growth_uses_specific_memory_delta() {
        let (delta, total) = desired_total_after_growth(80, 30, 45).unwrap();
        assert_eq!(delta, 15);
        assert_eq!(total, 95);

        let (_, over_limit_total) = desired_total_after_growth(total, 20, 30).unwrap();
        assert_eq!(over_limit_total, 105);
    }

    #[test]
    fn independent_memories_update_one_canonical_aggregate() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            30,
            30,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 42, 0, 0, 0)),
            grant(),
            now,
        );
        tracker.reconcile(30, now);

        assert_eq!(
            tracker.prepare_unshared_growth(10, 20),
            Some(UnsharedMemoryGrowth {
                admission_delta: 10,
                protected_total: 40,
            })
        );
        assert_eq!(tracker.grow(10, now), (40, false));
        assert_eq!(
            tracker.prepare_unshared_growth(20, 25),
            Some(UnsharedMemoryGrowth {
                admission_delta: 5,
                protected_total: 45,
            })
        );
        assert_eq!(tracker.current_bytes(), 40);
    }

    #[test]
    fn growth_total_detects_overflow() {
        assert_eq!(desired_total_after_growth(u64::MAX, 0, 1), None);
    }

    #[test]
    fn tracker_preserves_initial_reservation_across_growth_and_reconciliation() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            grant(),
            now,
        );
        tracker.grow(20, now);
        tracker.reconcile(60, now);

        assert_eq!(tracker.initially_reserved_bytes(), 40);
        assert_eq!(tracker.current_bytes(), 60);
    }

    #[test]
    fn startup_minimums_consume_the_existing_reservation() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            grant(),
            now,
        );

        assert_eq!(
            tracker.prepare_unshared_growth(0, 15),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 40,
            })
        );
        assert_eq!(
            tracker.prepare_unshared_growth(0, 25),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 40,
            })
        );
        assert_eq!(
            tracker.prepare_unshared_growth(25, 35),
            Some(UnsharedMemoryGrowth {
                admission_delta: 10,
                protected_total: 50,
            })
        );
    }

    #[test]
    fn live_initialization_consumes_the_startup_grant_without_double_counting() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            grant(),
            now,
        );
        tracker.switch_to_live();

        assert_eq!(
            tracker.prepare_unshared_growth(0, 40),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 40,
            })
        );
        assert_eq!(tracker.grow(40, now), (40, true));
        assert_eq!(tracker.reconcile(40, now), 0);
    }

    #[test]
    async fn live_reinstantiation_reuses_its_startup_grant_without_reporting_growth() {
        let now = Instant::now();
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe::new(100, 0)),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let retained_grant = controller.admit(40, &NoEvictionSource).await.unwrap();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            Arc::new(Mutex::new(retained_grant)),
            now,
        );
        let headroom_before_instantiation = controller.headroom_bytes();

        let growth = tracker.prepare_unshared_growth(0, 40).unwrap();
        assert_eq!(growth.admission_delta, 0);
        tracker.grow(40, now);

        assert_eq!(tracker.reconcile(40, now), 0);
        assert_eq!(controller.headroom_bytes(), headroom_before_instantiation);
    }

    #[test]
    fn live_instantiation_growth_is_reported_for_persistence() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            grant(),
            now,
        );
        tracker.switch_to_live();

        tracker.prepare_unshared_growth(0, 40).unwrap();
        tracker.grow(40, now);
        tracker.prepare_unshared_growth(40, 50).unwrap();
        tracker.grow(10, now);

        assert_eq!(tracker.reconcile(50, now), 10);
    }

    #[test]
    fn replay_growth_consumes_the_reconstructed_reservation() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            50,
            50,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            grant(),
            now,
        );

        assert_eq!(
            tracker.prepare_unshared_growth(0, 40),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 50,
            })
        );
        assert_eq!(tracker.reconcile(40, now), 0);
        assert_eq!(tracker.reconciliation_grant_bytes(40), 50);
        assert_eq!(
            tracker.prepare_unshared_growth(40, 50),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 50,
            })
        );
        tracker.grow(10, now);
        assert_eq!(tracker.current_bytes(), 50);
    }

    #[test]
    async fn failed_zero_page_growth_releases_its_pending_grant() {
        let now = Instant::now();
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe::new(100, 0)),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let tracker = LinearMemoryTracker::new(
            0,
            0,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            grant(),
            now,
        );
        tracker.reconcile(0, now);
        let pending_grant = controller.admit(10, &NoEvictionSource).await.unwrap();
        tracker.retain_growth_grant(pending_grant);

        assert_eq!(controller.headroom_bytes(), 90);
        tracker.memory_grow_failed();
        assert_eq!(
            controller.headroom_bytes(),
            100,
            "a failed Wasmtime growth must return its pending admission reservation"
        );
    }

    #[test]
    fn committed_growth_rechecks_a_concurrently_lowered_limit() {
        let now = Instant::now();
        let entry = Arc::new(AtomicResourceEntry::new(0, 10, 0, 0, 0));
        let tracker =
            LinearMemoryTracker::new(5, 5, AgentMode::Durable, false, entry, grant(), now);
        tracker.reconcile(5, now);
        let exceeded = Arc::new(AtomicBool::new(false));
        let exceeded_clone = exceeded.clone();
        tracker.set_limit_exceeded_callback(Arc::new(move || {
            exceeded_clone.store(true, Ordering::Release);
        }));

        tracker.grow(10, now);

        assert!(exceeded.load(Ordering::Acquire));
    }
}
