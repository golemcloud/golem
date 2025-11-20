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

use crate::services::domain_registration::provisioner::DomainProvisionerConfig;
use chrono::Duration;
use golem_common::SafeDisplay;
use golem_common::config::ConfigLoader;
use golem_common::config::DbConfig;
use golem_common::model::auth::AccountRole;
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_service_base::config::BlobStorageConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
use uuid::Uuid;
use uuid::uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub login: LoginConfig,
    pub cors_origin_regex: String,
    pub component_transformer_plugin_caller: ComponentTransformerPluginCallerConfig,
    pub component_compilation: ComponentCompilationConfig,
    pub blob_storage: BlobStorageConfig,
    pub plans: PlansConfig,
    pub accounts: AccountsConfig,
    pub domain_provisioner: DomainProvisionerConfig,
}

impl SafeDisplay for RegistryServiceConfig {
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
        let _ = writeln!(&mut result, "login:");
        let _ = writeln!(&mut result, "{}", self.login.to_safe_string_indented());
        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "plugin transformations:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_transformer_plugin_caller
                .to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);

        let _ = writeln!(&mut result, "domain provision:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.domain_provisioner.to_safe_string_indented()
        );

        result
    }
}

impl Default for RegistryServiceConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("registry-service"),
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            http_port: 8080,
            grpc_port: 8081,
            db: DbConfig::default(),
            login: LoginConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
            component_transformer_plugin_caller: ComponentTransformerPluginCallerConfig::default(),
            component_compilation: ComponentCompilationConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            plans: PlansConfig::default(),
            accounts: AccountsConfig::default(),
            domain_provisioner: DomainProvisionerConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum LoginConfig {
    OAuth2(OAuth2LoginSystemConfig),
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
        LoginConfig::OAuth2(OAuth2LoginSystemConfig::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OAuth2LoginSystemConfig {
    pub github: GitHubOAuth2Config,
    pub oauth2: OAuth2Config,
}

impl SafeDisplay for OAuth2LoginSystemConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "GitHub:");
        let _ = writeln!(&mut result, "{}", self.github.to_safe_string_indented());
        let _ = writeln!(&mut result, "OAuth2:");
        let _ = writeln!(&mut result, "{}", self.oauth2.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OAuth2Config {
    pub webflow_state_expiry: Duration,
    pub private_key: String,
    pub public_key: String,
}

impl SafeDisplay for OAuth2Config {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "webflow state expiry: {}",
            self.webflow_state_expiry
        );
        let _ = writeln!(&mut result, "private key: ****");
        let _ = writeln!(&mut result, "public key: {}", self.public_key);
        result
    }
}

impl Default for OAuth2Config {
    fn default() -> Self {
        Self {
            webflow_state_expiry: Duration::minutes(5),
            private_key: "MC4CAQAwBQYDK2VwBCIEIMDNO+xRAwWTDqt5wN84sCHviRldQMiylmSK715b5JnW"
                .to_string(),
            public_key: "MCowBQYDK2VwAyEA9gxANNtlWPBBTm0IEgvMgCEUXw+ohwffyM9wOL4O1pg=".to_string(),
        }
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
            redirect_uri: url::Url::parse("http://localhost:8080/v1/login/oauth2/web/callback")
                .unwrap(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountsConfig {
    pub accounts: HashMap<String, PrecreatedAccount>,
}

impl Default for AccountsConfig {
    fn default() -> Self {
        let mut accounts = HashMap::with_capacity(2);
        accounts.insert(
            "root".to_string(),
            PrecreatedAccount {
                id: uuid!("e71a6160-4144-4720-9e34-e5943458d129"),
                name: "Initial User".to_string(),
                email: "initial@user".to_string(),
                token: uuid!("5c832d93-ff85-4a8f-9803-513950fdfdb1"),
                role: AccountRole::Admin,
                plan_id: uuid!("157dc684-00eb-496d-941c-da8fd1d15c63"),
            },
        );
        accounts.insert(
            "marketing".to_string(),
            PrecreatedAccount {
                id: uuid!("0e8a0431-94b9-4644-89ca-fbf403edb6e7"),
                name: "Marketing User".to_string(),
                email: "marketing@user".to_string(),
                token: uuid!("39c8e462-1a4c-464c-91d5-5265e1e1b0e5"),
                role: AccountRole::MarketingAdmin,
                plan_id: uuid!("157dc684-00eb-496d-941c-da8fd1d15c63"),
            },
        );
        AccountsConfig { accounts }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrecreatedAccount {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub token: Uuid,
    pub plan_id: Uuid,
    pub role: AccountRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ComponentTransformerPluginCallerConfig {
    pub retries: RetryConfig,
}

impl SafeDisplay for ComponentTransformerPluginCallerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlansConfig {
    pub plans: HashMap<String, PrecreatedPlan>,
}

impl Default for PlansConfig {
    fn default() -> Self {
        let mut plans = HashMap::with_capacity(1);
        plans.insert(
            "default".to_string(),
            PrecreatedPlan {
                plan_id: uuid!("157dc684-00eb-496d-941c-da8fd1d15c63"),
                app_limit: 10,
                env_limit: 40,
                component_limit: 100,
                worker_limit: 10000,
                worker_connection_limit: 100,
                storage_limit: 500000000,
                monthly_gas_limit: 1000000000000,
                monthly_upload_limit: 1000000000,
                max_memory_per_worker: 1024 * 1024 * 1024, // 1 GB
            },
        );

        PlansConfig { plans }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrecreatedPlan {
    pub plan_id: Uuid,
    pub app_limit: i64,
    pub env_limit: i64,
    pub component_limit: i64,
    pub worker_limit: i64,
    pub worker_connection_limit: i64,
    pub storage_limit: i64,
    pub monthly_gas_limit: i64,
    pub monthly_upload_limit: i64,
    pub max_memory_per_worker: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentCompilationConfig {
    Enabled(ComponentCompilationEnabledConfig),
    Disabled(Empty),
}

impl SafeDisplay for ComponentCompilationConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ComponentCompilationConfig::Enabled(inner) => {
                let _ = writeln!(&mut result, "enabled:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            ComponentCompilationConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

impl Default for ComponentCompilationConfig {
    fn default() -> Self {
        Self::Enabled(ComponentCompilationEnabledConfig {
            host: "localhost".to_string(),
            port: 9091,
            retries: RetryConfig::default(),
            connect_timeout: std::time::Duration::from_secs(10),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: std::time::Duration,
}

impl SafeDisplay for ComponentCompilationEnabledConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "connect timeout: {:?}", self.connect_timeout);
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

impl ComponentCompilationEnabledConfig {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentCompilationService URI")
    }
}

pub fn make_config_loader() -> ConfigLoader<RegistryServiceConfig> {
    ConfigLoader::new(&PathBuf::from("config/registry-service.toml"))
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
