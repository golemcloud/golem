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

use super::{MigratedConnections, StoreConnector, session_stores};
use crate::config::{
    PostgresSessionStoreConfig, RedisSessionStoreConfig, SessionStoreConfig,
    SqliteSessionStoreConfig,
};
use crate::mcp::session::{
    McpPostgresStorageConfig, McpSessionConfig, McpSessionStorageConfig, McpSqliteStorageConfig,
};
use async_trait::async_trait;
use futures::TryStreamExt;
use golem_common::config::{DbPostgresConfig, DbSqliteConfig, RedisConfig};
use golem_common::redis::RedisPool;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::storage::blob::memory::InMemoryS3BlobStorageFactory;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::test;

#[test]
fn sqlite_session_state_is_independent_of_the_blob_root() {
    [false, true].into_iter().for_each(|independent_mcp| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                let state = jail.directory().join("state/new");
                let blobs = jail.directory().join("independent-blobs");
                let mcp_file = jail.directory().join("other/new/mcp.db");
                let settings = format!(
                    "state_store = 'sqlite'\nblob_store = 'data_disk'\ndata_dir = {:?}\n[data_disk]\nblob_root = {:?}\n[cache]\nenabled = false\n{}",
                    state.to_str().unwrap(),
                    blobs.to_str().unwrap(),
                    if independent_mcp {
                        format!(
                            "[advanced.worker_service.mcp.session_storage]\ntype = 'Sqlite'\n[advanced.worker_service.mcp.session_storage.config]\ncleanup_interval = '1h'\ndatabase = {:?}\nmax_connections = 2\nforeign_keys = false\n",
                            mcp_file.to_str().unwrap()
                        )
                    } else {
                        String::new()
                    }
                );
                jail.create_file("settings.toml", &settings)?;
                let config = golem_common::config::ConfigLoader::<crate::config::WorkerServiceConfig>::new(
                    std::path::Path::new("settings.toml"),
                )
                .for_service("worker_service")
                .load()?;
                assert!(!state.exists());
                assert!(!blobs.starts_with(&state));
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        use crate::custom_api::oidc::model::PendingOidcLogin;
                        use crate::mcp::session::store::{McpSessionId, McpSessionRecord};
                        use golem_common::model::deployment::DeploymentRevision;
                        use golem_common::model::environment::EnvironmentId;
                        use golem_common::model::security_scheme::SecuritySchemeId;

                        let issuer = "https://id.acme.example".parse().unwrap();
                        let services = super::Services::new(&config, issuer).await.unwrap();
                        let gateway = state.join("gateway-sessions.db");
                        assert!(gateway.is_file(), "{}", gateway.display());
                        let selected_mcp = if independent_mcp { &mcp_file } else { &gateway };
                        assert!(selected_mcp.is_file(), "{}", selected_mcp.display());
                        services.session_store.store_pending_oidc_login("directory-test", PendingOidcLogin {
                            scheme_id: SecuritySchemeId(uuid::Uuid::nil()),
                            original_uri: "/retained-login".to_string(),
                            nonce: openidconnect::Nonce::new("synthetic-nonce".to_string()),
                        }).await.unwrap();
                        let id: McpSessionId = "0123456789abcdef0123456789abcdef".parse().unwrap();
                        let record = McpSessionRecord {
                            domain: golem_common::model::domain_registration::Domain("sessions.example".into()),
                            environment_id: EnvironmentId(uuid::Uuid::nil()),
                            deployment_revision: DeploymentRevision::INITIAL,
                        };
                        assert!(services.mcp_session_store.create(id, &record, Duration::from_secs(3600)).await.unwrap());
                        drop(services);
                        let services = super::Services::new(&config, "https://id.acme.example".parse().unwrap()).await.unwrap();
                        let login = services.session_store.take_pending_oidc_login("directory-test").await.unwrap().unwrap();
                        assert_eq!(login.original_uri, "/retained-login");
                        assert_eq!(services.mcp_session_store.get(id, None).await.unwrap(), Some(record));
                    });
                Ok(())
            });
        });
}

#[test]
fn sqlite_session_database_accepts_a_basename_and_keeps_filesystem_errors() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_file("blocked", "not a directory")?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let config = golem_common::config::DbSqliteConfig {
                    database: "basename-sessions.db".into(),
                    max_connections: 2,
                    foreign_keys: false,
                };
                let pool = crate::custom_api::oidc::session_store::migrated_sqlite_pool(&config)
                    .await
                    .unwrap();
                assert!(std::path::Path::new("basename-sessions.db").is_file());
                pool.close().await;
                let config = golem_common::config::DbSqliteConfig {
                    database: "blocked/new/sessions.db".into(),
                    ..config
                };
                let error = crate::custom_api::oidc::session_store::migrated_sqlite_pool(&config)
                    .await
                    .expect_err("a regular-file ancestor must stop initialization");
                assert!(
                    error.downcast_ref::<std::io::Error>().is_some(),
                    "{error:#}"
                );
                assert!(!std::path::Path::new("blocked/new/sessions.db").exists());
            });
        Ok(())
    });
}

#[test]
#[test_r::timeout("5s")]
async fn s3_startup_failure_stops_worker_service_construction() {
    let config = crate::config::WorkerServiceConfig {
        blob_storage: golem_service_base::config::BlobStorageConfig::S3(
            golem_service_base::config::S3BlobStorageConfig {
                region: "us-east-1".to_string(),
                aws_endpoint_url: Some("http://s3.test".to_string()),
                initial_agent_files_bucket: "worker-service-startup-only".to_string(),
                object_prefix: "objects-not-the-bucket".to_string(),
                ..Default::default()
            },
        ),
        ..Default::default()
    };
    let failure = std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "selected bucket transport refusal",
    );
    let factory = InMemoryS3BlobStorageFactory::failed(failure);
    let error = super::Services::new_with_blob_storage_factory(
        &config,
        "https://id.acme.example".parse().unwrap(),
        factory,
    )
    .await
    .err()
    .expect("S3 failure must stop service construction");
    assert_eq!(
        error.to_string(),
        "S3 startup check failed for bucket worker-service-startup-only"
    );
    assert_eq!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .unwrap()
            .kind(),
        std::io::ErrorKind::ConnectionRefused
    );
    assert_eq!(
        error.root_cause().to_string(),
        "selected bucket transport refusal"
    );
}

/// Opens Redis and SQLite connections like production and Postgres pools that connect only
/// when used, and records every connection it opens.
#[derive(Default)]
struct RecordingConnector {
    opened: Mutex<Vec<String>>,
}

impl RecordingConnector {
    fn record(&self, connection: String) {
        self.opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(connection);
    }

    fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl StoreConnector for RecordingConnector {
    async fn redis(&self, config: &RedisConfig) -> anyhow::Result<RedisPool> {
        self.record(format!("redis database {}", config.database));
        MigratedConnections.redis(config).await
    }

    async fn sqlite(&self, config: &DbSqliteConfig) -> anyhow::Result<SqlitePool> {
        self.record(format!("sqlite {}", config.database));
        MigratedConnections.sqlite(config).await
    }

    async fn postgres(&self, config: &DbPostgresConfig) -> anyhow::Result<PostgresPool> {
        self.record(format!("postgres schema {:?}", config.schema));
        PostgresPool::new(
            sqlx::postgres::PgPoolOptions::new().connect_lazy_with(config.connect_options()),
        )
        .await
    }
}

fn sqlite_db(database: &str) -> DbSqliteConfig {
    DbSqliteConfig {
        database: database.to_string(),
        max_connections: 2,
        foreign_keys: false,
    }
}

fn postgres_db(schema: Option<&str>) -> DbPostgresConfig {
    DbPostgresConfig {
        host: "localhost".to_string(),
        database: "golem".to_string(),
        username: "golem".to_string(),
        password: "golem".to_string(),
        port: 5432,
        max_connections: 4,
        schema: schema.map(str::to_string),
        acquire_timeout: None,
    }
}

fn mcp(session_storage: Option<McpSessionStorageConfig>) -> McpSessionConfig {
    McpSessionConfig {
        session_storage,
        ..McpSessionConfig::default()
    }
}

/// The connections that building both stores opens.
async fn opened(
    gateway: &SessionStoreConfig,
    mcp: &McpSessionConfig,
) -> anyhow::Result<Vec<String>> {
    let connector = RecordingConnector::default();
    session_stores(gateway, mcp, &connector).await?;
    Ok(connector.opened())
}

#[test]
async fn the_mcp_store_uses_the_gateway_pool_when_the_connections_are_equal() -> anyhow::Result<()>
{
    let gateway_file = NamedTempFile::new()?;
    let gateway_db = gateway_file.path().to_string_lossy().to_string();
    let other_file = NamedTempFile::new()?;
    let other_db = other_file.path().to_string_lossy().to_string();
    let gateway_sqlite = SessionStoreConfig::Sqlite(SqliteSessionStoreConfig {
        pending_login_expiration: Duration::from_secs(60),
        cleanup_interval: Duration::from_secs(3600),
        sqlite_config: sqlite_db(&gateway_db),
    });
    let mcp_sqlite = |database: &str| {
        Some(McpSessionStorageConfig::Sqlite(McpSqliteStorageConfig {
            cleanup_interval: Duration::from_secs(60),
            sqlite_config: sqlite_db(database),
        }))
    };
    let gateway_postgres = SessionStoreConfig::Postgres(PostgresSessionStoreConfig {
        pending_login_expiration: Duration::from_secs(60),
        cleanup_interval: Duration::from_secs(3600),
        postgres_config: postgres_db(None),
    });
    let mcp_postgres = |schema| {
        Some(McpSessionStorageConfig::Postgres(
            McpPostgresStorageConfig {
                cleanup_interval: Duration::from_secs(60),
                postgres_config: postgres_db(schema),
            },
        ))
    };
    let gateway_redis = SessionStoreConfig::Redis(RedisSessionStoreConfig::default());
    let mcp_redis = |database| {
        Some(McpSessionStorageConfig::Redis(RedisConfig {
            database,
            ..RedisConfig::default()
        }))
    };
    let default_schema = format!(
        "postgres schema {:?}",
        Some(crate::config::DEFAULT_POSTGRES_SESSION_STORE_SCHEMA)
    );

    let rows = [
        (&gateway_sqlite, None, vec![format!("sqlite {gateway_db}")]),
        (
            &gateway_sqlite,
            mcp_sqlite(&gateway_db),
            vec![format!("sqlite {gateway_db}")],
        ),
        (
            &gateway_sqlite,
            mcp_sqlite(&other_db),
            vec![format!("sqlite {gateway_db}"), format!("sqlite {other_db}")],
        ),
        (&gateway_postgres, None, vec![default_schema.clone()]),
        (
            &gateway_postgres,
            mcp_postgres(Some(crate::config::DEFAULT_POSTGRES_SESSION_STORE_SCHEMA)),
            vec![default_schema.clone()],
        ),
        (
            &gateway_postgres,
            mcp_postgres(Some("mcp_sessions")),
            vec![
                default_schema.clone(),
                format!("postgres schema {:?}", Some("mcp_sessions")),
            ],
        ),
        (&gateway_redis, None, vec!["redis database 0".to_string()]),
        (
            &gateway_redis,
            mcp_redis(0),
            vec!["redis database 0".to_string()],
        ),
        (
            &gateway_redis,
            mcp_redis(1),
            vec![
                "redis database 0".to_string(),
                "redis database 1".to_string(),
            ],
        ),
        (
            &gateway_redis,
            mcp_sqlite(&other_db),
            vec!["redis database 0".to_string(), format!("sqlite {other_db}")],
        ),
    ];
    futures::stream::iter(rows.into_iter().enumerate().map(Ok))
        .try_for_each(|(row, (gateway, session_storage, expected))| async move {
            let opened = opened(gateway, &mcp(session_storage)).await?;
            assert_eq!(opened, expected, "row {row}");
            Ok(())
        })
        .await
}
