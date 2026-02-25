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

use crate::benchmarks::delete_workers;
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
use tracing::{info, Level};

pub struct Sleep {
    config: RunConfig,
}

pub struct SleepBenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct SleepIterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    agent_ids: Vec<AgentId>,
}

#[async_trait]
impl Benchmark for Sleep {
    type BenchmarkContext = SleepBenchmarkContext;
    type IterationContext = SleepIterationContext;

    fn name() -> &'static str {
        "sleep"
    }

    fn description() -> &'static str {
        indoc! {
            "Launch `size` workers and invoke a function on each in parallel that sleeps for `length` milliseconds.
            The result is the measured invocation time, which is affected by the amount of workers fitting in memory
            and also the scheduler that wakes them up.
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
        SleepBenchmarkContext {
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

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all().await;
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let user = benchmark_context.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();

        info!("Registering component");
        let component = user
            .component(&env.id, "benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await
            .unwrap();

        let mut agent_ids = vec![];
        for n in 0..self.config.size {
            let agent_id = agent_id!("rust-benchmark-agent", format!("test-{n}"));
            agent_ids.push(agent_id);
        }

        SleepIterationContext {
            user,
            component,
            agent_ids,
        }
    }

    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        info!("Warming up {} workers...", context.agent_ids.len());

        let result_futures = context
            .agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = context.user.clone();

                crate::benchmarks::invoke_and_await_agent(
                    &user_clone,
                    &context.component,
                    agent_id,
                    "sleep",
                    data_value!(10u64),
                )
                .await
            })
            .collect::<Vec<_>>();
        let _ = result_futures.join().await;

        info!("Warmed up {} workers", context.agent_ids.len());
    }

    async fn run(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        let length = self.config.length as u64;
        let result_futures = context
            .agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = context.user.clone();

                crate::benchmarks::invoke_and_await_agent(
                    &user_clone,
                    &context.component,
                    agent_id,
                    "sleep",
                    data_value!(length),
                )
                .await
            })
            .collect::<Vec<_>>();
        let results = result_futures.join().await;
        for (idx, result) in results.iter().enumerate() {
            result.record(&recorder, "", idx.to_string().as_str());
        }
    }

    async fn cleanup_iteration(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        let worker_ids: Vec<WorkerId> = context
            .agent_ids
            .iter()
            .filter_map(|agent_id| WorkerId::from_agent_id(context.component.id, agent_id).ok())
            .collect();
        delete_workers(&context.user, &worker_ids).await
    }
}
