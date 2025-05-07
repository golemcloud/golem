use crate::CohereClient;
use embed_common::benchmarking::{
    benchmark_embedding_latency, benchmark_rerank_latency, benchmark_throughput,
};
use std::{env, time::Duration};
use tokio;

async fn create_bench_client() -> CohereClient {
    if env::var("COHERE_API_KEY").is_err() {
        env::set_var("COHERE_API_KEY", "dummy_key_for_ci");
    }
    CohereClient::new().unwrap()
}

#[tokio::test]
async fn benchmark_single_embedding() {
    let client = create_bench_client().await;
    let texts = vec!["This is a benchmark test".to_string()];
    
    let results = benchmark_embedding_latency(&client, texts, 5)
        .await
        .unwrap();
        
    println!("Single embedding latency results:");
    println!("Average latency: {:?}", results.average_latency);
    println!("Throughput: {:.2} req/s", results.throughput);
    println!("Error rate: {:.2}%", results.error_rate * 100.0);
}

#[tokio::test]
async fn benchmark_batch_embedding() {
    let client = create_bench_client().await;
    let texts = vec![
        "First benchmark text".to_string(),
        "Second benchmark text".to_string(),
        "Third benchmark text".to_string(),
        "Fourth benchmark text".to_string(),
        "Fifth benchmark text".to_string(),
    ];
    
    let results = benchmark_embedding_latency(&client, texts, 3)
        .await
        .unwrap();
        
    println!("Batch embedding latency results:");
    println!("Batch size: {}", results.batch_size);
    println!("Average latency: {:?}", results.average_latency);
    println!("Throughput: {:.2} req/s", results.throughput);
    println!("Error rate: {:.2}%", results.error_rate * 100.0);
}

#[tokio::test]
async fn benchmark_reranking() {
    let client = create_bench_client().await;
    let query = "artificial intelligence".to_string();
    let documents = vec![
        "Machine learning is a subset of AI".to_string(),
        "Natural language processing in modern applications".to_string(),
        "The history of computer science".to_string(),
        "Impact of AI on society".to_string(),
    ];
    
    let results = benchmark_rerank_latency(&client, query, documents, 3)
        .await
        .unwrap();
        
    println!("Reranking latency results:");
    println!("Documents processed: {}", results.batch_size);
    println!("Average latency: {:?}", results.average_latency);
    println!("Throughput: {:.2} req/s", results.throughput);
    println!("Error rate: {:.2}%", results.error_rate * 100.0);
}

#[tokio::test]
async fn benchmark_sustained_throughput() {
    let client = create_bench_client().await;
    let texts = vec!["Throughput benchmark text".to_string()];
    
    let results = benchmark_throughput(&client, texts, Duration::from_secs(5))
        .await
        .unwrap();
        
    println!("Sustained throughput results (5 seconds):");
    println!("Total requests: {}", results.total_requests);
    println!("Average latency: {:?}", results.average_latency);
    println!("Throughput: {:.2} req/s", results.throughput);
    println!("Error rate: {:.2}%", results.error_rate * 100.0);
}