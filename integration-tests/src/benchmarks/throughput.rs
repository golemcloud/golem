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

use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use golem_common::model::WorkerId;
use golem_wasm_rpc::Value;

use golem_test_framework::config::{
    CliParams, CliTestDependencies, CliTestService, TestDependencies, TestService,
};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::{run_benchmark, setup_with};
use reqwest::Client;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use rand::distributions::{Alphanumeric, DistString};
use rand::thread_rng;

struct Throughput {
    params: CliParams,
    config: RunConfig,
}

#[derive(Clone)]
pub struct Context {
    pub deps: CliTestDependencies,
    pub rust_service: CliTestService,
    pub rust_client: RustServiceClient,
    pub worker_ids: Vec<WorkerId>,
}

#[derive(Debug, Clone)]
pub struct RustServiceClient {
    client: Client,
    base_url: Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Data {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub timestamp: u64,
}

impl Data {
    fn generate() -> Self {
        fn generate_random_string(len: usize) -> String {
            let mut rng = thread_rng();
            let string = Alphanumeric.sample_string(&mut rng, len);
            string
        }

        Self {
            id: generate_random_string(256),
            name: generate_random_string(512),
            desc: generate_random_string(1024),
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    fn generate_list(count: usize) -> Vec<Self> {
        (0..count).map(|_| Self::generate()).collect()
    }
}

impl From<Data> for Value {
    fn from(data: Data) -> Self {
        Value::Record(vec![
            Value::String(data.id),
            Value::String(data.name),
            Value::String(data.desc),
            Value::U64(data.timestamp),
        ])
    }
}

impl RustServiceClient {
    pub fn new(url: &str) -> Self {
        let base_url = Url::parse(url).unwrap();
        let client = Client::builder().connection_verbose(true).build().unwrap();

        Self { client, base_url }
    }

    async fn calculate(&self, input: u64) -> u64 {
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

    async fn process(&self, input: Vec<Data>) -> Vec<Data> {
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

    async fn echo(&self, input: &str) -> String {
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

#[async_trait]
impl Benchmark for Throughput {
    type IterationContext = Context;

    fn name() -> &'static str {
        "throughput"
    }

    async fn create(params: CliParams, config: RunConfig) -> Self {
        Self { params, config }
    }

    async fn setup_iteration(&self) -> Self::IterationContext {
        let rust_client = RustServiceClient::new("http://localhost:3000");
        let rust_service = CliTestService::new(
            self.params.clone(),
            "rust-http-service".to_string(),
            HashMap::new(),
            Some("test-components/rust-service".to_string()),
        );

        let deps = CliTestDependencies::new(self.params.clone(), self.config.clone()).await;
        let worker_ids = setup_with(
            1, //self.config.size,
            "rust_component_service",
            "worker",
            true,
            deps.clone(),
        )
        .await;

        Context {
            deps,
            rust_service,
            rust_client,
            worker_ids,
        }
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

        context.rust_client.echo("hello").await;
    }

    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder) {
        let calculate_iter: u64 = 200000;

        let mut fibers = Vec::new();
        for worker_id in context.worker_ids.iter() {
            let context_clone = context.clone();
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
                            "golem:it/api/echo",
                            vec![Value::String("hello".to_string())],
                        )
                        .await
                        .expect("invoke_and_await failed");
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"worker-echo-invocation".to_string(), elapsed);
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        let mut fibers = Vec::new();
        for worker_id in context.worker_ids.iter() {
            let context_clone = context.clone();
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
                            "golem:it/api/calculate",
                            vec![Value::U64(calculate_iter)],
                        )
                        .await
                        .expect("invoke_and_await failed");
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"worker-calculate-invocation".to_string(), elapsed);
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        let mut fibers = Vec::new();
        for _ in context.worker_ids.iter() {
            let context_clone = context.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone.rust_client.echo("hello").await;
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"rust-http-echo-invocation".to_string(), elapsed);
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        let mut fibers = Vec::new();
        for _ in context.worker_ids.iter() {
            let context_clone = context.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone.rust_client.calculate(calculate_iter).await;
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"rust-http-calculate-invocation".to_string(), elapsed);
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        let data = Data::generate_list(1000);

        let values = data
            .clone()
            .into_iter()
            .map(|d| d.into())
            .collect::<Vec<Value>>();

        let mut fibers = Vec::new();
        for worker_id in context.worker_ids.iter() {
            let context_clone = context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let values_clone = values.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:it/api/calculate",
                            vec![Value::List(values_clone.clone())],
                        )
                        .await
                        .expect("invoke_and_await failed");
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"worker-calculate-invocation".to_string(), elapsed);
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }

        let mut fibers = Vec::new();
        for _ in context.worker_ids.iter() {
            let context_clone = context.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let data_clone = data.clone();
            let fiber = tokio::task::spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone.rust_client.process(data_clone.clone()).await;
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"rust-http-process-invocation".to_string(), elapsed);
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
        context.rust_service.kill_all();
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<Throughput>().await;
}
