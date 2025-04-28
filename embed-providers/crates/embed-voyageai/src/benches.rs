use crate::VoyageAIClient;
use embed_common::{
    benchmarking::{
        benchmark_embedding_latency, benchmark_rerank_latency, benchmark_throughput,
    },
    EmbeddingConfig,
};
use std::{env, time::Duration};
use tokio;

async fn create_bench_client() -> VoyageAIClient {
    if env::var("VOYAGE_API_KEY").is_err() {
        env::set_var("VOYAGE_API_KEY", "dummy_key_for_ci");
    }
    VoyageAIClient::new().unwrap()
}

#[tokio::test]
async fn benchmark_voyage_models() {
    let client = create_bench_client().await;
    let texts = vec!["This is a benchmark test".to_string()];
    
    let models = vec![
        "voyage-01",
        "voyage-lite-01",
        "voyage-large-02",
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
async fn benchmark_rerank_models() {
    let client = create_bench_client().await;
    let query = "artificial intelligence applications".to_string();
    let documents = vec![
        "AI and machine learning in modern software".to_string(),
        "The impact of artificial intelligence on society".to_string(),
        "Natural language processing advancements".to_string(),
        "Computer vision and image recognition".to_string(),
    ];

    let models = vec![
        "voyage-rerank-01",
        "voyage-rerank-02",
    ];

    for model in models {
        println!("\nBenchmarking rerank model: {}", model);
        
        let config = EmbeddingConfig {
            model: Some(model.to_string()),
            dimensions: None,
            truncate: true,
        };

        // Warm-up request
        let _ = client.rerank_documents(query.clone(), documents.clone(), &config).await;

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
async fn benchmark_encoding_throughput() {
    let client = create_bench_client().await;
    let base_text = "Benchmark test text".to_string();
    
    let batch_sizes = vec![1, 8, 32, 64];
    
    for size in batch_sizes {
        println!("\nBenchmarking batch size: {}", size);
        
        let texts = vec![base_text.clone(); size];
        let results = benchmark_throughput(&client, texts, Duration::from_secs(3))
            .await
            .unwrap();
            
        println!("Results (3 second test):");
        println!("Total requests: {}", results.total_requests);
        println!("Average latency: {:?}", results.average_latency);
        println!("Throughput: {:.2} req/s", results.throughput);
        println!("Error rate: {:.2}%", results.error_rate * 100.0);
        println!("Effective items/second: {:.2}", results.throughput * size as f64);
    }
}

#[tokio::test]
async fn benchmark_multilingual() {
    let client = create_bench_client().await;
    
    let texts = vec![
        "English text for benchmarking".to_string(),
        "Texto en español para pruebas".to_string(),
        "Texte français pour les tests".to_string(),
        "测试用的中文文本".to_string(),
        "テストのための日本語テキスト".to_string(),
    ];

    println!("\nBenchmarking multilingual performance");
    
    let results = benchmark_embedding_latency(&client, texts, 3)
        .await
        .unwrap();
        
    println!("Results:");
    println!("Languages processed: {}", results.batch_size);
    println!("Average latency: {:?}", results.average_latency);
    println!("Throughput: {:.2} req/s", results.throughput);
    println!("Error rate: {:.2}%", results.error_rate * 100.0);
}

#[tokio::test]
async fn benchmark_long_context() {
    let client = create_bench_client().await;
    let long_text = "This is a long text for benchmark testing. ".repeat(100);
    
    let texts = vec![long_text];
    
    println!("\nBenchmarking long context performance");
    
    let results = benchmark_embedding_latency(&client, texts, 3)
        .await
        .unwrap();
        
    println!("Results:");
    println!("Average latency: {:?}", results.average_latency);
    println!("Throughput: {:.2} req/s", results.throughput);
    println!("Error rate: {:.2}%", results.error_rate * 100.0);
}

use super::*;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use embed_common::EmbeddingConfig;

pub fn benchmark_embedding(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = VoyageAIClient::new().unwrap();
    
    let small_text = "This is a small test sentence.".to_string();
    let large_text = "This is a much larger test sentence that contains multiple words and should require more tokens to process and embed properly using the Voyage AI embeddings API. The goal is to test performance with varying input sizes.".to_string();
    
    let config = EmbeddingConfig {
        model: Some("voyage-01".to_string()),
        dimensions: None,
        truncate: true,
    };

    c.bench_function("embed_small_text", |b| {
        b.iter(|| {
            rt.block_on(async {
                client.embed(vec![black_box(small_text.clone())], &config).await.unwrap();
            })
        })
    });

    c.bench_function("embed_large_text", |b| {
        b.iter(|| {
            rt.block_on(async {
                client.embed(vec![black_box(large_text.clone())], &config).await.unwrap();
            })
        })
    });

    c.bench_function("embed_batch", |b| {
        b.iter(|| {
            rt.block_on(async {
                client.embed(vec![black_box(small_text.clone()); 5], &config).await.unwrap();
            })
        })
    });
}

criterion_group!(benches, benchmark_embedding);
criterion_main!(benches);