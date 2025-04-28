use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

#[derive(Clone)]
pub struct CircuitBreaker {
    state: Arc<CircuitBreakerState>,
    failure_threshold: u64,
    reset_timeout: Duration,
}

struct CircuitBreakerState {
    is_open: AtomicBool,
    failure_count: AtomicU64,
    last_failure_time: AtomicU64,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u64, reset_timeout: Duration) -> Self {
        Self {
            state: Arc::new(CircuitBreakerState {
                is_open: AtomicBool::new(false),
                failure_count: AtomicU64::new(0),
                last_failure_time: AtomicU64::new(0),
            }),
            failure_threshold,
            reset_timeout,
        }
    }

    pub async fn execute<F, T, E>(&self, operation: F) -> Result<T, E>
    where
        F: FnOnce() -> futures::future::BoxFuture<'static, Result<T, E>>,
    {
        if self.is_open() {
            if self.should_retry() {
                self.state.is_open.store(false, Ordering::SeqCst);
            } else {
                return Err(std::convert::Into::into("Circuit breaker is open"));
            }
        }

        let result = operation().await;
        
        match &result {
            Ok(_) => {
                self.record_success();
            }
            Err(_) => {
                self.record_failure();
            }
        }

        result
    }

    fn is_open(&self) -> bool {
        self.state.is_open.load(Ordering::SeqCst)
    }

    fn should_retry(&self) -> bool {
        let last_failure = self.state.last_failure_time.load(Ordering::SeqCst);
        let now = Instant::now().elapsed().as_secs();
        
        now - last_failure >= self.reset_timeout.as_secs()
    }

    fn record_success(&self) {
        self.state.failure_count.store(0, Ordering::SeqCst);
        self.state.is_open.store(false, Ordering::SeqCst);
    }

    fn record_failure(&self) {
        let failures = self.state.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.last_failure_time.store(
            Instant::now().elapsed().as_secs(),
            Ordering::SeqCst,
        );

        if failures >= self.failure_threshold {
            self.state.is_open.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_circuit_breaker() {
        let circuit_breaker = CircuitBreaker::new(3, Duration::from_secs(5));
        
        // Test successful operation
        let result = circuit_breaker
            .execute(|| Box::pin(async { Ok::<_, &str>("success") }))
            .await;
        assert!(result.is_ok());
        
        // Test failures
        for _ in 0..3 {
            let result = circuit_breaker
                .execute(|| Box::pin(async { Err::<&str, _>("error") }))
                .await;
            assert!(result.is_err());
        }
        
        // Circuit should be open now
        let result = circuit_breaker
            .execute(|| Box::pin(async { Ok::<_, &str>("success") }))
            .await;
        assert!(result.is_err());
        
        // Wait for reset timeout
        sleep(Duration::from_secs(5)).await;
        
        // Circuit should allow retry
        let result = circuit_breaker
            .execute(|| Box::pin(async { Ok::<_, &str>("success") }))
            .await;
        assert!(result.is_ok());
    }
}