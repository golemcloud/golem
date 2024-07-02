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
use golem_common::model::WorkerId;
use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;
use integration_tests::benchmarks::{
    get_worker_ids, invoke_and_await, run_benchmark, setup_benchmark, start, BenchmarkContext,
};
use tokio::task::JoinSet;
use tracing::warn;

struct DurabilityOverhead {
    config: RunConfig,
}

#[derive(Clone)]
pub struct Context {
    pub durable_worker_ids: Vec<WorkerId>,
    pub not_durable_worker_ids: Vec<WorkerId>,
}

#[async_trait]
impl Benchmark for DurabilityOverhead {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = Context;

    fn name() -> &'static str {
        "durability-overhead"
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
        let component_id = benchmark_context
            .deps
            .store_unique_component("shopping-cart")
            .await;

        let durable_worker_ids = get_worker_ids(self.config.size, &component_id, "durable-worker");

        start(durable_worker_ids.clone(), benchmark_context.deps.clone()).await;

        let not_durable_worker_ids =
            get_worker_ids(self.config.size, &component_id, "not-durable-worker");

        start(
            not_durable_worker_ids.clone(),
            benchmark_context.deps.clone(),
        )
        .await;

        Context {
            durable_worker_ids,
            not_durable_worker_ids,
        }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        async fn initialize(
            worker_ids: Vec<WorkerId>,
            context: &BenchmarkContext,
            not_durable: bool,
        ) {
            // Invoke each worker a few times in parallel
            let mut fibers = JoinSet::new();
            for worker_id in worker_ids.clone() {
                let context_clone = context.clone();
                let worker_id_clone = worker_id.clone();
                let _ = fibers.spawn(async move {
                    if not_durable {
                        invoke_and_await(
                            &context_clone.deps,
                            &worker_id_clone,
                            "golem:it/api.{not-durable}",
                            vec![],
                        )
                        .await;
                    }

                    invoke_and_await(
                        &context_clone.deps,
                        &worker_id_clone,
                        "golem:it/api.{initialize-cart}",
                        vec![Value::String(worker_id_clone.worker_name.clone())],
                    )
                    .await
                });
            }

            while let Some(fiber) = fibers.join_next().await {
                fiber.expect("fiber failed");
            }
        }

        let bc = benchmark_context.clone();
        let ids = context.durable_worker_ids.clone();
        let init1 = tokio::spawn(async move { initialize(ids, &bc, false).await });
        let bc = benchmark_context.clone();
        let ids = context.not_durable_worker_ids.clone();
        let init2 = tokio::spawn(async move { initialize(ids, &bc, true).await });

        init1.await.unwrap();
        init2.await.unwrap();
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        async fn run_for(
            worker_ids: Vec<WorkerId>,
            prefix: String,
            length: usize,
            context: &BenchmarkContext,
            recorder: &BenchmarkRecorder,
        ) {
            // Invoke each worker a 'length' times in parallel and record the duration
            let mut fibers = JoinSet::new();
            for (n, worker_id) in worker_ids.iter().enumerate() {
                let prefix_clone = prefix.clone();
                let context_clone = context.clone();
                let worker_id_clone = worker_id.clone();
                let recorder_clone = recorder.clone();
                let _ = fibers.spawn(async move {
                    for i in 0..length {
                        let result = invoke_and_await(
                            &context_clone.deps,
                            &worker_id_clone,
                            "golem:it/api.{add-item}",
                            vec![Value::Record(vec![
                                Value::String(i.to_string()),
                                Value::String(format!("{} Golem T-Shirt M", i)),
                                Value::F32(100.0 + i as f32),
                                Value::U32(i as u32),
                            ])],
                        )
                        .await;
                        recorder_clone.duration(
                            &format!("{prefix_clone}-invocation"),
                            result.accumulated_time,
                        );
                        recorder_clone.duration(
                            &format!("{prefix_clone}-worker-{n}"),
                            result.accumulated_time,
                        );
                        recorder_clone.count(
                            &format!("{prefix_clone}-invocation-retries"),
                            result.retries as u64,
                        );
                        recorder_clone.count(
                            &format!("{prefix_clone}-worker-{n}-retries"),
                            result.retries as u64,
                        );
                        recorder_clone.count(
                            &format!("{prefix_clone}-invocation-timeouts"),
                            result.timeouts as u64,
                        );
                        recorder_clone.count(
                            &format!("{prefix_clone}-worker-{n}-timeouts"),
                            result.timeouts as u64,
                        );
                    }
                });
            }

            while let Some(fiber) = fibers.join_next().await {
                fiber.expect("fiber failed");
            }
        }

        let length = self.config.length;
        let bc = benchmark_context.clone();
        let ids = context.durable_worker_ids.clone();
        let rec = recorder.clone();
        let set1 =
            tokio::spawn(
                async move { run_for(ids, "durable".to_string(), length, &bc, &rec).await },
            );

        let bc = benchmark_context.clone();
        let ids = context.not_durable_worker_ids.clone();
        let rec = recorder.clone();
        let set2 = tokio::spawn(async move {
            run_for(ids, "not-durable".to_string(), length, &bc, &rec).await
        });

        // Running the two types simulatenously to eliminate differences coming from ordering
        set1.await.unwrap();
        set2.await.unwrap();
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        for worker_id in &context.durable_worker_ids {
            if let Err(err) = benchmark_context.deps.delete_worker(worker_id).await {
                warn!("Failed to delete worker: {:?}", err);
            }
        }

        for worker_id in &context.not_durable_worker_ids {
            if let Err(err) = benchmark_context.deps.delete_worker(worker_id).await {
                warn!("Failed to delete worker: {:?}", err);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<DurabilityOverhead>().await;
}
