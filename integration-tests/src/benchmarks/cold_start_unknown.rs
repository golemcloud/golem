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

use crate::benchmarks::{delete_workers, invoke_and_await_agent};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::base_model::agent::AgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::WorkerId;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use std::time::Duration;
use tracing::{info, Level};

pub struct ColdStartUnknownSmall {
    config: RunConfig,
}
pub struct ColdStartUnknownMedium {
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
            This variant uses a relatively small Rust agent component. The `size` parameter is the number of
            unique components, and `length` is the time in seconds _per component_ to wait for pre-compilation.
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
            "benchmark_agent_rust_release",
            "rust-benchmark-agent",
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
                This variant uses a basic TypeScript agent component. The `size` parameter is the number of unique
                components, and `length` is the time in seconds _per component_ to wait for pre-compilation.
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
            "benchmark-agent",
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
    agents: Vec<(ComponentDto, AgentId)>,
}

pub struct ColdStartUnknownBenchmark {
    component_name: String,
    agent_type_name: String,
    deps: BenchmarkTestDependencies,
}

impl ColdStartUnknownBenchmark {
    pub async fn new(
        component_name: &str,
        agent_type_name: &str,
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self {
        Self {
            component_name: component_name.to_string(),
            agent_type_name: agent_type_name.to_string(),
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
        let mut agents = vec![];

        for _ in 0..config.size {
            // Agent types names are unique within one environment,
            // so make sure each component get its own env
            let (_, env) = user.app_and_env().await.unwrap();

            let component = user
                .component(&env.id, &self.component_name)
                .unique()
                .store()
                .await
                .unwrap();

            let agent_id = agent_id!(&self.agent_type_name, "test");
            agents.push((component, agent_id));
        }

        IterationContext { user, agents }
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
            .agents
            .iter()
            .map(move |(component, agent_id)| async move {
                let user_clone = iteration.user.clone();
                invoke_and_await_agent(
                    &user_clone,
                    component,
                    agent_id,
                    "echo",
                    data_value!("benchmark"),
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
        let worker_ids: Vec<WorkerId> = iteration
            .agents
            .iter()
            .filter_map(|(component, agent_id)| {
                WorkerId::from_agent_id(component.id, agent_id).ok()
            })
            .collect();
        delete_workers(&iteration.user, &worker_ids).await
    }
}
