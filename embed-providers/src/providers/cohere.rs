use async_trait::async_trait;
use golem_common::config::RetryConfig;
use serde::Deserialize;
use std::env;
use wasm_wave::components::embedding::{Embedding, EmbeddingError, EmbeddingProvider};

#[derive(Debug, Clone)]
pub struct CohereEmbedProvider {
    client: reqwest::Client,
    retry_config: RetryConfig,
}

#[async_trait]
impl EmbeddingProvider for CohereEmbedProvider {
    async fn generate(
        &self,
        content: Vec<String>,
        model: Option<&str>,
    ) -> Result<Embedding, EmbeddingError> {
        let api_key = env::var("COHERE_API_KEY")
            .map_err(|_| EmbeddingError::Configuration("Missing API key".into()))?;

        // Implementation using Cohere API
        Ok(Embedding::default())
    }
}

impl CohereEmbedProvider {
    pub fn new(retry_config: RetryConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            retry_config,
        }
    }
}