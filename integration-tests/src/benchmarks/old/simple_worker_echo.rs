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

use crate::benchmarks::{
    benchmark_invocations, delete_workers, setup_benchmark, setup_simple_iteration, warmup_workers,
    SimpleBenchmarkContext, SimpleIterationContext,
};
use async_trait::async_trait;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::TestDependencies;
use golem_wasm::IntoValueAndType;
use indoc::indoc;
use tracing::Level;

pub struct SimpleWorkerEcho {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for SimpleWorkerEcho {
    type BenchmarkContext = SimpleBenchmarkContext;
    type IterationContext = SimpleIterationContext;

    fn name() -> &'static str {
        "simple-worker-echo"
    }

    fn description() -> &'static str {
        indoc! {
            "This benchmark starts the `option-service.wasm` test component on `size` number of workers,
            calls the `echo` function on each as a warmup, and then measures the invoke-and-await time
            of calling each worker in parallel `length` times.
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
    ) -> Self::BenchmarkContext {
        setup_benchmark(mode, verbosity, cluster_size).await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all().await
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        setup_simple_iteration(
            benchmark_context,
            self.config.clone(),
            "option-service",
            true,
        )
        .await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        // Invoke each worker in parallel
        warmup_workers(
            &benchmark_context.deps,
            &context.worker_ids,
            "golem:it/api.{echo}",
            vec![Some("hello".to_string()).into_value_and_type()],
        )
        .await;
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // Invoke each worker a 'length' times in parallel and record the duration
        benchmark_invocations(
            &benchmark_context.deps,
            recorder,
            self.config.length,
            &context.worker_ids,
            "golem:it/api.{echo}",
            vec![Some("hello".to_string()).into_value_and_type()],
            "",
        )
        .await;
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(&benchmark_context.deps, &context.worker_ids).await
    }
}
