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
use crate::services::shard::ShardService;
use std::num::NonZeroUsize;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn revoke_shards_internal(
        &self,
        request: golem::workerexecutor::v1::RevokeShardsRequest,
    ) -> Result<(), WorkerExecutorError> {
        match apply_revoke_request(self.shard_service().as_ref(), request)? {
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
        match apply_assignment_request(self.shard_service().as_ref(), request)? {
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
    /// recovery deferred. An accepted registration or renewal runs it once
    /// the lease is live. A stale renewal can extend the lease without
    /// scheduling recovery; the recovery stays owed until an accepted grant.
    ///
    /// The owed-recovery ticket is taken here, before anything runs, and handed
    /// back only after a recovery ran, so the startup registration and the push
    /// cannot disagree about it and an accepted grant that revives the lease meanwhile
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

fn apply_assignment_request(
    service: &dyn ShardService,
    request: golem::workerexecutor::v1::AssignShardsRequest,
) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
    let shard_epochs: HashMap<ShardId, ShardEpoch> = shard_epochs_from_proto(request.shard_epochs)
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
    let executor_count = NonZeroUsize::new(request.executor_count as usize).ok_or_else(|| {
        WorkerExecutorError::invalid_request("AssignShardsRequest.executor_count must not be 0")
    })?;
    let revision = ShardLeaseRevision::from_wire(&request.incarnation_id, request.revision)
        .map_err(|error| {
            WorkerExecutorError::invalid_request(format!("AssignShardsRequest.{error}"))
        })?;
    service.assign_shards(number_of_shards, &shard_epochs, revision, executor_count)
}

fn apply_revoke_request(
    service: &dyn ShardService,
    request: golem::workerexecutor::v1::RevokeShardsRequest,
) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
    let executor_count = NonZeroUsize::new(request.executor_count as usize).ok_or_else(|| {
        WorkerExecutorError::invalid_request("RevokeShardsRequest.executor_count must not be 0")
    })?;
    let shard_ids = request.shard_ids.into_iter().map(ShardId::from).collect();
    let revision = ShardLeaseRevision::from_wire(&request.incarnation_id, request.revision)
        .map_err(|error| {
            WorkerExecutorError::invalid_request(format!("RevokeShardsRequest.{error}"))
        })?;
    service.revoke_shards(&shard_ids, revision, executor_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::shard::ShardServiceDefault;
    use golem_common::model::protobuf::shard_epochs_to_proto;
    use prost::Message;
    use std::time::{Duration, Instant};
    use test_r::test;
    use uuid::Uuid;

    #[test]
    fn wire_deliveries_keep_the_count_at_the_newest_revision() {
        let service = ShardServiceDefault::new();
        let revision = ShardLeaseRevision {
            incarnation: Uuid::from_u128(0x5eed),
            number: 1,
        };
        let expiry = Instant::now() + Duration::from_secs(60);
        let epochs = HashMap::from([(ShardId::new(0), ShardEpoch(7))]);
        service.register(
            8,
            &epochs,
            Some(expiry),
            revision,
            NonZeroUsize::new(2).unwrap(),
        );

        let request = golem::workerexecutor::v1::AssignShardsRequest {
            number_of_shards: 8,
            shard_epochs: shard_epochs_to_proto(epochs.iter().map(|(id, epoch)| (*id, *epoch))),
            revision: 3,
            incarnation_id: revision.incarnation.to_string(),
            executor_count: 5,
        };
        let bytes = request.encode_to_vec();
        apply_assignment_request(
            &service,
            golem::workerexecutor::v1::AssignShardsRequest::decode(bytes.as_slice()).unwrap(),
        )
        .unwrap();
        assert_eq!(service.executor_count(), Some(5));

        let request = golem::workerexecutor::v1::RevokeShardsRequest {
            shard_ids: vec![ShardId::new(0).into()],
            revision: 4,
            incarnation_id: revision.incarnation.to_string(),
            executor_count: 3,
        };
        let bytes = request.encode_to_vec();
        apply_revoke_request(
            &service,
            golem::workerexecutor::v1::RevokeShardsRequest::decode(bytes.as_slice()).unwrap(),
        )
        .unwrap();
        assert_eq!(service.executor_count(), Some(3));
        assert!(
            service
                .current_assignment()
                .unwrap()
                .shard_epochs
                .is_empty()
        );
        assert_eq!(
            service.current_assignment().unwrap().expires_at,
            Some(expiry)
        );

        service
            .update_lease(
                &epochs,
                expiry,
                ShardLeaseRevision {
                    number: 3,
                    ..revision
                },
                NonZeroUsize::new(5).unwrap(),
            )
            .unwrap();
        let held = service.current_assignment().unwrap();
        assert_eq!(held.executor_count, 3);
        assert_eq!(
            held.revision,
            Some(ShardLeaseRevision {
                number: 4,
                ..revision
            })
        );
        assert!(held.shard_epochs.is_empty());
    }

    #[test]
    fn malformed_wire_deliveries_leave_the_installed_assignment_unchanged() {
        let service = ShardServiceDefault::new();
        let revision = ShardLeaseRevision {
            incarnation: Uuid::from_u128(0x5eed),
            number: 1,
        };
        let expiry = Instant::now() + Duration::from_secs(60);
        let epochs = HashMap::from([(ShardId::new(0), ShardEpoch(7))]);
        service.register(
            8,
            &epochs,
            Some(expiry),
            revision,
            NonZeroUsize::new(2).unwrap(),
        );
        let assign = golem::workerexecutor::v1::AssignShardsRequest {
            number_of_shards: 8,
            shard_epochs: vec![],
            revision: 3,
            incarnation_id: revision.incarnation.to_string(),
            executor_count: 5,
        };
        [
            golem::workerexecutor::v1::AssignShardsRequest {
                executor_count: 0,
                ..assign.clone()
            },
            golem::workerexecutor::v1::AssignShardsRequest {
                number_of_shards: 0,
                ..assign.clone()
            },
            golem::workerexecutor::v1::AssignShardsRequest {
                incarnation_id: String::new(),
                ..assign
            },
        ]
        .into_iter()
        .for_each(|request| {
            let bytes = request.encode_to_vec();
            assert!(
                apply_assignment_request(
                    &service,
                    golem::workerexecutor::v1::AssignShardsRequest::decode(bytes.as_slice())
                        .unwrap(),
                )
                .is_err()
            );
        });
        let revoke = golem::workerexecutor::v1::RevokeShardsRequest {
            shard_ids: vec![ShardId::new(0).into()],
            revision: 4,
            incarnation_id: revision.incarnation.to_string(),
            executor_count: 0,
        };
        let bytes = revoke.encode_to_vec();
        assert!(
            apply_revoke_request(
                &service,
                golem::workerexecutor::v1::RevokeShardsRequest::decode(bytes.as_slice()).unwrap(),
            )
            .is_err()
        );
        let held = service.current_assignment().unwrap();
        assert_eq!(held.executor_count, 2);
        assert_eq!(held.revision, Some(revision));
        assert_eq!(held.expires_at, Some(expiry));
        assert_eq!(held.shard_epochs, epochs);
    }
}
