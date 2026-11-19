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

use crate::WorkerExecutorTestDependencies;
use crate::persistent_storage::PersistentStorage;
use futures::StreamExt;
use golem_common::config::RedisConfig;
use golem_common::model::AgentFingerprint;
use golem_common::model::environment::EnvironmentId;
use golem_common::redis::RedisPool;
use golem_worker_executor::storage::keyvalue::namespace_routed::NamespaceRoutedKeyValueStorage;
use golem_worker_executor::storage::keyvalue::redis::RedisKeyValueStorage;
use golem_worker_executor::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use golem_worker_executor_test_utils::{LastUniqueId, TestContext};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_r::{define_matrix_dimension, inherit_test_dep, test};
use uuid::{Uuid, uuid};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(#[tagged_as("durable_sqlite")] Arc<PersistentStorage>);
inherit_test_dep!(#[tagged_as("durable_postgres")] Arc<PersistentStorage>);
define_matrix_dimension!(durable_storage: Arc<PersistentStorage> -> "durable_sqlite", "durable_postgres");

async fn build_namespace_routed_kvs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    durable_storage: &PersistentStorage,
) -> (
    NamespaceRoutedKeyValueStorage,
    std::sync::Arc<RedisKeyValueStorage>,
    Arc<dyn KeyValueStorage + Send + Sync>,
) {
    let random_prefix = Uuid::new_v4();
    let redis_pool = RedisPool::configured(&RedisConfig {
        host: deps.redis.public_host(),
        port: deps.redis.public_port(),
        database: 0,
        tracing: false,
        pool_size: 1,
        retries: Default::default(),
        key_prefix: random_prefix.to_string(),
        username: None,
        password: None,
        tls: false,
    })
    .await
    .expect("Cannot create redis pool for routed kvs tests");

    let redis_storage = std::sync::Arc::new(RedisKeyValueStorage::new(redis_pool.clone()));

    let persistent = durable_storage.key_value(deps, context).await.unwrap();
    let kvs = NamespaceRoutedKeyValueStorage::new(redis_storage.clone(), persistent.clone());
    (kvs, redis_storage, persistent)
}

#[test]
async fn routes_agent_namespaces_to_redis(
    deps: &WorkerExecutorTestDependencies,
    last_unique_id: &LastUniqueId,
    #[dimension(durable_storage)] durable_storage: &Arc<PersistentStorage>,
) {
    let context = TestContext::new(last_unique_id);
    let (kvs, redis, persistent) =
        build_namespace_routed_kvs(deps, &context, durable_storage).await;

    let agent_id = crate::fresh_owned_agent_id();
    let cases = [
        (
            KeyValueStorageNamespace::Worker {
                owned_agent_id: agent_id.clone(),
            },
            "worker-route-key",
            b"worker-route-value".as_slice(),
        ),
        (
            KeyValueStorageNamespace::AgentInvocationResultIndex {
                owned_agent_id: agent_id,
                fingerprint: AgentFingerprint(uuid::Uuid::from_u128(1)),
            },
            "result-index-route-key",
            b"result-index-route-value".as_slice(),
        ),
    ];

    let (kvs, redis, persistent) = (&kvs, &redis, &persistent);
    futures::stream::iter(cases)
        .for_each(|(namespace, key, value)| async move {
            kvs.set("test", "api", "entity", namespace.clone(), key, value)
                .await
                .unwrap();

            let redis_read = redis
                .get("test", "api", "entity", namespace.clone(), key)
                .await
                .unwrap();
            let persistent_read = persistent
                .get("test", "api", "entity", namespace, key)
                .await
                .unwrap();

            assert_eq!(redis_read, Some(value.into()));
            assert_eq!(persistent_read, None);
        })
        .await;
}

#[test]
async fn routes_non_worker_namespace_to_persistent_storage(
    deps: &WorkerExecutorTestDependencies,
    last_unique_id: &LastUniqueId,
    #[dimension(durable_storage)] durable_storage: &Arc<PersistentStorage>,
) {
    let context = TestContext::new(last_unique_id);
    let (kvs, redis, persistent) =
        build_namespace_routed_kvs(deps, &context, durable_storage).await;

    let ns = KeyValueStorageNamespace::UserDefined {
        environment_id: EnvironmentId(uuid!("2ae7a48f-84fc-4951-b9ec-87d09fcb0fa4")),
        bucket: "route-test-bucket".into(),
    };
    let key = "user-route-key";
    let value = b"user-route-value";

    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();

    let persistent_read = persistent
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    let redis_read = redis.get("test", "api", "entity", ns, key).await.unwrap();

    assert_eq!(persistent_read, Some(value.as_slice().into()));
    assert_eq!(redis_read, None);
}
