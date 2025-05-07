use async_trait::async_trait;
use embed_common::{EmbeddingError, EmbeddingProvider, durability::{DurableRequest, Operation, EmbeddingOperation}};
use golem_api_1_x::durability::{LazyInitializedPollable, PollableStatus};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use tracing::{debug, error, info};

pub mod wit;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct OpenAIProvider {
    api_key: String,
    client: Client,
    model: String,
}

#[derive(Debug, Serialize)]
struct OpenAIEmbeddingRequest {
    model: String,
    input: Vec<String>,
    encoding_format: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingResponse {
    data: Vec<OpenAIEmbedding>,
    model: String,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbedding {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct OpenAIRerankRequest {
    model: String,
    query: String,
    documents: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIRerankResponse {
    results: Vec<OpenAIRerankResult>,
    model: String,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIRerankResult {
    document_index: usize,
    relevance_score: f32,
}

impl OpenAIProvider {
    pub fn new() -> Result<Self, EmbeddingError> {
        let api_key = env::var("OPENAI_API_KEY")
            .map_err(|_| EmbeddingError::InvalidRequest("OPENAI_API_KEY environment variable not set".to_string()))?;

        Ok(Self {
            api_key,
            client: Client::new(),
            model: "text-embedding-3-large".to_string(),
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    async fn execute_embedding_request(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let request = OpenAIEmbeddingRequest {
            model: self.model.clone(),
            input: texts,
            encoding_format: "float".to_string(),
        };

        let response = self.client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| EmbeddingError::ProviderError(format!("Failed to send request: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await
                .unwrap_or_else(|_| "Failed to get error response".to_string());
            
            if status.as_u16() == 429 {
                return Err(EmbeddingError::RateLimitExceeded);
            }
            
            return Err(EmbeddingError::ProviderError(format!("Error response: {} - {}", status, error_text)));
        }

        let embedding_response: OpenAIEmbeddingResponse = response.json().await
            .map_err(|e| EmbeddingError::ProviderError(format!("Failed to parse response: {}", e)))?;

        // Sort by index to ensure correct order
        let mut embeddings = embedding_response.data;
        embeddings.sort_by_key(|e| e.index);

        Ok(embeddings.into_iter().map(|e| e.embedding).collect())
    }

    async fn execute_rerank_request(&self, query: String, documents: Vec<String>) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        let request = OpenAIRerankRequest {
            model: self.model.clone(),
            query,
            documents,
        };

        let response = self.client
            .post("https://api.openai.com/v1/rerank")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| EmbeddingError::ProviderError(format!("Failed to send request: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await
                .unwrap_or_else(|_| "Failed to get error response".to_string());
            
            if status.as_u16() == 429 {
                return Err(EmbeddingError::RateLimitExceeded);
            }
            
            return Err(EmbeddingError::ProviderError(format!("Error response: {} - {}", status, error_text)));
        }

        let rerank_response: OpenAIRerankResponse = response.json().await
            .map_err(|e| EmbeddingError::ProviderError(format!("Failed to parse response: {}", e)))?;

        Ok(rerank_response.results.into_iter()
            .map(|r| (r.document_index, r.relevance_score))
            .collect())
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIProvider {
    async fn generate_embeddings(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if inputs.is_empty() {
            return Err(EmbeddingError::InvalidRequest("No inputs provided".to_string()));
        }

        self.execute_embedding_request(inputs).await
    }

    async fn rerank(&self, query: String, documents: Vec<String>) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        if documents.is_empty() {
            return Err(EmbeddingError::InvalidRequest("No documents provided".to_string()));
        }

        self.execute_rerank_request(query, documents).await
    }

    async fn create_durable_embedding(&self, operation: Operation) -> Result<Arc<LazyInitializedPollable>, EmbeddingError> {
        match operation {
            Operation::Embed(embedding_op) => {
                let texts = embedding_op.texts.clone();
                let provider = self.clone();
                
                let pollable = LazyInitializedPollable::new(move || {
                    let texts = texts.clone();
                    let provider = provider.clone();
                    
                    Box::pin(async move {
                        match provider.execute_embedding_request(texts).await {
                            Ok(embeddings) => Ok(serde_json::to_vec(&embeddings).unwrap()),
                            Err(e) => Err(format!("{:?}", e)),
                        }
                    })
                });
                
                Ok(Arc::new(pollable))
            },
            Operation::Rerank(_) => {
                Err(EmbeddingError::Unsupported("Durable reranking not implemented".to_string()))
            }
        }
    }

    async fn poll_durable_request(&self, request: &DurableRequest<Vec<Vec<f32>>>) -> Result<Option<Vec<Vec<f32>>>, EmbeddingError> {
        if let Some(result) = &request.result {
            return Ok(Some(result.clone()));
        }

        let status = request.pollable.poll().await
            .map_err(|e| EmbeddingError::Durability(format!("Failed to poll: {}", e)))?;

        match status {
            PollableStatus::Pending => Ok(None),
            PollableStatus::Ready(bytes) => {
                let embeddings: Vec<Vec<f32>> = serde_json::from_slice(&bytes)
                    .map_err(|e| EmbeddingError::Durability(format!("Failed to deserialize result: {}", e)))?;
                Ok(Some(embeddings))
            },
            PollableStatus::Error(e) => Err(EmbeddingError::Durability(e)),
        }
    }
}