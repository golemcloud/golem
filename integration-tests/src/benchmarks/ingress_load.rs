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
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, IngressMethod,
    IngressMixEntry, IngressSettings, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use reqwest::Url;
use serde_json::json;
use std::collections::BTreeMap;
use std::net::SocketAddr;
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
/// A request that starts this much after it was due shows that the runner, not the
/// cluster, could not keep the rate.
const LATE_START: Duration = Duration::from_millis(20);

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
    /// The entry of the mix for each slot of the request cycle. See `mix_schedule`.
    schedule: Arc<Vec<usize>>,
}

/// One client: an account with its own connection, token and agents.
struct Account {
    user: TestUserContext<BenchmarkTestDependencies>,
    bearer: String,
    client: AccountClient,
    /// The requests for each entry of the mix.
    calls: Vec<AccountCall>,
}

/// The requests of one entry of the mix, for each agent of an account.
struct AccountCall {
    /// The REST request for each agent.
    rest_targets: Vec<RestTarget>,
    /// The request for each HTTP API agent.
    http_api_targets: Vec<HttpApiTarget>,
}

struct RestTarget {
    /// The body for the invoke endpoint.
    body: bytes::Bytes,
    /// worker-service refuses the request before it calls an executor, and that answer
    /// counts as success. See `IngressMethod::NoInvocation`.
    refused: bool,
}

/// What worker-service answers to constructor parameters of the wrong type.
const REFUSED_STATUS: reqwest::StatusCode = reqwest::StatusCode::BAD_REQUEST;
const REFUSED_MESSAGE: &str = "constructor parameters type error";

struct HttpApiTarget {
    get: bool,
    url: Url,
    /// The Host header, outside the cloud.
    host: Option<String>,
    body: Option<bytes::Bytes>,
}

struct AccountClient {
    http1: bool,
    requests_per_connection: usize,
    /// The address each host name of the account connects to. See `pinned_addresses`.
    pins: Vec<(String, SocketAddr)>,
    state: Mutex<(reqwest::Client, usize)>,
}

impl AccountClient {
    fn new(http1: bool, requests_per_connection: usize, pins: Vec<(String, SocketAddr)>) -> Self {
        Self {
            http1,
            requests_per_connection,
            state: Mutex::new((build_client(http1, &pins), 0)),
            pins,
        }
    }

    /// The client for the next request. After `requests_per_connection` requests the
    /// client is replaced, and its connections close once their open requests finish.
    fn next(&self, clients_created: &AtomicU64) -> reqwest::Client {
        let mut state = self.state.lock().unwrap();
        if self.requests_per_connection > 0 && state.1 >= self.requests_per_connection {
            *state = (build_client(self.http1, &self.pins), 0);
            clients_created.fetch_add(1, Ordering::Relaxed);
        }
        state.1 += 1;
        state.0.clone()
    }
}

fn build_client(http1: bool, pins: &[(String, SocketAddr)]) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .tcp_nodelay(true)
        .timeout(REQUEST_TIMEOUT)
        .pool_idle_timeout(Duration::from_secs(90));
    for (host, address) in pins {
        // TLS and SNI still use the host name; only the address to connect to is fixed.
        builder = builder.resolve(host, *address);
    }
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
    /// Requests that the runner started more than LATE_START after they were due.
    late_starts: AtomicU64,
    max_start_lag_micros: AtomicU64,
}

impl StepStats {
    fn path(&self, path: Path) -> &PathStats {
        match path {
            Path::Rest => &self.rest,
            Path::HttpApi => &self.http_api,
        }
    }
}

/// The addresses of each host, resolved once at setup and sorted.
///
/// A golem-dev NLB has one address per zone, and each address sends only to the proxy
/// pods in its zone. Left to the resolver, the clients could all take the same first
/// address and load one zone. Account `index` connects to address `index` mod the number
/// of addresses instead, so the accounts spread evenly over the zones in every run.
struct HostAddresses(Vec<(String, Vec<SocketAddr>)>);

impl HostAddresses {
    async fn resolve(hosts: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let mut resolved = Vec::new();
        for host in hosts {
            let mut addresses = tokio::net::lookup_host((host.as_str(), 443))
                .await
                .map_err(|err| anyhow::anyhow!("Cannot resolve {host}: {err}"))?
                .collect::<Vec<_>>();
            // A dual-stack NLB also has IPv6 addresses; the runner may have no IPv6 route.
            if addresses.iter().any(SocketAddr::is_ipv4) {
                addresses.retain(SocketAddr::is_ipv4);
            }
            addresses.sort();
            addresses.dedup();
            if addresses.is_empty() {
                anyhow::bail!("{host} has no addresses");
            }
            info!("{host} resolves to {addresses:?}");
            resolved.push((host, addresses));
        }
        Ok(Self(resolved))
    }

    /// The addresses of `hosts` for account `index`.
    fn pinned_addresses(&self, hosts: &[&str], index: usize) -> Vec<(String, SocketAddr)> {
        self.0
            .iter()
            .filter(|(host, _)| hosts.contains(&host.as_str()))
            .map(|(host, addresses)| (host.clone(), addresses[index % addresses.len()]))
            .collect()
    }
}

fn setup_error(message: impl std::fmt::Display) -> BenchmarkError {
    BenchmarkError::new("setup", format!("{message:#}"))
}

fn agent(name: String) -> ParsedAgentId {
    agent_id!(AGENT_TYPE, name)
}

/// The name of the method in result keys and failure messages.
fn method_label(method: IngressMethod) -> &'static str {
    match method {
        IngressMethod::NoInvocation => "no-invocation",
        IngressMethod::Echo => "echo",
        IngressMethod::Sleep => "sleep",
        IngressMethod::OplogHeavy => "oplog-heavy",
        IngressMethod::CpuIntensive => "cpu-intensive",
    }
}

/// Checks that each entry of the mix has a weight and the values its method needs.
fn check_mix(mix: &[IngressMixEntry]) -> Result<(), String> {
    if mix.is_empty() {
        return Err("the mix has no entries".to_string());
    }
    for entry in mix {
        let label = method_label(entry.method);
        if entry.weight == 0 {
            return Err(format!("{label} has weight 0"));
        }
        let missing = match entry.method {
            IngressMethod::NoInvocation | IngressMethod::Echo => None,
            IngressMethod::Sleep => entry.millis.is_none().then_some("millis"),
            IngressMethod::OplogHeavy | IngressMethod::CpuIntensive => {
                entry.length.is_none().then_some("length")
            }
        };
        if let Some(field) = missing {
            return Err(format!("{label} needs {field}"));
        }
    }
    Ok(())
}

/// The mix entry for each slot of one request cycle. Each entry gets as many slots as its
/// weight (divided by the common divisor of the weights), spread over the cycle, so the
/// methods interleave instead of coming in runs.
fn mix_schedule(weights: &[usize]) -> Vec<usize> {
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let divisor = weights.iter().copied().fold(0, gcd).max(1);
    let weights = weights
        .iter()
        .map(|w| (w / divisor) as i64)
        .collect::<Vec<_>>();
    let total: i64 = weights.iter().sum();
    let mut current = vec![0i64; weights.len()];
    (0..total)
        .map(|_| {
            for (c, w) in current.iter_mut().zip(&weights) {
                *c += w;
            }
            // The first of the largest, so the cycle is the same in every run.
            let best = (0..weights.len())
                .max_by_key(|&i| (current[i], std::cmp::Reverse(i)))
                .expect("the mix has entries");
            current[best] -= total;
            best
        })
        .collect()
}

/// The duration key of a path and mix entry. With one entry, the key has no method name,
/// as in the echo-only runs.
fn duration_key(path: Path, mix: &[IngressMixEntry], call: usize) -> String {
    if mix.len() == 1 {
        format!("{}-invocation", path.prefix())
    } else {
        format!(
            "{}-{}-invocation",
            path.prefix(),
            method_label(mix[call].method)
        )
    }
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

    fn rest_target(
        owner_email: &str,
        app_name: &str,
        env_name: &str,
        agent_id: &ParsedAgentId,
        entry: &IngressMixEntry,
    ) -> anyhow::Result<RestTarget> {
        let refused = entry.method == IngressMethod::NoInvocation;
        let (method_name, parameters) = match entry.method {
            IngressMethod::NoInvocation => ("echo", data_value!("benchmark")),
            IngressMethod::Echo => ("echo", data_value!("benchmark")),
            IngressMethod::Sleep => ("sleep", data_value!(entry.millis.unwrap_or_default())),
            IngressMethod::OplogHeavy => (
                "oplog_heavy",
                data_value!(entry.length.unwrap_or_default(), true),
            ),
            IngressMethod::CpuIntensive => (
                "cpu_intensive",
                data_value!(entry.length.unwrap_or_default() as f64),
            ),
        };
        let request = AgentInvocationRequest {
            app_name: app_name.to_string(),
            env_name: env_name.to_string(),
            agent_type_name: agent_id.agent_type.0.clone(),
            // The agent takes a string; a number is refused before any executor sees it.
            parameters: if refused {
                ExternalSchemaValue::try_from(data_value!(0u32).into_parts().1)
            } else {
                ExternalSchemaValue::try_from(agent_id.parameters.value().clone())
            }
            .map_err(anyhow::Error::msg)?,
            phantom_id: agent_id.phantom_id,
            config: None,
            method_name: method_name.to_string(),
            method_parameters: ExternalSchemaValue::try_from(parameters.into_parts().1)
                .map_err(anyhow::Error::msg)?,
            mode: AgentInvocationMode::Await,
            schedule_at: None,
            idempotency_key: None,
            deployment_revision: None,
            owner_account_email: Some(owner_email.to_string()),
        };
        Ok(RestTarget {
            body: bytes::Bytes::from(serde_json::to_vec(&request)?),
            refused,
        })
    }

    /// The HTTP API request of a mix entry for the agent `name`, on the domain at `base`.
    fn http_api_target(
        base: &str,
        host: &Option<String>,
        name: &str,
        entry: &IngressMixEntry,
    ) -> anyhow::Result<HttpApiTarget> {
        if entry.method == IngressMethod::NoInvocation {
            return Ok(HttpApiTarget {
                get: true,
                url: Url::parse(&format!("{base}/openapi.json"))?,
                host: host.clone(),
                body: None,
            });
        }
        let (path, body) = match entry.method {
            IngressMethod::NoInvocation => unreachable!(),
            IngressMethod::Echo => ("echo/load", None),
            // The HTTP API takes a u64 only as a string.
            IngressMethod::Sleep => (
                "sleep",
                Some(json!({"millis": entry.millis.map(|millis| millis.to_string())})),
            ),
            IngressMethod::OplogHeavy => (
                "oplog-heavy",
                Some(json!({"length": entry.length, "commit": true})),
            ),
            IngressMethod::CpuIntensive => ("cpu-intensive", Some(json!({"length": entry.length}))),
        };
        Ok(HttpApiTarget {
            get: false,
            url: Url::parse(&format!("{base}/rust/{name}/{path}"))?,
            host: host.clone(),
            body: body.map(|body| bytes::Bytes::from(body.to_string())),
        })
    }
}

/// Sends one request and returns why it failed, if it did.
async fn send(
    account: &Account,
    rest_url: &Url,
    path: Path,
    call: usize,
    agent: usize,
    clients_created: &AtomicU64,
) -> Result<(), String> {
    let client = account.client.next(clients_created);
    let call = &account.calls[call];
    let request = match path {
        Path::Rest => client
            .post(rest_url.clone())
            .bearer_auth(&account.bearer)
            .header("Content-Type", "application/json; charset=utf-8")
            .body(call.rest_targets[agent].body.clone()),
        Path::HttpApi => {
            let target = &call.http_api_targets[agent];
            let mut request = if target.get {
                client.get(target.url.clone())
            } else {
                client.post(target.url.clone())
            };
            if let Some(host) = &target.host {
                request = request.header("Host", host);
            }
            if let Some(body) = &target.body {
                request = request
                    .header("Content-Type", "application/json")
                    .body(body.clone());
            }
            request
        }
    };
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            // Read the body, so the time includes the whole response.
            response
                .bytes()
                .await
                .map(|_| ())
                .map_err(|err| error_chain(&err))
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let refused = matches!(path, Path::Rest) && call.rest_targets[agent].refused;
            if refused && status == REFUSED_STATUS && body.contains(REFUSED_MESSAGE) {
                return Ok(());
            }
            Err(format!(
                "{status}: {}",
                body.chars().take(300).collect::<String>()
            ))
        }
        Err(err) => Err(error_chain(&err)),
    }
}

/// The error and all its sources, because reqwest shows only "error sending request" at the top.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
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
#[allow(clippy::too_many_arguments)]
async fn drive(
    accounts: Arc<Vec<Account>>,
    rest_url: Arc<Url>,
    schedule: Arc<Vec<usize>>,
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
    let keys = Arc::new(
        (0..settings.mix.len())
            .map(|call| {
                [Path::Rest, Path::HttpApi].map(|path| duration_key(path, &settings.mix, call))
            })
            .collect::<Vec<_>>(),
    );
    let labels = Arc::new(
        settings
            .mix
            .iter()
            .map(|entry| (settings.mix.len() > 1).then(|| method_label(entry.method)))
            .collect::<Vec<_>>(),
    );

    let clients = (0..count)
        .map(|index| {
            let accounts = accounts.clone();
            let rest_url = rest_url.clone();
            let schedule = schedule.clone();
            let (keys, labels) = (keys.clone(), labels.clone());
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
                    let lag = due.elapsed();
                    if lag > LATE_START {
                        stats.late_starts.fetch_add(1, Ordering::Relaxed);
                    }
                    stats
                        .max_start_lag_micros
                        .fetch_max(lag.as_micros() as u64, Ordering::Relaxed);
                    let path = path_of(n, http_api_percent);
                    let agent = (n as usize / 2) % agents;
                    // The two requests of a pair (one per path at 50%) call the same method,
                    // and each client starts at its own place in the cycle.
                    let call = schedule[(n as usize / 2 + index) % schedule.len()];
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
                    let (keys, labels) = (keys.clone(), labels.clone());
                    tokio::spawn(async move {
                        let account = &accounts[index];
                        let result = send(
                            account,
                            &rest_url,
                            path,
                            call,
                            agent,
                            &stats.clients_created,
                        )
                        .await;
                        let latency = due.elapsed();
                        match result {
                            Ok(()) => {
                                stats.path(path).succeeded.fetch_add(1, Ordering::Relaxed);
                                let key = &keys[call][matches!(path, Path::HttpApi) as usize];
                                recorder.duration(&ResultKey::primary(key.clone()), latency);
                            }
                            Err(message) => stats.path(path).fail(match labels[call] {
                                Some(label) => format!("{label}: {message}"),
                                None => message,
                            }),
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
            the suite. The accounts call agents in one shared environment, through the REST invoke
            endpoint and through the HTTP API, with the methods of the `mix` setting (echo only by
            default). The clients send at a fixed rate, whether or not their earlier requests have
            returned.
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
        check_mix(&settings.mix).map_err(|err| setup_error(format!("ingress mix: {err}")))?;

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

        // In the cloud the hosts are NLB names with one address per zone; see HostAddresses.
        let rest_url = Self::rest_url(deps);
        let rest_host = rest_url.host_str().unwrap_or_default().to_string();
        let addresses = if deps.apps_base_domain().is_some() {
            HostAddresses::resolve(
                std::iter::once(rest_host.clone())
                    .chain(domains.iter().map(|(_, _, domain)| domain.0.clone()))
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(setup_error)?
        } else {
            HostAddresses(Vec::new())
        };

        info!("Setting up {} accounts", self.config.size);
        let http1_accounts = self.config.size * settings.http1_percent.min(100) / 100;
        let owner_ref = &owner;
        let accounts = stream::iter(0..self.config.size)
            .map(|index| {
                let (app_name, env_name, component_name) = (&app_name, &env_name, &component_name);
                let domain_bases = &domain_bases;
                let (domains, addresses, rest_host) = (&domains, &addresses, &rest_host);
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
                    let (base, host) = &domain_bases[index % domain_bases.len()];
                    let mut calls = Vec::new();
                    for entry in &settings.mix {
                        let mut rest_targets = Vec::new();
                        let mut http_api_targets = Vec::new();
                        for n in 0..settings.agents_per_account.max(1) {
                            rest_targets.push(Self::rest_target(
                                owner_email,
                                app_name,
                                env_name,
                                &agent(format!("a{index}-{n}")),
                                entry,
                            )?);
                            http_api_targets.push(Self::http_api_target(
                                base,
                                host,
                                &format!("a{index}-{n}-http"),
                                entry,
                            )?);
                        }
                        calls.push(AccountCall {
                            rest_targets,
                            http_api_targets,
                        });
                    }
                    anyhow::Ok(Account {
                        bearer: user.token.secret().to_string(),
                        user,
                        client: AccountClient::new(
                            index < http1_accounts,
                            settings.requests_per_connection,
                            addresses.pinned_addresses(
                                &[
                                    rest_host.as_str(),
                                    domains[index % domains.len()].2.0.as_str(),
                                ],
                                index,
                            ),
                        ),
                        calls,
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
            rest_url,
            schedule: Arc::new(mix_schedule(
                &settings
                    .mix
                    .iter()
                    .map(|entry| entry.weight)
                    .collect::<Vec<_>>(),
            )),
        })
    }

    /// Calls every agent once on each path, so the agents exist before the step starts. It
    /// uses the first echo entry of the mix, or the first entry when the mix has no echo.
    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        let clients_created = AtomicU64::new(0);
        let call = self
            .settings
            .mix
            .iter()
            .position(|entry| entry.method == IngressMethod::Echo)
            .unwrap_or(0);
        let calls = context
            .accounts
            .iter()
            .enumerate()
            .flat_map(|(index, account)| {
                (0..account.calls[call].rest_targets.len()).flat_map(move |agent| {
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
                    last = send(account, rest_url, path, call, agent, clients_created).await;
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
            context.schedule.clone(),
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
            &ResultKey::primary("late-starts"),
            stats.late_starts.load(Ordering::Relaxed),
        );
        recorder.count(
            &ResultKey::secondary("max-start-lag-ms"),
            stats.max_start_lag_micros.load(Ordering::Relaxed) / 1000,
        );
        recorder.count(
            &ResultKey::secondary("clients-created"),
            context.accounts.len() as u64 + stats.clients_created.load(Ordering::Relaxed),
        );
        info!(
            "Step done: {succeeded} succeeded, {dropped} dropped, {} REST and {} HTTP API failures, {} late starts",
            stats.rest.failed.load(Ordering::Relaxed),
            stats.http_api.failed.load(Ordering::Relaxed),
            stats.late_starts.load(Ordering::Relaxed)
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
    fn pinned_addresses_spread_the_accounts_evenly() {
        let zones = ["10.0.1.5:443", "10.0.2.5:443", "10.0.3.5:443"]
            .map(|address| address.parse::<SocketAddr>().unwrap());
        let addresses = HostAddresses(vec![
            ("api.example".to_string(), zones.to_vec()),
            ("apps.example".to_string(), zones.to_vec()),
        ]);
        let mut per_zone = [0; 3];
        for index in 0..300 {
            let pins = addresses.pinned_addresses(&["api.example"], index);
            assert_eq!(pins.len(), 1, "only the hosts the account uses");
            per_zone[zones.iter().position(|zone| *zone == pins[0].1).unwrap()] += 1;
        }
        assert_eq!(per_zone, [100, 100, 100]);
    }

    #[test]
    async fn a_pinned_client_connects_to_the_pinned_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = build_client(
            true,
            &[("pinned.invalid".to_string(), listener.local_addr().unwrap())],
        );
        // The name does not resolve, so the request reaches the listener only through the pin.
        let request = tokio::spawn(async move {
            let _ = client
                .get(format!("http://pinned.invalid:{port}/"))
                .send()
                .await;
        });
        let accepted = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await;
        request.abort();
        assert!(
            accepted.is_ok(),
            "the client did not connect to the pinned address"
        );
    }

    #[test]
    fn mix_schedule_gives_each_entry_its_weight_and_interleaves() {
        let schedule = mix_schedule(&[50, 20, 20, 10]);
        assert_eq!(schedule.len(), 10);
        for (entry, weight) in [(0, 5), (1, 2), (2, 2), (3, 1)] {
            assert_eq!(schedule.iter().filter(|&&e| e == entry).count(), weight);
        }
        // The entries interleave: echo, with half the weight, never comes three times in a row.
        assert!(
            schedule.windows(3).all(|run| run != [0, 0, 0]),
            "{schedule:?}"
        );
        assert_eq!(mix_schedule(&[1]), vec![0]);
    }

    #[test]
    fn check_mix_wants_the_values_of_each_method() {
        let entry = |method, millis, length| IngressMixEntry {
            method,
            weight: 1,
            millis,
            length,
        };
        assert!(check_mix(&[entry(IngressMethod::Echo, None, None)]).is_ok());
        assert!(check_mix(&[entry(IngressMethod::NoInvocation, None, None)]).is_ok());
        assert!(check_mix(&[]).is_err());
        assert!(check_mix(&[entry(IngressMethod::Sleep, None, None)]).is_err());
        assert!(check_mix(&[entry(IngressMethod::OplogHeavy, None, Some(10))]).is_ok());
        assert!(check_mix(&[entry(IngressMethod::CpuIntensive, Some(5), None)]).is_err());
    }

    #[test]
    fn the_duration_key_names_the_method_only_in_a_mix() {
        let echo = IngressSettings::default().mix;
        assert_eq!(duration_key(Path::Rest, &echo, 0), "rest-invocation");
        let mut mix = echo.clone();
        mix.push(IngressMixEntry {
            method: IngressMethod::Sleep,
            weight: 1,
            millis: Some(10),
            length: None,
        });
        assert_eq!(
            duration_key(Path::HttpApi, &mix, 1),
            "http-api-sleep-invocation"
        );
    }

    #[test]
    async fn a_failed_request_keeps_the_cause() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let err = build_client(true, &[])
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap_err();
        let message = error_chain(&err);
        assert!(
            message.len() > err.to_string().len(),
            "no cause in {message:?}"
        );
    }

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
