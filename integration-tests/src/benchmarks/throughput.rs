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

use crate::agents::agent_type_id;
use crate::benchmarks::{
    cleanup_user_state, delete_workers, deployment_base_url, deployment_domain,
    invoke_and_await_agent, invoke_and_await_http, lookup_keyed_agents,
};
use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue};
use futures_concurrency::future::Join;
use golem_client::api::RegistryServiceClient;
use golem_common::base_model::type_id::TypeId;
use golem_common::model::agent::{AgentKey, AgentTypeName, ParsedAgentKey};
use golem_common::model::component::ComponentDto;
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::{AgentId, Pod, RoutingTable};
use golem_common::schema::TypedSchemaValue;
use golem_common::{agent_key, data_value};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use reqwest::{Body, Method, Request, Url};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tracing::{Instrument, Level, info};

pub struct ThroughputEcho<const AGGREGATE: bool> {
    config: RunConfig,
    iteration: AtomicUsize,
}
pub struct ThroughputLargeInput<const AGGREGATE: bool> {
    config: RunConfig,
    iteration: AtomicUsize,
}
pub struct ThroughputCpuIntensive<const AGGREGATE: bool> {
    config: RunConfig,
    iteration: AtomicUsize,
}

#[async_trait]
impl<const AGGREGATE: bool> Benchmark for ThroughputEcho<AGGREGATE> {
    type BenchmarkContext = ThroughputBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        if AGGREGATE {
            "throughput-echo-aggregate"
        } else {
            "throughput-echo"
        }
    }

    fn description() -> &'static str {
        indoc! {
            "Spawns `size` number of workers of various implementations, and calls the `echo` endpoint
            a fixed number of times. The `length` parameter is not used.
            The `size` should be chosen in a way that all workers fit in the available executor's memory,
            and `size=1` can be used to test isolated throughput of a single worker.
            The benchmarks measures multiple implementations separately after each other:
            rust agent, TS agent, rust agent through HTTP mapping, TS agent through HTTP mapping, TS agent RPC and rust agent RPC
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(ThroughputBenchmark::new(
            "echo",
            "echo",
            Box::new(|_| data_value!("benchmark")),
            Box::new(|base_url, idx, _length| {
                let url =
                    Url::parse(&format!("{base_url}/test-{idx}-http/echo/test-message")).unwrap();
                Request::new(Method::POST, url)
            }),
            Box::new(|base_url, idx, _length| {
                let url = Url::parse(&format!(
                    "{base_url}/rust/test-{idx}-http/echo/test-message"
                ))
                .unwrap();
                Request::new(Method::POST, url)
            }),
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
            250,
            otlp,
        )
        .await)
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) -> BenchmarkResultValue {
        benchmark_context.cleanup().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self {
            config,
            iteration: AtomicUsize::new(0),
        })
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        benchmark_context
            .setup_iteration(
                &self.config,
                self.iteration.fetch_add(1, Ordering::Relaxed),
                recorder,
            )
            .await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        benchmark_context.warmup(context).await
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        benchmark_context.run(context, recorder, AGGREGATE).await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        benchmark_context.cleanup_iteration(context, recorder).await;
        Ok(())
    }
}

#[async_trait]
impl<const AGGREGATE: bool> Benchmark for ThroughputLargeInput<AGGREGATE> {
    type BenchmarkContext = ThroughputBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        if AGGREGATE {
            "throughput-large-input-aggregate"
        } else {
            "throughput-large-input"
        }
    }

    fn description() -> &'static str {
        indoc! {
            "Spawns `size` number of workers of various implementations, and calls the `large-input` endpoint
            with an input based on `length` in a fixed number of times.
            `size` should be chosen in a way that all workers fit in the available executor's memory,
            and `size=1` can be used to test isolated throughput of a single worker.
            The benchmarks measures multiple implementations separately after each other:
            rust agent, TS agent, rust agent through HTTP mapping, TS agent through HTTP mapping, TS agent RPC and rust agent RPC
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(ThroughputBenchmark::new(
            "large_input",
            "largeInput",
            Box::new(|length| {
                let bytes = vec![0u8; length];
                data_value!(bytes)
            }),
            Box::new(|base_url, idx, length| {
                let url = Url::parse(&format!("{base_url}/test-{idx}-http/large-input")).unwrap();
                let json_body = json!({"input": vec![0u8; length]}).to_string();
                let mut request = Request::new(Method::POST, url);
                *request.body_mut() = Some(Body::wrap(json_body));
                request
            }),
            Box::new(|base_url, idx, length| {
                let url =
                    Url::parse(&format!("{base_url}/rust/test-{idx}-http/large-input")).unwrap();
                let json_body = json!({"input": vec![0u8; length]}).to_string();
                let mut request = Request::new(Method::POST, url);
                *request.body_mut() = Some(Body::wrap(json_body));
                request
            }),
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
            100,
            otlp,
        )
        .await)
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) -> BenchmarkResultValue {
        benchmark_context.cleanup().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self {
            config,
            iteration: AtomicUsize::new(0),
        })
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        benchmark_context
            .setup_iteration(
                &self.config,
                self.iteration.fetch_add(1, Ordering::Relaxed),
                recorder,
            )
            .await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        benchmark_context.warmup(context).await
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        benchmark_context.run(context, recorder, AGGREGATE).await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        benchmark_context.cleanup_iteration(context, recorder).await;
        Ok(())
    }
}

#[async_trait]
impl<const AGGREGATE: bool> Benchmark for ThroughputCpuIntensive<AGGREGATE> {
    type BenchmarkContext = ThroughputBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        if AGGREGATE {
            "throughput-cpu-intensive-aggregate"
        } else {
            "throughput-cpu-intensive"
        }
    }

    fn description() -> &'static str {
        indoc! {
            "Spawns `size` number of workers of various implementations, and calls the `cpu-intensive` endpoint
            with the CPU intensive task's length based on `length` in a fixed number of times.
            `size` should be chosen in a way that all workers fit in the available executor's memory,
            and `size=1` can be used to test isolated throughput of a single worker.
            The benchmarks measures multiple implementations separately after each other:
            rust agent, TS agent, rust agent through HTTP mapping, TS agent through HTTP mapping, TS agent RPC and rust agent RPC
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(ThroughputBenchmark::new(
            "cpu_intensive",
            "cpuIntensive",
            Box::new(|length| data_value!(length as f64)),
            Box::new(|base_url, idx, length| {
                let url = Url::parse(&format!("{base_url}/test-{idx}-http/cpu-intensive")).unwrap();
                let json_body = json!({"length": length}).to_string();
                let mut request = Request::new(Method::POST, url);
                *request.body_mut() = Some(Body::wrap(json_body));
                request
            }),
            Box::new(|base_url, idx, length| {
                let url =
                    Url::parse(&format!("{base_url}/rust/test-{idx}-http/cpu-intensive")).unwrap();
                let json_body = json!({"length": length}).to_string();
                let mut request = Request::new(Method::POST, url);
                *request.body_mut() = Some(Body::wrap(json_body));
                request
            }),
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
            10,
            otlp,
        )
        .await)
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) -> BenchmarkResultValue {
        benchmark_context.cleanup().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self {
            config,
            iteration: AtomicUsize::new(0),
        })
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        benchmark_context
            .setup_iteration(
                &self.config,
                self.iteration.fetch_add(1, Ordering::Relaxed),
                recorder,
            )
            .await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        benchmark_context.warmup(context).await
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        benchmark_context.run(context, recorder, AGGREGATE).await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        benchmark_context.cleanup_iteration(context, recorder).await;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AgentIdPair {
    pub parent: ParsedAgentKey,
    pub child: ParsedAgentKey,
    parent_agent_id: AgentId,
    child_agent_id: AgentId,
}

impl AgentIdPair {
    fn assigned_pods(&self, routing_table: &RoutingTable) -> BenchmarkResultValue<(Pod, Pod)> {
        if routing_table.number_of_shards.value == 0 {
            return Err(BenchmarkError::new(
                "rpc-routing",
                "Routing table has no shards",
            ));
        }
        let parent_pod = routing_table.lookup(&self.parent_agent_id);
        let child_pod = routing_table.lookup(&self.child_agent_id);

        match (parent_pod, child_pod) {
            (Some(parent_pod), Some(child_pod)) => Ok((*parent_pod, *child_pod)),
            _ => Err(BenchmarkError::new(
                "rpc-routing",
                format!(
                    "Missing assignment for RPC pair {} / {}",
                    self.parent, self.child
                ),
            )),
        }
    }

    fn at_same_worker_executor(&self, routing_table: &RoutingTable) -> bool {
        let (parent, child) = self
            .assigned_pods(routing_table)
            .expect("Invalid RPC routing");
        parent == child
    }

    fn ensure_unchanged(
        &self,
        expected: &RoutingTable,
        current: &RoutingTable,
    ) -> BenchmarkResultValue {
        let assigned = self.assigned_pods(expected)?;
        if self.assigned_pods(current)? != assigned {
            return Err(BenchmarkError::new(
                "rpc-routing",
                format!(
                    "RPC assignment changed for {} / {}",
                    self.parent, self.child
                ),
            ));
        }
        Ok(())
    }
}

fn select_rpc_agent_pairs(
    environment_id: EnvironmentId,
    (parent_type, parent_type_id): (&str, TypeId),
    (child_type, child_type_id): (&str, TypeId),
    size: usize,
    iteration: usize,
    routing_table: Option<&RoutingTable>,
) -> BenchmarkResultValue<Vec<AgentIdPair>> {
    (0..size).map(|target| {
        let pair = |name: String| -> BenchmarkResultValue<AgentIdPair> {
            let parent = agent_key!(parent_type, name.clone());
            let child = agent_key!(child_type, name);
            let parent_key = AgentKey::new(parent.agent_type.clone(), parent.parameters.clone())
                .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            let child_key = AgentKey::new(child.agent_type.clone(), child.parameters.clone())
                .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            Ok(AgentIdPair {
                parent_agent_id: AgentId::keyed(environment_id, parent_type_id, &parent_key),
                child_agent_id: AgentId::keyed(environment_id, child_type_id, &child_key),
                parent,
                child,
            })
        };
        let Some(table) = routing_table else {
            return pair(format!("rpc-test-{target}"));
        };
        // Alternate both across targets and across iterations. A single executor
        // has no remote route; an odd target count gives local one extra on even iterations.
        let local = table.all().len() == 1 || iteration % 2 == target % 2;
        const MAX_CANDIDATES: usize = 10_000;
        for candidate in 0..MAX_CANDIDATES {
            let selected = pair(format!("rpc-test-{target}-{candidate}"))?;
            let (parent_pod, child_pod) = selected.assigned_pods(table)?;
            if (parent_pod == child_pod) == local {
                info!(rpc_selection = %serde_json::json!({
                    "environment_id": environment_id.to_string(), "parent": selected.parent.to_string(),
                    "child": selected.child.to_string(), "parent_pod": parent_pod.to_string(),
                    "child_pod": child_pod.to_string(), "local": local, "iteration": iteration,
                    "target": target, "candidate": candidate
                }),
                      "Selected RPC benchmark pair");
                return Ok(selected);
            }
        }
        Err(BenchmarkError::new("rpc-routing", format!("No {parent_type}/{child_type} pair for local={local}, target={target} after {MAX_CANDIDATES} candidates")))
    }).collect()
}

#[allow(clippy::large_enum_variant)]
enum AgentInvocationTarget {
    Single {
        component: ComponentDto,
        agent_id: ParsedAgentKey,
    },
    Pair {
        component: ComponentDto,
        pair: AgentIdPair,
    },
}

impl AgentInvocationTarget {
    pub fn component(&self) -> &ComponentDto {
        match self {
            AgentInvocationTarget::Single { component, .. } => component,
            AgentInvocationTarget::Pair { component, .. } => component,
        }
    }

    pub fn agent_id(&self) -> &ParsedAgentKey {
        match self {
            AgentInvocationTarget::Single { agent_id, .. } => agent_id,
            AgentInvocationTarget::Pair { pair, .. } => &pair.parent,
        }
    }

    pub fn prefix(&self, prefix: &str, routing_table: &Option<RoutingTable>) -> String {
        match self {
            AgentInvocationTarget::Single { .. } => prefix.to_string(),
            AgentInvocationTarget::Pair { pair, .. } => {
                if let Some(rt) = routing_table {
                    if pair.at_same_worker_executor(rt) {
                        format!("{prefix}local-")
                    } else {
                        format!("{prefix}remote-")
                    }
                } else {
                    // Routing table not available (no shard-manager port-forward
                    // configured); all RPC pairs go into a single unlabeled bucket.
                    prefix.to_string()
                }
            }
        }
    }
}

pub struct IterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    domain: Domain,
    rust_agent_component: ComponentDto,
    ts_agent_component: ComponentDto,
    rust_agent_ids: Vec<ParsedAgentKey>,
    ts_agent_ids: Vec<ParsedAgentKey>,
    rust_agent_ids_for_http: Vec<ParsedAgentKey>,
    ts_agent_ids_for_http: Vec<ParsedAgentKey>,
    length: usize,
    /// `None` when shard-manager host/port are not configured (cloud mode
    /// without port-forward). When `None`, RPC pairs go into a single unlabeled
    /// bucket instead of being split into local/remote.
    routing_table: Option<RoutingTable>,
    ts_rpc_agent_id_pairs: Vec<AgentIdPair>,
    rust_rpc_agent_id_pairs: Vec<AgentIdPair>,
    env_id: EnvironmentId,
}

/// Type for HTTP request builder closures used by the throughput benchmark.
/// Receives `(base_url, agent_index, length)` where `base_url` is the full
/// scheme+host+port prefix (e.g. `http://localhost:8084` in local mode or
/// `https://myenv.apps.golem.dev` in cloud mode).
type HttpRequestFn = Box<dyn for<'a> Fn(&'a str, usize, usize) -> Request + Send + Sync + 'static>;

pub struct ThroughputBenchmark {
    rust_method_name: String,
    ts_method_name: String,
    agent_params: Box<dyn Fn(usize) -> TypedSchemaValue + Send + Sync + 'static>,
    http_request: HttpRequestFn,
    rust_http_request: HttpRequestFn,
    deps: BenchmarkTestDependencies,
    call_count: usize,
    routing_required: bool,
    spawned_cluster_size: Option<usize>,
    /// Pre-built HTTP client for cloud-mode apps-domain calls
    /// (`https://{env_id}.{apps_base_domain}`).  Cached here so the
    /// connection pool is warm across benchmark iterations.
    /// `None` in local/provided mode (client is built per-iteration from the
    /// custom-request port with a Host header override).
    cloud_http_client: Option<reqwest::Client>,
}

/// Records aggregate throughput (invocations per second) for a measurement
/// block as a `count` result under the key `{prefix}throughput-ops-per-sec`.
///
/// `total_calls` is the total number of invocations issued across all targets
/// in the block; `elapsed` is the wall-clock duration of the concurrently
/// executed block. Throughput is therefore the realised aggregate rate the
/// cluster sustained for this implementation, not a per-call latency.
fn record_throughput(
    recorder: &BenchmarkRecorder,
    prefix: &str,
    total_calls: usize,
    elapsed: std::time::Duration,
) {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || total_calls == 0 {
        return;
    }
    let ops_per_sec = (total_calls as f64 / secs).round() as u64;
    info!("{prefix}throughput: {total_calls} calls in {secs:.3}s = {ops_per_sec} ops/sec");
    recorder.count(
        &ResultKey::primary(format!("{prefix}throughput-ops-per-sec")),
        ops_per_sec,
    );
}

impl ThroughputBenchmark {
    pub async fn new(
        rust_method_name: &str,
        ts_method_name: &str,
        agent_params: Box<dyn Fn(usize) -> TypedSchemaValue + Send + Sync + 'static>,
        http_request: HttpRequestFn,
        rust_http_request: HttpRequestFn,
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        call_count: usize,
        otlp: bool,
    ) -> Self {
        let deps = BenchmarkTestDependencies::new(
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
            otlp,
        )
        .await;

        // Build the cloud HTTP client once so the connection pool stays alive
        // across all benchmark iterations.  In cloud mode requests go to
        // https://{env_id}.{apps_base_domain}, so we use standard TLS with
        // ALPN negotiation — NOT http2_prior_knowledge() which is for h2c
        // (cleartext HTTP/2) and would bypass the ALPN step that the NLB
        // terminating TLS expects.
        let cloud_http_client = deps.apps_base_domain().map(|_| {
            reqwest::ClientBuilder::new()
                .pool_max_idle_per_host(1024)
                .pool_idle_timeout(std::time::Duration::from_secs(90))
                .tcp_nodelay(true)
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("Failed to build cloud HTTP client for throughput benchmark")
        });

        Self {
            rust_method_name: rust_method_name.to_string(),
            ts_method_name: ts_method_name.to_string(),
            agent_params,
            http_request,
            rust_http_request,
            deps,
            call_count,
            routing_required: !matches!(
                mode,
                TestMode::Cloud {
                    shard_manager_grpc_host: None,
                    ..
                } | TestMode::Cloud {
                    shard_manager_grpc_port: None,
                    ..
                }
            ),
            spawned_cluster_size: matches!(mode, TestMode::Spawned { .. }).then_some(cluster_size),
            cloud_http_client,
        }
    }

    pub async fn cleanup(&self) {
        self.deps.kill_all().await
    }

    pub async fn setup_iteration(
        &self,
        config: &RunConfig,
        iteration: usize,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<IterationContext> {
        let mut rust_agent_ids = vec![];
        let mut ts_agent_ids = vec![];
        let mut rust_agent_ids_for_http = vec![];
        let mut ts_agent_ids_for_http = vec![];

        // Fetch routing table when shard-manager is configured; fall back to
        // None (unlabeled single-bucket RPC) when not configured (e.g. cloud
        // mode without a port-forward to the shard-manager).
        let routing_table: Option<RoutingTable> =
            match self.deps.shard_manager().get_routing_table().await {
                Ok(rt) => {
                    info!(routing_table = %rt, "Fetched routing table");
                    Some(rt)
                }
                Err(err) if self.routing_required => {
                    return Err(BenchmarkError::new("rpc-routing", err));
                }
                Err(err) => {
                    info!(
                        error = %err, "Shard-manager not configured; RPC pairs will be unlabeled"
                    );
                    None
                }
            };

        if let (Some(expected), Some(table)) = (self.spawned_cluster_size, &routing_table)
            && table.all().len() != expected
        {
            return Err(BenchmarkError::new(
                "rpc-routing",
                format!(
                    "Expected {expected} assigned executors, found {}",
                    table.all().len()
                ),
            ));
        }

        let user = self.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();

        let (rust_agent_component, ts_agent_component) = async {
            let rust_agent_component = user
                .component(&env.id, "benchmark_agent_rust_release")
                .name("benchmark:agent-rust")
                .store()
                .await
                .unwrap();

            let ts_agent_component = user
                .component(&env.id, "benchmark_agent_ts")
                .name("benchmark:agent-ts")
                .store()
                .await
                .unwrap();

            (rust_agent_component, ts_agent_component)
        }
        .instrument(tracing::info_span!("register_components"))
        .await;

        for n in 0..config.size {
            rust_agent_ids.push(agent_key!("RustBenchmarkAgent", format!("test-{n}")));
            ts_agent_ids.push(agent_key!("BenchmarkAgent", format!("test-{n}")));
            rust_agent_ids_for_http
                .push(agent_key!("RustBenchmarkAgent", format!("test-{n}-http")));
            ts_agent_ids_for_http.push(agent_key!("BenchmarkAgent", format!("test-{n}-http")));
        }

        let client = user.registry_service_client().await;

        let domain = deployment_domain(&self.deps, &env.id);

        async {
            client
                .create_domain_registration(
                    &env.id.0,
                    &DomainRegistrationCreation {
                        domain: domain.clone(),
                    },
                )
                .await
                .expect("Failed to register to register domain");
        }
        .instrument(tracing::info_span!("register_domain"))
        .await;

        async {
            let http_api_deployment_creation = HttpApiDeploymentCreation {
                scheme: Default::default(),
                domain: domain.clone(),
                webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
                openapi_endpoint_prefix: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(
                ),
                agents: BTreeMap::from_iter([
                    (
                        AgentTypeName("BenchmarkAgent".to_string()),
                        HttpApiDeploymentAgentOptions::default(),
                    ),
                    (
                        AgentTypeName("RustBenchmarkAgent".to_string()),
                        HttpApiDeploymentAgentOptions::default(),
                    ),
                ]),
            };

            client
                .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
                .await
                .expect("Failed to create http api deployment");
        }
        .instrument(tracing::info_span!("create_http_deployment"))
        .await;

        async {
            user.deploy_environment(env.id)
                .await
                .expect("Failed to deploy environment");
        }
        .instrument(tracing::info_span!("deploy_environment"))
        .await;

        let pairs = async {
            let ts_parent_type = AgentTypeName("RpcBenchmarkAgent".to_string());
            let ts_child_type = AgentTypeName("BenchmarkAgent".to_string());
            let rust_parent_type = AgentTypeName("RustRpcBenchmarkAgent".to_string());
            let rust_child_type = AgentTypeName("RustBenchmarkAgent".to_string());
            let ts_parent_type_id = agent_type_id(&user, &ts_agent_component, &ts_parent_type)
                .await
                .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            let ts_child_type_id = agent_type_id(&user, &ts_agent_component, &ts_child_type)
                .await
                .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            let rust_parent_type_id =
                agent_type_id(&user, &rust_agent_component, &rust_parent_type)
                    .await
                    .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            let rust_child_type_id = agent_type_id(&user, &rust_agent_component, &rust_child_type)
                .await
                .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            let ts = select_rpc_agent_pairs(
                env.id,
                (&ts_parent_type.0, ts_parent_type_id),
                (&ts_child_type.0, ts_child_type_id),
                config.size,
                iteration,
                routing_table.as_ref(),
            )?;
            let rust = select_rpc_agent_pairs(
                env.id,
                (&rust_parent_type.0, rust_parent_type_id),
                (&rust_child_type.0, rust_child_type_id),
                config.size,
                iteration,
                routing_table.as_ref(),
            )?;
            Ok((ts, rust))
        }
        .await;
        let (ts_rpc_agent_id_pairs, rust_rpc_agent_id_pairs) = match pairs {
            Ok(pairs) => pairs,
            Err(error) => {
                cleanup_user_state(&user, &env.id, &recorder).await;
                return Err(error);
            }
        };

        Ok(IterationContext {
            user,
            domain,
            rust_agent_component,
            ts_agent_component,
            rust_agent_ids,
            ts_agent_ids,
            rust_agent_ids_for_http,
            ts_agent_ids_for_http,
            length: config.length,
            routing_table,
            ts_rpc_agent_id_pairs,
            rust_rpc_agent_id_pairs,
            env_id: env.id,
        })
    }

    async fn ensure_rpc_routing(&self, iteration: &IterationContext) -> BenchmarkResultValue {
        if let Some(expected) = &iteration.routing_table {
            let current = self
                .deps
                .shard_manager()
                .get_routing_table()
                .await
                .map_err(|error| BenchmarkError::new("rpc-routing", error))?;
            for pair in iteration
                .ts_rpc_agent_id_pairs
                .iter()
                .chain(&iteration.rust_rpc_agent_id_pairs)
            {
                pair.ensure_unchanged(expected, &current)?;
            }
        }
        Ok(())
    }

    pub async fn warmup(&self, iteration: &IterationContext) -> BenchmarkResultValue {
        self.ensure_rpc_routing(iteration).await?;
        async fn warmup_agents(
            user: &TestUserContext<BenchmarkTestDependencies>,
            component: &ComponentDto,
            ids: &[ParsedAgentKey],
            method_name: &str,
            params: &(dyn Fn(usize) -> TypedSchemaValue + Send + Sync + 'static),
            length: usize,
        ) {
            let result_futures = ids
                .iter()
                .map(move |agent_id| async move {
                    let user_clone = user.clone();
                    invoke_and_await_agent(
                        &user_clone,
                        component,
                        agent_id,
                        method_name,
                        (params)(length),
                    )
                    .await
                })
                .collect::<Vec<_>>();

            let _ = result_futures.join().await;
        }

        warmup_agents(
            &iteration.user,
            &iteration.rust_agent_component,
            &iteration.rust_agent_ids,
            &self.rust_method_name,
            &self.agent_params,
            iteration.length,
        )
        .instrument(tracing::info_span!("warmup_rust_agents"))
        .await;

        warmup_agents(
            &iteration.user,
            &iteration.ts_agent_component,
            &iteration.ts_agent_ids,
            &self.ts_method_name,
            &self.agent_params,
            iteration.length,
        )
        .instrument(tracing::info_span!("warmup_ts_agents"))
        .await;

        warmup_agents(
            &iteration.user,
            &iteration.rust_agent_component,
            &iteration.rust_agent_ids_for_http,
            &self.rust_method_name,
            &self.agent_params,
            iteration.length,
        )
        .instrument(tracing::info_span!("warmup_rust_agents_http"))
        .await;

        warmup_agents(
            &iteration.user,
            &iteration.ts_agent_component,
            &iteration.ts_agent_ids_for_http,
            &self.ts_method_name,
            &self.agent_params,
            iteration.length,
        )
        .instrument(tracing::info_span!("warmup_ts_agents_http"))
        .await;

        warmup_agents(
            &iteration.user,
            &iteration.ts_agent_component,
            &iteration
                .ts_rpc_agent_id_pairs
                .iter()
                .map(|pair| pair.parent.clone())
                .collect::<Vec<_>>(),
            &self.ts_method_name,
            &self.agent_params,
            iteration.length,
        )
        .instrument(tracing::info_span!("warmup_ts_rpc_agents"))
        .await;

        warmup_agents(
            &iteration.user,
            &iteration.rust_agent_component,
            &iteration
                .rust_rpc_agent_id_pairs
                .iter()
                .map(|pair| pair.parent.clone())
                .collect::<Vec<_>>(),
            &self.rust_method_name,
            &self.agent_params,
            iteration.length,
        )
        .instrument(tracing::info_span!("warmup_rust_rpc_agents"))
        .await;
        self.ensure_rpc_routing(iteration).await
    }

    pub async fn run(
        &self,
        iteration: &IterationContext,
        recorder: BenchmarkRecorder,
        aggregate: bool,
    ) -> BenchmarkResultValue {
        self.ensure_rpc_routing(iteration).await?;
        async fn measure_agents(
            user: &TestUserContext<BenchmarkTestDependencies>,
            routing_table: &Option<RoutingTable>,
            recorder: &BenchmarkRecorder,
            length: usize,
            call_count: usize,
            targets: &[AgentInvocationTarget],
            method_name: &str,
            params: &(dyn Fn(usize) -> TypedSchemaValue + Send + Sync + 'static),
            prefix: &str,
            aggregate: bool,
        ) {
            let result_futures = targets
                .iter()
                .map(move |target| {
                    let user_clone = user.clone();

                    async move {
                        let mut results = vec![];
                        for _ in 0..call_count {
                            results.push(
                                invoke_and_await_agent(
                                    &user_clone,
                                    target.component(),
                                    target.agent_id(),
                                    method_name,
                                    (params)(length),
                                )
                                .await,
                            )
                        }
                        results
                    }
                })
                .collect::<Vec<_>>();

            let start = Instant::now();
            let results = result_futures.join().await;
            let elapsed = start.elapsed();
            record_throughput(recorder, prefix, targets.len() * call_count, elapsed);
            if aggregate {
                recorder.duration(&format!("{prefix}batch-duration").into(), elapsed);
                recorder.count(
                    &format!("{prefix}batch-completions").into(),
                    results.iter().map(Vec::len).sum::<usize>() as u64,
                );
            }
            for (idx, (results, target)) in results.iter().zip(targets).enumerate() {
                let prefix = target.prefix(prefix, routing_table);
                for result in results {
                    if aggregate {
                        for failure in &result.failures {
                            recorder
                                .failure(&format!("{prefix}invocation").into(), failure.clone());
                        }
                    } else {
                        result.record(recorder, &prefix, idx.to_string().as_str());
                    }
                }
            }
        }

        measure_agents(
            &iteration.user,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .rust_agent_ids
                .iter()
                .cloned()
                .map(|id| AgentInvocationTarget::Single {
                    component: iteration.rust_agent_component.clone(),
                    agent_id: id,
                })
                .collect::<Vec<_>>(),
            &self.rust_method_name,
            &self.agent_params,
            "rust-agent-",
            aggregate,
        )
        .instrument(tracing::info_span!("measure_rust_agents"))
        .await;

        measure_agents(
            &iteration.user,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .ts_agent_ids
                .iter()
                .cloned()
                .map(|id| AgentInvocationTarget::Single {
                    component: iteration.ts_agent_component.clone(),
                    agent_id: id,
                })
                .collect::<Vec<_>>(),
            &self.ts_method_name,
            &self.agent_params,
            "ts-agent-",
            aggregate,
        )
        .instrument(tracing::info_span!("measure_ts_agents"))
        .await;

        // Resolve the base URL prefix and HTTP client for the code-first HTTP
        // API benchmark paths. The request-builder closures append the route
        // path (e.g. "/test-0-http/echo/...") to this prefix.
        //
        //   cloud mode:  base = "https://{env_id}.apps.dev.golem.cloud"
        //                → reqwest connects directly to that host (TLS/SNI +
        //                  Host set from the URL); the apps gateway routes it
        //                  to worker-service. Uses the cached, pool-warm client.
        //
        //   local mode:  base = "http://localhost:{custom_request_port}"
        //                → reqwest connects to localhost; an explicit Host
        //                  header ("{env_id}.golem.cloud") tells the local
        //                  worker-service which deployment to route to.
        let http_base_url = deployment_base_url(&self.deps, &iteration.domain);
        let client: reqwest::Client = if let Some(ref cached) = self.cloud_http_client {
            cached.clone()
        } else {
            let mut headers = HeaderMap::new();
            headers.insert("Host", HeaderValue::from_str(&iteration.domain.0).unwrap());
            reqwest::Client::builder()
                .default_headers(headers)
                .build()
                .expect("Failed to create HTTP client")
        };

        async {
            let client = client.clone();
            let base = http_base_url.clone();
            let result_futures = iteration
                .rust_agent_ids_for_http
                .iter()
                .enumerate()
                .map(move |(idx, _agent_id)| {
                    let client = client.clone();
                    let base = base.clone();
                    async move {
                        let mut results = vec![];
                        for _ in 0..self.call_count {
                            results.push(
                                invoke_and_await_http(client.clone(), || {
                                    (self.rust_http_request)(&base, idx, iteration.length)
                                })
                                .await,
                            )
                        }
                        results
                    }
                })
                .collect::<Vec<_>>();

            let start = Instant::now();
            let results = result_futures.join().await;
            let elapsed = start.elapsed();
            record_throughput(
                &recorder,
                "rust-agent-http-",
                iteration.rust_agent_ids_for_http.len() * self.call_count,
                elapsed,
            );
            if aggregate {
                recorder.duration(&"rust-agent-http-batch-duration".into(), elapsed);
                recorder.count(
                    &"rust-agent-http-batch-completions".into(),
                    results.iter().map(Vec::len).sum::<usize>() as u64,
                );
            }
            for (idx, results) in results.iter().enumerate() {
                for result in results {
                    if aggregate {
                        for failure in &result.failures {
                            recorder.failure(&"rust-agent-http-invocation".into(), failure.clone());
                        }
                    } else {
                        result.record(&recorder, "rust-agent-http-", idx.to_string().as_str());
                    }
                }
            }
        }
        .instrument(tracing::info_span!("measure_rust_agents_http"))
        .await;

        async {
            let result_futures = iteration
                .ts_agent_ids_for_http
                .iter()
                .enumerate()
                .map(move |(idx, _agent_id)| {
                    let client = client.clone();
                    let base = http_base_url.clone();
                    async move {
                        let mut results = vec![];
                        for _ in 0..self.call_count {
                            results.push(
                                invoke_and_await_http(client.clone(), || {
                                    (self.http_request)(&base, idx, iteration.length)
                                })
                                .await,
                            )
                        }
                        results
                    }
                })
                .collect::<Vec<_>>();

            let start = Instant::now();
            let results = result_futures.join().await;
            let elapsed = start.elapsed();
            record_throughput(
                &recorder,
                "ts-agent-http-",
                iteration.ts_agent_ids_for_http.len() * self.call_count,
                elapsed,
            );
            if aggregate {
                recorder.duration(&"ts-agent-http-batch-duration".into(), elapsed);
                recorder.count(
                    &"ts-agent-http-batch-completions".into(),
                    results.iter().map(Vec::len).sum::<usize>() as u64,
                );
            }
            for (idx, results) in results.iter().enumerate() {
                for result in results {
                    if aggregate {
                        for failure in &result.failures {
                            recorder.failure(&"ts-agent-http-invocation".into(), failure.clone());
                        }
                    } else {
                        result.record(&recorder, "ts-agent-http-", idx.to_string().as_str());
                    }
                }
            }
        }
        .instrument(tracing::info_span!("measure_ts_agents_http"))
        .await;

        measure_agents(
            &iteration.user,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .ts_rpc_agent_id_pairs
                .iter()
                .cloned()
                .map(|pair| AgentInvocationTarget::Pair {
                    component: iteration.ts_agent_component.clone(),
                    pair,
                })
                .collect::<Vec<_>>(),
            &self.ts_method_name,
            &self.agent_params,
            "ts-agent-rpc-",
            aggregate,
        )
        .instrument(tracing::info_span!("measure_ts_rpc"))
        .await;

        measure_agents(
            &iteration.user,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .rust_rpc_agent_id_pairs
                .iter()
                .cloned()
                .map(|pair| AgentInvocationTarget::Pair {
                    component: iteration.rust_agent_component.clone(),
                    pair,
                })
                .collect::<Vec<_>>(),
            &self.rust_method_name,
            &self.agent_params,
            "rust-agent-rpc-",
            aggregate,
        )
        .instrument(tracing::info_span!("measure_rust_rpc"))
        .await;
        self.ensure_rpc_routing(iteration).await
    }

    pub async fn cleanup_iteration(
        &self,
        iteration: IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        delete_workers(
            &iteration.user,
            &lookup_keyed_agents(
                &iteration.user,
                &iteration.rust_agent_component,
                &iteration.rust_agent_ids,
            )
            .await,
            &recorder,
        )
        .await;
        delete_workers(
            &iteration.user,
            &lookup_keyed_agents(
                &iteration.user,
                &iteration.ts_agent_component,
                &iteration.ts_agent_ids,
            )
            .await,
            &recorder,
        )
        .await;
        delete_workers(
            &iteration.user,
            &lookup_keyed_agents(
                &iteration.user,
                &iteration.rust_agent_component,
                &iteration.rust_agent_ids_for_http,
            )
            .await,
            &recorder,
        )
        .await;
        delete_workers(
            &iteration.user,
            &lookup_keyed_agents(
                &iteration.user,
                &iteration.ts_agent_component,
                &iteration.ts_agent_ids_for_http,
            )
            .await,
            &recorder,
        )
        .await;

        let ts_rpc_workers = lookup_keyed_agents(
            &iteration.user,
            &iteration.ts_agent_component,
            &iteration
                .ts_rpc_agent_id_pairs
                .iter()
                .flat_map(|pair| [pair.parent.clone(), pair.child.clone()])
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(&iteration.user, &ts_rpc_workers, &recorder).await;

        let rust_rpc_workers = lookup_keyed_agents(
            &iteration.user,
            &iteration.rust_agent_component,
            &iteration
                .rust_rpc_agent_id_pairs
                .iter()
                .flat_map(|pair| [pair.parent.clone(), pair.child.clone()])
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(&iteration.user, &rust_rpc_workers, &recorder).await;
        cleanup_user_state(&iteration.user, &iteration.env_id, &recorder).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::shardmanager::{
        RoutingTable as GrpcRoutingTable, RoutingTableEntry,
    };
    use golem_common::model::ShardId;
    use std::collections::HashSet;
    use test_r::test;

    fn routing_table(pods: usize) -> RoutingTable {
        GrpcRoutingTable {
            number_of_shards: 64,
            shard_assignments: (0..64)
                .map(|shard| RoutingTableEntry {
                    shard_id: Some(ShardId::new(shard).into()),
                    pod: Some(
                        Pod {
                            ip: "127.0.0.1".parse().unwrap(),
                            port: 9100 + (shard as usize % pods) as u16,
                        }
                        .into(),
                    ),
                })
                .collect(),
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn rpc_selector_balances_routes_without_changing_target_count() {
        let table = routing_table(5);
        for environment in [1, 0x123456789abcdef0123456789abcdef0] {
            let environment_id = EnvironmentId(uuid::Uuid::from_u128(environment));
            for (parent, parent_type_id, child, child_type_id) in [
                ("RustRpcBenchmarkAgent", 1u32, "RustBenchmarkAgent", 2u32),
                ("RpcBenchmarkAgent", 37u32, "BenchmarkAgent", 91u32),
            ] {
                let parent_type_id = TypeId::try_from(parent_type_id).unwrap();
                let child_type_id = TypeId::try_from(child_type_id).unwrap();
                for size in [1, 3, 10] {
                    let mut local_counts = vec![];
                    for iteration in 0..5 {
                        let pairs = select_rpc_agent_pairs(
                            environment_id,
                            (parent, parent_type_id),
                            (child, child_type_id),
                            size,
                            iteration,
                            Some(&table),
                        )
                        .unwrap();
                        assert_eq!(pairs.len(), size);
                        let mut names = HashSet::new();
                        for pair in &pairs {
                            assert!(names.insert(pair.parent.to_string()));
                            assert_eq!(pair.parent.parameters, pair.child.parameters);
                            let caller = AgentId::keyed(
                                environment_id,
                                parent_type_id,
                                &AgentKey::new(
                                    pair.parent.agent_type.clone(),
                                    pair.parent.parameters.clone(),
                                )
                                .unwrap(),
                            );
                            let callee = AgentId::keyed(
                                environment_id,
                                child_type_id,
                                &AgentKey::new(
                                    pair.child.agent_type.clone(),
                                    pair.child.parameters.clone(),
                                )
                                .unwrap(),
                            );
                            assert_eq!(pair.parent_agent_id, caller);
                            assert_eq!(pair.child_agent_id, callee);
                            assert_eq!(
                                table.lookup(&caller) == table.lookup(&callee),
                                iteration % 2 == (names.len() - 1) % 2
                            );
                        }
                        local_counts.push(
                            pairs
                                .iter()
                                .filter(|pair| pair.at_same_worker_executor(&table))
                                .count(),
                        );
                    }
                    assert_eq!(
                        local_counts,
                        vec![
                            size.div_ceil(2),
                            size / 2,
                            size.div_ceil(2),
                            size / 2,
                            size.div_ceil(2)
                        ]
                    );
                }
            }
        }
    }

    #[test]
    fn rpc_selector_rejects_unreachable_or_incomplete_routing() {
        let environment = EnvironmentId(uuid::Uuid::from_u128(42));
        let parent_type_id = TypeId::try_from(1u32).unwrap();
        let child_type_id = TypeId::try_from(2u32).unwrap();
        let table = routing_table(5);
        // Identical agent types cannot form a remote pair for any shared name.
        assert!(
            select_rpc_agent_pairs(
                environment,
                ("BenchmarkAgent", child_type_id),
                ("BenchmarkAgent", child_type_id),
                1,
                1,
                Some(&table)
            )
            .is_err()
        );
        let mut incomplete: GrpcRoutingTable = table.into();
        incomplete.shard_assignments.clear();
        assert!(
            select_rpc_agent_pairs(
                environment,
                ("RpcBenchmarkAgent", parent_type_id),
                ("BenchmarkAgent", child_type_id),
                1,
                0,
                Some(&incomplete.clone().try_into().unwrap())
            )
            .is_err()
        );
        incomplete.number_of_shards = 0;
        assert!(
            select_rpc_agent_pairs(
                environment,
                ("RpcBenchmarkAgent", parent_type_id),
                ("BenchmarkAgent", child_type_id),
                1,
                0,
                Some(&incomplete.try_into().unwrap())
            )
            .is_err()
        );
    }

    #[test]
    fn rpc_selector_handles_single_executor_and_unavailable_cloud_routing() {
        let environment = EnvironmentId(uuid::Uuid::from_u128(42));
        let parent_type_id = TypeId::try_from(1u32).unwrap();
        let child_type_id = TypeId::try_from(2u32).unwrap();
        for iteration in 0..2 {
            let table = routing_table(1);
            let pairs = select_rpc_agent_pairs(
                environment,
                ("RpcBenchmarkAgent", parent_type_id),
                ("BenchmarkAgent", child_type_id),
                3,
                iteration,
                Some(&table),
            )
            .unwrap();
            assert!(
                pairs
                    .iter()
                    .all(|pair| pair.at_same_worker_executor(&table))
            );
        }
        let pairs = select_rpc_agent_pairs(
            environment,
            ("RpcBenchmarkAgent", parent_type_id),
            ("BenchmarkAgent", child_type_id),
            1,
            1,
            None,
        )
        .unwrap();
        assert_eq!(
            pairs[0].parent,
            agent_key!("RpcBenchmarkAgent", "rpc-test-0")
        );
        assert_eq!(pairs[0].child, agent_key!("BenchmarkAgent", "rpc-test-0"));
    }

    #[test]
    fn rpc_routing_rejects_movement_even_when_locality_is_unchanged() {
        let table = routing_table(5);
        let pairs = select_rpc_agent_pairs(
            EnvironmentId(uuid::Uuid::from_u128(42)),
            ("RpcBenchmarkAgent", TypeId::try_from(1u32).unwrap()),
            ("BenchmarkAgent", TypeId::try_from(2u32).unwrap()),
            2,
            0,
            Some(&table),
        )
        .unwrap();
        let mut moved: GrpcRoutingTable = table.clone().into();
        for assignment in &mut moved.shard_assignments {
            assignment.pod.as_mut().unwrap().port += 10;
        }
        let moved: RoutingTable = moved.try_into().unwrap();
        for pair in pairs {
            pair.ensure_unchanged(&table, &table).unwrap();
            assert_eq!(
                pair.at_same_worker_executor(&table),
                pair.at_same_worker_executor(&moved)
            );
            assert!(pair.ensure_unchanged(&table, &moved).is_err());
        }
    }

    #[test]
    fn aggregate_variants_have_distinct_benchmark_names() {
        assert_eq!(ThroughputEcho::<false>::name(), "throughput-echo");
        assert_eq!(ThroughputEcho::<true>::name(), "throughput-echo-aggregate");
        assert_eq!(
            ThroughputLargeInput::<false>::name(),
            "throughput-large-input"
        );
        assert_eq!(
            ThroughputLargeInput::<true>::name(),
            "throughput-large-input-aggregate"
        );
        assert_eq!(
            ThroughputCpuIntensive::<false>::name(),
            "throughput-cpu-intensive"
        );
        assert_eq!(
            ThroughputCpuIntensive::<true>::name(),
            "throughput-cpu-intensive-aggregate"
        );
    }
}
