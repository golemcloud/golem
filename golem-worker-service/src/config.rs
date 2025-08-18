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

use crate::service::gateway::api_definition::ApiDefinitionServiceConfig;
use golem_common::config::RedisConfig;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_service_base::clients::RemoteServiceConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceConfig {
    pub environment: String,
    pub tracing: TracingConfig,
    pub gateway_session_storage: GatewaySessionStorageConfig,
    pub db: DbConfig,
    pub component_service: ComponentServiceConfig,
    pub port: u16,
    pub custom_request_port: u16,
    pub worker_grpc_port: u16,
    pub routing_table: RoutingTableConfig,
    pub worker_executor_retries: RetryConfig,
    pub blob_storage: BlobStorageConfig,
    pub api_definition: ApiDefinitionServiceConfig,
    pub workspace: String,
    pub domain_records: DomainRecordsConfig,
    pub cloud_service: RemoteServiceConfig,
    pub cors_origin_regex: String,
}

impl WorkerServiceConfig {
    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }
}

impl Default for WorkerServiceConfig {
    fn default() -> Self {
        Self {
            environment: "local".to_string(),
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: "../data/golem_worker.sqlite".to_string(),
                max_connections: 10,
            }),
            gateway_session_storage: GatewaySessionStorageConfig::default_redis(),
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
            blob_storage: BlobStorageConfig::default(),
            api_definition: ApiDefinitionServiceConfig::default(),
            workspace: "release".to_string(),
            domain_records: DomainRecordsConfig::default(),
            cloud_service: RemoteServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
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
pub struct ComponentServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
    pub connect_timeout: Duration,
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
            connect_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainRecordsConfig {
    pub subdomain_black_list: Vec<String>,
    pub domain_allow_list: Vec<String>,
    pub register_domain_black_list: Vec<String>,
}

impl Default for DomainRecordsConfig {
    fn default() -> Self {
        Self {
            subdomain_black_list: vec![
                "api-gateway".to_string(),
                "release".to_string(),
                "grafana".to_string(),
            ],
            domain_allow_list: vec![],
            register_domain_black_list: vec![
                "dev-api.golem.cloud".to_string(),
                "api.golem.cloud".to_string(),
            ],
        }
    }
}

impl DomainRecordsConfig {
    pub fn is_domain_available_for_registration(&self, domain_name: &str) -> bool {
        let dn = domain_name.to_lowercase();
        !self
            .register_domain_black_list
            .iter()
            .any(|d| domain_match(&dn, d))
    }

    pub fn is_domain_available(&self, domain_name: &str) -> bool {
        let dn = domain_name.to_lowercase();

        let in_register_black_list = self
            .register_domain_black_list
            .iter()
            .any(|d| domain_match(&dn, d));

        if in_register_black_list {
            let in_allow_list = self.domain_allow_list.iter().any(|d| domain_match(&dn, d));

            if !in_allow_list {
                return false;
            }
        }

        true
    }

    pub fn is_site_available(&self, api_site: &str, hosted_zone: &str) -> bool {
        let hz = if hosted_zone.ends_with('.') {
            &hosted_zone[0..hosted_zone.len() - 1]
        } else {
            hosted_zone
        };

        let s = api_site.to_lowercase();

        !self.subdomain_black_list.iter().any(|p| {
            let d = format!("{p}.{hz}").to_lowercase();
            d == s
        })
    }
}

fn domain_match(domain: &str, domain_cfg: &str) -> bool {
    if domain.ends_with(domain_cfg) {
        let prefix = &domain[0..domain.len() - domain_cfg.len()];
        prefix.is_empty() || prefix.ends_with('.')
    } else {
        false
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
    use crate::config::{domain_match, DomainRecordsConfig};

    #[test]
    pub fn config_is_loadable() {
        make_worker_service_config_loader()
            .load_or_dump_config()
            .expect("Failed to load config");
    }

    #[test]
    pub fn test_is_domain_match() {
        assert!(domain_match("dev-api.golem.cloud", "dev-api.golem.cloud"));
        assert!(domain_match("api.golem.cloud", "api.golem.cloud"));
        assert!(domain_match("dev.api.golem.cloud", "api.golem.cloud"));
        assert!(!domain_match("dev.api.golem.cloud", "dev-api.golem.cloud"));
        assert!(!domain_match("dev-api.golem.cloud", "api.golem.cloud"));
    }

    #[test]
    pub fn test_is_domain_available_for_registration() {
        let config = DomainRecordsConfig::default();
        assert!(!config.is_domain_available_for_registration("dev-api.golem.cloud"));
        assert!(!config.is_domain_available_for_registration("api.golem.cloud"));
        assert!(!config.is_domain_available_for_registration("my.dev-api.golem.cloud"));
        assert!(config.is_domain_available_for_registration("test.cloud"));
    }

    #[test]
    pub fn test_is_domain_available() {
        let config = DomainRecordsConfig {
            domain_allow_list: vec!["dev-api.golem.cloud".to_string()],
            ..Default::default()
        };

        assert!(config.is_domain_available("dev-api.golem.cloud"));
        assert!(config.is_domain_available("test.cloud"));
        assert!(!config.is_domain_available("api.golem.cloud"));
    }

    #[test]
    pub fn test_is_site_available() {
        let config = DomainRecordsConfig::default();

        let hosted_zone = "dev-api.golem.cloud.";

        assert!(!config.is_site_available("api-gateway.dev-api.golem.cloud", hosted_zone));
        assert!(!config.is_site_available("RELEASE.dev-api.golem.cloud", hosted_zone));
        assert!(!config.is_site_available("Grafana.dev-api.golem.cloud", hosted_zone));
        assert!(config.is_site_available("foo.dev-api.golem.cloud", hosted_zone));
    }
}
