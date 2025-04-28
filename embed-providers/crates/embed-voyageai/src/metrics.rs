use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct VoyageMetrics {
    total_requests: AtomicU64,
    total_errors: AtomicU64,
    total_tokens: AtomicU64,
    total_latency_ms: AtomicU64,
}

impl VoyageMetrics {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
        }
    }

    pub fn record_request(&self, duration: Duration, token_count: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_tokens.fetch_add(token_count, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(duration.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    pub fn total_errors(&self) -> u64 {
        self.total_errors.load(Ordering::Relaxed)
    }

    pub fn total_tokens(&self) -> u64 {
        self.total_tokens.load(Ordering::Relaxed)
    }

    pub fn average_latency(&self) -> f64 {
        let total_requests = self.total_requests.load(Ordering::Relaxed);
        if total_requests == 0 {
            0.0
        } else {
            self.total_latency_ms.load(Ordering::Relaxed) as f64 / total_requests as f64
        }
    }

    pub fn error_rate(&self) -> f64 {
        let total_requests = self.total_requests.load(Ordering::Relaxed);
        if total_requests == 0 {
            0.0
        } else {
            self.total_errors.load(Ordering::Relaxed) as f64 / total_requests as f64
        }
    }

    pub fn reset(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.total_errors.store(0, Ordering::Relaxed);
        self.total_tokens.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
    }
}

impl Default for VoyageMetrics {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Timer<'a> {
    metrics: &'a VoyageMetrics,
    start: Instant,
    token_count: u64,
}

impl<'a> Timer<'a> {
    pub fn new(metrics: &'a VoyageMetrics, token_count: u64) -> Self {
        Self {
            metrics,
            start: Instant::now(),
            token_count,
        }
    }
}

impl<'a> Drop for Timer<'a> {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        self.metrics.record_request(duration, self.token_count);
    }
}