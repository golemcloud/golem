use std::time::Duration;

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use golem_common::config::{RedisConfig, RetryConfig};
use golem_common::tracing;
use golem_service_base::routing_table::RoutingTableConfig;

// The base configuration for the worker service
// If there are extra cofigurations for custom services,
// its preferred to reuse base config.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceBaseConfig {
    pub environment: String,
    pub tracing: tracing::Config,
    pub redis: RedisConfig,
    pub component_service: ComponentServiceConfig,
    pub port: u16,
    pub custom_request_port: u16,
    pub worker_grpc_port: u16,
    pub routing_table: RoutingTableConfig,
    pub worker_executor_client_cache: WorkerExecutorClientCacheConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerExecutorClientCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl Default for WorkerExecutorClientCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 1000,
            time_to_idle: Duration::from_secs(60 * 60 * 4),
        }
    }
}

impl WorkerServiceBaseConfig {
    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }

    pub fn new() -> Self {
        Figment::new()
            .merge(Serialized::defaults(WorkerServiceBaseConfig::default()))
            .merge(Toml::file("config/worker-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

impl Default for WorkerServiceBaseConfig {
    fn default() -> Self {
        Self {
            environment: "local".to_string(),
            redis: RedisConfig::default(),
            component_service: ComponentServiceConfig::default(),
            tracing: tracing::Config::local_dev("worker-service"),
            port: 9000,
            custom_request_port: 9001,
            worker_grpc_port: 9092,
            routing_table: RoutingTableConfig::default(),
            worker_executor_client_cache: WorkerExecutorClientCacheConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl ComponentServiceConfig {
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

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            access_token: Uuid::new_v4(),
            retries: RetryConfig::default(),
        }
    }
}
