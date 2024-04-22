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
use clap::Parser;
use golem_wasm_rpc::Value;

use golem_common::model::WorkerId;
use golem_test_framework::config::{CliParams, CliTestDependencies, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkApi, BenchmarkRecorder};
use golem_test_framework::dsl::TestDsl;

#[derive(Clone)]
struct Context {
    deps: CliTestDependencies,
    worker_ids: Vec<WorkerId>,
}

struct SuspendWorkerLatency {
    config: CliParams,
}

#[async_trait]
impl Benchmark for SuspendWorkerLatency {
    type IterationContext = Context;

    fn name() -> &'static str {
        "suspend"
    }

    async fn create(config: CliParams) -> Self {
        Self { config }
    }

    async fn setup_iteration(&self) -> Self::IterationContext {
        // Initialize infrastructure
        let deps = CliTestDependencies::new(self.config.clone()).await;

        // Upload test component
        let component_id = deps.store_component("clocks").await;
        let mut worker_ids = Vec::new();

        // Create 'size' workers
        for i in 0..self.config.benchmark_config.size {
            let worker_id = deps
                .start_worker(&component_id, &format!("worker-{i}"))
                .await;
            worker_ids.push(worker_id);
        }

        Self::IterationContext { deps, worker_ids }
    }

    async fn warmup(&self, context: &Self::IterationContext) {
        // Invoke each worker a few times in parallel
        let mut fibers = Vec::new();
        for worker_id in &context.worker_ids {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                context_clone
                    .deps
                    .invoke_and_await(&worker_id_clone, "sleep-for", vec![Value::F64(1.0)])
                    .await
                    .expect("invoke_and_await failed");
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }
    }

    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder) {
        // Invoke each worker a 'length' times in parallel and record the duration
        let mut fibers = Vec::new();
        for (n, worker_id) in context.worker_ids.iter().enumerate() {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.benchmark_config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone
                        .deps
                        .invoke_and_await(&worker_id_clone, "sleep-for", vec![Value::F64(10.0)])
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

    async fn cleanup_iteration(&self, context: Self::IterationContext) {
        context.deps.kill_all();
    }
}

#[tokio::main]
async fn main() {
    let params = CliParams::parse();
    CliTestDependencies::init_logging(&params);
    let result = SuspendWorkerLatency::run_benchmark(params).await;
    println!("{}", result);
}
