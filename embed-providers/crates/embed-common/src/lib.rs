use std::error::Error;
use async_trait::async_trait;
pub mod durability;
pub mod wit;

use durability::{DurableRequest, Operation};
use golem_api_1_x::durability::LazyInitializedPollable;
use std::sync::Arc;

/// Common trait for embedding providers
#[async_trait]
pub trait EmbeddingProvider {
    /// Generate embeddings for the given inputs
    async fn generate_embeddings(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    
    /// Rerank documents based on query relevance
    async fn rerank(&self, query: String, documents: Vec<String>) -> Result<Vec<(usize, f32)>, EmbeddingError>;

    /// Create a durable embedding request
    async fn create_durable_embedding(&self, operation: Operation) -> Result<Arc<LazyInitializedPollable>, EmbeddingError>;

    /// Poll durable request
    async fn poll_durable_request(&self, request: &DurableRequest<Vec<Vec<f32>>>) -> Result<Option<Vec<Vec<f32>>>, EmbeddingError>;
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    
    #[error("Feature not supported: {0}")]
    Unsupported(String),
    
    #[error("Provider error: {0}")]
    ProviderError(String),
    
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    
    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Durability error: {0}")]
    Durability(String),
}

/// Configuration for embedding requests
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    pub model: Option<String>,
    pub dimensions: Option<u32>,
    pub truncate: bool,
}

/// Provider-agnostic embedding response
#[derive(Debug)]
pub struct EmbeddingResponse {
    pub embeddings: Vec<Vec<f32>>,
    pub usage: Option<Usage>,
    pub model: String,
}

#[derive(Debug)]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}