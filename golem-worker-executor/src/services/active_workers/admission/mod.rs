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
//! Gates worker admission on the executor environment's *real* memory headroom
//! read from the [`MemoryProbe`], rather than on the estimate-based semaphore in
//! [`super::ActiveWorkers`]. This controller is the primary, authoritative
//! check against measured resident usage and refuses admission in normal
//! operation; the estimate semaphore is the second line of defence behind it,
//! its atomic permit acquisition catching the concurrent admissions this
//! (lockless) controller can let through on the same snapshot. When headroom is
//! short it evicts already-resident idle-then-warm work; if it still cannot make
//! room it rejects rather than over-committing.
//!
//! The controller is decoupled from `Worker`/wasmtime via the [`EvictionSource`]
//! trait so its decision logic can be exercised in isolation with synthetic
//! probes and candidate sets.

use super::memory_probe::MemoryProbe;
use async_trait::async_trait;

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
/// work as needed. Holds only its policy and probe; live state is read fresh
/// from the probe and the eviction source on each call (never cached).
pub struct AdmissionController {
    probe: Box<dyn MemoryProbe>,
    policy: AdmissionPolicy,
}

impl AdmissionController {
    pub fn new(probe: Box<dyn MemoryProbe>, policy: AdmissionPolicy) -> Self {
        Self { probe, policy }
    }

    /// Bytes available for new admissions: the carve-out ceiling
    /// (`usable_ratio × limit`) minus current usage. Saturating — never
    /// underflows when already over the ceiling.
    fn admissible_headroom(&self) -> u64 {
        let snapshot = self.probe.snapshot();
        let ceiling = (snapshot.limit_bytes as f64 * self.policy.usable_ratio) as u64;
        ceiling.saturating_sub(snapshot.current_bytes)
    }

    /// Decide whether `request_bytes` can be admitted, evicting from `source` if
    /// the current headroom is insufficient.
    ///
    /// Eviction is attempted idle-first, then warm, and only up to the shortfall
    /// (never evicts when headroom already suffices). After eviction the
    /// headroom is re-measured against ground truth; the request is admitted only
    /// if the real headroom now covers it, otherwise it is rejected.
    pub async fn try_admit(
        &self,
        request_bytes: u64,
        source: &dyn EvictionSource,
    ) -> AdmissionDecision {
        // Fast path: enough real headroom already, admit without evicting.
        if self.admissible_headroom() >= request_bytes {
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
        // in either direction while we were evicting.
        if self.admissible_headroom() >= request_bytes {
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
