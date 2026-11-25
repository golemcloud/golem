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
    RecoveryOutcome, RetirementReason, ShardDeliveryOutcome, ShardEpoch, ShardLeaseRevision,
    UsesAllDeps, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, golem, retired_by_assignment,
};
use crate::services::shard::ShardService;
use golem_common::model::protobuf::{shard_assignment_from_proto, shard_revocation_from_proto};

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

        // Select agents outside the current assignment. A later assignment can prevent
        // selection. A selected agent is retired, not restarted with its former shard epoch.
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
    /// scheduling or completing recovery. Successful recovery, including
    /// recovery from a push, can clear the debt. An accepted grant can
    /// schedule recovery that is still owed.
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
    let decoded =
        shard_assignment_from_proto(request).map_err(WorkerExecutorError::invalid_request)?;
    service.assign_shards(
        decoded.number_of_shards,
        &decoded.shard_epochs,
        decoded.revision,
        decoded.executor_count,
    )
}

fn apply_revoke_request(
    service: &dyn ShardService,
    request: golem::workerexecutor::v1::RevokeShardsRequest,
) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
    let decoded =
        shard_revocation_from_proto(request).map_err(WorkerExecutorError::invalid_request)?;
    service.revoke_shards(&decoded.shard_ids, decoded.revision, decoded.executor_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::shard::ShardServiceDefault;
    use golem_common::model::ShardId;
    use golem_common::model::protobuf::shard_epochs_to_proto;
    use prost::Message;
    use std::num::NonZeroUsize;
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

        let pushed_epochs = HashMap::from([
            (ShardId::new(0), ShardEpoch(9)),
            (ShardId::new(2), ShardEpoch(11)),
        ]);
        let request = golem::workerexecutor::v1::AssignShardsRequest {
            number_of_shards: 16,
            shard_epochs: shard_epochs_to_proto(
                pushed_epochs.iter().map(|(id, epoch)| (*id, *epoch)),
            ),
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

        let mut expected = golem_common::model::ShardAssignment {
            number_of_shards: 16,
            shard_epochs: pushed_epochs,
            revision: Some(ShardLeaseRevision {
                number: 3,
                ..revision
            }),
            executor_count: 5,
            expires_at: Some(expiry),
        };
        assert_assignment(&service, &expected);
        let renewed_expiry = expiry + Duration::from_secs(20);
        service
            .update_lease(
                &epochs,
                renewed_expiry,
                ShardLeaseRevision {
                    number: 2,
                    ..revision
                },
                NonZeroUsize::new(9).unwrap(),
            )
            .unwrap();
        expected.expires_at = Some(renewed_expiry);
        assert_assignment(&service, &expected);

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
        expected.shard_epochs.remove(&ShardId::new(0));
        expected.revision.as_mut().unwrap().number = 4;
        expected.executor_count = 3;
        assert_assignment(&service, &expected);

        let shorter_expiry = expiry + Duration::from_secs(5);
        service
            .update_lease(
                &epochs,
                shorter_expiry,
                ShardLeaseRevision {
                    number: 3,
                    ..revision
                },
                NonZeroUsize::new(8).unwrap(),
            )
            .unwrap();
        expected.expires_at = Some(shorter_expiry);
        assert_assignment(&service, &expected);
    }

    fn assert_assignment(
        service: &ShardServiceDefault,
        expected: &golem_common::model::ShardAssignment,
    ) {
        let held = service.current_assignment().unwrap();
        assert_eq!(held.number_of_shards, expected.number_of_shards);
        assert_eq!(held.executor_count, expected.executor_count);
        assert_eq!(held.shard_epochs, expected.shard_epochs);
        assert_eq!(held.revision, expected.revision);
        assert_eq!(held.expires_at, expected.expires_at);
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
        let expected = service.current_assignment().unwrap();
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
                ..assign.clone()
            },
            golem::workerexecutor::v1::AssignShardsRequest {
                shard_epochs: vec![golem::shardmanager::ShardEpochEntry {
                    shard_id: None,
                    epoch: 9,
                }],
                ..assign.clone()
            },
            golem::workerexecutor::v1::AssignShardsRequest {
                incarnation_id: "invalid".to_string(),
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
            assert_assignment(&service, &expected);
        });
        let revoke = golem::workerexecutor::v1::RevokeShardsRequest {
            shard_ids: vec![ShardId::new(0).into()],
            revision: 4,
            incarnation_id: revision.incarnation.to_string(),
            executor_count: 0,
        };
        [
            revoke.clone(),
            golem::workerexecutor::v1::RevokeShardsRequest {
                executor_count: 5,
                incarnation_id: String::new(),
                ..revoke.clone()
            },
            golem::workerexecutor::v1::RevokeShardsRequest {
                executor_count: 5,
                incarnation_id: "invalid".to_string(),
                ..revoke
            },
        ]
        .into_iter()
        .for_each(|request| {
            let bytes = request.encode_to_vec();
            assert!(
                apply_revoke_request(
                    &service,
                    golem::workerexecutor::v1::RevokeShardsRequest::decode(bytes.as_slice())
                        .unwrap(),
                )
                .is_err()
            );
            assert_assignment(&service, &expected);
        });
    }
}
