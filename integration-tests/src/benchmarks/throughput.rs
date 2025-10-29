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
use golem_common::model::WorkerId;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValueAndType, ValueAndType};
use indoc::indoc;
use reqwest::{Body, Method, Request, Url};
use serde_json::json;
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
            "
            Spawns `size` number of workers of various implementations, and calls the `echo` endpoint
            a fixed number of times. The 'length' parameter is not used.
            `size` should be chosen in a way that all workers fit in the available executor's memory,
            and `size=1` can be used to test isolated throughput of a single worker.
            The benchmarks measures multiple implementations separately after each other:
            direct rust, native rust, TS agent, TS agent through rib mapping
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
    ) -> Self::BenchmarkContext {
        ThroughputBenchmark::new(
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{echo}",
            "benchmark:agent-ts/benchmark-agent.{echo}",
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
            "
            Spawns `size` number of workers of various implementations, and calls the `large-input` endpoint
            with an input based on `length` in a fixed number of times.
            `size` should be chosen in a way that all workers fit in the available executor's memory,
            and `size=1` can be used to test isolated throughput of a single worker.
            The benchmarks measures multiple implementations separately after each other:
            direct rust, native rust, TS agent, TS agent through rib mapping
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
    ) -> Self::BenchmarkContext {
        ThroughputBenchmark::new(
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{large-input}",
            "benchmark:agent-ts/benchmark-agent.{large-input}",
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
            "
            Spawns `size` number of workers of various implementations, and calls the `cpu-intensive` endpoint
            with the CPU intensive task's length based on `length` in a fixed number of times.
            `size` should be chosen in a way that all workers fit in the available executor's memory,
            and `size=1` can be used to test isolated throughput of a single worker.
            The benchmarks measures multiple implementations separately after each other:
            direct rust, native rust, TS agent, TS agent through rib mapping
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
    ) -> Self::BenchmarkContext {
        ThroughputBenchmark::new(
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{cpu-intensive}",
            "benchmark:agent-ts/benchmark-agent.{cpu-intensive}",
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

pub struct IterationContext {
    direct_rust_worker_ids: Vec<WorkerId>,
    ts_agent_worker_ids: Vec<WorkerId>,
    ts_agent_worker_ids_for_rib: Vec<WorkerId>,
    length: usize,
    api_definition_id: String,
}

pub struct ThroughputBenchmark {
    rust_function_name: String,
    ts_function_name: String,
    function_params: Box<dyn Fn(usize) -> Vec<ValueAndType> + Send + Sync + 'static>,
    http_request: Box<dyn Fn(u16, usize, String, usize) -> Request + Send + Sync + 'static>,
    deps: BenchmarkTestDependencies,
    call_count: usize,
}

impl ThroughputBenchmark {
    pub async fn new(
        rust_function_name: &str,
        ts_function_name: &str,
        function_params: Box<dyn Fn(usize) -> Vec<ValueAndType> + Send + Sync + 'static>,
        http_request: Box<dyn Fn(u16, usize, String, usize) -> Request + Send + Sync + 'static>,
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        call_count: usize,
    ) -> Self {
        Self {
            rust_function_name: rust_function_name.to_string(),
            ts_function_name: ts_function_name.to_string(),
            function_params,
            http_request,
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
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
        let mut ts_agent_worker_ids = vec![];
        let mut ts_agent_worker_ids_for_rib = vec![];

        info!("Registering components");

        let rust_direct_component_id = self
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust")
            .name("benchmark:direct-rust")
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

        for n in 0..config.size {
            direct_rust_worker_ids.push(WorkerId {
                component_id: rust_direct_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}\")"),
            });
            ts_agent_worker_ids.push(WorkerId {
                component_id: ts_agent_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}\")"),
            });
            ts_agent_worker_ids_for_rib.push(WorkerId {
                component_id: ts_agent_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}-rib\")"),
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
            project_id: admin.default_project_id.0.clone(),
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
            ts_agent_worker_ids,
            ts_agent_worker_ids_for_rib,
            length: config.length,
            api_definition_id,
        }
    }

    pub async fn warmup(&self, iteration: &IterationContext) {
        info!("Warming up direct rust workers...");
        let result_futures = iteration
            .direct_rust_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = self.deps.clone().into_admin().await;

                invoke_and_await(
                    &deps_clone,
                    &worker_id,
                    &self.rust_function_name,
                    (self.function_params)(iteration.length),
                )
                .await
            })
            .collect::<Vec<_>>();

        let _ = result_futures.join().await;

        info!("Warming up TS agents...");
        let result_futures = iteration
            .ts_agent_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = self.deps.clone().into_admin().await;

                invoke_and_await(
                    &deps_clone,
                    &worker_id,
                    &self.ts_function_name,
                    (self.function_params)(iteration.length),
                )
                .await
            })
            .collect::<Vec<_>>();

        let _ = result_futures.join().await;

        info!("Warming up TS agents for Rib mapping...");
        let result_futures = iteration
            .ts_agent_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = self.deps.clone().into_admin().await;

                invoke_and_await(
                    &deps_clone,
                    &worker_id,
                    &self.ts_function_name,
                    (self.function_params)(iteration.length),
                )
                .await
            })
            .collect::<Vec<_>>();

        let _ = result_futures.join().await;
        info!("Warmup completed");
    }

    pub async fn run(&self, iteration: &IterationContext, recorder: BenchmarkRecorder) {
        info!("Measuring direct rust throughput");

        let result_futures = iteration
            .direct_rust_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = self.deps.clone().into_admin().await;

                let mut results = vec![];
                for _ in 0..self.call_count {
                    results.push(
                        invoke_and_await(
                            &deps_clone,
                            &worker_id,
                            &self.rust_function_name,
                            (self.function_params)(iteration.length),
                        )
                        .await,
                    )
                }
                results
            })
            .collect::<Vec<_>>();

        let results = result_futures.join().await;
        for (idx, results) in results.iter().enumerate() {
            for result in results {
                result.record(&recorder, "direct-rust-", idx.to_string().as_str());
            }
        }

        info!("Measuring TS agent throughput...");
        let result_futures = iteration
            .ts_agent_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = self.deps.clone().into_admin().await;

                let mut results = vec![];
                for _ in 0..self.call_count {
                    results.push(
                        invoke_and_await(
                            &deps_clone,
                            &worker_id,
                            &self.ts_function_name,
                            (self.function_params)(iteration.length),
                        )
                        .await,
                    );
                }
                results
            })
            .collect::<Vec<_>>();

        let results = result_futures.join().await;
        for (idx, results) in results.iter().enumerate() {
            for result in results {
                result.record(&recorder, "ts-agent-", idx.to_string().as_str());
            }
        }

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

        // TODO: RPC
        // TODO: native rust
    }

    pub async fn cleanup_iteration(&self, iteration: IterationContext) {
        delete_workers(&self.deps, &iteration.direct_rust_worker_ids).await;
        delete_workers(&self.deps, &iteration.ts_agent_worker_ids).await;
        delete_workers(&self.deps, &iteration.ts_agent_worker_ids_for_rib).await;
    }
}
