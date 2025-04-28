mod wit;
mod metrics;
use async_trait::async_trait;
use embed_common::{
    durability::{DurableRequest, EmbeddingOperation, Operation, RerankOperation},
    logging,
    EmbeddingConfig, EmbeddingError, EmbeddingProvider, RateLimiter,
};
use golem_api_1_x::durability::LazyInitializedPollable;
use metrics::{Timer, VoyageMetrics};
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc};
use tokio::runtime::Runtime;
use once_cell::sync::Lazy;

static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    Runtime::new().expect("Failed to create Tokio runtime")
});

fn execute_async<F, T>(future: F) -> T 
where
    F: std::future::Future<Output = T>,
{
    RUNTIME.block_on(future)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod benches;

const VOYAGE_API_BASE: &str = "https://api.voyageai.com/v1";

#[derive(Clone)]
pub struct VoyageAIClient {
    api_key: String,
    client: reqwest::Client,
    rate_limiter: Arc<RateLimiter>,
    metrics: Arc<VoyageMetrics>,
}

#[derive(Debug, Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
    truncate: Option<bool>,
    encode_format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
    model: String,
    usage: VoyageUsage,
}

#[derive(Debug, Deserialize)]
struct VoyageUsage {
    prompt_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct RerankRequest {
    model: String,
    query: String,
    documents: Vec<String>,
    truncate: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
    model: String,
    usage: VoyageUsage,
}

#[derive(Debug, Deserialize)]
struct RerankResult {
    document: String,
    index: usize,
    relevance_score: f32,
}

impl VoyageAIClient {
    pub fn new() -> Result<Self, EmbeddingError> {
        let api_key = env::var("VOYAGE_API_KEY")
            .map_err(|_| EmbeddingError::Internal("VOYAGE_API_KEY not set".to_string()))?;

        Ok(Self {
            api_key,
            client: reqwest::Client::new(),
            rate_limiter: Arc::new(RateLimiter::new(5, 120)), // 5 concurrent, 120 rpm
            metrics: Arc::new(VoyageMetrics::new()),
        })
    }

    async fn embed(
        &self,
        texts: Vec<String>,
        config: &EmbeddingConfig,
    ) -> Result<EmbedResponse, EmbeddingError> {
        if texts.is_empty() {
            return Err(EmbeddingError::InvalidRequest("Empty input texts".to_string()));
        }

        // Acquire rate limit permit
        self.rate_limiter.acquire().await;

        let token_count = texts.iter().map(|t| t.split_whitespace().count() as u64).sum();
        let _timer = Timer::new(&self.metrics, token_count);

        let model = config.model.clone().unwrap_or_else(|| 
            "voyage-01".to_string()
        );
        logging::log_embedding_request(&texts, Some(&model));

        let request = EmbedRequest {
            model,
            input: texts,
            truncate: Some(config.truncate),
            encode_format: Some("float".to_string()),
        };

        let response = self.client
            .post(&format!("{}/embeddings", VOYAGE_API_BASE))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                self.metrics.record_error();
                error
            })?;

        if !response.status().is_success() {
            self.metrics.record_error();
            let error = match response.status().as_u16() {
                404 => EmbeddingError::ModelNotFound("Model not found".to_string()),
                429 => EmbeddingError::RateLimitExceeded,
                _ => EmbeddingError::ProviderError(format!(
                    "Voyage AI API error: {}",
                    response.status()
                )),
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
                self.metrics.record_error();
                error
            })?;

        logging::log_embedding_success(&embed_response.embeddings, &embed_response.model);
        Ok(embed_response)
    }

    async fn rerank_documents(
        &self,
        query: String,
        documents: Vec<String>,
        config: &EmbeddingConfig,
    ) -> Result<RerankResponse, EmbeddingError> {
        if documents.is_empty() {
            return Err(EmbeddingError::InvalidRequest("Empty documents list".to_string()));
        }

        // Acquire rate limit permit
        self.rate_limiter.acquire().await;

        let token_count = documents.iter().map(|d| d.split_whitespace().count() as u64).sum();
        let _timer = Timer::new(&self.metrics, token_count);

        let model = config.model.clone().unwrap_or_else(|| 
            "voyage-rerank-01".to_string()
        );
        logging::log_rerank_request(&query, &documents, Some(&model));

        let request = RerankRequest {
            model,
            query,
            documents,
            truncate: Some(config.truncate),
        };

        let response = self.client
            .post(&format!("{}/rerank", VOYAGE_API_BASE))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                self.metrics.record_error();
                error
            })?;

        if !response.status().is_success() {
            self.metrics.record_error();
            let error = match response.status().as_u16() {
                404 => EmbeddingError::ModelNotFound("Model not found".to_string()),
                429 => EmbeddingError::RateLimitExceeded,
                _ => EmbeddingError::ProviderError(format!(
                    "Voyage AI API error: {}",
                    response.status()
                )),
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
                self.metrics.record_error();
                error
            })?;

        logging::log_rerank_success(
            &rerank_response.results.iter().map(|r| (r.index, r.relevance_score)).collect::<Vec<_>>(),
            &rerank_response.model
        );
        Ok(rerank_response)
    }

    pub fn get_metrics(&self) -> &VoyageMetrics {
        &self.metrics
    }
}

#[async_trait]
impl EmbeddingProvider for VoyageAIClient {
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
                let config = EmbeddingConfig {
                    model: embed_op.model,
                    dimensions: None,
                    truncate: embed_op.truncate,
                };

                let response = self.embed(embed_op.texts, &config).await?;
                result_pollable.complete(serde_json::to_vec(&response.embeddings).unwrap()).await
                    .map_err(|e| EmbeddingError::Durability(e.to_string()))?;
            }
            Operation::Rerank(rerank_op) => {
                let result_pollable = pollable.clone();
                let config = EmbeddingConfig {
                    model: rerank_op.model,
                    dimensions: None,
                    truncate: rerank_op.truncate,
                };

                let response = self.rerank_documents(rerank_op.query, rerank_op.documents, &config).await?;
                let results = response.results
                    .into_iter()
                    .map(|r| (r.index, r.relevance_score))
                    .collect::<Vec<_>>();

                result_pollable.complete(serde_json::to_vec(&results).unwrap()).await
                    .map_err(|e| EmbeddingError::Durability(e.to_string()))?;
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