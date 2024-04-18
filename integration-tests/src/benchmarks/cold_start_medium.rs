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

use golem_test_framework::config::{CliParams, TestDependencies};
use golem_test_framework::dsl::benchmark::{Benchmark, BenchmarkRecorder};
use integration_tests::{run_benchmark, run_echo, setup, Context};

struct ColdStartEchoMedium {
    config: CliParams,
}

#[async_trait]
impl Benchmark for ColdStartEchoMedium {
    type IterationContext = Context;

    fn name() -> &'static str {
        "cold-start-medium"
    }

    async fn create(config: CliParams) -> Self {
        Self { config }
    }

    async fn setup_iteration(&self) -> Self::IterationContext {
        setup(self.config.clone(), "js-1").await
    }

    async fn warmup(&self, _: &Self::IterationContext) {}

    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder) {
        run_echo(self.config.benchmark_config.length, context, recorder).await
    }

    async fn cleanup_iteration(&self, context: Self::IterationContext) {
        context.deps.kill_all();
    }
}

#[tokio::main]
async fn main() {
    run_benchmark::<ColdStartEchoMedium>().await;
}
