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
use golem_wasm_rpc::Value;

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::{
    cleanup_iteration, run_benchmark, setup_benchmark, setup_iteration, BenchmarkContext,
    IterationContext,
};

struct SimpleWorkerEcho {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for SimpleWorkerEcho {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "simple-worker-echo"
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
        // Invoke each worker a few times in parallel
        let mut fibers = Vec::new();
        for worker_id in &context.worker_ids {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                for _ in 0..5 {
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:it/api.{echo}",
                            vec![Value::Option(Some(Box::new(Value::String(
                                "hello".to_string(),
                            ))))],
                        )
                        .await
                        .expect("invoke_and_await failed");
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // Invoke each worker a 'length' times in parallel and record the duration
        let mut fibers = Vec::new();
        for (n, worker_id) in context.worker_ids.iter().enumerate() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:it/api.{echo}",
                            vec![Value::Option(Some(Box::new(Value::String(
                                "hello".to_string(),
                            ))))],
                        )
                        .await
                        .expect("invoke_and_await failed");
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"invocation".to_string(), elapsed);
                    recorder_clone.duration(&format!("worker-{n}"), elapsed);
                }
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
    run_benchmark::<SimpleWorkerEcho>().await;
}
