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

pub struct LatencySmall {
    config: RunConfig,
}
pub struct LatencyMedium {
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
            be swapped out of the executor's memory. This variant uses a small rust agent.
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
            "benchmark_agent_rust_release",
            "benchmark:agent-rust",
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
    component: ComponentDto,
    agent_ids: Vec<AgentId>,
    length: usize,
}

pub struct LatencyBenchmark {
    component_name: String,
    root_package_name: String,
    agent_type_name: String,
    deps: BenchmarkTestDependencies,
}

impl LatencyBenchmark {
    pub async fn new(
        component_name: &str,
        root_package_name: &str,
        agent_type_name: &str,
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self {
        Self {
            component_name: component_name.to_string(),
            root_package_name: root_package_name.to_string(),
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
        let (_, env) = user.app_and_env().await.unwrap();

        let component = user
            .component(&env.id, &self.component_name)
            .name(&self.root_package_name)
            .store()
            .await
            .unwrap();

        let mut agent_ids = vec![];
        for n in 0..config.size {
            let agent_id = agent_id!(&self.agent_type_name, format!("test-{n}"));
            agent_ids.push(agent_id);
        }

        IterationContext {
            user,
            component,
            agent_ids,
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
            .agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = iteration.user.clone();

                let cold_result = invoke_and_await_agent(
                    &user_clone,
                    &iteration.component,
                    agent_id,
                    "echo",
                    data_value!("benchmark"),
                )
                .await;

                let mut hot_results = vec![];
                for _ in 0..iteration.length {
                    let hot_result = invoke_and_await_agent(
                        &user_clone,
                        &iteration.component,
                        agent_id,
                        "echo",
                        data_value!("benchmark"),
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
        let worker_ids: Vec<WorkerId> = iteration
            .agent_ids
            .iter()
            .filter_map(|agent_id| WorkerId::from_agent_id(iteration.component.id, agent_id).ok())
            .collect();
        delete_workers(&iteration.user, &worker_ids).await
    }
}
