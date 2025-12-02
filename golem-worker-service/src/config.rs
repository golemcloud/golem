// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// use crate::service::gateway::api_definition::ApiDefinitionServiceConfig;
use golem_common::config::RedisConfig;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use golem_service_base::clients::RegistryServiceConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Write};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceConfig {
    pub environment: String,
    pub tracing: TracingConfig,
    pub gateway_session_storage: GatewaySessionStorageConfig,
    pub db: DbConfig,
    pub port: u16,
    pub custom_request_port: u16,
    pub worker_grpc_port: u16,
    pub routing_table: RoutingTableConfig,
    pub worker_executor_retries: RetryConfig,
    pub blob_storage: BlobStorageConfig,
    pub workspace: String,
    pub registry_service: RegistryServiceConfig,
    pub cors_origin_regex: String,
    pub route_resolver: RouteResolverConfig,
    pub component_service: ComponentServiceConfig,
    pub auth_service: AuthServiceConfig,
}

impl WorkerServiceConfig {
    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }
}

impl SafeDisplay for WorkerServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "environment: {}", self.environment);
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "gateway session storage:");
        let _ = writeln!(
            result,
            "{}",
            self.gateway_session_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "db:");
        let _ = writeln!(&mut result, "{}", self.db.to_safe_string_indented());
        let _ = writeln!(&mut result, "HTTP port: {}", self.port);
        let _ = writeln!(
            &mut result,
            "Custom request port: {}",
            self.custom_request_port
        );
        let _ = writeln!(&mut result, "gRPC port: {}", self.worker_grpc_port);
        let _ = writeln!(&mut result, "routing table:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.routing_table.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "worker executor retries:");
        let _ = writeln!(
            result,
            "{}",
            self.worker_executor_retries.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "workspace: {}", self.workspace);
        let _ = writeln!(&mut result, "registry service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.registry_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);

        let _ = writeln!(&mut result, "route resolver:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.route_resolver.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "auth service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.auth_service.to_safe_string_indented()
        );

        result
    }
}

impl Default for WorkerServiceConfig {
    fn default() -> Self {
        Self {
            environment: "local".to_string(),
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: "../data/golem_worker.sqlite".to_string(),
                max_connections: 10,
                foreign_keys: false,
            }),
            gateway_session_storage: GatewaySessionStorageConfig::default_redis(),
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
            blob_storage: BlobStorageConfig::default(),
            // api_definition: ApiDefinitionServiceConfig::default(),
            workspace: "release".to_string(),
            registry_service: RegistryServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
            route_resolver: RouteResolverConfig::default(),
            component_service: ComponentServiceConfig::default(),
            auth_service: AuthServiceConfig::default(),
        }
    }
}

impl HasConfigExamples<WorkerServiceConfig> for WorkerServiceConfig {
    fn examples() -> Vec<ConfigExample<WorkerServiceConfig>> {
        vec![
            (
                "with postgres",
                Self {
                    db: DbConfig::postgres_example(),
                    ..Self::default()
                },
            ),
            (
                "with postgres and s3",
                Self {
                    db: DbConfig::postgres_example(),
                    blob_storage: BlobStorageConfig::default_s3(),
                    ..Self::default()
                },
            ),
        ]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum GatewaySessionStorageConfig {
    Redis(RedisConfig),
    Sqlite(DbSqliteConfig),
}

impl SafeDisplay for GatewaySessionStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            GatewaySessionStorageConfig::Redis(redis) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", redis.to_safe_string_indented());
            }
            GatewaySessionStorageConfig::Sqlite(sqlite) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", sqlite.to_safe_string_indented());
            }
        }
        result
    }
}

impl Default for GatewaySessionStorageConfig {
    fn default() -> Self {
        Self::default_redis()
    }
}

impl GatewaySessionStorageConfig {
    pub fn default_redis() -> Self {
        Self::Redis(RedisConfig::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteResolverConfig {
    pub router_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub router_cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub router_cache_eviction_period: Duration,
}

impl SafeDisplay for RouteResolverConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "router_cache_max_capacity: {}",
            self.router_cache_max_capacity
        );
        let _ = writeln!(&mut result, "router_cache_ttl: {:?}", self.router_cache_ttl);
        let _ = writeln!(
            &mut result,
            "router_cache_eviction_period: {:?}",
            self.router_cache_eviction_period
        );
        result
    }
}

impl Default for RouteResolverConfig {
    fn default() -> Self {
        Self {
            router_cache_max_capacity: 1024,
            router_cache_ttl: Duration::from_mins(10),
            router_cache_eviction_period: Duration::from_mins(1),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceConfig {
    pub component_cache_max_capacity: usize,
}

impl SafeDisplay for ComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "component_cache_max_capacity: {}",
            self.component_cache_max_capacity
        );
        result
    }
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            component_cache_max_capacity: 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthServiceConfig {
    pub auth_ctx_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub auth_ctx_cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub auth_ctx_cache_eviction_period: Duration,

    pub environment_auth_details_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub environment_auth_details_cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub environment_auth_details_cache_eviction_period: Duration,
}

impl SafeDisplay for AuthServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(
            &mut result,
            "auth_ctx_cache_max_capacity: {}",
            self.auth_ctx_cache_max_capacity
        );
        let _ = writeln!(
            &mut result,
            "auth_ctx_cache_ttl: {:?}",
            self.auth_ctx_cache_ttl
        );
        let _ = writeln!(
            &mut result,
            "auth_ctx_cache_eviction_period: {:?}",
            self.auth_ctx_cache_eviction_period
        );

        let _ = writeln!(
            &mut result,
            "environment_auth_details_cache_max_capacity: {}",
            self.environment_auth_details_cache_max_capacity
        );
        let _ = writeln!(
            &mut result,
            "environment_auth_details_cache_ttl: {:?}",
            self.environment_auth_details_cache_ttl
        );
        let _ = writeln!(
            &mut result,
            "environment_auth_details_cache_eviction_period: {:?}",
            self.environment_auth_details_cache_eviction_period
        );

        result
    }
}

impl Default for AuthServiceConfig {
    fn default() -> Self {
        Self {
            auth_ctx_cache_max_capacity: 1024,
            auth_ctx_cache_ttl: Duration::from_mins(10),
            auth_ctx_cache_eviction_period: Duration::from_mins(1),

            environment_auth_details_cache_max_capacity: 1024,
            environment_auth_details_cache_ttl: Duration::from_mins(10),
            environment_auth_details_cache_eviction_period: Duration::from_mins(1),
        }
    }
}

const CONFIG_FILE_NAME: &str = "config/worker-service.toml";

pub fn make_worker_service_config_loader() -> ConfigLoader<WorkerServiceConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from(CONFIG_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::make_worker_service_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_worker_service_config_loader()
            .load_or_dump_config()
            .expect("Failed to load config");
    }
}
