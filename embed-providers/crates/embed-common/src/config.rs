use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingServiceConfig {
    pub rate_limit: RateLimitConfig,
    pub retry_policy: RetryConfig,
    pub providers: ProvidersConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub concurrent_requests: u32,
    pub max_tokens_per_minute: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub cohere: CohereConfig,
    pub huggingface: HuggingFaceConfig,
    pub voyageai: VoyageAIConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CohereConfig {
    pub api_key: String,
    pub default_model: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuggingFaceConfig {
    pub api_key: String,
    pub default_model: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoyageAIConfig {
    pub api_key: String,
    pub default_model: String,
    pub timeout: Duration,
}

impl Default for EmbeddingServiceConfig {
    fn default() -> Self {
        Self {
            rate_limit: RateLimitConfig {
                requests_per_minute: 600,
                concurrent_requests: 50,
                max_tokens_per_minute: 150_000,
            },
            retry_policy: RetryConfig {
                max_retries: 3,
                initial_backoff: Duration::from_secs(1),
                max_backoff: Duration::from_secs(30),
            },
            providers: ProvidersConfig {
                cohere: CohereConfig {
                    api_key: String::new(),
                    default_model: "embed-english-v3.0".to_string(),
                    timeout: Duration::from_secs(30),
                },
                huggingface: HuggingFaceConfig {
                    api_key: String::new(),
                    default_model: "sentence-transformers/all-mpnet-base-v2".to_string(),
                    timeout: Duration::from_secs(30),
                },
                voyageai: VoyageAIConfig {
                    api_key: String::new(),
                    default_model: "voyage-01".to_string(),
                    timeout: Duration::from_secs(30),
                },
            },
        }
    }
}

impl EmbeddingServiceConfig {
    pub fn from_env() -> Result<Self, config::ConfigError> {
        let mut cfg = config::Config::default();
        
        // Add env variables with prefix EMBED_
        cfg.merge(config::Environment::with_prefix("EMBED"))?;
        
        cfg.try_into()
    }
}