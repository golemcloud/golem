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

use golem_common::config::ConfigLoader;
use golem_common::config::DbConfig;
use golem_common::model::auth::Role;
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_service_base::config::BlobStorageConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    pub component_transformer_plugin_caller: ComponentTransformerPluginCallerConfig, // TODO:
    // pub plans: PlansConfig,
    // pub accounts: AccountsConfig,
    pub blob_storage: BlobStorageConfig,
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
            blob_storage: BlobStorageConfig::default(),
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
#[serde(tag = "type", content = "config")]
pub enum LoginConfig {
    OAuth2(OAuth2Config),
    Disabled(Empty),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubOAuth2Config {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: url::Url,
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

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ComponentTransformerPluginCallerConfig {
    pub retries: RetryConfig,
}

pub fn make_config_loader() -> ConfigLoader<RegistryServiceConfig> {
    ConfigLoader::new(&PathBuf::from("config/registry-service.toml"))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::make_config_loader;

    // TODO
    #[ignore]
    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
