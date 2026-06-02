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

use crate::benchmarks::{
    cleanup_user_state, delete_workers, invoke_and_await_agent, invoke_and_await_http,
};
use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue};
use futures_concurrency::future::Join;
use golem_client::api::RegistryServiceClient;
use golem_common::base_model::agent::{DataValue, ParsedAgentId};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentDto, ComponentId};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::{AgentId, RoutingTable};
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use reqwest::{Body, Method, Request, Url};
use serde_json::json;
use std::collections::BTreeMap;
use tracing::{Instrument, Level, info};

pub struct ThroughputEcho {
    config: RunConfig,
}
pub struct ThroughputLargeInput {
    config: RunConfig,
}
pub struct ThroughputCpuIntensive {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for ThroughputEcho {
    type BenchmarkContext = ThroughputBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "throughput-echo"
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
    ) -> Self::BenchmarkContext {
        ThroughputBenchmark::new(
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
        .await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.cleanup().await
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        benchmark_context.setup_iteration(&self.config).await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        benchmark_context.warmup(context).await
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        benchmark_context.run(context, recorder).await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        benchmark_context.cleanup_iteration(context).await
    }
}

#[async_trait]
impl Benchmark for ThroughputLargeInput {
    type BenchmarkContext = ThroughputBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "throughput-large-input"
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
    ) -> Self::BenchmarkContext {
        ThroughputBenchmark::new(
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
        .await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.cleanup().await
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        benchmark_context.setup_iteration(&self.config).await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        benchmark_context.warmup(context).await
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        benchmark_context.run(context, recorder).await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        benchmark_context.cleanup_iteration(context).await
    }
}

#[async_trait]
impl Benchmark for ThroughputCpuIntensive {
    type BenchmarkContext = ThroughputBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "throughput-cpu-intensive"
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
    ) -> Self::BenchmarkContext {
        ThroughputBenchmark::new(
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
        .await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.cleanup().await
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        benchmark_context.setup_iteration(&self.config).await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        benchmark_context.warmup(context).await
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        benchmark_context.run(context, recorder).await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        benchmark_context.cleanup_iteration(context).await
    }
}

#[derive(Debug, Clone)]
pub struct AgentIdPair {
    pub component_id: ComponentId,
    pub parent: ParsedAgentId,
    pub child: ParsedAgentId,
}

impl AgentIdPair {
    fn at_same_worker_executor(&self, routing_table: &RoutingTable) -> bool {
        let parent_agent_id = AgentId::from_agent_id(self.component_id, &self.parent)
            .expect("Failed to create worker id from parent agent id");
        let child_agent_id = AgentId::from_agent_id(self.component_id, &self.child)
            .expect("Failed to create worker id from child agent id");
        let parent_pod = routing_table.lookup(&parent_agent_id);
        let child_pod = routing_table.lookup(&child_agent_id);

        match (parent_pod, child_pod) {
            (Some(parent_pod), Some(child_pod)) => parent_pod == child_pod,
            _ => panic!("Failed to find the pod of parent and child agents in RPC benchmark"),
        }
    }
}

enum AgentInvocationTarget {
    Single {
        component: ComponentDto,
        agent_id: ParsedAgentId,
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

    pub fn agent_id(&self) -> &ParsedAgentId {
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
    rust_agent_ids: Vec<ParsedAgentId>,
    ts_agent_ids: Vec<ParsedAgentId>,
    rust_agent_ids_for_http: Vec<ParsedAgentId>,
    ts_agent_ids_for_http: Vec<ParsedAgentId>,
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
    agent_params: Box<dyn Fn(usize) -> DataValue + Send + Sync + 'static>,
    http_request: HttpRequestFn,
    rust_http_request: HttpRequestFn,
    deps: BenchmarkTestDependencies,
    call_count: usize,
    /// Pre-built HTTP client for cloud-mode apps-domain calls
    /// (`https://{env_id}.{apps_base_domain}`).  Cached here so the
    /// connection pool is warm across benchmark iterations.
    /// `None` in local/provided mode (client is built per-iteration from the
    /// custom-request port with a Host header override).
    cloud_http_client: Option<reqwest::Client>,
}

fn agent_ids_to_agent_ids(component_id: ComponentId, ids: &[ParsedAgentId]) -> Vec<AgentId> {
    ids.iter()
        .filter_map(|id| AgentId::from_agent_id(component_id, id).ok())
        .collect()
}

impl ThroughputBenchmark {
    pub async fn new(
        rust_method_name: &str,
        ts_method_name: &str,
        agent_params: Box<dyn Fn(usize) -> DataValue + Send + Sync + 'static>,
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
            cloud_http_client,
        }
    }

    pub async fn cleanup(&self) {
        self.deps.kill_all().await
    }

    pub async fn setup_iteration(&self, config: &RunConfig) -> IterationContext {
        let mut rust_agent_ids = vec![];
        let mut ts_agent_ids = vec![];
        let mut rust_agent_ids_for_http = vec![];
        let mut ts_agent_ids_for_http = vec![];
        let mut ts_rpc_agent_id_pairs = vec![];
        let mut rust_rpc_agent_id_pairs = vec![];

        // Fetch routing table when shard-manager is configured; fall back to
        // None (unlabeled single-bucket RPC) when not configured (e.g. cloud
        // mode without a port-forward to the shard-manager).
        let routing_table: Option<RoutingTable> =
            match self.deps.shard_manager().get_routing_table().await {
                Ok(rt) => {
                    info!("Fetched routing table: {rt}");
                    Some(rt)
                }
                Err(err) => {
                    info!(
                        "Shard-manager not available, skipping routing table (RPC pairs \
                         will be unlabeled): {err:#}"
                    );
                    None
                }
            };

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
            rust_agent_ids.push(agent_id!("RustBenchmarkAgent", format!("test-{n}")));
            ts_agent_ids.push(agent_id!("BenchmarkAgent", format!("test-{n}")));
            rust_agent_ids_for_http.push(agent_id!("RustBenchmarkAgent", format!("test-{n}-http")));
            ts_agent_ids_for_http.push(agent_id!("BenchmarkAgent", format!("test-{n}-http")));

            ts_rpc_agent_id_pairs.push(AgentIdPair {
                component_id: ts_agent_component.id,
                parent: agent_id!("RpcBenchmarkAgent", format!("rpc-test-{n}")),
                child: agent_id!("BenchmarkAgent", format!("rpc-test-{n}")),
            });
            rust_rpc_agent_id_pairs.push(AgentIdPair {
                component_id: rust_agent_component.id,
                parent: agent_id!("RustRpcBenchmarkAgent", format!("rpc-test-{n}")),
                child: agent_id!("RustBenchmarkAgent", format!("rpc-test-{n}")),
            });
        }

        let client = user.registry_service_client().await;

        // In cloud mode, use the configured apps_base_domain. Fall back to
        // "golem.cloud" for local/provided modes.
        let apps_base_domain = self
            .deps
            .apps_base_domain()
            .unwrap_or("golem.cloud")
            .to_string();
        let domain = Domain(format!("{}.{}", env.id, apps_base_domain));

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

        IterationContext {
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
        }
    }

    pub async fn warmup(&self, iteration: &IterationContext) {
        async fn warmup_agents(
            user: &TestUserContext<BenchmarkTestDependencies>,
            component: &ComponentDto,
            ids: &[ParsedAgentId],
            method_name: &str,
            params: &(dyn Fn(usize) -> DataValue + Send + Sync + 'static),
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
    }

    pub async fn run(&self, iteration: &IterationContext, recorder: BenchmarkRecorder) {
        async fn measure_agents(
            user: &TestUserContext<BenchmarkTestDependencies>,
            routing_table: &Option<RoutingTable>,
            recorder: &BenchmarkRecorder,
            length: usize,
            call_count: usize,
            targets: &[AgentInvocationTarget],
            method_name: &str,
            params: &(dyn Fn(usize) -> DataValue + Send + Sync + 'static),
            prefix: &str,
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

            let results = result_futures.join().await;
            for (idx, (results, target)) in results.iter().zip(targets).enumerate() {
                let prefix = target.prefix(prefix, routing_table);
                for result in results {
                    result.record(recorder, &prefix, idx.to_string().as_str());
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
        )
        .instrument(tracing::info_span!("measure_ts_agents"))
        .await;

        // In cloud mode use the pre-built, cached client (warm connection pool).
        // In local/provided mode build a fresh client with the explicit Host
        // header pointing at the custom-request port on localhost.
        let (http_base_url, client): (String, reqwest::Client) =
            if let Some(ref cached) = self.cloud_http_client {
                let base = format!("https://{}", iteration.domain.0);
                (base, cached.clone())
            } else {
                // Local/provided mode: connect to localhost with Host header.
                let port = self.deps.worker_service().custom_request_port();
                let base = format!("http://localhost:{port}");
                let mut headers = HeaderMap::new();
                headers.insert("Host", HeaderValue::from_str(&iteration.domain.0).unwrap());
                let c = reqwest::Client::builder()
                    .default_headers(headers)
                    .build()
                    .expect("Failed to create HTTP client");
                (base, c)
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

            let results = result_futures.join().await;
            for (idx, results) in results.iter().enumerate() {
                for result in results {
                    result.record(&recorder, "rust-agent-http-", idx.to_string().as_str());
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

            let results = result_futures.join().await;
            for (idx, results) in results.iter().enumerate() {
                for result in results {
                    result.record(&recorder, "ts-agent-http-", idx.to_string().as_str());
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
        )
        .instrument(tracing::info_span!("measure_rust_rpc"))
        .await;
    }

    pub async fn cleanup_iteration(&self, iteration: IterationContext) {
        delete_workers(
            &iteration.user,
            &agent_ids_to_agent_ids(iteration.rust_agent_component.id, &iteration.rust_agent_ids),
        )
        .await;
        delete_workers(
            &iteration.user,
            &agent_ids_to_agent_ids(iteration.ts_agent_component.id, &iteration.ts_agent_ids),
        )
        .await;
        delete_workers(
            &iteration.user,
            &agent_ids_to_agent_ids(
                iteration.rust_agent_component.id,
                &iteration.rust_agent_ids_for_http,
            ),
        )
        .await;
        delete_workers(
            &iteration.user,
            &agent_ids_to_agent_ids(
                iteration.ts_agent_component.id,
                &iteration.ts_agent_ids_for_http,
            ),
        )
        .await;

        let mut ts_rpc_workers: Vec<AgentId> = Vec::new();
        for pair in &iteration.ts_rpc_agent_id_pairs {
            if let Ok(id) = AgentId::from_agent_id(pair.component_id, &pair.parent) {
                ts_rpc_workers.push(id);
            }
            if let Ok(id) = AgentId::from_agent_id(pair.component_id, &pair.child) {
                ts_rpc_workers.push(id);
            }
        }
        delete_workers(&iteration.user, &ts_rpc_workers).await;

        let mut rust_rpc_workers: Vec<AgentId> = Vec::new();
        for pair in &iteration.rust_rpc_agent_id_pairs {
            if let Ok(id) = AgentId::from_agent_id(pair.component_id, &pair.parent) {
                rust_rpc_workers.push(id);
            }
            if let Ok(id) = AgentId::from_agent_id(pair.component_id, &pair.child) {
                rust_rpc_workers.push(id);
            }
        }
        delete_workers(&iteration.user, &rust_rpc_workers).await;
        cleanup_user_state(&iteration.user, &iteration.env_id).await;
    }
}
