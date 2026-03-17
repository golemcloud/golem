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
use crate::repo::Deps;
use golem_common::config::DbPostgresConfig;
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
use golem_registry_service::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RegistryChangeRepo,
};
use golem_registry_service::services::registry_change_notifier::{
    PostgresRegistryChangeNotifier, RegistryChangeNotifier,
};
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use std::time::{Duration, Instant};
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tracing::{error, info};

inherit_test_dep!(Tracing);

define_matrix_dimension!(postgres_variant: Deps -> "postgres", "postgres_tls");

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

pub struct PostgresDb {
    _container: ContainerAsync<Postgres>,
    pub pool: PostgresPool,
    pub config: DbPostgresConfig,
}

pub struct PostgresTlsDb {
    _container: ContainerAsync<Postgres>,
    pub pool: PostgresPool,
    pub config: DbPostgresConfig,
}

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

async fn make_deps(pool: PostgresPool) -> Deps {
    let deps = Deps {
        account_repo: Box::new(DbAccountRepo::logged(pool.clone())),
        account_usage_repo: Box::new(DbAccountUsageRepo::logged(pool.clone())),
        application_repo: Box::new(DbApplicationRepo::logged(pool.clone())),
        environment_repo: Box::new(DbEnvironmentRepo::logged(pool.clone())),
        plan_repo: Box::new(DbPlanRepo::logged(pool.clone())),
        component_repo: Box::new(DbComponentRepo::logged(pool.clone())),
        http_api_deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(pool.clone())),
        mcp_deployment_repo: Box::new(DbMcpDeploymentRepo::logged(pool.clone())),
        deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(pool.clone())),
        full_deployment_repo: Box::new(DbDeploymentRepo::logged(pool.clone())),
        environment_share_repo: Box::new(DbEnvironmentShareRepo::logged(pool.clone())),
        plugin_repo: Box::new(DbPluginRepo::logged(pool.clone())),
        registry_change_repo: Box::new(DbRegistryChangeRepo::new(pool.clone())),
    };
    deps.setup().await;
    deps
}

#[test_dep]
async fn postgres_db(_tracing: &Tracing) -> PostgresDb {
    let (config, container) = start_plain_postgres().await;
    wait_for_postgres(&config, Duration::from_secs(30)).await;
    let pool = make_pool(&config).await;
    PostgresDb {
        _container: container,
        pool,
        config,
    }
}

#[test_dep(tagged_as = "postgres")]
async fn postgres_deps(db: &PostgresDb) -> Deps {
    make_deps(db.pool.clone()).await
}

#[test_dep]
async fn postgres_tls_db(_tracing: &Tracing) -> PostgresTlsDb {
    let (config, container) = start_tls_postgres().await;
    let pool = make_pool(&config).await;
    PostgresTlsDb {
        _container: container,
        pool,
        config,
    }
}

#[test_dep(tagged_as = "postgres_tls")]
async fn postgres_tls_deps(db: &PostgresTlsDb) -> Deps {
    make_deps(db.pool.clone()).await
}

#[test]
async fn test_create_and_get_account(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_create_and_get_account(deps).await;
}

#[test]
async fn test_application_create(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_application_create(deps).await;
}

#[test]
async fn test_application_create_concurrent(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_application_create_concurrent(deps).await;
}

#[test]
async fn test_application_delete(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_application_delete(deps).await;
}

#[test]
async fn test_environment_create(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_environment_create(deps).await;
}

#[test]
async fn test_environment_create_concurrently(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_environment_create_concurrently(deps).await;
}

#[test]
async fn test_environment_update(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_environment_update(deps).await;
}

#[test]
async fn test_environment_update_concurrently(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_environment_update_concurrently(deps).await;
}

#[test]
async fn test_component_stage(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_component_stage(deps).await;
}

#[test]
async fn test_http_api_deployment_stage(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_http_api_deployment_stage(deps).await;
}

#[test]
async fn test_account_usage(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_account_usage(deps).await;
}

#[test]
async fn test_resolve_agent_type_owner_no_email(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_resolve_agent_type_owner_no_email(deps).await;
}

#[test]
async fn test_resolve_agent_type_shared_with_email(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_resolve_agent_type_shared_with_email(deps).await;
}

#[test]
async fn test_resolve_agent_type_no_share_returns_zero_roles(
    #[dimension(postgres_variant)] deps: &Deps,
) {
    crate::repo::common::test_resolve_agent_type_no_share_returns_zero_roles(deps).await;
}

#[test]
async fn test_resolve_agent_type_no_deployment_returns_none(
    #[dimension(postgres_variant)] deps: &Deps,
) {
    crate::repo::common::test_resolve_agent_type_no_deployment_returns_none(deps).await;
}

#[test]
async fn test_resolve_agent_type_nonexistent_revision_returns_none(
    #[dimension(postgres_variant)] deps: &Deps,
) {
    crate::repo::common::test_resolve_agent_type_nonexistent_revision_returns_none(deps).await;
}

#[test]
async fn test_resolve_agent_type_unknown_email_returns_none(
    #[dimension(postgres_variant)] deps: &Deps,
) {
    crate::repo::common::test_resolve_agent_type_unknown_email_returns_none(deps).await;
}

#[test]
async fn test_mcp_deployment_create_and_update(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_mcp_deployment_create_and_update(deps).await;
}

#[test]
async fn test_mcp_deployment_list_and_delete(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_mcp_deployment_list_and_delete(deps).await;
}

#[test]
async fn test_registry_change_record_and_query(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_registry_change_record_and_query(deps).await;
}

#[test]
async fn test_registry_change_cleanup(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_registry_change_cleanup(deps).await;
}

#[test]
async fn test_registry_change_replay_and_broadcast(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_registry_change_replay_and_broadcast(deps).await;
}

#[test]
async fn test_registry_change_cursor_expired_detection(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_registry_change_cursor_expired_detection(deps).await;
}

#[test]
async fn test_registry_change_mixed_event_types(#[dimension(postgres_variant)] deps: &Deps) {
    crate::repo::common::test_registry_change_mixed_event_types(deps).await;
}

/// Tests that Postgres LISTEN/NOTIFY propagates events through the
/// `PostgresRegistryChangeNotifier` background PgListener task.
/// This validates the cross-node broadcast path used in multi-registry deployments.
#[test]
async fn test_pg_notify_propagates_through_notifier(db: &PostgresDb) {
    use std::sync::Arc;
    use uuid::Uuid;

    let repo: Arc<dyn RegistryChangeRepo> = Arc::new(DbRegistryChangeRepo::new(db.pool.clone()));

    let notifier = PostgresRegistryChangeNotifier::new(64, repo.clone(), &db.config);

    let mut join_set = tokio::task::JoinSet::new();
    notifier.start_background_tasks(&mut join_set);

    // Subscribe to the broadcast channel BEFORE writing the event
    let mut rx = notifier.subscribe();

    // Give the PgListener background task time to connect and LISTEN
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Record an event via the repo — this INSERT triggers pg_notify('registry_change', ...)
    let env_id = Uuid::new_v4();
    let event_id = repo
        .record_change_event(&NewRegistryChangeEvent::deployment_changed(
            env_id,
            42,
            vec![],
        ))
        .await
        .unwrap();

    // The PgListener should pick up the NOTIFY, read the event from the DB,
    // and broadcast it through the sender.
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for event via PgListener")
        .expect("broadcast channel closed");

    assert_eq!(received.event_id, event_id);
    assert_eq!(received.environment_id.unwrap(), env_id);
    assert_eq!(received.deployment_revision_id.unwrap(), 42);

    join_set.abort_all();
}
