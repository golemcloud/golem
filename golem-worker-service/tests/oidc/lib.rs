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

mod handler;
mod mcp_session_store;
mod session_store;
mod sql_session_store;

use golem_common::config::{DbPostgresConfig, DbSqliteConfig, RedisConfig};
use golem_common::redis::RedisPool;
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::components::rdb::{DbInfo, Rdb};
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis::spawned_tls::{CERT_PEM, SpawnedRedisTls};
use golem_worker_service::config::with_session_store_schema;
use golem_worker_service::custom_api::oidc::session_store::SessionStore;
use golem_worker_service::custom_api::oidc::session_store::{
    PostgresSessionStore, RedisSessionStore, SqliteSessionStore, migrated_postgres_pool,
    migrated_sqlite_pool,
};
use golem_worker_service::mcp::session::store::{
    McpSessionStore, PostgresMcpSessionStore, RedisMcpSessionStore, SqliteMcpSessionStore,
};
use rustls::RootCertStore;
use std::sync::{Arc, Once};
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::test_dep;
use tracing::Level;
use uuid::Uuid;

test_r::enable!();

#[derive(Clone)]
pub struct SessionStorePair {
    pub first: Arc<dyn SessionStore>,
    pub second: Arc<dyn SessionStore>,
}

#[derive(Clone)]
pub struct BearerWriteFailureStores {
    pub healthy: Arc<dyn SessionStore>,
    pub failing: Arc<dyn SessionStore>,
    _sqlite_file: Option<Arc<NamedTempFile>>,
}

#[derive(Clone)]
pub struct AuthorizationCodeWriteFailureStores {
    pub healthy: Arc<dyn SessionStore>,
    pub failing: Arc<dyn SessionStore>,
    _sqlite_file: Option<Arc<NamedTempFile>>,
}

static TRACING_INIT: Once = Once::new();

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        TRACING_INIT.call_once(|| {
            // The AWS SDK crates transitively pull in both aws-lc-rs and ring as rustls backends.
            // When both are compiled in, rustls requires an explicit process-level default.
            // We use ring consistently with the rest of the workspace.
            let _ = rustls::crypto::ring::default_provider().install_default();
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test_pretty_without_time("worker-service-session-store-tests")
                    .with_env_overrides(),
            );
        });
        Self
    }
}

#[test_dep(scope = PerWorker)]
fn tracing() -> Tracing {
    Tracing::init()
}

/// Long enough that background cleanup never runs during a test after the first tick at store
/// construction, so expiry tests observe the read-side expiry checks. Background cleanup is tested
/// separately with its own stores.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);

#[test_dep(scope = PerWorker)]
async fn sqlite_store_file() -> NamedTempFile {
    NamedTempFile::new().unwrap()
}

fn sqlite_config(db_file: &NamedTempFile) -> DbSqliteConfig {
    DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    }
}

/// Config for the per-worker SQLite file, migrated once so that the stores that share the file do
/// not race to apply the migrations.
#[test_dep(scope = PerWorker)]
async fn migrated_sqlite_config(db_file: &NamedTempFile) -> DbSqliteConfig {
    let config = sqlite_config(db_file);
    drop(sqlite_store(&config, Duration::from_secs(60)).await);
    config
}

pub async fn sqlite_store(
    sqlite_config: &DbSqliteConfig,
    pending_login_expiration: Duration,
) -> SqliteSessionStore {
    SqliteSessionStore::new(
        migrated_sqlite_pool(sqlite_config).await.unwrap(),
        pending_login_expiration,
        CLEANUP_INTERVAL,
    )
    .unwrap()
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite")]
async fn sqlite_store_default(
    _tracing: &Tracing,
    config: &DbSqliteConfig,
) -> Arc<dyn SessionStore> {
    Arc::new(sqlite_store(config, Duration::from_secs(60)).await)
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite_pair")]
async fn sqlite_store_pair(_tracing: &Tracing, config: &DbSqliteConfig) -> SessionStorePair {
    SessionStorePair {
        first: Arc::new(sqlite_store(config, Duration::from_secs(60)).await),
        second: Arc::new(sqlite_store(config, Duration::from_secs(60)).await),
    }
}

async fn sqlite_stores_with_failing_inserts(
    table: &str,
) -> (
    Arc<dyn SessionStore>,
    Arc<dyn SessionStore>,
    Arc<NamedTempFile>,
) {
    use golem_service_base::db::PoolApi;

    let db_file = Arc::new(NamedTempFile::new().unwrap());
    let config = sqlite_config(&db_file);
    let healthy: Arc<dyn SessionStore> =
        Arc::new(sqlite_store(&config, Duration::from_secs(60)).await);
    let failing: Arc<dyn SessionStore> =
        Arc::new(sqlite_store(&config, Duration::from_secs(60)).await);
    SqlitePool::configured(&config)
        .await
        .unwrap()
        .with_rw("session_store_test", "reject_inserts")
        .execute(sqlx::query(&format!(
            r#"
            CREATE TRIGGER reject_{table}_insert
            BEFORE INSERT ON {table}
            BEGIN
                SELECT RAISE(ABORT, 'injected write failure');
            END;
            "#
        )))
        .await
        .unwrap();
    (healthy, failing, db_file)
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite_bearer_write_failure")]
async fn sqlite_bearer_write_failure_stores(_tracing: &Tracing) -> BearerWriteFailureStores {
    let (healthy, failing, db_file) = sqlite_stores_with_failing_inserts("oidc_pkce_bearer").await;
    BearerWriteFailureStores {
        healthy,
        failing,
        _sqlite_file: Some(db_file),
    }
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite_authorization_code_write_failure")]
async fn sqlite_authorization_code_write_failure_stores(
    _tracing: &Tracing,
) -> AuthorizationCodeWriteFailureStores {
    let (healthy, failing, db_file) =
        sqlite_stores_with_failing_inserts("oidc_pkce_authorization_code").await;
    AuthorizationCodeWriteFailureStores {
        healthy,
        failing,
        _sqlite_file: Some(db_file),
    }
}

/// A file of its own for the fast-expiry store: the construction-time cleanup of a store created
/// later on a shared file would delete expired records before the read-side expiry checks see them.
#[test_dep(scope = PerWorker, tagged_as = "fast_expiry")]
async fn sqlite_fast_expiry_store_file() -> NamedTempFile {
    NamedTempFile::new().unwrap()
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite_fast_expiry")]
async fn sqlite_store_fast_expiry(
    _tracing: &Tracing,
    #[tagged_as("fast_expiry")] db_file: &NamedTempFile,
) -> Arc<dyn SessionStore> {
    Arc::new(sqlite_store(&sqlite_config(db_file), Duration::ZERO).await)
}

#[test_dep(scope = Shared)]
async fn postgres() -> DockerPostgresRdb {
    DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await
}

/// Config for the database of the shared Postgres container. `schema` is `None` when the store
/// should use its default schema.
pub fn postgres_config(postgres: &DockerPostgresRdb, schema: Option<String>) -> DbPostgresConfig {
    let DbInfo::Postgres(info) = postgres.info() else {
        unreachable!("DockerPostgresRdb always reports Postgres info")
    };
    DbPostgresConfig {
        host: info.public_host,
        port: info.public_port,
        database: info.database_name,
        username: info.username,
        password: info.password,
        max_connections: 10,
        schema,
        acquire_timeout: None,
    }
}

/// Config for a new, empty schema in the database of the shared Postgres container.
pub fn unique_postgres_config(postgres: &DockerPostgresRdb) -> DbPostgresConfig {
    postgres_config(
        postgres,
        Some(format!("session_store_{}", Uuid::new_v4().simple())),
    )
}

pub async fn postgres_store(
    postgres_config: &DbPostgresConfig,
    pending_login_expiration: Duration,
) -> PostgresSessionStore {
    PostgresSessionStore::new(
        migrated_postgres_pool(&with_session_store_schema(postgres_config))
            .await
            .unwrap(),
        pending_login_expiration,
        CLEANUP_INTERVAL,
    )
    .unwrap()
}

#[test_dep(scope = Shared, tagged_as = "postgres")]
async fn postgres_store_default(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> Arc<dyn SessionStore> {
    Arc::new(postgres_store(&unique_postgres_config(postgres), Duration::from_secs(60)).await)
}

#[test_dep(scope = Shared, tagged_as = "postgres_pair")]
async fn postgres_store_pair(_tracing: &Tracing, postgres: &DockerPostgresRdb) -> SessionStorePair {
    let config = unique_postgres_config(postgres);
    SessionStorePair {
        first: Arc::new(postgres_store(&config, Duration::from_secs(60)).await),
        second: Arc::new(postgres_store(&config, Duration::from_secs(60)).await),
    }
}

async fn postgres_stores_with_failing_inserts(
    postgres: &DockerPostgresRdb,
    table: &str,
) -> (Arc<dyn SessionStore>, Arc<dyn SessionStore>) {
    use golem_service_base::db::{Pool, PoolApi};

    let config = unique_postgres_config(postgres);
    let healthy: Arc<dyn SessionStore> =
        Arc::new(postgres_store(&config, Duration::from_secs(60)).await);
    let failing: Arc<dyn SessionStore> =
        Arc::new(postgres_store(&config, Duration::from_secs(60)).await);
    let pool = PostgresPool::configured(&config).await.unwrap();
    pool.with_rw("session_store_test", "reject_inserts")
        .execute(sqlx::query(
            r#"
            CREATE FUNCTION reject_insert() RETURNS trigger AS $$
            BEGIN
                RAISE EXCEPTION 'injected write failure';
            END;
            $$ LANGUAGE plpgsql
            "#,
        ))
        .await
        .unwrap();
    pool.with_rw("session_store_test", "reject_inserts")
        .execute(sqlx::query(&format!(
            "CREATE TRIGGER reject_{table}_insert BEFORE INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION reject_insert()"
        )))
        .await
        .unwrap();
    (healthy, failing)
}

#[test_dep(scope = Shared, tagged_as = "postgres_bearer_write_failure")]
async fn postgres_bearer_write_failure_stores(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> BearerWriteFailureStores {
    let (healthy, failing) =
        postgres_stores_with_failing_inserts(postgres, "oidc_pkce_bearer").await;
    BearerWriteFailureStores {
        healthy,
        failing,
        _sqlite_file: None,
    }
}

#[test_dep(scope = Shared, tagged_as = "postgres_authorization_code_write_failure")]
async fn postgres_authorization_code_write_failure_stores(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> AuthorizationCodeWriteFailureStores {
    let (healthy, failing) =
        postgres_stores_with_failing_inserts(postgres, "oidc_pkce_authorization_code").await;
    AuthorizationCodeWriteFailureStores {
        healthy,
        failing,
        _sqlite_file: None,
    }
}

#[test_dep(scope = Shared, tagged_as = "postgres_fast_expiry")]
async fn postgres_store_fast_expiry(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> Arc<dyn SessionStore> {
    Arc::new(postgres_store(&unique_postgres_config(postgres), Duration::ZERO).await)
}

#[test_dep(scope = Shared)]
async fn redis() -> Arc<dyn Redis> {
    Arc::new(SpawnedRedis::new(
        6379,
        "".to_string(),
        Level::INFO,
        Level::ERROR,
    ))
}

#[test_dep(scope = Shared)]
async fn redis_pool(redis: &Arc<dyn Redis>) -> RedisPool {
    RedisPool::configured(&RedisConfig {
        host: redis.public_host(),
        port: redis.public_port(),
        ..Default::default()
    })
    .await
    .unwrap()
}

async fn redis_store(redis_pool: &RedisPool, expiration_millis: i64) -> RedisSessionStore {
    let expiration = fred::types::Expiration::PX(expiration_millis);
    RedisSessionStore::new(redis_pool.clone(), expiration)
}

#[test_dep(scope = Shared)]
async fn default_session_store(
    _tracing: &Tracing,
    redis_pool: &RedisPool,
) -> Arc<dyn SessionStore> {
    Arc::new(redis_store(redis_pool, 6000).await)
}

#[test_dep(scope = Shared, tagged_as = "redis")]
async fn redis_store_default(store: &Arc<dyn SessionStore>) -> Arc<dyn SessionStore> {
    store.clone()
}

#[test_dep(scope = Shared, tagged_as = "redis_pair")]
async fn redis_store_pair(_tracing: &Tracing, redis_pool: &RedisPool) -> SessionStorePair {
    SessionStorePair {
        first: Arc::new(redis_store(redis_pool, 6000).await),
        second: Arc::new(redis_store(redis_pool, 6000).await),
    }
}

#[test_dep(scope = Shared, tagged_as = "redis_bearer_write_failure")]
async fn redis_bearer_write_failure_stores(
    _tracing: &Tracing,
    redis: &Arc<dyn Redis>,
    store: &Arc<dyn SessionStore>,
) -> BearerWriteFailureStores {
    const USERNAME: &str = "oidc-bearer-write-failure";
    const PASSWORD: &str = "oidc-bearer-write-failure-password";

    let mut admin = redis.get_connection(0);
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(USERNAME)
        .arg("reset")
        .arg("on")
        .arg(format!(">{PASSWORD}"))
        .arg("~*")
        .arg("+@all")
        .arg("-set")
        .query::<()>(&mut admin)
        .unwrap();
    let restricted_pool = RedisPool::configured(&RedisConfig {
        host: redis.public_host(),
        port: redis.public_port(),
        username: Some(USERNAME.into()),
        password: Some(PASSWORD.into()),
        ..Default::default()
    })
    .await
    .unwrap();
    BearerWriteFailureStores {
        healthy: store.clone(),
        failing: Arc::new(redis_store(&restricted_pool, 6000).await),
        _sqlite_file: None,
    }
}

#[test_dep(scope = Shared, tagged_as = "redis_authorization_code_write_failure")]
async fn redis_authorization_code_write_failure_stores(
    _tracing: &Tracing,
    redis: &Arc<dyn Redis>,
    store: &Arc<dyn SessionStore>,
) -> AuthorizationCodeWriteFailureStores {
    const USERNAME: &str = "oidc-authorization-code-write-failure";
    const PASSWORD: &str = "oidc-authorization-code-write-failure-password";

    let mut admin = redis.get_connection(0);
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(USERNAME)
        .arg("reset")
        .arg("on")
        .arg(format!(">{PASSWORD}"))
        .arg("~*")
        .arg("+@all")
        .arg("-set")
        .query::<()>(&mut admin)
        .unwrap();
    let restricted_pool = RedisPool::configured(&RedisConfig {
        host: redis.public_host(),
        port: redis.public_port(),
        username: Some(USERNAME.into()),
        password: Some(PASSWORD.into()),
        ..Default::default()
    })
    .await
    .unwrap();
    AuthorizationCodeWriteFailureStores {
        healthy: store.clone(),
        failing: Arc::new(redis_store(&restricted_pool, 6000).await),
        _sqlite_file: None,
    }
}

#[test_dep(scope = Shared, tagged_as = "redis_fast_expiry")]
async fn redis_store_fast_expiry(
    _tracing: &Tracing,
    redis_pool: &RedisPool,
) -> Arc<dyn SessionStore> {
    Arc::new(redis_store(redis_pool, 100).await)
}

#[test_dep(scope = Shared, tagged_as = "mcp_redis")]
async fn mcp_redis_store(_tracing: &Tracing, redis_pool: &RedisPool) -> Arc<dyn McpSessionStore> {
    Arc::new(RedisMcpSessionStore::new(redis_pool.clone()))
}

#[test_dep(scope = PerWorker, tagged_as = "mcp_sqlite")]
async fn mcp_sqlite_store(_tracing: &Tracing, config: &DbSqliteConfig) -> Arc<dyn McpSessionStore> {
    let pool = migrated_sqlite_pool(config).await.unwrap();
    Arc::new(SqliteMcpSessionStore::new(pool, CLEANUP_INTERVAL))
}

#[test_dep(scope = Shared, tagged_as = "mcp_postgres")]
async fn mcp_postgres_store(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> Arc<dyn McpSessionStore> {
    let pool = migrated_postgres_pool(&unique_postgres_config(postgres))
        .await
        .unwrap();
    Arc::new(PostgresMcpSessionStore::new(pool, CLEANUP_INTERVAL))
}

#[test_dep(scope = Shared, tagged_as = "tls")]
async fn redis_tls() -> Arc<dyn Redis> {
    Arc::new(SpawnedRedisTls::new(
        6380,
        "".to_string(),
        Level::INFO,
        Level::ERROR,
    ))
}

#[test_dep(scope = Shared, tagged_as = "tls")]
async fn redis_tls_pool(#[tagged_as("tls")] redis_tls: &Arc<dyn Redis>) -> RedisPool {
    // Build a rustls ClientConfig that trusts the self-signed CA used by SpawnedRedisTls.
    let mut cert_store = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(CERT_PEM)) {
        cert_store
            .add(cert.expect("valid cert PEM"))
            .expect("cert added to store");
    }
    let tls_client_config = rustls::ClientConfig::builder()
        .with_root_certificates(cert_store)
        .with_no_client_auth();
    let tls_connector = fred::types::config::TlsConnector::from(tls_client_config).into();

    let redis_config = golem_common::config::RedisConfig {
        host: redis_tls.public_host(),
        port: redis_tls.public_port(),
        tls: true,
        ..Default::default()
    };

    let fred_config = fred::prelude::Config {
        username: redis_config.username.clone(),
        password: redis_config.password.clone(),
        server: fred::prelude::ServerConfig::new_centralized(
            redis_config.host.clone(),
            redis_config.port,
        ),
        database: Some(redis_config.database as u8),
        tls: Some(tls_connector),
        ..Default::default()
    };

    let policy = fred::prelude::ReconnectPolicy::new_exponential(
        redis_config.retries.max_attempts,
        redis_config.retries.min_delay.as_millis() as u32,
        redis_config.retries.max_delay.as_millis() as u32,
        redis_config.retries.multiplier.round() as u32,
    );
    let pool = fred::clients::Pool::new(
        fred_config,
        None,
        None,
        Some(policy),
        redis_config.pool_size,
    )
    .unwrap();
    RedisPool::new(pool, redis_config.key_prefix.clone())
}

#[test_dep(scope = Shared, tagged_as = "redis_tls")]
async fn redis_tls_store(
    _tracing: &Tracing,
    #[tagged_as("tls")] redis_tls_pool: &RedisPool,
) -> Arc<dyn SessionStore> {
    let expiration = fred::types::Expiration::PX(6000);
    Arc::new(RedisSessionStore::new(redis_tls_pool.clone(), expiration))
}

#[test_dep(scope = Shared, tagged_as = "redis_tls_pair")]
async fn redis_tls_store_pair(
    _tracing: &Tracing,
    #[tagged_as("tls")] redis_tls_pool: &RedisPool,
) -> SessionStorePair {
    SessionStorePair {
        first: Arc::new(RedisSessionStore::new(
            redis_tls_pool.clone(),
            fred::types::Expiration::PX(6000),
        )),
        second: Arc::new(RedisSessionStore::new(
            redis_tls_pool.clone(),
            fred::types::Expiration::PX(6000),
        )),
    }
}
