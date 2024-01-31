use std::collections::HashMap;

use cloud_common::model::PlanId;
use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_service_base::config::TemplateStoreConfig;
use golem_service_base::routing_table::RoutingTableConfig;
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
        Self::new()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct EdDsaConfig {
    pub private_key: String,
    pub public_key: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplatesConfig {
    pub store: TemplateStoreConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DbConfig {
    Postgres(DbPostgresConfig),
    Sqlite(DbSqliteConfig),
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

#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
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
