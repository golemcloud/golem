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
use golem_common::model::WorkerId;

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder};
use integration_tests::benchmarks::{
    get_worker_ids, run_benchmark, run_echo, setup, start, Context,
};

struct ColdStartEchoLarge {
    config: CliParams,
}

#[async_trait]
impl Benchmark for ColdStartEchoLarge {
    type IterationContext = Context;

    fn name() -> &'static str {
        "cold-start-large"
    }

    async fn create(config: CliParams) -> Self {
        Self { config }
    }

    async fn setup_iteration(&self) -> Self::IterationContext {
        setup(self.config.clone(), "py-echo", false).await
    }

    async fn warmup(&self, context: &Self::IterationContext) {
        // warmup with other workers
        if let Some(WorkerId { component_id, .. }) = context.worker_ids.clone().first() {
            start(
                get_worker_ids(context.worker_ids.len(), component_id, "warmup-worker"),
                context.deps.clone(),
            )
            .await
        }
    }

    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder) {
        // config.benchmark_config.length is not used, we want to have only one invocation per worker in this benchmark
        run_echo(1, context, recorder).await
    }

    async fn cleanup_iteration(&self, context: Self::IterationContext) {
        context.deps.kill_all();
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<ColdStartEchoLarge>().await;
}
