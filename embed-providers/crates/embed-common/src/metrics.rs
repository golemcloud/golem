use std::time::Instant;
use std::sync::atomic::{AtomicU64, Ordering};

/// Metrics for tracking embedding and reranking operations
#[derive(Default)]
pub struct EmbeddingMetrics {
    total_embedding_requests: AtomicU64,
    total_rerank_requests: AtomicU64,
    total_errors: AtomicU64,
    total_tokens_processed: AtomicU64,
}

impl EmbeddingMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_embedding_request(&self, token_count: u64, duration: std::time::Duration) {
        self.total_embedding_requests.fetch_add(1, Ordering::Relaxed);
        self.total_tokens_processed.fetch_add(token_count, Ordering::Relaxed);
    }

    pub fn record_rerank_request(&self, token_count: u64, duration: std::time::Duration) {
        self.total_rerank_requests.fetch_add(1, Ordering::Relaxed);
        self.total_tokens_processed.fetch_add(token_count, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_total_embedding_requests(&self) -> u64 {
        self.total_embedding_requests.load(Ordering::Relaxed)
    }

    pub fn get_total_rerank_requests(&self) -> u64 {
        self.total_rerank_requests.load(Ordering::Relaxed)
    }

    pub fn get_total_errors(&self) -> u64 {
        self.total_errors.load(Ordering::Relaxed)
    }

    pub fn get_total_tokens_processed(&self) -> u64 {
        self.total_tokens_processed.load(Ordering::Relaxed)
    }
}

/// Timer guard for measuring operation duration
pub struct TimerGuard {
    start: Instant,
    metrics: Option<EmbeddingMetrics>,
    token_count: u64,
    is_embedding: bool,
}

impl TimerGuard {
    pub fn new_embedding(metrics: &EmbeddingMetrics, token_count: u64) -> Self {
        Self {
            start: Instant::now(),
            metrics: Some(metrics.clone()),
            token_count,
            is_embedding: true,
        }
    }

    pub fn new_rerank(metrics: &EmbeddingMetrics, token_count: u64) -> Self {
        Self {
            start: Instant::now(),
            metrics: Some(metrics.clone()),
            token_count,
            is_embedding: false,
        }
    }
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        if let Some(metrics) = &self.metrics {
            let duration = self.start.elapsed();
            if self.is_embedding {
                metrics.record_embedding_request(self.token_count, duration);
            } else {
                metrics.record_rerank_request(self.token_count, duration);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_metrics_recording() {
        let metrics = EmbeddingMetrics::new();

        // Record some test metrics
        metrics.record_embedding_request(100, Duration::from_millis(50));
        metrics.record_rerank_request(50, Duration::from_millis(30));
        metrics.record_error();

        assert_eq!(metrics.get_total_embedding_requests(), 1);
        assert_eq!(metrics.get_total_rerank_requests(), 1);
        assert_eq!(metrics.get_total_errors(), 1);
        assert_eq!(metrics.get_total_tokens_processed(), 150);
    }

    #[test]
    fn test_concurrent_metrics() {
        let metrics = EmbeddingMetrics::new();
        let metrics_ref = &metrics;

        let handles: Vec<_> = (0..10)
            .map(|_| {
                thread::spawn(move || {
                    metrics_ref.record_embedding_request(10, Duration::from_millis(10));
                    metrics_ref.record_rerank_request(5, Duration::from_millis(5));
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(metrics.get_total_embedding_requests(), 10);
        assert_eq!(metrics.get_total_rerank_requests(), 10);
        assert_eq!(metrics.get_total_tokens_processed(), 150);
    }

    #[test]
    fn test_timer_guard() {
        let metrics = EmbeddingMetrics::new();
        
        {
            let _guard = TimerGuard::new_embedding(&metrics, 100);
            thread::sleep(Duration::from_millis(10));
        }

        {
            let _guard = TimerGuard::new_rerank(&metrics, 50);
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(metrics.get_total_embedding_requests(), 1);
        assert_eq!(metrics.get_total_rerank_requests(), 1);
        assert_eq!(metrics.get_total_tokens_processed(), 150);
    }
}