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

mod handler;
mod session_store;

use golem_common::config::{DbSqliteConfig, RedisConfig};
use golem_common::redis::RedisPool;
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_service_base::db::sqlite::SqlitePool;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_worker_service::custom_api::oidc::session_store::SessionStore;
use golem_worker_service::custom_api::oidc::session_store::{
    RedisSessionStore, SqliteSessionStore,
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::test_dep;
use tracing::Level;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("worker-service-session-store-tests")
                .with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
async fn sqlite_store_file() -> NamedTempFile {
    NamedTempFile::new().unwrap()
}

#[test_dep]
async fn sqlite_pool(db_file: &NamedTempFile) -> SqlitePool {
    let db_config = DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    };

    SqlitePool::configured(&db_config).await.unwrap()
}

async fn sqlite_store(sqlite_pool: &SqlitePool, expiration_secs: i64) -> SqliteSessionStore {
    SqliteSessionStore::new(
        sqlite_pool.clone(),
        expiration_secs,
        Duration::from_millis(50),
    )
    .await
    .unwrap()
}

#[test_dep(tagged_as = "sqlite")]
async fn sqlite_store_default(
    _tracing: &Tracing,
    sqlite_pool: &SqlitePool,
) -> Arc<dyn SessionStore> {
    Arc::new(sqlite_store(sqlite_pool, 60).await)
}

#[test_dep(tagged_as = "sqlite_fast_expiry")]
async fn sqlite_store_fast_expiry(
    _tracing: &Tracing,
    sqlite_pool: &SqlitePool,
) -> Arc<dyn SessionStore> {
    Arc::new(sqlite_store(sqlite_pool, 0).await)
}

#[test_dep]
async fn redis() -> Arc<dyn Redis> {
    Arc::new(SpawnedRedis::new(
        6379,
        "".to_string(),
        Level::INFO,
        Level::ERROR,
    ))
}

#[test_dep]
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

#[test_dep]
async fn default_session_store(
    _tracing: &Tracing,
    redis_pool: &RedisPool,
) -> Arc<dyn SessionStore> {
    Arc::new(redis_store(redis_pool, 6000).await)
}

#[test_dep(tagged_as = "redis")]
async fn redis_store_default(store: &Arc<dyn SessionStore>) -> Arc<dyn SessionStore> {
    store.clone()
}

#[test_dep(tagged_as = "redis_fast_expiry")]
async fn redis_store_fast_expiry(
    _tracing: &Tracing,
    redis_pool: &RedisPool,
) -> Arc<dyn SessionStore> {
    Arc::new(redis_store(redis_pool, 100).await)
}
