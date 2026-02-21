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
use golem_registry_service::repo::account::DbAccountRepo;
use golem_registry_service::repo::account_usage::DbAccountUsageRepo;
use golem_registry_service::repo::application::DbApplicationRepo;
use golem_registry_service::repo::component::DbComponentRepo;
use golem_registry_service::repo::environment::DbEnvironmentRepo;
use golem_registry_service::repo::http_api_deployment::DbHttpApiDeploymentRepo;
use golem_registry_service::repo::plan::DbPlanRepo;
use golem_registry_service::repo::plugin::DbPluginRepo;
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use std::time::{Duration, Instant};
use test_r::test;
use test_r::{inherit_test_dep, test_dep};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tracing::{error, info};

inherit_test_dep!(Tracing);

// Deps setup --------------------------------------------------------------------------------------

struct PostgresDb {
    _container: ContainerAsync<Postgres>,
    pub pool: PostgresPool,
}

impl PostgresDb {
    async fn new() -> Self {
        let (db_config, container) = Self::start_docker_postgres().await;

        Self::wait_for_postgres(&db_config, Duration::from_secs(30)).await;

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

    async fn wait_for_postgres(info: &DbPostgresConfig, timeout: Duration) {
        info!(
            "Waiting for Postgres start on host {}:{}, timeout: {}s",
            info.host,
            info.port,
            timeout.as_secs()
        );
        let start = Instant::now();
        loop {
            let running = Self::check_if_postgres_ready(info).await;

            match running {
                Ok(_) => break,
                Err(e) => {
                    if start.elapsed() > timeout {
                        error!(
                            "Failed to verify that Postgres host {}:{} is running: {}",
                            info.host, info.port, e
                        );
                        std::panic!(
                            "Failed to verify that Postgres host {}:{} is running",
                            info.host,
                            info.port
                        );
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn check_if_postgres_ready(info: &DbPostgresConfig) -> Result<(), sqlx::Error> {
        use sqlx::Executor;
        let connection_options = PgConnectOptions::new()
            .username(&info.username)
            .password(&info.password)
            .database(&info.database)
            .host(&info.host)
            .port(info.port);

        let mut conn = connection_options.connect().await?;

        let r = conn.execute(sqlx::query("SELECT 1;")).await;
        if let Err(e) = r {
            error!("Postgres connection error: {}", e);
        }

        Ok(())
    }
}

#[test_dep]
async fn db_pool(_tracing: &Tracing) -> PostgresDb {
    PostgresDb::new().await
}

#[test_dep]
async fn deps(db: &PostgresDb) -> Deps {
    let deps = Deps {
        account_repo: Box::new(DbAccountRepo::logged(db.pool.clone())),
        account_usage_repo: Box::new(DbAccountUsageRepo::logged(db.pool.clone())),
        application_repo: Box::new(DbApplicationRepo::logged(db.pool.clone())),
        environment_repo: Box::new(DbEnvironmentRepo::logged(db.pool.clone())),
        plan_repo: Box::new(DbPlanRepo::logged(db.pool.clone())),
        component_repo: Box::new(DbComponentRepo::logged(db.pool.clone())),
        http_api_deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
        deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
        plugin_repo: Box::new(DbPluginRepo::logged(db.pool.clone())),
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
async fn test_application_create(deps: &Deps) {
    crate::repo::common::test_application_create(deps).await;
}

#[test]
async fn test_application_create_concurrent(deps: &Deps) {
    crate::repo::common::test_application_create_concurrent(deps).await;
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

#[test]
async fn test_component_stage(deps: &Deps) {
    crate::repo::common::test_component_stage(deps).await;
}

#[test]
async fn test_http_api_deployment_stage(deps: &Deps) {
    crate::repo::common::test_http_api_deployment_stage(deps).await;
}

#[test]
async fn test_account_usage(deps: &Deps) {
    crate::repo::common::test_account_usage(deps).await;
}
