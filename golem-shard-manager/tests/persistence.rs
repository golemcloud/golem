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
use chrono::{DateTime, Utc};
use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_common::model::ShardId;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_shard_manager::{
    DbRoutingTablePersistence, ExecutorAddr, ExecutorId, RoutingTablePersistence,
    ShardLeaseRevision, ShardLeaseState,
};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use test_r::{define_matrix_dimension, test, test_dep};
use url::Url;
use uuid::Uuid;

const LEASE_TTL: Duration = Duration::from_secs(60);

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

        Arc::new(DbRoutingTablePersistence::new(pool, 16, LEASE_TTL))
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

        Arc::new(DbRoutingTablePersistence::new(pool, 16, LEASE_TTL))
    }
}

#[test_dep(scope = Shared, tagged_as = "sqlite")]
async fn sqlite_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    let temp_dir = TempDir::new().expect("Cannot create temp dir");
    Arc::new(SqliteRoutingTablePersistence { temp_dir })
}

#[test_dep(scope = Shared, tagged_as = "postgres")]
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
    let shard_state = persistence
        .read()
        .await
        .expect("Reading default shard lease state should succeed");

    assert_eq!(shard_state.number_of_shards, 16);
    assert_eq!(shard_state.revision, ShardLeaseRevision::INITIAL);
    assert!(shard_state.shard_assignments.is_empty());
    assert!(shard_state.executor_leases.is_empty());
    assert!(shard_state.pending_rebalance.is_empty());
}

#[test]
#[tracing::instrument]
async fn write_then_read_roundtrip(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let persistence = persistence.get_persistence().await;
    let expected = sample_shard_state(16);

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
    let first = sample_shard_state(16);
    let second = replacement_shard_state(16);

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

fn granted_at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
}

fn executor(idx: u128) -> ExecutorId {
    ExecutorId(Uuid::from_u128(idx))
}

fn addr(last_octet: u8, port: u16) -> ExecutorAddr {
    ExecutorAddr {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
        port,
    }
}

fn shard_state_with_executors(
    number_of_shards: usize,
    executors: &[(ExecutorId, ExecutorAddr, Option<&str>, &[i64])],
) -> ShardLeaseState {
    let mut shard_state = ShardLeaseState::new(number_of_shards);
    for (executor_id, addr, pod_name, shard_ids) in executors {
        shard_state.add_executor(
            *executor_id,
            *addr,
            pod_name.map(str::to_string),
            granted_at(),
            LEASE_TTL,
        );
        for shard_id in *shard_ids {
            shard_state.assign_shard(*executor_id, ShardId::new(*shard_id));
        }
    }
    shard_state
        .bump_revision()
        .expect("revision bump should succeed");
    shard_state
}

fn sample_shard_state(number_of_shards: usize) -> ShardLeaseState {
    let mut shard_state = shard_state_with_executors(
        number_of_shards,
        &[
            (
                executor(1),
                addr(1, 9010),
                Some("worker-executor-0"),
                &[0, 1, 2],
            ),
            (executor(2), addr(2, 9011), None, &[3, 4]),
        ],
    );
    // make sure a non-initial epoch and an orphaned shard survive the roundtrip too
    shard_state.assign_shard(executor(2), ShardId::new(0));
    shard_state.add_executor(executor(3), addr(3, 9012), None, granted_at(), LEASE_TTL);
    shard_state.assign_shard(executor(3), ShardId::new(9));
    shard_state.remove_executor(executor(3));
    shard_state
}

fn replacement_shard_state(number_of_shards: usize) -> ShardLeaseState {
    shard_state_with_executors(
        number_of_shards,
        &[(executor(3), addr(3, 9012), None, &[5, 6, 7])],
    )
}
