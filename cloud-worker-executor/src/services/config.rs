use std::time::Duration;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::RetryConfig;
use http::Uri;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Debug, Deserialize, Default)]
pub struct AdditionalGolemConfig {
    pub resource_limits: ResourceLimitsConfig,
}

impl AdditionalGolemConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/worker-executor.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }

    pub fn from_file(path: &str) -> Self {
        Figment::new()
            .merge(Toml::file(path))
            .extract()
            .expect("Failed to parse config")
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ResourceLimitsConfig {
    Grpc(ResourceLimitsGrpcConfig),
    Disabled,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResourceLimitsGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub batch_update_interval: Duration,
}

impl ResourceLimitsGrpcConfig {
    #[allow(unused)]
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse resource limits service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build resource limits service URI")
    }
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self::Grpc(ResourceLimitsGrpcConfig {
            host: "localhost".to_string(),
            port: 8080,
            access_token: "access_token".to_string(),
            retries: RetryConfig::default(),
            batch_update_interval: Duration::from_secs(60),
        })
    }
}

#[cfg(test)]
mod tests {
    use golem_worker_executor_base::services::golem_config::GolemConfig;

    use crate::services::config::AdditionalGolemConfig;

    #[test]
    pub fn config_is_loadable() {
        // The following settings are always coming through environment variables:
        std::env::set_var("GOLEM__REDIS__HOST", "localhost");
        std::env::set_var("GOLEM__REDIS__PORT", "1234");
        std::env::set_var("GOLEM__REDIS__DATABASE", "1");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__CONFIG__HOST", "localhost");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__CONFIG__PORT", "1234");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN", "token");
        std::env::set_var(
            "GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__REGION",
            "us-east-1",
        );
        std::env::set_var(
            "GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__BUCKET",
            "golem-compiled-components",
        );
        std::env::set_var(
            "GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__OBJECT_PREFIX",
            "",
        );
        std::env::set_var("GOLEM__BLOB_STORE_SERVICE__CONFIG__REGION", "us-east-1");
        std::env::set_var(
            "GOLEM__BLOB_STORE_SERVICE__CONFIG__BUCKET",
            "golem-compiled-components",
        );
        std::env::set_var("GOLEM__BLOB_STORE_SERVICE__CONFIG__OBJECT_PREFIX", "");
        std::env::set_var("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST", "localhost");
        std::env::set_var("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT", "1234");
        std::env::set_var("GOLEM__PORT", "1234");
        std::env::set_var("GOLEM__RESOURCE_LIMITS__CONFIG__HOST", "localhost");
        std::env::set_var("GOLEM__RESOURCE_LIMITS__CONFIG__PORT", "1234");
        std::env::set_var("GOLEM__RESOURCE_LIMITS__CONFIG__ACCESS_TOKEN", "token");
        std::env::set_var("GOLEM__HTTP_PORT", "1235");
        std::env::set_var("GOLEM__ENABLE_JSON_LOG", "true");
        std::env::set_var("GOLEM__PUBLIC_WORKER_API__HOST", "localhost");
        std::env::set_var("GOLEM__PUBLIC_WORKER_API__PORT", "1234");
        std::env::set_var("GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN", "token");

        // The rest can be loaded from the toml
        let _ = GolemConfig::new();
        let _ = AdditionalGolemConfig::new();
    }
}
