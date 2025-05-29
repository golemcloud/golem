// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::benchmarks::data::Data;
use clap::Parser;
use golem_common::model::{ComponentId, IdempotencyKey, WorkerId};
use golem_test_framework::config::{CliParams, CliTestDependencies};
use golem_test_framework::dsl::benchmark::{
    BenchmarkApi, BenchmarkRecorder, BenchmarkResult, ResultKey, RunConfig,
};
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::{Value, ValueAndType};
use reqwest::{Client, Url};
use std::time::{Duration, SystemTime};
use tokio::task::JoinSet;
use tracing::warn;

pub mod data;

pub mod cold_start_large;
pub mod cold_start_medium;
pub mod cold_start_small;

pub mod durability_overhead;

pub mod large_dynamic_memory;
pub mod large_initial_memory;

pub mod latency_large;
pub mod latency_medium;
pub mod latency_small;

pub mod rpc;
pub mod rpc_cpu_intensive;
pub mod rpc_large_input;

pub mod suspend_worker;

pub mod simple_worker_echo;

pub mod throughput;
pub mod throughput_cpu_intensive;
pub mod throughput_large_input;

#[derive(Clone)]
pub struct SimpleBenchmarkContext {
    pub deps: CliTestDependencies,
}

#[derive(Clone)]
pub struct SimpleIterationContext {
    pub worker_ids: Vec<WorkerId>,
}

pub fn generate_worker_ids(size: usize, component_id: &ComponentId, prefix: &str) -> Vec<WorkerId> {
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

pub async fn setup_iteration(
    size: usize,
    component_name: &str,
    worker_name_prefix: &str,
    start_workers: bool,
    deps: &CliTestDependencies,
) -> Vec<WorkerId> {
    // Initialize infrastructure

    // Upload test component
    let component_id = deps.component(component_name).unique().store().await;
    // Create 'size' workers
    let worker_ids = generate_worker_ids(size, &component_id, worker_name_prefix);

    if start_workers {
        crate::benchmarks::start_workers(&worker_ids, deps).await
    }

    worker_ids
}

pub async fn start_workers(worker_ids: &[WorkerId], deps: &CliTestDependencies) {
    for worker_id in worker_ids {
        let _ = deps
            .start_worker(&worker_id.component_id, &worker_id.worker_name)
            .await;
    }
}

pub async fn setup_benchmark(params: CliParams, cluster_size: usize) -> SimpleBenchmarkContext {
    // Initialize infrastructure
    let deps = CliTestDependencies::new(params.clone(), cluster_size).await;

    SimpleBenchmarkContext { deps }
}

pub async fn setup_simple_iteration(
    benchmark_context: &SimpleBenchmarkContext,
    config: RunConfig,
    component_name: &str,
    start_workers: bool,
) -> SimpleIterationContext {
    let worker_ids = setup_iteration(
        config.size,
        component_name,
        "worker",
        start_workers,
        &benchmark_context.deps,
    )
    .await;

    SimpleIterationContext { worker_ids }
}

pub async fn delete_workers(deps: &CliTestDependencies, worker_ids: &[WorkerId]) {
    for worker_id in worker_ids {
        if let Err(err) = deps.delete_worker(worker_id).await {
            warn!("Failed to delete worker: {:?}", err);
        }
    }
}

pub async fn warmup_workers(
    deps: &CliTestDependencies,
    worker_ids: &[WorkerId],
    function: &str,
    params: Vec<ValueAndType>,
) {
    let mut fibers = JoinSet::new();
    for worker_id in worker_ids {
        let deps_clone = deps.clone();
        let worker_id_clone = worker_id.clone();
        let params_clone = params.clone();
        let function_clone = function.to_string();
        let _ = fibers.spawn(async move {
            invoke_and_await(&deps_clone, &worker_id_clone, &function_clone, params_clone).await
        });
    }

    while let Some(res) = fibers.join_next().await {
        let _ = res.expect("Failed to warmup");
    }
}

pub async fn benchmark_invocations(
    deps: &CliTestDependencies,
    recorder: BenchmarkRecorder,
    length: usize,
    worker_ids: &[WorkerId],
    function: &str,
    params: Vec<ValueAndType>,
    prefix: &str,
) {
    // Invoke each worker a 'length' times in parallel and record the duration
    let mut fibers = JoinSet::new();
    for (n, worker_id) in worker_ids.iter().enumerate() {
        let deps_clone = deps.clone();
        let function_clone = function.to_string();
        let params_clone = params.clone();
        let worker_id_clone = worker_id.clone();
        let recorder_clone = recorder.clone();
        let prefix_clone = prefix.to_string();
        let _ = fibers.spawn(async move {
            for _ in 0..length {
                let result = invoke_and_await(
                    &deps_clone,
                    &worker_id_clone,
                    &function_clone,
                    params_clone.clone(),
                )
                .await;
                recorder_clone.duration(
                    &format!("{prefix_clone}invocation").into(),
                    result.accumulated_time,
                );
                recorder_clone.duration(
                    &ResultKey::secondary(format!("{prefix_clone}worker-{n}")),
                    result.accumulated_time,
                );
                recorder_clone.count(
                    &format!("{prefix_clone}invocation-retries").into(),
                    result.retries as u64,
                );
                recorder_clone.count(
                    &ResultKey::secondary(format!("{prefix_clone}worker-{n}-retries")),
                    result.retries as u64,
                );
                recorder_clone.count(
                    &format!("{prefix_clone}invocation-timeouts").into(),
                    result.timeouts as u64,
                );
                recorder_clone.count(
                    &ResultKey::secondary(format!("{prefix_clone}worker-{n}-timeouts")),
                    result.timeouts as u64,
                );
            }
        });
    }

    while let Some(res) = fibers.join_next().await {
        res.unwrap();
    }
}

pub async fn get_benchmark_results<A: BenchmarkApi>(params: CliParams) -> BenchmarkResult {
    CliTestDependencies::init_logging(&params);
    let primary_only = params.primary_only;
    let results = A::run_benchmark(params).await;
    if primary_only {
        results.primary_only()
    } else {
        results
    }
}

pub async fn run_benchmark<A: BenchmarkApi>() {
    let params = CliParams::parse_from(std::env::args_os().skip(1));
    let result = get_benchmark_results::<A>(params.clone()).await;
    if params.json {
        let str = serde_json::to_string(&result).expect("Failed to serialize BenchmarkResult");
        println!("{}", str);
    } else {
        println!("{}", result.view());
    }
}

pub struct InvokeResult {
    pub value: Vec<Value>,
    pub retries: usize,
    pub timeouts: usize,
    pub accumulated_time: Duration,
}

pub async fn invoke_and_await(
    deps: &impl TestDsl,
    worker_id: &WorkerId,
    function_name: &str,
    params: Vec<ValueAndType>,
) -> InvokeResult {
    const TIMEOUT: Duration = Duration::from_secs(180);
    const RETRY_DELAY: Duration = Duration::from_millis(100);

    let key = IdempotencyKey::fresh();

    let mut accumulated_time = Duration::from_secs(0);
    let mut retries = 0;
    let mut timeouts = 0;

    loop {
        let start = SystemTime::now();
        let result = tokio::time::timeout(
            TIMEOUT,
            deps.invoke_and_await_with_key(worker_id, &key, function_name, params.clone()),
        )
        .await;
        let duration = start.elapsed().expect("SystemTime elapsed failed");

        match result {
            Ok(Ok(Ok(r))) => {
                accumulated_time += duration;
                break InvokeResult {
                    value: r,
                    retries,
                    timeouts,
                    accumulated_time,
                };
            }
            Ok(Ok(Err(e))) => {
                // worker error
                println!("Invocation failed, retrying: {:?}", e);
                retries += 1;
                accumulated_time += duration;
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Ok(Err(e)) => {
                // client error
                println!("Invocation failed, retrying: {:?}", e);
                retries += 1;
                accumulated_time += duration;
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(e) => {
                // timeout
                // not counting timeouts into the accumulated time
                timeouts += 1;
                println!("Invocation timed out, retrying: {:?}", e);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RustServiceClient {
    client: Client,
    base_url: Url,
}

impl RustServiceClient {
    pub fn new(url: &str) -> Self {
        let base_url = Url::parse(url).unwrap();
        let client = Client::builder().connection_verbose(true).build().unwrap();

        Self { client, base_url }
    }

    pub async fn calculate(&self, input: u64) -> u64 {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("calculate")
            .push(&input.to_string());

        let request = self.client.get(url.clone());

        let response = request
            .send()
            .await
            .expect("calculate - unexpected response");

        let status = response.status().as_u16();
        match status {
            200 => response
                .json::<u64>()
                .await
                .expect("calculate - unexpected response"),
            _ => panic!("calculate - unexpected response: {status}"),
        }
    }

    pub async fn process(&self, input: Vec<Data>) -> Vec<Data> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("process");

        let mut request = self.client.post(url.clone());

        request = request.json(&input);

        let response = request.send().await.expect("process - unexpected response");

        let status = response.status().as_u16();
        match status {
            200 => response
                .json::<Vec<Data>>()
                .await
                .expect("process - unexpected response"),
            _ => panic!("process - unexpected response: {status}"),
        }
    }

    pub async fn echo(&self, input: &str) -> String {
        let mut url = self.base_url.clone();

        url.path_segments_mut().unwrap().push("echo").push(input);

        let request = self.client.get(url.clone());

        let response = request.send().await.expect("echo - unexpected response");

        let status = response.status().as_u16();
        match status {
            200 => response.text().await.expect("echo - unexpected response"),
            _ => panic!("echo - unexpected response: {status}"),
        }
    }
}
