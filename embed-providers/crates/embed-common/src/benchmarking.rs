use crate::{EmbeddingError, EmbeddingProvider};
use std::time::{Duration, Instant};

pub struct BenchmarkResults {
    pub total_time: Duration,
    pub average_latency: Duration,
    pub throughput: f64,  // requests per second
    pub error_rate: f64,
    pub batch_size: usize,
    pub total_requests: usize,
}

pub async fn benchmark_embedding_latency<T: EmbeddingProvider>(
    provider: &T,
    texts: Vec<String>,
    num_iterations: usize,
) -> Result<BenchmarkResults, EmbeddingError> {
    let mut total_time = Duration::new(0, 0);
    let mut error_count = 0;
    let batch_size = texts.len();

    for _ in 0..num_iterations {
        let start = Instant::now();
        match provider.generate_embeddings(texts.clone()).await {
            Ok(_) => total_time += start.elapsed(),
            Err(_) => error_count += 1,
        }
    }

    let successful_requests = num_iterations - error_count;
    let average_latency = if successful_requests > 0 {
        total_time / successful_requests as u32
    } else {
        Duration::new(0, 0)
    };

    let throughput = if total_time.as_secs_f64() > 0.0 {
        successful_requests as f64 / total_time.as_secs_f64()
    } else {
        0.0
    };

    Ok(BenchmarkResults {
        total_time,
        average_latency,
        throughput,
        error_rate: error_count as f64 / num_iterations as f64,
        batch_size,
        total_requests: num_iterations,
    })
}

pub async fn benchmark_rerank_latency<T: EmbeddingProvider>(
    provider: &T,
    query: String,
    documents: Vec<String>,
    num_iterations: usize,
) -> Result<BenchmarkResults, EmbeddingError> {
    let mut total_time = Duration::new(0, 0);
    let mut error_count = 0;
    let batch_size = documents.len();

    for _ in 0..num_iterations {
        let start = Instant::now();
        match provider.rerank(query.clone(), documents.clone()).await {
            Ok(_) => total_time += start.elapsed(),
            Err(_) => error_count += 1,
        }
    }

    let successful_requests = num_iterations - error_count;
    let average_latency = if successful_requests > 0 {
        total_time / successful_requests as u32
    } else {
        Duration::new(0, 0)
    };

    let throughput = if total_time.as_secs_f64() > 0.0 {
        successful_requests as f64 / total_time.as_secs_f64()
    } else {
        0.0
    };

    Ok(BenchmarkResults {
        total_time,
        average_latency,
        throughput,
        error_rate: error_count as f64 / num_iterations as f64,
        batch_size,
        total_requests: num_iterations,
    })
}

pub async fn benchmark_throughput<T: EmbeddingProvider>(
    provider: &T,
    texts: Vec<String>,
    duration: Duration,
) -> Result<BenchmarkResults, EmbeddingError> {
    let start = Instant::now();
    let mut request_count = 0;
    let mut error_count = 0;
    let batch_size = texts.len();

    while start.elapsed() < duration {
        match provider.generate_embeddings(texts.clone()).await {
            Ok(_) => request_count += 1,
            Err(_) => error_count += 1,
        }
    }

    let total_time = start.elapsed();
    let successful_requests = request_count;
    let total_requests = request_count + error_count;

    let throughput = if total_time.as_secs_f64() > 0.0 {
        successful_requests as f64 / total_time.as_secs_f64()
    } else {
        0.0
    };

    Ok(BenchmarkResults {
        total_time,
        average_latency: if request_count > 0 {
            total_time / request_count as u32
        } else {
            Duration::new(0, 0)
        },
        throughput,
        error_rate: error_count as f64 / total_requests as f64,
        batch_size,
        total_requests,
    })
}