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

use crate::auth::{Auth, Authentication};
use crate::config::{AuthenticationConfigWithSource, ClientConfig, HttpClientConfig};
use anyhow::bail;
use golem_client::api::{
    AccountClientLive, AccountSummaryClientLive, AgentClientLive, AgentTypesClientLive,
    ApiDeploymentClientLive, ApiDomainClientLive, ApiSecurityClientLive, ApplicationClientLive,
    ComponentClientLive, DeploymentClientLive, EnvironmentClientLive, GrantClientLive,
    HealthCheckClientLive, HttpApiDefinitionClientLive, LimitsClientLive, LoginClientLive,
    PluginClientLive, TokenClientLive, WorkerClientLive,
    AccountClientLive, AccountSummaryClientLive, AgentTypesClientLive, ApiDeploymentClientLive,
    ApiDomainClientLive, ApiSecurityClientLive, ApplicationClientLive, ComponentClientLive,
    DeploymentClientLive, EnvironmentClientLive, GrantClientLive, HealthCheckClientLive,
    HttpApiDefinitionClientLive, LimitsClientLive, LoginClientLive, McpDeploymentClientLive,
    PluginClientLive, TokenClientLive, WorkerClientLive,
};
use golem_client::{Context as ClientContext, Security};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use std::borrow::Cow;
use std::path::Path;

pub struct GolemClients {
    authentication: Authentication,

    pub account: AccountClientLive,
    pub account_summary: AccountSummaryClientLive,
    pub agent: AgentClientLive,
    pub agent_types: AgentTypesClientLive,
    pub api_definition: HttpApiDefinitionClientLive,
    pub api_deployment: ApiDeploymentClientLive,
    pub api_domain: ApiDomainClientLive,
    pub api_security: ApiSecurityClientLive,
    pub application: ApplicationClientLive,
    pub component: ComponentClientLive,
    pub component_healthcheck: HealthCheckClientLive,
    pub deployment: DeploymentClientLive,
    pub environment: EnvironmentClientLive,
    pub grant: GrantClientLive,
    pub limits: LimitsClientLive,
    pub login: LoginClientLive,
    pub mcp_deployment: McpDeploymentClientLive,
    pub plugin: PluginClientLive,
    pub token: TokenClientLive,
    pub worker: WorkerClientLive,
    pub worker_invoke: WorkerClientLive,
}

impl GolemClients {
    pub async fn new(
        config: &ClientConfig,
        token_override: Option<String>,
        auth_config: &AuthenticationConfigWithSource,
        config_dir: &Path,
    ) -> anyhow::Result<Self> {
        let healthcheck_http_client = new_reqwest_client(&config.health_check_http_client_config)?;

        let service_http_client = new_reqwest_client(&config.service_http_client_config)?;
        let invoke_http_client = new_reqwest_client(&config.invoke_http_client_config)?;

        let auth = Auth::new(LoginClientLive {
            context: ClientContext {
                client: service_http_client.clone(),
                base_url: config.registry_url.clone(),
                security_token: Security::Empty,
            },
        });

        let authentication = auth
            .authenticate(token_override, auth_config, config_dir)
            .await?;

        let security_token = Security::Bearer(authentication.0.secret.secret().to_string());

        let registry_context = || ClientContext {
            client: service_http_client.clone(),
            base_url: config.registry_url.clone(),
            security_token: security_token.clone(),
        };

        let registry_healthcheck_context = || ClientContext {
            client: healthcheck_http_client,
            base_url: config.registry_url.clone(),
            security_token: Security::Empty,
        };

        let worker_context = || ClientContext {
            client: service_http_client.clone(),
            base_url: config.worker_url.clone(),
            security_token: security_token.clone(),
        };

        let worker_invoke_context = || ClientContext {
            client: invoke_http_client.clone(),
            base_url: config.worker_url.clone(),
            security_token: security_token.clone(),
        };

        let login_context = || ClientContext {
            client: service_http_client.clone(),
            base_url: config.registry_url.clone(),
            security_token: security_token.clone(),
        };

        Ok(GolemClients {
            authentication,
            account: AccountClientLive {
                context: registry_context(),
            },
            account_summary: AccountSummaryClientLive {
                context: registry_context(),
            },
            agent: AgentClientLive {
                context: worker_invoke_context(),
            },
            agent_types: AgentTypesClientLive {
                context: registry_context(),
            },
            api_definition: HttpApiDefinitionClientLive {
                context: registry_context(),
            },
            api_deployment: ApiDeploymentClientLive {
                context: registry_context(),
            },
            api_domain: ApiDomainClientLive {
                context: registry_context(),
            },
            api_security: ApiSecurityClientLive {
                context: registry_context(),
            },
            application: ApplicationClientLive {
                context: registry_context(),
            },
            component: ComponentClientLive {
                context: registry_context(),
            },
            component_healthcheck: HealthCheckClientLive {
                context: registry_healthcheck_context(),
            },
            deployment: DeploymentClientLive {
                context: registry_context(),
            },
            environment: EnvironmentClientLive {
                context: registry_context(),
            },
            grant: GrantClientLive {
                context: registry_context(),
            },
            limits: LimitsClientLive {
                context: worker_context(),
            },
            login: LoginClientLive {
                context: login_context(),
            },
            mcp_deployment: McpDeploymentClientLive {
                context: registry_context(),
            },
            plugin: PluginClientLive {
                context: registry_context(),
            },
            token: TokenClientLive {
                context: registry_context(),
            },
            worker: WorkerClientLive {
                context: worker_context(),
            },
            worker_invoke: WorkerClientLive {
                context: worker_invoke_context(),
            },
        })
    }

    pub fn account_id(&self) -> &AccountId {
        self.authentication.account_id()
    }

    pub fn auth_token(&self) -> &TokenSecret {
        &self.authentication.0.secret
    }
}

pub fn new_reqwest_client(config: &HttpClientConfig) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();

    if config.allow_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }

    if let Some(timeout) = config.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect_timeout) = config.connect_timeout {
        builder = builder.connect_timeout(connect_timeout);
    }
    if let Some(read_timeout) = config.read_timeout {
        builder = builder.read_timeout(read_timeout);
    }

    Ok(builder.connection_verbose(true).build()?)
}

pub async fn check_http_response_success(
    response: reqwest::Response,
) -> anyhow::Result<reqwest::Response> {
    if !response.status().is_success() {
        let url = response.url().clone();
        let status = response.status();
        let bytes = response.bytes().await.ok();
        let error_payload = bytes
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes.as_ref()))
            .unwrap_or_else(|| Cow::from(""));

        bail!(
            "Received unexpected response for {}: {}\n{}",
            url,
            status,
            error_payload
        );
    }
    Ok(response)
}
