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

use std::time::SystemTime;

use async_trait::async_trait;

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::{
    cleanup_iteration, run_benchmark, setup_benchmark, setup_iteration, BenchmarkContext,
    IterationContext,
};

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
            "large-initial-memory",
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
            let start = SystemTime::now();
            benchmark_context
                .deps
                .invoke_and_await(worker_id, "run", vec![])
                .await
                .expect("invoke_and_await failed");
            let elapsed = start.elapsed().expect("SystemTime elapsed failed");
            println!("Warmup invocation took {:?}", elapsed);
        }
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // Start each worker and invoke `run` - each worker takes an initial 512Mb memory
        let mut fibers = Vec::new();
        for (n, worker_id) in context.worker_ids.iter().enumerate() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let fiber = tokio::task::spawn(async move {
                let start = SystemTime::now();
                context_clone
                    .deps
                    .invoke_and_await(&worker_id_clone, "run", vec![])
                    .await
                    .expect("invoke_and_await failed");
                let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                recorder_clone.duration(&"invocation".to_string(), elapsed);
                recorder_clone.duration(&format!("worker-{n}"), elapsed);
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
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
