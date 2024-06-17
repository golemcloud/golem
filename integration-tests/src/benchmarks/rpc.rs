use std::collections::HashMap;
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
use async_trait::async_trait;
use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::GetRoutingTableRequest;
use golem_common::model::{RoutingTable, WorkerId};
use golem_wasm_rpc::Value;
use std::time::SystemTime;

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::dsl::TestDsl;
use integration_tests::benchmarks::data::Data;
use integration_tests::benchmarks::{
    cleanup_iteration, get_worker_ids, run_benchmark, setup_benchmark, setup_with, start,
    BenchmarkContext, IterationContext,
};

struct Rpc {
    config: RunConfig,
    params: CliParams,
}

#[async_trait]
impl Benchmark for Rpc {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "cold-start-large"
    }

    async fn create_benchmark_context(
        params: CliParams,
        cluster_size: usize,
    ) -> Self::BenchmarkContext {
        setup_benchmark(params, cluster_size).await
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) {
        benchmark_context.deps.kill_all()
    }

    async fn create(params: CliParams, config: RunConfig) -> Self {
        Self { config, params }
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext {
        let child_component_id = benchmark_context.deps.store_unique_component("child_component").await;
        let component_id = benchmark_context.deps.store_unique_component("parent_component_composed").await;

        // Rpc parent worker-id
        let parent_worker_id = WorkerId {
            component_id: component_id.clone(),
            worker_name: "parent_worker".to_string(),
        };

        let mut env = HashMap::new();

        env.insert("CHILD_COMPONENT_ID".to_string(), child_component_id.0.to_string());

        benchmark_context.deps
            .start_worker_with(&parent_worker_id.component_id, &parent_worker_id.worker_name, vec![], env)
            .await;

        IterationContext { worker_ids: vec![parent_worker_id] }
    }

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        if !self.params.mode.component_compilation_disabled() {
            // warmup with other workers
            if let Some(WorkerId { component_id, .. }) = context.worker_ids.clone().first() {
                start(
                    get_worker_ids(context.worker_ids.len(), component_id, "warmup-worker"),
                    benchmark_context.deps.clone(),
                )
                .await
            }
        }
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        // config.benchmark_config.length is not used, we want to have only one invocation per worker in this benchmark
        let calculate_iter: u64 = 200000;

        let data = Data::generate_list(2000);

        let values = data
            .clone()
            .into_iter()
            .map(|d| d.into())
            .collect::<Vec<Value>>();

        let shard_manager = benchmark_context.deps.shard_manager();

        let mut shard_manager_client = shard_manager.client().await;

        let routing_table = shard_manager_client
            .get_routing_table(GetRoutingTableRequest {})
            .await
            .expect("Unable to fetch the routing table");

        let shard_manager_routing_table: RoutingTable = match routing_table.into_inner() {
            shardmanager::GetRoutingTableResponse {
                result:
                    Some(shardmanager::get_routing_table_response::Result::Success(routing_table)),
            } => routing_table.into(),
            _ => panic!("Failed to fetch the routing table"),
        };

        let mut fibers = Vec::new();

        // For parent worker-id, we will have a child worker once the parent's function is invoked
        for worker_id in context.worker_ids.iter() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let rt_clone = shard_manager_routing_table.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..100 {
                    let start = SystemTime::now();
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:itrpc/api.{echo}",
                            vec![Value::String("hello".to_string())],
                        )
                        .await
                        .expect("invoke_and_await failed");

                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");

                    dbg!(rt_clone.lookup(&worker_id_clone));

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
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..3 {
                    let start = SystemTime::now();
                    let res = context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:itrpc/api.{calculate}",
                            vec![Value::U64(calculate_iter)],
                        )
                        .await
                        .expect("invoke_and_await failed");
                    println!("worker-calculate-res: {:?}", res[0]);
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
        for worker_id in context.worker_ids.iter() {
            let context_clone = benchmark_context.clone();
            let worker_id_clone = worker_id.clone();
            let recorder_clone = recorder.clone();
            let values_clone = values.clone();
            let length = self.config.length;
            let fiber = tokio::task::spawn(async move {
                for _ in 0..3 {
                    let start = SystemTime::now();
                    context_clone
                        .deps
                        .invoke_and_await(
                            &worker_id_clone,
                            "golem:itrpc/api.{process}",
                            vec![Value::List(values_clone.clone())],
                        )
                        .await
                        .expect("invoke_and_await failed");
                    let elapsed = start.elapsed().expect("SystemTime elapsed failed");
                    recorder_clone.duration(&"worker-process-invocation".to_string(), elapsed);
                }
            });
            fibers.push(fiber);
        }

        for fiber in fibers {
            fiber.await.expect("fiber failed");
        }
    }

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        cleanup_iteration(benchmark_context, context).await
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<Rpc>().await;
}
