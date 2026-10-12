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
use crate::log::{LogIndent, log_warn_action, logln};
use anyhow::bail;
use golem_client::api::{
    AccountClientLive, AccountSummaryClientLive, AgentClientLive, AgentSecretsClientLive,
    AgentTypesClientLive, ApiDeploymentClientLive, ApiDomainClientLive, ApiSecurityClientLive,
    ApplicationClientLive, CardClientLive, ComponentClientLive, DeploymentClientLive,
    EnvironmentClientLive, EnvironmentToolGrantsClientLive,
    EnvironmentToolMiddlewareGrantsClientLive, LoginClientLive, McpDeploymentClientLive,
    MeClientLive, PermissionSharesClientLive, PluginClientLive, ResourcesClientLive,
    RetryPoliciesClientLive, TokenClientLive, ToolMiddlewareReleasesClientLive,
    ToolReleasesClientLive, WorkerClientLive,
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
use std::time::{Duration, Instant};
use url::Url;

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
            Ok(response) if is_server_starting(response) => Some(Retryable::Fatal),
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
            Ok(response) if is_server_starting(response) => Some(Retryable::Fatal),
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
struct ErrorResponseBody {
    error: Option<String>,
    code: Option<String>,
}

/// The server has bound its API port but is still starting, so it cannot serve the request yet.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ServerStartingError {
    pub message: String,
}

fn is_server_starting(response: &Response) -> bool {
    response.status() == StatusCode::SERVICE_UNAVAILABLE
        && response
            .headers()
            .get(api::header::GOLEM_SERVER_STATUS)
            .is_some_and(|value| value == api::header::GOLEM_SERVER_STATUS_STARTING)
}

async fn server_starting_error(response: Response) -> ServerStartingError {
    let message = response
        .bytes()
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<ErrorResponseBody>(&body).ok())
        .and_then(|body| body.error)
        .unwrap_or_else(|| "The Golem server is still starting".to_string());
    ServerStartingError { message }
}

#[derive(Debug, Clone, Copy)]
struct ServerStartingMiddleware;

#[async_trait::async_trait]
impl Middleware for ServerStartingMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        let response = next.run(req, extensions).await?;
        if is_server_starting(&response) {
            return Err(reqwest_middleware::Error::middleware(
                server_starting_error(response).await,
            ));
        }
        Ok(response)
    }
}

const SERVER_STARTUP_WAIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const SERVER_STARTUP_POLL_INTERVAL: Duration = Duration::from_secs(1);
const SERVER_STARTUP_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Waits while the server at `base_url` reports that it is still starting.
///
/// Returns as soon as the server answers in any other way, including when it cannot be reached;
/// the requests that follow report those failures.
pub async fn wait_for_server_startup(
    client: &reqwest::Client,
    base_url: &Url,
) -> anyhow::Result<()> {
    wait_for_server_startup_with(
        client,
        base_url,
        SERVER_STARTUP_WAIT_TIMEOUT,
        SERVER_STARTUP_POLL_INTERVAL,
    )
    .await
}

async fn wait_for_server_startup_with(
    client: &reqwest::Client,
    base_url: &Url,
    timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<()> {
    let mut healthcheck_url = base_url.clone();
    healthcheck_url
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Invalid server URL: {base_url}"))?
        .pop_if_empty()
        .push("healthcheck");

    let started_at = Instant::now();
    let mut indent: Option<LogIndent> = None;
    let mut last_logged: Option<(String, Instant)> = None;

    loop {
        let response = match client.get(healthcheck_url.clone()).send().await {
            Ok(response) => response,
            Err(_) => return Ok(()),
        };
        if !is_server_starting(&response) {
            return Ok(());
        }
        let message = server_starting_error(response).await.message;

        if started_at.elapsed() >= timeout {
            bail!(
                "The server at {base_url} did not finish starting within {} seconds. Last status: {message}",
                timeout.as_secs()
            );
        }

        if indent.is_none() {
            log_warn_action(
                "Waiting",
                format!("for the server at {base_url} to finish starting"),
            );
            indent = Some(LogIndent::new());
        }
        if should_log_startup_progress(last_logged.as_ref(), &message) {
            logln(&message);
            last_logged = Some((message, Instant::now()));
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Logs every new startup phase, but progress updates within one phase (such as downloaded bytes)
/// at most once per [`SERVER_STARTUP_PROGRESS_LOG_INTERVAL`].
fn should_log_startup_progress(last_logged: Option<&(String, Instant)>, message: &str) -> bool {
    fn phase(message: &str) -> &str {
        message.split_once(" (").map_or(message, |(phase, _)| phase)
    }

    match last_logged {
        None => true,
        Some((last_message, _)) if last_message == message => false,
        Some((last_message, _)) if phase(last_message) != phase(message) => true,
        Some((_, logged_at)) => logged_at.elapsed() >= SERVER_STARTUP_PROGRESS_LOG_INTERVAL,
    }
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

            let parsed: Option<ErrorResponseBody> = serde_json::from_slice(&body).ok();

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
    pub retry_policies: RetryPoliciesClientLive,
    pub resources: ResourcesClientLive,
    pub api_deployment: ApiDeploymentClientLive,
    pub api_domain: ApiDomainClientLive,
    pub api_security: ApiSecurityClientLive,
    pub application: ApplicationClientLive,
    pub card: CardClientLive,
    pub component: ComponentClientLive,
    pub component_upload: ComponentClientLive,
    pub deployment: DeploymentClientLive,
    pub environment: EnvironmentClientLive,
    pub environment_tool_grants: EnvironmentToolGrantsClientLive,
    pub environment_tool_middleware_grants: EnvironmentToolMiddlewareGrantsClientLive,
    pub tool_releases: ToolReleasesClientLive,
    pub tool_middleware_releases: ToolMiddlewareReleasesClientLive,
    pub login: LoginClientLive,
    pub mcp_deployment: McpDeploymentClientLive,
    pub me: MeClientLive,
    pub permission_shares: PermissionSharesClientLive,
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
        let with_service_retry = MiddlewareConfig::with_service_retry();
        let with_invoke_retry = MiddlewareConfig::with_invoke_retry();

        let service_http_client =
            new_reqwest_client(&config.service_http_client_config, &with_service_retry)?;
        let invoke_http_client =
            new_reqwest_client(&config.invoke_http_client_config, &with_invoke_retry)?;
        let component_upload_http_client = new_reqwest_client(
            &config.component_upload_http_client_config,
            &MiddlewareConfig::without_retry(),
        )?;

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

        let worker_context = || ClientContext {
            client: service_http_client.clone(),
            base_url: config.worker_url.clone(),
            security_token: security_token.clone(),
        };

        let component_upload_context = || ClientContext {
            client: component_upload_http_client.clone(),
            base_url: config.registry_url.clone(),
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
            retry_policies: RetryPoliciesClientLive {
                context: registry_context(),
            },
            agent_types: AgentTypesClientLive {
                context: registry_context(),
            },
            resources: ResourcesClientLive {
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
            card: CardClientLive {
                context: registry_context(),
            },
            component: ComponentClientLive {
                context: registry_context(),
            },
            component_upload: ComponentClientLive {
                context: component_upload_context(),
            },
            deployment: DeploymentClientLive {
                context: registry_context(),
            },
            environment: EnvironmentClientLive {
                context: registry_context(),
            },
            environment_tool_grants: EnvironmentToolGrantsClientLive {
                context: registry_context(),
            },
            environment_tool_middleware_grants: EnvironmentToolMiddlewareGrantsClientLive {
                context: registry_context(),
            },
            tool_releases: ToolReleasesClientLive {
                context: registry_context(),
            },
            tool_middleware_releases: ToolMiddlewareReleasesClientLive {
                context: registry_context(),
            },
            login: LoginClientLive {
                context: login_context(),
            },
            mcp_deployment: McpDeploymentClientLive {
                context: registry_context(),
            },
            me: MeClientLive {
                context: registry_context(),
            },
            permission_shares: PermissionSharesClientLive {
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
    let mut builder = ClientBuilder::new(client).with(ServerStartingMiddleware);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::service::{ServiceError, ServiceErrorKind};
    use poem::listener::{Acceptor, Listener};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    fn starting_response() -> poem::Response {
        poem::Response::builder()
            .status(poem::http::StatusCode::SERVICE_UNAVAILABLE)
            .header(
                api::header::GOLEM_SERVER_STATUS,
                api::header::GOLEM_SERVER_STATUS_STARTING,
            )
            .content_type("application/json")
            .body(
                serde_json::json!({
                    "code": api::error_code::SERVER_STARTING,
                    "error": "The Golem server is still starting: provisioning built-in tools",
                })
                .to_string(),
            )
    }

    /// Serves `starting_requests` "still starting" responses, then `200 OK`.
    async fn start_server(starting_requests: usize) -> (Url, Arc<AtomicUsize>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await
            .unwrap();
        let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
        let endpoint = poem::endpoint::make({
            let requests = requests.clone();
            move |_| {
                let request = requests.fetch_add(1, Ordering::SeqCst);
                async move {
                    if request < starting_requests {
                        starting_response()
                    } else {
                        poem::Response::builder().body("ok")
                    }
                }
            }
        });
        tokio::spawn(async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(endpoint)
                .await
                .unwrap();
        });
        (
            Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
            requests,
        )
    }

    #[test]
    async fn waits_until_the_server_finished_starting() {
        let (url, requests) = start_server(2).await;

        wait_for_server_startup_with(
            &reqwest::Client::new(),
            &url,
            Duration::from_secs(30),
            Duration::from_millis(10),
        )
        .await
        .unwrap();

        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    async fn waiting_gives_up_after_the_timeout() {
        let (url, _) = start_server(usize::MAX).await;

        let error = wait_for_server_startup_with(
            &reqwest::Client::new(),
            &url,
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains(
                "Last status: The Golem server is still starting: provisioning built-in tools"
            ),
            "{error}"
        );
    }

    #[test]
    async fn waiting_returns_when_the_server_is_unreachable() {
        let (url, _) = start_server(0).await;
        let unreachable = Url::parse("http://127.0.0.1:1/").unwrap();

        wait_for_server_startup_with(
            &reqwest::Client::new(),
            &unreachable,
            Duration::from_secs(30),
            Duration::from_millis(10),
        )
        .await
        .unwrap();
        wait_for_server_startup_with(
            &reqwest::Client::new(),
            &url,
            Duration::from_secs(30),
            Duration::from_millis(10),
        )
        .await
        .unwrap();
    }

    #[test]
    async fn starting_server_is_reported_without_retries() {
        let (url, requests) = start_server(usize::MAX).await;
        let client = new_reqwest_client(
            &HttpClientConfig::new_for_service_calls(false),
            &MiddlewareConfig::with_service_retry(),
        )
        .unwrap();

        let error = client
            .get(url.join("v1/me").unwrap())
            .send()
            .await
            .unwrap_err();

        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let rendered = ServiceError {
            service_name: "Me",
            kind: ServiceErrorKind::MiddlewareError(error),
        }
        .to_string();
        assert!(
            rendered.contains(
                "The Golem server is still starting: provisioning built-in tools. Retry when the server is ready."
            ),
            "{rendered}"
        );
    }

    #[test]
    async fn connection_failure_after_retries_names_the_server() {
        let client = new_reqwest_client(
            &HttpClientConfig::new_for_service_calls(false),
            &MiddlewareConfig::with_service_retry(),
        )
        .unwrap();

        let error = client
            .get("http://127.0.0.1:1/v1/me")
            .send()
            .await
            .unwrap_err();

        let rendered = ServiceError {
            service_name: "Me",
            kind: ServiceErrorKind::MiddlewareError(error),
        }
        .render();
        assert!(
            rendered.contains("Cannot connect to")
                && rendered.contains("http://127.0.0.1:1")
                && rendered.contains("golem server run"),
            "{rendered}"
        );
        assert!(!rendered.contains("retries"), "{rendered}");
    }

    #[test]
    fn startup_progress_logging_is_throttled_within_a_phase() {
        let just_now = Instant::now();
        let long_ago = just_now - SERVER_STARTUP_PROGRESS_LOG_INTERVAL;
        let fetching = "The Golem server is still starting: fetching built-in tools (1/6 ready)";
        let fetching_more =
            "The Golem server is still starting: fetching built-in tools (2/6 ready)";
        let provisioning = "The Golem server is still starting: provisioning built-in tools";

        assert!(should_log_startup_progress(None, fetching));
        assert!(!should_log_startup_progress(
            Some(&(fetching.to_string(), long_ago)),
            fetching
        ));
        assert!(!should_log_startup_progress(
            Some(&(fetching.to_string(), just_now)),
            fetching_more
        ));
        assert!(should_log_startup_progress(
            Some(&(fetching.to_string(), long_ago)),
            fetching_more
        ));
        assert!(should_log_startup_progress(
            Some(&(fetching.to_string(), just_now)),
            provisioning
        ));
    }
}
