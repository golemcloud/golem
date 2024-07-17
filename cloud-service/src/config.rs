use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cloud_common::model::PlanId;
use cloud_common::model::Role;
use golem_common::config::ConfigLoader;
use golem_common::tracing::TracingConfig;
use golem_service_base::config::DbConfig;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{Plan, PlanData};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub plans: PlansConfig,
    pub ed_dsa: EdDsaConfig,
    pub accounts: AccountsConfig,
    pub oauth2: OAuth2Config,
}

impl Default for CloudServiceConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("cloud-service"),
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            http_port: 8080,
            grpc_port: 8081,
            db: DbConfig::default(),
            plans: PlansConfig::default(),
            ed_dsa: EdDsaConfig::default(),
            accounts: AccountsConfig::default(),
            oauth2: OAuth2Config::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlansConfig {
    pub default: PlanConfig,
}

impl Default for PlansConfig {
    fn default() -> Self {
        PlansConfig {
            default: PlanConfig {
                plan_id: Uuid::nil(),
                project_limit: 100,
                component_limit: 100,
                worker_limit: 10000,
                storage_limit: 500000000,
                monthly_gas_limit: 1000000000000,
                monthly_upload_limit: 1000000000,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanConfig {
    pub plan_id: Uuid,
    pub project_limit: i32,
    pub component_limit: i32,
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
                component_limit: config.component_limit,
                worker_limit: config.worker_limit,
                storage_limit: config.storage_limit,
                monthly_gas_limit: config.monthly_gas_limit,
                monthly_upload_limit: config.monthly_upload_limit,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountsConfig {
    pub accounts: HashMap<String, AccountConfig>,
}

impl Default for AccountsConfig {
    fn default() -> Self {
        let mut accounts = HashMap::new();
        accounts.insert(
            "root".to_string(),
            AccountConfig {
                id: "root".to_string(),
                name: "Initial User".to_string(),
                email: "initial@user".to_string(),
                token: Default::default(),
                role: Role::Admin,
            },
        );
        accounts.insert(
            "marketing".to_string(),
            AccountConfig {
                id: "marketing".to_string(),
                name: "Marketing User".to_string(),
                email: "marketing@user".to_string(),
                token: Default::default(),
                role: Role::MarketingAdmin,
            },
        );
        AccountsConfig { accounts }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountConfig {
    pub id: String,
    pub name: String,
    pub email: String,
    pub token: Uuid,
    pub role: Role,
}

// TODO: move to the base library
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

pub fn make_config_loader() -> ConfigLoader<CloudServiceConfig> {
    ConfigLoader::new(&PathBuf::from("config/cloud-service.toml"))
}

#[cfg(test)]
mod tests {
    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
