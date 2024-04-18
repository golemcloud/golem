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
use golem_wasm_rpc::Value;

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::{run_benchmark, run_echo, setup, Context};

struct SimpleWorkerEcho {
    config: CliParams,
}

#[async_trait]
impl Benchmark for SimpleWorkerEcho {
    type IterationContext = Context;

    fn name() -> &'static str {
        "simple-worker-echo"
    }

    async fn create(config: CliParams) -> Self {
        Self { config }
    }

    async fn setup_iteration(&self) -> Self::IterationContext {
        setup(self.config.clone(), "option-service").await
    }

    async fn warmup(&self, context: &Self::IterationContext) {
        // Invoke each worker a few times in parallel
        let mut fibers = Vec::new();
        for worker_id in &context.worker_ids {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let fiber = tokio::task::spawn(async move {
                for _ in 0..5 {
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:it/api/echo",
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

    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder) {
        run_echo(self.config.benchmark_config.length, context, recorder).await
    }

    async fn cleanup_iteration(&self, context: Self::IterationContext) {
        context.deps.kill_all();
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<SimpleWorkerEcho>().await;
}
