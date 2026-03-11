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

use crate::ShardManagerTestDependencies;
use async_trait::async_trait;
use golem_api_grpc::proto::golem;
use golem_common::config::RedisConfig;
use golem_common::model::ShardId;
use golem_common::redis::RedisPool;
use golem_shard_manager::{Pod, RoutingTable, RoutingTablePersistence, RoutingTableRedisPersistence};
use golem_test_framework::components::redis::Redis;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use uuid::Uuid;

#[async_trait]
trait GetRoutingTablePersistence: std::fmt::Debug {
    async fn get_persistence(&self) -> Arc<dyn RoutingTablePersistence + Send + Sync>;
}

struct RedisRoutingTablePersistence {
    redis: Arc<dyn Redis + Send + Sync>,
}

impl std::fmt::Debug for RedisRoutingTablePersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedisRoutingTablePersistence")
    }
}

#[async_trait]
impl GetRoutingTablePersistence for RedisRoutingTablePersistence {
    async fn get_persistence(&self) -> Arc<dyn RoutingTablePersistence + Send + Sync> {
        let redis_pool = RedisPool::configured(&RedisConfig {
            host: self.redis.public_host(),
            port: self.redis.public_port(),
            database: 0,
            tracing: false,
            pool_size: 1,
            retries: Default::default(),
            key_prefix: format!("shard-manager-persistence-test:{}:", Uuid::new_v4()),
            username: None,
            password: None,
            tls: false,
        })
        .await
        .expect("Failed to create Redis pool for persistence tests");

        Arc::new(RoutingTableRedisPersistence::new(&redis_pool, 16))
    }
}

#[test_dep(tagged_as = "redis")]
async fn redis_persistence(
    deps: &ShardManagerTestDependencies,
) -> Arc<dyn GetRoutingTablePersistence + Send + Sync> {
    deps.redis.assert_valid();
    Arc::new(RedisRoutingTablePersistence {
        redis: deps.redis.clone(),
    })
}

inherit_test_dep!(ShardManagerTestDependencies);

define_matrix_dimension!(persistence: Arc<dyn GetRoutingTablePersistence + Send + Sync> -> "redis");

#[test]
#[tracing::instrument]
async fn read_returns_default_when_empty(
    _deps: &ShardManagerTestDependencies,
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence + Send + Sync>,
) {
    let persistence = persistence.get_persistence().await;
    let routing_table = persistence
        .read()
        .await
        .expect("Reading default routing table should succeed");

    assert_eq!(routing_table.number_of_shards, 16);
    assert!(routing_table.shard_assignments.is_empty());
}

#[test]
#[tracing::instrument]
async fn write_then_read_roundtrip(
    _deps: &ShardManagerTestDependencies,
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence + Send + Sync>,
) {
    let persistence = persistence.get_persistence().await;
    let expected = sample_routing_table(16);

    persistence
        .write(&expected)
        .await
        .expect("Writing routing table should succeed");

    let actual = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");

    assert_eq!(actual, expected);
}

#[test]
#[tracing::instrument]
async fn last_write_wins(
    _deps: &ShardManagerTestDependencies,
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence + Send + Sync>,
) {
    let persistence = persistence.get_persistence().await;
    let first = sample_routing_table(16);
    let second = replacement_routing_table(16);

    persistence
        .write(&first)
        .await
        .expect("Writing first routing table should succeed");
    persistence
        .write(&second)
        .await
        .expect("Writing second routing table should succeed");

    let actual = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");

    assert_eq!(actual, second);
}

fn sample_routing_table(number_of_shards: usize) -> RoutingTable {
    let mut assignments: BTreeMap<Pod, BTreeSet<ShardId>> = BTreeMap::new();
    assignments.insert(
        pod("pod-a", 9010, Ipv4Addr::new(10, 0, 0, 1)),
        BTreeSet::from([ShardId::new(0), ShardId::new(1), ShardId::new(2)]),
    );
    assignments.insert(
        pod("pod-b", 9011, Ipv4Addr::new(10, 0, 0, 2)),
        BTreeSet::from([ShardId::new(3), ShardId::new(4)]),
    );

    RoutingTable {
        number_of_shards,
        shard_assignments: assignments,
    }
}

fn replacement_routing_table(number_of_shards: usize) -> RoutingTable {
    let mut assignments: BTreeMap<Pod, BTreeSet<ShardId>> = BTreeMap::new();
    assignments.insert(
        pod("pod-c", 9012, Ipv4Addr::new(10, 0, 0, 3)),
        BTreeSet::from([ShardId::new(5), ShardId::new(6), ShardId::new(7)]),
    );

    RoutingTable {
        number_of_shards,
        shard_assignments: assignments,
    }
}

fn pod(host: &str, port: u16, ip: Ipv4Addr) -> Pod {
    Pod::from_register_request(
        IpAddr::V4(ip),
        golem::shardmanager::v1::RegisterRequest {
            host: host.to_string(),
            port: port as i32,
            pod_name: Some(host.to_string()),
        },
    )
    .expect("Pod fixture should be valid")
}
