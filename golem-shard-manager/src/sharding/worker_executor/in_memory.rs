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
    HealthCheckError, Pod, ShardAssignmentPush, ShardId, ShardLeaseRevision, ShardManagerError,
    WorkerExecutorError, WorkerExecutorService,
};
use async_trait::async_trait;
use golem_common::model::sharding::{
    ShardAssignmentEvent, ShardAssignmentState, transition_shard_assignment,
};
use golem_common::model::{ShardAssignment, ShardDeliveryOutcome};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::RwLock;

/// In-process executors with the same assignment validation and revision gate as remote
/// executors. Each supplied pod is reachable and healthy. Its initial state can be absent,
/// standalone, or registered. Deliveries never extend its lease.
pub struct InMemoryWorkerExecutorService {
    assignments: RwLock<BTreeMap<Pod, ShardAssignmentState>>,
}

impl InMemoryWorkerExecutorService {
    pub fn new(executors: impl IntoIterator<Item = (Pod, ShardAssignmentState)>) -> Self {
        Self {
            assignments: RwLock::new(executors.into_iter().collect()),
        }
    }

    /// A copy of the accepted assignment. Unreachable and unregistered pods have no snapshot.
    pub fn current_assignment(&self, pod: &Pod) -> Option<ShardAssignment> {
        self.assignments
            .read()
            .unwrap()
            .get(pod)
            .and_then(|state| state.as_ref().cloned())
    }

    fn apply(&self, pod: &Pod, event: ShardAssignmentEvent<'_>) -> Result<(), ShardManagerError> {
        let mut assignments = self.assignments.write().unwrap();
        let state = assignments.get_mut(pod).ok_or_else(|| {
            ShardManagerError::GrpcError(tonic::Status::unavailable("Executor is not reachable"))
        })?;
        let (next, outcome) = transition_shard_assignment(std::mem::take(state), event);
        *state = next;
        delivery_result(outcome).map_err(ShardManagerError::WorkerExecutionError)
    }
}

#[async_trait]
impl WorkerExecutorService for InMemoryWorkerExecutorService {
    async fn assign_shards(
        &self,
        pod: &Pod,
        assignment: &ShardAssignmentPush,
    ) -> Result<(), ShardManagerError> {
        let (number_of_shards, executor_count) =
            assignment_counts(assignment).map_err(ShardManagerError::WorkerExecutionError)?;
        let shard_epochs: HashMap<_, _> = assignment
            .shard_epochs
            .iter()
            .map(|(shard_id, epoch)| (*shard_id, golem_common::model::ShardEpoch(epoch.0)))
            .collect();
        self.apply(
            pod,
            ShardAssignmentEvent::Assign {
                number_of_shards,
                shard_epochs: &shard_epochs,
                revision: delivery_revision(
                    &super::super::model::incarnation_id(),
                    assignment.revision,
                ),
                executor_count,
            },
        )
    }

    async fn health_check(&self, pod: &Pod) -> Result<(), HealthCheckError> {
        self.assignments
            .read()
            .unwrap()
            .contains_key(pod)
            .then_some(())
            .ok_or_else(|| {
                HealthCheckError::GrpcError(tonic::Status::unavailable("Executor is not reachable"))
            })
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
        revision: ShardLeaseRevision,
        executor_count: usize,
    ) -> Result<(), ShardManagerError> {
        let executor_count = delivered_count(executor_count, "RevokeShardsRequest")
            .map_err(ShardManagerError::WorkerExecutionError)?;
        let shard_ids: HashSet<_> = shard_ids.iter().copied().collect();
        self.apply(
            pod,
            ShardAssignmentEvent::Revoke {
                shard_ids: &shard_ids,
                revision: delivery_revision(&super::super::model::incarnation_id(), revision),
                executor_count,
            },
        )
    }
}

fn assignment_counts(
    assignment: &ShardAssignmentPush,
) -> Result<(usize, NonZeroUsize), WorkerExecutorError> {
    let number_of_shards = assignment.number_of_shards as u32 as usize;
    if number_of_shards == 0 {
        return Err(WorkerExecutorError::invalid_request(
            "AssignShardsRequest.number_of_shards must not be 0",
        ));
    }
    delivered_count(assignment.executor_count, "AssignShardsRequest")
        .map(|count| (number_of_shards, count))
}

fn delivered_count(count: usize, request: &str) -> Result<NonZeroUsize, WorkerExecutorError> {
    NonZeroUsize::new(count as u32 as usize).ok_or_else(|| {
        WorkerExecutorError::invalid_request(format!("{request}.executor_count must not be 0"))
    })
}

fn delivery_revision(
    incarnation_id: &str,
    revision: ShardLeaseRevision,
) -> golem_common::model::ShardLeaseRevision {
    golem_common::model::ShardLeaseRevision::from_wire(incarnation_id, revision.0)
        .expect("the manager incarnation is a UUID")
}

fn delivery_result(outcome: Option<ShardDeliveryOutcome>) -> Result<(), WorkerExecutorError> {
    outcome
        .map(|_| ())
        .ok_or(WorkerExecutorError::ShardingNotReady)
}
