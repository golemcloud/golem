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

use crate::benchmarks::{InvokeResult, cleanup_user_state, delete_workers};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::model::AgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::time::{Duration, Instant};
use tracing::Level;

pub struct Streaming<const TOOL: bool> {
    config: RunConfig,
}

pub struct StreamingContext {
    deps: BenchmarkTestDependencies,
}

pub struct IterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    agent_ids: Vec<ParsedAgentId>,
    chunk_count: u32,
    env_id: EnvironmentId,
}

#[async_trait]
impl<const TOOL: bool> Benchmark for Streaming<TOOL> {
    type BenchmarkContext = StreamingContext;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        if TOOL {
            "streaming-tool"
        } else {
            "streaming-rpc"
        }
    }

    fn description() -> &'static str {
        if TOOL {
            "Warm streaming tool stdout: outer invocation and guest-observed first chunk. size is concurrent callers; length is 4 KiB chunks. No cold-load or recovery measurement."
        } else {
            "Warm ordinary agent-to-agent streaming RPC: outer invocation and guest-observed first chunk. size is concurrent callers; length is 4 KiB chunks. A single-executor cluster measures local RPC; no cold-load or recovery measurement."
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<StreamingContext> {
        Ok(StreamingContext {
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

    async fn cleanup(context: StreamingContext) -> BenchmarkResultValue {
        context.deps.kill_all().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self { config })
    }

    async fn setup_iteration(
        &self,
        context: &StreamingContext,
        _recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<IterationContext> {
        let user = context
            .deps
            .user()
            .await
            .map_err(|error| BenchmarkError::new("setup-user", error))?;
        let (_, env) = user
            .app_and_env()
            .await
            .map_err(|error| BenchmarkError::new("setup-environment", error))?;
        let (component, caller) = if TOOL {
            let component = user
                .component(&env.id, "golem_it_tool_streaming_rust_caller_release")
                .name("golem-it:tool-streaming-rust-caller")
                .store()
                .await
                .map_err(|error| BenchmarkError::new("setup-caller-component", error))?;
            user.component(&env.id, "golem_it_tool_streaming_rust_provider_release")
                .name("golem-it:tool-streaming-rust-provider")
                .with_tool_agent_binding("streaming", "ToolStreamingCaller")
                .map_err(|error| BenchmarkError::new("setup-tool-binding", error))?
                .store()
                .await
                .map_err(|error| BenchmarkError::new("setup-provider-component", error))?;
            (component, "ToolStreamingCaller")
        } else {
            let component = user
                .component(&env.id, "golem_it_agent_rpc_rust_release")
                .name("golem-it:agent-rpc-rust")
                .store()
                .await
                .map_err(|error| BenchmarkError::new("setup-rpc-component", error))?;
            (component, "StreamingRpcCaller")
        };
        let agent_ids = (0..self.config.size)
            .map(|index| agent_id!(caller, format!("streaming-{index}")))
            .collect();
        Ok(IterationContext {
            user,
            component,
            agent_ids,
            chunk_count: self.config.length.try_into().map_err(|error| {
                BenchmarkError::new("setup-parameters", format!("invalid chunk count: {error}"))
            })?,
            env_id: env.id,
        })
    }

    async fn warmup(
        &self,
        _context: &StreamingContext,
        iteration: &IterationContext,
    ) -> BenchmarkResultValue {
        let results = iteration
            .agent_ids
            .iter()
            .map(|agent_id| invoke_streaming_caller(iteration, agent_id))
            .collect::<Vec<_>>()
            .join()
            .await;

        for result in results {
            let result = result?;
            stream_result(&result.value, iteration.chunk_count)?;
        }
        Ok(())
    }

    async fn run(
        &self,
        _context: &StreamingContext,
        iteration: &IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let results = iteration
            .agent_ids
            .iter()
            .map(|agent_id| invoke_streaming_caller(iteration, agent_id))
            .collect::<Vec<_>>()
            .join()
            .await;

        for result in results {
            let result = result?;
            result.record(&recorder, "", Self::name());
            let (first, total) = stream_result(&result.value, iteration.chunk_count)?;
            recorder.duration(
                &ResultKey::primary("guest-time-to-first-chunk"),
                Duration::from_nanos(first),
            );
            recorder.duration(
                &ResultKey::secondary("guest-stream-total"),
                Duration::from_nanos(total),
            );
        }
        Ok(())
    }

    async fn cleanup_iteration(
        &self,
        _context: &StreamingContext,
        iteration: IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let ids = iteration
            .agent_ids
            .iter()
            .filter_map(|id| AgentId::from_agent_id(iteration.component.id, id).ok())
            .collect::<Vec<_>>();
        delete_workers(&iteration.user, &ids, &recorder).await;
        cleanup_user_state(&iteration.user, &iteration.env_id, &recorder).await;
        Ok(())
    }
}

async fn invoke_streaming_caller(
    iteration: &IterationContext,
    agent_id: &ParsedAgentId,
) -> BenchmarkResultValue<InvokeResult> {
    // Measure the whole request, without retrying or discarding timed-out intervals.
    let started = Instant::now();
    let value = tokio::time::timeout(
        Duration::from_secs(600),
        iteration.user.invoke_and_await_agent(
            &iteration.component,
            agent_id,
            "benchmark_producer",
            data_value!(iteration.chunk_count, 4096_u32),
        ),
    )
    .await
    .map_err(|_| {
        BenchmarkError::new(
            "stream-complete",
            "streaming benchmark exceeded its 10-minute deadline",
        )
    })?
    .map_err(|error| BenchmarkError::new("stream-complete", error))?;
    Ok(InvokeResult {
        accumulated_time: started.elapsed(),
        value: value
            .into_return_value()
            .map(|value| vec![value])
            .unwrap_or_default(),
        retries: 0,
        timeouts: 0,
        failures: vec![],
    })
}

fn stream_result(value: &[SchemaValue], expected_chunks: u32) -> BenchmarkResultValue<(u64, u64)> {
    let [SchemaValue::Record { fields }] = value else {
        return Err(BenchmarkError::new(
            "stream-correctness",
            format!("expected one benchmark result record, got {value:?}"),
        ));
    };
    let [
        SchemaValue::U64(first),
        SchemaValue::U64(total),
        SchemaValue::U32(chunks),
    ] = fields.as_slice()
    else {
        return Err(BenchmarkError::new(
            "stream-correctness",
            format!("unexpected benchmark result fields: {fields:?}"),
        ));
    };
    if *chunks != expected_chunks {
        return Err(BenchmarkError::new(
            "stream-correctness",
            format!("expected {expected_chunks} chunks, got {chunks}"),
        ));
    }
    if *first > *total {
        return Err(BenchmarkError::new(
            "stream-correctness",
            "first chunk cannot arrive after completion",
        ));
    }
    Ok((*first, *total))
}
