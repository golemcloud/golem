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

/// The number of refused admission attempts of one worker start between two warnings. With the
/// default retry delay of 500 ms, the first warning comes after about 10 s of waiting, and one
/// more comes every 10 s while the wait goes on.
const ADMISSION_REFUSALS_PER_WARNING: u32 = 20;

/// Whether `acquire_memory` logs a warning after its `refusals`-th refused attempt.
pub(super) fn admission_wait_warns(refusals: u32) -> bool {
    refusals.is_multiple_of(ADMISSION_REFUSALS_PER_WARNING)
}

impl UnloadedWorkerEvictionTask {
    pub(super) fn start<Ctx: WorkerCtx>(
        agents: Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
        card_interest_index: Arc<CardInterestIndex>,
        ttl: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        const MAX_SWEEP_PERIOD: Duration = Duration::from_secs(60);
        const ZERO_TTL_SWEEP_PERIOD: Duration = Duration::from_secs(1);

        let period = if ttl.is_zero() {
            ZERO_TTL_SWEEP_PERIOD
        } else {
            ttl.min(MAX_SWEEP_PERIOD)
        };
        Self(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => break,
                    _ = tokio::time::sleep(period) => {}
                }
                evict_expired_unloaded_agents(&agents, &card_interest_index, ttl).await;
            }

            // Cached workers own the service graph, which owns this cache. Shutdown must
            // break that cycle even for workers that have not reached their idle TTL.
            for agent_id in agents.keys().await {
                agents.remove(&agent_id).await;
            }
        }))
    }
}

impl Drop for UnloadedWorkerEvictionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<Ctx: WorkerCtx> ActiveAgents<Ctx> {
    /// Acquire (or share) the per-component module charge for a worker of the
    /// given component. The first resident worker of the component reserves its
    /// compiled-module size (scaled by `component_size_coefficient`) with the
    /// gate; subsequent workers share the same charge. The returned guard
    /// releases the charge when the last worker of the component unloads.
    pub async fn acquire_component_charge(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component_module_bytes: u64,
    ) -> WorkerComponentCharge {
        let charge_bytes =
            component_charge_bytes(self.component_size_coefficient, component_module_bytes);
        self.component_charges
            .acquire((component_id, component_revision), charge_bytes)
            .await
    }

    /// Blocking memory admission for a starting worker. Loops until the gate
    /// admits the request, backing off between attempts, and returns a
    /// [`MemoryGrant`] guard owning the reservation: the worker holds it for as
    /// long as it is resident and releases it by dropping the guard, so a start
    /// cancelled before the worker becomes resident cannot leak the reservation.
    ///
    /// A rejection is transient, not terminal. The gate reads resident memory
    /// from the probe, which lags real usage (cgroup `memory.current` only counts
    /// already-touched pages), so a worker admitted earlier may not yet be fully
    /// resident; pressure eases as its pages settle and as other workers finish.
    /// Each iteration backs off and re-reads the gate, so the caller eventually
    /// proceeds once headroom recovers rather than failing under momentary
    /// pressure. With measured admission disabled the worker is admitted
    /// immediately with an inert grant.
    pub(crate) async fn acquire_memory(&self, memory: u64) -> MemoryGrant {
        let Some(admission) = &self.admission else {
            return MemoryGrant::inert(memory);
        };
        let mut refusals: u32 = 0;
        loop {
            // Evicts idle-then-warm when real headroom is short; rejects (and we
            // back off) when it cannot make room rather than risking the limit.
            if let Some(grant) = admission.admit(memory, &self.eviction_source()).await {
                return grant;
            }
            refusals = refusals.saturating_add(1);
            if admission_wait_warns(refusals) {
                warn!(
                    requested = memory,
                    refusals,
                    retry_delay_ms = self.acquire_retry_delay.as_millis(),
                    "Memory admission keeps refusing a worker start"
                );
            }
            debug!("Measured headroom insufficient for {memory}, backing off and retrying");
            tokio::time::sleep(self.acquire_retry_delay).await;
        }
    }

    /// Builds an [`EvictionSource`] view over the live worker set for the
    /// admission controller to reclaim memory through.
    fn eviction_source(&self) -> WorkerEvictionSource<Ctx> {
        WorkerEvictionSource {
            agents: self.agents.clone(),
            component_charges: self.component_charges.clone(),
            component_size_coefficient: self.component_size_coefficient,
        }
    }

    /// Blocking admission of a starting worker together with its component's
    /// shared compiled module. Acquires the per-component module charge first —
    /// reserving the module's bytes with the gate for the first worker of the
    /// component, nothing more for later workers — then loops the worker's own
    /// memory admission until the gate admits it, backing off between attempts.
    ///
    /// Acquiring the module charge before admitting the worker's memory is what
    /// makes the first worker of a component gated on its memory *and* its module
    /// together: the memory admission measures headroom against a granted total
    /// that already includes the module, so a first worker is admitted only when
    /// both fit — the gate evicts or backs off rather than over-committing. Both
    /// the returned [`MemoryGrant`] (worker memory) and the
    /// [`WorkerComponentCharge`] (shared module) release their reservations on
    /// drop, so a start cancelled mid-flight returns the whole reservation.
    pub(crate) async fn acquire_with_component_charge(
        &self,
        memory: u64,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component_module_bytes: u64,
    ) -> (MemoryGrant, WorkerComponentCharge) {
        // Reserve the shared module first so the worker's memory admission
        // accounts for it. Held across admission retries and released on drop if
        // the start is cancelled.
        let charge = self
            .acquire_component_charge(component_id, component_revision, component_module_bytes)
            .await;
        let grant = self.acquire_memory(memory).await;
        (grant, charge)
    }

    /// Non-blocking memory admission for a growing worker. A single gate attempt:
    /// returns the additional [`MemoryGrant`] when the grow is admitted, or `None`
    /// when real headroom is insufficient even after eviction (the caller turns
    /// `None` into a retriable out-of-memory trap). The returned grant should be
    /// merged into the worker's existing grant so its whole reservation is
    /// released together on unload. With measured admission disabled the grow is
    /// always admitted with an inert grant.
    pub(crate) async fn try_acquire(&self, memory: u64) -> Option<MemoryGrant> {
        let Some(admission) = &self.admission else {
            return Some(MemoryGrant::inert(memory));
        };
        match admission.admit(memory, &self.eviction_source()).await {
            Some(grant) => Some(grant),
            None => {
                debug!("Measured headroom insufficient for {memory}, not admitting");
                None
            }
        }
    }
}

pub(super) async fn evict_expired_unloaded_agents<Ctx: WorkerCtx>(
    agents: &Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    card_interest_index: &CardInterestIndex,
    ttl: Duration,
) {
    for (owned_agent_id, active_agent) in agents.entries_older_than(ttl).await {
        let Some(worker) = active_agent.resolved_primary() else {
            let _ = agents
                .remove_if_cached_older_than(&owned_agent_id, ttl, |current| {
                    Arc::ptr_eq(current, &active_agent)
                        && Arc::strong_count(current) == 2
                        && current.primary.can_discard_unresolved()
                })
                .await;
            continue;
        };
        let Some(retirement) = worker.try_begin_cache_retirement().await else {
            continue;
        };
        if Arc::strong_count(&active_agent) != 2 || Arc::strong_count(&worker) != 2 {
            continue;
        }

        let removed = card_interest_index
            .clear_agent_interest_if(
                &owned_agent_id,
                agents.remove_if_cached_older_than(&owned_agent_id, ttl, |current| {
                    if !Arc::ptr_eq(current, &active_agent)
                        || Arc::strong_count(current) != 2
                        || !current
                            .resolved_primary()
                            .is_some_and(|worker| Arc::strong_count(&worker) == 3)
                    {
                        return false;
                    }
                    let Some(reopen_generation) = current.try_fence_idle_entity_bodies() else {
                        return false;
                    };
                    if worker.try_retire_durable_stream_producer() {
                        return true;
                    }
                    if let Some(generation) = reopen_generation {
                        current.reopen_entity_admission_if_generation(generation);
                    }
                    false
                }),
            )
            .await;

        if removed {
            retirement.commit();
        }
    }
}
impl<Ctx: WorkerCtx> FilesystemPressureVictim<Ctx> {
    pub(crate) fn stable_agent_id(&self) -> &str {
        &self.stable_agent_id
    }

    pub(crate) fn eligible_since(&self) -> u64 {
        self.eligible_since
    }
}

pub(crate) async fn eligible_loaded_idle_filesystem_pressure_victims<Ctx: WorkerCtx>(
    active_agents: &ActiveAgents<Ctx>,
) -> Vec<FilesystemPressureVictim<Ctx>> {
    let mut candidates = Vec::new();
    for (agent_id, active_agent) in active_agents.agents.iter().await {
        let Some(worker) = active_agent.resolved_primary() else {
            continue;
        };
        if is_loaded_idle_filesystem_pressure_candidate(worker.eviction_class().await) {
            let Some(eligibility) = worker.filesystem_pressure_eligibility().await else {
                continue;
            };
            candidates.push(FilesystemPressureVictim {
                stable_agent_id: agent_id.to_string(),
                eligible_since: Worker::<Ctx>::filesystem_pressure_eligible_since(eligibility),
                worker,
                eligibility,
            });
        }
    }
    candidates
}

pub(super) fn is_loaded_idle_filesystem_pressure_candidate(class: Option<EvictionClass>) -> bool {
    class == Some(EvictionClass::LoadedIdle)
}

pub(crate) fn request_loaded_idle_filesystem_unload<Ctx: WorkerCtx>(
    candidate: FilesystemPressureVictim<Ctx>,
    unload_request: UnloadRequest,
) -> tokio::task::JoinHandle<EvictionStopOutcome> {
    tokio::spawn(async move {
        stop_loaded_idle_if_eligible(
            candidate.eligibility,
            unload_request,
            move |target_class, eligibility, unload_request| async move {
                candidate
                    .worker
                    .stop_if_evictable_with_outcome(target_class, eligibility, unload_request)
                    .await
            },
        )
        .await
    })
}

pub(crate) async fn stop_loaded_idle_if_eligible<T>(
    eligibility: FilesystemPressureEligibility,
    unload_request: UnloadRequest,
    stop: impl AsyncFnOnce(EvictionClass, Option<FilesystemPressureEligibility>, UnloadRequest) -> T,
) -> T {
    stop(EvictionClass::LoadedIdle, Some(eligibility), unload_request).await
}

impl From<EvictionPriority> for crate::worker::EvictionClass {
    fn from(priority: EvictionPriority) -> Self {
        match priority {
            EvictionPriority::Idle => crate::worker::EvictionClass::LoadedIdle,
            EvictionPriority::Warm => crate::worker::EvictionClass::WarmRunnable,
        }
    }
}

/// Accounts the bytes freed by stopping one eviction candidate, updating the
/// working resident-count map.
///
/// A stop always frees the candidate's own linear `memory`. It additionally
/// frees the component's shared compiled `module_bytes`, but only when this stop
/// removes the *last* resident worker of the component — tracked by decrementing
/// `remaining[component]` and crediting the module when it reaches zero. Shared
/// by both [`plan_memory_eviction_stops`] (advisory planning) and
/// [`evict_at_most_memory`] (the actual stop loop) so the planned and the
/// returned freed totals use identical accounting.
pub(super) fn credit_eviction_stop<K: Eq + std::hash::Hash + Clone>(
    remaining: &mut std::collections::HashMap<K, usize>,
    component: &K,
    memory: u64,
    module_bytes: u64,
) -> u64 {
    let mut freed = memory;
    let count = remaining.entry(component.clone()).or_insert(0);
    *count = count.saturating_sub(1);
    if *count == 0 {
        freed += module_bytes;
    }
    freed
}

/// Plan how many leading (oldest-first) candidates the memory-eviction loop
/// should attempt to stop to free at least `needed_bytes`.
///
/// Each stop frees the candidate's own memory plus, when it removes the last
/// resident worker of its component, that component's shared module. `refcounts`
/// is the resident-worker count per component across the *whole* live set (not
/// just the candidates), so a component is credited its module only once every
/// resident worker of it — candidate or not — has been accounted as stopped.
///
/// Purely advisory: this decides how many workers to *attempt* to stop, never
/// releasing any bytes. The module charge is released only by the worker's
/// charge guard on drop (covering graceful stop, cancel and abort alike), and
/// the gate re-measures against the probe after eviction, so an imperfect plan
/// can at worst stop scanning slightly early or late.
pub(crate) fn plan_memory_eviction_stops<K: Eq + std::hash::Hash + Clone>(
    candidates: &[EvictionCandidateCost<K>],
    refcounts: &std::collections::HashMap<K, usize>,
    needed_bytes: u64,
) -> usize {
    // Working copy of the resident counts, decremented as we plan each stop, so
    // the module is credited exactly once — to the stop that takes a component's
    // resident count to zero.
    let mut remaining: std::collections::HashMap<K, usize> = refcounts.clone();
    let mut freed = 0u64;
    let mut stops = 0usize;
    for candidate in candidates {
        if freed >= needed_bytes {
            break;
        }
        freed += credit_eviction_stop(
            &mut remaining,
            &candidate.component,
            candidate.memory,
            candidate.module_bytes,
        );
        stops += 1;
    }
    stops
}

/// Evicts resident workers at a single priority tier, oldest-first, stopping
/// once at least `needed_bytes` have been freed or the tier is exhausted.
/// Returns the bytes actually reclaimed.
///
/// How many workers to attempt to stop is decided by
/// [`plan_memory_eviction_stops`], which credits a component's shared module to
/// the stop that removes its last resident worker — so stopping the last worker
/// of a component is correctly counted as freeing its memory *and* its module,
/// rather than memory alone, which would over-evict.
async fn evict_at_most_memory<Ctx: WorkerCtx>(
    agents: &Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    component_charges: &Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    component_size_coefficient: f64,
    priority: EvictionPriority,
    needed_bytes: u64,
) -> u64 {
    let target_class: crate::worker::EvictionClass = priority.into();

    let mut candidates = Vec::new();
    for (owned_agent_id, active_agent) in agents.iter().await {
        let Some(worker) = active_agent.resolved_primary() else {
            continue;
        };
        if let Some(class) = worker.eviction_class().await
            && class == target_class
            && let Ok(mem) = worker.memory_requirement().await
        {
            // Use the currently-loaded module the resident worker actually holds
            // a charge for, not any queued pending-update target: the update has
            // not been applied yet, so the held charge key and size must match the
            // loaded revision for the refcount lookup and freed accounting to be
            // correct.
            let (component_id, component_revision, module_bytes) =
                worker.resident_component_charge_requirement().await;
            let charge_bytes = component_charge_bytes(component_size_coefficient, module_bytes);
            let component: ComponentChargeKey = (component_id, component_revision);
            let last_changed = worker.last_execution_state_change();
            candidates.push((
                owned_agent_id,
                worker,
                mem,
                component,
                charge_bytes,
                last_changed,
            ));
        }
    }

    // Sort by timestamp oldest-first: the eviction plan and the stop loop both
    // walk candidates oldest-first.
    candidates.sort_by_key(|(_, _, _, _, _, ts)| ts.to_millis());

    // Decide, accounting for last-of-component module releases, how many leading
    // candidates to attempt to stop.
    let refcounts = component_charges.charge_refcounts();
    let costs: Vec<EvictionCandidateCost<ComponentChargeKey>> = candidates
        .iter()
        .map(
            |(_, _, mem, component, charge_bytes, _)| EvictionCandidateCost {
                memory: *mem,
                component: *component,
                module_bytes: *charge_bytes,
            },
        )
        .collect();
    let planned_stops = plan_memory_eviction_stops(&costs, &refcounts, needed_bytes);

    // Working copy of the resident counts, decremented on each successful stop so
    // a component's shared module is credited to `freed` exactly once — to the
    // stop that takes its resident count to zero. This mirrors
    // `plan_memory_eviction_stops`, but counts only stops that actually
    // succeeded, so the returned total reflects the memory genuinely reclaimed
    // (worker linear memory plus released module bytes). The admission gate uses
    // this total to decide whether to escalate to the next priority tier, so
    // omitting the module bytes here would under-report reclaimed headroom and
    // cause unnecessary higher-tier evictions.
    let mut remaining = refcounts;
    let mut freed = 0u64;
    for (agent_id, worker, mem, component, charge_bytes, _) in
        candidates.into_iter().take(planned_stops)
    {
        debug!("Trying to stop {target_class:?} {agent_id} to free up memory");
        if worker.stop_if_evictable(target_class).await {
            debug!("Stopped {target_class:?} {agent_id} to free up {mem} memory");
            crate::metrics::workers::record_worker_eviction(match priority {
                EvictionPriority::Idle => "LoadedIdle",
                EvictionPriority::Warm => "WarmRunnable",
            });
            // Credit the worker's linear memory plus, when this stop removes the
            // last resident worker of its component, the shared module bytes.
            freed += credit_eviction_stop(&mut remaining, &component, mem, charge_bytes);
        }
    }
    freed
}

#[async_trait]
impl<Ctx: WorkerCtx> EvictionSource for WorkerEvictionSource<Ctx> {
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64 {
        evict_at_most_memory(
            &self.agents,
            &self.component_charges,
            self.component_size_coefficient,
            priority,
            needed_bytes,
        )
        .await
    }
}

/// Gives the bytes that a component charges for its compiled module: the module size multiplied by
/// `coefficient`, rounded toward zero. A result above `u64::MAX` gives `u64::MAX`, and a negative
/// or NaN result gives 0.
pub(super) fn component_charge_bytes(coefficient: f64, module_bytes: u64) -> u64 {
    (coefficient * module_bytes as f64) as u64
}

/// Single attempt of the charge-first admission ordering used by
/// [`ActiveAgents::acquire_with_component_charge`]: reserve the component's
/// shared module, then admit the worker's own memory once.
///
/// Returns the worker's [`MemoryGrant`] and its [`WorkerComponentCharge`], or
/// `None` if the memory admission is refused (in which case dropping the charge
/// releases the module again). Exists so the composition of the admission gate
/// and the component-charge registry — the heart of the first-worker
/// memory + module gating — can be exercised without constructing a full
/// `ActiveAgents<Ctx>`. The production method runs this same ordering with the
/// memory admission wrapped in its blocking retry loop.
#[cfg(test)]
pub(super) async fn acquire_memory_and_component_charge(
    admission: &Arc<AdmissionController>,
    component_charges: &Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    source: &dyn EvictionSource,
    memory: u64,
    component: ComponentChargeKey,
    charge_bytes: u64,
) -> Option<(MemoryGrant, WorkerComponentCharge)> {
    // Reserve the component's shared module charge *first*. For the first worker
    // of a component this adds the module bytes to the gate's granted total; for
    // later workers the module is already held and nothing more is reserved.
    // Admitting the worker's own memory afterwards therefore measures headroom
    // against a granted total that already includes this module, so a first
    // worker is admitted only when its memory *and* its module both fit — the
    // gate can evict or reject rather than over-committing. If admission fails,
    // dropping the charge releases the module again, keeping the granted total
    // symmetric.
    let charge = component_charges.acquire(component, charge_bytes).await;
    let grant = admission.admit(memory, source).await?;
    Some((grant, charge))
}
impl Drop for GateCharge {
    fn drop(&mut self) {
        if let Some(admission) = &self.admission {
            admission.release(self.bytes);
        }
    }
}

#[async_trait]
impl ChargeSource for GateChargeSource {
    type Charge = GateCharge;

    async fn acquire_charge(&self, bytes: u64) -> GateCharge {
        if let Some(admission) = &self.admission {
            admission.reserve_committed(bytes);
        }
        GateCharge {
            admission: self.admission.clone(),
            bytes,
        }
    }
}
