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
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValueAndType, ValueAndType};
use indoc::indoc;
use std::time::Duration;
use tracing::{info, Level};

pub struct ColdStartUnknownSmall {
    config: RunConfig,
}
pub struct ColdStartUnknownMedium {
    config: RunConfig,
}
pub struct ColdStartUnknownLarge {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for ColdStartUnknownSmall {
    type BenchmarkContext = ColdStartUnknownBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "cold-start-unknown-small"
    }

    fn description() -> &'static str {
        indoc! {
            "
            Benchmarks the first-time invocation of a component that have never been instantiated before.
            This variant uses a relatively small Rust component. 'size' is the number of unique components,
            and 'length' is the time in seconds _per component_ to wait for pre-compilation.
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
        ColdStartUnknownBenchmark::new(
            "benchmark_direct_rust",
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
impl Benchmark for ColdStartUnknownMedium {
    type BenchmarkContext = ColdStartUnknownBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "cold-start-unknown-medium"
    }

    fn description() -> &'static str {
        indoc! {
            "
                Benchmarks the first-time invocation of a component that have never been instantiated before.
                This variant uses a basic TypeScript component. 'size' is the number of unique components,
            and 'length' is the time in seconds _per component_ to wait for pre-compilation.
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
        ColdStartUnknownBenchmark::new(
            "benchmark_agent_ts",
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
impl Benchmark for ColdStartUnknownLarge {
    type BenchmarkContext = ColdStartUnknownBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "cold-start-unknown-large"
    }

    fn description() -> &'static str {
        indoc! {
            "
                Benchmarks the first-time invocation of a component that have never been instantiated before.
                This variant uses a TypeScript component with a lot of linked AI libraries. 'size' is the number of unique components,
                and 'length' is the time in seconds _per component_ to wait for pre-compilation.
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
        ColdStartUnknownBenchmark::new(
            "benchmark_agent_ts_large",
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
    worker_ids: Vec<WorkerId>,
}

pub struct ColdStartUnknownBenchmark {
    component_name: String,
    function_name: String,
    function_params: Vec<ValueAndType>,
    deps: BenchmarkTestDependencies,
}

impl ColdStartUnknownBenchmark {
    pub async fn new(
        component_name: &str,
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
        let mut worker_ids = vec![];

        for _ in 0..config.size {
            let component_id = self
                .deps
                .admin()
                .await
                .component(&self.component_name)
                .unique()
                .store()
                .await;

            let worker_id = WorkerId {
                component_id,
                worker_name: "benchmark-agent(\"test\")".to_string(),
            };
            worker_ids.push(worker_id);
        }

        IterationContext { worker_ids }
    }

    pub async fn warmup(&self, config: &RunConfig) {
        if !config.disable_compilation_cache {
            let duration = Duration::from_secs(config.length as u64 * config.size as u64);
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
                let deps_clone = self.deps.clone().into_admin().await;
                invoke_and_await(
                    &deps_clone,
                    worker_id,
                    &self.function_name,
                    self.function_params.clone(),
                )
                .await
            })
            .collect::<Vec<_>>();

        let results = result_futures.join().await;
        for (idx, result) in results.iter().enumerate() {
            result.record(&recorder, "", idx.to_string().as_str());
        }
    }

    pub async fn cleanup_iteration(&self, iteration: IterationContext) {
        delete_workers(&self.deps, &iteration.worker_ids).await
    }
}
