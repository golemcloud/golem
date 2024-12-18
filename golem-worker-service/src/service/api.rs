use async_trait::async_trait;
use std::error::Error;
use std::fmt;
use serde::{Serialize, de::DeserializeOwned};
use redis::{Client, RedisError, AsyncCommands};
use std::sync::Arc;

#[async_trait]
pub trait Cache: Send + Sync {
    async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError>;
    async fn set<T: Serialize + Send + Sync>(&self, key: &str, value: &T) -> Result<(), CacheError>;
}

#[derive(Debug)]
pub enum CacheError {
    RedisError(RedisError),
    SerializationError(String),
    DeserializationError(String)
}


impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
             CacheError::RedisError(e) => write!(f, "Redis error: {}", e),
             CacheError::SerializationError(e) => write!(f, "Serialization error: {}", e),
             CacheError::DeserializationError(e) => write!(f, "Deserialization error: {}", e)
        }
    }
}

impl Error for CacheError {}

impl From<RedisError> for CacheError {
    fn from(error: RedisError) -> Self {
        CacheError::RedisError(error)
    }
}


pub struct RedisCache {
    redis_client: Arc<Client>,
}

impl RedisCache {
    pub async fn new(redis_url: String) -> Result<Self, CacheError> {
        let client = Client::open(redis_url)?;
        Ok(RedisCache {
            redis_client: Arc::new(client),
        })
    }
    
    async fn get_connection(&self) -> Result<redis::aio::Connection, RedisError> {
        self.redis_client.get_async_connection().await
    }
}


#[async_trait]
impl Cache for RedisCache {
    async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError> {
        let mut connection = self.get_connection().await?;
        let value: Option<Vec<u8>> = connection.get(key).await?;
        match value {
            None => Ok(None),
            Some(bytes) => {
                let deserialized_value: T = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::DeserializationError(e.to_string()))?;
                Ok(Some(deserialized_value))
            }
        }
    }

    async fn set<T: Serialize + Send + Sync>(&self, key: &str, value: &T) -> Result<(), CacheError> {
        let mut connection = self.get_connection().await?;
        let serialized_value = serde_json::to_vec(value)
            .map_err(|e| CacheError::SerializationError(e.to_string()))?;
        let _: () = connection.set(key, serialized_value).await?;
        Ok(())
    }
}