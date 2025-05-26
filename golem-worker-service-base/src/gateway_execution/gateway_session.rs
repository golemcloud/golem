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

use async_trait::async_trait;
use bincode::enc::Encoder;
use bincode::error::EncodeError;
use bytes::Bytes;
use fred::interfaces::RedisResult;
use golem_common::redis::RedisPool;
use golem_common::SafeDisplay;
use golem_service_base::db::sqlite::SqlitePool;
use sqlx::Row;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tokio::time::interval;
use tracing::{error, info, Instrument};

#[async_trait]
pub trait GatewaySession {
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), GatewaySessionError>;

    async fn get(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<DataValue, GatewaySessionError>;
}

#[derive(Debug, Clone)]
pub enum GatewaySessionError {
    InternalError(String),
    MissingValue {
        session_id: SessionId,
        data_key: DataKey,
    },
}

impl SafeDisplay for GatewaySessionError {
    fn to_safe_string(&self) -> String {
        match self {
            GatewaySessionError::InternalError(e) => format!("Internal error: {}", e),
            GatewaySessionError::MissingValue { session_id, .. } => {
                format!("Invalid session {}", session_id.0)
            }
        }
    }
}

pub type GatewaySessionStore = Arc<dyn GatewaySession + Send + Sync>;

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct SessionId(pub String);

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct DataKey(pub String);

impl DataKey {
    pub fn nonce() -> DataKey {
        DataKey("nonce".to_string())
    }

    pub fn access_token() -> DataKey {
        DataKey("access_token".to_string())
    }

    pub fn id_token() -> DataKey {
        DataKey("id_token".to_string())
    }

    pub fn claims() -> DataKey {
        DataKey("claims".to_string())
    }

    pub fn redirect_url() -> DataKey {
        DataKey("redirect_url".to_string())
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DataValue(pub serde_json::Value);

impl bincode::Encode for DataValue {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        let serialized = serde_json::to_vec(&self.0)
            .map_err(|_| EncodeError::OtherString("Failed to serialize JSON".into()))?;
        serialized.encode(encoder)
    }
}

impl bincode::Decode for DataValue {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        let serialized: Vec<u8> = bincode::Decode::decode(decoder)?;
        let value = serde_json::from_slice(&serialized).map_err(|_| {
            bincode::error::DecodeError::OtherString("Failed to deserialize JSON".into())
        })?;
        Ok(DataValue(value))
    }
}

impl DataValue {
    pub fn as_string(&self) -> Option<String> {
        self.0.as_str().map(|s| s.to_string())
    }
}

#[derive(Clone)]
pub struct SessionData {
    pub value: HashMap<DataKey, DataValue>,
}

#[derive(Clone)]
pub struct RedisGatewaySession {
    redis: RedisPool,
    expiration: RedisGatewaySessionExpiration,
}

impl RedisGatewaySession {
    pub fn new(redis: RedisPool, expiration: RedisGatewaySessionExpiration) -> Self {
        Self { redis, expiration }
    }

    pub fn redis_key(session_id: &SessionId) -> String {
        format!("gateway_session:{}", session_id.0)
    }
}

#[derive(Clone)]
pub struct RedisGatewaySessionExpiration {
    pub session_expiry: Duration,
}

impl RedisGatewaySessionExpiration {
    pub fn new(session_expiry: Duration) -> Self {
        Self { session_expiry }
    }
}

impl Default for RedisGatewaySessionExpiration {
    fn default() -> Self {
        Self::new(Duration::from_secs(60 * 60))
    }
}

#[async_trait]
impl GatewaySession for RedisGatewaySession {
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), GatewaySessionError> {
        let serialised = golem_common::serialization::serialize(&data_value)
            .map_err(|e| GatewaySessionError::InternalError(e.to_string()))?;

        let result: RedisResult<()> = self
            .redis
            .with("gateway_session", "insert")
            .hset(
                Self::redis_key(&session_id),
                (data_key.0.as_str(), serialised),
            )
            .await;

        result.map_err(|e| {
            error!("Failed to insert session data into Redis: {}", e);
            GatewaySessionError::InternalError(e.to_string())
        })?;

        self.redis
            .with("gateway_session", "insert")
            .expire(
                Self::redis_key(&session_id),
                self.expiration.session_expiry.as_secs() as i64,
            )
            .await
            .map_err(|e| {
                error!("Failed to set expiry on session data in Redis: {}", e);
                GatewaySessionError::InternalError(e.to_string())
            })
    }

    async fn get(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<DataValue, GatewaySessionError> {
        let result: Option<Bytes> = self
            .redis
            .with("gateway_session", "get_data_value")
            .hget(Self::redis_key(session_id), data_key.0.as_str())
            .await
            .map_err(|e| {
                error!("Failed to get session data from Redis: {}", e);
                GatewaySessionError::InternalError(e.to_string())
            })?;

        if let Some(result) = result {
            let data_value: DataValue = golem_common::serialization::deserialize(&result)
                .map_err(|e| GatewaySessionError::InternalError(e.to_string()))?;

            Ok(data_value)
        } else {
            Err(GatewaySessionError::MissingValue {
                session_id: session_id.clone(),
                data_key: data_key.clone(),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteGatewaySession {
    pool: SqlitePool,
    expiration: SqliteGatewaySessionExpiration,
}

#[derive(Debug, Clone)]
pub struct SqliteGatewaySessionExpiration {
    pub session_expiry: Duration,
    pub cleanup_interval: Duration,
}

impl SqliteGatewaySessionExpiration {
    pub fn new(session_expiry: Duration, cleanup_interval: Duration) -> Self {
        Self {
            session_expiry,
            cleanup_interval,
        }
    }
}

impl Default for SqliteGatewaySessionExpiration {
    fn default() -> Self {
        Self::new(Duration::from_secs(60 * 60), Duration::from_secs(60))
    }
}

impl SqliteGatewaySession {
    pub async fn new(
        pool: SqlitePool,
        expiration: SqliteGatewaySessionExpiration,
    ) -> Result<Self, String> {
        let result = Self { pool, expiration };

        result.init().await?;

        let cloned_session = result.clone();

        Self::spawn_expiration_task(
            cloned_session.expiration.cleanup_interval,
            cloned_session.pool,
        );

        Ok(result)
    }

    async fn init(&self) -> Result<(), String> {
        self.pool
            .with_rw("gateway_session", "init")
            .execute(sqlx::query(
                r#"
                  CREATE TABLE IF NOT EXISTS gateway_session (
                    session_id TEXT NOT NULL,
                    data_key TEXT NOT NULL,
                    data_value BLOB NOT NULL,
                    expiry_time INTEGER NOT NULL,
                    PRIMARY KEY (session_id, data_key)
                  );
                "#,
            ))
            .await
            .map_err(|err| err.to_safe_string())?;

        info!("Initialized gateway session SQLite table");

        Ok(())
    }

    pub fn spawn_expiration_task(cleanup_internal: Duration, db_pool: SqlitePool) {
        task::spawn(
            async move {
                let mut cleanup_interval = interval(cleanup_internal);

                loop {
                    cleanup_interval.tick().await;

                    if let Err(e) =
                        Self::cleanup_expired(db_pool.clone(), Self::current_time()).await
                    {
                        error!("Failed to expire sessions: {}", e);
                    }
                }
            }
            .in_current_span(),
        );
    }

    pub async fn cleanup_expired(pool: SqlitePool, current_time: i64) -> Result<(), String> {
        let query =
            sqlx::query("DELETE FROM gateway_session WHERE expiry_time < ?;").bind(current_time);

        pool.with_rw("gateway_session", "cleanup_expired")
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    pub fn current_time() -> i64 {
        chrono::Utc::now().timestamp()
    }
}

#[async_trait]
impl GatewaySession for SqliteGatewaySession {
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), GatewaySessionError> {
        let expiry_time = Self::current_time() + self.expiration.session_expiry.as_secs() as i64;

        let serialized_value: &[u8] = &golem_common::serialization::serialize(&data_value)
            .map_err(|e| GatewaySessionError::InternalError(e.to_string()))?;

        let result = self
            .pool
            .with_rw("gateway_session", "insert")
            .execute(
                sqlx::query(
                    r#"
                  INSERT INTO gateway_session (session_id, data_key, data_value, expiry_time)
                  VALUES (?, ?, ?, ?);
                "#,
                )
                .bind(session_id.0)
                .bind(data_key.0)
                .bind(serialized_value)
                .bind(expiry_time),
            )
            .await;

        result.map_err(|e| {
            error!("Failed to insert session data into SQLite: {}", e);
            GatewaySessionError::InternalError(e.to_string())
        })?;

        Ok(())
    }

    async fn get(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<DataValue, GatewaySessionError> {
        let query = sqlx::query(
            "SELECT data_value FROM gateway_session WHERE session_id = ? AND data_key = ?;",
        )
        .bind(&session_id.0)
        .bind(&data_key.0);

        let result = self
            .pool
            .with_ro("gateway_sesssion", "get")
            .fetch_optional(query)
            .await
            .map_err(|e| GatewaySessionError::InternalError(e.to_string()))?;

        match result {
            Some(row) => {
                let row = row.get::<Vec<u8>, _>(0);

                let data_value = golem_common::serialization::deserialize(&row)
                    .map_err(|e| GatewaySessionError::InternalError(e.to_string()))?;

                Ok(data_value)
            }
            None => Err(GatewaySessionError::MissingValue {
                session_id: session_id.clone(),
                data_key: data_key.clone(),
            }),
        }
    }
}
