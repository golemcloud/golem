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

use crate::model::{Plan, PlanData};
use golem_common::config::ConfigLoader;
use golem_common::config::DbConfig;
use golem_common::model::auth::Role;
use golem_common::model::{Empty, PlanId};
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use golem_service_base::clients::RemoteServiceConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
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
    pub accounts: AccountsConfig,
    pub login: LoginConfig,
    pub component_service: RemoteServiceConfig,
    pub cors_origin_regex: String,
}

impl SafeDisplay for CloudServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "environment: {}", self.environment);
        let _ = writeln!(&mut result, "workspace: {}", self.workspace);
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);
        let _ = writeln!(&mut result, "gRPC port: {}", self.grpc_port);
        let _ = writeln!(&mut result, "DB:");
        let _ = writeln!(&mut result, "{}", self.db.to_safe_string_indented());
        let _ = writeln!(&mut result, "plans:");
        let _ = writeln!(&mut result, "{}", self.plans.to_safe_string_indented());
        let _ = writeln!(&mut result, "accounts:");
        let _ = writeln!(&mut result, "{}", self.accounts.to_safe_string_indented());
        let _ = writeln!(&mut result, "login:");
        let _ = writeln!(&mut result, "{}", self.login.to_safe_string_indented());
        let _ = writeln!(&mut result, "component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);

        result
    }
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
            accounts: AccountsConfig::default(),
            login: LoginConfig::default(),
            component_service: RemoteServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdDsaConfig {
    pub private_key: String,
    pub public_key: String,
}

impl SafeDisplay for EdDsaConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "public key: {}", self.public_key);
        let _ = writeln!(&mut result, "private key: ****");
        result
    }
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

impl SafeDisplay for PlansConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "default:");
        let _ = writeln!(&mut result, "{}", self.default.to_safe_string_indented());
        result
    }
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

impl SafeDisplay for PlanConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "Plan ID: {}", self.plan_id);
        let _ = writeln!(&mut result, "Project limit: {}", self.project_limit);
        let _ = writeln!(&mut result, "Component limit: {}", self.component_limit);
        let _ = writeln!(&mut result, "Worker limit: {}", self.worker_limit);
        let _ = writeln!(&mut result, "Storage limit: {}", self.storage_limit);
        let _ = writeln!(&mut result, "Monthly gas limit: {}", self.monthly_gas_limit);
        let _ = writeln!(
            &mut result,
            "Monthly upload limit: {}",
            self.monthly_upload_limit
        );
        result
    }
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
#[serde(tag = "type", content = "config")]
pub enum LoginConfig {
    OAuth2(OAuth2Config),
    Disabled(Empty),
}

impl SafeDisplay for LoginConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            LoginConfig::OAuth2(inner) => {
                let _ = writeln!(&mut result, "OAuth2:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            LoginConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

impl Default for LoginConfig {
    fn default() -> LoginConfig {
        LoginConfig::OAuth2(OAuth2Config::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OAuth2Config {
    pub github: GitHubOAuth2Config,
    pub ed_dsa: EdDsaConfig,
}

impl SafeDisplay for OAuth2Config {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "GitHub:");
        let _ = writeln!(&mut result, "{}", self.github.to_safe_string_indented());
        let _ = writeln!(&mut result, "EdDSA:");
        let _ = writeln!(&mut result, "{}", self.ed_dsa.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubOAuth2Config {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: url::Url,
}

impl SafeDisplay for GitHubOAuth2Config {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "client id: {}", self.client_id);
        let _ = writeln!(&mut result, "client secret: ****");
        let _ = writeln!(&mut result, "redirect uri: {}", self.redirect_uri);
        result
    }
}

impl Default for GitHubOAuth2Config {
    fn default() -> Self {
        Self {
            client_id: "GITHUB_CLIENT_ID".to_string(),
            client_secret: "GITHUB_CLIENT_SECRET".to_string(),
            redirect_uri: url::Url::parse(
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

impl SafeDisplay for AccountsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        for (id, account) in &self.accounts {
            let _ = writeln!(&mut result, "{id}:");
            let _ = writeln!(&mut result, "{}", account.to_safe_string_indented());
        }
        result
    }
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

impl SafeDisplay for AccountConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "id: {}", self.id);
        let _ = writeln!(&mut result, "name: {}", self.name);
        let _ = writeln!(&mut result, "email: {}", self.email);
        let _ = writeln!(&mut result, "token: ****");
        let _ = writeln!(&mut result, "role: {:?}", self.role);
        result
    }
}

pub fn make_config_loader() -> ConfigLoader<CloudServiceConfig> {
    ConfigLoader::new(&PathBuf::from("config/cloud-service.toml"))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use test_r::test;

    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        env::set_current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("Failed to set current directory");

        make_config_loader().load().expect("Failed to load config");
    }
}
