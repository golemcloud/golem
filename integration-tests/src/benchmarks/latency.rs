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

use crate::benchmarks::{delete_workers, invoke_and_await};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::model::WorkerId;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::{IntoValueAndType, ValueAndType};
use indoc::indoc;
use std::time::Duration;
use tracing::{info, Level};

pub struct LatencySmall {
    config: RunConfig,
}
pub struct LatencyMedium {
    config: RunConfig,
}
pub struct LatencyLarge {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for LatencySmall {
    type BenchmarkContext = LatencyBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "latency-small"
    }

    fn description() -> &'static str {
        indoc! {
            "Benchmarks both the cold and hot latency of a invoking a component that can potentially
            be swapped out of the executor's memory. This variant uses a small rust component.
            The `size` parameter is the number of workers to create. The `length` parameter is the number
            of hot invocations to be done per worker (the first one is separately recorded as cold).
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
        LatencyBenchmark::new(
            "benchmark_direct_rust",
            "benchmark:direct-rust",
            "benchmark:direct-rust-exports/benchmark-direct-rust-api.{echo}",
            vec!["benchmark".into_value_and_type()],
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
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
        _context: &Self::IterationContext,
    ) {
        benchmark_context.warmup(&self.config).await
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
impl Benchmark for LatencyMedium {
    type BenchmarkContext = LatencyBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "latency-medium"
    }

    fn description() -> &'static str {
        indoc! {
            "Benchmarks both the cold and hot latency of a invoking a component that can potentially
            be swapped out of the executor's memory. This variant uses a simple TypeScript agent.
            The `size` parameter is the number of workers to create. The `length` parameter is the number
            of hot invocations to be done per worker (the first one is separately recorded as cold).
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
        LatencyBenchmark::new(
            "benchmark_agent_ts",
            "benchmark:agent-ts",
            "benchmark:agent-ts/benchmark-agent.{echo}",
            vec!["benchmark".into_value_and_type()],
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
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
        _context: &Self::IterationContext,
    ) {
        benchmark_context.warmup(&self.config).await
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
impl Benchmark for LatencyLarge {
    type BenchmarkContext = LatencyBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "latency-large"
    }

    fn description() -> &'static str {
        indoc! {
            "Benchmarks both the cold and hot latency of a invoking a component that can potentially
            be swapped out of the executor's memory. This variant uses a TypeScript agent with many AI dependencies.
            The `size` parameter is the number of workers to create. The `length` parameter is the number
            of hot invocations to be done per worker (the first one is separately recorded as cold).
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
        LatencyBenchmark::new(
            "benchmark_agent_ts_large",
            "benchmark:agent-ts-large",
            "benchmark:agent-ts-large/benchmark-agent.{echo}",
            vec!["benchmark".into_value_and_type()],
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
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
        _context: &Self::IterationContext,
    ) {
        benchmark_context.warmup(&self.config).await
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
    user: TestUserContext<BenchmarkTestDependencies>,
    worker_ids: Vec<WorkerId>,
    length: usize,
}

pub struct LatencyBenchmark {
    component_name: String,
    root_package_name: String,
    function_name: String,
    function_params: Vec<ValueAndType>,
    deps: BenchmarkTestDependencies,
}

impl LatencyBenchmark {
    pub async fn new(
        component_name: &str,
        root_package_name: &str,
        function_name: &str,
        function_params: Vec<ValueAndType>,
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self {
        Self {
            component_name: component_name.to_string(),
            root_package_name: root_package_name.to_string(),
            function_name: function_name.to_string(),
            function_params,
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
        }
    }

    pub async fn cleanup(&self) {
        self.deps.kill_all().await
    }

    pub async fn setup_iteration(&self, config: &RunConfig) -> IterationContext {
        let user = self.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();

        let mut worker_ids = vec![];

        let component = user
            .component(&env.id, &self.component_name)
            .name(&self.root_package_name)
            .store()
            .await
            .unwrap();

        for n in 0..config.size {
            let worker_id = WorkerId {
                component_id: component.id,
                worker_name: format!("benchmark-agent(\"test-{n}\")"),
            };
            worker_ids.push(worker_id);
        }

        IterationContext {
            user,
            worker_ids,
            length: config.length,
        }
    }

    pub async fn warmup(&self, config: &RunConfig) {
        if !config.disable_compilation_cache {
            let duration = Duration::from_secs(15);
            info!("Waiting {duration:?} for compilation cache");
            tokio::time::sleep(duration).await;
        } else {
            info!("Skipping waiting for compilation cache, as it is disabled");
        }
    }

    pub async fn run(&self, iteration: &IterationContext, recorder: BenchmarkRecorder) {
        let result_futures = iteration
            .worker_ids
            .iter()
            .map(move |worker_id| async move {
                let user_clone = iteration.user.clone();

                let cold_result = invoke_and_await(
                    &user_clone,
                    worker_id,
                    &self.function_name,
                    self.function_params.clone(),
                )
                .await;

                let mut hot_results = vec![];
                for _ in 0..iteration.length {
                    let hot_result = invoke_and_await(
                        &user_clone,
                        worker_id,
                        &self.function_name,
                        self.function_params.clone(),
                    )
                    .await;
                    hot_results.push(hot_result);
                }

                (cold_result, hot_results)
            })
            .collect::<Vec<_>>();

        let results = result_futures.join().await;
        for (idx, (cold_result, hot_results)) in results.iter().enumerate() {
            cold_result.record(&recorder, "cold-", idx.to_string().as_str());
            for hot_result in hot_results {
                hot_result.record(&recorder, "hot-", idx.to_string().as_str());
            }
        }
    }

    pub async fn cleanup_iteration(&self, iteration: IterationContext) {
        delete_workers(&iteration.user, &iteration.worker_ids).await
    }
}
