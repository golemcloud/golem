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
use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_common::model::{Pod, ShardId};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_shard_manager::{
    DbRoutingTablePersistence, PodState, RoutingTable, RoutingTablePersistence,
};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tempfile::TempDir;
use test_r::{define_matrix_dimension, test, test_dep};
use url::Url;
use uuid::Uuid;

#[async_trait]
trait GetRoutingTablePersistence: std::fmt::Debug + Send + Sync {
    async fn get_persistence(&self) -> Arc<dyn RoutingTablePersistence>;
}

struct PostgresRoutingTablePersistence {
    postgres: DockerPostgresRdb,
}

impl std::fmt::Debug for PostgresRoutingTablePersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PostgresRoutingTablePersistence")
    }
}

#[async_trait]
impl GetRoutingTablePersistence for PostgresRoutingTablePersistence {
    async fn get_persistence(&self) -> Arc<dyn RoutingTablePersistence> {
        let db_name = format!("shard_{}", Uuid::new_v4().simple());

        let admin_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.postgres.public_connection_string())
            .await
            .expect("Cannot create postgres admin pool");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\";"))
            .execute(&admin_pool)
            .await
            .expect("Cannot create postgres test database");

        let postgres_config = DbPostgresConfig {
            host: "localhost".to_string(),
            database: db_name,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: Url::parse(&self.postgres.public_connection_string())
                .expect("Invalid postgres connection string")
                .port()
                .expect("Postgres connection string missing port"),
            max_connections: 10,
            schema: None,
        };

        let migrations = IncludedMigrationsDir::new(&golem_shard_manager::DB_MIGRATIONS);

        golem_service_base::db::postgres::migrate(
            &postgres_config,
            migrations.postgres_migrations(),
        )
        .await
        .expect("Cannot apply postgres migrations");

        let pool = golem_service_base::db::postgres::PostgresPool::configured(&postgres_config)
            .await
            .expect("Cannot create postgres pool");

        Arc::new(DbRoutingTablePersistence::new(pool, 16))
    }
}

struct SqliteRoutingTablePersistence {
    temp_dir: TempDir,
}

impl std::fmt::Debug for SqliteRoutingTablePersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqliteRoutingTablePersistence")
    }
}

#[async_trait]
impl GetRoutingTablePersistence for SqliteRoutingTablePersistence {
    async fn get_persistence(&self) -> Arc<dyn RoutingTablePersistence> {
        let database_file = self
            .temp_dir
            .path()
            .join(format!("shard_{}", Uuid::new_v4().simple()))
            .to_str()
            .expect("tempfile path was not valid unicode")
            .to_string();

        let sqlite_config = DbSqliteConfig {
            database: database_file,
            max_connections: 10,
            foreign_keys: true,
        };

        let migrations = IncludedMigrationsDir::new(&golem_shard_manager::DB_MIGRATIONS);

        golem_service_base::db::sqlite::migrate(&sqlite_config, migrations.sqlite_migrations())
            .await
            .expect("Cannot apply sqlite migrations");

        let pool = golem_service_base::db::sqlite::SqlitePool::configured(&sqlite_config)
            .await
            .expect("Cannot create sqlite pool");

        Arc::new(DbRoutingTablePersistence::new(pool, 16))
    }
}

#[test_dep(tagged_as = "sqlite")]
async fn sqlite_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    let temp_dir = TempDir::new().expect("Cannot create temp dir");
    Arc::new(SqliteRoutingTablePersistence { temp_dir })
}

#[test_dep(tagged_as = "postgres")]
async fn postgres_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;
    Arc::new(PostgresRoutingTablePersistence { postgres })
}

define_matrix_dimension!(persistence: Arc<dyn GetRoutingTablePersistence> -> "sqlite", "postgres");

#[test]
#[tracing::instrument]
async fn read_returns_default_when_empty(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let persistence = persistence.get_persistence().await;
    let routing_table = persistence
        .read()
        .await
        .expect("Reading default routing table should succeed");

    assert_eq!(routing_table.number_of_shards, 16);
    assert!(routing_table.pod_states.is_empty());
}

#[test]
#[tracing::instrument]
async fn write_then_read_roundtrip(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
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
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
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
    let mut pod_states = BTreeMap::new();
    pod_states.insert(
        Pod {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            port: 9010,
        },
        PodState {
            pod_name: None,
            assigned_shards: BTreeSet::from([ShardId::new(0), ShardId::new(1), ShardId::new(2)]),
        },
    );
    pod_states.insert(
        Pod {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            port: 9011,
        },
        PodState {
            pod_name: None,
            assigned_shards: BTreeSet::from([ShardId::new(3), ShardId::new(4)]),
        },
    );

    RoutingTable {
        number_of_shards,
        pod_states,
    }
}

fn replacement_routing_table(number_of_shards: usize) -> RoutingTable {
    let mut pod_states = BTreeMap::new();
    pod_states.insert(
        Pod {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
            port: 9012,
        },
        PodState {
            pod_name: None,
            assigned_shards: BTreeSet::from([ShardId::new(5), ShardId::new(6), ShardId::new(7)]),
        },
    );

    RoutingTable {
        number_of_shards,
        pod_states,
    }
}
