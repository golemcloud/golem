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

use clap::Parser;
use golem_wasm_rpc::Value;

use golem_common::model::WorkerId;
use golem_test_framework::config::{CliParams, CliTestDependencies};
use golem_test_framework::dsl::benchmark::{BenchmarkApi, BenchmarkRecorder};
use golem_test_framework::dsl::TestDsl;

#[derive(Clone)]
pub struct Context {
    pub deps: CliTestDependencies,
    pub worker_ids: Vec<WorkerId>,
}

pub async fn setup(config: CliParams, template_name: &str) -> Context {
    // Initialize infrastructure
    let deps = CliTestDependencies::new(config.clone()).await;

    // Upload test template
    let template_id = deps.store_template(template_name).await;
    let mut worker_ids = Vec::new();

    // Create 'size' workers
    for i in 0..config.benchmark_config.size {
        let worker_id = deps
            .start_worker(&template_id, &format!("worker-{i}"))
            .await;
        worker_ids.push(worker_id);
    }

    Context { deps, worker_ids }
}

pub async fn run_echo(length: usize, context: &Context, recorder: BenchmarkRecorder) {
    // Invoke each worker a 'length' times in parallel and record the duration
    let mut fibers = Vec::new();
    for (n, worker_id) in context.worker_ids.iter().enumerate() {
        let context_clone = context.clone();
        let worker_id_clone = worker_id.clone();
        let recorder_clone = recorder.clone();
        let fiber = tokio::task::spawn(async move {
            for _ in 0..length {
                let start = SystemTime::now();
                context_clone
                    .deps
                    .invoke_and_await(
                        &worker_id_clone,
                        "golem:it/api/echo",
                        vec![Value::String("hello".to_string())],
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

pub async fn run_benchmark<A: BenchmarkApi>() {
    let params = CliParams::parse();
    CliTestDependencies::init_logging(&params);
    let result = A::run_benchmark(params).await;
    println!("{}", result);
}
