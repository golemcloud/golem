use std::env;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Missing environment variable: {0}")]
    MissingEnv(String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = env::var("VOYAGE_API_KEY")
            .map_err(|_| ConfigError::MissingEnv("VOYAGE_API_KEY".to_string()))?;

        Ok(Config {
            api_key,
            base_url: env::var("VOYAGE_API_BASE")
                .unwrap_or_else(|_| "https://api.voyageai.com/v1".to_string()),
        })
    }
}