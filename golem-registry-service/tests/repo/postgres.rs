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

use crate::Tracing;
use golem_common::config::DbPostgresConfig;
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, sequential_suite};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tracing::{error, info};

inherit_test_dep!(Tracing);

sequential_suite!(plain);
sequential_suite!(tls);

const SERVER_CERT_PEM: &[u8] = include_bytes!("tls/server.crt");
const SERVER_KEY_PEM: &[u8] = include_bytes!("tls/server.key");

const SSL_INIT_SCRIPT: &str = r#"#!/bin/bash
set -euo pipefail

PGDATA="${PGDATA:-/var/lib/postgresql/data}"

cp /tmp/golem_test_server.crt "$PGDATA/server.crt"
cp /tmp/golem_test_server.key "$PGDATA/server.key"
chmod 600 "$PGDATA/server.key"
chown postgres:postgres "$PGDATA/server.key" "$PGDATA/server.crt"

cat >> "$PGDATA/postgresql.conf" <<'PGCONF'
ssl = on
ssl_cert_file = 'server.crt'
ssl_key_file  = 'server.key'
PGCONF

sed -i 's/^host /hostssl /g' "$PGDATA/pg_hba.conf"
"#;

async fn start_plain_postgres() -> (DbPostgresConfig, ContainerAsync<Postgres>) {
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

async fn start_tls_postgres() -> (DbPostgresConfig, ContainerAsync<Postgres>) {
    let container = tryhard::retry_fn(|| {
        Postgres::default()
            .with_tag("14.7-alpine")
            .with_copy_to("/tmp/golem_test_server.crt", SERVER_CERT_PEM.to_vec())
            .with_copy_to("/tmp/golem_test_server.key", SERVER_KEY_PEM.to_vec())
            .with_copy_to(
                "/docker-entrypoint-initdb.d/00_setup_ssl.sh",
                SSL_INIT_SCRIPT.as_bytes().to_vec(),
            )
            .start()
    })
    .retries(5)
    .exponential_backoff(Duration::from_millis(10))
    .max_delay(Duration::from_secs(10))
    .await
    .expect("Failed to start TLS postgres container");

    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get host port");

    info!("TLS Postgres container started on port {port}");

    let config = DbPostgresConfig {
        host: "localhost".to_string(),
        port,
        database: "postgres".to_string(),
        username: "postgres".to_string(),
        password: "postgres".to_string(),
        schema: Some("test_tls".to_string()),
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
        let running = check_if_postgres_ready(info).await;
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

async fn make_pool(config: &DbPostgresConfig) -> PostgresPool {
    db::postgres::migrate(
        config,
        MigrationsDir::new("db/migration".into()).postgres_migrations(),
    )
    .await
    .unwrap();

    PostgresPool::configured(config).await.unwrap()
}

macro_rules! postgres_repo_tests {
    ($dep:ident) => {
        #[test]
        async fn test_create_and_get_account($dep: &Deps) {
            crate::repo::common::test_create_and_get_account($dep).await;
        }

        #[test]
        async fn test_application_create($dep: &Deps) {
            crate::repo::common::test_application_create($dep).await;
        }

        #[test]
        async fn test_application_create_concurrent($dep: &Deps) {
            crate::repo::common::test_application_create_concurrent($dep).await;
        }

        #[test]
        async fn test_application_delete($dep: &Deps) {
            crate::repo::common::test_application_delete($dep).await;
        }

        #[test]
        async fn test_environment_create($dep: &Deps) {
            crate::repo::common::test_environment_create($dep).await;
        }

        #[test]
        async fn test_environment_create_concurrently($dep: &Deps) {
            crate::repo::common::test_environment_create_concurrently($dep).await;
        }

        #[test]
        async fn test_environment_update($dep: &Deps) {
            crate::repo::common::test_environment_update($dep).await;
        }

        #[test]
        async fn test_environment_update_concurrently($dep: &Deps) {
            crate::repo::common::test_environment_update_concurrently($dep).await;
        }

        #[test]
        async fn test_component_stage($dep: &Deps) {
            crate::repo::common::test_component_stage($dep).await;
        }

        #[test]
        async fn test_http_api_deployment_stage($dep: &Deps) {
            crate::repo::common::test_http_api_deployment_stage($dep).await;
        }

        #[test]
        async fn test_account_usage($dep: &Deps) {
            crate::repo::common::test_account_usage($dep).await;
        }

        #[test]
        async fn test_resolve_agent_type_owner_no_email($dep: &Deps) {
            crate::repo::common::test_resolve_agent_type_owner_no_email($dep).await;
        }

        #[test]
        async fn test_resolve_agent_type_shared_with_email($dep: &Deps) {
            crate::repo::common::test_resolve_agent_type_shared_with_email($dep).await;
        }

        #[test]
        async fn test_resolve_agent_type_no_share_returns_zero_roles($dep: &Deps) {
            crate::repo::common::test_resolve_agent_type_no_share_returns_zero_roles($dep).await;
        }

        #[test]
        async fn test_resolve_agent_type_no_deployment_returns_none($dep: &Deps) {
            crate::repo::common::test_resolve_agent_type_no_deployment_returns_none($dep).await;
        }

        #[test]
        async fn test_resolve_agent_type_nonexistent_revision_returns_none($dep: &Deps) {
            crate::repo::common::test_resolve_agent_type_nonexistent_revision_returns_none($dep)
                .await;
        }

        #[test]
        async fn test_resolve_agent_type_unknown_email_returns_none($dep: &Deps) {
            crate::repo::common::test_resolve_agent_type_unknown_email_returns_none($dep).await;
        }

        #[test]
        async fn test_mcp_deployment_create_and_update($dep: &Deps) {
            crate::repo::common::test_mcp_deployment_create_and_update($dep).await;
        }

        #[test]
        async fn test_mcp_deployment_list_and_delete($dep: &Deps) {
            crate::repo::common::test_mcp_deployment_list_and_delete($dep).await;
        }
    };
}

pub mod plain {
    use super::{make_pool, start_plain_postgres, wait_for_postgres};
    use crate::Tracing;
    use crate::repo::Deps;
    use golem_registry_service::repo::account::DbAccountRepo;
    use golem_registry_service::repo::account_usage::DbAccountUsageRepo;
    use golem_registry_service::repo::application::DbApplicationRepo;
    use golem_registry_service::repo::component::DbComponentRepo;
    use golem_registry_service::repo::deployment::DbDeploymentRepo;
    use golem_registry_service::repo::environment::DbEnvironmentRepo;
    use golem_registry_service::repo::environment_share::DbEnvironmentShareRepo;
    use golem_registry_service::repo::http_api_deployment::DbHttpApiDeploymentRepo;
    use golem_registry_service::repo::mcp_deployment::DbMcpDeploymentRepo;
    use golem_registry_service::repo::plan::DbPlanRepo;
    use golem_registry_service::repo::plugin::DbPluginRepo;
    use golem_service_base::db::postgres::PostgresPool;
    use std::time::Duration;
    use test_r::test;
    use test_r::{inherit_test_dep, test_dep};
    use testcontainers::ContainerAsync;
    use testcontainers_modules::postgres::Postgres;

    inherit_test_dep!(Tracing);

    pub struct PostgresDb {
        _container: ContainerAsync<Postgres>,
        pub pool: PostgresPool,
    }

    #[test_dep]
    async fn db_pool(_tracing: &Tracing) -> PostgresDb {
        let (config, container) = start_plain_postgres().await;
        wait_for_postgres(&config, Duration::from_secs(30)).await;
        let pool = make_pool(&config).await;
        PostgresDb {
            _container: container,
            pool,
        }
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
            mcp_deployment_repo: Box::new(DbMcpDeploymentRepo::logged(db.pool.clone())),
            deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
            full_deployment_repo: Box::new(DbDeploymentRepo::logged(db.pool.clone())),
            environment_share_repo: Box::new(DbEnvironmentShareRepo::logged(db.pool.clone())),
            plugin_repo: Box::new(DbPluginRepo::logged(db.pool.clone())),
        };
        deps.setup().await;
        deps
    }

    postgres_repo_tests!(deps);
}

pub mod tls {
    use super::{make_pool, start_tls_postgres};
    use crate::Tracing;
    use crate::repo::Deps;
    use golem_registry_service::repo::account::DbAccountRepo;
    use golem_registry_service::repo::account_usage::DbAccountUsageRepo;
    use golem_registry_service::repo::application::DbApplicationRepo;
    use golem_registry_service::repo::component::DbComponentRepo;
    use golem_registry_service::repo::deployment::DbDeploymentRepo;
    use golem_registry_service::repo::environment::DbEnvironmentRepo;
    use golem_registry_service::repo::environment_share::DbEnvironmentShareRepo;
    use golem_registry_service::repo::http_api_deployment::DbHttpApiDeploymentRepo;
    use golem_registry_service::repo::mcp_deployment::DbMcpDeploymentRepo;
    use golem_registry_service::repo::plan::DbPlanRepo;
    use golem_registry_service::repo::plugin::DbPluginRepo;
    use golem_service_base::db::postgres::PostgresPool;
    use test_r::test;
    use test_r::{inherit_test_dep, test_dep};
    use testcontainers::ContainerAsync;
    use testcontainers_modules::postgres::Postgres;

    inherit_test_dep!(Tracing);

    pub struct PostgresTlsDb {
        _container: ContainerAsync<Postgres>,
        pub pool: PostgresPool,
    }

    #[test_dep]
    async fn tls_db(_tracing: &Tracing) -> PostgresTlsDb {
        let (config, container) = start_tls_postgres().await;
        let pool = make_pool(&config).await;
        PostgresTlsDb {
            _container: container,
            pool,
        }
    }

    #[test_dep]
    async fn tls_deps(db: &PostgresTlsDb) -> Deps {
        let deps = Deps {
            account_repo: Box::new(DbAccountRepo::logged(db.pool.clone())),
            account_usage_repo: Box::new(DbAccountUsageRepo::logged(db.pool.clone())),
            application_repo: Box::new(DbApplicationRepo::logged(db.pool.clone())),
            environment_repo: Box::new(DbEnvironmentRepo::logged(db.pool.clone())),
            plan_repo: Box::new(DbPlanRepo::logged(db.pool.clone())),
            component_repo: Box::new(DbComponentRepo::logged(db.pool.clone())),
            http_api_deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
            mcp_deployment_repo: Box::new(DbMcpDeploymentRepo::logged(db.pool.clone())),
            deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
            full_deployment_repo: Box::new(DbDeploymentRepo::logged(db.pool.clone())),
            environment_share_repo: Box::new(DbEnvironmentShareRepo::logged(db.pool.clone())),
            plugin_repo: Box::new(DbPluginRepo::logged(db.pool.clone())),
        };
        deps.setup().await;
        deps
    }

    postgres_repo_tests!(tls_deps);
}
