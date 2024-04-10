use std::collections::HashMap;
use std::time::Duration;

use cloud_common::model::PlanId;
use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_service_base::config::{TemplateStoreConfig, TemplateStoreLocalConfig};
use golem_service_base::routing_table::RoutingTableConfig;
use golem_template_service_base::config::TemplateCompilationConfig;
use serde::Deserialize;
use uuid::Uuid;

use crate::model::{Plan, PlanData, Role};

#[derive(Clone, Debug, Deserialize)]
pub struct CloudServiceConfig {
    pub environment: String,
    pub workspace: String,
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub plans: PlansConfig,
    pub templates: TemplatesConfig,
    pub routing_table: RoutingTableConfig,
    pub ed_dsa: EdDsaConfig,
    pub accounts: AccountsConfig,
    pub oauth2: OAuth2Config,
    pub worker_executor_client_cache: WorkerExecutorClientCacheConfig,
}

impl CloudServiceConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/cloud-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

impl Default for CloudServiceConfig {
    fn default() -> Self {
        Self {
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            enable_tracing_console: false,
            enable_json_log: false,
            http_port: 8080,
            grpc_port: 8081,
            db: DbConfig::default(),
            plans: PlansConfig::default(),
            templates: TemplatesConfig::default(),
            routing_table: RoutingTableConfig::default(),
            ed_dsa: EdDsaConfig::default(),
            accounts: AccountsConfig::default(),
            oauth2: OAuth2Config::default(),
            worker_executor_client_cache: WorkerExecutorClientCacheConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct EdDsaConfig {
    pub private_key: String,
    pub public_key: String,
}

impl Default for EdDsaConfig {
    fn default() -> Self {
        EdDsaConfig {
            private_key: "".to_string(),
            public_key: "".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplatesConfig {
    pub store: TemplateStoreConfig,
    pub compilation: TemplateCompilationConfig,
}

impl Default for TemplatesConfig {
    fn default() -> Self {
        TemplatesConfig {
            store: TemplateStoreConfig::Local(TemplateStoreLocalConfig {
                root_path: "templates".to_string(),
                object_prefix: "".to_string(),
            }),
            compilation: TemplateCompilationConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DbConfig {
    Postgres(DbPostgresConfig),
    Sqlite(DbSqliteConfig),
}

impl Default for DbConfig {
    fn default() -> Self {
        DbConfig::Sqlite(DbSqliteConfig {
            database: "golem_cloud_service.db".to_string(),
            max_connections: 10,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbPostgresConfig {
    pub host: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub port: u16,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbSqliteConfig {
    pub database: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlansConfig {
    pub default: PlanConfig,
}

impl Default for PlansConfig {
    fn default() -> Self {
        PlansConfig {
            default: PlanConfig {
                plan_id: Uuid::nil(),
                project_limit: 100,
                template_limit: 100,
                worker_limit: 10000,
                storage_limit: 500000000,
                monthly_gas_limit: 1000000000000,
                monthly_upload_limit: 1000000000,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlanConfig {
    pub plan_id: Uuid,
    pub project_limit: i32,
    pub template_limit: i32,
    pub worker_limit: i32,
    pub storage_limit: i32,
    pub monthly_gas_limit: i64,
    pub monthly_upload_limit: i32,
}

impl From<PlanConfig> for Plan {
    fn from(config: PlanConfig) -> Self {
        Plan {
            plan_id: PlanId(config.plan_id),
            plan_data: PlanData {
                project_limit: config.project_limit,
                template_limit: config.template_limit,
                worker_limit: config.worker_limit,
                storage_limit: config.storage_limit,
                monthly_gas_limit: config.monthly_gas_limit,
                monthly_upload_limit: config.monthly_upload_limit,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct OAuth2Config {
    pub github_client_id: String,
}

impl Default for OAuth2Config {
    fn default() -> Self {
        OAuth2Config {
            github_client_id: "GITHUB_CLIENT_ID".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
#[derive(Default)]
pub struct AccountsConfig {
    pub accounts: HashMap<String, AccountConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccountConfig {
    pub id: String,
    pub name: String,
    pub email: String,
    pub token: Uuid,
    pub role: Role,
}

// TODO: move to the base library
#[derive(Clone, Debug, Deserialize)]
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

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        std::env::set_var("GOLEM__DB__TYPE", "Postgres");
        std::env::set_var("GOLEM__DB__CONFIG__USERNAME", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__PASSWORD", "postgres");
        std::env::set_var("GOLEM__ENVIRONMENT", "dev");
        std::env::set_var("GOLEM__WORKSPACE", "test");
        std::env::set_var("GOLEM__ROUTING_TABLE__HOST", "localhost");
        std::env::set_var("GOLEM__ROUTING_TABLE__PORT", "1234");
        std::env::set_var(
            "GOLEM__ACCOUNTS__ROOT__TOKEN",
            "c88084af-3741-4946-8b58-fa445d770a26",
        );
        std::env::set_var(
            "GOLEM__ACCOUNTS__MARKETING__TOKEN",
            "bb249eb2-e54e-4bab-8e0e-836578e35912",
        );
        std::env::set_var("GOLEM__ED_DSA__PRIVATE_KEY", "x1234");
        std::env::set_var("GOLEM__ED_DSA__PUBLIC_KEY", "x1234");
        std::env::set_var("GOLEM__TEMPLATES__STORE__TYPE", "S3");
        std::env::set_var("GOLEM__TEMPLATES__STORE__CONFIG__BUCKET_NAME", "bucket");

        // The rest can be loaded from the toml
        let _ = super::CloudServiceConfig::new();
    }
}
