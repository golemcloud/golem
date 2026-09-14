// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::benchmarks::{cleanup_user_state, delete_workers};
use async_trait::async_trait;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use std::time::Instant;
use tracing::Level;

pub struct IdempotencyKeyLookup {
    config: RunConfig,
}

pub struct BenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct IterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    agent_id: ParsedAgentId,
    env_id: EnvironmentId,
}

#[async_trait]
impl Benchmark for IdempotencyKeyLookup {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "idempotency-key-lookup"
    }

    fn description() -> &'static str {
        indoc! {
            "Invokes one long-lived durable agent with `size` unique idempotency keys, then performs
            `length` duplicate lookups each for the newest and oldest keys. Records the latency and
            throughput of unique invocations, recent duplicates, and old duplicates separately."
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self::BenchmarkContext {
        BenchmarkContext {
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

    async fn cleanup(context: Self::BenchmarkContext) {
        context.deps.kill_all().await;
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> Self {
        Self { config }
    }

    async fn setup_iteration(&self, context: &Self::BenchmarkContext) -> Self::IterationContext {
        let user = context.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();
        let component = user
            .component(&env.id, "benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await
            .unwrap();

        IterationContext {
            user,
            component,
            agent_id: agent_id!("RustBenchmarkAgent", "idempotency-key-lookup"),
            env_id: env.id,
        }
    }

    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) {
        invoke(context, &IdempotencyKey::fresh()).await;
    }

    async fn run(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) {
        assert!(self.config.size > 0, "size must be at least one");

        let mut keys = Vec::with_capacity(self.config.size);
        for _ in 0..self.config.size {
            let key = IdempotencyKey::fresh();
            let started = Instant::now();
            invoke(context, &key).await;
            recorder.duration(&"unique-invocation".into(), started.elapsed());
            keys.push(key);
        }

        let old = keys.first().unwrap();
        let recent = keys.last().unwrap();
        for _ in 0..self.config.length {
            let started = Instant::now();
            invoke(context, recent).await;
            recorder.duration(&"recent-duplicate".into(), started.elapsed());
        }
        for _ in 0..self.config.length {
            let started = Instant::now();
            invoke(context, old).await;
            recorder.duration(&"old-duplicate".into(), started.elapsed());
        }
    }

    async fn cleanup_iteration(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    ) {
        if let Ok(agent_id) = AgentId::from_agent_id(context.component.id, &context.agent_id) {
            delete_workers(&context.user, &[agent_id]).await;
        }
        cleanup_user_state(&context.user, &context.env_id).await;
    }
}

async fn invoke(context: &IterationContext, key: &IdempotencyKey) {
    context
        .user
        .invoke_and_await_agent_with_key(
            &context.component,
            &context.agent_id,
            key,
            "echo",
            data_value!("benchmark"),
        )
        .await
        .expect("agent invocation failed");
}
