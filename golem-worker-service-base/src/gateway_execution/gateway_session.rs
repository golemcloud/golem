// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
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
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::redis::RedisPool;
use golem_common::SafeDisplay;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;
use futures_util::future::err;
use log::info;
use tokio::sync::Mutex;
use tracing::{debug, error};

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
    expire: i64,
}

#[derive(Clone)]
pub struct InMem {
    data: Arc<Mutex<HashMap<SessionId, SessionData>>>
}

impl InMem {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new()))
        }
    }
}

#[async_trait]
impl GatewaySession for InMem {
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), GatewaySessionError> {
        let mut data = self.data.lock().await;
        let session_data = data.entry(session_id).or_insert(SessionData { value: HashMap::new() });
        session_data.value.insert(data_key, data_value);
        Ok(())
    }

    async fn get(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<DataValue, GatewaySessionError> {
        let data = self.data.lock().await;
        let session_data = data.get(session_id).ok_or(GatewaySessionError::MissingValue {
            session_id: session_id.clone(),
            data_key: data_key.clone(),
        })?;
        session_data.value.get(data_key).cloned().ok_or(GatewaySessionError::MissingValue {
            session_id: session_id.clone(),
            data_key: data_key.clone(),
        })
    }
}

impl RedisGatewaySession {
    pub fn new(redis: RedisPool, expire: i64) -> Self {
        Self { redis, expire }
    }

    pub fn redis_key(session_id: &SessionId) -> String {
        format!("gateway_session:{}", session_id.0)
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

        self
            .redis
            .with("gateway_session", "insert")
            .expire(Self::redis_key(&session_id), self.expire)
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

pub struct GatewaySessionWithInMemoryCache<A> {
    backend: A,
    cache: Cache<(SessionId, DataKey), (), DataValue, GatewaySessionError>,
}

impl<A> GatewaySessionWithInMemoryCache<A> {
    pub fn new(
        inner: A,
        in_memory_expiration_in_seconds: i64,
        eviction_period_in_seconds: u64,
    ) -> Self {
        let cache = Cache::new(
            Some(1024),
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::OlderThan {
                ttl: Duration::from_secs(in_memory_expiration_in_seconds as u64),
                period: Duration::from_secs(eviction_period_in_seconds),
            },
            "gateway_session_in_memory",
        );

        Self {
            backend: inner,
            cache,
        }
    }
}

#[async_trait]
impl<A: GatewaySession + Sync + Clone + Send + 'static> GatewaySession
    for GatewaySessionWithInMemoryCache<A>
{
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), GatewaySessionError> {
        info!("Inserting session data to the backend");

        self.backend
            .insert(session_id, data_key, data_value)
            .await?;

        info!("Inserted session data into cache");

        Ok(())
    }

    async fn get(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<DataValue, GatewaySessionError> {
        info!("Getting session data from cache");
        let result = self.cache
            .get_or_insert_simple(&(session_id.clone(), data_key.clone()), || {
                let inner = self.backend.clone();
                let session_id = session_id.clone();
                let data_key = data_key.clone();

                Box::pin(async move { inner.get(&session_id, &data_key).await })
            })
            .await?;

        info!("Got session data from cache");
        Ok(result)

    }
}
