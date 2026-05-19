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

use async_trait::async_trait;
use golem_common::model::{Pod, ShardId};
use golem_shard_manager::{
    HealthCheck, HealthCheckError, PodState, RoutingTable, RoutingTablePersistence,
    ShardManagement, ShardManagerError, WorkerExecutorService,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

#[derive(Clone, Debug)]
struct TestPersistence {
    state: Arc<Mutex<RoutingTable>>,
    writes: Arc<Mutex<Vec<RoutingTable>>>,
}

impl TestPersistence {
    fn new(initial: RoutingTable) -> Self {
        Self {
            state: Arc::new(Mutex::new(initial)),
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn latest(&self) -> RoutingTable {
        self.state.lock().await.clone()
    }
}

#[async_trait]
impl RoutingTablePersistence for TestPersistence {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError> {
        *self.state.lock().await = routing_table.clone();
        self.writes.lock().await.push(routing_table.clone());
        Ok(())
    }

    async fn read(&self) -> Result<RoutingTable, ShardManagerError> {
        Ok(self.state.lock().await.clone())
    }
}

#[derive(Clone, Debug, Default)]
struct TestWorkerExecutors {
    local_assignments: Arc<Mutex<HashMap<Pod, BTreeSet<ShardId>>>>,
}

impl TestWorkerExecutors {
    async fn set_local_assignment(&self, pod: Pod, shard_ids: &[i64]) {
        self.local_assignments
            .lock()
            .await
            .insert(pod, shard_ids.iter().copied().map(ShardId::new).collect());
    }

    async fn local_assignment(&self, pod: Pod) -> BTreeSet<ShardId> {
        self.local_assignments
            .lock()
            .await
            .get(&pod)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl WorkerExecutorService for TestWorkerExecutors {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        self.local_assignments
            .lock()
            .await
            .entry(*pod)
            .or_default()
            .extend(shard_ids.iter().copied());
        Ok(())
    }

    async fn health_check(&self, _pod: &Pod) -> Result<(), HealthCheckError> {
        Ok(())
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        if let Some(local_assignment) = self.local_assignments.lock().await.get_mut(pod) {
            local_assignment.retain(|shard_id| !shard_ids.contains(shard_id));
        }
        Ok(())
    }

    async fn set_shard_assignment(
        &self,
        pod: &Pod,
        _number_of_shards: usize,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        self.local_assignments
            .lock()
            .await
            .insert(*pod, shard_ids.clone());
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct TestHealthCheck {
    healthy: Arc<Mutex<HashMap<Pod, bool>>>,
}

impl TestHealthCheck {
    fn all_healthy() -> Self {
        Self {
            healthy: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl HealthCheck for TestHealthCheck {
    async fn health_check(&self, pod: Pod, _pod_name: Option<String>) -> bool {
        self.healthy.lock().await.get(&pod).copied().unwrap_or(true)
    }
}

fn pod(last_octet: u8, port: u16) -> Pod {
    Pod {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
        port,
    }
}

fn routing_table_with_pods(
    number_of_shards: usize,
    pods: Vec<(Pod, &str, &[i64])>,
) -> RoutingTable {
    let mut pod_states = BTreeMap::new();
    for (pod, pod_name, shard_ids) in pods {
        pod_states.insert(
            pod,
            PodState {
                pod_name: Some(pod_name.to_string()),
                assigned_shards: shard_ids.iter().copied().map(ShardId::new).collect(),
            },
        );
    }

    RoutingTable {
        number_of_shards,
        pod_states,
    }
}

async fn new_shard_management(
    routing_table: RoutingTable,
    worker_executors: Arc<TestWorkerExecutors>,
) -> (
    ShardManagement,
    TestPersistence,
    JoinSet<anyhow::Result<()>>,
) {
    let persistence = TestPersistence::new(routing_table);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors,
        health_check,
        0.0,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    tokio::time::sleep(Duration::from_millis(50)).await;

    (shard_management, persistence, join_set)
}

#[test]
// Shard-manager startup reconciliation must be authoritative. If a live
// executor has stale extra local shards, restarting only the shard-manager
// should clear them without requiring the executor process to restart.
async fn startup_reconciliation_clears_stale_extra_shards() {
    let authoritative_pod = pod(1, 9000);
    let stale_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors.set_local_assignment(stale_pod, &[0]).await;

    let (_shard_management, _persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (authoritative_pod, "worker-executor-0", &[0]),
                (stale_pod, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    assert_eq!(
        worker_executors.local_assignment(authoritative_pod).await,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert_eq!(
        worker_executors.local_assignment(stale_pod).await,
        BTreeSet::new()
    );

    join_set.abort_all();
}

#[test]
// The "returned pod" path must replace the executor's local assignment. When
// the authoritative assignment is empty, stale local ownership should be
// cleared on re-registration.
async fn returned_pod_with_empty_authoritative_assignment_clears_stale_local_shards() {
    let existing_pod = pod(1, 9000);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(existing_pod, &[0])
        .await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (existing_pod, "worker-executor-0", &[]),
                (pod(2, 9001), "worker-executor-1", &[0]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_pod(existing_pod, Some("worker-executor-0".to_string()))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        worker_executors.local_assignment(existing_pod).await,
        BTreeSet::new()
    );

    let routing_table = persistence.latest().await;
    assert!(
        routing_table
            .pod_states
            .get(&existing_pod)
            .expect("existing pod missing")
            .assigned_shards
            .is_empty()
    );

    join_set.abort_all();
}

#[test]
// If the routing table assigns a shard to one pod, reconciliation should ensure
// no other live pod still locally owns it.
async fn routing_table_owner_assignment_clears_stale_local_owner() {
    let authoritative_pod = pod(1, 9000);
    let stale_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors.set_local_assignment(stale_pod, &[0]).await;

    let (_shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (authoritative_pod, "worker-executor-0", &[0]),
                (stale_pod, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    let routing_table = persistence.latest().await;
    assert_eq!(
        routing_table
            .pod_states
            .get(&authoritative_pod)
            .expect("authoritative pod missing")
            .assigned_shards,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert!(
        routing_table
            .pod_states
            .get(&stale_pod)
            .expect("stale pod missing")
            .assigned_shards
            .is_empty()
    );
    assert_eq!(
        worker_executors.local_assignment(authoritative_pod).await,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert_eq!(
        worker_executors.local_assignment(stale_pod).await,
        BTreeSet::new()
    );

    join_set.abort_all();
}
