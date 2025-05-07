use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, sleep};

pub struct RateLimiter {
    semaphore: Arc<Semaphore>,
}

impl RateLimiter {
    pub fn new(max_concurrent: usize, requests_per_minute: u32) -> Self {
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let replenish_semaphore = semaphore.clone();
        
        // Start replenishment task
        tokio::spawn(async move {
            let interval = Duration::from_secs(60) / requests_per_minute;
            loop {
                sleep(interval).await;
                replenish_semaphore.add_permits(1);
            }
        });

        Self { semaphore }
    }

    pub async fn acquire(&self) {
        let _permit = self.semaphore.acquire().await.unwrap();
    }
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            semaphore: self.semaphore.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_rate_limiting() {
        let limiter = RateLimiter::new(2, 60); // 2 concurrent, 60 per minute
        let start = Instant::now();
        
        let mut handles = vec![];
        for _ in 0..3 {
            let limiter = limiter.clone();
            handles.push(tokio::spawn(async move {
                limiter.acquire().await;
            }));
        }

        // First two should complete quickly
        for handle in handles.iter().take(2) {
            tokio::select! {
                _ = handle => {},
                _ = sleep(Duration::from_millis(100)) => {
                    panic!("First two requests should complete quickly");
                }
            }
        }

        // Third should be delayed
        let elapsed = start.elapsed();
        assert!(elapsed.as_secs_f32() < 0.2, "First two requests should complete quickly");

        // Wait for third request
        handles[2].await.unwrap();
        let elapsed = start.elapsed();
        assert!(elapsed.as_secs_f32() >= 1.0, "Third request should be rate limited");
    }
}