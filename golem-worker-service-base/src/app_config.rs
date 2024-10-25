use std::fmt::Debug;
use std::time::Duration;

use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use golem_common::config::{ConfigExample, HasConfigExamples, RetryConfig};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::tracing::TracingConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;

// The base configuration for the worker service
// If there are extra configurations for custom services,
// it's preferred to reuse base config.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceBaseConfig {
    pub environment: String,
    pub tracing: TracingConfig,
    pub db: DbConfig,
    pub component_service: ComponentServiceConfig,
    pub port: u16,
    pub custom_request_port: u16,
    pub worker_grpc_port: u16,
    pub routing_table: RoutingTableConfig,
    pub worker_executor_retries: RetryConfig,
}

impl WorkerServiceBaseConfig {
    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }
}

impl Default for WorkerServiceBaseConfig {
    fn default() -> Self {
        Self {
            environment: "local".to_string(),
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: "../data/golem_worker.sqlite".to_string(),
                max_connections: 10,
            }),
            component_service: ComponentServiceConfig::default(),
            tracing: TracingConfig::local_dev("worker-service"),
            port: 9005,
            custom_request_port: 9006,
            worker_grpc_port: 9007,
            routing_table: RoutingTableConfig::default(),
            worker_executor_retries: RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_secs(3),
                multiplier: 10.0,
                max_jitter_factor: Some(0.15),
            },
        }
    }
}

impl HasConfigExamples<WorkerServiceBaseConfig> for WorkerServiceBaseConfig {
    fn examples() -> Vec<ConfigExample<WorkerServiceBaseConfig>> {
        vec![(
            "with postgres",
            Self {
                db: DbConfig::postgres_example(),
                ..Self::default()
            },
        )]
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
            port: 9090,
            access_token: Uuid::parse_str("5c832d93-ff85-4a8f-9803-513950fdfdb1")
                .expect("invalid UUID"),
            retries: RetryConfig::max_attempts_3(),
        }
    }
}
