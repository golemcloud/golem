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
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;
use integration_tests::benchmarks::{
    invoke_and_await, run_benchmark, setup_benchmark, setup_iteration, BenchmarkContext,
    IterationContext,
};
use tokio::task::JoinSet;
use tracing::warn;

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
        let mut fibers = JoinSet::new();
        for worker_id in &context.worker_ids {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            fibers.spawn(async move {
                for _ in 0..5 {
                    invoke_and_await(
                        &context_clone.deps,
                        &worker_id_clone,
                        "golem:it/api.{echo}",
                        vec![Value::Option(Some(Box::new(Value::String(
                            "hello".to_string(),
                        ))))],
                    )
                    .await;
                }
            });
        }

        while let Some(res) = fibers.join_next().await {
            let _ = res.unwrap();
        }
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // Invoke each worker a 'length' times in parallel and record the duration
        let mut fibers = JoinSet::new();
        for (n, worker_id) in context.worker_ids.iter().enumerate() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            fibers.spawn(async move {
                for _ in 0..length {
                    let result = invoke_and_await(
                        &context_clone.deps,
                        &worker_id_clone,
                        "golem:it/api.{echo}",
                        vec![Value::Option(Some(Box::new(Value::String(
                            "hello".to_string(),
                        ))))],
                    )
                    .await;
                    recorder_clone.duration(&"invocation".to_string(), result.accumulated_time);
                    recorder_clone.duration(&format!("worker-{n}"), result.accumulated_time);
                    recorder_clone.count(&"invocation-retries".to_string(), result.retries as u64);
                    recorder_clone.count(&format!("worker-{n}-retries"), result.retries as u64);
                    recorder_clone
                        .count(&"invocation-timeouts".to_string(), result.timeouts as u64);
                    recorder_clone.count(&format!("worker-{n}-timeouts"), result.timeouts as u64);
                }
            });
        }

        while let Some(res) = fibers.join_next().await {
            let _ = res.unwrap();
        }
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        for worker_id in &context.worker_ids {
            if let Err(err) = benchmark_context.deps.delete_worker(worker_id).await {
                warn!("Failed to delete worker: {:?}", err);
            }
        }
    }
}

fn main() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main())
}

// #[tokio::main)]
async fn async_main() {
    run_benchmark::<SimpleWorkerEcho>().await;
}
