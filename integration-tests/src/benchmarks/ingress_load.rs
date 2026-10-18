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

//! Load on the ingress path from many independent clients.
//!
//! The other benchmarks send all their requests through one shared HTTP client, so the
//! whole run reaches the cluster over about one connection per host. That measures
//! Golem, not the proxies in front of it. Here every account is a client of its own,
//! with its own connection and token, and the clients send at a fixed rate whether or
//! not the earlier requests have returned.
//!
//! All accounts call agents in one environment, shared with them by permission shares,
//! so every token is different but the executors load one component. A small pool of
//! environments gives the HTTP API its domains.

use crate::benchmarks::{cleanup_account, cleanup_user_state, ingress_settings};
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt, stream};
use golem_client::api::RegistryServiceClient;
use golem_client::model::{AgentInvocationMode, AgentInvocationRequest};
use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName,
};
use golem_common::schema::ExternalSchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, IngressSettings, ResultKey,
    RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use reqwest::Url;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{Level, info, warn};

const AGENT_TYPE: &str = "RustBenchmarkAgent";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Requests a client keeps open at a time. A client that reaches it skips its next
/// requests and counts them as dropped, so an overloaded cluster shows up as drops
/// instead of as an ever growing number of open requests on the runner.
const MAX_IN_FLIGHT_PER_CLIENT: usize = 64;
/// Failure messages kept per path and step. The counts include all failures.
const MAX_FAILURE_MESSAGES: usize = 100;
const WARMUP_ATTEMPTS: usize = 3;

pub struct IngressLoad {
    config: RunConfig,
    settings: IngressSettings,
}

pub struct IngressLoadContext {
    deps: BenchmarkTestDependencies,
}

pub struct IngressLoadIteration {
    owner: TestUserContext<BenchmarkTestDependencies>,
    owner_env_id: EnvironmentId,
    domain_owners: Vec<(TestUserContext<BenchmarkTestDependencies>, EnvironmentId)>,
    accounts: Arc<Vec<Account>>,
    rest_url: Url,
}

/// One client: an account with its own connection, token and agents.
struct Account {
    user: TestUserContext<BenchmarkTestDependencies>,
    bearer: String,
    client: AccountClient,
    /// The request body for each agent of the account, for the REST invoke endpoint.
    rest_bodies: Vec<bytes::Bytes>,
    /// The URL and, outside the cloud, the Host header for each HTTP API agent.
    http_api_targets: Vec<(Url, Option<String>)>,
}

struct AccountClient {
    http1: bool,
    requests_per_connection: usize,
    state: Mutex<(reqwest::Client, usize)>,
}

impl AccountClient {
    fn new(http1: bool, requests_per_connection: usize) -> Self {
        Self {
            http1,
            requests_per_connection,
            state: Mutex::new((build_client(http1), 0)),
        }
    }

    /// The client for the next request. After `requests_per_connection` requests the
    /// client is replaced, and its connections close once their open requests finish.
    fn next(&self, clients_created: &AtomicU64) -> reqwest::Client {
        let mut state = self.state.lock().unwrap();
        if self.requests_per_connection > 0 && state.1 >= self.requests_per_connection {
            *state = (build_client(self.http1), 0);
            clients_created.fetch_add(1, Ordering::Relaxed);
        }
        state.1 += 1;
        state.0.clone()
    }
}

fn build_client(http1: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .tcp_nodelay(true)
        .timeout(REQUEST_TIMEOUT)
        .pool_idle_timeout(Duration::from_secs(90));
    let builder = if http1 { builder.http1_only() } else { builder };
    builder
        .build()
        .expect("Failed to build the client of an account")
}

#[derive(Clone, Copy)]
enum Path {
    Rest,
    HttpApi,
}

impl Path {
    fn prefix(self) -> &'static str {
        match self {
            Path::Rest => "rest",
            Path::HttpApi => "http-api",
        }
    }
}

#[derive(Default)]
struct PathStats {
    succeeded: AtomicU64,
    failed: AtomicU64,
    messages: Mutex<Vec<String>>,
}

impl PathStats {
    fn fail(&self, message: String) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        let mut messages = self.messages.lock().unwrap();
        if messages.len() < MAX_FAILURE_MESSAGES {
            messages.push(message);
        }
    }
}

#[derive(Default)]
struct StepStats {
    rest: PathStats,
    http_api: PathStats,
    dropped: AtomicU64,
    clients_created: AtomicU64,
}

impl StepStats {
    fn path(&self, path: Path) -> &PathStats {
        match path {
            Path::Rest => &self.rest,
            Path::HttpApi => &self.http_api,
        }
    }
}

fn setup_error(message: impl std::fmt::Display) -> BenchmarkError {
    BenchmarkError::new("setup", format!("{message:#}"))
}

fn agent(name: String) -> ParsedAgentId {
    agent_id!(AGENT_TYPE, name)
}

impl IngressLoad {
    fn rest_url(deps: &BenchmarkTestDependencies) -> Url {
        let worker_service = deps.worker_service();
        let base = if deps.apps_base_domain().is_some() {
            format!("https://{}", worker_service.http_host())
        } else {
            format!(
                "http://{}:{}",
                worker_service.http_host(),
                worker_service.http_port()
            )
        };
        Url::parse(&format!("{base}/v1/agents/invoke-agent")).expect("Invalid REST invoke URL")
    }

    /// The base URL and, outside the cloud, the Host header for an HTTP API domain.
    fn http_api_base(
        deps: &BenchmarkTestDependencies,
        domain: &Domain,
    ) -> (String, Option<String>) {
        if deps.apps_base_domain().is_some() {
            (format!("https://{}", domain.0), None)
        } else {
            let port = deps.worker_service().custom_request_port();
            (format!("http://localhost:{port}"), Some(domain.0.clone()))
        }
    }

    /// An environment with the agent component, its own domain and an HTTP API
    /// deployment of the agent.
    async fn setup_domain(
        deps: &BenchmarkTestDependencies,
    ) -> anyhow::Result<(
        TestUserContext<BenchmarkTestDependencies>,
        EnvironmentId,
        Domain,
    )> {
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        user.component(&env.id, "benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await?;

        let apps_base_domain = deps.apps_base_domain().unwrap_or("golem.cloud");
        let domain = Domain(format!("{}.{}", env.id, apps_base_domain));
        let client = user.registry_service_client().await;
        client
            .create_domain_registration(
                &env.id.0,
                &DomainRegistrationCreation {
                    domain: domain.clone(),
                },
            )
            .await?;
        client
            .create_http_api_deployment(
                &env.id.0,
                &HttpApiDeploymentCreation {
                    scheme: Default::default(),
                    domain: domain.clone(),
                    webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
                    openapi_endpoint_prefix:
                        HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
                    agents: BTreeMap::from_iter([(
                        AgentTypeName(AGENT_TYPE.to_string()),
                        HttpApiDeploymentAgentOptions::default(),
                    )]),
                },
            )
            .await?;
        user.deploy_environment(env.id).await?;
        Ok((user, env.id, domain))
    }

    /// Lets `grantee` see the shared environment and invoke its agents.
    async fn share_environment(
        owner: &TestUserContext<BenchmarkTestDependencies>,
        app_name: &str,
        env_name: &str,
        component_name: &str,
        grantee: &TestUserContext<BenchmarkTestDependencies>,
        index: usize,
    ) -> anyhow::Result<()> {
        let owner_email = owner.account_email.as_str();
        let grantee_email = grantee.account_email.as_str();
        let agents = format!("agent({owner_email}/{app_name}/{env_name}/{component_name}/*)");
        owner
            .registry_service_client()
            .await
            .create_permission_share(
                &owner.account_id.0,
                &PermissionShareCreation {
                    target_account_email: grantee.account_email.clone(),
                    name: PermissionShareName(format!("ingress-load-{index}")),
                    data: PermissionShareData {
                        lower_positive: vec![
                            format!(
                                "environment({owner_email}/{app_name}/{env_name}) @ {grantee_email} : view :"
                            ),
                            format!(
                                "component({owner_email}/{app_name}/{env_name}/*) @ {grantee_email} : view : *"
                            ),
                            format!("{agents} @ {grantee_email} : view : *"),
                            format!("{agents} @ {grantee_email} : invoke : *"),
                        ],
                        lower_negative: Vec::new(),
                        upper_positive: Vec::new(),
                        upper_negative: Vec::new(),
                    },
                },
            )
            .await?;
        Ok(())
    }

    fn rest_body(
        owner_email: &str,
        app_name: &str,
        env_name: &str,
        agent_id: &ParsedAgentId,
    ) -> anyhow::Result<bytes::Bytes> {
        let request = AgentInvocationRequest {
            app_name: app_name.to_string(),
            env_name: env_name.to_string(),
            agent_type_name: agent_id.agent_type.0.clone(),
            parameters: ExternalSchemaValue::try_from(agent_id.parameters.value().clone())
                .map_err(anyhow::Error::msg)?,
            phantom_id: agent_id.phantom_id,
            config: None,
            method_name: "echo".to_string(),
            method_parameters: ExternalSchemaValue::try_from(
                data_value!("benchmark").into_parts().1,
            )
            .map_err(anyhow::Error::msg)?,
            mode: AgentInvocationMode::Await,
            schedule_at: None,
            idempotency_key: None,
            deployment_revision: None,
            owner_account_email: Some(owner_email.to_string()),
        };
        Ok(bytes::Bytes::from(serde_json::to_vec(&request)?))
    }
}

/// Sends one request and returns why it failed, if it did.
async fn send(
    account: &Account,
    rest_url: &Url,
    path: Path,
    agent: usize,
    clients_created: &AtomicU64,
) -> Result<(), String> {
    let client = account.client.next(clients_created);
    let request = match path {
        Path::Rest => client
            .post(rest_url.clone())
            .bearer_auth(&account.bearer)
            .header("Content-Type", "application/json; charset=utf-8")
            .body(account.rest_bodies[agent].clone()),
        Path::HttpApi => {
            let (url, host) = &account.http_api_targets[agent];
            let request = client.post(url.clone());
            match host {
                Some(host) => request.header("Host", host),
                None => request,
            }
        }
    };
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            // Read the body, so the time includes the whole response.
            response
                .bytes()
                .await
                .map(|_| ())
                .map_err(|err| format!("{err}"))
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(format!(
                "{status}: {}",
                body.chars().take(300).collect::<String>()
            ))
        }
        Err(err) => Err(format!("{err}")),
    }
}

/// The request number `n` of a client goes through the HTTP API for `http_api_percent`
/// of the numbers, spread evenly, and to the REST endpoint for the others.
fn path_of(n: u64, http_api_percent: usize) -> Path {
    let percent = http_api_percent.min(100) as u64;
    if (n * percent) % 100 + percent >= 100 {
        Path::HttpApi
    } else {
        Path::Rest
    }
}

/// Sends from every account at `rate` requests per second in all, for `step`.
///
/// Each account sends at an equal share of the rate, starting at an offset so the
/// accounts do not all send at the same moment. A request is due at a fixed time and
/// its latency counts from that time, so a request that starts late because the
/// client fell behind counts the delay as well.
async fn drive(
    accounts: Arc<Vec<Account>>,
    rest_url: Arc<Url>,
    rate: f64,
    step: Duration,
    settings: &IngressSettings,
    stats: Arc<StepStats>,
    recorder: BenchmarkRecorder,
) {
    let count = accounts.len();
    let interval = Duration::from_secs_f64(count as f64 / rate);
    let start = Instant::now();
    let end = start + step;
    let agents = settings.agents_per_account.max(1);
    let http_api_percent = settings.http_api_percent;

    let clients = (0..count)
        .map(|index| {
            let accounts = accounts.clone();
            let rest_url = rest_url.clone();
            let stats = stats.clone();
            let recorder = recorder.clone();
            tokio::spawn(async move {
                let in_flight = Arc::new(AtomicUsize::new(0));
                let offset = interval.mul_f64(index as f64 / count as f64);
                let mut n: u64 = 0;
                loop {
                    let due = start + offset + interval.mul_f64(n as f64);
                    if due >= end {
                        break;
                    }
                    tokio::time::sleep_until(due).await;
                    let path = path_of(n, http_api_percent);
                    let agent = (n as usize / 2) % agents;
                    n += 1;

                    if in_flight.load(Ordering::Relaxed) >= MAX_IN_FLIGHT_PER_CLIENT {
                        stats.dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    in_flight.fetch_add(1, Ordering::Relaxed);
                    let accounts = accounts.clone();
                    let rest_url = rest_url.clone();
                    let stats = stats.clone();
                    let recorder = recorder.clone();
                    let in_flight = in_flight.clone();
                    tokio::spawn(async move {
                        let account = &accounts[index];
                        let result =
                            send(account, &rest_url, path, agent, &stats.clients_created).await;
                        let latency = due.elapsed();
                        match result {
                            Ok(()) => {
                                stats.path(path).succeeded.fetch_add(1, Ordering::Relaxed);
                                recorder.duration(
                                    &ResultKey::primary(format!("{}-invocation", path.prefix())),
                                    latency,
                                );
                            }
                            Err(message) => stats.path(path).fail(message),
                        }
                        in_flight.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                // Wait for the open requests of the client; the request timeout bounds it.
                while in_flight.load(Ordering::Relaxed) > 0 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
        })
        .collect::<Vec<_>>();

    for client in clients {
        if let Err(err) = client.await {
            warn!("An ingress-load client task failed: {err}");
        }
    }
}

#[async_trait]
impl Benchmark for IngressLoad {
    type BenchmarkContext = IngressLoadContext;
    type IterationContext = IngressLoadIteration;

    fn name() -> &'static str {
        "ingress-load"
    }

    fn description() -> &'static str {
        indoc! {
            "Sends `length` requests per second in all from `size` accounts, each one a client
            with its own connection and token, for the step length of the `ingress` settings of
            the suite. The accounts call the echo method of agents in one shared environment,
            through the REST invoke endpoint and through the HTTP API. The clients send at a fixed
            rate, whether or not their earlier requests have returned.
            "
        }
    }

    fn setup_deadline() -> Duration {
        Duration::from_secs(1800)
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(IngressLoadContext {
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
        })
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) -> BenchmarkResultValue {
        benchmark_context.deps.kill_all().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self {
            config,
            settings: ingress_settings(),
        })
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        _recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        let deps = &benchmark_context.deps;
        let settings = &self.settings;
        if self.config.size == 0 || self.config.length == 0 {
            return Err(setup_error(
                "size (accounts) and length (req/s) must be above 0",
            ));
        }

        info!("Setting up the shared environment");
        let owner = deps.user().await.map_err(setup_error)?;
        let (_, env) = owner.app_and_env().await.map_err(setup_error)?;
        let component = owner
            .component(&env.id, "benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await
            .map_err(setup_error)?;
        let app_name = env.application_name.0.clone();
        let env_name = env.name.0.clone();
        let component_name = component.component_name.0.clone();

        info!("Setting up {} HTTP API domains", settings.http_api_domains);
        let domains = stream::iter(0..settings.http_api_domains.max(1))
            .map(|_| Self::setup_domain(deps))
            .buffer_unordered(settings.setup_concurrency.max(1))
            .try_collect::<Vec<_>>()
            .await
            .map_err(setup_error)?;
        let domain_bases = domains
            .iter()
            .map(|(_, _, domain)| Self::http_api_base(deps, domain))
            .collect::<Vec<_>>();

        info!("Setting up {} accounts", self.config.size);
        let http1_accounts = self.config.size * settings.http1_percent.min(100) / 100;
        let owner_ref = &owner;
        let accounts = stream::iter(0..self.config.size)
            .map(|index| {
                let (app_name, env_name, component_name) = (&app_name, &env_name, &component_name);
                let domain_bases = &domain_bases;
                async move {
                    let user = deps.user().await?;
                    Self::share_environment(
                        owner_ref,
                        app_name,
                        env_name,
                        component_name,
                        &user,
                        index,
                    )
                    .await?;
                    let owner_email = owner_ref.account_email.as_str();
                    let mut rest_bodies = Vec::new();
                    let mut http_api_targets = Vec::new();
                    for n in 0..settings.agents_per_account.max(1) {
                        rest_bodies.push(Self::rest_body(
                            owner_email,
                            app_name,
                            env_name,
                            &agent(format!("a{index}-{n}")),
                        )?);
                        let (base, host) = &domain_bases[index % domain_bases.len()];
                        let url = Url::parse(&format!("{base}/rust/a{index}-{n}-http/echo/load"))?;
                        http_api_targets.push((url, host.clone()));
                    }
                    anyhow::Ok(Account {
                        bearer: user.token.secret().to_string(),
                        user,
                        client: AccountClient::new(
                            index < http1_accounts,
                            settings.requests_per_connection,
                        ),
                        rest_bodies,
                        http_api_targets,
                    })
                }
            })
            .buffer_unordered(settings.setup_concurrency.max(1))
            .try_collect::<Vec<_>>()
            .await
            .map_err(setup_error)?;

        Ok(IngressLoadIteration {
            owner,
            owner_env_id: env.id,
            domain_owners: domains
                .into_iter()
                .map(|(user, env_id, _)| (user, env_id))
                .collect(),
            accounts: Arc::new(accounts),
            rest_url: Self::rest_url(deps),
        })
    }

    /// Calls every agent once on each path, so the agents exist before the step starts.
    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        let clients_created = AtomicU64::new(0);
        let calls = context
            .accounts
            .iter()
            .enumerate()
            .flat_map(|(index, account)| {
                (0..account.rest_bodies.len()).flat_map(move |agent| {
                    [(index, Path::Rest, agent), (index, Path::HttpApi, agent)]
                })
            })
            .collect::<Vec<_>>();
        info!("Warming up {} agents", calls.len());
        let accounts = &context.accounts;
        let rest_url = &context.rest_url;
        let clients_created = &clients_created;
        let failures = stream::iter(calls)
            .map(|(index, path, agent)| async move {
                let account = &accounts[index];
                let mut last = Ok(());
                for _ in 0..WARMUP_ATTEMPTS {
                    last = send(account, rest_url, path, agent, clients_created).await;
                    if last.is_ok() {
                        break;
                    }
                }
                last.map_err(|message| format!("{}: {message}", path.prefix()))
            })
            .buffer_unordered(self.settings.setup_concurrency.max(1) * 4)
            .filter_map(|result| async move { result.err() })
            .collect::<Vec<_>>()
            .await;
        match failures.first() {
            None => Ok(()),
            Some(first) => Err(BenchmarkError::new(
                "warmup",
                format!(
                    "{} agents failed to warm up, first: {first}",
                    failures.len()
                ),
            )),
        }
    }

    async fn run(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let step = Duration::from_secs(self.settings.step_secs);
        let rate = self.config.length as f64;
        info!(
            "Sending {rate} req/s from {} accounts for {}s",
            context.accounts.len(),
            step.as_secs()
        );
        let stats = Arc::new(StepStats::default());
        drive(
            context.accounts.clone(),
            Arc::new(context.rest_url.clone()),
            rate,
            step,
            &self.settings,
            stats.clone(),
            recorder.clone(),
        )
        .await;

        let mut succeeded = 0;
        for path in [Path::Rest, Path::HttpApi] {
            let path_stats = stats.path(path);
            let ok = path_stats.succeeded.load(Ordering::Relaxed);
            let failed = path_stats.failed.load(Ordering::Relaxed);
            succeeded += ok;
            recorder.count(
                &ResultKey::primary(format!("{}-succeeded", path.prefix())),
                ok,
            );
            recorder.count(
                &ResultKey::primary(format!("{}-failed", path.prefix())),
                failed,
            );
            for message in path_stats.messages.lock().unwrap().drain(..) {
                recorder.failure(
                    &ResultKey::primary(format!("{}-invocation", path.prefix())),
                    message,
                );
            }
        }
        let dropped = stats.dropped.load(Ordering::Relaxed);
        recorder.count(
            &ResultKey::primary("target-req-per-sec"),
            self.config.length as u64,
        );
        recorder.count(
            &ResultKey::primary("achieved-req-per-sec"),
            (succeeded as f64 / step.as_secs_f64()).round() as u64,
        );
        recorder.count(&ResultKey::primary("dropped"), dropped);
        recorder.count(
            &ResultKey::secondary("clients-created"),
            context.accounts.len() as u64 + stats.clients_created.load(Ordering::Relaxed),
        );
        info!(
            "Step done: {succeeded} succeeded, {dropped} dropped, {} REST and {} HTTP API failures",
            stats.rest.failed.load(Ordering::Relaxed),
            stats.http_api.failed.load(Ordering::Relaxed)
        );
        Ok(())
    }

    async fn cleanup_iteration(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let concurrency = self.settings.setup_concurrency.max(1);
        stream::iter(context.accounts.iter())
            .for_each_concurrent(concurrency, |account| {
                cleanup_account(&account.user, &recorder)
            })
            .await;
        stream::iter(context.domain_owners.iter())
            .for_each_concurrent(concurrency, |(user, env_id)| {
                cleanup_user_state(user, env_id, &recorder)
            })
            .await;
        cleanup_user_state(&context.owner, &context.owner_env_id, &recorder).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn path_of_spreads_the_http_api_share_evenly() {
        for percent in [0, 10, 30, 50, 100] {
            let http_api = (0..1000)
                .filter(|n| matches!(path_of(*n, percent), Path::HttpApi))
                .count();
            assert_eq!(http_api, percent * 10, "{percent}%");
        }
        let first_ten = (0..10)
            .map(|n| matches!(path_of(n, 50), Path::HttpApi))
            .collect::<Vec<_>>();
        assert_eq!(
            first_ten,
            [
                false, true, false, true, false, true, false, true, false, true
            ]
        );
    }
}
