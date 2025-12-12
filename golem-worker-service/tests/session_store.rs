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

use golem_common::config::{DbSqliteConfig, RedisConfig};
use golem_common::model::RetryConfig;
use golem_common::redis::RedisPool;
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_service_base::db::sqlite::SqlitePool;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis::Redis;
use golem_worker_service::gateway_execution::gateway_session_store::{
    DataKey, DataValue, GatewaySessionError, GatewaySessionStore, RedisGatewaySession,
    RedisGatewaySessionExpiration, SessionId, SqliteGatewaySession, SqliteGatewaySessionExpiration,
};
use openidconnect::Nonce;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::{test, test_dep};

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test("session-store").with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

async fn start_redis() -> (RedisConfig, SpawnedRedis) {
    let redis = SpawnedRedis::new_default();

    let redis_config = RedisConfig {
        host: "localhost".to_string(),
        port: redis.public_port(),
        database: 0,
        tracing: false,
        pool_size: 10,
        retries: RetryConfig::default(),
        key_prefix: redis.prefix().to_string(),
        username: None,
        password: None,
    };

    (redis_config, redis)
}

#[test]
pub async fn test_gateway_session_with_sqlite(_tracing: &Tracing) {
    let db_file = NamedTempFile::new().unwrap();

    let db_config = DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    };

    let db_pool = SqlitePool::configured(&db_config).await.unwrap();

    let data_value = DataValue(serde_json::Value::String(
        Nonce::new_random().secret().to_string(),
    ));

    let value = insert_and_get_session_with_sqlite(
        SessionId("test1".to_string()),
        DataKey::nonce(),
        data_value.clone(),
        db_pool.clone(),
    )
    .await
    .expect("Expecting a value for longer expiry");

    assert_eq!(value, data_value.clone());
}

#[test]
pub async fn test_gateway_session_with_sqlite_expired(_tracing: &Tracing) {
    let db_file = NamedTempFile::new().unwrap();

    let db_config = DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    };

    let pool = SqlitePool::configured(&db_config).await.unwrap();

    let data_value = DataValue(serde_json::Value::String(
        Nonce::new_random().secret().to_string(),
    ));

    let expiration =
        SqliteGatewaySessionExpiration::new(Duration::from_secs(1), Duration::from_secs(1));

    let sqlite_session = SqliteGatewaySession::new(pool.clone(), expiration.clone())
        .await
        .expect("Failed to create sqlite session");

    let session_store = Arc::new(sqlite_session);

    let data_key = DataKey::nonce();
    let session_id = SessionId("test1".to_string());

    session_store
        .insert(session_id.clone(), data_key.clone(), data_value)
        .await
        .expect("Insert to session failed");

    SqliteGatewaySession::cleanup_expired(pool, SqliteGatewaySession::current_time() + 10)
        .await
        .expect("Failed to cleanup expired sessions");

    let result = session_store.get(&session_id, &data_key).await;

    assert!(matches!(
        result,
        Err(GatewaySessionError::MissingValue { .. })
    ));
}

#[test]
pub async fn test_gateway_session_redis(_tracing: &Tracing) {
    let (redis_config, _spawned_redis) = start_redis().await;

    let redis = RedisPool::configured(&redis_config).await.unwrap();

    let data_value = DataValue(serde_json::Value::String(
        Nonce::new_random().secret().to_string(),
    ));

    // Longer Expiry in Redis returns value
    let value = insert_and_get_with_redis(
        SessionId("test1".to_string()),
        DataKey::nonce(),
        data_value.clone(),
        60 * 60,
        &redis,
    )
    .await
    .expect("Expecting a value for longer expiry");

    assert_eq!(value, data_value.clone());

    // Instant expiry in Redis returns missing value, and we should get missing value
    let result = insert_and_get_with_redis(
        SessionId("test2".to_string()),
        DataKey::nonce(),
        data_value.clone(),
        0,
        &redis,
    )
    .await;

    assert!(matches!(
        result,
        Err(GatewaySessionError::MissingValue { .. })
    ));
}

async fn insert_and_get_with_redis(
    session_id: SessionId,
    data_key: DataKey,
    data_value: DataValue,
    redis_expiry_in_seconds: u64,
    redis: &RedisPool,
) -> Result<DataValue, GatewaySessionError> {
    let session_store = Arc::new(RedisGatewaySession::new(
        redis.clone(),
        RedisGatewaySessionExpiration::new(Duration::from_secs(redis_expiry_in_seconds)),
    ));

    session_store
        .insert(session_id.clone(), data_key.clone(), data_value)
        .await?;

    session_store.get(&session_id, &data_key).await
}

async fn insert_and_get_session_with_sqlite(
    session_id: SessionId,
    data_key: DataKey,
    data_value: DataValue,
    db_pool: SqlitePool,
) -> Result<DataValue, GatewaySessionError> {
    let sqlite_session =
        SqliteGatewaySession::new(db_pool, SqliteGatewaySessionExpiration::default())
            .await
            .map_err(|err| GatewaySessionError::InternalError(err.to_string()))?;

    let session_store = Arc::new(sqlite_session);

    session_store
        .insert(session_id.clone(), data_key.clone(), data_value)
        .await?;

    session_store.get(&session_id, &data_key).await
}
