use crate::HuggingFaceClient;
use embed_common::{
    benchmarking::{
        benchmark_embedding_latency, benchmark_rerank_latency, benchmark_throughput,
    },
    EmbeddingConfig,
};
use std::{env, time::Duration};
use tokio;

async fn create_bench_client() -> HuggingFaceClient {
    if env::var("HUGGINGFACE_API_KEY").is_err() {
        env::set_var("HUGGINGFACE_API_KEY", "dummy_key_for_ci");
    }
    HuggingFaceClient::new().unwrap()
}

#[tokio::test]
async fn benchmark_embedding_models() {
    let client = create_bench_client().await;
    let texts = vec!["This is a benchmark test".to_string()];
    
    let models = vec![
        "sentence-transformers/all-MiniLM-L6-v2",
        "sentence-transformers/all-mpnet-base-v2",
    ];

    for model in models {
        println!("\nBenchmarking model: {}", model);
        
        let config = EmbeddingConfig {
            model: Some(model.to_string()),
            dimensions: None,
            truncate: true,
        };

        // Warm-up request
        let _ = client.embed(texts.clone(), &config).await;

        // Actual benchmark
        let results = benchmark_embedding_latency(&client, texts.clone(), 3)
            .await
            .unwrap();
            
        println!("Results:");
        println!("Average latency: {:?}", results.average_latency);
        println!("Throughput: {:.2} req/s", results.throughput);
        println!("Error rate: {:.2}%", results.error_rate * 100.0);
    }
}

#[tokio::test]
async fn benchmark_batch_sizes() {
    let client = create_bench_client().await;
    let base_text = "Benchmark test text".to_string();
    
    let batch_sizes = vec![1, 4, 8, 16];
    
    for size in batch_sizes {
        println!("\nBenchmarking batch size: {}", size);
        
        let texts = vec![base_text.clone(); size];
        let results = benchmark_embedding_latency(&client, texts, 3)
            .await
            .unwrap();
            
        println!("Results:");
        println!("Average latency: {:?}", results.average_latency);
        println!("Throughput: {:.2} req/s", results.throughput);
        println!("Error rate: {:.2}%", results.error_rate * 100.0);
        println!("Latency per item: {:?}", results.average_latency / size as u32);
    }
}

#[tokio::test]
async fn benchmark_cross_encoders() {
    let client = create_bench_client().await;
    let query = "artificial intelligence".to_string();
    let documents = vec![
        "Machine learning and AI applications".to_string(),
        "Deep learning architectures".to_string(),
        "Neural networks in practice".to_string(),
        "Transformers and attention mechanisms".to_string(),
    ];

    let models = vec![
        "cross-encoder/ms-marco-MiniLM-L-6-v2",
        "cross-encoder/stsb-TinyBERT-L-4",
    ];

    for model in models {
        println!("\nBenchmarking cross-encoder: {}", model);
        
        let config = EmbeddingConfig {
            model: Some(model.to_string()),
            dimensions: None,
            truncate: true,
        };

        // Warm-up request
        let _ = client.cross_encode(query.clone(), documents.clone(), &config).await;

        // Actual benchmark
        let results = benchmark_rerank_latency(&client, query.clone(), documents.clone(), 3)
            .await
            .unwrap();
            
        println!("Results:");
        println!("Documents processed: {}", results.batch_size);
        println!("Average latency: {:?}", results.average_latency);
        println!("Throughput: {:.2} req/s", results.throughput);
        println!("Error rate: {:.2}%", results.error_rate * 100.0);
    }
}

#[tokio::test]
async fn benchmark_text_lengths() {
    let client = create_bench_client().await;
    let base_text = "This is a benchmark test sentence. ".repeat(10);
    
    let text_lengths = vec![
        ("short", base_text[..50].to_string()),
        ("medium", base_text[..200].to_string()),
        ("long", base_text.clone()),
    ];

    for (length_type, text) in text_lengths {
        println!("\nBenchmarking text length: {} ({} chars)", length_type, text.len());
        
        let texts = vec![text];
        let results = benchmark_embedding_latency(&client, texts, 3)
            .await
            .unwrap();
            
        println!("Results:");
        println!("Average latency: {:?}", results.average_latency);
        println!("Throughput: {:.2} req/s", results.throughput);
        println!("Error rate: {:.2}%", results.error_rate * 100.0);
    }
}