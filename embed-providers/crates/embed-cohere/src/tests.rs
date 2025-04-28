use super::*;
use embed_common::{
    durability::{EmbeddingOperation, Operation},
    testing::{
        test_embedding_provider, test_provider_interop, test_rate_limiting, test_concurrent_limits,
        MockEmbeddingProvider,
    },
};
use std::env;
use tokio::time::Duration;

async fn create_test_client() -> CohereClient {
    // For CI testing, skip if no API key available
    if env::var("COHERE_API_KEY").is_err() {
        env::set_var("COHERE_API_KEY", "dummy_key_for_ci");
    }
    CohereClient::new().unwrap()
}

#[tokio::test]
async fn test_provider_interface() {
    let client = create_test_client().await;
    test_embedding_provider(&client).await.unwrap();
}

#[tokio::test]
async fn test_durability() {
    let client = create_test_client().await;
    
    // Test embedding durability
    let operation = Operation::Embed(EmbeddingOperation {
        texts: vec!["This is a test".to_string()],
        model: None,
        truncate: true,
    });

    let pollable = client.create_durable_embedding(operation).await.unwrap();
    let request = DurableRequest::new(
        Operation::Embed(EmbeddingOperation {
            texts: vec!["This is a test".to_string()],
            model: None,
            truncate: true,
        }),
        pollable,
    );

    // Poll until result available or timeout
    let mut result = None;
    for _ in 0..10 {
        if let Some(embeddings) = client.poll_durable_request(&request).await.unwrap() {
            result = Some(embeddings);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(result.is_some(), "Should get embedding result");
    let embeddings = result.unwrap();
    assert!(!embeddings.is_empty(), "Should have embeddings");
    assert_eq!(embeddings.len(), 1, "Should have one embedding");
}

#[tokio::test]
async fn test_error_handling() {
    let client = create_test_client().await;
    
    // Test invalid model name
    let config = EmbeddingConfig {
        model: Some("invalid-model".to_string()),
        dimensions: None,
        truncate: true,
    };

    let result = client.embed(vec!["test".to_string()], &config).await;
    assert!(matches!(result, Err(EmbeddingError::ModelNotFound(_))));

    // Test empty input
    let result = client.generate_embeddings(vec![]).await;
    assert!(matches!(result, Err(EmbeddingError::InvalidRequest(_))));
}

#[tokio::test]
async fn test_mock_provider_interop() {
    let cohere = create_test_client().await;
    let mock = MockEmbeddingProvider::new()
        .with_embedding("test", vec![1.0; 1024]) // Match Cohere's dimension
        .with_rerank_score("doc1", 0.9);

    // Test interop with mock provider
    test_provider_interop(&cohere, &mock).await.unwrap();
}

#[tokio::test]
async fn test_usage_tracking() {
    let client = create_test_client().await;
    
    let config = EmbeddingConfig {
        model: None,
        dimensions: None,
        truncate: true,
    };

    let response = client.embed(vec!["Usage tracking test".to_string()], &config).await.unwrap();
    
    // Verify that usage metadata is captured
    assert!(response.meta.billable_tokens.is_some(), "Should track token usage");
}

#[tokio::test]
async fn test_concurrent_requests() {
    let client = create_test_client().await;
    
    let futures = (0..3).map(|i| {
        let texts = vec![format!("Concurrent test {}", i)];
        client.generate_embeddings(texts)
    });

    let results = futures::future::join_all(futures).await;
    
    // Verify all requests succeeded
    for result in results {
        assert!(result.is_ok(), "Concurrent request should succeed");
    }
}

#[tokio::test]
async fn test_rate_limits() {
    let client = create_test_client().await;
    test_rate_limiting(&client).await.unwrap();
}

#[tokio::test]
async fn test_concurrent_request_limits() {
    let client = create_test_client().await;
    test_concurrent_limits(&client).await.unwrap();
}

#[tokio::test]
async fn test_rate_limiting() {
    let client = setup_test_client();
    let config = EmbeddingConfig::default();
    
    // Send multiple requests in parallel to test rate limiting
    let futures: Vec<_> = (0..10)
        .map(|_| client.embed(vec!["test text".to_string()], &config))
        .collect();
    
    let results = futures::future::join_all(futures).await;
    
    // Verify all requests succeeded
    for result in results {
        assert!(result.is_ok());
    }
    
    // Verify metrics were collected
    let metrics = client.get_metrics();
    assert!(metrics.total_requests() > 0);
    assert!(metrics.total_tokens() > 0);
    assert_eq!(metrics.total_errors(), 0);
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
    
    // Test error metrics
    let result = client
        .embed(vec![], &config)
        .await;
    assert!(result.is_err());
    
    let metrics = client.get_metrics();
    assert_eq!(metrics.total_requests(), 2);
    assert_eq!(metrics.total_errors(), 1);
}