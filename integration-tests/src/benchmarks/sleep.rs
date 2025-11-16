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
use golem_common::base_model::WorkerId;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::IntoValueAndType;
use indoc::indoc;
use tracing::{info, Level};

pub struct Sleep {
    config: RunConfig,
}

pub struct SleepBenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct SleepIterationContext {
    worker_ids: Vec<WorkerId>,
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
        let mut worker_ids = vec![];

        info!("Registering component");
        let component_id = benchmark_context
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust")
            .name("benchmark:direct-rust")
            .store()
            .await;

        for n in 0..self.config.size {
            let worker_id = WorkerId {
                component_id: component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}\")"),
            };
            worker_ids.push(worker_id);
        }

        SleepIterationContext { worker_ids }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        info!("Warming up {} workers...", context.worker_ids.len());

        let result_futures = context
            .worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = benchmark_context.deps.clone();

                invoke_and_await(
                    &deps_clone,
                    worker_id,
                    "benchmark:direct-rust-exports/benchmark-direct-rust-api.{sleep}",
                    vec![10u64.into_value_and_type()],
                )
                .await
            })
            .collect::<Vec<_>>();
        let _ = result_futures.join().await;

        info!("Warmed up {} workers", context.worker_ids.len());
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        let length = self.config.length as u64;
        let result_futures = context
            .worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = benchmark_context.deps.clone();

                invoke_and_await(
                    &deps_clone,
                    worker_id,
                    "benchmark:direct-rust-exports/benchmark-direct-rust-api.{sleep}",
                    vec![length.into_value_and_type()],
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
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(&benchmark_context.deps, &context.worker_ids).await
    }
}
