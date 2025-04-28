use crate::{execute_async, VoyageAIClient};
use golem_embed as ge;
use std::str::FromStr;

struct Component;

impl ge::embed::Guest for Component {
    fn generate(inputs: Vec<ge::embed::ContentPart>, config: ge::embed::Config) -> Result<ge::embed::EmbeddingResponse, ge::embed::Error> {
        let texts = inputs.into_iter().map(|part| match part {
            ge::embed::ContentPart::Text(text) => Ok(text),
            ge::embed::ContentPart::Image(_) => Err(ge::embed::Error {
                code: ge::embed::ErrorCode::Unsupported,
                message: "Image embeddings are not supported by Voyage AI".to_string(),
                provider_error_json: None,
            })
        }).collect::<Result<Vec<String>, ge::embed::Error>>()?;

        let client = match VoyageAIClient::new() {
            Ok(client) => client,
            Err(err) => return Err(ge::embed::Error {
                code: ge::embed::ErrorCode::ProviderError,
                message: err.to_string(),
                provider_error_json: None,
            })
        };

        let config = crate::EmbeddingConfig {
            model: config.model,
            dimensions: config.dimensions,
            truncate: config.truncation.unwrap_or(true),
        };

        match execute_async(client.embed(texts, &config)) {
            Ok(response) => Ok(ge::embed::EmbeddingResponse {
                embeddings: response.embeddings.into_iter().enumerate().map(|(i, vector)| {
                    ge::embed::Embedding {
                        index: i as u32,
                        vector,
                    }
                }).collect(),
                usage: Some(ge::embed::Usage {
                    input_tokens: Some(response.usage.prompt_tokens),
                    total_tokens: Some(response.usage.total_tokens),
                }),
                model: Some(response.model),
                provider_metadata_json: None,
            }),
            Err(err) => Err(ge::embed::Error {
                code: match err {
                    crate::EmbeddingError::InvalidRequest(_) => ge::embed::ErrorCode::InvalidRequest,
                    crate::EmbeddingError::ModelNotFound(_) => ge::embed::ErrorCode::ModelNotFound, 
                    crate::EmbeddingError::ProviderError(_) => ge::embed::ErrorCode::ProviderError,
                    crate::EmbeddingError::RateLimitExceeded => ge::embed::ErrorCode::RateLimitExceeded,
                    crate::EmbeddingError::Internal(_) => ge::embed::ErrorCode::InternalError,
                    crate::EmbeddingError::Durability(_) => ge::embed::ErrorCode::InternalError,
                },
                message: err.to_string(),
                provider_error_json: None,
            })
        }
    }

    fn rerank(query: String, documents: Vec<String>, config: ge::embed::Config) -> Result<ge::embed::RerankResponse, ge::embed::Error> {
        let client = match VoyageAIClient::new() {
            Ok(client) => client,
            Err(err) => return Err(ge::embed::Error {
                code: ge::embed::ErrorCode::ProviderError,
                message: err.to_string(),
                provider_error_json: None,
            })
        };

        let config = crate::EmbeddingConfig {
            model: config.model.or(Some("voyage-rerank-01".to_string())),
            dimensions: config.dimensions,
            truncate: config.truncation.unwrap_or(true),
        };

        match execute_async(client.rerank_documents(query, documents, &config)) {
            Ok(response) => Ok(ge::embed::RerankResponse {
                results: response.results.into_iter().map(|result| {
                    ge::embed::RerankResult {
                        index: result.index as u32,
                        relevance_score: result.relevance_score,
                        document: Some(result.document),
                    }
                }).collect(),
                usage: Some(ge::embed::Usage {
                    input_tokens: Some(response.usage.prompt_tokens),
                    total_tokens: Some(response.usage.total_tokens),
                }),
                model: Some(response.model),
                provider_metadata_json: None,
            }),
            Err(err) => Err(ge::embed::Error {
                code: match err {
                    crate::EmbeddingError::InvalidRequest(_) => ge::embed::ErrorCode::InvalidRequest,
                    crate::EmbeddingError::ModelNotFound(_) => ge::embed::ErrorCode::ModelNotFound,
                    crate::EmbeddingError::ProviderError(_) => ge::embed::ErrorCode::ProviderError,
                    crate::EmbeddingError::RateLimitExceeded => ge::embed::ErrorCode::RateLimitExceeded,
                    crate::EmbeddingError::Internal(_) => ge::embed::ErrorCode::InternalError,
                    crate::EmbeddingError::Durability(_) => ge::embed::ErrorCode::InternalError,
                },
                message: err.to_string(),
                provider_error_json: None,
            })
        }
    }
}