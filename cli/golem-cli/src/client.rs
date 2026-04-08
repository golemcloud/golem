// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
    AccountClientLive, AccountSummaryClientLive, AgentClientLive, AgentSecretsClientLive,
    AgentTypesClientLive, ApiDeploymentClientLive, ApiDomainClientLive, ApiSecurityClientLive,
    ApplicationClientLive, ComponentClientLive, DeploymentClientLive, EnvironmentClientLive,
    HealthCheckClientLive, LoginClientLive, McpDeploymentClientLive, PluginClientLive,
    TokenClientLive, WorkerClientLive,
};
use golem_client::{Context as ClientContext, Security};
use golem_common::base_model::api;
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use http::Extensions;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Request, Response, StatusCode};
use reqwest_middleware::{ClientBuilder, Middleware, Next};
use reqwest_retry::{
    Jitter, RetryTransientMiddleware, Retryable, RetryableStrategy, default_on_request_failure,
    policies::ExponentialBackoff,
};
use std::borrow::Cow;
use std::path::Path;
use std::time::Duration;

const RETRY_MAX_RETRIES: u32 = 3;
const RETRY_MIN_DELAY: Duration = Duration::from_millis(150);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy)]
pub enum RetryProfile {
    ServiceDefault,
    InvokeConservative,
}

#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub min_delay: Duration,
    pub max_delay: Duration,
    pub profile: RetryProfile,
}

#[derive(Debug, Clone, Copy)]
pub struct MiddlewareConfig {
    pub retry: Option<RetryConfig>,
    pub with_kill_switch: bool,
    pub with_static_headers: bool,
}

impl MiddlewareConfig {
    pub const fn with_service_retry() -> Self {
        Self {
            retry: Some(RetryConfig {
                max_retries: RETRY_MAX_RETRIES,
                min_delay: RETRY_MIN_DELAY,
                max_delay: RETRY_MAX_DELAY,
                profile: RetryProfile::ServiceDefault,
            }),
            with_kill_switch: true,
            with_static_headers: true,
        }
    }

    pub const fn with_invoke_retry() -> Self {
        Self {
            retry: Some(RetryConfig {
                max_retries: RETRY_MAX_RETRIES,
                min_delay: RETRY_MIN_DELAY,
                max_delay: RETRY_MAX_DELAY,
                profile: RetryProfile::InvokeConservative,
            }),
            with_kill_switch: true,
            with_static_headers: true,
        }
    }

    pub const fn without_retry() -> Self {
        Self {
            retry: None,
            with_kill_switch: true,
            with_static_headers: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ServiceRetryStrategy;

impl RetryableStrategy for ServiceRetryStrategy {
    fn handle(
        &self,
        res: &Result<reqwest::Response, reqwest_middleware::Error>,
    ) -> Option<Retryable> {
        match res {
            Ok(success)
                if success.status().is_server_error()
                    || success.status() == StatusCode::REQUEST_TIMEOUT
                    || success.status() == StatusCode::TOO_MANY_REQUESTS =>
            {
                Some(Retryable::Transient)
            }
            Ok(success) if success.status().is_success() => None,
            Ok(_) => Some(Retryable::Fatal),
            Err(error) => default_on_request_failure(error),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct InvokeRetryStrategy;

impl RetryableStrategy for InvokeRetryStrategy {
    fn handle(
        &self,
        res: &Result<reqwest::Response, reqwest_middleware::Error>,
    ) -> Option<Retryable> {
        match res {
            Ok(success)
                if matches!(
                    success.status(),
                    StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT
                ) =>
            {
                Some(Retryable::Transient)
            }
            Ok(success) if success.status().is_success() => None,
            Ok(_) => Some(Retryable::Fatal),
            Err(error) => default_on_request_failure(error),
        }
    }
}

#[derive(Debug, Clone)]
struct StaticHeadersMiddleware {
    cli_version: String,
    cli_platform: String,
}

#[async_trait::async_trait]
impl Middleware for StaticHeadersMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        let mut req = req;
        req.headers_mut().insert(
            HeaderName::from_static(api::header::GOLEM_CLI_VERSION),
            HeaderValue::from_str(&self.cli_version)
                .map_err(reqwest_middleware::Error::middleware)?,
        );
        req.headers_mut().insert(
            HeaderName::from_static(api::header::GOLEM_CLI_PLATFORM),
            HeaderValue::from_str(&self.cli_platform)
                .map_err(reqwest_middleware::Error::middleware)?,
        );

        next.run(req, extensions).await
    }
}

struct RetryIfCloneableMiddleware<T: Middleware> {
    inner: T,
}

#[async_trait::async_trait]
impl<T: Middleware> Middleware for RetryIfCloneableMiddleware<T> {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        if req.try_clone().is_some() {
            self.inner.handle(req, extensions, next).await
        } else {
            next.run(req, extensions).await
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct GoneErrorBody {
    error: Option<String>,
    code: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct KillSwitchMiddleware;

#[async_trait::async_trait]
impl Middleware for KillSwitchMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        let response = next.run(req, extensions).await?;

        if response.status() == StatusCode::GONE {
            let body = response
                .bytes()
                .await
                .map_err(reqwest_middleware::Error::middleware)?;

            let parsed: Option<GoneErrorBody> = serde_json::from_slice(&body).ok();

            let code = parsed.as_ref().and_then(|b| b.code.as_deref());
            let message = parsed
                .as_ref()
                .and_then(|b| b.error.as_deref())
                .unwrap_or("To use the currently selected server you have to update your CLI!");

            if code == Some(api::error_code::CLI_UPDATE_REQUIRED) {
                return Err(reqwest_middleware::Error::middleware(
                    std::io::Error::other(message.to_string()),
                ));
            }

            return Err(reqwest_middleware::Error::middleware(
                std::io::Error::other(format!("Server returned 410 Gone: {message}")),
            ));
        }

        Ok(response)
    }
}

pub struct GolemClients {
    authentication: Authentication,

    pub account: AccountClientLive,
    pub account_summary: AccountSummaryClientLive,
    pub agent: AgentClientLive,
    pub agent_secrets: AgentSecretsClientLive,
    pub agent_types: AgentTypesClientLive,
    pub api_deployment: ApiDeploymentClientLive,
    pub api_domain: ApiDomainClientLive,
    pub api_security: ApiSecurityClientLive,
    pub application: ApplicationClientLive,
    pub component: ComponentClientLive,
    pub component_healthcheck: HealthCheckClientLive,
    pub deployment: DeploymentClientLive,
    pub environment: EnvironmentClientLive,
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
        let without_retry = MiddlewareConfig::without_retry();
        let with_service_retry = MiddlewareConfig::with_service_retry();
        let with_invoke_retry = MiddlewareConfig::with_invoke_retry();

        let healthcheck_http_client =
            new_reqwest_client(&config.health_check_http_client_config, &without_retry)?;
        let service_http_client =
            new_reqwest_client(&config.service_http_client_config, &with_service_retry)?;
        let invoke_http_client =
            new_reqwest_client(&config.invoke_http_client_config, &with_invoke_retry)?;

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
            agent_secrets: AgentSecretsClientLive {
                context: registry_context(),
            },
            agent_types: AgentTypesClientLive {
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

pub fn new_reqwest_client(
    config: &HttpClientConfig,
    middleware_config: &MiddlewareConfig,
) -> anyhow::Result<reqwest_middleware::ClientWithMiddleware> {
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

    let client = builder.connection_verbose(true).build()?;
    let mut builder = ClientBuilder::new(client);

    if let Some(retry_config) = middleware_config.retry {
        let retry_policy = ExponentialBackoff::builder()
            .retry_bounds(retry_config.min_delay, retry_config.max_delay)
            .jitter(Jitter::Bounded)
            .base(2)
            .build_with_max_retries(retry_config.max_retries);

        builder = match retry_config.profile {
            RetryProfile::ServiceDefault => builder.with(RetryIfCloneableMiddleware {
                inner: RetryTransientMiddleware::new_with_policy_and_strategy(
                    retry_policy,
                    ServiceRetryStrategy,
                ),
            }),
            RetryProfile::InvokeConservative => builder.with(RetryIfCloneableMiddleware {
                inner: RetryTransientMiddleware::new_with_policy_and_strategy(
                    retry_policy,
                    InvokeRetryStrategy,
                ),
            }),
        };
    }

    if middleware_config.with_kill_switch {
        builder = builder.with(KillSwitchMiddleware);
    }

    if middleware_config.with_static_headers {
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        builder = builder.with(StaticHeadersMiddleware {
            cli_version: crate::version().to_string(),
            cli_platform: platform,
        });
    }

    Ok(builder.build())
}

pub fn new_raw_reqwest_client(config: &HttpClientConfig) -> anyhow::Result<reqwest::Client> {
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
