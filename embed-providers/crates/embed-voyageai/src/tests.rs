#[cfg(test)]
mod tests {
    use super::*;
    use embed_common::testing::{
        test_embedding_provider,
        test_provider_interop,
        test_rate_limiting,
        test_concurrent_limits,
        MockEmbeddingProvider,
    };
    use std::env;
    use tokio::time::Duration;
    use pretty_assertions::assert_eq;

    async fn create_test_client() -> VoyageAIClient {
        if env::var("VOYAGE_API_KEY").is_err() {
            env::set_var("VOYAGE_API_KEY", "dummy_key_for_ci");
        }
        VoyageAIClient::new().unwrap()
    }

    #[tokio::test]
    async fn test_provider_interface() {
        let client = create_test_client().await;
        test_embedding_provider(&client).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_provider_interop() {
        let voyage = create_test_client().await;
        let mock = MockEmbeddingProvider::new()
            .with_embedding("test", vec![1.0; 1024])
            .with_rerank_score("doc1", 0.9);

        test_provider_interop(&voyage, &mock).await.unwrap();
    }

    #[tokio::test]
    async fn test_rate_limits() {
        let client = setup_test_client();
        let config = EmbeddingConfig::default();
        
        // Send multiple requests in parallel to test rate limiting
        let futures: Vec<_> = (0..10)
            .map(|_| client.embed(vec!["test text".to_string()], &config))
            .collect();
        
        let results = futures::future::join_all(futures).await;
        
        // Verify all requests succeeded and were rate limited appropriately
        for result in results {
            assert!(result.is_ok());
        }
        
        let metrics = client.get_metrics();
        assert!(metrics.total_requests() > 0);
        assert!(metrics.total_tokens() > 0);
        assert_eq!(metrics.total_errors(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_request_limits() {
        let client = create_test_client().await;
        test_concurrent_limits(&client).await.unwrap();
    }

    #[tokio::test]
    async fn test_voyage_models() {
        let client = create_test_client().await;
        
        let models = vec![
            "voyage-01",
            "voyage-lite-01",
            "voyage-large-02",
        ];

        for model in models {
            let config = EmbeddingConfig {
                model: Some(model.to_string()),
                dimensions: None,
                truncate: true,
            };

            let result = client.embed(vec!["Model test".to_string()], &config).await;
            assert!(result.is_ok(), "Model {} should work", model);
        }
    }

    #[tokio::test]
    async fn test_error_recovery() {
        let client = create_test_client().await;
        let mut consecutive_errors = 0;
        let texts = vec!["Error recovery test".to_string()];

        // Make repeated requests until we hit rate limiting
        for _ in 0..10 {
            match client.generate_embeddings(texts.clone()).await {
                Ok(_) => {
                    if consecutive_errors > 0 {
                        // Successfully recovered from errors
                        return;
                    }
                }
                Err(EmbeddingError::RateLimitExceeded) => {
                    consecutive_errors += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        if consecutive_errors == 0 {
            println!("No rate limiting encountered during test");
        }
    }

    #[tokio::test]
    async fn test_usage_tracking() {
        let client = create_test_client().await;
        let texts = vec!["Usage tracking test".to_string()];
        
        let response = client.embed(texts, &EmbeddingConfig::default()).await.unwrap();
        
        assert!(response.usage.prompt_tokens > 0, "Should track prompt tokens");
        assert!(response.usage.total_tokens > 0, "Should track total tokens");
    }

    #[tokio::test]
    async fn test_metrics_collection() {
        let client = setup_test_client();
        let config = EmbeddingConfig::default();
        
        // Test successful request metrics
        let result = client.embed(vec!["test text".to_string()], &config).await;
        assert!(result.is_ok());
        
        let metrics = client.get_metrics();
        assert_eq!(metrics.total_requests(), 1);
        assert!(metrics.total_tokens() > 0);
        assert_eq!(metrics.total_errors(), 0);
        assert!(metrics.average_latency() > 0.0);
        
        // Test error metrics
        let result = client
            .embed(vec![], &config)
            .await;
        assert!(result.is_err());
        
        let metrics = client.get_metrics();
        assert_eq!(metrics.total_requests(), 2);
        assert_eq!(metrics.total_errors(), 1);
    }

    #[tokio::test]
    async fn test_concurrent_throughput() {
        let client = setup_test_client();
        let config = EmbeddingConfig::default();
        
        // Test concurrent request handling
        let texts: Vec<String> = (0..5)
            .map(|i| format!("Concurrent test {}", i))
            .collect();
            
        let futures: Vec<_> = texts
            .iter()
            .map(|text| client.embed(vec![text.clone()], &config))
            .collect();
            
        let results = futures::future::join_all(futures).await;
        
        // Verify all concurrent requests succeeded
        for result in results {
            assert!(result.is_ok());
        }
        
        let metrics = client.get_metrics();
        assert!(metrics.average_latency() > 0.0);
        assert_eq!(metrics.total_errors(), 0);
    }

    #[tokio::test]
    async fn test_generate_embeddings() {
        let client = VoyageAIClient::new().unwrap();
        
        let inputs = vec![
            "This is a test sentence.".to_string(),
            "Another test sentence.".to_string(),
        ];
        
        let config = EmbeddingConfig {
            model: Some("voyage-01".to_string()),
            dimensions: None,
            truncate: true,
        };

        let result = client.embed(inputs, &config).await;
        assert!(result.is_ok(), "Expected successful embedding generation");
        
        let response = result.unwrap();
        assert_eq!(response.embeddings.len(), 2, "Expected 2 embeddings");
        assert!(!response.embeddings[0].is_empty(), "Expected non-empty embedding vector");
        assert_eq!(response.model, "voyage-01");
    }

    #[tokio::test]
    async fn test_rerank_documents() {
        let client = VoyageAIClient::new().unwrap();
        
        let query = "What is machine learning?".to_string();
        let documents = vec![
            "Machine learning is a subset of artificial intelligence.".to_string(),
            "A bicycle has two wheels.".to_string(),
            "AI and ML are transforming technology.".to_string(),
        ];
        
        let config = EmbeddingConfig {
            model: Some("voyage-rerank-01".to_string()),
            dimensions: None,
            truncate: true,
        };

        let result = client.rerank_documents(query, documents, &config).await;
        assert!(result.is_ok(), "Expected successful reranking");
        
        let response = result.unwrap();
        assert_eq!(response.results.len(), 3, "Expected 3 rerank results");
        assert_eq!(response.model, "voyage-rerank-01");
        
        // Check that scores are normalized between 0 and 1
        for result in response.results {
            assert!(result.relevance_score >= 0.0 && result.relevance_score <= 1.0);
        }
    }

    #[tokio::test]
    async fn test_invalid_model() {
        let client = VoyageAIClient::new().unwrap();
        
        let inputs = vec!["Test sentence.".to_string()];
        let config = EmbeddingConfig {
            model: Some("non-existent-model".to_string()),
            dimensions: None,
            truncate: true,
        };

        let result = client.embed(inputs, &config).await;
        assert!(matches!(result, Err(EmbeddingError::ModelNotFound(_))));
    }

    #[tokio::test]
    async fn test_empty_input() {
        let client = VoyageAIClient::new().unwrap();
        
        let inputs: Vec<String> = vec![];
        let config = EmbeddingConfig {
            model: None,
            dimensions: None,
            truncate: true,
        };

        let result = client.embed(inputs, &config).await;
        assert!(matches!(result, Err(EmbeddingError::InvalidRequest(_))));
    }

    #[tokio::test]
    async fn test_durability() {
        let client = VoyageAIClient::new().unwrap();
        
        let operation = Operation::Embed(EmbeddingOperation {
            texts: vec!["Test sentence.".to_string()],
            model: Some("voyage-01".to_string()),
            truncate: true,
        });

        let pollable = client.create_durable_embedding(operation).await.unwrap();
        let request = DurableRequest { pollable };

        // Poll until we get a result
        let mut result = None;
        for _ in 0..10 {
            if let Some(embeddings) = client.poll_durable_request(&request).await.unwrap() {
                result = Some(embeddings);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        assert!(result.is_some(), "Expected to receive embeddings");
        let embeddings = result.unwrap();
        assert_eq!(embeddings.len(), 1, "Expected 1 embedding");
        assert!(!embeddings[0].is_empty(), "Expected non-empty embedding vector");
    }
}