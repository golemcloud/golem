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
use golem_wasm_rpc::Value;
use reqwest::Client;
use reqwest::Url;
use tokio::task::JoinSet;

use golem_common::model::WorkerId;
use golem_test_framework::config::{
    CliParams, CliTestDependencies, CliTestService, TestDependencies, TestService,
};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use integration_tests::benchmarks::data::Data;
use integration_tests::benchmarks::{
    benchmark_invocations, delete_workers, run_benchmark, setup_iteration, warmup_workers,
};

struct Throughput {
    config: RunConfig,
}

#[derive(Clone)]
pub struct BenchmarkContext {
    pub deps: CliTestDependencies,
    pub rust_service: CliTestService,
    pub rust_client: RustServiceClient,
}

#[derive(Clone)]
pub struct IterationContext {
    pub worker_ids: Vec<WorkerId>,
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
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "throughput"
    }

    async fn create_benchmark_context(
        params: CliParams,
        cluster_size: usize,
    ) -> Self::BenchmarkContext {
        let rust_client = RustServiceClient::new("http://localhost:3000");
        let rust_service = CliTestService::new(
            params.clone(),
            "rust-http-service".to_string(),
            HashMap::new(),
            Some("test-components/rust-service".to_string()),
        );

        let deps = CliTestDependencies::new(params.clone(), cluster_size).await;

        BenchmarkContext {
            deps,
            rust_service,
            rust_client,
        }
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all();
        benchmark_context.rust_service.kill_all();
    }

    async fn create(_params: CliParams, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let worker_ids = setup_iteration(
            1, //self.config.size,
            "rust_component_service",
            "worker",
            true,
            &benchmark_context.deps,
        )
        .await;

        IterationContext { worker_ids }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        // Invoke each worker in parallel
        warmup_workers(
            &benchmark_context.deps,
            &context.worker_ids,
            "golem:it/api.{echo}",
            vec![Value::String("hello".to_string())],
        )
        .await;

        benchmark_context.rust_client.echo("hello").await;
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        let calculate_iter: u64 = 200000;
        let data = Data::generate_list(2000);
        let values = data
            .clone()
            .into_iter()
            .map(|d| d.into())
            .collect::<Vec<Value>>();

        benchmark_invocations(
            &benchmark_context.deps,
            recorder.clone(),
            self.config.length,
            &context.worker_ids,
            "golem:it/api.{echo}",
            vec![Value::String("hello".to_string())],
            "worker-echo-",
        )
        .await;

        benchmark_invocations(
            &benchmark_context.deps,
            recorder.clone(),
            self.config.length,
            &context.worker_ids,
            "golem:it/api.{calculate}",
            vec![Value::U64(calculate_iter)],
            "worker-calculate-",
        )
        .await;

        benchmark_invocations(
            &benchmark_context.deps,
            recorder.clone(),
            self.config.length,
            &context.worker_ids,
            "golem:it/api.{process}",
            vec![Value::List(values.clone())],
            "worker-process-",
        )
        .await;

        let mut fibers = JoinSet::new();
        for _ in context.worker_ids.iter() {
            let context_clone = benchmark_context.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let _ = fibers.spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone.rust_client.echo("hello").await;
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"rust-http-echo-invocation".to_string(), elapsed);
                }
            });
        }

        while let Some(res) = fibers.join_next().await {
            res.expect("fiber failed");
        }

        let mut fibers = JoinSet::new();
        for _ in context.worker_ids.iter() {
            let context_clone = benchmark_context.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let _ = fibers.spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone.rust_client.calculate(calculate_iter).await;
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"rust-http-calculate-invocation".to_string(), elapsed);
                }
            });
        }

        while let Some(res) = fibers.join_next().await {
            res.expect("fiber failed");
        }

        let mut fibers = JoinSet::new();
        for _ in context.worker_ids.iter() {
            let context_clone = benchmark_context.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let data_clone = data.clone();
            let _ = fibers.spawn(async move {
                for _ in 0..length {
                    let start = SystemTime::now();
                    context_clone.rust_client.process(data_clone.clone()).await;
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"rust-http-process-invocation".to_string(), elapsed);
                }
            });
        }

        while let Some(res) = fibers.join_next().await {
            res.expect("fiber failed");
        }
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(&benchmark_context.deps, &context.worker_ids).await;
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<Throughput>().await;
}
