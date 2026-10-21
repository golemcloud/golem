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

use super::{
    AgentId, HasActiveAgents, HasAll, HasOplog, HasShardManagerService, HasShardService, HashMap,
    RecoveryOutcome, RetirementReason, ShardDeliveryOutcome, ShardEpoch, ShardId,
    ShardLeaseRevision, UsesAllDeps, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, golem,
    retired_by_assignment, shard_epochs_from_proto,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn revoke_shards_internal(
        &self,
        request: golem::workerexecutor::v1::RevokeShardsRequest,
    ) -> Result<(), WorkerExecutorError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();
        let revision = ShardLeaseRevision::from_wire(&request.incarnation_id, request.revision)
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!("RevokeShardsRequest.{error}"))
            })?;

        match self.shard_service().revoke_shards(&shard_ids, revision)? {
            ShardDeliveryOutcome::Applied { .. } => {}
            ShardDeliveryOutcome::Stale { delivered, applied } => {
                // A newer delivery has already been applied and its set is the
                // authority; taking shards out of it would be acting on stale news.
                tracing::warn!(
                    %delivered,
                    %applied,
                    "Ignoring a RevokeShards older than the last delivery applied"
                );
                return Ok(());
            }
            ShardDeliveryOutcome::FromAnotherManager { delivered, applied } => {
                self.renew_after_a_push_from_another_manager("RevokeShards", delivered, applied);
                return Ok(());
            }
        }

        // Given up, not restarted: a restart in place would reopen each agent's oplog with the
        // epoch this executor no longer holds. They are dropped from here and recovered by the
        // shards' new owners.
        let shard_service = self.shard_service();
        self.active_agents()
            .give_up_matching(
                |agent_id| shard_service.check_worker(agent_id).is_err(),
                RetirementReason::ShardRevoked,
            )
            .await;

        Ok(())
    }

    /// Full replace: the request carries this executor's complete shard set
    /// with epochs and the cluster's shard count. Anything absent from the
    /// set is dropped, and any agent whose shard went away is given up.
    pub(super) async fn assign_shards_internal(
        &self,
        request: golem::workerexecutor::v1::AssignShardsRequest,
    ) -> Result<(), WorkerExecutorError> {
        let shard_epochs: HashMap<ShardId, ShardEpoch> =
            shard_epochs_from_proto(request.shard_epochs)
                .map_err(WorkerExecutorError::invalid_request)?;

        let number_of_shards: usize = request
            .number_of_shards
            .try_into()
            .map_err(|_| WorkerExecutorError::invalid_request("Invalid number of shards"))?;

        // `ShardId::from_agent_id` divides by it.
        if number_of_shards == 0 {
            return Err(WorkerExecutorError::invalid_request(
                "AssignShardsRequest.number_of_shards must not be 0",
            ));
        }

        let revision = ShardLeaseRevision::from_wire(&request.incarnation_id, request.revision)
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!("AssignShardsRequest.{error}"))
            })?;
        match self
            .shard_service()
            .assign_shards(number_of_shards, &shard_epochs, revision)?
        {
            ShardDeliveryOutcome::Applied { .. } => {}
            ShardDeliveryOutcome::Stale { delivered, applied } => {
                // Crossed on the network with a newer delivery, which has already
                // been applied; applying this one would put the older set back.
                tracing::warn!(
                    %delivered,
                    %applied,
                    "Ignoring an AssignShards push older than the last delivery applied"
                );
                return Ok(());
            }
            ShardDeliveryOutcome::FromAnotherManager { delivered, applied } => {
                self.renew_after_a_push_from_another_manager("AssignShards", delivered, applied);
                return Ok(());
            }
        }

        Self::apply_shard_assignment_effects(self).await?;
        Ok(())
    }

    /// A push from a shard manager process this executor does not follow is either a deposed
    /// manager still sending, or a new one this executor has not heard a reply from yet. The
    /// push cannot say which, so it is ignored either way and the renewal asks: its answer names
    /// the process in charge and carries that process's set.
    fn renew_after_a_push_from_another_manager(
        &self,
        push: &'static str,
        delivered: ShardLeaseRevision,
        applied: ShardLeaseRevision,
    ) {
        tracing::warn!(
            push,
            %delivered,
            %applied,
            "Ignoring a push from a shard manager process other than the one followed; renewing the lease to hear from the one in charge"
        );
        self.shard_manager_service().renew_now();
    }

    /// The one receipt path for a delivered shard set, whichever way it came:
    /// a registration, an `AssignShards` push, or a renewal reply that
    /// corrected the set. Sweeps the agents whose shard went away or came back
    /// at a higher epoch, then hands
    /// the executor the new set to recover agents for. The sweep runs for
    /// every path, because a renewal can narrow the set as well as widen it:
    /// a path without it would leave agents running on shards this executor
    /// no longer owns. The recovery runs only under a live lease: a fenced
    /// executor starts nothing, so a push that lands after the local lease
    /// lapsed - the first one after a manager outage does - reports the
    /// recovery deferred, and the grant that revives the lease runs it.
    ///
    /// The owed-recovery ticket is taken here, before anything runs, and handed
    /// back only after a recovery ran, so the startup registration and the push
    /// cannot disagree about it and a grant that revives the lease meanwhile
    /// cannot find nothing owed. An error keeps it owed, which is what a retry
    /// needs, and an attempt that completes after a newer delivery deferred its
    /// own does not retire that one. One run more than needed is idempotent.
    pub(crate) async fn apply_shard_assignment_effects<T>(
        this: &T,
    ) -> Result<RecoveryOutcome, anyhow::Error>
    where
        T: HasAll<Ctx> + Send + Sync + 'static,
    {
        let ticket = this.shard_manager_service().recovery_deferred();
        // Membership and epochs, never the lease: a lapsed lease must not give up every running
        // agent - a lapsed lease refuses new work and leaves running work alone.
        //
        // Given up rather than restarted: a narrowing delivery means these shards have another
        // owner now, and a restart in place would reopen their oplogs at the stale epoch. A
        // delivery that raises the epoch of a shard this executor kept means the shard left and
        // came back, so another executor may have written to its agents. Those are given up the
        // same way, and the recovery below or their next invocation reopens them at the new epoch.
        //
        // The epochs come from one snapshot and the assignment from one read, both taken just
        // before the sweep selects. An agent created after the snapshot read its epoch from the
        // delivered assignment, so only membership applies to it. An agent given up and reopened
        // at the new epoch between the snapshot and the selection is given up once more, which
        // the same reopen repairs.
        let held_epochs: HashMap<AgentId, Option<ShardEpoch>> = this
            .active_agents()
            .snapshot()
            .await
            .into_iter()
            .map(|(agent_id, worker)| (agent_id, worker.oplog().shard_epoch()))
            .collect();
        let assignment = this.shard_service().try_get_current_assignment();
        this.active_agents()
            .give_up_matching(
                |agent_id| {
                    retired_by_assignment(
                        assignment.as_ref(),
                        agent_id,
                        held_epochs.get(agent_id).copied().flatten(),
                    )
                },
                RetirementReason::ShardNotAssigned,
            )
            .await;

        if !this.shard_service().is_ready() {
            tracing::info!(
                "Shard lease has lapsed; agents for the delivered set are recovered once it is renewed"
            );
            return Ok(RecoveryOutcome::DeferredUntilLeaseIsLive);
        }
        Ctx::on_shard_assignment_changed(this).await?;
        this.shard_manager_service().recovery_succeeded(ticket);
        Ok(RecoveryOutcome::Recovered)
    }
}
