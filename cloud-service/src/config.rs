use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::{Plan, PlanData};
use cloud_common::config::RemoteCloudServiceConfig;
use cloud_common::model::PlanId;
use cloud_common::model::Role;
use golem_common::config::ConfigLoader;
use golem_common::config::DbConfig;
use golem_common::tracing::TracingConfig;
use serde::{Deserialize, Serialize};
use uuid::uuid;
use uuid::Uuid;

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
    pub component_service: RemoteCloudServiceConfig,
    pub cors_origin_regex: String,
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
            component_service: RemoteCloudServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
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
            private_key: "MC4CAQAwBQYDK2VwBCIEIMDNO+xRAwWTDqt5wN84sCHviRldQMiylmSK715b5JnW"
                .to_string(),
            public_key: "MCowBQYDK2VwAyEA9gxANNtlWPBBTm0IEgvMgCEUXw+ohwffyM9wOL4O1pg=".to_string(),
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
    pub github_client_secret: String,
    pub github_redirect_uri: url::Url,
}

impl Default for OAuth2Config {
    fn default() -> Self {
        OAuth2Config {
            github_client_id: "GITHUB_CLIENT_ID".to_string(),
            github_client_secret: "GITHUB_CLIENT_SECRET".to_string(),
            github_redirect_uri: url::Url::parse(
                "http://localhost:8080/v1/login/oauth2/web/callback/github",
            )
            .unwrap(),
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
                token: uuid!("5c832d93-ff85-4a8f-9803-513950fdfdb1"),
                role: Role::Admin,
            },
        );
        accounts.insert(
            "marketing".to_string(),
            AccountConfig {
                id: "marketing".to_string(),
                name: "Marketing User".to_string(),
                email: "marketing@user".to_string(),
                token: uuid!("39c8e462-1a4c-464c-91d5-5265e1e1b0e5"),
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

pub fn make_config_loader() -> ConfigLoader<CloudServiceConfig> {
    ConfigLoader::new(&PathBuf::from("config/cloud-service.toml"))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
