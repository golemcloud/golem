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

use crate::benchmarks::{
    benchmark_invocations, delete_workers, setup_iteration, warmup_workers, RustServiceClient,
};
use async_trait::async_trait;
use golem_common::model::WorkerId;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::{
    BenchmarkTestDependencies, CliTestService, TestDependencies,
    TestService,
};
use golem_wasm::IntoValueAndType;
use indoc::indoc;
use std::collections::HashMap;
use std::time::SystemTime;
use tokio::task::JoinSet;
use tracing::Level;

pub struct Throughput {
    config: RunConfig,
}

#[derive(Clone)]
pub struct BenchmarkContext {
    pub deps: BenchmarkTestDependencies,
    pub rust_service: CliTestService,
    pub rust_client: RustServiceClient,
}

#[derive(Clone)]
pub struct IterationContext {
    pub worker_ids: Vec<WorkerId>,
}

#[async_trait]
impl Benchmark for Throughput {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "throughput"
    }

    fn description() -> &'static str {
        indoc! {
            "The throughput benchmark starts `size` workers, calls each of them once in the warmup phase
            and calls a simple 'echo' function returning a small string `length` times to measure invocation time.

            We are running this benchmark with a `size` parameter that is low enough that every worker is always
            in memory as we are not interested in the overhead introduced by recovering workers.

            The same endpoints are also implemented as a native Rust http server to compare Golem’s general overhead."
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
    ) -> Self::BenchmarkContext {
        let rust_client = RustServiceClient::new("http://localhost:3000");
        let rust_service = CliTestService::new(
            mode,
            verbosity,
            "rust-http-service".to_string(),
            HashMap::new(),
            Some("test-components/rust-service".to_string()),
        );

        let deps = BenchmarkTestDependencies::new(mode, verbosity, cluster_size).await;

        BenchmarkContext {
            deps,
            rust_service,
            rust_client,
        }
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all().await;
        benchmark_context.rust_service.kill_all();
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let worker_ids = setup_iteration(
            self.config.size,
            "child_component",
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
            "golem:it-exports/api.{echo}",
            vec!["hello".into_value_and_type()],
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
        benchmark_invocations(
            &benchmark_context.deps,
            recorder.clone(),
            self.config.length,
            &context.worker_ids,
            "golem:it-exports/api.{echo}",
            vec!["hello".into_value_and_type()],
            "worker-echo-",
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
                    recorder_clone.duration(&"rust-http-echo-invocation".into(), elapsed);
                }
            });
        }

        while let Some(res) = fibers.join_next().await {
            res.expect("fiber failed");
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
