use crate::{EmbeddingConfig, EmbeddingError, EmbeddingProvider};
use golem_embed::embed::{
    Config, ContentPart, EmbeddingResponse, Error, ErrorCode, TaskType, OutputFormat, OutputDtype,
};

pub async fn generate_embeddings<T: EmbeddingProvider>(
    provider: &T,
    inputs: Vec<ContentPart>,
    config: Config,
) -> Result<EmbeddingResponse, Error> {
    // Convert content parts to strings
    let texts = inputs.into_iter().map(|part| match part {
        ContentPart::Text(text) => Ok(text),
        ContentPart::Image(_) => Err(Error {
            code: ErrorCode::Unsupported,
            message: "Image inputs not supported".to_string(),
            provider_error_json: None,
        }),
    }).collect::<Result<Vec<_>, _>>()?;

    // Convert config
    let embedding_config = EmbeddingConfig {
        model: config.model,
        dimensions: config.dimensions,
        truncate: config.truncation.unwrap_or(true),
    };

    // Generate embeddings
    let result = provider.generate_embeddings(texts).await.map_err(|e| match e {
        EmbeddingError::InvalidRequest(msg) => Error {
            code: ErrorCode::InvalidRequest,
            message: msg,
            provider_error_json: None,
        },
        EmbeddingError::ModelNotFound(msg) => Error {
            code: ErrorCode::ModelNotFound,
            message: msg,
            provider_error_json: None,
        },
        EmbeddingError::Unsupported(msg) => Error {
            code: ErrorCode::Unsupported,
            message: msg,
            provider_error_json: None,
        },
        EmbeddingError::ProviderError(msg) => Error {
            code: ErrorCode::ProviderError,
            message: msg.clone(),
            provider_error_json: Some(msg),
        },
        EmbeddingError::RateLimitExceeded => Error {
            code: ErrorCode::RateLimitExceeded,
            message: "Rate limit exceeded".to_string(),
            provider_error_json: None,
        },
        EmbeddingError::Internal(msg) => Error {
            code: ErrorCode::InternalError,
            message: msg,
            provider_error_json: None,
        },
    })?;

    Ok(EmbeddingResponse {
        embeddings: result.into_iter().enumerate().map(|(i, v)| {
            golem_embed::embed::Embedding {
                index: i as u32,
                vector: v,
            }
        }).collect(),
        usage: None,
        model: "default".to_string(),
        provider_metadata_json: None,
    })
}

pub async fn rerank<T: EmbeddingProvider>(
    provider: &T,
    query: String,
    documents: Vec<String>,
    config: Config,
) -> Result<golem_embed::embed::RerankResponse, Error> {
    // Use provider to rerank
    let results = provider.rerank(query, documents).await.map_err(|e| match e {
        EmbeddingError::InvalidRequest(msg) => Error {
            code: ErrorCode::InvalidRequest,
            message: msg,
            provider_error_json: None,
        },
        EmbeddingError::ModelNotFound(msg) => Error {
            code: ErrorCode::ModelNotFound,
            message: msg,
            provider_error_json: None,
        },
        EmbeddingError::Unsupported(msg) => Error {
            code: ErrorCode::Unsupported,
            message: msg,
            provider_error_json: None,
        },
        EmbeddingError::ProviderError(msg) => Error {
            code: ErrorCode::ProviderError,
            message: msg.clone(),
            provider_error_json: Some(msg),
        },
        EmbeddingError::RateLimitExceeded => Error {
            code: ErrorCode::RateLimitExceeded,
            message: "Rate limit exceeded".to_string(),
            provider_error_json: None,
        },
        EmbeddingError::Internal(msg) => Error {
            code: ErrorCode::InternalError,
            message: msg,
            provider_error_json: None,
        },
    })?;

    Ok(golem_embed::embed::RerankResponse {
        results: results.into_iter().map(|(i, score)| {
            golem_embed::embed::RerankResult {
                index: i as u32,
                relevance_score: score,
                document: None,
            }
        }).collect(),
        usage: None,
        model: "default".to_string(),
        provider_metadata_json: None,
    })
}