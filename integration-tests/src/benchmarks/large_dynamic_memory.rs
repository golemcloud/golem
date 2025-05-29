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
    benchmark_invocations, delete_workers, invoke_and_await, run_benchmark, setup_benchmark,
    setup_simple_iteration, SimpleBenchmarkContext, SimpleIterationContext,
};
use async_trait::async_trait;
use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};

struct LargeDynamicMemory {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for LargeDynamicMemory {
    type BenchmarkContext = SimpleBenchmarkContext;
    type IterationContext = SimpleIterationContext;

    fn name() -> &'static str {
        "large-dynamic-memory"
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

    async fn create(_params: CliParams, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        setup_simple_iteration(
            benchmark_context,
            self.config.clone(),
            "large-dynamic-memory",
            false,
        )
        .await
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        if let Some(worker_id) = context.worker_ids.first() {
            let result = invoke_and_await(&benchmark_context.deps, worker_id, "run", vec![]).await;
            println!("Warmup invocation took {:?}", result.accumulated_time);
        }
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // Start each worker and invoke `run` - each worker gradually allocates 512Mb memory
        benchmark_invocations(
            &benchmark_context.deps,
            recorder,
            1,
            &context.worker_ids,
            "run",
            vec![],
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

pub async fn run() {
    run_benchmark::<LargeDynamicMemory>().await;
}
