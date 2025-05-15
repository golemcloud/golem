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

const HF_API_BASE: &str = "https://api-inference.huggingface.co/models";

pub struct HuggingFaceClient {
    api_key: String,
    client: reqwest::Client,
    rate_limiter: RateLimiter,
    metrics: EmbeddingMetrics,
}

#[derive(Debug, Serialize)]
struct EmbedRequest {
    inputs: Vec<String>,
    parameters: Option<Parameters>,
}

#[derive(Debug, Serialize)]
struct Parameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    truncate: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct InferenceResponse {
    embeddings: Vec<Vec<f32>>,
}

impl HuggingFaceClient {
    pub fn new() -> Result<Self, EmbeddingError> {
        let api_key = env::var("HUGGINGFACE_API_KEY")
            .map_err(|_| EmbeddingError::Internal("HUGGINGFACE_API_KEY not set".to_string()))?;

        Ok(Self {
            api_key,
            client: reqwest::Client::new(),
            rate_limiter: RateLimiter::new(5, 150), // 5 concurrent, 150 rpm
            metrics: EmbeddingMetrics::new(),
        })
    }

    async fn embed(
        &self,
        texts: Vec<String>,
        config: &EmbeddingConfig,
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        // Acquire rate limit permit
        self.rate_limiter.acquire().await;

        if texts.is_empty() {
            return Err(EmbeddingError::InvalidRequest("Empty input texts".to_string()));
        }

        let token_count = texts.iter().map(|t| t.split_whitespace().count() as u64).sum();
        let _timer = TimerGuard::new_embedding(&self.metrics, token_count);

        let model = config.model.clone().unwrap_or_else(|| 
            "sentence-transformers/all-MiniLM-L6-v2".to_string()
        );
        logging::log_embedding_request(&texts, Some(&model));

        let request = EmbedRequest {
            inputs: texts,
            parameters: Some(Parameters {
                truncate: Some(config.truncate),
            }),
        };

        let response = self.client
            .post(&format!("{}/{}", HF_API_BASE, model))
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
                404 => EmbeddingError::ModelNotFound(format!(
                    "Model {} not found",
                    model
                )),
                429 => EmbeddingError::RateLimitExceeded,
                _ => EmbeddingError::ProviderError(format!(
                    "HuggingFace API error: {}",
                    response.status()
                )),
            };
            logging::log_embedding_error(&error);
            return Err(error);
        }

        let embeddings = response
            .json::<Vec<InferenceResponse>>()
            .await
            .map_err(|e| {
                let error = EmbeddingError::ProviderError(e.to_string());
                logging::log_embedding_error(&error);
                self.metrics.record_error();
                error
            })?
            .into_iter()
            .map(|r| r.embeddings)
            .flatten()
            .collect::<Vec<_>>();

        logging::log_embedding_success(&embeddings, &model);
        Ok(embeddings)
    }

    async fn cross_encode(
        &self,
        query: String,
        documents: Vec<String>,
        config: &EmbeddingConfig,
    ) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        // Acquire rate limit permit
        self.rate_limiter.acquire().await;

        if documents.is_empty() {
            return Err(EmbeddingError::InvalidRequest("Empty documents list".to_string()));
        }

        let token_count = documents.iter()
            .map(|d| d.split_whitespace().count() as u64)
            .sum::<u64>() + query.split_whitespace().count() as u64;
        let _timer = TimerGuard::new_rerank(&self.metrics, token_count);

        let model = config.model.clone().unwrap_or_else(|| 
            "cross-encoder/ms-marco-MiniLM-L-6-v2".to_string()
        );
        logging::log_rerank_request(&query, &documents, Some(&model));

        let mut results = Vec::with_capacity(documents.len());
        
        // Create query-document pairs
        for (idx, doc) in documents.iter().enumerate() {
            let request = EmbedRequest {
                inputs: vec![query.clone(), doc.clone()],
                parameters: Some(Parameters {
                    truncate: Some(config.truncate),
                }),
            };

            let response = self.client
                .post(&format!("{}/{}", HF_API_BASE, model))
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
                    404 => EmbeddingError::ModelNotFound(format!(
                        "Model {} not found",
                        model
                    )),
                    429 => EmbeddingError::RateLimitExceeded,
                    _ => EmbeddingError::ProviderError(format!(
                        "HuggingFace API error: {}",
                        response.status()
                    )),
                };
                logging::log_embedding_error(&error);
                return Err(error);
            }

            let score: f32 = response
                .json()
                .await
                .map_err(|e| {
                    let error = EmbeddingError::ProviderError(e.to_string());
                    logging::log_embedding_error(&error);
                    self.metrics.record_error();
                    error
                })?;

            results.push((idx, score));
        }

        // Sort by score in descending order
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        logging::log_rerank_success(&results, &model);
        Ok(results)
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

        let embeddings = self.embed(operation.texts, &config).await?;
        pollable.complete(serde_json::to_vec(&embeddings).unwrap()).await.map_err(|e| 
            EmbeddingError::Durability(e.to_string())
        )?;

        Ok(embeddings)
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

        let results = self.cross_encode(operation.query, operation.documents, &config).await?;
        pollable.complete(serde_json::to_vec(&results).unwrap()).await.map_err(|e| 
            EmbeddingError::Durability(e.to_string())
        )?;

        Ok(results)
    }

    pub fn get_metrics(&self) -> &EmbeddingMetrics {
        &self.metrics
    }
}

impl Clone for HuggingFaceClient {
    fn clone(&self) -> Self {
        Self {
            api_key: self.api_key.clone(),
            client: self.client.clone(),
            rate_limiter: self.rate_limiter.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for HuggingFaceClient {
    async fn generate_embeddings(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let config = EmbeddingConfig {
            model: None, // Use default model
            dimensions: None,
            truncate: true,
        };

        self.embed(inputs, &config).await
    }

    async fn rerank(&self, query: String, documents: Vec<String>) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        let config = EmbeddingConfig {
            model: None, // Use default cross-encoder
            dimensions: None,
            truncate: true,
        };

        self.cross_encode(query, documents, &config).await
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