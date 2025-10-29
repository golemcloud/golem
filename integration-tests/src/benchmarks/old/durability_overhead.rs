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
    benchmark_invocations, delete_workers, generate_worker_ids, setup_benchmark, start_workers,
    warmup_workers, SimpleBenchmarkContext,
};
use async_trait::async_trait;
use golem_common::model::WorkerId;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::IntoValueAndType;
use indoc::indoc;
use tracing::Level;

pub struct DurabilityOverhead {
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

    fn description() -> &'static str {
        indoc! {
            "This benchmarks measures the overhead introduced by persisting operations.
             The component invoked generates N (=1000) random numbers by calling `wall-clock::now` and sums them.
             As the time query is something Golem needs to persist, this means at least 1000 oplog entries per invocation.

            (Note that it could be also calling the WASI random number generator interface directly
            - first attempt was using the `rand` crate but that does not call the underlying WASI interface for each `next_u32`.)

            We run and measure the invocations on `size` number of workers in parallel in three modes:

            - `durable-invocation` is the default mode, durability is enabled
            - `durable-committed-invocation` performs a `oplog_commit` before returning with the result
            - `not-durable-invocation` sets the persistency level to `PersistNothing` before starting generating the numbers
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
    ) -> Self::BenchmarkContext {
        setup_benchmark(mode, verbosity, cluster_size).await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all().await
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let component_id = benchmark_context
            .deps
            .admin()
            .await
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
