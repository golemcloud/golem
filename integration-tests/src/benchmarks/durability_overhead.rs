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

use crate::benchmarks::{delete_workers, invoke_and_await};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::base_model::WorkerId;
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::IntoValueAndType;
use indoc::indoc;
use tracing::{info, Level};

pub struct DurabilityOverhead {
    config: RunConfig,
}

pub struct DurabilityOverheadBenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct DurabilityOverheadIterationContext {
    durable_persistent_worker_ids: Vec<WorkerId>,
    durable_nonpersistent_worker_ids: Vec<WorkerId>,
    ephemeral_worker_ids: Vec<WorkerId>,
}

#[async_trait]
impl Benchmark for DurabilityOverhead {
    type BenchmarkContext = DurabilityOverheadBenchmarkContext;
    type IterationContext = DurabilityOverheadIterationContext;

    fn name() -> &'static str {
        "durability-overhead"
    }

    fn description() -> &'static str {
        indoc! {
            "Invokes oplog-heavy functions in parallel on `size` number of workers,
            using ephemeral components, and durable components with persistence on and off.
            The invoked function gets the `length` parameter to control the length of its inner loop.
            The benchmark can be used to compare the overhead caused by of persistence (using a low
            `size`) and also the effect of heavy persistence load caused by parallel running workers
            (using high `size`).
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self::BenchmarkContext {
        DurabilityOverheadBenchmarkContext {
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
        }
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all().await;
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let mut durable_persistent_worker_ids = vec![];
        let mut durable_nonpersistent_worker_ids = vec![];
        let mut ephemeral_worker_ids = vec![];

        info!("Registering component");
        let durable_component_id = benchmark_context
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust")
            .name("benchmark:direct-rust")
            .store()
            .await;
        let ephemeral_component_id = benchmark_context
            .deps
            .admin()
            .await
            .component("benchmark_direct_rust")
            .unique()
            .ephemeral()
            .store()
            .await;

        for n in 0..self.config.size {
            let worker_id = WorkerId {
                component_id: durable_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}-persistent\")"),
            };
            durable_persistent_worker_ids.push(worker_id);

            let worker_id = WorkerId {
                component_id: durable_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}-persistent\")"),
            };
            durable_nonpersistent_worker_ids.push(worker_id);

            let worker_id = WorkerId {
                component_id: ephemeral_component_id.clone(),
                worker_name: format!("benchmark-agent(\"test-{n}-ephemeral\")"),
            };
            ephemeral_worker_ids.push(worker_id);
        }

        DurabilityOverheadIterationContext {
            durable_nonpersistent_worker_ids,
            durable_persistent_worker_ids,
            ephemeral_worker_ids,
        }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        info!(
            "Warming up {} workers...",
            context.durable_persistent_worker_ids.len()
                + context.durable_nonpersistent_worker_ids.len()
        );

        async fn warmup(deps: &BenchmarkTestDependencies, ids: &[WorkerId]) {
            let result_futures = ids
                .iter()
                .map(move |worker_id| async move {
                    let deps_clone = deps.clone().into_admin().await;
                    invoke_and_await(
                        &deps_clone,
                        worker_id,
                        "benchmark:direct-rust-exports/benchmark-direct-rust-api.{echo}",
                        vec!["test".into_value_and_type()],
                    )
                    .await
                })
                .collect::<Vec<_>>();
            let _ = result_futures.join().await;
        }

        warmup(
            &benchmark_context.deps,
            &context.durable_persistent_worker_ids,
        )
        .await;
        warmup(
            &benchmark_context.deps,
            &context.durable_nonpersistent_worker_ids,
        )
        .await;

        info!(
            "Warmed up {} workers",
            context.durable_persistent_worker_ids.len()
                + context.durable_nonpersistent_worker_ids.len()
        );
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        let length = self.config.length as u32;
        let result_futures1 = context
            .durable_persistent_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = benchmark_context.deps.clone().into_admin().await;

                invoke_and_await(
                    &deps_clone,
                    worker_id,
                    "benchmark:direct-rust-exports/benchmark-direct-rust-api.{oplog-heavy}",
                    vec![length.into_value_and_type(), true.into_value_and_type()],
                )
                .await
            })
            .collect::<Vec<_>>();
        let results1 = result_futures1.join().await;
        for (idx, result) in results1.iter().enumerate() {
            result.record(&recorder, "durable-persistent-", idx.to_string().as_str());
        }

        let result_futures2 = context
            .durable_nonpersistent_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = benchmark_context.deps.clone().into_admin().await;

                invoke_and_await(
                    &deps_clone,
                    worker_id,
                    "benchmark:direct-rust-exports/benchmark-direct-rust-api.{oplog-heavy}",
                    vec![length.into_value_and_type(), false.into_value_and_type()],
                )
                .await
            })
            .collect::<Vec<_>>();
        let results2 = result_futures2.join().await;
        for (idx, result) in results2.iter().enumerate() {
            result.record(
                &recorder,
                "durable-non-persistent-",
                idx.to_string().as_str(),
            );
        }

        let result_futures3 = context
            .ephemeral_worker_ids
            .iter()
            .map(move |worker_id| async move {
                let deps_clone = benchmark_context.deps.clone().into_admin().await;

                invoke_and_await(
                    &deps_clone,
                    worker_id,
                    "benchmark:direct-rust-exports/benchmark-direct-rust-api.{oplog-heavy}",
                    vec![length.into_value_and_type(), false.into_value_and_type()],
                )
                .await
            })
            .collect::<Vec<_>>();
        let results3 = result_futures3.join().await;
        for (idx, result) in results3.iter().enumerate() {
            result.record(&recorder, "ephemeral-", idx.to_string().as_str());
        }
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(
            &benchmark_context.deps,
            &context.durable_persistent_worker_ids,
        )
        .await;
        delete_workers(
            &benchmark_context.deps,
            &context.durable_nonpersistent_worker_ids,
        )
        .await;
        delete_workers(&benchmark_context.deps, &context.ephemeral_worker_ids).await;
    }
}
