use crate::EmbeddingError;
use tracing::{debug, error, info, warn};

pub fn log_embedding_request(texts: &[String], model: Option<&str>) {
    info!(
        texts_count = texts.len(),
        model = model.unwrap_or("default"),
        "Generating embeddings"
    );
    debug!(
        first_text = texts.first().map(|s| s.as_str()),
        "Sample input text"
    );
}

pub fn log_rerank_request(query: &str, documents: &[String], model: Option<&str>) {
    info!(
        query = query,
        docs_count = documents.len(),
        model = model.unwrap_or("default"),
        "Reranking documents"
    );
    debug!(
        first_doc = documents.first().map(|s| s.as_str()),
        "Sample document"
    );
}

pub fn log_embedding_error(error: &EmbeddingError) {
    match error {
        EmbeddingError::RateLimitExceeded => warn!("Rate limit exceeded"),
        EmbeddingError::InvalidRequest(msg) => warn!(error = msg, "Invalid request"),
        EmbeddingError::ModelNotFound(msg) => error!(error = msg, "Model not found"),
        EmbeddingError::ProviderError(msg) => error!(error = msg, "Provider error"),
        EmbeddingError::Unsupported(msg) => warn!(error = msg, "Unsupported operation"),
        EmbeddingError::Internal(msg) => error!(error = msg, "Internal error"),
        EmbeddingError::Durability(msg) => error!(error = msg, "Durability error"),
    }
}

pub fn log_embedding_success(embeddings: &[Vec<f32>], model: &str) {
    info!(
        embeddings_count = embeddings.len(),
        model = model,
        "Successfully generated embeddings"
    );
    if !embeddings.is_empty() {
        debug!(
            dimensions = embeddings[0].len(),
            "Embedding dimensions"
        );
    }
}

pub fn log_rerank_success(rankings: &[(usize, f32)], model: &str) {
    info!(
        rankings_count = rankings.len(),
        model = model,
        "Successfully reranked documents"
    );
    if !rankings.is_empty() {
        debug!(
            top_score = rankings[0].1,
            "Top relevance score"
        );
    }
}