use super::*;
use embed_common::{
    durability::{EmbeddingOperation, Operation},
    testing::{test_embedding_provider, test_provider_interop, test_rate_limiting, test_concurrent_limits, MockEmbeddingProvider},
};
use std::env;
use tokio::time::Duration;

async fn create_test_client() -> HuggingFaceClient {
    // For CI testing, skip if no API key available
    if env::var("HUGGINGFACE_API_KEY").is_err() {
        env::set_var("HUGGINGFACE_API_KEY", "dummy_key_for_ci");
    }
    HuggingFaceClient::new().unwrap()
}

#[tokio::test]
async fn test_provider_interface() {
    let client = create_test_client().await;
    test_embedding_provider(&client).await.unwrap();
}

#[tokio::test]
async fn test_durability() {
    let client = create_test_client().await;
    
    // Test embedding durability with default model
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
async fn test_custom_model() {
    let client = create_test_client().await;
    
    // Test with a specific sentence transformer model
    let config = EmbeddingConfig {
        model: Some("sentence-transformers/all-mpnet-base-v2".to_string()),
        dimensions: None,
        truncate: true,
    };

    let result = client.embed(vec!["Custom model test".to_string()], &config).await;
    assert!(result.is_ok(), "Should work with custom model");
}

#[tokio::test]
async fn test_error_handling() {
    let client = create_test_client().await;
    
    // Test invalid model name
    let config = EmbeddingConfig {
        model: Some("invalid-model-name".to_string()),
        dimensions: None,
        truncate: true,
    };

    let result = client.embed(vec!["test".to_string()], &config).await;
    assert!(matches!(result, Err(EmbeddingError::ModelNotFound(_))));

    // Test empty input
    let result = client.generate_embeddings(vec![]).await;
    assert!(matches!(result, Err(EmbeddingError::InvalidRequest(_))));

    // Test with cross-encoder for embeddings (should fail)
    let config = EmbeddingConfig {
        model: Some("cross-encoder/ms-marco-MiniLM-L-6-v2".to_string()),
        dimensions: None,
        truncate: true,
    };

    let result = client.embed(vec!["test".to_string()], &config).await;
    assert!(matches!(result, Err(EmbeddingError::ProviderError(_))));
}

#[tokio::test]
async fn test_mock_provider_interop() {
    let hf = create_test_client().await;
    let mock = MockEmbeddingProvider::new()
        .with_embedding("test", vec![1.0; 384]) // Match MiniLM-L6's dimension
        .with_rerank_score("doc1", 0.9);

    // Test interop with mock provider
    test_provider_interop(&hf, &mock).await.unwrap();
}

#[tokio::test]
async fn test_model_validation() {
    let client = create_test_client().await;
    
    // Test with various model types
    let models = vec![
        "sentence-transformers/all-MiniLM-L6-v2",  // Should work
        "sentence-transformers/all-mpnet-base-v2", // Should work
        "cross-encoder/ms-marco-MiniLM-L-6-v2",   // Should fail for embeddings
    ];

    for model in models {
        let config = EmbeddingConfig {
            model: Some(model.to_string()),
            dimensions: None,
            truncate: true,
        };

        let result = client.embed(vec!["Model test".to_string()], &config).await;
        
        match model {
            m if m.starts_with("sentence-transformers/") => {
                assert!(result.is_ok(), "Sentence transformer model should work for embeddings");
            }
            m if m.starts_with("cross-encoder/") => {
                assert!(result.is_err(), "Cross-encoder should not work for embeddings");
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn test_batch_processing() {
    let client = create_test_client().await;
    
    // Test with different batch sizes
    let batch_sizes = vec![1, 2, 5];
    
    for size in batch_sizes {
        let texts: Vec<String> = (0..size)
            .map(|i| format!("Batch test text {}", i))
            .collect();
            
        let result = client.generate_embeddings(texts.clone()).await;
        assert!(result.is_ok(), "Should handle batch size {}", size);
        
        let embeddings = result.unwrap();
        assert_eq!(embeddings.len(), size, "Should return correct number of embeddings");
    }
}

#[tokio::test]
async fn test_cross_encode_ranking() {
    let client = create_test_client().await;
    
    let query = "What is machine learning?";
    let documents = vec![
        "Machine learning is a branch of artificial intelligence".to_string(),
        "The history of ancient civilizations".to_string(),
        "Deep learning and neural networks".to_string(),
    ];
    
    let config = EmbeddingConfig {
        model: Some("cross-encoder/ms-marco-MiniLM-L-6-v2".to_string()),
        dimensions: None,
        truncate: true,
    };

    let result = client.cross_encode(
        query.to_string(),
        documents.clone(),
        &config
    ).await.unwrap();
    
    // First document should be most relevant
    assert_eq!(result[0].0, 0, "Most relevant document should be the ML definition");
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
async fn test_cross_encoder_models() {
    let client = create_test_client().await;
    let query = "What is machine learning?";
    let documents = vec![
        "Machine learning is a subset of AI".to_string(),
        "Artificial intelligence encompasses many fields".to_string(),
    ];

    let models = vec![
        "cross-encoder/ms-marco-MiniLM-L-6-v2",
        "cross-encoder/stsb-TinyBERT-L-4",
    ];

    for model in models {
        let config = EmbeddingConfig {
            model: Some(model.to_string()),
            dimensions: None,
            truncate: true,
        };

        let result = client.cross_encode(
            query.to_string(),
            documents.clone(),
            &config
        ).await;

        assert!(result.is_ok(), "Cross-encoder {} should work", model);
        let scores = result.unwrap();
        assert_eq!(scores.len(), 2, "Should score both documents");
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
async fn test_rate_limiting() {
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
async fn test_rerank_metrics() {
    let client = setup_test_client();
    let config = EmbeddingConfig::default();
    
    let query = "test query".to_string();
    let documents = vec!["test document 1".to_string(), "test document 2".to_string()];
    
    let result = client.cross_encode(query, documents, &config).await;
    assert!(result.is_ok());
    
    let metrics = client.get_metrics();
    assert_eq!(metrics.total_rerank_requests(), 1);
    assert!(metrics.total_rerank_tokens() > 0);
    assert_eq!(metrics.total_rerank_errors(), 0);
    assert!(metrics.average_rerank_latency() > 0.0);
}