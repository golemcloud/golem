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
//! added on admission and removed on unload (via [`AdmissionController::release`]
//! from the worker lifecycle). The headroom check re-derives the reservation
//! from this maintained total and the current probe reading, so it is O(1) and
//! exact regardless of worker churn.
//!
//! When headroom is short the controller evicts already-resident idle-then-warm
//! work; if it still cannot make room it rejects rather than over-committing.
//!
//! The controller is decoupled from `Worker`/wasmtime via the [`EvictionSource`]
//! trait so its decision logic can be exercised in isolation with synthetic
//! probes and candidate sets.

use super::memory_probe::MemoryProbe;
use async_trait::async_trait;
use std::sync::Mutex;

/// Why an eviction candidate is worth evicting, in priority order. Lower
/// variants are evicted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvictionPriority {
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
pub trait EvictionSource: Send + Sync {
    /// Evict at the given priority tier, attempting to free at least
    /// `needed_bytes`. Returns the number of bytes actually reclaimed (which may
    /// be less if the tier is exhausted, or more if a single victim was larger
    /// than needed). Must not evict from a higher (more expensive) tier than the
    /// one requested.
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64;
}

/// The outcome of an admission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
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
pub struct AdmissionPolicy {
    /// Fraction (0.0..=1.0) of the measured limit usable for WASM admission.
    pub usable_ratio: f64,
}

/// Decides admission against measured headroom, evicting resident idle/warm
/// work as needed. Holds its policy and probe; live usage is read fresh from the
/// probe on each call. The only retained state is `granted`: the total linear
/// memory granted to live workers, maintained across admit and unload, which the
/// gate reserves so a worker cannot OOM the node by faulting in granted pages.
pub struct AdmissionController {
    probe: Box<dyn MemoryProbe>,
    policy: AdmissionPolicy,
    granted: Mutex<u64>,
}

impl AdmissionController {
    pub fn new(probe: Box<dyn MemoryProbe>, policy: AdmissionPolicy) -> Self {
        let ceiling = (probe.snapshot().limit_bytes as f64 * policy.usable_ratio) as u64;
        crate::metrics::workers::record_worker_memory_ceiling(ceiling);
        Self {
            probe,
            policy,
            granted: Mutex::new(0),
        }
    }

    /// Bytes available for a new admission: the usable ceiling minus the larger
    /// of measured RSS and the total memory granted to live workers. Saturating —
    /// never underflows when already over the ceiling.
    ///
    /// A worker can fault in any page of the virtual memory it was granted at any
    /// time, with no admission call to intercept it, so the gate must reserve the
    /// full granted total even before it is resident. Measured RSS is only larger
    /// than the granted total transiently (host/runtime overhead the grant does
    /// not cover), so taking the maximum keeps the gate safe against both the
    /// grant a worker may yet fault in and any usage the grant does not capture.
    fn admissible_headroom(&self) -> u64 {
        let granted = *self.granted.lock().unwrap();
        self.headroom_with_granted(granted)
    }

    /// Computes admissible headroom for an already-read `granted` value. Reads
    /// the probe and emits the ceiling/RSS metrics. Kept separate from the lock
    /// acquisition so the decision-and-reserve sequence can hold the lock across
    /// both steps (see [`Self::try_reserve_locked`]).
    fn headroom_with_granted(&self, granted: u64) -> u64 {
        let snapshot = self.probe.snapshot();
        let ceiling = (snapshot.limit_bytes as f64 * self.policy.usable_ratio) as u64;
        crate::metrics::workers::record_worker_memory_ceiling(ceiling);
        crate::metrics::workers::record_worker_admission_rss(snapshot.current_bytes);
        ceiling.saturating_sub(snapshot.current_bytes.max(granted))
    }

    /// Atomically admits `request_bytes` if the headroom computed against the
    /// current granted total covers it: reads `granted`, computes headroom, and
    /// adds the reservation all under one lock so two concurrent admissions
    /// cannot both pass the check against the same headroom and overshoot the
    /// ceiling. Returns whether the request was admitted.
    fn try_reserve_locked(&self, request_bytes: u64) -> bool {
        let mut granted = self.granted.lock().unwrap();
        if self.headroom_with_granted(*granted) >= request_bytes {
            *granted += request_bytes;
            crate::metrics::workers::record_worker_memory_granted(*granted);
            true
        } else {
            false
        }
    }

    /// Record `request_bytes` of memory granted to a newly admitted worker. The
    /// gate reserves this until the worker unloads, because the worker may fault
    /// the granted pages in at any later time.
    fn reserve(&self, request_bytes: u64) {
        let mut granted = self.granted.lock().unwrap();
        *granted += request_bytes;
        crate::metrics::workers::record_worker_memory_granted(*granted);
    }

    /// Reserve memory for a cost that is a committed consequence of an already
    /// admitted worker rather than a fresh admission — currently a component's
    /// compiled module, loaded into RAM when the first worker of the component
    /// becomes resident and shared by all its workers. Unlike admission this does
    /// not evict or reject (the worker is already in); it accounts the bytes so
    /// later admissions see them. Released with [`Self::release`].
    pub fn reserve_committed(&self, bytes: u64) {
        self.reserve(bytes);
    }

    /// Release the grant of a worker that has unloaded, given the bytes it was
    /// granted. Its pages leave memory, so its grant no longer needs reserving;
    /// not releasing it would permanently shrink admissible headroom as workers
    /// come and go.
    pub fn release(&self, reserved_bytes: u64) {
        let mut granted = self.granted.lock().unwrap();
        *granted = granted.saturating_sub(reserved_bytes);
        crate::metrics::workers::record_worker_memory_granted(*granted);
    }

    /// Pre-register grant bytes for workers that were already live when the
    /// controller was created. Test-only: production registers every worker's
    /// grant through admission.
    #[cfg(test)]
    pub fn seed_granted(&self, bytes: u64) {
        *self.granted.lock().unwrap() += bytes;
    }

    /// Decide whether `request_bytes` can be admitted, evicting from `source` if
    /// the current headroom is insufficient.
    ///
    /// Eviction is attempted idle-first, then warm, and only up to the shortfall
    /// (never evicts when headroom already suffices). After eviction the
    /// headroom is re-measured against ground truth; the request is admitted only
    /// if the real headroom now covers it, otherwise it is rejected. On admit the
    /// request is added to the in-flight reservation.
    pub async fn try_admit(
        &self,
        request_bytes: u64,
        source: &dyn EvictionSource,
    ) -> AdmissionDecision {
        // Fast path: atomically admit if there is already enough real headroom.
        if self.try_reserve_locked(request_bytes) {
            return AdmissionDecision::Admit;
        }

        // Reclaim resident, idle-then-warm work up to the shortfall.
        let shortfall = request_bytes.saturating_sub(self.admissible_headroom());
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
        if self.try_reserve_locked(request_bytes) {
            AdmissionDecision::Admit
        } else {
            AdmissionDecision::Reject
        }
    }

    /// The current admissible headroom. Exposed for metrics and for callers that
    /// want to make their own pre-check.
    pub fn headroom_bytes(&self) -> u64 {
        self.admissible_headroom()
    }
}

#[cfg(test)]
mod tests;
