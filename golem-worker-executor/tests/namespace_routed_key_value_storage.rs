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
use golem_common::config::{DbPostgresConfig, RedisConfig};
use golem_common::model::AgentId;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::redis::RedisPool;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_executor::services::golem_config::KeyValueStoragePostgresConfig;
use golem_worker_executor::storage::keyvalue::namespace_routed::NamespaceRoutedKeyValueStorage;
use golem_worker_executor::storage::keyvalue::postgres::PostgresKeyValueStorage;
use golem_worker_executor::storage::keyvalue::redis::RedisKeyValueStorage;
use golem_worker_executor::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};
use url::Url;
use uuid::{Uuid, uuid};

inherit_test_dep!(WorkerExecutorTestDependencies);

async fn build_namespace_routed_kvs(
    deps: &WorkerExecutorTestDependencies,
) -> (
    NamespaceRoutedKeyValueStorage,
    std::sync::Arc<RedisKeyValueStorage>,
    std::sync::Arc<PostgresKeyValueStorage>,
    DockerPostgresRdb,
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

    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;
    let db_name = format!("kv_routed_test_{}", Uuid::new_v4().simple());
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&postgres.public_connection_string())
        .await
        .expect("Cannot create postgres admin pool");
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\";"))
        .execute(&admin_pool)
        .await
        .expect("Cannot create postgres database for routed kvs tests");

    let postgres_config = KeyValueStoragePostgresConfig {
        postgres: DbPostgresConfig {
            host: "localhost".to_string(),
            database: db_name,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: Url::parse(&postgres.public_connection_string())
                .expect("Invalid postgres connection string")
                .port()
                .expect("Postgres connection string missing port"),
            max_connections: 10,
            schema: None,
        },
    };

    let postgres_storage = std::sync::Arc::new(
        PostgresKeyValueStorage::configured(&postgres_config)
            .await
            .expect("Cannot create postgres key value storage for routed kvs tests"),
    );

    let kvs = NamespaceRoutedKeyValueStorage::new(redis_storage.clone(), postgres_storage.clone());

    (kvs, redis_storage, postgres_storage, postgres)
}

#[test]
async fn routes_worker_namespace_to_redis(deps: &WorkerExecutorTestDependencies) {
    let (kvs, redis, postgres, _postgres_container) = build_namespace_routed_kvs(deps).await;

    let ns = KeyValueStorageNamespace::Worker {
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "route-test-agent".to_string(),
        },
    };
    let key = "worker-route-key";
    let value = b"worker-route-value";

    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();

    let redis_read = redis
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    let postgres_read = postgres
        .get("test", "api", "entity", ns, key)
        .await
        .unwrap();

    assert_eq!(redis_read, Some(value.as_slice().into()));
    assert_eq!(postgres_read, None);
}

#[test]
async fn routes_non_worker_namespace_to_postgres(deps: &WorkerExecutorTestDependencies) {
    let (kvs, redis, postgres, _postgres_container) = build_namespace_routed_kvs(deps).await;

    let ns = KeyValueStorageNamespace::UserDefined {
        environment_id: EnvironmentId(uuid!("2ae7a48f-84fc-4951-b9ec-87d09fcb0fa4")),
        bucket: "route-test-bucket".to_string(),
    };
    let key = "user-route-key";
    let value = b"user-route-value";

    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();

    let postgres_read = postgres
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    let redis_read = redis.get("test", "api", "entity", ns, key).await.unwrap();

    assert_eq!(postgres_read, Some(value.as_slice().into()));
    assert_eq!(redis_read, None);
}
