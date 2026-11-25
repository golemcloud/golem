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

use super::super::model::RegisterAck;
use super::{
    HealthCheckError, Pod, ShardAssignmentPush, ShardId, ShardLeaseRevision, ShardManagerError,
    WorkerExecutorError, WorkerExecutorService, invalid_delivery, revoke_request,
};
use async_trait::async_trait;
use golem_common::model::protobuf::{shard_assignment_from_proto, shard_revocation_from_proto};
use golem_common::model::sharding::{
    ShardAssignmentEvent, ShardAssignmentState, transition_shard_assignment,
};
use golem_common::model::{ShardAssignment, ShardDeliveryOutcome};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;
use std::time::Instant;

/// The result of a health check, independent of shard readiness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InMemoryWorkerExecutorHealth {
    #[default]
    Serving,
    NotServing,
    /// Returns the health timeout error immediately. It does not simulate elapsed time.
    Timeout,
}

/// Persistent dependency conditions. Defaults to serving with an available recovery index.
#[derive(Debug, Default)]
pub struct InMemoryWorkerExecutorConditions {
    pub health: InMemoryWorkerExecutorHealth,
    /// Raw key-value storage error detail, without the worker runtime error prefix.
    pub recovery_index_error: Option<Box<str>>,
}

struct MemoryExecutor {
    assignment: ShardAssignmentState,
    conditions: InMemoryWorkerExecutorConditions,
}

/// In-process executors with the same assignment validation and revision gate as remote
/// executors. Each supplied pod starts reachable and healthy. Its initial state can be absent,
/// standalone, or registered. An omitted pod is unreachable. Assignment and revoke leave the
/// lease expiry unchanged. Registration adopts the supplied local expiry.
pub struct InMemoryWorkerExecutorService {
    assignments: RwLock<BTreeMap<Pod, MemoryExecutor>>,
}

impl InMemoryWorkerExecutorService {
    pub fn new(executors: impl IntoIterator<Item = (Pod, ShardAssignmentState)>) -> Self {
        Self {
            assignments: RwLock::new(
                executors
                    .into_iter()
                    .map(|(pod, assignment)| {
                        (
                            pod,
                            MemoryExecutor {
                                assignment,
                                conditions: InMemoryWorkerExecutorConditions::default(),
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }

    /// Replaces the conditions until the next replacement. Keeps the assignment and its expiry.
    /// An omitted pod returns unavailable and stays omitted.
    pub fn set_conditions(
        &self,
        pod: &Pod,
        conditions: InMemoryWorkerExecutorConditions,
    ) -> Result<(), ShardManagerError> {
        let mut assignments = self.assignments.write().unwrap();
        let executor = assignments.get_mut(pod).ok_or_else(|| {
            ShardManagerError::GrpcError(tonic::Status::unavailable("Executor is not reachable"))
        })?;
        executor.conditions = conditions;
        Ok(())
    }

    /// Returns a copy of the installed assignment. Returns `None` if the pod was omitted
    /// or has no installed assignment.
    pub fn current_assignment(&self, pod: &Pod) -> Option<ShardAssignment> {
        self.assignments
            .read()
            .unwrap()
            .get(pod)
            .and_then(|executor| executor.assignment.as_ref().cloned())
    }

    /// Installs a registration with this manager process's incarnation and the supplied local
    /// expiry. A stale registration changes only the expiry. An omitted pod stays unreachable.
    pub fn install_registration(
        &self,
        pod: &Pod,
        registration: &RegisterAck,
        expires_at: Instant,
    ) -> Result<(), ShardManagerError> {
        let assignment = ShardAssignmentPush {
            number_of_shards: registration.number_of_shards,
            shard_epochs: registration.grant.shard_epochs.clone(),
            executor_count: registration.grant.executor_count,
            revision: registration.grant.revision,
        };
        let request = assignment
            .to_proto(super::super::model::incarnation_id())
            .map_err(invalid_delivery)?;
        let decoded = shard_assignment_from_proto(request).map_err(invalid_delivery)?;
        self.apply(
            pod,
            ShardAssignmentEvent::Register {
                number_of_shards: decoded.number_of_shards,
                shard_epochs: &decoded.shard_epochs,
                revision: decoded.revision,
                executor_count: decoded.executor_count,
                expires_at: Some(expires_at),
            },
        )
    }

    fn apply(&self, pod: &Pod, event: ShardAssignmentEvent<'_>) -> Result<(), ShardManagerError> {
        let mut assignments = self.assignments.write().unwrap();
        let executor = assignments.get_mut(pod).ok_or_else(|| {
            ShardManagerError::GrpcError(tonic::Status::unavailable("Executor is not reachable"))
        })?;
        let state = &mut executor.assignment;
        let (next, outcome) = transition_shard_assignment(std::mem::take(state), event);
        *state = next;
        delivery_result(
            outcome,
            state.as_ref(),
            event,
            executor.conditions.recovery_index_error.as_deref(),
            Instant::now(),
        )
        .map_err(ShardManagerError::WorkerExecutionError)
    }
}

#[async_trait]
impl WorkerExecutorService for InMemoryWorkerExecutorService {
    async fn assign_shards(
        &self,
        pod: &Pod,
        assignment: &ShardAssignmentPush,
    ) -> Result<(), ShardManagerError> {
        let request = assignment
            .to_proto(super::super::model::incarnation_id())
            .map_err(invalid_delivery)?;
        let decoded = shard_assignment_from_proto(request).map_err(invalid_delivery)?;
        self.apply(
            pod,
            ShardAssignmentEvent::Assign {
                number_of_shards: decoded.number_of_shards,
                shard_epochs: &decoded.shard_epochs,
                revision: decoded.revision,
                executor_count: decoded.executor_count,
            },
        )
    }

    async fn health_check(&self, pod: &Pod) -> Result<(), HealthCheckError> {
        let assignments = self.assignments.read().unwrap();
        let executor = assignments.get(pod).ok_or_else(|| {
            HealthCheckError::GrpcError(tonic::Status::unavailable("Executor is not reachable"))
        })?;
        health_result(executor.conditions.health)
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
        revision: ShardLeaseRevision,
        executor_count: usize,
    ) -> Result<(), ShardManagerError> {
        let request = revoke_request(
            shard_ids,
            revision,
            executor_count,
            super::super::model::incarnation_id(),
        )
        .map_err(invalid_delivery)?;
        let decoded = shard_revocation_from_proto(request).map_err(invalid_delivery)?;
        self.apply(
            pod,
            ShardAssignmentEvent::Revoke {
                shard_ids: &decoded.shard_ids,
                revision: decoded.revision,
                executor_count: decoded.executor_count,
            },
        )
    }
}

fn delivery_result(
    outcome: Option<ShardDeliveryOutcome>,
    assignment: Option<&ShardAssignment>,
    event: ShardAssignmentEvent<'_>,
    recovery_index_error: Option<&str>,
    now: Instant,
) -> Result<(), WorkerExecutorError> {
    let outcome = outcome.ok_or(WorkerExecutorError::ShardingNotReady)?;
    if matches!(event, ShardAssignmentEvent::Assign { .. })
        && matches!(outcome, ShardDeliveryOutcome::Applied { .. })
        && assignment.is_some_and(|assignment| {
            assignment.lease_is_live(now) && !assignment.shard_epochs.is_empty()
        })
        && let Some(detail) = recovery_index_error
    {
        return Err(WorkerExecutorError::runtime(format!(
            "failed to get worker ids from KV storage: {detail}"
        )));
    }
    Ok(())
}

fn health_result(health: InMemoryWorkerExecutorHealth) -> Result<(), HealthCheckError> {
    match health {
        InMemoryWorkerExecutorHealth::NotServing => Err(HealthCheckError::GrpcOther("NOT_SERVING")),
        InMemoryWorkerExecutorHealth::Timeout => {
            Err(HealthCheckError::GrpcOther("health check timeout"))
        }
        InMemoryWorkerExecutorHealth::Serving => Ok(()),
    }
}
