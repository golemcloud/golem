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
use golem_common::model::WorkerId;
use golem_wasm_rpc::Value;

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::{
    get_worker_ids, run_benchmark, setup_benchmark, start, BenchmarkContext,
};

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
        async fn initialize(worker_ids: Vec<WorkerId>, context: &BenchmarkContext) {
            // Invoke each worker a few times in parallel
            let mut fibers = Vec::new();
            for worker_id in worker_ids.clone() {
                let context_clone = context.clone();
                let worker_id_clone = worker_id.clone();
                let fiber = tokio::task::spawn(async move {
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:it/api.{initialize-cart}",
                            vec![Value::String(worker_id_clone.worker_name.clone())],
                        )
                        .await
                        .expect("initialize-cart invoke_and_await failed");
                });
                fibers.push(fiber);
            }

            for fiber in fibers {
                fiber.await.expect("fiber failed");
            }
        }

        initialize(context.durable_worker_ids.clone(), benchmark_context).await;

        let mut fibers = Vec::new();
        for worker_id in &context.not_durable_worker_ids.clone() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                context_clone
                    .deps
                    .invoke_and_await(&worker_id_clone, "golem:it/api.{not-durable}", vec![])
                    .await
                    .expect("not-durable invoke_and_await failed");
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        initialize(context.not_durable_worker_ids.clone(), benchmark_context).await;
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
            let mut fibers = Vec::new();
            for (n, worker_id) in worker_ids.iter().enumerate() {
                let prefix_clone = prefix.clone();
                let context_clone = context.clone();
                let worker_id_clone = worker_id.clone();
                let recorder_clone = recorder.clone();
                let fiber = tokio::task::spawn(async move {
                    for i in 0..length {
                        let start = SystemTime::now();
                        context_clone
                            .deps
                            .invoke_and_await(
                                &worker_id_clone,
                                "golem:it/api.{add-item}",
                                vec![Value::Record(vec![
                                    Value::String(i.to_string()),
                                    Value::String(format!("{} Golem T-Shirt M", i)),
                                    Value::F32(100.0 + i as f32),
                                    Value::U32(i as u32),
                                ])],
                            )
                            .await
                            .expect("add-item invoke_and_await failed");
                        let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                        recorder_clone.duration(&format!("{prefix_clone}-invocation"), elapsed);
                        recorder_clone.duration(&format!("{prefix_clone}-worker-{n}"), elapsed);
                    }
                });
                fibers.push(fiber);
            }

            for fiber in fibers {
                fiber.await.expect("fiber failed");
            }
        }

        run_for(
            context.durable_worker_ids.clone(),
            "durable".to_string(),
            self.config.length,
            benchmark_context,
            &recorder,
        )
        .await;

        run_for(
            context.not_durable_worker_ids.clone(),
            "not-durable".to_string(),
            self.config.length,
            benchmark_context,
            &recorder,
        )
        .await;
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        for worker_id in &context.durable_worker_ids {
            benchmark_context.deps.delete_worker(worker_id).await
        }

        for worker_id in &context.not_durable_worker_ids {
            benchmark_context.deps.delete_worker(worker_id).await
        }
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<DurabilityOverhead>().await;
}
