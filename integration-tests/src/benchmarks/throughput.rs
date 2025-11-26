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

use crate::benchmarks::{delete_workers, invoke_and_await, invoke_and_await_http};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_client::model::{
    ApiDefinitionInfo, ApiDeploymentRequest, ApiSite, GatewayBindingComponent, GatewayBindingData,
    GatewayBindingType, HttpApiDefinitionRequest, MethodPattern, RouteRequestData,
};
use golem_common::model::component_metadata::{
    DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::{RoutingTable, WorkerId};
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValueAndType, ValueAndType};
use indoc::indoc;
use reqwest::{Body, Method, Request, Url};
use serde_json::json;
use std::collections::HashMap;
use tracing::{info, Level};
use uuid::Uuid;

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
            direct rust, native rust, TS agent, TS agent through rib mapping, direct Rust RPC and TS agent RPC
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
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{echo}",
            "benchmark:direct-rust-rpc-parent-exports/benchmark-direct-rust-rpc-parent-api.{echo}",
            "benchmark:agent-rust/rust-benchmark-agent.{echo}",
            "benchmark:agent-rust/rust-rpc-benchmark-agent.{echo}",
            "benchmark:agent-ts/benchmark-agent.{echo}",
            "benchmark:agent-ts/rpc-benchmark-agent.{echo}",
            Box::new(|_| vec!["benchmark".into_value_and_type()]),
            Box::new(|port, idx, api_definition_id, _length| {
                let url = Url::parse(&format!(
                    "http://localhost:{port}/{}/test-{idx}-rib/echo/test-message",
                    api_definition_id
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
            direct rust, native rust, TS agent, TS agent through rib mapping, direct Rust RPC and TS agent RPC
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
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{large-input}",
            "benchmark:direct-rust-rpc-parent-exports/benchmark-direct-rust-rpc-parent-api.{large-input}",
            "benchmark:agent-rust/rust-benchmark-agent.{large-input}",
            "benchmark:agent-rust/rust-rpc-benchmark-agent.{large-input}",
            "benchmark:agent-ts/benchmark-agent.{large-input}",
            "benchmark:agent-ts/rpc-benchmark-agent.{large-input}",
            Box::new(|length| {
                let bytes = vec![0u8; length];
                vec![bytes.into_value_and_type()]
            }),
            Box::new(|port, idx, api_definition_id, length| {
                let url = Url::parse(&format!(
                    "http://localhost:{port}/{}/test-{idx}-rib/large-input",
                    api_definition_id
                ))
                .unwrap();
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
            direct rust, native rust, TS agent, TS agent through rib mapping, direct Rust RPC and TS agent RPC
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
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{cpu-intensive}",
            "benchmark:direct-rust-rpc-parent-exports/benchmark-direct-rust-rpc-parent-api.{cpu-intensive}",
            "benchmark:agent-rust/rust-benchmark-agent.{cpu-intensive}",
            "benchmark:agent-rust/rust-rpc-benchmark-agent.{cpu-intensive}",
            "benchmark:agent-ts/benchmark-agent.{cpu-intensive}",
            "benchmark:agent-ts/rpc-benchmark-agent.{cpu-intensive}",
            Box::new(|length| vec![(length as f64).into_value_and_type()]),
            Box::new(|port, idx, api_definition_id, length| {
                let url = Url::parse(&format!(
                    "http://localhost:{port}/{}/test-{idx}-rib/cpu-intensive",
                    api_definition_id
                ))
                .unwrap();
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
pub struct WorkerIdPair {
    pub parent: WorkerId,
    pub child: WorkerId,
}

impl WorkerIdPair {
    fn at_same_worker_executor(&self, routing_table: &RoutingTable) -> bool {
        let parent_pod = routing_table.lookup(&self.parent);
        let child_pod = routing_table.lookup(&self.child);

        match (parent_pod, child_pod) {
            (Some(parent_pod), Some(child_pod)) => parent_pod == child_pod,
            _ => panic!("Failed to find the pod of parent and child workers in RPC benchmark"),
        }
    }
}

enum WorkerIdOrPair {
    Id(WorkerId),
    Pair(WorkerIdPair),
}

impl WorkerIdOrPair {
    pub fn worker_id(&self) -> &WorkerId {
        match self {
            WorkerIdOrPair::Id(id) => id,
            WorkerIdOrPair::Pair(pair) => &pair.parent,
        }
    }

    pub fn prefix(&self, prefix: &str, routing_table: &RoutingTable) -> String {
        match self {
            WorkerIdOrPair::Id(_) => prefix.to_string(),
            WorkerIdOrPair::Pair(pair) => {
                if pair.at_same_worker_executor(routing_table) {
                    format!("{prefix}local-")
                } else {
                    format!("{prefix}remote-")
                }
            }
        }
    }
}

impl From<WorkerId> for WorkerIdOrPair {
    fn from(id: WorkerId) -> Self {
        WorkerIdOrPair::Id(id)
    }
}

impl From<WorkerIdPair> for WorkerIdOrPair {
    fn from(pair: WorkerIdPair) -> Self {
        WorkerIdOrPair::Pair(pair)
    }
}

pub struct IterationContext {
    direct_rust_worker_ids: Vec<WorkerId>,
    rust_agent_worker_ids: Vec<WorkerId>,
    ts_agent_worker_ids: Vec<WorkerId>,
    ts_agent_worker_ids_for_rib: Vec<WorkerId>,
    length: usize,
    api_definition_id: String,
    direct_rust_rpc_worker_id_pairs: Vec<WorkerIdPair>,
    routing_table: RoutingTable,
    ts_rpc_agent_worker_id_pairs: Vec<WorkerIdPair>,
    rust_rpc_agent_worker_id_pairs: Vec<WorkerIdPair>,
}

pub struct ThroughputBenchmark {
    rust_function_name: String,
    rust_rpc_function_name: String,
    rust_agent_function_name: String,
    rust_agent_rpc_function_name: String,
    ts_function_name: String,
    ts_rpc_function_name: String,
    function_params: Box<dyn Fn(usize) -> Vec<ValueAndType> + Send + Sync + 'static>,
    http_request: Box<dyn Fn(u16, usize, String, usize) -> Request + Send + Sync + 'static>,
    deps: BenchmarkTestDependencies,
    call_count: usize,
}

impl ThroughputBenchmark {
    pub async fn new(
        rust_function_name: &str,
        rust_rpc_function_name: &str,
        rust_agent_function_name: &str,
        rust_agent_rpc_function_name: &str,
        ts_function_name: &str,
        ts_rpc_function_name: &str,
        function_params: Box<dyn Fn(usize) -> Vec<ValueAndType> + Send + Sync + 'static>,
        http_request: Box<dyn Fn(u16, usize, String, usize) -> Request + Send + Sync + 'static>,
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        call_count: usize,
        otlp: bool,
    ) -> Self {
        Self {
            rust_function_name: rust_function_name.to_string(),
            rust_rpc_function_name: rust_rpc_function_name.to_string(),
            rust_agent_function_name: rust_agent_function_name.to_string(),
            rust_agent_rpc_function_name: rust_agent_rpc_function_name.to_string(),
            ts_function_name: ts_function_name.to_string(),
            ts_rpc_function_name: ts_rpc_function_name.to_string(),
            function_params,
            http_request,
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
            call_count,
        }
    }

    pub async fn cleanup(&self) {
        self.deps.kill_all().await
    }

    pub async fn setup_iteration(&self, config: &RunConfig) -> IterationContext {
        let mut direct_rust_worker_ids = vec![];
        let mut rust_agent_worker_ids = vec![];
        let mut ts_agent_worker_ids = vec![];
        let mut ts_agent_worker_ids_for_rib = vec![];
        let mut direct_rust_rpc_worker_id_pairs = vec![];
        let mut ts_rpc_agent_worker_id_pairs = vec![];
        let mut rust_rpc_agent_worker_id_pairs = vec![];

        let routing_table = self
            .deps
            .shard_manager()
            .get_routing_table()
            .await
            .expect("Failed to get routing table");

        info!("Fetched routing table: {routing_table}");

        info!("Registering components");

        let rust_direct_component_id = self
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust")
            .name("benchmark:direct-rust")
            .store()
            .await;

        let rust_agent_component_id = self
            .deps
            .admin()
            .await
            .component("benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await;

        let ts_agent_component_id = self
            .deps
            .admin()
            .await
            .component("benchmark_agent_ts")
            .name("benchmark:agent-ts")
            .store()
            .await;

        let rust_rpc_parent_component_id = self
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust_rpc_parent")
            .name("benchmark:direct-rust-rpc-parent")
            .with_dynamic_linking(&[(
                "benchmark:direct-rust-rpc-child-client/benchmark-direct-rust-rpc-child-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "benchmark-direct-rust-rpc-child-api".to_string(),
                        WasmRpcTarget {
                            interface_name: "benchmark:direct-rust-rpc-child-exports/benchmark-direct-rust-rpc-child-api"
                                .to_string(),
                            component_name: "benchmark:direct-rust-rpc-child".to_string(),
                        },
                    )]),
                }),
            )])
            .store()
            .await;

        let rust_rpc_child_component_id = self
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust_rpc_child")
            .name("benchmark:direct-rust-rpc-child")
            .store()
            .await;

        for n in 0..config.size {
            direct_rust_worker_ids.push(WorkerId {
                component_id: rust_direct_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}\")"),
            });
            rust_agent_worker_ids.push(WorkerId {
                component_id: rust_agent_component_id.clone(),
                worker_name: format!("rust-benchmark-agent(\"test-{n}\")"),
            });
            ts_agent_worker_ids.push(WorkerId {
                component_id: ts_agent_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}\")"),
            });
            ts_agent_worker_ids_for_rib.push(WorkerId {
                component_id: ts_agent_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}-rib\")"),
            });
            let direct_rust_rpc_parent = WorkerId {
                component_id: rust_rpc_parent_component_id.clone(),
                worker_name: format!("rpc-benchmark-agent(\"test-{n}\")"),
            };
            let direct_rust_rpc_child = WorkerId {
                component_id: rust_rpc_child_component_id.clone(),
                worker_name: format!("rpc-benchmark-agent(\"test-{n}\")"),
            };
            direct_rust_rpc_worker_id_pairs.push(WorkerIdPair {
                parent: direct_rust_rpc_parent,
                child: direct_rust_rpc_child,
            });
            let ts_agent_rpc_parent = WorkerId {
                component_id: ts_agent_component_id.clone(),
                worker_name: format!("rpc-benchmark-agent(\"rpc-test-{n}\")"),
            };
            let ts_agent_rpc_child = WorkerId {
                component_id: ts_agent_component_id.clone(),
                worker_name: format!("benchmark-agent(\"rpc-test-{n}\")"),
            };
            ts_rpc_agent_worker_id_pairs.push(WorkerIdPair {
                parent: ts_agent_rpc_parent,
                child: ts_agent_rpc_child,
            });
            let rust_agent_rpc_parent = WorkerId {
                component_id: rust_agent_component_id.clone(),
                worker_name: format!("rust-rpc-benchmark-agent(\"rpc-test-{n}\")"),
            };
            let rust_agent_rpc_child = WorkerId {
                component_id: rust_agent_component_id.clone(),
                worker_name: format!("rust-benchmark-agent(\"rpc-test-{n}\")"),
            };
            rust_rpc_agent_worker_id_pairs.push(WorkerIdPair {
                parent: rust_agent_rpc_parent,
                child: rust_agent_rpc_child,
            });
        }

        info!("Registering API");

        let api_definition_id = Uuid::new_v4().to_string();
        let request = HttpApiDefinitionRequest {
            id: api_definition_id.clone(),
            version: "1".to_string(),
            draft: true,
            security: None,
            routes: vec![
                RouteRequestData {
                    method: MethodPattern::Post,
                    path: format!("/{api_definition_id}/{{name}}/echo/{{param}}"),
                    binding: GatewayBindingData {
                        component: Some(GatewayBindingComponent {
                            name: "benchmark:agent-ts".to_string(),
                            version: Some(0),
                        }),
                        worker_name: None,
                        response: Some(
                            r#"
                        let agent = benchmark-agent(request.path.name);
                        let result = agent.echo(request.path.param);
                        { status: 200, body: { result: "${result}" } }
                        "#
                            .to_string(),
                        ),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default),
                        invocation_context: None,
                    },
                    security: None,
                },
                RouteRequestData {
                    method: MethodPattern::Post,
                    path: format!("/{api_definition_id}/{{name}}/large-input"),
                    binding: GatewayBindingData {
                        component: Some(GatewayBindingComponent {
                            name: "benchmark:agent-ts".to_string(),
                            version: Some(0),
                        }),
                        worker_name: None,
                        response: Some(
                            r#"
                                    let agent = benchmark-agent(request.path.name);
                                    let result: f64 = agent.large-input(request.body.input);
                                    { status: 200, body: { result: "${result}" } }
                                    "#
                            .to_string(),
                        ),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default),
                        invocation_context: None,
                    },
                    security: None,
                },
                RouteRequestData {
                    method: MethodPattern::Post,
                    path: format!("/{api_definition_id}/{{name}}/cpu-intensive"),
                    binding: GatewayBindingData {
                        component: Some(GatewayBindingComponent {
                            name: "benchmark:agent-ts".to_string(),
                            version: Some(0),
                        }),
                        worker_name: None,
                        response: Some(
                            r#"
                                    let agent = benchmark-agent(request.path.name);
                                    let length: f64 = request.body.length;
                                    let result: f64 = agent.cpu-intensive(length);
                                    { status: 200, body: { result: "${result}" } }
                                    "#
                            .to_string(),
                        ),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default),
                        invocation_context: None,
                    },
                    security: None,
                },
            ],
        };

        let admin = self.deps.admin().await;
        let _ = self
            .deps
            .worker_service()
            .create_api_definition(&admin.token, &admin.default_project_id, &request)
            .await
            .expect("Failed to register API definition");

        let request = ApiDeploymentRequest {
            project_id: admin.default_project_id.0,
            api_definitions: vec![ApiDefinitionInfo {
                id: api_definition_id.clone(),
                version: "1".to_string(),
            }],
            site: ApiSite {
                host: format!(
                    "localhost:{}",
                    self.deps.worker_service().public_custom_request_port()
                ),
                subdomain: None,
            },
        };

        self.deps
            .worker_service()
            .create_or_update_api_deployment(&admin.token, request)
            .await
            .expect("Failed to create API deployment");

        IterationContext {
            direct_rust_worker_ids,
            rust_agent_worker_ids,
            ts_agent_worker_ids,
            ts_agent_worker_ids_for_rib,
            length: config.length,
            api_definition_id,
            direct_rust_rpc_worker_id_pairs,
            routing_table,
            ts_rpc_agent_worker_id_pairs,
            rust_rpc_agent_worker_id_pairs,
        }
    }

    pub async fn warmup(&self, iteration: &IterationContext) {
        async fn warmup_workers(
            deps: &BenchmarkTestDependencies,
            length: usize,
            ids: &[WorkerId],
            function_name: &str,
            params: &(dyn Fn(usize) -> Vec<ValueAndType> + Send + Sync + 'static),
        ) {
            let result_futures = ids
                .iter()
                .map(move |worker_id| async move {
                    let deps_clone = deps.clone();

                    invoke_and_await(&deps_clone, worker_id, function_name, (params)(length)).await
                })
                .collect::<Vec<_>>();

            let _ = result_futures.join().await;
        }

        info!("Warming up direct rust workers...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration.direct_rust_worker_ids,
            &self.rust_function_name,
            &self.function_params,
        )
        .await;

        info!("Warming up rust agents...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration.rust_agent_worker_ids,
            &self.rust_agent_function_name,
            &self.function_params,
        )
        .await;

        info!("Warming up TS agents...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration.ts_agent_worker_ids,
            &self.ts_function_name,
            &self.function_params,
        )
        .await;

        info!("Warming up TS agents for Rib mapping...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration.ts_agent_worker_ids_for_rib,
            &self.ts_function_name,
            &self.function_params,
        )
        .await;

        info!("Warming up direct rust RPC parent workers...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration
                .direct_rust_rpc_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.parent)
                .collect::<Vec<_>>(),
            &self.rust_rpc_function_name,
            &self.function_params,
        )
        .await;

        info!("Warming up TS RPC agents...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration
                .ts_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.parent)
                .collect::<Vec<_>>(),
            &self.ts_rpc_function_name,
            &self.function_params,
        )
        .await;

        info!("Warming up Rust RPC agents...");
        warmup_workers(
            &self.deps,
            iteration.length,
            &iteration
                .rust_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.parent)
                .collect::<Vec<_>>(),
            &self.rust_agent_rpc_function_name,
            &self.function_params,
        )
        .await;

        info!("Warmup completed");
    }

    pub async fn run(&self, iteration: &IterationContext, recorder: BenchmarkRecorder) {
        async fn measure_workers(
            deps: &BenchmarkTestDependencies,
            routing_table: &RoutingTable,
            recorder: &BenchmarkRecorder,
            length: usize,
            call_count: usize,
            ids: &[WorkerIdOrPair],
            function_name: &str,
            params: &(dyn Fn(usize) -> Vec<ValueAndType> + Send + Sync + 'static),
            prefix: &str,
        ) {
            let result_futures = ids
                .iter()
                .map(move |worker_id| async move {
                    let worker_id = worker_id.worker_id();
                    let deps_clone = deps.clone();

                    let mut results = vec![];
                    for _ in 0..call_count {
                        results.push(
                            invoke_and_await(
                                &deps_clone,
                                worker_id,
                                function_name,
                                (params)(length),
                            )
                            .await,
                        )
                    }
                    results
                })
                .collect::<Vec<_>>();

            let results = result_futures.join().await;
            for (idx, (results, id)) in results.iter().zip(ids).enumerate() {
                let prefix = id.prefix(prefix, routing_table);
                for result in results {
                    result.record(recorder, &prefix, idx.to_string().as_str());
                }
            }
        }

        info!("Measuring direct rust throughput");
        measure_workers(
            &self.deps,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .direct_rust_worker_ids
                .iter()
                .cloned()
                .map(|id| id.into())
                .collect::<Vec<_>>(),
            &self.rust_function_name,
            &self.function_params,
            "direct-rust-",
        )
        .await;

        info!("Measuring rust agent throughput");
        measure_workers(
            &self.deps,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .rust_agent_worker_ids
                .iter()
                .cloned()
                .map(|id| id.into())
                .collect::<Vec<_>>(),
            &self.rust_agent_function_name,
            &self.function_params,
            "rust-agent-",
        )
        .await;

        info!("Measuring TS agent throughput...");
        measure_workers(
            &self.deps,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .ts_agent_worker_ids
                .iter()
                .cloned()
                .map(|id| id.into())
                .collect::<Vec<_>>(),
            &self.ts_function_name,
            &self.function_params,
            "ts-agent-",
        )
        .await;

        info!("Measuring TS agent throughput through HTTP mapping...");
        let port = self.deps.worker_service().public_custom_request_port();

        let client = reqwest::Client::builder()
            .build()
            .expect("Failed to create HTTP client");
        let result_futures = iteration
            .ts_agent_worker_ids_for_rib
            .iter()
            .enumerate()
            .map(move |(idx, _worker_id)| {
                let client = client.clone();

                async move {
                    let mut results = vec![];
                    for _ in 0..self.call_count {
                        results.push(
                            invoke_and_await_http(client.clone(), || {
                                (self.http_request)(
                                    port,
                                    idx,
                                    iteration.api_definition_id.clone(),
                                    iteration.length,
                                )
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
                result.record(&recorder, "ts-agent-rib-", idx.to_string().as_str());
            }
        }

        info!("Measuring direct rust throughput via RPC");
        measure_workers(
            &self.deps,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .direct_rust_rpc_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.into())
                .collect::<Vec<_>>(),
            &self.rust_rpc_function_name,
            &self.function_params,
            "direct-rust-rpc-",
        )
        .await;

        info!("Measuring TS agent RPC throughput...");
        measure_workers(
            &self.deps,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .ts_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.into())
                .collect::<Vec<_>>(),
            &self.ts_rpc_function_name,
            &self.function_params,
            "ts-agent-rpc-",
        )
        .await;

        info!("Measuring Rust agent RPC throughput...");
        measure_workers(
            &self.deps,
            &iteration.routing_table,
            &recorder,
            iteration.length,
            self.call_count,
            &iteration
                .rust_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.into())
                .collect::<Vec<_>>(),
            &self.rust_agent_rpc_function_name,
            &self.function_params,
            "rust-agent-rpc-",
        )
        .await;
        // TODO: native rust
    }

    pub async fn cleanup_iteration(&self, iteration: IterationContext) {
        delete_workers(&self.deps, &iteration.direct_rust_worker_ids).await;
        delete_workers(&self.deps, &iteration.rust_agent_worker_ids).await;
        delete_workers(&self.deps, &iteration.ts_agent_worker_ids).await;
        delete_workers(&self.deps, &iteration.ts_agent_worker_ids_for_rib).await;
        delete_workers(
            &self.deps,
            &iteration
                .direct_rust_rpc_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.parent)
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(
            &self.deps,
            &iteration
                .direct_rust_rpc_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.child)
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(
            &self.deps,
            &iteration
                .ts_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.parent)
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(
            &self.deps,
            &iteration
                .ts_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.child)
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(
            &self.deps,
            &iteration
                .rust_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.parent)
                .collect::<Vec<_>>(),
        )
        .await;
        delete_workers(
            &self.deps,
            &iteration
                .rust_rpc_agent_worker_id_pairs
                .iter()
                .cloned()
                .map(|pair| pair.child)
                .collect::<Vec<_>>(),
        )
        .await;
    }
}
