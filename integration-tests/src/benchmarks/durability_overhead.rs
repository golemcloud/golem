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

use golem_test_framework::config::{CliParams, CliTestDependencies, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::{run_benchmark, setup_with};

struct DurabilityOverhead {
    config: CliParams,
}

#[derive(Clone)]
pub struct Context {
    pub deps: CliTestDependencies,
    pub durable_worker_ids: Vec<WorkerId>,
    pub not_durable_worker_ids: Vec<WorkerId>,
}

#[async_trait]
impl Benchmark for DurabilityOverhead {
    type IterationContext = Context;

    fn name() -> &'static str {
        "durability-overhead"
    }

    async fn create(config: CliParams) -> Self {
        Self { config }
    }

    async fn setup_iteration(&self) -> Self::IterationContext {
        let deps = CliTestDependencies::new(self.config.clone()).await;

        let durable_worker_ids = setup_with(
            self.config.benchmark_config.size,
            "shopping-cart",
            "durable-worker",
            true,
            deps.clone(),
        )
        .await;

        let not_durable_worker_ids = setup_with(
            self.config.benchmark_config.size,
            "shopping-cart",
            "not-durable-worker",
            true,
            deps.clone(),
        )
        .await;

        Context {
            deps,
            durable_worker_ids,
            not_durable_worker_ids,
        }
    }

    async fn warmup(&self, context: &Self::IterationContext) {
        // Invoke each worker a few times in parallel
        let mut fibers = Vec::new();
        for worker_id in &context.durable_worker_ids.clone() {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                context_clone
                    .deps
                    .invoke_and_await(
                        &worker_id_clone,
                        "golem:it/api/initialize-cart",
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

        let mut fibers = Vec::new();
        for worker_id in &context.not_durable_worker_ids.clone() {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                context_clone
                    .deps
                    .invoke_and_await(&worker_id_clone, "golem:it/api/not-durable", vec![])
                    .await
                    .expect("not-durable invoke_and_await failed");
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        let mut fibers = Vec::new();
        for worker_id in &context.not_durable_worker_ids.clone() {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                context_clone
                    .deps
                    .invoke_and_await(
                        &worker_id_clone,
                        "golem:it/api/initialize-cart",
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

    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder) {
        async fn run_for(
            worker_ids: Vec<WorkerId>,
            prefix: String,
            length: usize,
            context: &Context,
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
                                "golem:it/api/add-item",
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
            self.config.benchmark_config.length,
            context,
            &recorder,
        )
        .await;

        run_for(
            context.not_durable_worker_ids.clone(),
            "not-durable".to_string(),
            self.config.benchmark_config.length,
            context,
            &recorder,
        )
        .await;
    }

    async fn cleanup_iteration(&self, context: Self::IterationContext) {
        context.deps.kill_all();
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<DurabilityOverhead>().await;
}
