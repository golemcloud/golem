// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::Tracing;
use crate::repo::Deps;
use golem_common::config::DbPostgresConfig;
use golem_registry_service::repo::account::{DbAccountRepo, LoggedAccountRepo};
use golem_registry_service::repo::application::{DbApplicationRepo, LoggedApplicationRepo};
use golem_registry_service::repo::environment::{DbEnvironmentRepo, LoggedEnvironmentRepo};
use golem_registry_service::repo::plan::{DbPlanRepository, LoggedPlanRepository};
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use test_r::{inherit_test_dep, test_dep};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;

inherit_test_dep!(Tracing);

// Deps setup --------------------------------------------------------------------------------------

struct PostgresDb {
    _container: ContainerAsync<Postgres>,
    pub pool: PostgresPool,
}

impl PostgresDb {
    async fn new() -> Self {
        let (db_config, container) = Self::start_docker_postgres().await;

        db::postgres::migrate(
            &db_config,
            MigrationsDir::new("db/migration".into()).postgres_migrations(),
        )
        .await
        .unwrap();

        let pool = PostgresPool::configured(&db_config).await.unwrap();

        Self {
            _container: container,
            pool,
        }
    }

    async fn start_docker_postgres() -> (DbPostgresConfig, ContainerAsync<Postgres>) {
        let container = tryhard::retry_fn(|| Postgres::default().with_tag("14.7-alpine").start())
            .retries(5)
            .exponential_backoff(Duration::from_millis(10))
            .max_delay(Duration::from_secs(10))
            .await
            .expect("Failed to start postgres container");

        let config = DbPostgresConfig {
            host: "localhost".to_string(),
            port: container
                .get_host_port_ipv4(5432)
                .await
                .expect("Failed to get port"),
            database: "postgres".to_string(),
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            schema: Some("test".to_string()),
            max_connections: 10,
        };

        (config, container)
    }
}

#[test_dep]
async fn db_pool(_tracing: &Tracing) -> PostgresDb {
    PostgresDb::new().await
}

#[test_dep]
async fn deps(db: &PostgresDb) -> Deps {
    let deps = Deps {
        account_repo: Arc::new(LoggedAccountRepo::new(DbAccountRepo::new(db.pool.clone()))),
        application_repo: Arc::new(LoggedApplicationRepo::new(DbApplicationRepo::new(
            db.pool.clone(),
        ))),
        environment_repo: Arc::new(LoggedEnvironmentRepo::new(DbEnvironmentRepo::new(
            db.pool.clone(),
        ))),
        plan_repo: Arc::new(LoggedPlanRepository::new(DbPlanRepository::new(
            db.pool.clone(),
        ))),
    };
    deps.setup().await;
    deps
}

// Test cases --------------------------------------------------------------------------------------

#[test]
async fn test_create_and_get_account(deps: &Deps) {
    crate::repo::common::test_create_and_get_account(deps).await;
}

#[test]
async fn test_application_ensure(deps: &Deps) {
    crate::repo::common::test_application_ensure(deps).await;
}

#[test]
async fn test_application_ensure_concurrent(deps: &Deps) {
    crate::repo::common::test_application_ensure_concurrent(deps).await;
}

#[test]
async fn test_application_delete(deps: &Deps) {
    crate::repo::common::test_application_delete(deps).await;
}

#[test]
async fn test_environment_create(deps: &Deps) {
    crate::repo::common::test_environment_create(deps).await;
}

#[test]
async fn test_environment_create_concurrently(deps: &Deps) {
    crate::repo::common::test_environment_create_concurrently(deps).await;
}

#[test]
async fn test_environment_update(deps: &Deps) {
    crate::repo::common::test_environment_update(deps).await;
}

#[test]
async fn test_environment_update_concurrently(deps: &Deps) {
    crate::repo::common::test_environment_update_concurrently(deps).await;
}
