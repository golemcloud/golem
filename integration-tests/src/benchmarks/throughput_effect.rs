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

use crate::benchmarks::{cleanup_user_state, delete_workers, invoke_and_await_agent};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::model::AgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use tracing::Level;

pub struct ThroughputLargeInputEffect {
    config: RunConfig,
}

pub struct BenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct IterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    agent_ids: Vec<ParsedAgentId>,
    env_id: EnvironmentId,
}

#[async_trait]
impl Benchmark for ThroughputLargeInputEffect {
    type BenchmarkContext = BenchmarkContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "throughput-large-input-effect"
    }

    fn description() -> &'static str {
        "Spawns `size` Effect agents and calls largeInput 100 times per worker with a list of `length` zero bytes. Uses the throughput-large-input invocation timer and one warmup call per worker. Measures direct REST invocation, not HTTP mapping or RPC."
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(BenchmarkContext {
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
        })
    }

    async fn cleanup(context: Self::BenchmarkContext) -> BenchmarkResultValue {
        context.deps.kill_all().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self { config })
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        _recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        let user = benchmark_context.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();
        let component = user
            .component(&env.id, "benchmark_agent_effect")
            .name("benchmark:agent-effect")
            .store()
            .await
            .unwrap();
        let agent_ids = (0..self.config.size)
            .map(|n| agent_id!("EffectBenchmarkAgent", format!("test-{n}")))
            .collect();
        Ok(IterationContext {
            user,
            component,
            agent_ids,
            env_id: env.id,
        })
    }

    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        let calls = context
            .agent_ids
            .iter()
            .map(|id| {
                invoke_and_await_agent(
                    &context.user,
                    &context.component,
                    id,
                    "largeInput",
                    data_value!(vec![0u8; self.config.length]),
                )
            })
            .collect::<Vec<_>>();
        for result in calls.join().await {
            check_length(&result.value, self.config.length)?;
        }
        Ok(())
    }

    async fn run(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let calls = context
            .agent_ids
            .iter()
            .map(|id| async move {
                let mut results = vec![];
                for _ in 0..100 {
                    results.push(
                        invoke_and_await_agent(
                            &context.user,
                            &context.component,
                            id,
                            "largeInput",
                            data_value!(vec![0u8; self.config.length]),
                        )
                        .await,
                    );
                }
                results
            })
            .collect::<Vec<_>>();
        for (idx, results) in calls.join().await.iter().enumerate() {
            for result in results {
                result.record(&recorder, "effect-agent-", &idx.to_string());
                check_length(&result.value, self.config.length)?;
            }
        }
        Ok(())
    }

    async fn cleanup_iteration(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let ids: Vec<AgentId> = context
            .agent_ids
            .iter()
            .filter_map(|id| AgentId::from_agent_id(context.component.id, id).ok())
            .collect();
        delete_workers(&context.user, &ids, &recorder).await;
        cleanup_user_state(&context.user, &context.env_id, &recorder).await;
        Ok(())
    }
}

fn check_length(value: &[SchemaValue], expected: usize) -> BenchmarkResultValue {
    match value {
        [SchemaValue::U32(length)] if *length as usize == expected => Ok(()),
        _ => Err(BenchmarkError::new(
            "large-input-correctness",
            format!("expected U32 length {expected}, got {value:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn large_input_result_is_length_not_sum() {
        assert!(check_length(&[SchemaValue::U32(37)], 37).is_ok());
        assert!(check_length(&[SchemaValue::U32(0)], 37).is_err());
        assert!(check_length(&[SchemaValue::F64(37.0)], 37).is_err());
    }
}
