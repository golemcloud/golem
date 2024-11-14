use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;

#[async_trait]
pub trait GatewayData {
    async fn insert(&self, session_id: SessionId, data_key: DataKey, data_value: DataValue) -> Result<(), String>;
    async fn get(&self, session_id: SessionId, data_key: DataKey) -> Result<Option<DataValue>, String>;
    async fn get_params(&self, session_id: SessionId) -> Result<Option<HashMap<DataKey, DataValue>>, String>;
}

#[derive(Clone)]
pub struct GatewaySessionStore(pub Arc<dyn GatewayData + Send + Sync>,);


impl GatewaySessionStore {
    pub fn in_memory() -> Self {
        GatewaySessionStore(Arc::new(InMemoryGatewaySession::new()))
    }
}

#[derive(Hash, PartialEq, Eq)]
pub struct SessionId(String);

#[derive(Hash, PartialEq, Eq, Clone)]
pub struct DataKey(String);

#[derive(Clone)]
pub struct DataValue(pub String);

// Should be used only for testing

pub struct InMemoryGatewaySession {
    data: Arc<Mutex<HashMap<SessionId, HashMap<DataKey, DataValue>>>>
}

impl InMemoryGatewaySession {
    pub fn new() -> Self {
        InMemoryGatewaySession {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl GatewayData for InMemoryGatewaySession {
    async fn insert(&self, session_id: SessionId, data_key: DataKey, data_value: DataValue) -> Result<(), String> {
        let mut data = self.data.lock().await;
        let session_data = data.entry(session_id).or_insert(HashMap::new());
        session_data.insert(data_key, data_value);
        Ok(())
    }

    async fn get(&self, session_id: SessionId, data_key: DataKey) -> Result<DataValue, String> {
        let data = self.data.lock().await;
        match data.get(&session_id) {
            Some(session_data) => {
                match session_data.get(&data_key) {
                    Some(data_value) => Ok(data_value.clone()),
                    None => Err("Data key not found".to_string())
                }
            }
            None => Err("Session not found".to_string())
        }
    }

    async fn get_params(&self, session_id: SessionId) -> Result<Option<HashMap<DataKey, DataValue>>, String> {
        let data = self.data.lock().await;
        match data.get(&session_id) {
            Some(session_data) => Ok(Some(session_data.clone())),
            None => Ok(None)
        }
    }
}