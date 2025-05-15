use anyhow::Error;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;

#[cfg(feature = "durability")]
use golem_api_1_x::durability::{LazyInitializedPollable, PollableStatus};

wit_bindgen::generate!({"world": "embed"});

// Export the embed interface implementation
exports::golem::embed::embed::Embed::export(Embedder::new().unwrap_or_else(|e| {
    eprintln!("Failed to initialize OpenAI embedder: {}", e);
    std::process::exit(1);
}));

// Error type for embedding operations
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
    
    #[error("Provider error: {0}")]
    ProviderError(String),
    
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    
    #[error("Internal error: {0}")]
    InternalError(String),
    
    #[error("Unknown error: {0}")]
    Unknown(String),
    
    #[cfg(feature = "durability")]
    #[error("Durability error: {0}")]
    Durability(String),
}

// Convert EmbedError to WIT Error type
impl From<EmbedError> for exports::golem::embed::embed::Error {
    fn from(err: EmbedError) -> Self {
        match err {
            EmbedError::InvalidRequest(msg) => Self::InvalidRequest {
                code: exports::golem::embed::embed::ErrorCode::InvalidRequest,
                message: msg,
                provider_error_json: None,
            },
            EmbedError::ModelNotFound(msg) => Self::ModelNotFound {
                code: exports::golem::embed::embed::ErrorCode::ModelNotFound,
                message: msg,
                provider_error_json: None,
            },
            EmbedError::Unsupported(msg) => Self::Unsupported {
                code: exports::golem::embed::embed::ErrorCode::Unsupported,
                message: msg,
                provider_error_json: None,
            },
            EmbedError::ProviderError(msg) => Self::ProviderError {
                code: exports::golem::embed::embed::ErrorCode::ProviderError,
                message: msg,
                provider_error_json: None,
            },
            EmbedError::RateLimitExceeded => Self::RateLimitExceeded {
                code: exports::golem::embed::embed::ErrorCode::RateLimitExceeded,
                message: "Rate limit exceeded".into(),
                provider_error_json: None,
            },
            EmbedError::InternalError(msg) => Self::InternalError {
                code: exports::golem::embed::embed::ErrorCode::InternalError,
                message: msg,
                provider_error_json: None,
            },
            EmbedError::Unknown(msg) => Self::Unknown {
                code: exports::golem::embed::embed::ErrorCode::Unknown,
                message: msg,
                provider_error_json: None,
            },
            #[cfg(feature = "durability")]
            EmbedError::Durability(msg) => Self::InternalError {
                code: exports::golem::embed::embed::ErrorCode::InternalError,
                message: format!("Durability error: {}", msg),
                provider_error_json: None,
            },
        }
    }
}

struct Embedder {
    client: Client,
    api_key: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct OpenAIEmbeddingRequest {
    model: String,
    input: Vec<String>,
    encoding_format: Option<String>,
    user: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingResponse {
    data: Vec<OpenAIEmbedding>,
    model: String,
    usage: OpenAIUsage,
    object: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbedding {
    embedding: Vec<f32>,
    index: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    total_tokens: u32,
}

// Durability operation types
#[cfg(feature = "durability")]
#[derive(Debug, Serialize, Deserialize)]
pub struct EmbeddingOperation {
    pub texts: Vec<String>,
    pub model: Option<String>,
    pub dimensions: Option<u32>,
    pub user: Option<String>,
}

#[cfg(feature = "durability")]
#[derive(Debug, Serialize, Deserialize)]
pub enum Operation {
    Embed(EmbeddingOperation),
}

#[cfg(feature = "durability")]
pub struct DurableRequest<T> {
    pub operation: Operation,
    pub pollable: Arc<LazyInitializedPollable>,
    pub result: Option<T>,
}

#[cfg(feature = "durability")]
impl<T> DurableRequest<T> {
    pub fn new(operation: Operation, pollable: Arc<LazyInitializedPollable>) -> Self {
        Self {
            operation,
            pollable,
            result: None,
        }
    }
}

impl Embedder {
    fn new() -> Result<Self, EmbedError> {
        // Load environment variables from .env file if present
        dotenv::dotenv().ok();
        
        let api_key = env::var("OPENAI_API_KEY")
            .map_err(|_| EmbedError::InvalidRequest("OPENAI_API_KEY environment variable not set".to_string()))?;
        
        Ok(Self {
            client: Client::new(),
            api_key,
            model: "text-embedding-3-large".to_string(),
        })
    }

    async fn create_embeddings(
        &self,
        inputs: Vec<String>,
        config: Option<&exports::golem::embed::embed::Config>,
    ) -> Result<OpenAIEmbeddingResponse, EmbedError> {
        let model = config.and_then(|c| c.model.clone())
            .unwrap_or(self.model.clone());

        let request = OpenAIEmbeddingRequest {
            model,
            input: inputs,
            encoding_format: Some("float".to_string()),
            user: config.and_then(|c| c.user.clone()),
        };

        let response = self.client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| EmbedError::ProviderError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                429 => EmbedError::RateLimitExceeded,
                404 => EmbedError::ModelNotFound(format!("Model not found: {}", error_text)),
                _ => EmbedError::ProviderError(format!("OpenAI API error: {}", error_text)),
            });
        }

        let embedding_response = response.json::<OpenAIEmbeddingResponse>().await
            .map_err(|e| EmbedError::ProviderError(format!("Failed to parse response: {}", e)))?;

        Ok(embedding_response)
    }

    fn validate_config(
        &self,
        config: &exports::golem::embed::embed::Config,
    ) -> Result<(), EmbedError> {
        if let Some(format) = &config.output_format {
            if !matches!(format, exports::golem::embed::embed::OutputFormat::FloatArray) {
                return Err(EmbedError::Unsupported("OpenAI only supports float array output format".into()));
            }
        }

        if let Some(dtype) = &config.output_dtype {
            if !matches!(dtype, exports::golem::embed::embed::OutputDtype::Float32) {
                return Err(EmbedError::Unsupported("OpenAI only supports float32 output dtype".into()));
            }
        }

        Ok(())
    }
    
    #[cfg(feature = "durability")]
    async fn create_durable_embedding(
        &self, 
        operation: Operation
    ) -> Result<Arc<LazyInitializedPollable>, EmbedError> {
        match operation {
            Operation::Embed(embedding_op) => {
                let texts = embedding_op.texts.clone();
                let model = embedding_op.model.clone();
                let user = embedding_op.user.clone();
                let api_key = self.api_key.clone();
                
                let pollable = LazyInitializedPollable::new(move || {
                    let texts = texts.clone();
                    let model = model.clone();
                    let user = user.clone();
                    let api_key = api_key.clone();
                    
                    Box::pin(async move {
                        let client = Client::new();
                        let request = OpenAIEmbeddingRequest {
                            model: model.unwrap_or_else(|| "text-embedding-3-large".to_string()),
                            input: texts,
                            encoding_format: Some("float".to_string()),
                            user,
                        };

                        let response = client
                            .post("https://api.openai.com/v1/embeddings")
                            .header("Authorization", format!("Bearer {}", api_key))
                            .json(&request)
                            .send()
                            .await
                            .map_err(|e| format!("Failed to send request: {}", e))?;

                        if !response.status().is_success() {
                            let status = response.status();
                            let error_text = response.text().await
                                .unwrap_or_else(|_| "Failed to get error response".to_string());
                            
                            return Err(format!("Error response: {} - {}", status, error_text));
                        }

                        let embedding_response: OpenAIEmbeddingResponse = response.json().await
                            .map_err(|e| format!("Failed to parse response: {}", e))?;

                        // Convert to serializable format
                        let embeddings: Vec<(u32, Vec<f32>)> = embedding_response.data.into_iter()
                            .map(|e| (e.index, e.embedding))
                            .collect();
                            
                        Ok(serde_json::to_vec(&embeddings).unwrap())
                    })
                });
                
                Ok(Arc::new(pollable))
            }
        }
    }
    
    #[cfg(feature = "durability")]
    async fn poll_durable_request(
        &self, 
        request: &DurableRequest<Vec<(u32, Vec<f32>)>>
    ) -> Result<Option<Vec<(u32, Vec<f32>)>>, EmbedError> {
        if let Some(result) = &request.result {
            return Ok(Some(result.clone()));
        }

        let status = request.pollable.poll().await
            .map_err(|e| EmbedError::Durability(format!("Failed to poll: {}", e)))?;

        match status {
            PollableStatus::Pending => Ok(None),
            PollableStatus::Ready(bytes) => {
                let embeddings: Vec<(u32, Vec<f32>)> = serde_json::from_slice(&bytes)
                    .map_err(|e| EmbedError::Durability(format!("Failed to deserialize result: {}", e)))?;
                Ok(Some(embeddings))
            },
            PollableStatus::Error(e) => Err(EmbedError::Durability(e)),
        }
    }
}

impl exports::golem::embed::embed::Embed for Embedder {
    fn generate(
        &mut self,
        inputs: Vec<exports::golem::embed::embed::ContentPart>,
        config: exports::golem::embed::embed::Config,
    ) -> Result<exports::golem::embed::embed::EmbeddingResponse, exports::golem::embed::embed::Error> {
        self.validate_config(&config)?;

        // Convert content parts to strings
        let text_inputs: Vec<String> = inputs
            .into_iter()
            .map(|part| match part {
                exports::golem::embed::embed::ContentPart::Text(text) => Ok(text),
                exports::golem::embed::embed::ContentPart::Image(_) => {
                    Err(EmbedError::Unsupported("Image embeddings not supported by OpenAI text-embedding models".into()))
                }
            })
            .collect::<Result<_, _>>()?;

        // Create durable operation if durability is enabled
        #[cfg(feature = "durability")]
        {
            let operation = Operation::Embed(EmbeddingOperation {
                texts: text_inputs.clone(),
                model: config.model.clone(),
                dimensions: config.dimensions,
                user: config.user.clone(),
            });
            
            let pollable = futures::executor::block_on(self.create_durable_embedding(operation))?;
            let request = DurableRequest::new(operation, pollable);
            
            // Poll immediately to check if we can get a result
            let result = futures::executor::block_on(self.poll_durable_request(&request))?;
            
            if let Some(embeddings) = result {
                // Convert to WIT response format
                return Ok(exports::golem::embed::embed::EmbeddingResponse {
                    embeddings: embeddings.into_iter()
                        .map(|(index, vector)| exports::golem::embed::embed::Embedding {
                            index,
                            vector,
                        })
                        .collect(),
                    usage: Some(exports::golem::embed::embed::Usage {
                        input_tokens: None, // We don't have this info from the durable operation
                        total_tokens: None,
                    }),
                    model: config.model.unwrap_or_else(|| "text-embedding-3-large".to_string()),
                    provider_metadata_json: None,
                });
            }
        }

        // Call OpenAI API directly if durability is not enabled or if we didn't get a result from the durable operation
        let response = futures::executor::block_on(self.create_embeddings(text_inputs, Some(&config)))?;

        // Convert response to WIT format
        Ok(exports::golem::embed::embed::EmbeddingResponse {
            embeddings: response.data.into_iter()
                .map(|emb| exports::golem::embed::embed::Embedding {
                    index: emb.index,
                    vector: emb.embedding,
                })
                .collect(),
            usage: Some(exports::golem::embed::embed::Usage {
                input_tokens: Some(response.usage.prompt_tokens),
                total_tokens: Some(response.usage.total_tokens),
            }),
            model: response.model,
            provider_metadata_json: Some(serde_json::json!({
                "object": response.object
            }).to_string()),
        })
    }

    fn rerank(
        &mut self,
        _query: String,
        _documents: Vec<String>, 
        _config: exports::golem::embed::embed::Config,
    ) -> Result<exports::golem::embed::embed::RerankResponse, exports::golem::embed::embed::Error> {
        Err(EmbedError::Unsupported("Reranking not supported by OpenAI embedding models".into()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exports::golem::embed::embed::{Config, ContentPart};

    #[test]
    fn test_validate_config() {
        let embedder = Embedder::new().unwrap();
        
        // Test valid config
        let config = Config {
            model: Some("text-embedding-3-large".to_string()),
            task_type: Some(exports::golem::embed::embed::TaskType::SemanticSimilarity),
            dimensions: None,
            truncation: Some(true),
            output_format: Some(exports::golem::embed::embed::OutputFormat::FloatArray),
            output_dtype: Some(exports::golem::embed::embed::OutputDtype::Float32),
            user: Some("test-user".to_string()),
            provider_options: vec![],
        };
        assert!(embedder.validate_config(&config).is_ok());

        // Test invalid output format
        let config = Config {
            output_format: Some(exports::golem::embed::embed::OutputFormat::Binary),
            ..config
        };
        assert!(matches!(
            embedder.validate_config(&config),
            Err(EmbedError::Unsupported(_))
        ));

        // Test invalid dtype
        let config = Config {
            output_dtype: Some(exports::golem::embed::embed::OutputDtype::Int8),
            ..config
        };
        assert!(matches!(
            embedder.validate_config(&config),
            Err(EmbedError::Unsupported(_))
        ));
    }

    #[test]
    fn test_image_input_rejected() {
        let mut embedder = Embedder::new().unwrap();
        let config = Config {
            model: Some("text-embedding-3-large".to_string()),
            task_type: None,
            dimensions: None,
            truncation: None,
            output_format: None,
            output_dtype: None,
            user: None,
            provider_options: vec![],
        };

        let result = embedder.generate(
            vec![ContentPart::Image(exports::golem::embed::embed::ImageUrl {
                url: "https://example.com/image.jpg".to_string(),
            })],
            config,
        );

        assert!(matches!(
            result,
            Err(exports::golem::embed::embed::Error::Unsupported { .. })
        ));
    }

    #[test]
    fn test_rerank_unsupported() {
        let mut embedder = Embedder::new().unwrap();
        let config = Config {
            model: None,
            task_type: None,
            dimensions: None,
            truncation: None,
            output_format: None,
            output_dtype: None,
            user: None,
            provider_options: vec![],
        };

        let result = embedder.rerank(
            "query".to_string(),
            vec!["doc1".to_string(), "doc2".to_string()],
            config,
        );

        assert!(matches!(
            result,
            Err(exports::golem::embed::embed::Error::Unsupported { .. })
        ));
    }
    
    #[cfg(feature = "durability")]
    mod durability_tests {
        use super::*;
        use std::sync::Arc;
        
        #[test]
        fn test_create_durable_embedding() {
            dotenv::dotenv().ok();
            if env::var("OPENAI_API_KEY").is_err() {
                eprintln!("Skipping test_create_durable_embedding: OPENAI_API_KEY not set");
                return;
            }
            
            let embedder = Embedder::new().unwrap();
            let operation = Operation::Embed(EmbeddingOperation {
                texts: vec!["Test input".to_string()],
                model: Some("text-embedding-3-small".to_string()),
                dimensions: Some(768),
                user: None,
            });
            
            let pollable = futures::executor::block_on(embedder.create_durable_embedding(operation)).unwrap();
            assert!(Arc::strong_count(&pollable) >= 1);
        }
    }
}