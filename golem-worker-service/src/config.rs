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

use golem_common::SafeDisplay;
use golem_common::config::DbSqliteConfig;
use golem_common::config::RedisConfig;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::RetryConfig;
use golem_common::model::base64::Base64;
use golem_common::tracing::TracingConfig;
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Write};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceConfig {
    pub environment: String,
    pub tracing: TracingConfig,
    pub gateway_session_storage: SessionStoreConfig,
    pub port: u16,
    pub custom_request_port: u16,
    pub grpc: GrpcApiConfig,
    pub routing_table: RoutingTableConfig,
    pub worker_executor: WorkerExecutorClientConfig,
    pub workspace: String,
    pub registry_service: GrpcRegistryServiceConfig,
    pub cors_origin_regex: String,
    pub route_resolver: RouteResolverConfig,
    pub component_service: ComponentServiceConfig,
    pub auth_service: AuthServiceConfig,
    pub webhook_callback_handler: WebhookCallbackHandlerConfig,
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
        let _ = writeln!(&mut result, "HTTP port: {}", self.port);
        let _ = writeln!(
            &mut result,
            "Custom request port: {}",
            self.custom_request_port
        );

        let _ = writeln!(&mut result, "grpc:");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

        let _ = writeln!(&mut result, "routing table:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.routing_table.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "worker executor:");
        let _ = writeln!(result, "{}", self.worker_executor.to_safe_string_indented());

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

        let _ = writeln!(&mut result, "webhook callback handler:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.webhook_callback_handler.to_safe_string_indented()
        );

        result
    }
}

impl Default for WorkerServiceConfig {
    fn default() -> Self {
        Self {
            environment: "local".to_string(),
            gateway_session_storage: SessionStoreConfig::Redis(Default::default()),
            tracing: TracingConfig::local_dev("worker-service"),
            port: 9005,
            custom_request_port: 9006,
            grpc: GrpcApiConfig::default(),
            routing_table: RoutingTableConfig::default(),
            worker_executor: WorkerExecutorClientConfig::default(),
            workspace: "release".to_string(),
            registry_service: GrpcRegistryServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
            route_resolver: RouteResolverConfig::default(),
            component_service: ComponentServiceConfig::default(),
            auth_service: AuthServiceConfig::default(),
            webhook_callback_handler: WebhookCallbackHandlerConfig::default(),
        }
    }
}

impl HasConfigExamples<WorkerServiceConfig> for WorkerServiceConfig {
    fn examples() -> Vec<ConfigExample<WorkerServiceConfig>> {
        vec![]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    pub tls: GrpcServerTlsConfig,
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9094,
            tls: GrpcServerTlsConfig::disabled(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum SessionStoreConfig {
    Redis(RedisSessionStoreConfig),
    Sqlite(SqliteSessionStoreConfig),
}

impl SafeDisplay for SessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            SessionStoreConfig::Redis(redis) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", redis.to_safe_string_indented());
            }
            SessionStoreConfig::Sqlite(sqlite) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", sqlite.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedisSessionStoreConfig {
    #[serde(with = "humantime_serde")]
    pub pending_login_expiration: std::time::Duration,
    #[serde(flatten)]
    pub redis_config: RedisConfig,
}

impl Default for RedisSessionStoreConfig {
    fn default() -> Self {
        Self {
            pending_login_expiration: Duration::from_hours(1),
            redis_config: RedisConfig::default(),
        }
    }
}

impl SafeDisplay for RedisSessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "pending_login_expiration: {:?}",
            self.pending_login_expiration
        );
        let _ = writeln!(&mut result, "{}", self.redis_config.to_safe_string());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqliteSessionStoreConfig {
    #[serde(with = "humantime_serde")]
    pub pending_login_expiration: std::time::Duration,
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: std::time::Duration,
    #[serde(flatten)]
    pub sqlite_config: DbSqliteConfig,
}

impl SafeDisplay for SqliteSessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "pending_login_expiration: {:?}",
            self.pending_login_expiration
        );
        let _ = writeln!(&mut result, "cleanup_interval: {:?}", self.cleanup_interval);
        let _ = writeln!(&mut result, "{}", self.sqlite_config.to_safe_string());
        result
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerExecutorClientConfig {
    pub retries: RetryConfig,
    #[serde(flatten)]
    pub client: GrpcClientConfig,
}

impl Default for WorkerExecutorClientConfig {
    fn default() -> Self {
        Self {
            retries: RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_secs(3),
                multiplier: 10.0,
                max_jitter_factor: Some(0.15),
            },
            client: GrpcClientConfig {
                retries_on_unavailable: RetryConfig {
                    max_attempts: 0, // we want to invalidate the routing table asap
                    min_delay: Duration::from_millis(100),
                    max_delay: Duration::from_secs(2),
                    multiplier: 2.0,
                    max_jitter_factor: Some(0.15),
                },
                connect_timeout: Duration::from_secs(10),
                ..Default::default()
            },
        }
    }
}

impl SafeDisplay for WorkerExecutorClientConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(result, "{}", self.retries.to_safe_string_indented());

        let _ = writeln!(&mut result, "{}", self.client.to_safe_string());

        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebhookCallbackHandlerConfig {
    pub hmac_key: Base64,
}

impl SafeDisplay for WebhookCallbackHandlerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "hmac_key: *******");
        result
    }
}

impl Default for WebhookCallbackHandlerConfig {
    fn default() -> Self {
        Self {
            hmac_key: Base64(vec![
                0x2b, 0x7e, 0x02, 0xa3, 0x8a, 0x51, 0x30, 0x39, 0x7b, 0x74, 0x1d, 0xdc, 0x60, 0x1f,
                0xb5, 0xfc, 0xdd, 0x09, 0xde, 0xd3, 0x33, 0x25, 0x62, 0x38, 0x17, 0x23, 0xcd, 0x3a,
                0xc9, 0x86, 0x1e, 0x41,
            ]),
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
