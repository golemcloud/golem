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

use super::session_store::{
    sample_authorization_code, sample_bearer, sample_binding, sample_pending_login,
    sample_pending_pkce, sample_session,
};
use super::{Tracing, postgres_config, postgres_store, unique_postgres_config};
use chrono::{TimeDelta, Utc};
use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_service::config::{
    DEFAULT_POSTGRES_SESSION_STORE_SCHEMA, PostgresSessionStoreConfig, SqliteSessionStoreConfig,
};
use golem_worker_service::custom_api::oidc::model::{
    AuthorizationCode, BearerToken, McpPendingAuth, McpProxyCodeEntry, SessionId,
};
use golem_worker_service::custom_api::oidc::session_store::{
    PostgresSessionStore, SessionStore, SqliteSessionStore,
};
use sqlx::{Connection, Executor, Row};
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(DockerPostgresRdb);

const TABLES: [&str; 7] = [
    "oidc_pending_login",
    "oidc_session",
    "oidc_pkce_pending",
    "oidc_pkce_authorization_code",
    "oidc_pkce_bearer",
    "mcp_pending_auth",
    "mcp_proxy_code",
];

/// Every record that the writer store stores, except the live session, expires after this delay.
const WRITER_EXPIRATION: Duration = Duration::from_secs(1);

/// Stores one record in every table that expires after [`WRITER_EXPIRATION`], and one authenticated
/// session that stays valid. The store must have a `pending_login_expiration` of
/// [`WRITER_EXPIRATION`].
async fn store_expiring_and_live_records(store: &dyn SessionStore) -> anyhow::Result<SessionId> {
    let soon = Utc::now() + TimeDelta::from_std(WRITER_EXPIRATION)?;
    let binding = sample_binding();

    store
        .store_pending_oidc_login(&Uuid::new_v4().to_string(), sample_pending_login())
        .await?;
    store
        .store_mcp_pending_auth(
            &Uuid::new_v4().to_string(),
            McpPendingAuth {
                client_redirect_uri: "https://client.example/callback".into(),
                client_state: None,
            },
        )
        .await?;
    store
        .store_mcp_proxy_code(
            &Uuid::new_v4().to_string(),
            McpProxyCodeEntry {
                id_token: "id-token".into(),
                refresh_token: None,
                expires_in: None,
                token_type: "Bearer".into(),
            },
        )
        .await?;
    store
        .store_authenticated_session(&SessionId(Uuid::now_v7()), sample_session(soon))
        .await?;
    store
        .store_pending_pkce_login(
            &Uuid::new_v4().to_string(),
            sample_pending_pkce(binding.clone(), soon),
        )
        .await?;
    store
        .store_authorization_code(
            &AuthorizationCode::generate(),
            sample_authorization_code(binding.clone(), soon),
        )
        .await?;
    store
        .store_bearer_credential(&BearerToken::generate(), sample_bearer(binding, soon))
        .await?;

    let live = SessionId(Uuid::now_v7());
    store
        .store_authenticated_session(&live, sample_session(Utc::now() + TimeDelta::minutes(5)))
        .await?;
    Ok(live)
}

async fn sqlite_row_count(pool: &SqlitePool) -> anyhow::Result<i64> {
    let mut total = 0;
    for table in TABLES {
        let row = pool
            .with_ro("session_store_test", "row_count")
            .fetch_one(sqlx::query(&format!("SELECT COUNT(*) FROM {table}")))
            .await?;
        total += row.try_get::<i64, _>(0)?;
    }
    Ok(total)
}

async fn postgres_row_count(pool: &PostgresPool) -> anyhow::Result<i64> {
    let mut total = 0;
    for table in TABLES {
        let row = pool
            .with_ro("session_store_test", "row_count")
            .fetch_one(sqlx::query(&format!("SELECT COUNT(*) FROM {table}")))
            .await?;
        total += row.try_get::<i64, _>(0)?;
    }
    Ok(total)
}

async fn wait_for_row_count<F, Fut>(expected: i64, count: F) -> anyhow::Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<i64>>,
{
    let mut last = count().await?;
    for _ in 0..100 {
        if last == expected {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        last = count().await?;
    }
    anyhow::bail!("expected {expected} rows, found {last}")
}

fn sqlite_store_config(
    sqlite_config: &DbSqliteConfig,
    cleanup_interval: Duration,
) -> SqliteSessionStoreConfig {
    SqliteSessionStoreConfig {
        pending_login_expiration: WRITER_EXPIRATION,
        cleanup_interval,
        sqlite_config: sqlite_config.clone(),
    }
}

fn postgres_store_config(
    postgres_config: &DbPostgresConfig,
    cleanup_interval: Duration,
) -> PostgresSessionStoreConfig {
    PostgresSessionStoreConfig {
        pending_login_expiration: WRITER_EXPIRATION,
        cleanup_interval,
        postgres_config: postgres_config.clone(),
    }
}

// The writer store runs its cleanup only once, at construction. That first cleanup can run while
// the records are written, so no record expires before `WRITER_EXPIRATION`. The test then waits
// until all records except the live session have expired, and starts the cleaner store.

#[test]
#[timeout("30s")]
async fn sqlite_cleanup_deletes_only_expired_records(_tracing: &Tracing) -> anyhow::Result<()> {
    let db_file = NamedTempFile::new()?;
    let config = DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    };
    let writer =
        SqliteSessionStore::configured(&sqlite_store_config(&config, Duration::from_secs(3600)))
            .await?;
    let pool = SqlitePool::configured(&config).await?;

    let live = store_expiring_and_live_records(&writer).await?;
    assert_eq!(sqlite_row_count(&pool).await?, 8);

    tokio::time::sleep(WRITER_EXPIRATION).await;
    let _cleaner =
        SqliteSessionStore::configured(&sqlite_store_config(&config, Duration::from_millis(50)))
            .await?;
    wait_for_row_count(1, || sqlite_row_count(&pool)).await?;
    assert!(writer.get_authenticated_session(&live).await?.is_some());
    Ok(())
}

#[test]
#[timeout("60s")]
async fn postgres_cleanup_deletes_only_expired_records(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    let config = unique_postgres_config(postgres);
    let writer = PostgresSessionStore::configured(&postgres_store_config(
        &config,
        Duration::from_secs(3600),
    ))
    .await?;
    let pool = PostgresPool::configured(&config).await?;

    let live = store_expiring_and_live_records(&writer).await?;
    assert_eq!(postgres_row_count(&pool).await?, 8);

    tokio::time::sleep(WRITER_EXPIRATION).await;
    let _cleaner = PostgresSessionStore::configured(&postgres_store_config(
        &config,
        Duration::from_millis(50),
    ))
    .await?;
    wait_for_row_count(1, || postgres_row_count(&pool)).await?;
    assert!(writer.get_authenticated_session(&live).await?.is_some());
    Ok(())
}

async fn create_database(postgres: &DockerPostgresRdb) -> anyhow::Result<DbPostgresConfig> {
    let admin_config = postgres_config(postgres, None);
    let mut admin = sqlx::PgConnection::connect_with(&admin_config.connect_options()).await?;
    let database = format!("session_store_{}", Uuid::new_v4().simple());
    admin
        .execute(sqlx::query(&format!("CREATE DATABASE {database}")))
        .await?;
    admin.close().await?;
    Ok(DbPostgresConfig {
        database,
        ..admin_config
    })
}

async fn tables_in_schema(config: &DbPostgresConfig, schema: &str) -> anyhow::Result<Vec<String>> {
    let mut conn = sqlx::PgConnection::connect_with(&config.connect_options()).await?;
    let tables = sqlx::query(
        "SELECT table_name::TEXT FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name",
    )
    .bind(schema)
    .fetch_all(&mut conn)
    .await?
    .into_iter()
    .map(|row| row.get::<String, _>(0))
    .collect();
    conn.close().await?;
    Ok(tables)
}

#[test]
#[timeout("60s")]
async fn postgres_store_without_schema_shares_a_database_with_other_migrations(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    let config = create_database(postgres).await?;

    // Another service that keeps its own migrations, including a version 1, in `public`.
    let other_migrations = tempfile::tempdir()?;
    std::fs::write(
        other_migrations.path().join("001_init.sql"),
        "CREATE TABLE other_service (id BIGINT NOT NULL);",
    )?;
    golem_service_base::db::postgres::migrate(&config, other_migrations.path().to_path_buf())
        .await?;

    let store = postgres_store(&config, Duration::from_secs(60)).await;

    let mut expected_tables = TABLES.map(str::to_string).to_vec();
    expected_tables.push("_sqlx_migrations".to_string());
    expected_tables.sort();
    assert_eq!(
        tables_in_schema(&config, DEFAULT_POSTGRES_SESSION_STORE_SCHEMA).await?,
        expected_tables
    );
    assert_eq!(
        tables_in_schema(&config, "public").await?,
        vec!["_sqlx_migrations".to_string(), "other_service".to_string()]
    );

    let state = Uuid::new_v4().to_string();
    store
        .store_pending_oidc_login(&state, sample_pending_login())
        .await?;
    assert!(store.take_pending_oidc_login(&state).await?.is_some());
    Ok(())
}

#[test]
#[timeout("60s")]
async fn postgres_stores_in_different_schemas_are_isolated(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    let first = postgres_store(&unique_postgres_config(postgres), Duration::from_secs(60)).await;
    let second = postgres_store(&unique_postgres_config(postgres), Duration::from_secs(60)).await;

    let state = Uuid::new_v4().to_string();
    first
        .store_pending_oidc_login(&state, sample_pending_login())
        .await?;
    assert!(second.take_pending_oidc_login(&state).await?.is_none());
    assert!(first.take_pending_oidc_login(&state).await?.is_some());
    Ok(())
}

#[test]
#[timeout("120s")]
async fn postgres_stores_started_together_on_a_new_schema_all_start(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    for _ in 0..5 {
        let store_config =
            postgres_store_config(&unique_postgres_config(postgres), Duration::from_secs(3600));
        let results = futures::future::join_all(
            (0..8).map(|_| PostgresSessionStore::configured(&store_config)),
        )
        .await;
        for result in results {
            result?;
        }
    }
    Ok(())
}
