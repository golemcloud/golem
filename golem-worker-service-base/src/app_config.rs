use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::{RedisConfig, RetryConfig};
use golem_service_base::routing_table::RoutingTableConfig;
use http::Uri;
use serde::Deserialize;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

// The base configuration for the worker service
// If there are extra cofigurations for custom services,
// its preferred to reuse base config.
#[derive(Clone, Debug, Deserialize)]
pub struct WorkerServiceBaseConfig {
    pub environment: String,
    pub redis: RedisConfig,
    pub template_service: TemplateServiceConfig,
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub port: u16,
    pub custom_request_port: u16,
    pub worker_grpc_port: u16,
    pub routing_table: RoutingTableConfig,
    pub worker_executor_client_cache: WorkerExecutorClientCacheConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerExecutorClientCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl WorkerServiceBaseConfig {
    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }
}

impl Default for WorkerServiceBaseConfig {
    fn default() -> Self {
        Figment::new()
            .merge(Toml::file("config/worker-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl TemplateServiceConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse ComponentService URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentService URI")
    }
}
