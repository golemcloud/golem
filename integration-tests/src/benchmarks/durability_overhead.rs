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

use async_trait::async_trait;
use golem_wasm_rpc::IntoValueAndType;

use crate::benchmarks::{
    benchmark_invocations, delete_workers, generate_worker_ids, run_benchmark, setup_benchmark,
    start_workers, warmup_workers, SimpleBenchmarkContext,
};
use golem_common::model::WorkerId;
use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;

struct DurabilityOverhead {
    config: RunConfig,
}

#[derive(Clone)]
pub struct Context {
    pub durable_worker_ids: Vec<WorkerId>,
    pub durable_committed_worker_ids: Vec<WorkerId>,
    pub not_durable_worker_ids: Vec<WorkerId>,
}

const COUNT: u64 = 1000; // Number of durable operations to perform in each invocation

#[async_trait]
impl Benchmark for DurabilityOverhead {
    type BenchmarkContext = SimpleBenchmarkContext;
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
        benchmark_context.deps.kill_all().await
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
            .component("durability-overhead")
            .unique()
            .store()
            .await;

        let durable_worker_ids =
            generate_worker_ids(self.config.size, &component_id, "durable-worker");

        start_workers(&durable_worker_ids, &benchmark_context.deps).await;

        let durable_committed_worker_ids =
            generate_worker_ids(self.config.size, &component_id, "durable-committed-worker");

        start_workers(&durable_committed_worker_ids, &benchmark_context.deps).await;

        let not_durable_worker_ids =
            generate_worker_ids(self.config.size, &component_id, "not-durable-worker");

        start_workers(&not_durable_worker_ids, &benchmark_context.deps).await;

        Context {
            durable_worker_ids,
            durable_committed_worker_ids,
            not_durable_worker_ids,
        }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        warmup_workers(
            &benchmark_context.deps,
            &context.durable_worker_ids,
            "golem:it/api.{run}",
            vec![
                1u64.into_value_and_type(),
                false.into_value_and_type(),
                false.into_value_and_type(),
            ],
        )
        .await;
        warmup_workers(
            &benchmark_context.deps,
            &context.durable_committed_worker_ids,
            "golem:it/api.{run}",
            vec![
                1u64.into_value_and_type(),
                false.into_value_and_type(),
                true.into_value_and_type(),
            ],
        )
        .await;
        warmup_workers(
            &benchmark_context.deps,
            &context.not_durable_worker_ids,
            "golem:it/api.{run}",
            vec![
                1u64.into_value_and_type(),
                true.into_value_and_type(),
                false.into_value_and_type(),
            ],
        )
        .await;
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
            &context.durable_worker_ids,
            "golem:it/api.{run}",
            vec![
                COUNT.into_value_and_type(),
                false.into_value_and_type(),
                false.into_value_and_type(),
            ],
            "durable-",
        )
        .await;

        benchmark_invocations(
            &benchmark_context.deps,
            recorder.clone(),
            self.config.length,
            &context.durable_committed_worker_ids,
            "golem:it/api.{run}",
            vec![
                COUNT.into_value_and_type(),
                false.into_value_and_type(),
                true.into_value_and_type(),
            ],
            "durable-committed-",
        )
        .await;

        benchmark_invocations(
            &benchmark_context.deps,
            recorder.clone(),
            self.config.length,
            &context.durable_committed_worker_ids,
            "golem:it/api.{run}",
            vec![
                COUNT.into_value_and_type(),
                true.into_value_and_type(),
                false.into_value_and_type(),
            ],
            "not-durable-",
        )
        .await;
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(&benchmark_context.deps, &context.durable_worker_ids).await;
        delete_workers(
            &benchmark_context.deps,
            &context.durable_committed_worker_ids,
        )
        .await;
        delete_workers(&benchmark_context.deps, &context.not_durable_worker_ids).await;
    }
}

pub async fn run() {
    run_benchmark::<DurabilityOverhead>().await;
}
