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
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[async_trait]
pub trait GatewaySession {
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), String>;

    async fn get(&self, session_id: &SessionId) -> Result<Option<SessionData>, String>;

    async fn get_data_value(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<Option<DataValue>, String>;
    async fn get_params(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<HashMap<DataKey, DataValue>>, String>;
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

#[derive(Debug, Clone)]
pub struct DataValue(pub serde_json::Value);

impl DataValue {
    pub fn as_string(&self) -> Option<String> {
        self.0.as_str().map(|s| s.to_string())
    }
}

#[derive(Clone)]
pub struct SessionData {
    pub value: HashMap<DataKey, DataValue>,
    created_at: Instant,
}

impl Default for SessionData {
    fn default() -> Self {
        SessionData {
            value: HashMap::new(),
            created_at: Instant::now(),
        }
    }
}

pub struct InMemoryGatewaySession {
    data: Arc<Mutex<HashMap<SessionId, SessionData>>>,
    eviction_strategy: EvictionStrategy,
}

#[derive(Clone)]
pub struct EvictionStrategy {
    ttl: Duration,
    period: Duration,
}

impl EvictionStrategy {
    pub fn new(ttl: &Duration, period: &Duration) -> EvictionStrategy {
        EvictionStrategy {
            ttl: *ttl,
            period: *period,
        }
    }
}

impl Default for EvictionStrategy {
    fn default() -> Self {
        EvictionStrategy {
            ttl: Duration::from_secs(60 * 60),
            period: Duration::from_secs(60),
        }
    }
}

impl InMemoryGatewaySession {
    pub fn new(expiry_strategy: &EvictionStrategy) -> Self {
        let session = InMemoryGatewaySession {
            data: Arc::new(Mutex::new(HashMap::new())),
            eviction_strategy: expiry_strategy.clone(),
        };

        let data_clone = Arc::clone(&session.data);
        let eviction_strategy_clone = session.eviction_strategy.clone();

        // Start the eviction task in the background
        tokio::spawn(async move {
            let session = InMemoryGatewaySession {
                data: data_clone,
                eviction_strategy: eviction_strategy_clone,
            };

            session.start_eviction_task().await;
        });

        session
    }

    async fn start_eviction_task(self) {
        loop {
            tokio::time::sleep(self.eviction_strategy.period).await;
            self.perform_ttl_eviction(self.eviction_strategy.ttl).await;
        }
    }

    async fn perform_ttl_eviction(&self, ttl: Duration) {
        let mut data = self.data.lock().await;
        let now = Instant::now();

        let mut sessions_to_evict = Vec::new();

        for (session_id, session_data) in data.iter_mut() {
            let age = now.duration_since(session_data.created_at);
            if age > ttl {
                sessions_to_evict.push(session_id.clone());
            }
        }

        for session_id in sessions_to_evict {
            data.remove(&session_id);
        }
    }
}

#[async_trait]
impl GatewaySession for InMemoryGatewaySession {
    async fn insert(
        &self,
        session_id: SessionId,
        data_key: DataKey,
        data_value: DataValue,
    ) -> Result<(), String> {
        let mut data = self.data.lock().await;
        let session_data = data.entry(session_id).or_insert(SessionData::default());
        session_data.value.insert(data_key, data_value);
        Ok(())
    }

    async fn get(&self, session_id: &SessionId) -> Result<Option<SessionData>, String> {
        let data = self.data.lock().await;
        match data.get(session_id) {
            Some(session_data) => Ok(Some(session_data.clone())),
            None => Ok(None),
        }
    }

    async fn get_data_value(
        &self,
        session_id: &SessionId,
        data_key: &DataKey,
    ) -> Result<Option<DataValue>, String> {
        let data = self.data.lock().await;
        match data.get(session_id) {
            Some(session_data) => match session_data.value.get(data_key) {
                Some(data_value) => Ok(Some(data_value.clone())),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    async fn get_params(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<HashMap<DataKey, DataValue>>, String> {
        let data = self.data.lock().await;
        match data.get(session_id) {
            Some(session_data) => Ok(Some(session_data.value.clone())),
            None => Ok(None),
        }
    }
}
