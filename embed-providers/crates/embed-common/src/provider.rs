use async_trait::async_trait;
use crate::circuit_breaker::CircuitBreaker;
use crate::error::EmbedError;
use std::time::Duration;

#[async_trait]
pub trait Provider: Send + Sync {
    fn new(config: ProviderConfig) -> Self where Self: Sized;
    
    fn circuit_breaker(&self) -> &CircuitBreaker;
    
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        self.circuit_breaker()
            .execute(|| Box::pin(async move {
                self.do_embed(text).await
            }))
            .await
    }
    
    async fn do_embed(&self, text: &str) -> Result<Vec<f32>, EmbedError>;
}

#[derive(Clone)]
pub struct ProviderConfig {
    pub api_key: String,
    pub failure_threshold: u64,
    pub reset_timeout: Duration,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            failure_threshold: 3,
            reset_timeout: Duration::from_secs(60),
        }
    }
}