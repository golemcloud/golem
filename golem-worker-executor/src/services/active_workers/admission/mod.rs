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

//! Measured-headroom admission decision.
//!
//! Gates worker admission on the executor environment's memory headroom. It is
//! the sole admission authority: there is no estimate-based semaphore behind it.
//!
//! The gate weighs two quantities against the usable ceiling:
//!
//! * Measured RSS from the [`MemoryProbe`] (cgroup `memory.current` on a
//!   constrained pod) — what is resident right now.
//! * The total linear memory *granted* to live workers — what they could fault
//!   in at any moment.
//!
//! Both matter because they fail in opposite directions. Measured RSS lags
//! admission: `memory.current` counts only touched pages, so a worker admitted
//! moments ago is not yet resident and a burst admitted against the same low
//! snapshot would collectively over-commit. The granted total leads residency: a
//! worker can fault in any page of the virtual memory it was already granted at
//! any later time, with no admission call to intercept it, so a gate that
//! reserved only what is resident would let a node full of lightly-touched
//! workers OOM by writing into memory they already hold. The gate therefore
//! reserves the full granted total from admission until unload, and admits
//! against the *larger* of measured RSS and that granted total — safe against
//! both the burst race and later faulting of granted pages.
//!
//! The granted total is maintained by two integer updates: a worker's grant is
//! added on admission, and removed when the [`MemoryGrant`] guard returned by
//! admission is dropped. Tying the removal to the guard's drop — rather than to
//! an explicit release call on some worker-lifecycle path — keeps the accounting
//! symmetric no matter how a worker's start ends: whether it becomes resident and
//! later stops, or its start is cancelled mid-flight (e.g. the worker is deleted
//! while still waiting for permits), dropping the guard returns its reservation
//! exactly once. The headroom check re-derives the reservation from the
//! maintained total and the current probe reading, so it is O(1) and exact
//! regardless of worker churn.
//!
//! When headroom is short the controller evicts already-resident idle-then-warm
//! work; if it still cannot make room it rejects rather than over-committing.
//!
//! The controller is decoupled from `Worker`/wasmtime via the [`EvictionSource`]
//! trait so its decision logic can be exercised in isolation with synthetic
//! probes and candidate sets.

use super::memory_probe::{MemoryProbe, MemorySnapshot};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Why an eviction candidate is worth evicting, in priority order. Lower
/// variants are evicted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EvictionPriority {
    /// Resident in memory, not executing, no durable pending work. Cheapest to
    /// evict — losing it costs at most a re-load on next use.
    Idle,
    /// Resident in memory, not executing, but has durable pending work. Evicted
    /// only after all idle candidates are exhausted.
    Warm,
}

/// A source of evictable, already-resident memory the controller can reclaim to
/// restore headroom. Abstracts over the live worker set so the decision logic
/// is testable without `Worker`/wasmtime.
#[async_trait]
pub(crate) trait EvictionSource: Send + Sync {
    /// Evict at the given priority tier, attempting to free at least
    /// `needed_bytes`. Returns the number of bytes actually reclaimed (which may
    /// be less if the tier is exhausted, or more if a single victim was larger
    /// than needed). Must not evict from a higher (more expensive) tier than the
    /// one requested.
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64;
}

#[cfg(test)]
pub(crate) struct NoEvictionSource;

#[cfg(test)]
#[async_trait]
impl EvictionSource for NoEvictionSource {
    async fn evict_at_most(&self, _priority: EvictionPriority, _needed_bytes: u64) -> u64 {
        0
    }
}

/// The outcome of an admission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionDecision {
    /// There is enough real headroom (possibly after eviction) to admit the
    /// request without risking the limit.
    Admit,
    /// Not enough headroom could be freed; the request must back off rather
    /// than over-commit.
    Reject,
}

/// Configuration for the headroom-based admission decision.
///
/// * `usable_ratio` — fraction of the measured limit usable for WASM admission.
///   The remainder is left for the host (the executor process, allocator
///   arenas, runtime buffers). Mirrors `worker_memory_ratio`, but applied to the
///   measured limit rather than the configured total.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AdmissionPolicy {
    /// Fraction (0.0..=1.0) of the measured limit usable for WASM admission.
    pub usable_ratio: f64,
}

/// Decides admission against measured headroom, evicting resident idle/warm
/// work as needed. Holds its policy and probe; live usage is read from the
/// probe's last coherent snapshot on each call. The only retained state is
/// `granted`: the total linear memory granted to live workers, maintained across
/// admit and unload, which the gate reserves so a worker cannot OOM the node by
/// faulting in granted pages.
pub(crate) struct AdmissionController {
    probe: Box<dyn MemoryProbe>,
    policy: AdmissionPolicy,
    granted: AtomicU64,
}

impl AdmissionController {
    pub fn new(probe: Box<dyn MemoryProbe>, policy: AdmissionPolicy) -> Self {
        let snapshot = probe.snapshot();
        let ceiling = snapshot.usable_limit_bytes(policy.usable_ratio);
        crate::metrics::workers::record_worker_memory_ceiling(ceiling);

        Self {
            probe,
            policy,
            granted: AtomicU64::new(0),
        }
    }

    /// Atomically admits `request_bytes` if the headroom computed against the
    /// current granted total covers it: reads `granted`, computes headroom, and
    /// reserves with an atomic compare-and-exchange so concurrent admissions
    /// cannot both pass against the same granted total and overshoot the ceiling.
    fn try_reserve(&self, request_bytes: u64) -> Result<(), u64> {
        let snapshot = self.probe.snapshot();
        self.try_reserve_with_snapshot(request_bytes, snapshot)
    }

    fn try_reserve_with_snapshot(
        &self,
        request_bytes: u64,
        snapshot: MemorySnapshot,
    ) -> Result<(), u64> {
        let ceiling = snapshot.usable_limit_bytes(self.policy.usable_ratio);
        crate::metrics::workers::record_worker_memory_ceiling(ceiling);
        crate::metrics::workers::record_worker_admission_rss(snapshot.current_bytes);
        let mut granted = self.granted.load(Ordering::Acquire);
        loop {
            let headroom = ceiling.saturating_sub(snapshot.current_bytes.max(granted));
            if headroom < request_bytes {
                return Err(headroom);
            }
            let Some(new_granted) = granted.checked_add(request_bytes) else {
                return Err(headroom);
            };
            match self.granted.compare_exchange_weak(
                granted,
                new_granted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    crate::metrics::workers::increase_worker_memory_granted(request_bytes);
                    return Ok(());
                }
                Err(current) => granted = current,
            }
        }
    }

    /// Record `request_bytes` of memory granted to a newly admitted worker. The
    /// gate reserves this until the worker unloads, because the worker may fault
    /// the granted pages in at any later time.
    fn reserve(&self, request_bytes: u64) {
        self.granted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |granted| {
                granted.checked_add(request_bytes)
            })
            .expect("committed memory reservation overflowed");
        crate::metrics::workers::increase_worker_memory_granted(request_bytes);
    }

    /// Reserve memory for a cost that is a committed consequence of an already
    /// admitted worker rather than a fresh admission — currently a component's
    /// compiled module, loaded into RAM when the first worker of the component
    /// becomes resident and shared by all its workers. Unlike admission this does
    /// not evict or reject (the worker is already in); it accounts the bytes so
    /// later admissions see them. Released with [`Self::release`].
    pub(crate) fn reserve_committed(&self, bytes: u64) {
        self.reserve(bytes);
    }

    /// Release the grant of a worker that has unloaded, given the bytes it was
    /// granted. Its pages leave memory, so its grant no longer needs reserving;
    /// not releasing it would permanently shrink admissible headroom as workers
    /// come and go.
    pub(crate) fn release(&self, reserved_bytes: u64) {
        let previously_granted = self
            .granted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |granted| {
                Some(granted.saturating_sub(reserved_bytes))
            })
            .expect("memory reservation release must always produce a value");
        if previously_granted < reserved_bytes {
            tracing::error!(
                granted_bytes = previously_granted,
                released_bytes = reserved_bytes,
                "Released memory exceeds the committed reservation"
            );
        }
        crate::metrics::workers::decrease_worker_memory_granted(
            reserved_bytes.min(previously_granted),
        );
    }

    /// Decide whether `request_bytes` can be admitted, evicting from `source` if
    /// the current headroom is insufficient.
    ///
    /// Eviction is attempted idle-first, then warm, and only up to the shortfall
    /// (never evicts when headroom already suffices). After eviction the
    /// headroom is re-measured against ground truth; the request is admitted only
    /// if the real headroom now covers it, otherwise it is rejected. On admit the
    /// request is added to the in-flight reservation.
    async fn try_admit(
        &self,
        request_bytes: u64,
        source: &dyn EvictionSource,
    ) -> AdmissionDecision {
        // Fast path: atomically admit if there is already enough real headroom.
        let headroom = match self.try_reserve(request_bytes) {
            Ok(()) => {
                return AdmissionDecision::Admit;
            }
            Err(headroom) => headroom,
        };

        // Reclaim resident, idle-then-warm work up to the shortfall.
        let shortfall = request_bytes.saturating_sub(headroom);
        let mut remaining = shortfall;

        for priority in [EvictionPriority::Idle, EvictionPriority::Warm] {
            if remaining == 0 {
                break;
            }
            let freed = source.evict_at_most(priority, remaining).await;
            remaining = remaining.saturating_sub(freed);
        }

        // Re-measure against ground truth rather than trusting the freed tally:
        // the probe is the authority, and other activity may have moved usage
        // in either direction while we were evicting. The check-and-reserve is
        // atomic so a concurrent admission cannot slip in between.
        if self.try_reserve(request_bytes).is_ok() {
            AdmissionDecision::Admit
        } else {
            AdmissionDecision::Reject
        }
    }

    /// The current admissible headroom. Used by tests to assert the gate's
    /// accounting without publishing production metrics. Production reads
    /// headroom indirectly through admission.
    #[cfg(test)]
    pub(crate) fn headroom_bytes(&self) -> u64 {
        let snapshot = self.probe.snapshot();
        let ceiling = snapshot.usable_limit_bytes(self.policy.usable_ratio);
        let granted = self.granted.load(Ordering::Acquire);
        ceiling.saturating_sub(snapshot.current_bytes.max(granted))
    }

    /// Admit `request_bytes`, evicting resident idle-then-warm work if needed,
    /// and on success return a [`MemoryGrant`] guard that owns the reservation
    /// and releases it on drop; `None` if the request cannot be admitted.
    ///
    /// The grant a starting worker holds passes through several `.await` points
    /// before the worker becomes resident (per-account concurrency, component
    /// charge, filesystem storage); if that work is cancelled — as when the
    /// worker is deleted while still waiting — the guard's drop returns the
    /// reservation, so a cancelled start cannot leak headroom.
    pub(crate) async fn admit(
        self: &Arc<Self>,
        request_bytes: u64,
        source: &dyn EvictionSource,
    ) -> Option<MemoryGrant> {
        match self.try_admit(request_bytes, source).await {
            AdmissionDecision::Admit => Some(MemoryGrant {
                controller: Some(self.clone()),
                bytes: request_bytes,
                reserved_bytes: request_bytes,
            }),
            AdmissionDecision::Reject => None,
        }
    }
}

impl Drop for AdmissionController {
    fn drop(&mut self) {
        let granted = self.granted.swap(0, Ordering::AcqRel);
        crate::metrics::workers::decrease_worker_memory_granted(granted);
    }
}

/// Owns a memory reservation made with the [`AdmissionController`] and returns it
/// to the gate when dropped, so a reservation is released exactly once regardless
/// of whether the worker became resident or its start was cancelled.
///
/// When measured admission is disabled (no controller) the grant is inert: it
/// reserves nothing and releasing it is a no-op, so callers can hold a grant
/// uniformly without branching on whether admission is active.
pub(crate) struct MemoryGrant {
    controller: Option<Arc<AdmissionController>>,
    bytes: u64,
    reserved_bytes: u64,
}

impl MemoryGrant {
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn is_tracked(&self) -> bool {
        self.controller.is_some()
    }

    pub(crate) fn shrink_to(&mut self, bytes: u64) {
        let released = self.bytes.saturating_sub(bytes);
        self.bytes -= released;
        let released_reservation = released.min(self.reserved_bytes);
        self.reserved_bytes -= released_reservation;
        if released_reservation > 0
            && let Some(controller) = &self.controller
        {
            controller.release(released_reservation);
        }
    }

    /// An inert grant for when measured admission is disabled: tracks the
    /// worker's bytes without reserving global headroom or releasing on drop.
    pub(crate) fn inert(bytes: u64) -> Self {
        Self {
            controller: None,
            bytes,
            reserved_bytes: 0,
        }
    }

    /// Fold another grant's bytes into this one, so a worker that grows its
    /// memory carries a single grant covering its whole reservation. The other
    /// grant is consumed and its reservation transferred here; the combined total
    /// is released exactly once when this grant drops.
    pub(crate) fn merge(&mut self, mut other: MemoryGrant) {
        self.bytes += other.bytes;
        self.reserved_bytes += other.reserved_bytes;
        if other.controller.is_some() {
            // Adopt the controller so a merged grant acquired while admission was
            // enabled still releases, even if `self` started inert.
            if self.controller.is_none() {
                self.controller = other.controller.take();
            }
        }
        // Neutralize the absorbed grant so its drop does not release the bytes
        // now owned by `self`.
        other.bytes = 0;
        other.reserved_bytes = 0;
        other.controller = None;
    }
}

impl std::fmt::Debug for MemoryGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryGrant")
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl Drop for MemoryGrant {
    fn drop(&mut self) {
        if let Some(controller) = &self.controller {
            controller.release(self.reserved_bytes);
        }
    }
}

#[cfg(test)]
mod tests;
