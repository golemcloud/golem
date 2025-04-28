use crate::OpenAIProvider;
use embed_common::{EmbeddingConfig, EmbeddingProvider};
use std::env;
use tokio_test::block_on;

#[test]
fn test_openai_provider_initialization() {
    // This test will be skipped if OPENAI_API_KEY is not set
    if env::var("OPENAI_API_KEY").is_err() {
        println!("Skipping test_openai_provider_initialization: OPENAI_API_KEY not set");
        return;
    }

    let provider = OpenAIProvider::new();
    assert!(provider.is_ok(), "Failed to initialize OpenAI provider");
}

#[test]
fn test_generate_embeddings() {
    // This test will be skipped if OPENAI_API_KEY is not set
    if env::var("OPENAI_API_KEY").is_err() {
        println!("Skipping test_generate_embeddings: OPENAI_API_KEY not set");
        return;
    }

    let provider = OpenAIProvider::new().expect("Failed to initialize OpenAI provider");
    let texts = vec!["Hello, world!".to_string(), "This is a test".to_string()];
    
    let result = block_on(provider.generate_embeddings(texts));
    assert!(result.is_ok(), "Failed to generate embeddings: {:?}", result.err());
    
    let embeddings = result.unwrap();
    assert_eq!(embeddings.len(), 2, "Expected 2 embeddings, got {}", embeddings.len());
    
    // Check that embeddings have reasonable dimensions
    assert!(!embeddings[0].is_empty(), "First embedding is empty");
    assert!(!embeddings[1].is_empty(), "Second embedding is empty");
}

#[test]
fn test_rerank() {
    // This test will be skipped if OPENAI_API_KEY is not set
    if env::var("OPENAI_API_KEY").is_err() {
        println!("Skipping test_rerank: OPENAI_API_KEY not set");
        return;
    }

    let provider = OpenAIProvider::new().expect("Failed to initialize OpenAI provider");
    let query = "What is machine learning?".to_string();
    let documents = vec![
        "Machine learning is a branch of artificial intelligence.".to_string(),
        "The weather is nice today.".to_string(),
        "Neural networks are used in deep learning.".to_string(),
    ];
    
    let result = block_on(provider.rerank(query, documents));
    assert!(result.is_ok(), "Failed to rerank documents: {:?}", result.err());
    
    let rankings = result.unwrap();
    assert_eq!(rankings.len(), 3, "Expected 3 rankings, got {}", rankings.len());
    
    // Check that rankings are sorted by relevance score (highest first)
    for i in 1..rankings.len() {
        assert!(rankings[i-1].1 >= rankings[i].1, 
                "Rankings not sorted by relevance score: {} < {}", 
                rankings[i-1].1, rankings[i].1);
    }
}

#[test]
fn test_durability() {
    // This test will be skipped if OPENAI_API_KEY is not set
    if env::var("OPENAI_API_KEY").is_err() {
        println!("Skipping test_durability: OPENAI_API_KEY not set");
        return;
    }

    let provider = OpenAIProvider::new().expect("Failed to initialize OpenAI provider");
    let texts = vec!["Hello, world!".to_string()];
    
    let operation = embed_common::durability::Operation::Embed(
        embed_common::durability::EmbeddingOperation {
            texts: texts.clone(),
            model: Some("text-embedding-3-small".to_string()),
            truncate: true,
        }
    );
    
    // Create durable request
    let durable_result = block_on(provider.create_durable_embedding(operation.clone()));
    assert!(durable_result.is_ok(), "Failed to create durable embedding: {:?}", durable_result.err());
    
    let pollable = durable_result.unwrap();
    let request = embed_common::durability::DurableRequest::new(operation, pollable);
    
    // Poll for result
    let poll_result = block_on(provider.poll_durable_request(&request));
    assert!(poll_result.is_ok(), "Failed to poll durable request: {:?}", poll_result.err());
    
    let embeddings_option = poll_result.unwrap();
    assert!(embeddings_option.is_some(), "No embeddings returned from durable request");
    
    let embeddings = embeddings_option.unwrap();
    assert_eq!(embeddings.len(), 1, "Expected 1 embedding, got {}", embeddings.len());
    assert!(!embeddings[0].is_empty(), "Embedding is empty");
}