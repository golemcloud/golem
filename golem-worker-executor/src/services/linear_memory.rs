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

use crate::services::active_agents::MemoryGrant;
use crate::services::agent_memory_meter::AgentMemoryMeter;
use crate::services::byte_time_accumulator::MeteringTime;
use crate::services::resource_limits::{AgentMemoryLimitTarget, AtomicResourceEntry};
use crate::services::resource_usage_metering::ResourceUsageLiveActivation;
use golem_common::model::agent::AgentMode;
use std::future::Future;
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

#[derive(Debug, Clone, Copy)]
pub(crate) enum MemoryGrowthFailure {
    ExceededMemoryLimit,
    OutOfMemory,
}

impl std::fmt::Display for MemoryGrowthFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExceededMemoryLimit => f.write_str("Exceeded plan memory limit"),
            Self::OutOfMemory => f.write_str("Worker out of memory"),
        }
    }
}

impl std::error::Error for MemoryGrowthFailure {}

struct Inner {
    bytes: AtomicU64,
    protected_bytes: AtomicU64,
    persisted_startup_bytes: u64,
    initially_reserved_bytes: u64,
    state: Mutex<TrackerState>,
    retained_growth_grant: Arc<Mutex<MemoryGrant>>,
    usage_live: AtomicBool,
    guest_bodies: AtomicU64,
    live_activation: Mutex<Option<ResourceUsageLiveActivation>>,
    transitions: Arc<Mutex<()>>,
    live_reservation_activation: tokio::sync::Mutex<()>,
    resource_entry: Arc<AtomicResourceEntry>,
    limit_exceeded: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    meter: Option<AgentMemoryMeter>,
}

// Mutations lock transitions, then state, then the retained grant. Readers may lock state
// without transitions. Callbacks run with transitions held but without state or grant guards.
struct TrackerState {
    reconciling: bool,
    replaying: bool,
    accounting_started: bool,
    historical_admission: bool,
    live_reservation_activated: bool,
    pending_initial_bytes: u64,
    pending_growth_protected_total: u64,
    growth_has_pending_grant: bool,
    pending_growth_grants: Vec<PendingGrowthGrant>,
    transient_growth_grants: Vec<MemoryGrant>,
    growth_failure: Option<MemoryGrowthFailure>,
}

impl TrackerState {
    fn reconciliation_grant_bytes(&self, bytes: u64, initially_reserved_bytes: u64) -> u64 {
        if self.replaying {
            bytes.max(initially_reserved_bytes)
        } else {
            bytes
        }
    }

    fn required_live_reservation_bytes(&self, bytes: u64, initially_reserved_bytes: u64) -> u64 {
        self.reconciliation_grant_bytes(bytes, initially_reserved_bytes)
            .max(self.pending_growth_protected_total)
    }

    fn reserved_bytes(&self, retained: &Mutex<MemoryGrant>) -> u64 {
        retained.lock().unwrap().bytes()
            + self
                .pending_growth_grants
                .iter()
                .map(|pending| pending.grant.bytes())
                .sum::<u64>()
    }
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Inner")
            .field("bytes", &self.bytes)
            .field("protected_bytes", &self.protected_bytes)
            .field("metering_enabled", &self.meter.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct PendingGrowthGrant {
    grant: MemoryGrant,
    transient: bool,
}

impl LinearMemoryTracker {
    #[cfg(test)]
    pub(crate) fn new(
        bytes: u64,
        initially_reserved_bytes: u64,
        mode: AgentMode,
        replaying: bool,
        resource_entry: Arc<AtomicResourceEntry>,
        retained_growth_grant: Arc<Mutex<MemoryGrant>>,
        now: Instant,
    ) -> Self {
        Self::new_at(
            bytes,
            initially_reserved_bytes,
            mode,
            replaying,
            resource_entry,
            retained_growth_grant,
            MeteringTime::now().at_instant(now),
        )
    }

    #[cfg(test)]
    pub(crate) fn new_at(
        bytes: u64,
        initially_reserved_bytes: u64,
        mode: AgentMode,
        replaying: bool,
        resource_entry: Arc<AtomicResourceEntry>,
        retained_growth_grant: Arc<Mutex<MemoryGrant>>,
        now: MeteringTime,
    ) -> Self {
        Self::new_inner(
            bytes,
            initially_reserved_bytes,
            mode,
            replaying,
            resource_entry,
            retained_growth_grant,
            Some(now),
        )
    }

    pub(crate) fn new_with_metering(
        bytes: u64,
        initially_reserved_bytes: u64,
        mode: AgentMode,
        replaying: bool,
        resource_entry: Arc<AtomicResourceEntry>,
        retained_growth_grant: Arc<Mutex<MemoryGrant>>,
        metering_enabled: bool,
    ) -> Self {
        Self::new_inner(
            bytes,
            initially_reserved_bytes,
            mode,
            replaying,
            resource_entry,
            retained_growth_grant,
            metering_enabled.then(MeteringTime::now),
        )
    }

    fn new_inner(
        bytes: u64,
        initially_reserved_bytes: u64,
        mode: AgentMode,
        replaying: bool,
        resource_entry: Arc<AtomicResourceEntry>,
        retained_growth_grant: Arc<Mutex<MemoryGrant>>,
        meter_started_at: Option<MeteringTime>,
    ) -> Self {
        let inner = Arc::new(Inner {
            bytes: AtomicU64::new(bytes),
            protected_bytes: AtomicU64::new(initially_reserved_bytes),
            persisted_startup_bytes: bytes,
            initially_reserved_bytes,
            state: Mutex::new(TrackerState {
                reconciling: true,
                replaying,
                accounting_started: false,
                historical_admission: replaying,
                live_reservation_activated: false,
                pending_initial_bytes: 0,
                pending_growth_protected_total: 0,
                growth_has_pending_grant: false,
                pending_growth_grants: Vec::new(),
                transient_growth_grants: Vec::new(),
                growth_failure: None,
            }),
            retained_growth_grant,
            usage_live: AtomicBool::new(!replaying),
            guest_bodies: AtomicU64::new(0),
            live_activation: Mutex::new(None),
            transitions: Arc::new(Mutex::new(())),
            live_reservation_activation: tokio::sync::Mutex::new(()),
            resource_entry: resource_entry.clone(),
            limit_exceeded: Mutex::new(None),
            meter: meter_started_at.map(|now| {
                let meter = AgentMemoryMeter::new_at(mode, bytes, false, resource_entry, now);
                meter.set_billable_at(!replaying, now);
                meter
            }),
        });
        let limit_target: Arc<dyn AgentMemoryLimitTarget> = inner.clone();
        inner
            .resource_entry
            .register_agent_memory_limit_target(Arc::downgrade(&limit_target));
        Self { inner }
    }

    pub fn current_bytes(&self) -> u64 {
        self.inner.bytes.load(Ordering::Acquire)
    }

    pub(crate) fn begin_instantiation(&self, historical_admission: bool) {
        let _transition = self.inner.transitions.lock().unwrap();
        let mut state = self.inner.state.lock().unwrap();
        state.growth_failure = None;
        self.inner.bytes.store(0, Ordering::Release);
        state.historical_admission = historical_admission;
        drop(state);
        if let Some(meter) = &self.inner.meter {
            meter.set_bytes(0, Instant::now());
        }
    }

    /// Enqueues the absolute baseline and enables subsequent delta recording in one transition.
    /// The caller owns the enqueue result and any storage-completion barrier it represents.
    pub(crate) fn start_memory_accounting<R>(
        &self,
        reset: bool,
        enqueue: impl FnOnce(u64) -> R,
    ) -> Option<R> {
        let _transition = self.inner.transitions.lock().unwrap();
        if !reset && self.inner.state.lock().unwrap().accounting_started {
            return None;
        }
        let bytes = self.current_bytes();
        let result = if reset || bytes != self.inner.persisted_startup_bytes {
            Some(enqueue(bytes))
        } else {
            None
        };
        self.inner.state.lock().unwrap().accounting_started = true;
        result
    }

    /// The initially-live primary has already persisted its instantiation growth.
    pub(crate) fn start_live_memory_accounting(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.state.lock().unwrap().accounting_started = true;
    }

    pub(crate) fn usage_is_live(&self) -> bool {
        self.inner.usage_live.load(Ordering::Acquire)
    }

    /// Returns the current byte rate observed by resource-usage metering.
    pub fn billable_bytes(&self) -> u64 {
        self.inner
            .meter
            .as_ref()
            .map_or(0, AgentMemoryMeter::billable_bytes)
    }

    pub(crate) fn subscribe_billable_bytes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.inner
            .meter
            .as_ref()
            .map(AgentMemoryMeter::subscribe_billable_bytes)
    }

    pub(crate) fn set_live_activation(&self, activation: ResourceUsageLiveActivation) {
        *self.inner.live_activation.lock().unwrap() = Some(activation);
    }

    pub(crate) fn begin_reconstruction_usage(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.usage_live.store(false, Ordering::Release);
        if let Some(meter) = &self.inner.meter {
            meter.set_billable_at(false, MeteringTime::now());
        }
    }

    pub(crate) fn begin_guest_body(&self) -> GuestResourceUsageGuard {
        self.inner.guest_bodies.fetch_add(1, Ordering::AcqRel);
        GuestResourceUsageGuard(self.clone())
    }

    pub(crate) fn has_guest_body(&self) -> bool {
        self.inner.guest_bodies.load(Ordering::Acquire) != 0
    }

    pub(crate) fn activate_live_usage_if_guest_body(&self) {
        if self.has_guest_body() {
            self.activate_live_usage();
        }
    }

    pub(crate) fn activate_live_usage(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        let activation = self.inner.live_activation.lock().unwrap().clone();
        match activation {
            Some(activation) => activation.activate(
                self.inner.meter.as_ref(),
                self.current_bytes(),
                &self.inner.transitions,
            ),
            None => {
                if let Some(meter) = &self.inner.meter {
                    let at = MeteringTime::now();
                    meter.set_billable_at(true, at);
                    meter.resume_at(self.current_bytes(), at);
                }
            }
        }
        self.inner.usage_live.store(true, Ordering::Release);
    }

    pub(crate) fn set_limit_exceeded_callback(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.inner.limit_exceeded.lock().unwrap() = Some(callback.clone());
        if self.exceeds_current_limit() {
            callback();
        }
    }

    pub(crate) fn exceeds_current_limit(&self) -> bool {
        self.inner.protected_bytes.load(Ordering::Acquire)
            > self.inner.resource_entry.max_memory_limit() as u64
    }

    pub(crate) fn clear_limit_exceeded_callback(&self) {
        *self.inner.limit_exceeded.lock().unwrap() = None;
    }

    pub fn initially_reserved_bytes(&self) -> u64 {
        self.inner.initially_reserved_bytes
    }

    pub fn switch_to_live(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        let mut state = self.inner.state.lock().unwrap();
        if !state.reconciling {
            let protected = self
                .current_bytes()
                .max(state.pending_growth_protected_total);
            self.inner
                .retained_growth_grant
                .lock()
                .unwrap()
                .shrink_to(protected);
            drop(state);
            self.set_protected_bytes_and_enforce_limit(protected);
            state = self.inner.state.lock().unwrap();
        }
        state.historical_admission = false;
        state.replaying = false;
    }

    pub fn reconciliation_grant_bytes(&self, bytes: u64) -> u64 {
        self.inner
            .state
            .lock()
            .unwrap()
            .reconciliation_grant_bytes(bytes, self.inner.initially_reserved_bytes)
    }

    pub(crate) fn retained_growth_grant(&self) -> Arc<Mutex<MemoryGrant>> {
        self.inner.retained_growth_grant.clone()
    }

    /// Returns held retained and pending admission grants for this Store.
    pub fn reserved_bytes(&self) -> u64 {
        self.inner
            .state
            .lock()
            .unwrap()
            .reserved_bytes(&self.inner.retained_growth_grant)
    }

    pub(crate) async fn activate_live_reservation<F, Fut>(&self, mut acquire: F)
    where
        F: FnMut(u64) -> Fut,
        Fut: Future<Output = MemoryGrant>,
    {
        let _activation = self.inner.live_reservation_activation.lock().await;
        if self.inner.state.lock().unwrap().live_reservation_activated {
            return;
        }

        let mut grant = MemoryGrant::inert(0);
        loop {
            let required = {
                let _transition = self.inner.transitions.lock().unwrap();
                self.required_live_reservation_bytes()
            };
            if required > grant.bytes() {
                grant.merge(acquire(required - grant.bytes()).await);
                continue;
            }

            let _transition = self.inner.transitions.lock().unwrap();
            let mut state = self.inner.state.lock().unwrap();
            let required = state.required_live_reservation_bytes(
                self.current_bytes(),
                self.inner.initially_reserved_bytes,
            );
            if required > grant.bytes() {
                continue;
            }
            *self.inner.retained_growth_grant.lock().unwrap() = grant;
            state.historical_admission = false;
            state.live_reservation_activated = true;
            return;
        }
    }

    fn required_live_reservation_bytes(&self) -> u64 {
        self.inner
            .state
            .lock()
            .unwrap()
            .required_live_reservation_bytes(
                self.current_bytes(),
                self.inner.initially_reserved_bytes,
            )
    }

    pub fn reconcile(&self, bytes: u64, now: Instant) -> u64 {
        self.reconcile_inner(bytes, Some(now))
    }

    #[cfg(test)]
    pub(crate) fn reconcile_at(&self, bytes: u64, now: Instant) -> u64 {
        self.reconcile_inner(bytes, Some(now))
    }

    fn reconcile_inner(&self, bytes: u64, now: Option<Instant>) -> u64 {
        let _transition = self.inner.transitions.lock().unwrap();
        let state = self.inner.state.lock().unwrap();
        let live_growth = if state.replaying {
            0
        } else {
            bytes.saturating_sub(self.inner.persisted_startup_bytes)
        };
        let protected =
            state.reconciliation_grant_bytes(bytes, self.inner.initially_reserved_bytes);
        drop(state);
        if let (Some(meter), Some(now)) = (&self.inner.meter, now) {
            meter.set_bytes(bytes, now);
        }
        self.inner.bytes.store(bytes, Ordering::Release);
        self.set_protected_bytes_and_enforce_limit(protected);
        let mut state = self.inner.state.lock().unwrap();
        state.pending_initial_bytes = 0;
        state.pending_growth_protected_total = 0;
        let pending_grants = std::mem::take(&mut state.pending_growth_grants);
        let mut retained_grant = self.inner.retained_growth_grant.lock().unwrap();
        for pending in pending_grants {
            if pending.transient {
                state.transient_growth_grants.push(pending.grant);
            } else {
                retained_grant.merge(pending.grant);
            }
        }
        state.growth_has_pending_grant = false;
        state.reconciling = false;
        live_growth
    }

    pub fn grow(&self, delta: u64, now: Instant) -> (u64, bool) {
        self.grow_inner(delta, Some(now), None)
    }

    pub(crate) fn grow_and_record(&self, delta: u64, now: Instant, record: impl Fn(u64)) {
        self.grow_inner(delta, Some(now), Some(&record));
    }

    #[cfg(test)]
    fn grow_at(&self, delta: u64, now: Instant) -> (u64, bool) {
        self.grow_inner(delta, Some(now), None)
    }

    fn grow_inner(
        &self,
        delta: u64,
        now: Option<Instant>,
        record: Option<&dyn Fn(u64)>,
    ) -> (u64, bool) {
        let _transition = self.inner.transitions.lock().unwrap();
        let mut state = self.inner.state.lock().unwrap();
        state.growth_failure = None;
        state.pending_growth_protected_total = 0;
        if std::mem::take(&mut state.growth_has_pending_grant)
            && let Some(pending) = state.pending_growth_grants.pop()
        {
            if pending.transient {
                state.transient_growth_grants.push(pending.grant);
            } else {
                self.inner
                    .retained_growth_grant
                    .lock()
                    .unwrap()
                    .merge(pending.grant);
            }
        }
        let reconciling = state.reconciling;
        let initial = std::mem::take(&mut state.pending_initial_bytes);
        let bytes = self
            .current_bytes()
            .saturating_sub(initial)
            .saturating_add(delta);
        let protected =
            state.reconciliation_grant_bytes(bytes, self.inner.initially_reserved_bytes);
        let accounting_started = state.accounting_started;
        drop(state);
        if let (Some(meter), Some(now)) = (&self.inner.meter, now) {
            meter.set_bytes(bytes, now);
        }
        self.inner.bytes.store(bytes, Ordering::Release);
        self.set_protected_bytes_and_enforce_limit(protected);
        if !reconciling
            && accounting_started
            && let Some(record) = record
        {
            record(delta);
        }
        (bytes, reconciling)
    }

    pub(crate) fn clear_memory_growth_failure(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.state.lock().unwrap().growth_failure = None;
    }

    /// Keeps the refusal as typed context without discarding the GC error or call-owned markers.
    pub(crate) fn annotate_gc_heap_error(&self, error: wasmtime::Error) -> wasmtime::Error {
        if !error.root_cause().is::<wasmtime::GcHeapOutOfMemory<()>>() {
            return error;
        }
        let _transition = self.inner.transitions.lock().unwrap();
        match self.inner.state.lock().unwrap().growth_failure.take() {
            Some(reason) => error.context(reason),
            None => error,
        }
    }

    pub fn memory_grow_failed(&self) {
        self.fail_memory_growth(None);
    }

    pub(crate) fn memory_growth_refused(&self, reason: MemoryGrowthFailure) {
        self.fail_memory_growth(Some(reason));
    }

    fn fail_memory_growth(&self, reason: Option<MemoryGrowthFailure>) {
        let _transition = self.inner.transitions.lock().unwrap();
        let mut state = self.inner.state.lock().unwrap();
        state.growth_failure = reason;
        let initial = std::mem::take(&mut state.pending_initial_bytes);
        self.inner.bytes.fetch_sub(initial, Ordering::AcqRel);
        state.pending_growth_protected_total = 0;
        if std::mem::take(&mut state.growth_has_pending_grant) {
            state.pending_growth_grants.pop();
        }
        if state.live_reservation_activated || (!state.replaying && !state.reconciling) {
            let protected = state.required_live_reservation_bytes(
                self.current_bytes(),
                self.inner.initially_reserved_bytes,
            );
            self.inner
                .retained_growth_grant
                .lock()
                .unwrap()
                .shrink_to(protected);
            drop(state);
            self.set_protected_bytes_and_enforce_limit(protected);
        }
    }

    pub(crate) fn prepare_unshared_growth(
        &self,
        current_memory: usize,
        desired_memory: usize,
    ) -> Option<UnsharedMemoryGrowth> {
        let _transition = self.inner.transitions.lock().unwrap();
        let mut state = self.inner.state.lock().unwrap();
        state.growth_failure = None;
        state.pending_initial_bytes = 0;
        state.growth_has_pending_grant = false;
        let delta = desired_memory.saturating_sub(current_memory) as u64;
        let historical_admission = state.historical_admission;
        let protected_total = self.current_bytes().checked_add(delta)?;
        let reserved_bytes = state.reserved_bytes(&self.inner.retained_growth_grant);
        let admission_delta = if historical_admission {
            0
        } else {
            protected_total.saturating_sub(reserved_bytes)
        };
        // Wasmtime calls memory_growing for initial allocations but not memory_grown.
        // Include them provisionally until enumeration. Growth from a zero-sized memory
        // does receive memory_grown, which replaces this provisional contribution.
        if state.reconciling && current_memory == 0 {
            self.inner.bytes.store(protected_total, Ordering::Release);
            state.pending_initial_bytes = delta;
        }
        state.pending_growth_protected_total = protected_total;
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
        let mut state = self.inner.state.lock().unwrap();
        state
            .pending_growth_grants
            .push(PendingGrowthGrant { grant, transient });
        state.growth_has_pending_grant = true;
    }

    pub(crate) fn resource_transition(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.inner.transitions)
    }

    pub fn meter(&self) -> &AgentMemoryMeter {
        self.inner
            .meter
            .as_ref()
            .expect("linear-memory usage metering is disabled")
    }

    pub(crate) fn meter_if_enabled(&self) -> Option<&AgentMemoryMeter> {
        self.inner.meter.as_ref()
    }

    fn set_protected_bytes_and_enforce_limit(&self, bytes: u64) {
        self.inner.protected_bytes.store(bytes, Ordering::Release);
        if bytes > self.inner.resource_entry.max_memory_limit() as u64
            && let Some(callback) = self.inner.limit_exceeded.lock().unwrap().as_ref()
        {
            callback();
        }
    }
}

#[must_use]
pub(crate) struct GuestResourceUsageGuard(LinearMemoryTracker);

impl Drop for GuestResourceUsageGuard {
    fn drop(&mut self) {
        let previous = self.0.inner.guest_bodies.fetch_sub(1, Ordering::AcqRel);
        assert!(previous != 0, "guest usage scope was released twice");
    }
}

impl AgentMemoryLimitTarget for Inner {
    fn enforce_limit(&self, limit: u64) {
        if self.protected_bytes.load(Ordering::Acquire) > limit
            && let Some(callback) = self.limit_exceeded.lock().unwrap().as_ref()
        {
            callback();
        }
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
    use crate::services::active_agents::admission::{
        AdmissionController, AdmissionPolicy, NoEvictionSource,
    };
    use crate::services::active_agents::memory_probe::FixedProbe;
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
            Arc::new(Mutex::new(MemoryGrant::inert(30))),
            now,
        );
        tracker.reconcile_at(30, now);

        assert_eq!(
            tracker.prepare_unshared_growth(10, 20),
            Some(UnsharedMemoryGrowth {
                admission_delta: 10,
                protected_total: 40,
            })
        );
        tracker.retain_growth_grant(MemoryGrant::inert(10));
        assert_eq!(tracker.grow_at(10, now), (40, false));
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
        tracker.grow_at(20, now);
        tracker.reconcile_at(60, now);

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
            Arc::new(Mutex::new(MemoryGrant::inert(40))),
            now,
        );
        tracker.begin_instantiation(false);

        assert_eq!(
            tracker.prepare_unshared_growth(0, 15),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 15,
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
        *tracker.retained_growth_grant().lock().unwrap() = MemoryGrant::inert(40);
        tracker.begin_instantiation(false);
        tracker.switch_to_live();

        assert_eq!(
            tracker.prepare_unshared_growth(0, 40),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 40,
            })
        );
        assert_eq!(tracker.grow_at(40, now), (40, true));
        assert_eq!(tracker.reconcile_at(40, now), 0);
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
        tracker.begin_instantiation(false);
        let headroom_before_instantiation = controller.headroom_bytes();

        let growth = tracker.prepare_unshared_growth(0, 40).unwrap();
        assert_eq!(growth.admission_delta, 0);
        tracker.grow_at(40, now);

        assert_eq!(tracker.reconcile_at(40, now), 0);
        assert_eq!(controller.headroom_bytes(), headroom_before_instantiation);
    }

    #[test]
    async fn historical_startup_grant_is_activated_once_before_live_repair() {
        let now = Instant::now();
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe::new(100, 0)),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let retained_grant = grant();
        *retained_grant.lock().unwrap() = MemoryGrant::inert(40);
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            retained_grant.clone(),
            now,
        );

        let acquire_controller = controller.clone();
        tracker
            .activate_live_reservation(move |bytes| {
                let acquire_controller = acquire_controller.clone();
                async move {
                    acquire_controller
                        .admit(bytes, &NoEvictionSource)
                        .await
                        .unwrap()
                }
            })
            .await;
        assert!(retained_grant.lock().unwrap().is_tracked());
        assert_eq!(retained_grant.lock().unwrap().bytes(), 40);
        assert_eq!(controller.headroom_bytes(), 60);

        tracker
            .activate_live_reservation(|_| async {
                panic!("an activated startup grant must not be acquired twice")
            })
            .await;
        assert_eq!(controller.headroom_bytes(), 60);
    }

    #[test]
    async fn cancelled_reservation_releases_partial_grant_and_can_retry() {
        let now = Instant::now();
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe::new(100, 0)),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 100, 0, 0, 0)),
            Arc::new(Mutex::new(MemoryGrant::inert(40))),
            now,
        );
        let mut activation = Box::pin(tracker.activate_live_reservation(|bytes| {
            let controller = controller.clone();
            let tracker = tracker.clone();
            async move {
                if bytes == 40 {
                    let grant = controller.admit(bytes, &NoEvictionSource).await.unwrap();
                    tracker.grow(10, now);
                    grant
                } else {
                    assert_eq!(bytes, 10);
                    std::future::pending().await
                }
            }
        }));
        assert!(futures::poll!(activation.as_mut()).is_pending());
        assert_eq!(controller.headroom_bytes(), 60);
        assert_eq!(tracker.current_bytes(), 50);
        drop(activation);
        assert_eq!(controller.headroom_bytes(), 100);
        assert_eq!(tracker.reserved_bytes(), 40);

        tracker
            .activate_live_reservation(|bytes| {
                let controller = controller.clone();
                async move {
                    assert_eq!(bytes, 50);
                    controller.admit(bytes, &NoEvictionSource).await.unwrap()
                }
            })
            .await;
        assert_eq!(tracker.reserved_bytes(), 50);
        assert_eq!(controller.headroom_bytes(), 50);
        drop(tracker);
        assert_eq!(controller.headroom_bytes(), 100);
    }

    #[test]
    fn accounting_and_limit_callbacks_can_read_reservation_state() {
        // A separate thread lets the deadline detect a synchronous mutex deadlock.
        let (done, completed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let now = Instant::now();
            let tracker = LinearMemoryTracker::new(
                5,
                10,
                AgentMode::Durable,
                true,
                Arc::new(AtomicResourceEntry::new(0, 10, 0, 0, 0)),
                Arc::new(Mutex::new(MemoryGrant::inert(10))),
                now,
            );
            let observations = Arc::new(Mutex::new(Vec::new()));
            let observed = observations.clone();
            let reader = tracker.clone();
            tracker.set_limit_exceeded_callback(Arc::new(move || {
                observed.lock().unwrap().push((
                    reader.current_bytes(),
                    reader.reconciliation_grant_bytes(reader.current_bytes()),
                    reader.reserved_bytes(),
                ));
            }));
            tracker.reconcile(12, now);
            assert_eq!(
                tracker.start_memory_accounting(false, |bytes| {
                    assert_eq!(tracker.reserved_bytes(), 10);
                    assert_eq!(tracker.reconciliation_grant_bytes(bytes), 12);
                    bytes
                }),
                Some(12)
            );
            tracker.grow_and_record(3, now, |delta| {
                assert_eq!(delta, 3);
                assert_eq!(tracker.reserved_bytes(), 10);
                assert_eq!(
                    tracker.reconciliation_grant_bytes(tracker.current_bytes()),
                    15
                );
            });
            tracker.switch_to_live();
            tracker.switch_to_live();
            tracker.memory_grow_failed();
            tracker.clear_limit_exceeded_callback();
            assert_eq!(
                *observations.lock().unwrap(),
                vec![
                    (12, 12, 10),
                    (15, 15, 10),
                    (15, 15, 10),
                    (15, 15, 10),
                    (15, 15, 10),
                ]
            );
            done.send(()).unwrap();
        });
        completed
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("tracker callbacks must not deadlock reservation readers");
        worker.join().unwrap();
    }

    #[test]
    async fn live_activation_includes_historical_growth_committed_while_admission_waits() {
        let now = Instant::now();
        let retained_grant = grant();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            retained_grant.clone(),
            now,
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let first_request = Arc::new(Mutex::new(Some(started)));
        let release = Arc::new(tokio::sync::Notify::new());
        let activation = {
            let tracker = tracker.clone();
            let requests = requests.clone();
            let first_request = first_request.clone();
            let release = release.clone();
            tokio::spawn(async move {
                tracker
                    .activate_live_reservation(move |bytes| {
                        let requests = requests.clone();
                        let first_request = first_request.clone();
                        let release = release.clone();
                        async move {
                            requests.lock().unwrap().push(bytes);
                            let started = first_request.lock().unwrap().take();
                            if let Some(started) = started {
                                let _ = started.send(());
                                release.notified().await;
                            }
                            MemoryGrant::inert(bytes)
                        }
                    })
                    .await;
            })
        };

        started_rx.await.unwrap();
        assert_eq!(
            tracker.prepare_unshared_growth(40, 50),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 50,
            })
        );
        assert_eq!(tracker.grow(10, now), (50, true));
        release.notify_one();
        activation.await.unwrap();

        assert_eq!(*requests.lock().unwrap(), vec![40, 10]);
        assert_eq!(retained_grant.lock().unwrap().bytes(), 50);
    }

    #[test]
    async fn live_activation_includes_prepared_historical_growth_and_shrinks_if_it_fails() {
        let now = Instant::now();
        let retained_grant = grant();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            retained_grant.clone(),
            now,
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let first_request = Arc::new(Mutex::new(Some(started)));
        let release = Arc::new(tokio::sync::Notify::new());
        let activation = {
            let tracker = tracker.clone();
            let requests = requests.clone();
            let first_request = first_request.clone();
            let release = release.clone();
            tokio::spawn(async move {
                tracker
                    .activate_live_reservation(move |bytes| {
                        let requests = requests.clone();
                        let first_request = first_request.clone();
                        let release = release.clone();
                        async move {
                            requests.lock().unwrap().push(bytes);
                            let started = first_request.lock().unwrap().take();
                            if let Some(started) = started {
                                let _ = started.send(());
                                release.notified().await;
                            }
                            MemoryGrant::inert(bytes)
                        }
                    })
                    .await;
            })
        };

        started_rx.await.unwrap();
        assert_eq!(
            tracker.prepare_unshared_growth(40, 50),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 50,
            })
        );
        release.notify_one();
        activation.await.unwrap();

        assert_eq!(*requests.lock().unwrap(), vec![40, 10]);
        assert_eq!(retained_grant.lock().unwrap().bytes(), 50);
        tracker.memory_grow_failed();
        assert_eq!(retained_grant.lock().unwrap().bytes(), 40);
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
        tracker.begin_instantiation(false);
        tracker.switch_to_live();

        tracker.prepare_unshared_growth(0, 40).unwrap();
        tracker.grow_at(40, now);
        tracker.prepare_unshared_growth(40, 50).unwrap();
        tracker.grow_at(10, now);

        assert_eq!(tracker.reconcile_at(50, now), 10);
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
        tracker.begin_instantiation(true);

        assert_eq!(
            tracker.prepare_unshared_growth(0, 40),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 40,
            })
        );
        assert_eq!(tracker.reconcile_at(40, now), 0);
        assert_eq!(tracker.reconciliation_grant_bytes(40), 50);
        assert_eq!(
            tracker.prepare_unshared_growth(40, 70),
            Some(UnsharedMemoryGrowth {
                admission_delta: 0,
                protected_total: 70,
            })
        );
        tracker.grow_at(30, now);
        assert_eq!(tracker.current_bytes(), 70);
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
        tracker.reconcile_at(0, now);
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
    async fn reconciled_transient_tracker_accounts_repeated_growth_and_releases_grants() {
        let now = Instant::now();
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe::new(100, 0)),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let initial_grant = controller.admit(40, &NoEvictionSource).await.unwrap();
        let tracker = LinearMemoryTracker::new(
            40,
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 100, 0, 0, 0)),
            Arc::new(Mutex::new(initial_grant)),
            now,
        );
        tracker.reconcile(40, now);

        for (current, desired) in [(40, 50), (50, 60)] {
            let growth = tracker.prepare_unshared_growth(current, desired).unwrap();
            assert_eq!(growth.protected_total, desired as u64);
            let grant = controller
                .admit(growth.admission_delta, &NoEvictionSource)
                .await
                .unwrap();
            tracker.retain_growth_grant(grant);
            assert!(!tracker.grow((desired - current) as u64, now).1);
        }

        assert_eq!(tracker.current_bytes(), 60);
        assert_eq!(controller.headroom_bytes(), 40);
        drop(tracker);
        assert_eq!(controller.headroom_bytes(), 100);
    }

    #[test]
    fn committed_growth_rechecks_a_concurrently_lowered_limit() {
        let now = Instant::now();
        let entry = Arc::new(AtomicResourceEntry::new(0, 10, 0, 0, 0));
        let tracker =
            LinearMemoryTracker::new(5, 5, AgentMode::Durable, false, entry, grant(), now);
        tracker.reconcile_at(5, now);
        let exceeded = Arc::new(AtomicBool::new(false));
        let exceeded_clone = exceeded.clone();
        tracker.set_limit_exceeded_callback(Arc::new(move || {
            exceeded_clone.store(true, Ordering::Release);
        }));

        tracker.grow_at(10, now);

        assert!(exceeded.load(Ordering::Acquire));
    }

    #[test]
    fn disabled_memory_metering_keeps_limit_enforcement_without_usage() {
        let now = Instant::now();
        let entry = Arc::new(AtomicResourceEntry::new(0, 10, 0, 0, 0));
        let tracker = LinearMemoryTracker::new_with_metering(
            5,
            5,
            AgentMode::Durable,
            false,
            entry.clone(),
            grant(),
            false,
        );
        tracker.reconcile(5, now);
        let exceeded = Arc::new(AtomicBool::new(false));
        let exceeded_clone = Arc::clone(&exceeded);
        tracker.set_limit_exceeded_callback(Arc::new(move || {
            exceeded_clone.store(true, Ordering::Release);
        }));

        tracker.grow(10, now);

        assert!(tracker.meter_if_enabled().is_none());
        assert!(exceeded.load(Ordering::Acquire));
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    }

    #[test]
    fn lowered_limit_is_enforced_when_memory_metering_is_disabled() {
        let now = Instant::now();
        let entry = Arc::new(AtomicResourceEntry::new(0, 10, 0, 0, 0));
        let tracker = LinearMemoryTracker::new_with_metering(
            5,
            5,
            AgentMode::Durable,
            false,
            entry.clone(),
            grant(),
            false,
        );
        tracker.reconcile(5, now);
        let exceeded = Arc::new(AtomicBool::new(false));
        let exceeded_clone = Arc::clone(&exceeded);
        tracker.set_limit_exceeded_callback(Arc::new(move || {
            exceeded_clone.store(true, Ordering::Release);
        }));

        entry.update_memory_limit(6);
        assert!(!exceeded.load(Ordering::Acquire));
        entry.update_memory_limit(4);

        assert!(tracker.meter_if_enabled().is_none());
        assert!(exceeded.load(Ordering::Acquire));
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    }
}
