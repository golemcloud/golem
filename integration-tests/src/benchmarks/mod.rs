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

use golem_common::model::{ComponentId, WorkerId};
use golem_test_framework::config::{CliParams, CliTestDependencies};
use golem_test_framework::dsl::benchmark::{BenchmarkApi, BenchmarkRecorder, BenchmarkResult, RunConfig};
use golem_test_framework::dsl::TestDsl;
pub mod cold_start_large;

#[derive(Clone)]
pub struct BenchmarkContext {
    pub deps: CliTestDependencies,
}

#[derive(Clone)]
pub struct IterationContext {
    pub worker_ids: Vec<WorkerId>,
}

pub fn get_worker_ids(size: usize, component_id: &ComponentId, prefix: &str) -> Vec<WorkerId> {
    let mut worker_ids = Vec::new();
    for i in 0..size {
        let worker_name = format!("{prefix}-{i}");
        worker_ids.push(WorkerId {
            component_id: component_id.clone(),
            worker_name,
        });
    }
    worker_ids
}

pub async fn setup_with(
    size: usize,
    component_name: &str,
    worker_name_prefix: &str,
    start_workers: bool,
    deps: CliTestDependencies,
) -> Vec<WorkerId> {
    // Initialize infrastructure

    // Upload test component
    let component_id = deps.store_unique_component(component_name).await;
    // Create 'size' workers
    let worker_ids = get_worker_ids(size, &component_id, worker_name_prefix);

    if start_workers {
        start(worker_ids.clone(), deps.clone()).await
    }

    worker_ids
}

pub async fn start(worker_ids: Vec<WorkerId>, deps: CliTestDependencies) {
    for worker_id in worker_ids {
        let _ = deps
            .start_worker(&worker_id.component_id, &worker_id.worker_name)
            .await;
    }
}

pub async fn setup_benchmark(params: CliParams, cluster_size: usize) -> BenchmarkContext {
    // Initialize infrastructure
    let deps = CliTestDependencies::new(params.clone(), cluster_size).await;

    BenchmarkContext { deps }
}

pub async fn setup_iteration(
    benchmark_context: &BenchmarkContext,
    config: RunConfig,
    component_name: &str,
    start_workers: bool,
) -> IterationContext {
    let worker_ids = setup_with(
        config.size,
        component_name,
        "worker",
        start_workers,
        benchmark_context.deps.clone(),
    )
    .await;

    IterationContext { worker_ids }
}

pub async fn cleanup_iteration(benchmark_context: &BenchmarkContext, context: IterationContext) {
    for worker_id in &context.worker_ids {
        benchmark_context.deps.delete_worker(worker_id).await
    }
}

pub async fn warmup_echo(
    benchmark_context: &BenchmarkContext,
    iteration_context: &IterationContext,
) {
    let mut fibers = Vec::new();
    for worker_id in &iteration_context.worker_ids {
        let context_clone = benchmark_context.clone();
        let worker_id_clone = worker_id.clone();
        let fiber = tokio::task::spawn(async move {
            context_clone
                .deps
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:it/api/echo",
                    vec![Value::String("hello".to_string())],
                )
                .await
                .expect("invoke_and_await failed");
        });
        fibers.push(fiber);
    }

    for fiber in fibers {
        fiber.await.expect("fiber failed");
    }
}

pub async fn run_echo(
    length: usize,
    benchmark_context: &BenchmarkContext,
    iteration_context: &IterationContext,
    recorder: BenchmarkRecorder,
) {
    // Invoke each worker a 'length' times in parallel and record the duration
    let mut fibers = Vec::new();
    for (n, worker_id) in iteration_context.worker_ids.iter().enumerate() {
        let context_clone = benchmark_context.clone();
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

pub async fn get_benchmark_results<A: BenchmarkApi>(params: CliParams)-> BenchmarkResult {
    CliTestDependencies::init_logging(&params);
    A::run_benchmark(params).await
}

pub async fn run_benchmark<A: BenchmarkApi>() {
    let params = CliParams::parse();
    let result = get_benchmark_results::<A>(params.clone()).await;
    if params.json {
        let str = serde_json::to_string(&result).expect("Failed to serialize BenchmarkResult");
        println!("{}", str);
    } else {
        println!("{}", result.view());
    }
}
