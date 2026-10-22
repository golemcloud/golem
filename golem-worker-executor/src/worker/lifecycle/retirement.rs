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
    /// Records retirement without taking the lifecycle lock. Owner writes stop immediately;
    /// `interrupt_and_retire` carries out the shared stop after that lock is released.
    pub(crate) fn record_retirement(&self, kind: InterruptKind, reason: RetirementReason) {
        self.owner_retirement.get_or_init(|| OwnerRetirement {
            kind,
            stop: tokio::sync::OnceCell::new(),
        });
        // The lost shard is known before any stop that the retirement causes, so an upload that
        // a stop ends sees it and cancels its save.
        let newly_lost =
            self.lost_shard
                .send_if_modified(|lost| match newly_lost(lost.is_some(), &reason) {
                    Some(reason) => {
                        *lost = Some(reason);
                        true
                    }
                    None => false,
                });
        self.owner_retirement_requested.cancel();
        self.durable_stream_producer.fence();
        if newly_lost {
            // Debug rather than warn: the oplog that latched a fence has already warned with
            // both epochs, and a revoke or reassignment is logged by the sweep.
            debug!(
                agent_id = %self.owned_agent_id,
                %reason,
                "Retiring the agent: this executor no longer owns its shard"
            );
        }
    }
    /// A receiver of why the shard of this worker moved to another executor, once it has.
    pub(crate) fn lost_shard(&self) -> LostShard {
        self.lost_shard.subscribe()
    }
    /// Whether this worker is retired because its shard moved to another executor.
    pub(crate) fn retired_for_lost_shard(&self) -> bool {
        self.lost_shard.borrow().is_some()
    }
    /// Why the shard moved, once it has.
    pub(crate) fn lost_shard_reason(&self) -> Option<RetirementReason> {
        self.lost_shard.borrow().clone()
    }
    /// Starts the lost-shard retirement in a task of its own, for a caller the retirement's stop
    /// would wait for. Not async, so the retirement's future is not part of the caller's.
    pub(crate) fn spawn_lost_shard_retirement(self: Arc<Self>, reason: RetirementReason) {
        tokio::spawn(async move {
            let _ = self
                .interrupt_and_retire(InterruptKind::ShardLost, reason)
                .await;
        });
    }
    /// Records a lost-shard retirement when `error` is one, per [`RetirementReason::of_error`], and
    /// returns whether it was: the caller must then neither record the failure nor retry in place.
    /// Any other failure is the caller's to handle; a write it makes is refused by the fence once
    /// the shard's new owner has claimed the oplog.
    pub(crate) fn retire_if_shard_lost(&self, error: &WorkerExecutorError) -> bool {
        let Some(reason) = RetirementReason::of_error(error) else {
            return false;
        };
        self.record_retirement(InterruptKind::ShardLost, reason);
        true
    }
    /// What a caller of a retired worker is told. A lost shard is answered with the routing miss
    /// the worker service retries on the shard's new owner; any other retirement is final here.
    pub(crate) fn retirement_error(&self) -> WorkerExecutorError {
        if self.retired_for_lost_shard() {
            WorkerExecutorError::ShardingNotReady
        } else {
            WorkerExecutorError::runtime("Worker ownership has retired")
        }
    }
    /// What retiring reports to whoever asked for it: done, or for a lost shard the routing miss,
    /// so a caller that wanted this executor to act on the agent goes to its new owner instead.
    pub(in crate::worker) fn retirement_result(&self) -> Result<(), WorkerExecutorError> {
        if self.retired_for_lost_shard() {
            Err(WorkerExecutorError::ShardingNotReady)
        } else {
            Ok(())
        }
    }
    /// Drops this generation from `ActiveAgents`. A newer generation cached under the same id is
    /// left alone: a retiring agent can pass through here more than once, and no repeat pass may
    /// evict the generation that replaced it.
    ///
    /// Takes `&self` rather than the `Arc`, because the stop path that removes a retiring
    /// generation holds only a reference; the cache supplies the `Arc` it checks identity against.
    pub(crate) async fn remove_from_active_agents(&self) {
        // A lost shard's entity bodies are torn down as `ShardLost`: the agent was not
        // interrupted through the Golem API, its shard moved.
        let owner_failure = if self.retired_for_lost_shard() {
            OwnerFailureWinner::Lifecycle(InterruptKind::ShardLost)
        } else {
            OwnerFailureWinner::Lifecycle(InterruptKind::Interrupt(Timestamp::now_utc()))
        };
        self.deps
            .active_agents()
            .remove_generation(self, owner_failure)
            .await;
    }
    /// Whether this worker is still the generation `ActiveAgents` has cached for its agent id.
    pub(in crate::worker) async fn is_current_cached_owner(&self) -> bool {
        self.active_agents()
            .try_get(&self.owned_agent_id)
            .await
            .is_some_and(|worker| std::ptr::eq(worker.as_ref(), self))
    }
    pub(crate) async fn active_agent_for_this_owner(&self) -> Option<Arc<ActiveAgent<Ctx>>> {
        self.active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await
            .filter(|active_agent| std::ptr::eq(active_agent.primary().as_ref(), self))
    }
    /// Interrupts and retires this worker as its owner - an environment deletion, or its shard
    /// moving to another executor (`ShardLost`) - draining it and dropping it from `ActiveAgents`.
    /// Idempotent and shared: a second caller while retirement is already in flight awaits the
    /// same outcome instead of repeating it.
    ///
    /// A lost shard writes and caches nothing - the oplog is the new owner's - and answers every
    /// caller with the routing miss that sends it to the new owner.
    pub(crate) async fn interrupt_and_retire(
        self: &Arc<Self>,
        interrupt: InterruptKind,
        reason: RetirementReason,
    ) -> Result<(), WorkerExecutorError> {
        self.record_retirement(interrupt, reason);
        if matches!(interrupt, InterruptKind::ShardLost) {
            // Signalled first, so a running guest leaves wasmtime even when a deletion owns the
            // stop or this generation is no longer the cached one. The ack is not awaited: a
            // caller blocking on it would panic if the worker was already stopping.
            self.set_interrupting_for(InterruptKind::ShardLost, UnloadReason::ShardLost)
                .await?;
        }
        // A deletion that already failed on the lost shard holds the cache entry with nothing
        // running, and nothing here can finish it any more. Checked on every call rather than in
        // the shared stop below: a stop started while that deletion was still running returned
        // early, and its result is what every later call receives.
        if self.retired_for_lost_shard()
            && self.deletion_owns_retirement().await
            && self.deletion_failed().await
        {
            self.evict_failed_deletion().await;
        }
        let retirement = self
            .owner_retirement
            .get()
            .expect("the retirement was recorded above");
        let interrupt = retirement.kind;
        retirement
            .stop
            .get_or_init(|| {
                std::future::ready({
                    let worker = self.clone();
                    let task = tokio::spawn(async move {
                        let mut cleanup = worker.owner_cleanup.lock().await;
                        if worker.deletion_owns_retirement().await {
                            return worker.retirement_result();
                        }
                        if *cleanup == OwnerCleanupState::Retired
                            || !worker.is_current_cached_owner().await
                        {
                            return worker.retirement_result();
                        }
                        let forwarding = downcast_oplog::<ForwardingOplog>(&worker.oplog);
                        worker
                            .quiesce_for_owner_retirement(Some(interrupt), forwarding.as_deref())
                            .await?;
                        worker.remove_from_active_agents().await;
                        *cleanup = OwnerCleanupState::Retired;
                        worker.retirement_result()
                    });
                    async move {
                        task.await.map_err(|error| {
                            WorkerExecutorError::runtime(format!(
                                "Worker retirement failed: {error}"
                            ))
                        })?
                    }
                    .boxed()
                    .shared()
                })
            })
            .await
            .clone()
            .await
    }
    pub(in crate::worker) async fn quiesce_for_owner_retirement(
        self: &Arc<Self>,
        interrupt: Option<InterruptKind>,
        forwarding: Option<&ForwardingOplog>,
    ) -> Result<(), WorkerExecutorError> {
        let retirement = self.retire_durable_stream_producer();
        if let Some(interrupt) = interrupt {
            self.set_interrupting(interrupt).await?;
        }
        let unload_reason = interrupt.map_or(UnloadReason::Idle, UnloadReason::from_interrupt);
        self.stop_internal(
            false,
            None,
            UnloadRequest::ordinary(unload_reason),
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            PendingLiveInvocationDisposition::Fail,
        )
        .await;
        if let WorkerInstance::CleanupFailed(error) = &*self.instance.lock().await {
            return Err(error.clone());
        }
        self.owner_execution
            .tool_operations()
            .join_owner_failure_cleanup()
            .await?;
        // A lost shard: the oplog is the new owner's. No terminal is claimed, written or cached,
        // and the waiters are sent to the owner (`fail_pending_invocations` maps the error).
        if self.retired_for_lost_shard() {
            self.fail_pending_invocations(WorkerExecutorError::runtime(
                "Worker ownership has retired",
            ))
            .await;
        } else if interrupt.is_some() {
            self.join_interrupt_establishment().await?;
            self.commit_pending_terminal_stop().await?;
        }
        self.join_interrupt_establishment().await?;
        self.durable_stream_attachment_reconciler.stop().await;
        let lost_shard = self.retired_for_lost_shard();
        // A lost shard's producer and lifecycle writes are refused, which changes nothing about
        // the stop that follows.
        let retired = retirement.await;
        let drained = self.state_actor.drain_lifecycle().await;
        if !lost_shard {
            retired?;
            drained?;
        }
        if let Some(forwarding) = forwarding {
            forwarding.drain_forwarding().await;
        }
        if !lost_shard {
            self.durable_stream_commit()(None).await;
        }
        self.status_flusher.begin_delete().await;
        self.status_checkpointer.begin_delete().await;
        Ok(())
    }
    /// Settles a published terminal left after the Store's selected outcome or physical unload.
    /// A claimed invocation terminal and a completed or failed result are never written again.
    pub(in crate::worker) async fn commit_pending_terminal_stop(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        if self.retired_for_lost_shard() {
            return Ok(());
        }
        let pending = self.interrupt_signal.lock().await.claim_pending_terminal();
        let Some(pending) = pending else {
            return Ok(());
        };
        pending.establishment.wait().await;
        let status = self.get_last_known_status().await;
        if !matches!(
            status.status,
            AgentStatus::Running | AgentStatus::Retrying | AgentStatus::Suspended
        ) {
            return Ok(());
        }
        let entry = match pending.kind {
            InterruptKind::EntityCancellation => {
                unreachable!("entity cancellation cannot interrupt the owner")
            }
            InterruptKind::Suspend(_) => OplogEntry::suspend(),
            InterruptKind::Interrupt(_) => OplogEntry::interrupted(),
            InterruptKind::Restart | InterruptKind::Jump | InterruptKind::ShardLost => {
                return Ok(());
            }
        };
        let recorded = self.add_and_commit_oplog(entry).await.map(|_| ());
        match recorded {
            Ok(_) => {
                if matches!(pending.kind, InterruptKind::Interrupt(_))
                    && let Some(key) = &status.current_idempotency_key
                {
                    self.store_invocation_failure(key, &TrapType::Interrupt(pending.kind))
                        .await;
                }
                Ok(())
            }
            Err(error) => {
                self.terminal_write_failed(error).await;
                Ok(())
            }
        }
    }
}

/// The shard epoch the agent's oplog is opened to assert, read from this executor's current
/// assignment. See [`shard_epoch_to_assert`].
pub(in crate::worker) fn owned_shard_epoch<T: HasShardService>(
    this: &T,
    agent_id: &AgentId,
) -> Result<Option<ShardEpoch>, WorkerExecutorError> {
    shard_epoch_to_assert(
        this.shard_service().try_get_current_assignment().as_ref(),
        agent_id,
    )
}

/// The shard epoch an oplog opened for `agent_id` asserts under `assignment`.
///
/// `Ok(None)` only when there is no assignment at all, before the first registration.
///
/// An assignment that does not hold the agent's shard is refused with `ShardingNotReady`, which
/// the worker service answers by refreshing its routing and retrying. It does not map to `None`,
/// because admission and this read take separate locks. A revoke can land between them, and its
/// sweep cannot see a worker that is still being built. That worker would otherwise open an oplog
/// that asserts nothing and stay cached, unfenced, across a later re-grant of the shard.
///
/// The same refusal covers a cleared assignment (lapsed lease, deregistration), which holds no
/// shards at all.
pub(in crate::worker) fn shard_epoch_to_assert(
    assignment: Option<&ShardAssignment>,
    agent_id: &AgentId,
) -> Result<Option<ShardEpoch>, WorkerExecutorError> {
    let Some(assignment) = assignment else {
        return Ok(None);
    };
    let shard_id = ShardId::from_agent_id(agent_id, assignment.number_of_shards);
    assignment
        .epoch_of(&shard_id)
        .map(Some)
        .ok_or(WorkerExecutorError::ShardingNotReady)
}

/// The lost-shard reason that a retirement for `reason` records, when the worker has `lost` one
/// already or not. The first reason that is not `Requested` is kept; a later one changes nothing.
pub(in crate::worker) fn newly_lost(
    lost: bool,
    reason: &RetirementReason,
) -> Option<RetirementReason> {
    match (lost, reason) {
        (true, _) | (false, RetirementReason::Requested) => None,
        (false, reason) => Some(reason.clone()),
    }
}

impl RetirementReason {
    /// The lost-shard reason `error` carries, if it means the agent's shard was lost.
    ///
    /// Such a failure is not this executor's to record or to retry. An append would be refused by
    /// the same fence that caused it, and on a shard lost without one it would write an error into
    /// an oplog the new owner is already recovering. A retry in place would reopen the oplog at the
    /// epoch this executor no longer holds. Every path that fails on a lost shard - startup and
    /// replay recovery, streaming-session completion, the dropped-call drain, an interrupt -
    /// classifies with this one function, so none of them can take the in-place retry another one
    /// refuses.
    pub(crate) fn of_error(error: &WorkerExecutorError) -> Option<Self> {
        match error {
            WorkerExecutorError::OplogFenced {
                agent_id,
                expected_epoch,
                actual_epoch,
            } => Some(Self::Fenced(Some(OplogFence {
                agent_id: agent_id.clone(),
                expected_epoch: ShardEpoch(*expected_epoch),
                actual_epoch: actual_epoch.map(ShardEpoch),
            }))),
            WorkerExecutorError::Interrupted {
                kind: InterruptKind::ShardLost,
            } => Some(Self::Fenced(None)),
            _ => None,
        }
    }
}

impl std::fmt::Display for RetirementReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Requested => write!(f, "requested"),
            Self::Fenced(Some(fence)) => write!(
                f,
                "fenced: asserted epoch {}, stored {:?}",
                fence.expected_epoch.0,
                fence.actual_epoch.map(|epoch| epoch.0)
            ),
            Self::Fenced(None) => write!(f, "fenced"),
            Self::ShardRevoked => write!(f, "shard revoked"),
            Self::ShardNotAssigned => write!(f, "shard not assigned"),
        }
    }
}

/// Whether an agent whose oplog asserts `held` has been superseded by a delivery that assigns its
/// shard at `assigned`.
///
/// - A shard's epoch rises only when it changed owner in between. A kept shard at a higher epoch
///   therefore means another executor may have written to the agent.
/// - An equal epoch is the same ownership generation.
/// - A lower epoch never comes from a newer owner, because the shard manager never lowers an
///   epoch. Giving the agent up would only reopen it below the epoch its oplog row already holds.
/// - `None` on either side is not this rule's business. An ephemeral handle asserts nothing, and
///   an absent shard is the membership check's to handle.
pub(crate) fn epoch_superseded(held: Option<ShardEpoch>, assigned: Option<ShardEpoch>) -> bool {
    matches!((held, assigned), (Some(held), Some(assigned)) if held < assigned)
}

/// Whether a delivered `assignment` takes `agent_id` away from this executor. `held` is the epoch
/// the agent's oplog asserts, `None` when it asserts none or the agent has no oplog yet.
///
/// True when either:
/// - the assignment does not hold the agent's shard. No assignment at all holds nothing, the same
///   answer `ShardService::check_worker` gives.
/// - it holds the shard at a higher epoch than `held`, per [`epoch_superseded`]. The shard left and
///   came back, so another executor may have written to the agent in between.
///
/// Every sweep of a delivered assignment selects with this one predicate. A caller that checks
/// agents still being created passes `None`, which leaves only the membership test.
///
/// Membership and epochs only, never the lease. A lapsed lease refuses new work and leaves running
/// work alone.
pub(crate) fn retired_by_assignment(
    assignment: Option<&ShardAssignment>,
    agent_id: &AgentId,
    held: Option<ShardEpoch>,
) -> bool {
    let Some(assignment) = assignment else {
        return true;
    };
    let shard_id = ShardId::from_agent_id(agent_id, assignment.number_of_shards);
    match assignment.epoch_of(&shard_id) {
        None => true,
        assigned => epoch_superseded(held, assigned),
    }
}
