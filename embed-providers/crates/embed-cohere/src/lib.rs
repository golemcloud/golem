mod wit;
use async_trait::async_trait;
use embed_common::{
    durability::{DurableRequest, EmbeddingOperation, Operation, RerankOperation},
    logging,
    metrics::{EmbeddingMetrics, TimerGuard},
    EmbeddingConfig, EmbeddingError, EmbeddingProvider, RateLimiter,
};
use golem_api_1_x::durability::LazyInitializedPollable;
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod benches;

const COHERE_API_BASE: &str = "https://api.cohere.ai/v1";

pub struct CohereClient {
    api_key: String,
    client: reqwest::Client,
    rate_limiter: RateLimiter,
    metrics: EmbeddingMetrics,
}

#[derive(Debug, Serialize)]
struct EmbedRequest {
    texts: Vec<String>,
    model: String,
    truncate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
    meta: ResponseMeta,
}

#[derive(Debug, Deserialize)]
struct ResponseMeta {
    api_version: String,
    billable_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct RerankRequest {
    query: String,
    documents: Vec<String>,
    model: String,
    top_n: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
    meta: ResponseMeta,
}

#[derive(Debug, Deserialize)]
struct RerankResult {
    index: usize,
    relevance_score: f32,
    document: String,
}

impl CohereClient {
    pub fn new() -> Result<Self, EmbeddingError> {
        let api_key = env::var("COHERE_API_KEY")
            .map_err(|_| EmbeddingError::Internal("COHERE_API_KEY not set".to_string()))?;

        Ok(Self {
            api_key,
            client: reqwest::Client::new(),
            rate_limiter: RateLimiter::new(5, 100), // 5 concurrent, 100 rpm
            metrics: EmbeddingMetrics::new(),
        })
    }

    async fn embed(&self, texts: Vec<String>, config: &EmbeddingConfig) -> Result<EmbedResponse, EmbeddingError> {
        // Acquire rate limit permit
        self.rate_limiter.acquire().await;

        let token_count = texts.iter().map(|t| t.split_whitespace().count() as u64).sum();
        let _timer = TimerGuard::new_embedding(&self.metrics, token_count);

        if texts.is_empty() {
            return Err(EmbeddingError::InvalidRequest("Empty input texts".to_string()));
        }

        let model = config.model.clone().unwrap_or_else(|| "embed-english-v3.0".to_string());
        logging::log_embedding_request(&texts, Some(&model));
        
        let truncate = if config.truncate {
            Some("END".to_string())
        } else {
            None
        };

        let request = EmbedRequest {
            texts,
            model,
            truncate,
        };

        let response = self.client
            .post(&format!("{}/embed", COHERE_API_BASE))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                error
            })?;

        if !response.status().is_success() {
            self.metrics.record_error();
            let error = match response.status().as_u16() {
                404 => EmbeddingError::ModelNotFound(format!("Model {} not found", model)),
                429 => EmbeddingError::RateLimitExceeded,
                _ => EmbeddingError::ProviderError(format!("Cohere API error: {}", response.status())),
            };
            logging::log_embedding_error(&error);
            return Err(error);
        }

        let embed_response = response
            .json::<EmbedResponse>()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                error
            })?;

        logging::log_embedding_success(&embed_response.embeddings, &model);
        Ok(embed_response)
    }

    async fn rerank_documents(
        &self,
        query: String,
        documents: Vec<String>,
        config: &EmbeddingConfig,
    ) -> Result<RerankResponse, EmbeddingError> {
        // Acquire rate limit permit
        self.rate_limiter.acquire().await;

        let token_count = documents.iter()
            .map(|d| d.split_whitespace().count() as u64)
            .sum::<u64>() + query.split_whitespace().count() as u64;
        let _timer = TimerGuard::new_rerank(&self.metrics, token_count);

        if documents.is_empty() {
            return Err(EmbeddingError::InvalidRequest("Empty documents list".to_string()));
        }

        let model = config.model.clone().unwrap_or_else(|| "rerank-english-v2.0".to_string());
        logging::log_rerank_request(&query, &documents, Some(&model));

        let request = RerankRequest {
            query,
            documents,
            model,
            top_n: Some(documents.len() as u32),
        };

        let response = self.client
            .post(&format!("{}/rerank", COHERE_API_BASE))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                error
            })?;

        if !response.status().is_success() {
            self.metrics.record_error();
            let error = match response.status().as_u16() {
                404 => EmbeddingError::ModelNotFound(format!("Model {} not found", model)),
                429 => EmbeddingError::RateLimitExceeded,
                _ => EmbeddingError::ProviderError(format!("Cohere API error: {}", response.status())),
            };
            logging::log_embedding_error(&error);
            return Err(error);
        }

        let rerank_response = response
            .json::<RerankResponse>()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                error
            })?;

        let results: Vec<(usize, f32)> = rerank_response.results
            .iter()
            .map(|r| (r.index, r.relevance_score))
            .collect();

        logging::log_rerank_success(&results, &model);
        Ok(rerank_response)
    }

    async fn create_durable_embed(
        &self,
        operation: EmbeddingOperation,
        pollable: Arc<LazyInitializedPollable>,
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let config = EmbeddingConfig {
            model: operation.model,
            dimensions: None,
            truncate: operation.truncate,
        };

        let response = self.embed(operation.texts, &config).await?;
        pollable.complete(serde_json::to_vec(&response.embeddings).unwrap()).await.map_err(|e| 
            EmbeddingError::Durability(e.to_string())
        )?;

        Ok(response.embeddings)
    }

    async fn create_durable_rerank(
        &self,
        operation: RerankOperation,
        pollable: Arc<LazyInitializedPollable>,
    ) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        let config = EmbeddingConfig {
            model: operation.model,
            dimensions: None,
            truncate: operation.truncate,
        };

        let response = self.rerank_documents(operation.query, operation.documents, &config).await?;
        let results: Vec<(usize, f32)> = response.results
            .into_iter()
            .map(|r| (r.index, r.relevance_score))
            .collect();

        pollable.complete(serde_json::to_vec(&results).unwrap()).await.map_err(|e| 
            EmbeddingError::Durability(e.to_string())
        )?;

        Ok(results)
    }

    pub fn get_metrics(&self) -> &EmbeddingMetrics {
        &self.metrics
    }
}

#[async_trait]
impl EmbeddingProvider for CohereClient {
    async fn generate_embeddings(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let config = EmbeddingConfig {
            model: None,
            dimensions: None,
            truncate: true,
        };

        let response = self.embed(inputs, &config).await?;
        Ok(response.embeddings)
    }

    async fn rerank(&self, query: String, documents: Vec<String>) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        let config = EmbeddingConfig {
            model: None,
            dimensions: None,
            truncate: true,
        };

        let response = self.rerank_documents(query, documents, &config).await?;
        
        Ok(response.results
            .into_iter()
            .map(|r| (r.index, r.relevance_score))
            .collect())
    }

    async fn create_durable_embedding(
        &self,
        operation: Operation,
    ) -> Result<Arc<LazyInitializedPollable>, EmbeddingError> {
        let pollable = Arc::new(LazyInitializedPollable::new());
        
        match operation {
            Operation::Embed(embed_op) => {
                let result_pollable = pollable.clone();
                let client = self.clone();
                
                tokio::spawn(async move {
                    let _ = client.create_durable_embed(embed_op, result_pollable).await;
                });
            }
            Operation::Rerank(rerank_op) => {
                let result_pollable = pollable.clone();
                let client = self.clone();
                
                tokio::spawn(async move {
                    let _ = client.create_durable_rerank(rerank_op, result_pollable).await;
                });
            }
        }
        
        Ok(pollable)
    }

    async fn poll_durable_request(
        &self,
        request: &DurableRequest<Vec<Vec<f32>>>,
    ) -> Result<Option<Vec<Vec<f32>>>, EmbeddingError> {
        let data = request.pollable.poll().await.map_err(|e| 
            EmbeddingError::Durability(e.to_string())
        )?;

        match data {
            Some(bytes) => {
                let result = serde_json::from_slice(&bytes).map_err(|e|
                    EmbeddingError::Durability(format!("Failed to deserialize result: {}", e))
                )?;
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }
}

// Add Clone implementation for CohereClient to support spawning
impl Clone for CohereClient {
    fn clone(&self) -> Self {
        Self {
            api_key: self.api_key.clone(),
            client: self.client.clone(),
            rate_limiter: self.rate_limiter.clone(),
            metrics: self.metrics.clone(),
        }
    }
}