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
use golem_common::config::ConfigLoader;
use golem_common::config::DbConfig;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::plan::{PlanId, PlanName};
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_common::{SafeDisplay, grpc_uri};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
use uuid::uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc: GrpcApiConfig,
    pub db: DbConfig,
    pub login: LoginConfig,
    pub blob_storage: BlobStorageConfig,
    pub component_transformer_plugin_caller: ComponentTransformerPluginCallerConfig,
    pub cors_origin_regex: String,
    pub domain_provisioner: DomainProvisionerConfig,
    pub component_compilation: ComponentCompilationConfig,
    pub initial_accounts: HashMap<String, PrecreatedAccount>,
    pub initial_plans: HashMap<String, PrecreatedPlan>,
}

impl SafeDisplay for RegistryServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "environment: {}", self.environment);
        let _ = writeln!(&mut result, "workspace: {}", self.workspace);
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);

        let _ = writeln!(&mut result, "grpc:");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

        let _ = writeln!(&mut result, "db:");
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

        let _ = writeln!(&mut result, "domain provisioner:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.domain_provisioner.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "component compilation:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_compilation.to_safe_string_indented()
        );

        result
    }
}

impl Default for RegistryServiceConfig {
    fn default() -> Self {
        let mut initial_accounts = HashMap::with_capacity(2);
        initial_accounts.insert(
            "root".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("e71a6160-4144-4720-9e34-e5943458d129")),
                name: "Initial User".to_string(),
                email: AccountEmail("initial@user".to_string()),
                token: TokenSecret::trusted(
                    "lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28".to_string(),
                ),
                role: AccountRole::Admin,
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
            },
        );
        initial_accounts.insert(
            "marketing".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("0e8a0431-94b9-4644-89ca-fbf403edb6e7")),
                name: "Marketing User".to_string(),
                email: AccountEmail("marketing@user".to_string()),
                token: TokenSecret::trusted(
                    "2dwnjEdx8a_bw8TTN7r6yqcvLY2jAQuoD1N6U3uRy9I".to_string(),
                ),
                role: AccountRole::MarketingAdmin,
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
            },
        );

        let mut initial_plans = HashMap::with_capacity(1);
        initial_plans.insert(
            "default".to_string(),
            PrecreatedPlan {
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
                plan_name: PlanName("default".to_string()),
                app_limit: 10,
                env_limit: 40,
                component_limit: 100,
                worker_limit: 10000,
                worker_connection_limit: 100,
                storage_limit: 500000000,
                monthly_gas_limit: 1000000000000000000,
                monthly_upload_limit: 1000000000,
                max_memory_per_worker: 1024 * 1024 * 1024, // 1 GB
            },
        );

        Self {
            tracing: TracingConfig::local_dev("registry-service"),
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            http_port: 8081,
            grpc: GrpcApiConfig::default(),
            db: DbConfig::default(),
            login: LoginConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
            component_transformer_plugin_caller: ComponentTransformerPluginCallerConfig::default(),
            component_compilation: ComponentCompilationConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            domain_provisioner: DomainProvisionerConfig::default(),
            initial_accounts,
            initial_plans,
        }
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
            port: 9090,
            tls: GrpcServerTlsConfig::disabled(),
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
    #[serde(with = "humantime_serde")]
    pub webflow_state_expiry: std::time::Duration,
    pub private_key: String,
    pub public_key: String,
}

impl SafeDisplay for OAuth2Config {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "webflow state expiry: {:?}",
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
            webflow_state_expiry: std::time::Duration::from_mins(5),
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
#[serde(tag = "type", content = "config")]
pub enum ComponentCompilationConfig {
    Enabled(Box<ComponentCompilationEnabledConfig>),
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
        Self::Enabled(Box::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl SafeDisplay for ComponentCompilationEnabledConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for ComponentCompilationEnabledConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9091,
            client_config: GrpcClientConfig::default(),
        }
    }
}

impl ComponentCompilationEnabledConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrecreatedAccount {
    pub id: AccountId,
    pub name: String,
    pub email: AccountEmail,
    pub token: TokenSecret,
    pub plan_id: PlanId,
    pub role: AccountRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrecreatedPlan {
    pub plan_id: PlanId,
    pub plan_name: PlanName,
    pub app_limit: u64,
    pub env_limit: u64,
    pub component_limit: u64,
    pub worker_limit: u64,
    pub worker_connection_limit: u64,
    pub storage_limit: u64,
    pub monthly_gas_limit: u64,
    pub monthly_upload_limit: u64,
    pub max_memory_per_worker: u64,
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
