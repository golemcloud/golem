use crate::{EmbeddingConfig, EmbeddingError, EmbeddingProvider};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct MockEmbeddingProvider {
    pub embeddings: HashMap<String, Vec<f32>>,
    pub rerank_scores: HashMap<String, f32>,
}

impl MockEmbeddingProvider {
    pub fn new() -> Self {
        Self {
            embeddings: HashMap::new(),
            rerank_scores: HashMap::new(),
        }
    }

    pub fn with_embedding(mut self, text: &str, embedding: Vec<f32>) -> Self {
        self.embeddings.insert(text.to_string(), embedding);
        self
    }

    pub fn with_rerank_score(mut self, doc: &str, score: f32) -> Self {
        self.rerank_scores.insert(doc.to_string(), score);
        self
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn generate_embeddings(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            let embedding = self.embeddings.get(&input).ok_or_else(|| {
                EmbeddingError::Internal(format!("No mock embedding for text: {}", input))
            })?;
            results.push(embedding.clone());
        }
        Ok(results)
    }

    async fn rerank(&self, _query: String, documents: Vec<String>) -> Result<Vec<(usize, f32)>, EmbeddingError> {
        let mut results: Vec<(usize, f32)> = documents
            .iter()
            .enumerate()
            .map(|(i, doc)| {
                let score = self.rerank_scores.get(doc).copied().unwrap_or(0.0);
                (i, score)
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        Ok(results)
    }

    async fn create_durable_embedding(
        &self,
        _operation: crate::durability::Operation,
    ) -> Result<std::sync::Arc<golem_api_1_x::durability::LazyInitializedPollable>, EmbeddingError> {
        unimplemented!("Mock doesn't support durability")
    }

    async fn poll_durable_request(
        &self,
        _request: &crate::durability::DurableRequest<Vec<Vec<f32>>>,
    ) -> Result<Option<Vec<Vec<f32>>>, EmbeddingError> {
        unimplemented!("Mock doesn't support durability")
    }
}

pub async fn test_embedding_provider<T: EmbeddingProvider>(provider: &T) -> Result<(), EmbeddingError> {
    // Test basic embedding generation
    let inputs = vec![
        "This is a test sentence".to_string(),
        "Another test sentence".to_string(),
    ];
    
    let embeddings = provider.generate_embeddings(inputs).await?;
    assert!(!embeddings.is_empty(), "Should return embeddings");
    assert_eq!(embeddings.len(), 2, "Should return correct number of embeddings");

    // Test reranking
    let query = "test query".to_string();
    let documents = vec![
        "First document".to_string(),
        "Second document".to_string(),
        "Third document".to_string(),
    ];

    let rankings = provider.rerank(query, documents).await?;
    assert!(!rankings.is_empty(), "Should return rankings");
    assert_eq!(rankings.len(), 3, "Should return correct number of rankings");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_provider() {
        let mock = MockEmbeddingProvider::new()
            .with_embedding("test", vec![1.0, 2.0, 3.0])
            .with_rerank_score("doc1", 0.9)
            .with_rerank_score("doc2", 0.5);

        let embeddings = mock.generate_embeddings(vec!["test".to_string()])
            .await
            .unwrap();
        assert_eq!(embeddings[0], vec![1.0, 2.0, 3.0]);

        let rankings = mock.rerank("query".to_string(), vec!["doc1".to_string(), "doc2".to_string()])
            .await
            .unwrap();
        assert_eq!(rankings, vec![(0, 0.9), (1, 0.5)]);
    }
}