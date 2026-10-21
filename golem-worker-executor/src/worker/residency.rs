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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// This method is supposed to be called on a worker for what `is_currently_idle_but_running`
    /// previously returned true.
    ///
    /// It is not guaranteed that the worker is still "running (loaded in memory) but idle" when
    /// this method is called, so it rechecks this condition and only stops the worker if it
    /// is still true. If it was not true, it returns false.
    ///
    /// There are two conditions to this:
    /// - the ExecutionStatus must be suspended; this means the worker is currently not running any invocations
    /// - there must be no more pending invocations in the invocation queue
    ///
    /// Here we first acquire the worker lifecycle lock. This means the worker cannot be
    /// started/stopped or claimed for deletion while we are processing this method.
    /// If it was not running, then we don't have to stop it.
    /// If it was running, then we recheck the conditions and then stop the worker.
    ///
    /// We know that the conditions remain true because:
    /// - the invocation queue is empty, so it cannot get into `ExecutionStatus::Running`, as there is nothing to run
    /// - nothing can be added to the invocation queue because we are holding the worker lifecycle lock
    ///
    /// By passing the running lock to `stop_internal_running` it is never released and the stop eventually
    /// drops the `RunningWorker` instance.
    ///
    /// The `stopping` flag is only used to prevent re-entrance of the stopping sequence in case the invocation loop
    /// triggers a stop (in case of a failure - by the way it should not happen here because the worker is idle).
    pub async fn stop_if_idle(&self) -> bool {
        let active_agent = self.active_agent_for_this_owner().await;
        let reopen_entity_generation = match active_agent.as_ref() {
            Some(active_agent) => match active_agent.try_fence_idle_entity_bodies() {
                Some(reopen_generation) => reopen_generation,
                None => return false,
            },
            None => None,
        };
        let mut instance_guard = self.lock_non_stopping_worker().await;
        if instance_guard.ensure_not_deleting().is_err() {
            return false;
        }
        let stop_result = match &*instance_guard {
            WorkerInstance::Running(running) => {
                if self.is_running_worker_idle(running).await {
                    let stop_result = self
                        .stop_internal_locked(
                            &mut instance_guard,
                            false,
                            None,
                            UnloadRequest::ordinary(UnloadReason::Idle),
                            FinalWorkerState::Unloaded {
                                startup_failure: None,
                            },
                            PendingLiveInvocationDisposition::Fail,
                        )
                        .await;

                    Some(stop_result)
                } else {
                    None
                }
            }
            WorkerInstance::WaitingForPermit(_) => None,
            WorkerInstance::Stopping(_) => None,
            WorkerInstance::Unloaded { .. } | WorkerInstance::CleanupFailed(_) => None,
            WorkerInstance::Deleting(_) | WorkerInstance::StoppedForDeletion => None,
            WorkerInstance::Unresolved | WorkerInstance::Initializing(_) => None,
        };

        drop(instance_guard);

        if let Some(stop_result) = stop_result {
            self.handle_stop_result(stop_result).await;
            true
        } else {
            if let (Some(generation), Some(active_agent)) = (reopen_entity_generation, active_agent)
            {
                active_agent.reopen_entity_admission_if_generation(generation);
            }
            false
        }
    }
    /// Gets the estimated memory requirement of the worker.
    ///
    /// This covers only the per-worker linear memory. The compiled component
    /// module is shared by all workers of a component and is charged once per
    /// resident component via the component-charge registry, not per worker.
    pub async fn memory_requirement(&self) -> Result<u64, WorkerExecutorError> {
        let metadata = self.get_latest_worker_metadata().await;

        Ok(metadata.last_known_status.total_linear_memory_size)
    }
    /// Startup module-charge requirement for a worker about to be (re)started.
    ///
    /// Returns the component identity and compiled-module size to reserve with
    /// the gate, keyed to the revision [`Worker::create_instance`] will actually
    /// instantiate: when a pending update is queued, the worker loads the
    /// update's `target_revision`, not the last known one, so the charge must be
    /// keyed to — and sized from — the target. Keying it to the old revision
    /// would attach the held charge to the wrong resident module and, if the
    /// target module is larger, under-reserve memory.
    ///
    /// The invariant is: if `create_instance` can still successfully load the
    /// target revision, admission must charge the target revision. Resolving the
    /// target's module size is therefore handled by error class:
    ///
    /// - `ComponentServiceUnavailable`: `create_instance`'s later
    ///   `component_service().get(target)` may still succeed and load the target,
    ///   so we must not fall back to the current revision (that would under-reserve
    ///   and mis-key the charge). Back off and retry resolving the target, exactly
    ///   as the memory admission loop treats transient pressure, until it resolves
    ///   to a definite answer.
    /// - Any other error (`ComponentNotFound`, `ComponentServiceRefused` or any
    ///   other kind): the target cannot load, so `create_instance` will write a
    ///   `failed_update` and retry the *current* revision. Charge the current
    ///   revision/size to match — falling back here keeps the worker startable
    ///   instead of wedged, and `create_instance` drives the recovery.
    pub(in crate::worker) async fn startup_component_charge_requirement(
        &self,
    ) -> StartupComponentChargeRequirement {
        let metadata = self.get_latest_worker_metadata().await;
        let component_id = self.owned_agent_id.component_id();
        let current_revision = metadata.last_known_status.component_revision;
        let current_size = metadata.last_known_status.component_size;

        // Mirror create_instance. Deterministically rejected assisted heads are failed without
        // loading their targets, so charge the first remaining update that can reach a target
        // load, or the current revision when every queued update is rejected.
        let component_revision = startup_component_charge_revision(&metadata.last_known_status);

        // The currently-loaded revision's module size is already recorded in the
        // status; a pending-update target's size must be resolved from its
        // metadata so the reservation matches the module create_instance loads.
        if component_revision == current_revision {
            let canonical_bytes = metadata.last_known_status.total_linear_memory_size;
            let current = self.current_component.load();
            return startup_component_requirement(
                component_id,
                current_revision,
                current_size,
                current.metadata.initial_linear_memory_bytes(),
                canonical_bytes,
            );
        }

        let retry_delay = self.config().memory.acquire_retry_delay;
        loop {
            let result = self
                .component_service()
                .get_metadata(component_id, Some(component_revision))
                .await
                .map(|target| {
                    let initial_linear_memory_bytes = target.metadata.initial_linear_memory_bytes();
                    ResolvedComponentCharge {
                        module_bytes: target.component_size,
                        initial_linear_memory_bytes,
                        reserved_linear_memory_bytes: initial_linear_memory_bytes,
                        supported: component_support_error(&target.metadata).is_none(),
                    }
                });
            match classify_target_charge(&result) {
                TargetChargeAction::ChargeTarget(charge) => {
                    return StartupComponentChargeRequirement {
                        component_id,
                        component_revision,
                        module_bytes: charge.module_bytes,
                        startup_linear_memory_bytes: charge.initial_linear_memory_bytes,
                        reserved_linear_memory_bytes: charge.reserved_linear_memory_bytes,
                    };
                }
                TargetChargeAction::FallBackToCurrent => {
                    // The target revision cannot load; create_instance will fail
                    // the update and load the current revision, so charge that.
                    debug!(
                        "Pending-update target revision {component_revision} cannot load; charging against current revision and letting create_instance fail the update and recover"
                    );
                    let canonical_bytes = metadata.last_known_status.total_linear_memory_size;
                    let current = self.current_component.load();
                    return startup_component_requirement(
                        component_id,
                        current_revision,
                        current_size,
                        current.metadata.initial_linear_memory_bytes(),
                        canonical_bytes,
                    );
                }
                TargetChargeAction::Retry => {
                    // The component service is unavailable: create_instance may
                    // still load the target, so do not fall back to the current
                    // revision (that would under-reserve). Back off and retry
                    // resolving the target.
                    debug!(
                        "Transient failure resolving pending-update target revision {component_revision} for charge sizing, backing off and retrying"
                    );
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }
    /// Eviction module-charge accounting for an already-resident worker.
    ///
    /// Returns the component identity and compiled-module size of the module the
    /// worker is *currently* holding a charge for — the last known (loaded)
    /// revision, never a queued pending-update target. The pending update has not
    /// been applied yet, so the held charge is still keyed to the loaded
    /// revision; the eviction planner must use that same key and size, otherwise
    /// its refcount lookup and freed-bytes accounting would not match the charge
    /// that is actually released when the worker stops. Infallible: it reads only
    /// the persisted status, doing no metadata lookup.
    pub async fn resident_component_charge_requirement(
        &self,
    ) -> (ComponentId, ComponentRevision, u64) {
        let metadata = self.get_latest_worker_metadata().await;
        (
            self.owned_agent_id.component_id(),
            metadata.last_known_status.component_revision,
            metadata.last_known_status.component_size,
        )
    }
    /// Returns true if the worker is running, but it is not performing any invocations at the moment
    /// (ExecutionStatus::Suspended) and has no pending work that should keep the
    /// loaded worker resident while memory pressure is low.
    ///
    /// These workers can be stopped to free up available worker memory.
    pub async fn is_currently_idle_but_running(&self) -> bool {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => self.is_running_worker_idle(running).await,
            WorkerInstance::WaitingForPermit(_) => {
                debug!(
                    "Worker {} is waiting for permit, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            WorkerInstance::Unloaded { .. } => {
                debug!(
                    "Worker {} is unloaded, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            WorkerInstance::CleanupFailed(_) => {
                debug!(
                    "Worker {} has failed filesystem cleanup, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            // TODO: this probably wants to cooperate with memory free up
            WorkerInstance::Stopping(_) => {
                debug!(
                    "Worker {} is stopping, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            // TODO: this probably wants to cooperate with memory free up
            WorkerInstance::Deleting(_) | WorkerInstance::StoppedForDeletion => {
                debug!(
                    "Worker {} is deleting, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            WorkerInstance::Unresolved | WorkerInstance::Initializing(_) => false,
        }
    }
    pub(in crate::worker) async fn is_running_worker_idle(&self, running: &RunningWorker) -> bool {
        let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
        let has_pending_invocations = !self.pending_invocations().await.is_empty();
        let has_queued_internal_work = {
            let mut queue = running.queue.write().await;
            queue.retain(|invocation| !invocation.is_abandoned());
            !queue.is_empty()
        };
        let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
        let has_interrupt = running.interrupts.lock().await.has_interrupt();
        let has_filesystem_effects = self.has_active_filesystem_effects(running);
        let has_concurrent_agent_permit =
            running.concurrent_agent_permit_held.load(Ordering::Acquire);

        debug!(
            "Worker {} idle check: waiting_for_command={waiting_for_command} has_pending_invocations={has_pending_invocations} has_queued_internal_work={has_queued_internal_work} has_resume_replay={has_resume_replay} has_interrupt={has_interrupt} has_filesystem_effects={has_filesystem_effects} has_concurrent_agent_permit={has_concurrent_agent_permit}",
            self.owned_agent_id
        );

        waiting_for_command
            && !has_pending_invocations
            && !has_queued_internal_work
            && !has_resume_replay
            && !has_interrupt
            && !has_filesystem_effects
            && !has_concurrent_agent_permit
    }
    pub(in crate::worker) fn has_active_filesystem_effects(&self, running: &RunningWorker) -> bool {
        running
            .filesystem_activity
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(ResidentFilesystemActivity::has_active_effects)
    }
    /// Returns `true` iff this worker currently has a loaded wasmtime instance
    /// (i.e. its [`WorkerInstance`] is in the `Running` state).
    ///
    /// `Worker` shells can outlive their wasmtime instance — for example after
    /// memory-pressure eviction unloads the instance but the shell stays alive
    /// in [`ActiveAgents`] so its caches (read-only cache, pending
    /// invocations, …) can keep serving. This accessor lets callers
    /// distinguish those two states.
    pub async fn is_loaded(&self) -> bool {
        matches!(&*self.instance.lock().await, WorkerInstance::Running(_))
    }
    /// Starts a conditional ordinary `ActiveAgents` retirement unless deletion already owns it.
    /// The returned guard rolls the marker back unless the cache removal commits.
    pub(crate) async fn try_begin_cache_retirement(&self) -> Option<WorkerCacheRetirement<'_>> {
        if self
            .cache_retirement_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }

        let retirement = WorkerCacheRetirement {
            in_progress: &self.cache_retirement_in_progress,
            changed: self.durable_stream_producer.changed(),
            committed: false,
        };
        let instance = self.instance.lock().await;
        if !instance.deletion_owns_retirement() {
            Some(retirement)
        } else {
            None
        }
    }
    pub(crate) async fn try_begin_deletion_cache_retirement(
        &self,
    ) -> Option<WorkerCacheRetirement<'_>> {
        if self
            .cache_retirement_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }

        let retirement = WorkerCacheRetirement {
            in_progress: &self.cache_retirement_in_progress,
            changed: self.durable_stream_producer.changed(),
            committed: false,
        };
        matches!(&*self.instance.lock().await, WorkerInstance::Deleting(_)).then_some(retirement)
    }
    pub(in crate::worker) fn cache_retirement_in_progress(&self) -> bool {
        self.cache_retirement_in_progress.load(Ordering::Acquire)
    }
    /// Classifies the worker for eviction ordering under memory pressure.
    /// Returns `None` if the worker is not evictable.
    ///
    /// - `LoadedIdle`: resident in memory, not executing, no durable pending work.
    ///   Evicted first.
    /// - `WarmRunnable`: resident in memory, not executing, has durable pending
    ///   invocations. Evicted only when `LoadedIdle` workers are exhausted.
    /// - `None`: worker is actively executing, has non-durable in-memory work
    ///   pending, or is not loaded. Never evicted.
    pub async fn eviction_class(&self) -> Option<EvictionClass> {
        if self
            .active_agent_for_this_owner()
            .await
            .is_some_and(|active_agent| {
                active_agent
                    .entity_slots()
                    .iter()
                    .any(|slot| slot.active_invocation_count() != 0)
            })
        {
            return None;
        }
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                let has_queued_internal_work = {
                    let mut queue = running.queue.write().await;
                    queue.retain(|invocation| !invocation.is_abandoned());
                    !queue.is_empty()
                };
                let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
                let has_interrupt = running.interrupts.lock().await.has_interrupt();
                let has_filesystem_effects = self.has_active_filesystem_effects(running);
                let has_concurrent_agent_permit =
                    running.concurrent_agent_permit_held.load(Ordering::Acquire);

                // Non-evictable if actively executing or has non-durable in-memory work
                if !running_worker_can_be_evicted(
                    waiting_for_command,
                    has_queued_internal_work,
                    has_resume_replay,
                    has_interrupt,
                    has_filesystem_effects,
                    has_concurrent_agent_permit,
                ) {
                    return None;
                }

                let has_pending_invocations = !self.pending_invocations().await.is_empty();
                if has_pending_invocations {
                    Some(EvictionClass::WarmRunnable)
                } else {
                    Some(EvictionClass::LoadedIdle)
                }
            }
            _ => None,
        }
    }
    /// Stop this worker if it matches the given eviction class.
    ///
    /// Re-checks the eviction classification under the worker lifecycle lock to avoid
    /// races. Returns `true` if the worker was actually stopped.
    pub async fn stop_if_evictable(&self, target_class: EvictionClass) -> bool {
        self.stop_if_evictable_with_outcome(
            target_class,
            None,
            UnloadRequest::ordinary(UnloadReason::MemoryPressure),
        )
        .await
            != EvictionStopOutcome::Ineligible
    }
    pub(crate) async fn stop_if_evictable_with_outcome(
        &self,
        target_class: EvictionClass,
        expected_eligibility: Option<FilesystemPressureEligibility>,
        unload_request: UnloadRequest,
    ) -> EvictionStopOutcome {
        let active_agent = self.active_agent_for_this_owner().await;
        let reopen_entity_generation = match active_agent.as_ref() {
            Some(active_agent) => match active_agent.try_fence_idle_entity_bodies() {
                Some(reopen_generation) => reopen_generation,
                None => return EvictionStopOutcome::Ineligible,
            },
            None => None,
        };
        let mut instance_guard = self.lock_non_stopping_worker().await;
        if instance_guard.ensure_not_deleting().is_err() {
            return EvictionStopOutcome::Ineligible;
        }
        let should_stop = match &*instance_guard {
            WorkerInstance::Running(running) => {
                let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                let has_queued_internal_work = {
                    let mut queue = running.queue.write().await;
                    queue.retain(|invocation| !invocation.is_abandoned());
                    !queue.is_empty()
                };
                let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
                let has_interrupt = running.interrupts.lock().await.has_interrupt();
                let has_filesystem_effects = self.has_active_filesystem_effects(running);
                let has_concurrent_agent_permit =
                    running.concurrent_agent_permit_held.load(Ordering::Acquire);

                if !running_worker_can_be_evicted(
                    waiting_for_command,
                    has_queued_internal_work,
                    has_resume_replay,
                    has_interrupt,
                    has_filesystem_effects,
                    has_concurrent_agent_permit,
                ) {
                    false
                } else {
                    let eligibility = self.current_filesystem_pressure_eligibility(running);
                    let has_pending_invocations = !self.pending_invocations().await.is_empty();
                    let current_class = if has_pending_invocations {
                        EvictionClass::WarmRunnable
                    } else {
                        EvictionClass::LoadedIdle
                    };
                    current_class.eviction_priority() <= target_class.eviction_priority()
                        && expected_eligibility.is_none_or(|expected| expected == eligibility)
                }
            }
            _ => false,
        };

        if should_stop {
            let stop_result = self
                .stop_internal_locked(
                    &mut instance_guard,
                    false,
                    None,
                    unload_request,
                    FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                    PendingLiveInvocationDisposition::Fail,
                )
                .await;
            drop(instance_guard);
            self.handle_stop_result(stop_result).await;
            match &*self.instance.lock().await {
                WorkerInstance::CleanupFailed(_) => EvictionStopOutcome::CleanupFailed,
                WorkerInstance::Unloaded { .. } => EvictionStopOutcome::Unloaded,
                _ => EvictionStopOutcome::CleanupFailed,
            }
        } else {
            drop(instance_guard);
            if let (Some(generation), Some(active_agent)) = (reopen_entity_generation, active_agent)
            {
                active_agent.reopen_entity_admission_if_generation(generation);
            }
            EvictionStopOutcome::Ineligible
        }
    }
    pub(crate) async fn filesystem_pressure_eligibility(
        &self,
    ) -> Option<FilesystemPressureEligibility> {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                Some(self.current_filesystem_pressure_eligibility(running))
            }
            _ => None,
        }
    }
    pub(in crate::worker) fn current_filesystem_pressure_eligibility(
        &self,
        running: &RunningWorker,
    ) -> FilesystemPressureEligibility {
        let idle_since = running.idle_since_millis.load(Ordering::Acquire);
        let last_effect_completion = running
            .filesystem_activity
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |runtime| runtime.last_effect_completion_millis());
        FilesystemPressureEligibility {
            idle_since,
            last_effect_completion,
        }
    }
    pub(crate) fn filesystem_pressure_eligible_since(
        eligibility: FilesystemPressureEligibility,
    ) -> u64 {
        eligibility
            .idle_since
            .max(eligibility.last_effect_completion)
    }
    /// Records a committed guest `memory.grow` without blocking the store callback.
    pub fn request_memory_grow(self: &Arc<Self>, delta: u64) {
        let growth = self.memory_growth.lock().unwrap();
        growth
            .delta
            .try_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                Some(pending.saturating_add(delta))
            })
            .ok();
        if !growth.job_queued.swap(true, Ordering::AcqRel) {
            self.state_actor.grow_memory(self.clone(), growth.clone());
        }
    }
    pub(in crate::worker) async fn persist_pending_memory_growth(
        self: &Arc<Self>,
        growth: Arc<PendingMemoryGrowth>,
    ) {
        loop {
            let delta = growth.delta.swap(0, Ordering::AcqRel);
            if delta > 0 {
                self.add_to_oplog_or_retire(OplogEntry::grow_memory(delta))
                    .await;
            }

            let current_growth = self.memory_growth.lock().unwrap();
            if Arc::ptr_eq(&current_growth, &growth) {
                growth.job_queued.store(false, Ordering::Release);
                if growth.delta.load(Ordering::Acquire) > 0
                    && !growth.job_queued.swap(true, Ordering::AcqRel)
                {
                    self.state_actor.grow_memory(self.clone(), growth.clone());
                }
                return;
            }

            // A successful update rotated this accumulator before queueing its
            // ordered entry. No producer can now add to it, so drain it fully
            // here to keep all preceding growth ahead of that entry.
            if growth.delta.load(Ordering::Acquire) == 0 {
                growth.job_queued.store(false, Ordering::Release);
                return;
            }
        }
    }
    pub(crate) fn recover_wallet_generation(self: &Arc<Self>) {
        self.state_actor.recover_wallet_generation(self.clone());
    }
    pub(crate) fn request_memory_limit_interrupt(self: &Arc<Self>, memory: LinearMemoryTracker) {
        if !self
            .memory_limit_interrupt_queued
            .swap(true, Ordering::AcqRel)
        {
            self.state_actor.memory_limit_exceeded(self.clone(), memory);
        }
    }
    pub(in crate::worker) async fn request_agent_filesystem_limit_update(
        &self,
        allocated_bytes: u64,
    ) -> Result<(), WorkerExecutorError> {
        let receiver = {
            let instance = self.lock_non_stopping_worker().await;
            let WorkerInstance::Running(running) = &*instance else {
                return Ok(());
            };
            let (sender, receiver) = oneshot::channel();
            if running
                .sender
                .send(WorkerCommand::UpdateFilesystemLimit {
                    allocated_bytes,
                    sender,
                })
                .is_err()
            {
                return Ok(());
            }
            receiver
        };
        receiver.await.unwrap_or(Ok(()))
    }
    pub(crate) fn linear_memory_grant(&self) -> Arc<StdMutex<MemoryGrant>> {
        self.linear_memory_grant
            .lock()
            .unwrap()
            .clone()
            .expect("linear memory grant requested while worker is not running")
    }
    pub(in crate::worker) fn release_linear_memory_grant(&self) {
        if let Some(grant) = self.linear_memory_grant.lock().unwrap().take() {
            *grant.lock().unwrap() = MemoryGrant::inert(0);
        }
    }
    pub(crate) fn startup_linear_memory_bytes(&self) -> u64 {
        self.startup_linear_memory_bytes.load(Ordering::Acquire)
    }
}

/// The component revision a starting worker should be charged/admitted against.
///
/// When a pending update is queued, `create_instance` instantiates the update's
/// `target_revision` rather than the last known revision, so admission must
/// reserve and key the component charge against the target. With no pending
/// update, the last known revision is the one that will be instantiated.
pub(in crate::worker) fn component_charge_revision(
    pending_target_revision: Option<ComponentRevision>,
    last_known_revision: ComponentRevision,
) -> ComponentRevision {
    pending_target_revision.unwrap_or(last_known_revision)
}

pub(in crate::worker) fn startup_component_charge_revision(
    status: &AgentStatusRecord,
) -> ComponentRevision {
    component_charge_revision(
        snapshot_selection::active_head(status).map(|update| update.target_revision),
        status.component_revision,
    )
}

/// Classifies a `get_metadata(target)` result into the startup charge action,
/// preserving the invariant that admission charges the target revision whenever
/// `create_instance` can still load it. It uses the fetch classification of the
/// outcome table: only an unavailable component service leaves the update pending, so
/// only that is retried. Any other error fails the update at the start, which then
/// loads the current revision, so the charge falls back to the current revision.
/// An unsupported target also charges the current revision: startup refuses
/// it before loading it, even when its advertised memory would not fit.
pub(in crate::worker) fn classify_target_charge(
    result: &Result<ResolvedComponentCharge, WorkerExecutorError>,
) -> TargetChargeAction {
    match result {
        Ok(charge) if !charge.supported => TargetChargeAction::FallBackToCurrent,
        Ok(charge) => TargetChargeAction::ChargeTarget(*charge),
        Err(error) => match start_outcome::FetchProblem::of(error) {
            start_outcome::FetchProblem::Unavailable => TargetChargeAction::Retry,
            start_outcome::FetchProblem::NotFound
            | start_outcome::FetchProblem::Refused(_)
            | start_outcome::FetchProblem::Other => TargetChargeAction::FallBackToCurrent,
        },
    }
}

pub(in crate::worker) fn startup_component_requirement(
    component_id: ComponentId,
    component_revision: ComponentRevision,
    module_bytes: u64,
    initial_linear_memory_bytes: u64,
    canonical_linear_memory_bytes: u64,
) -> StartupComponentChargeRequirement {
    StartupComponentChargeRequirement {
        component_id,
        component_revision,
        module_bytes,
        startup_linear_memory_bytes: canonical_linear_memory_bytes,
        reserved_linear_memory_bytes: canonical_linear_memory_bytes
            .max(initial_linear_memory_bytes),
    }
}

pub(in crate::worker) fn running_worker_can_be_evicted(
    waiting_for_command: bool,
    has_queued_internal_work: bool,
    has_resume_replay: bool,
    has_interrupt: bool,
    has_filesystem_effects: bool,
    has_concurrent_agent_permit: bool,
) -> bool {
    waiting_for_command
        && !has_queued_internal_work
        && !has_resume_replay
        && !has_interrupt
        && !has_filesystem_effects
        && !has_concurrent_agent_permit
}

impl WorkerCacheRetirement<'_> {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for WorkerCacheRetirement<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.in_progress.store(false, Ordering::Release);
        }
        self.changed.notify_waiters();
    }
}

impl EvictionClass {
    /// Lower values are evicted first.
    pub fn eviction_priority(self) -> u8 {
        match self {
            EvictionClass::LoadedIdle => 0,
            EvictionClass::WarmRunnable => 1,
        }
    }
}
