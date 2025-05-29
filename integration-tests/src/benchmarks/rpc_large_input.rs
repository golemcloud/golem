use std::collections::HashMap;

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
use crate::benchmarks::{
    invoke_and_await, run_benchmark, setup_benchmark, warmup_workers, SimpleBenchmarkContext,
};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::v1::GetRoutingTableRequest;
use golem_common::model::{RoutingTable, WorkerId};
use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::{IntoValueAndType, ValueAndType};
use tokio::task::JoinSet;

struct RpcLargeInput {
    config: RunConfig,
    _params: CliParams,
}

#[derive(Debug, Clone)]
struct ParentChildWorkerId {
    parent: WorkerId,
    child: WorkerId,
}

impl ParentChildWorkerId {
    fn at_same_worker_executor(&self, routing_table: &RoutingTable) -> bool {
        let parent_pod = routing_table.lookup(&self.parent);
        let child_pod = routing_table.lookup(&self.child);

        match (parent_pod, child_pod) {
            (Some(parent_pod), Some(child_pod)) => parent_pod == child_pod,
            _ => panic!("Failed to find the pod of parent and child workers in RPC benchmark"),
        }
    }
}

#[derive(Clone)]
struct RpcBenchmarkIteratorContext {
    worker_ids: Vec<ParentChildWorkerId>,
}

#[async_trait]
impl Benchmark for RpcLargeInput {
    type BenchmarkContext = SimpleBenchmarkContext;
    type IterationContext = RpcBenchmarkIteratorContext;

    fn name() -> &'static str {
        "rpc-benchmark"
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

    async fn create(params: CliParams, config: RunConfig) -> Self {
        Self {
            config,
            _params: params,
        }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let child_component_id = benchmark_context
            .deps
            .component("child_component")
            .unique()
            .store()
            .await;
        let component_id = benchmark_context
            .deps
            .component("parent_component_composed")
            .unique()
            .store()
            .await;

        let mut worker_ids = Vec::new();
        for i in 0..self.config.size {
            // Rpc parent worker-id
            let parent_worker_id = WorkerId {
                component_id: component_id.clone(),
                worker_name: format!("parent_worker-{i}"),
            };

            let child_worker_name = format!("child_worker-{i}");

            let child_worker_id = WorkerId {
                component_id: child_component_id.clone(),
                worker_name: child_worker_name.to_string(),
            };

            let mut env = HashMap::new();

            env.insert(
                "CHILD_COMPONENT_ID".to_string(),
                child_component_id.0.to_string(),
            );
            env.insert(
                "CHILD_WORKER_NAME".to_string(),
                child_worker_name.to_string(),
            );

            benchmark_context
                .deps
                .start_worker_with(
                    &parent_worker_id.component_id,
                    &parent_worker_id.worker_name,
                    vec![],
                    env,
                )
                .await
                .expect("Failed to start parent worker");

            worker_ids.push(ParentChildWorkerId {
                parent: parent_worker_id,
                child: child_worker_id,
            });
        }

        RpcBenchmarkIteratorContext { worker_ids }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        warmup_workers(
            &benchmark_context.deps,
            &context
                .worker_ids
                .iter()
                .map(|w| w.parent.clone())
                .collect::<Vec<WorkerId>>(),
            "golem:itrpc-exports/rpc-api.{echo}",
            vec!["hello".into_value_and_type()],
        )
        .await;
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        let shard_manager = benchmark_context.deps.shard_manager();

        let mut shard_manager_client = shard_manager.client().await;

        let routing_table = shard_manager_client
            .get_routing_table(GetRoutingTableRequest {})
            .await
            .expect("Unable to fetch the routing table from shard-manager-service");

        let shard_manager_routing_table: RoutingTable = match routing_table.into_inner() {
            shardmanager::v1::GetRoutingTableResponse {
                result:
                    Some(shardmanager::v1::get_routing_table_response::Result::Success(routing_table)),
            } => routing_table.into(),
            _ => panic!("Unable to fetch the routing table from shard-manager-service"),
        };

        self.benchmark_rpc_invocation(
            benchmark_context,
            context,
            &recorder,
            &shard_manager_routing_table,
            "golem:itrpc-exports/rpc-api.{process}",
            vec![Data::generate_list(2000).into_value_and_type()],
            "worker-process-invocation",
        )
        .await;
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        for worker_id in &context.worker_ids {
            benchmark_context
                .deps
                .delete_worker(&worker_id.parent)
                .await
                .expect("Failed to delete parent worker");
            benchmark_context
                .deps
                .delete_worker(&worker_id.child)
                .await
                .expect("Failed to delete child worker");
        }
    }
}

impl RpcLargeInput {
    async fn benchmark_rpc_invocation(
        &self,
        benchmark_context: &SimpleBenchmarkContext,
        context: &RpcBenchmarkIteratorContext,
        recorder: &BenchmarkRecorder,
        shard_manager_routing_table: &RoutingTable,
        function: &str,
        params: Vec<ValueAndType>,
        name: &str,
    ) {
        let mut fibers = JoinSet::new();
        for parent_child_worker_pair in context.worker_ids.iter().cloned() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = parent_child_worker_pair.parent.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let function_clone = function.to_string();
            let params_clone = params.clone();
            let name_clone = name.to_string();

            let same_executor =
                parent_child_worker_pair.at_same_worker_executor(shard_manager_routing_table);

            let _ = fibers.spawn(async move {
                for _ in 0..length {
                    let result = invoke_and_await(
                        &context_clone.deps,
                        &worker_id_clone,
                        &function_clone,
                        params_clone.clone(),
                    )
                    .await;

                    if same_executor {
                        recorder_clone.duration(
                            &format!("{name_clone}-local").into(),
                            result.accumulated_time,
                        );
                    } else {
                        recorder_clone.duration(
                            &format!("{name_clone}-remote").into(),
                            result.accumulated_time,
                        );
                    }
                }
            });
        }

        while let Some(fiber) = fibers.join_next().await {
            fiber.expect("fiber failed");
        }
    }
}

pub async fn run() {
    run_benchmark::<RpcLargeInput>().await;
}
