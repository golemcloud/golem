use crate::{EmbeddingProvider, EmbeddingError};
use std::time::Instant;
use tokio::time::Duration;

/// Test that rate limiting is working properly for a provider
pub async fn test_rate_limiting<T: EmbeddingProvider>(provider: &T) -> Result<(), EmbeddingError> {
    let texts = vec!["Rate limit test".to_string(); 1];
    let mut times = Vec::new();
    let start = Instant::now();

    // Send 10 requests in quick succession
    for _ in 0..10 {
        match provider.generate_embeddings(texts.clone()).await {
            Ok(_) => times.push(start.elapsed()),
            Err(EmbeddingError::RateLimitExceeded) => {
                // Expected behavior for some requests
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    // Verify we got fewer successful responses than requests due to rate limiting
    assert!(times.len() < 10, "Rate limiting should prevent all requests from succeeding immediately");

    // Check that later requests were delayed
    if let (Some(first), Some(last)) = (times.first(), times.last()) {
        assert!(
            last.as_secs_f32() - first.as_secs_f32() >= 1.0,
            "Later requests should be delayed by rate limiting"
        );
    }

    // Verify recovery after waiting
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    let recovery_result = provider.generate_embeddings(texts.clone()).await;
    assert!(recovery_result.is_ok(), "Should recover after waiting");

    Ok(())
}

/// Test that concurrent request limits are enforced
pub async fn test_concurrent_limits<T: EmbeddingProvider>(provider: &T) -> Result<(), EmbeddingError> {
    let texts = vec!["Concurrent limit test".to_string(); 1];
    let mut handles = Vec::new();
    let start = Instant::now();

    // Launch many concurrent requests
    for _ in 0..8 {
        let texts = texts.clone();
        let provider = provider.clone();
        handles.push(tokio::spawn(async move {
            let result = provider.generate_embeddings(texts).await;
            (result, start.elapsed())
        }));
    }

    let mut successful = 0;
    let mut rate_limited = 0;
    let mut completion_times = Vec::new();

    for handle in handles {
        match handle.await.unwrap() {
            (Ok(_), time) => {
                successful += 1;
                completion_times.push(time);
            }
            (Err(EmbeddingError::RateLimitExceeded), _) => {
                rate_limited += 1;
            }
            (Err(e), _) => return Err(e),
        }
    }

    // Verify some requests were rate limited
    assert!(rate_limited > 0, "Some requests should be rate limited");
    assert!(successful > 0, "Some requests should succeed");

    // Check that completion times are spread out
    completion_times.sort();
    if let (Some(first), Some(last)) = (completion_times.first(), completion_times.last()) {
        assert!(
            last.as_secs_f32() - first.as_secs_f32() >= 0.5,
            "Requests should be spread out due to rate limiting"
        );
    }

    Ok(())
}