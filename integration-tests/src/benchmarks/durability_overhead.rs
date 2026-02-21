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

use crate::benchmarks::{delete_workers, invoke_and_await_agent};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::base_model::agent::AgentId;
use golem_common::model::component::ComponentId;
use golem_common::model::WorkerId;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use tracing::{info, Level};

pub struct DurabilityOverhead {
    config: RunConfig,
}

pub struct DurabilityOverheadBenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct DurabilityOverheadIterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component_id: ComponentId,
    durable_persistent_agent_ids: Vec<AgentId>,
    durable_nonpersistent_agent_ids: Vec<AgentId>,
    ephemeral_agent_ids: Vec<AgentId>,
    durable_persistent_commit_agent_ids: Vec<AgentId>,
}

fn agent_ids_to_worker_ids(component_id: ComponentId, agent_ids: &[AgentId]) -> Vec<WorkerId> {
    agent_ids
        .iter()
        .filter_map(|agent_id| WorkerId::from_agent_id(component_id, agent_id).ok())
        .collect()
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
            using ephemeral components, and durable components with persistence on and off. There's
            also a variant of persistent run where after each operation we force oplog
            commit. The invoked function gets the `length` parameter to control the length of its inner loop.
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
        let user = benchmark_context.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();

        let mut durable_persistent_agent_ids = vec![];
        let mut durable_nonpersistent_agent_ids = vec![];
        let mut ephemeral_agent_ids = vec![];
        let mut durable_persistent_commit_agent_ids = vec![];

        info!("Registering component");
        let durable_component = user
            .component(&env.id, "benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await
            .unwrap();

        for n in 0..self.config.size {
            durable_persistent_agent_ids.push(agent_id!(
                "rust-benchmark-agent",
                format!("test-{n}-persistent")
            ));
            durable_nonpersistent_agent_ids.push(agent_id!(
                "rust-benchmark-agent",
                format!("test-{n}-nonpersistent")
            ));
            ephemeral_agent_ids.push(agent_id!(
                "rust-ephemeral-benchmark-agent",
                format!("test-{n}-ephemeral")
            ));
            durable_persistent_commit_agent_ids.push(agent_id!(
                "rust-benchmark-agent",
                format!("test-{n}-persistent-commit")
            ));
        }

        DurabilityOverheadIterationContext {
            user,
            component_id: durable_component.id,
            durable_persistent_agent_ids,
            durable_nonpersistent_agent_ids,
            ephemeral_agent_ids,
            durable_persistent_commit_agent_ids,
        }
    }

    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        info!(
            "Warming up {} workers...",
            context.durable_persistent_agent_ids.len()
                + context.durable_nonpersistent_agent_ids.len()
        );

        async fn warmup(
            user: &TestUserContext<BenchmarkTestDependencies>,
            component_id: &ComponentId,
            ids: &[AgentId],
        ) {
            let result_futures = ids
                .iter()
                .map(move |agent_id| async move {
                    let user_clone = user.clone();
                    invoke_and_await_agent(
                        &user_clone,
                        component_id,
                        agent_id,
                        "echo",
                        data_value!("test"),
                    )
                    .await
                })
                .collect::<Vec<_>>();
            let _ = result_futures.join().await;
        }

        warmup(
            &context.user,
            &context.component_id,
            &context.durable_persistent_agent_ids,
        )
        .await;
        warmup(
            &context.user,
            &context.component_id,
            &context.durable_nonpersistent_agent_ids,
        )
        .await;

        info!(
            "Warmed up {} workers",
            context.durable_persistent_agent_ids.len()
                + context.durable_nonpersistent_agent_ids.len()
        );
    }

    async fn run(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        let length = self.config.length as u32;
        let result_futures1 = context
            .durable_persistent_agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = context.user.clone();
                invoke_and_await_agent(
                    &user_clone,
                    &context.component_id,
                    agent_id,
                    "oplog-heavy",
                    data_value!(length, true, false),
                )
                .await
            })
            .collect::<Vec<_>>();
        let results1 = result_futures1.join().await;
        for (idx, result) in results1.iter().enumerate() {
            result.record(&recorder, "durable-persistent-", idx.to_string().as_str());
        }

        let result_futures2 = context
            .durable_nonpersistent_agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = context.user.clone();
                invoke_and_await_agent(
                    &user_clone,
                    &context.component_id,
                    agent_id,
                    "oplog-heavy",
                    data_value!(length, false, false),
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
            .ephemeral_agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = context.user.clone();
                invoke_and_await_agent(
                    &user_clone,
                    &context.component_id,
                    agent_id,
                    "oplog-heavy",
                    data_value!(length, false, false),
                )
                .await
            })
            .collect::<Vec<_>>();
        let results3 = result_futures3.join().await;
        for (idx, result) in results3.iter().enumerate() {
            result.record(&recorder, "ephemeral-", idx.to_string().as_str());
        }

        let result_futures4 = context
            .durable_persistent_commit_agent_ids
            .iter()
            .map(move |agent_id| async move {
                let user_clone = context.user.clone();
                invoke_and_await_agent(
                    &user_clone,
                    &context.component_id,
                    agent_id,
                    "oplog-heavy",
                    data_value!(length, true, true),
                )
                .await
            })
            .collect::<Vec<_>>();
        let results4 = result_futures4.join().await;
        for (idx, result) in results4.iter().enumerate() {
            result.record(
                &recorder,
                "durable-persistent-commit-",
                idx.to_string().as_str(),
            );
        }
    }

    async fn cleanup_iteration(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        delete_workers(
            &context.user,
            &agent_ids_to_worker_ids(context.component_id, &context.durable_persistent_agent_ids),
        )
        .await;
        delete_workers(
            &context.user,
            &agent_ids_to_worker_ids(
                context.component_id,
                &context.durable_nonpersistent_agent_ids,
            ),
        )
        .await;
        delete_workers(
            &context.user,
            &agent_ids_to_worker_ids(context.component_id, &context.ephemeral_agent_ids),
        )
        .await;
        delete_workers(
            &context.user,
            &agent_ids_to_worker_ids(
                context.component_id,
                &context.durable_persistent_commit_agent_ids,
            ),
        )
        .await;
    }
}
