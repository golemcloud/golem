use std::time::Duration;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::{RedisConfig, RetryConfig};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct ShardManagerConfig {
    pub redis: RedisConfig,
    pub instance_server_service: WorkerExecutorServiceConfig,
    pub health_check: HealthCheckConfig,
    pub enable_json_log: bool,
    pub http_port: u16,
    pub number_of_shards: usize,
}

impl ShardManagerConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/shard-manager.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerExecutorServiceConfig {
    #[serde(with = "humantime_serde")]
    pub assign_shards_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub health_check_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub revoke_shards_timeout: Duration,
    pub retries: RetryConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(with = "humantime_serde")]
    pub delay: Duration,
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        // The following settings are always coming through environment variables:
        std::env::set_var("GOLEM__REDIS__HOST", "localhost");
        std::env::set_var("GOLEM__REDIS__PORT", "1234");
        std::env::set_var("GOLEM__REDIS__DATABASE", "1");
        std::env::set_var("GOLEM__ENABLE_JSON_LOG", "true");
        std::env::set_var("GOLEM__HTTP_PORT", "8080");

        // The rest can be loaded from the toml
        let _ = super::ShardManagerConfig::new();
    }
}
