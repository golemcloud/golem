// Copyright 2024 Golem Cloud
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
use integration_tests::benchmarks::{
    cleanup_iteration, invoke_and_await, run_benchmark, setup_benchmark, setup_iteration,
    BenchmarkContext, IterationContext,
};
use tokio::task::JoinSet;

struct LargeDynamicMemory {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for LargeDynamicMemory {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

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
        benchmark_context.deps.kill_all()
    }

    async fn create(_params: CliParams, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        setup_iteration(
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
        let mut fibers = JoinSet::new();
        for (n, worker_id) in context.worker_ids.iter().enumerate() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let fiber = fibers.spawn(async move {
                let result =
                    invoke_and_await(&context_clone.deps, &worker_id_clone, "run", vec![]).await;
                recorder_clone.duration(&"invocation".to_string(), result.accumulated_time);
                recorder_clone.duration(&format!("worker-{n}"), result.accumulated_time);
                recorder_clone.count(&"invocation-retries".to_string(), result.retries as u64);
                recorder_clone.count(&format!("worker-{n}-retries"), result.retries as u64);
                recorder_clone.count(&"invocation-timeouts".to_string(), result.timeouts as u64);
                recorder_clone.count(&format!("worker-{n}-timeouts"), result.timeouts as u64);
            });
        }

        while let Some(fiber) = fibers.join_next().await {
            fiber.expect("fiber failed");
        }
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        cleanup_iteration(benchmark_context, context).await
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<LargeDynamicMemory>().await;
}
