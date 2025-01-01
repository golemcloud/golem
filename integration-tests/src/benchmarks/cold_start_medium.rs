// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_wasm_rpc::Value;
use integration_tests::benchmarks::{
    benchmark_invocations, delete_workers, run_benchmark, setup_benchmark, setup_simple_iteration,
    SimpleBenchmarkContext, SimpleIterationContext,
};

struct ColdStartEchoMedium {
    config: RunConfig,
    params: CliParams,
}

#[async_trait]
impl Benchmark for ColdStartEchoMedium {
    type BenchmarkContext = SimpleBenchmarkContext;
    type IterationContext = SimpleIterationContext;

    fn name() -> &'static str {
        "cold-start-medium"
    }

    async fn create_benchmark_context(
        params: CliParams,
        cluster_size: usize,
    ) -> Self::BenchmarkContext {
        setup_benchmark(params, cluster_size).await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all().await
    }

    async fn create(params: CliParams, config: RunConfig) -> Self {
        Self { config, params }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        setup_simple_iteration(benchmark_context, self.config.clone(), "js-echo", false).await
    }

    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        _context: &Self::IterationContext,
    ) {
        if !self.params.mode.compilation_service_disabled() {
            // Waiting a bit so the component compilation service can precompile the component
            tokio::time::sleep(std::time::Duration::from_secs(90)).await;
        }
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // Invoking the echo function on each worker once
        benchmark_invocations(
            &benchmark_context.deps,
            recorder,
            1,
            &context.worker_ids,
            "golem:it/api.{echo}",
            vec![Value::String("hello".to_string())],
            "",
        )
        .await
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(&benchmark_context.deps, &context.worker_ids).await
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<ColdStartEchoMedium>().await;
}
