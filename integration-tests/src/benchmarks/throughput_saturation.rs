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

//! Throughput-under-memory-saturation benchmarks.
//!
//! Unlike the regular throughput benchmark — which keeps `size` small enough
//! that all workers fit comfortably in memory — these benchmarks deliberately
//! ramp the number of *active* agents up to and past the executor's memory
//! ceiling, to find the knee: the agent count where the pod can still keep
//! everything resident (latency flat, throughput scaling linearly) just before
//! it starts evicting and replaying (latency spikes, throughput craters).
//!
//! The measured `run` phase drives sustained load over a fixed window: each
//! agent repeatedly does a short unit of work then goes idle for [`IDLE_GAP`].
//! During that gap the agent has no in-flight work and becomes a `LoadedIdle`
//! eviction candidate, so under memory pressure it can be evicted and then must
//! reload (oplog replay + re-admission) on its next call — the churn that makes
//! throughput crater past the knee. Starts are staggered so the fleet is not
//! synchronised.
//!
//! Three variants:
//! - `throughput-saturation-counters`: agent-counters with a synthetic,
//!   per-agent-distinct retained footprint (`allocate_memory`) plus CPU work
//!   (`busy_for`). The footprint is controllable via `length`.
//! - `throughput-saturation-echo-rust` / `throughput-saturation-echo-ts`: the
//!   benchmark `echo` agent (Rust / TS) called repeatedly. No synthetic
//!   footprint — the per-agent memory is the agent's natural footprint, which
//!   for the TS agent includes the QuickJS runtime. Answers "how many actively
//!   invoked echo agents fit per pod".
//!
//! Parameters:
//! - `size`   = number of active agents in this step (the ramp axis).
//! - `length` = for the counters variant, the base per-agent memory footprint in
//!   bytes (agent `i` retains a deterministic multiple); ignored by the echo
//!   variants.

use crate::benchmarks::{cleanup_user_state, delete_workers, invoke_and_await_agent};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::base_model::agent::{DataValue, LegacyParsedAgentId};
use golem_common::model::AgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{Benchmark, BenchmarkRecorder, ResultKey, RunConfig};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use std::time::{Duration, Instant};
use tracing::{Instrument, Level, info};

/// Number of distinct footprint buckets the synthetic per-agent memory spread
/// cycles through, so the fleet holds a mix of sizes rather than a uniform
/// amount.
const SPREAD_BUCKETS: usize = 8;

/// CPU busy time (ms) per `busy_for` invocation (counters variant only).
const BUSY_MILLIS: u32 = 50;

/// Idle gap each agent sleeps between calls. During this gap the agent has no
/// in-flight work and becomes a `LoadedIdle` eviction candidate. Under memory
/// pressure it may be evicted and then must reload on its next call — the churn
/// this benchmark exists to measure.
const IDLE_GAP: Duration = Duration::from_millis(200);

/// Total measured wall-clock duration of the sustained-load phase. Throughput
/// and churn are measured over this fixed window so steps with different `size`
/// are comparable. Held long enough that the high-residency plateau persists for
/// at least a minute, so steady-state behaviour at the memory ceiling (not just
/// the initial burst) is observed.
const RUN_DURATION: Duration = Duration::from_secs(90);

/// Maximum per-agent start stagger, so the fleet is not synchronised: at any
/// instant some agents are mid-call (demanding memory) while others sit idle
/// (evictable).
const MAX_STAGGER: Duration = Duration::from_millis(250);

/// Resident memory (bytes) the synthetic-footprint agent `index` retains for a
/// given `base`. Spreads deterministically across [`SPREAD_BUCKETS`] buckets
/// (`base * 1` .. `base * SPREAD_BUCKETS`) so different agents hold different
/// amounts and some sit much closer to the limit than others.
fn agent_memory_bytes(index: usize, base: usize) -> u32 {
    let bucket = (index % SPREAD_BUCKETS) + 1;
    (base.saturating_mul(bucket)).min(u32::MAX as usize) as u32
}

/// Per-agent start offset derived deterministically from the index, spread
/// across `[0, MAX_STAGGER)`.
fn agent_stagger(index: usize) -> Duration {
    let frac = (index as u32).wrapping_mul(2_654_435_761) % 1000;
    MAX_STAGGER.checked_mul(frac).unwrap_or_default() / 1000
}

/// Describes one saturation variant: which component to load, which agent type
/// and method to actively invoke, and whether to pre-load a synthetic footprint.
struct SaturationVariant {
    /// WASM file name (without `.wasm`) in the component directory.
    wasm_name: &'static str,
    /// Registry display name for the component.
    component_name: &'static str,
    /// Agent type to instantiate.
    agent_type: &'static str,
    /// Method invoked repeatedly during the measured phase.
    active_method: &'static str,
    /// Builds the parameter for one `active_method` call.
    active_params: fn() -> DataValue,
    /// When set, each agent calls this method once in warmup with its
    /// deterministic footprint (`allocate_memory`-style). `None` for the echo
    /// variants, whose footprint is the agent's natural memory.
    allocate_method: Option<&'static str>,
}

const COUNTERS_VARIANT: SaturationVariant = SaturationVariant {
    wasm_name: "it_agent_counters_release",
    component_name: "it:agent-counters",
    agent_type: "Counter",
    active_method: "busy_for",
    active_params: || data_value!(BUSY_MILLIS),
    allocate_method: Some("allocate_memory"),
};

const ECHO_RUST_VARIANT: SaturationVariant = SaturationVariant {
    wasm_name: "benchmark_agent_rust_release",
    component_name: "benchmark:agent-rust",
    agent_type: "RustBenchmarkAgent",
    active_method: "echo",
    active_params: || data_value!("saturation"),
    allocate_method: None,
};

const ECHO_TS_VARIANT: SaturationVariant = SaturationVariant {
    wasm_name: "benchmark_agent_ts",
    component_name: "benchmark:agent-ts",
    agent_type: "BenchmarkAgent",
    active_method: "echo",
    active_params: || data_value!("saturation"),
    allocate_method: None,
};

pub struct SaturationBenchmarkContext {
    deps: BenchmarkTestDependencies,
}

pub struct SaturationIterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    agent_ids: Vec<LegacyParsedAgentId>,
    base_memory_bytes: usize,
    env_id: EnvironmentId,
}

/// Shared implementation for all saturation variants. The variant-specific
/// config is supplied by the wrapper types' `variant()`.
async fn create_context(
    mode: &TestMode,
    verbosity: Level,
    cluster_size: usize,
    disable_compilation_cache: bool,
    otlp: bool,
) -> SaturationBenchmarkContext {
    SaturationBenchmarkContext {
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

async fn setup_iteration(
    variant: &SaturationVariant,
    config: &RunConfig,
    benchmark_context: &SaturationBenchmarkContext,
) -> SaturationIterationContext {
    let user = benchmark_context.deps.user().await.unwrap();
    let (_, env) = user.app_and_env().await.unwrap();

    info!("Registering component {}", variant.component_name);
    let component = user
        .component(&env.id, variant.wasm_name)
        .name(variant.component_name)
        .store()
        .await
        .unwrap();

    let mut agent_ids = vec![];
    for n in 0..config.size {
        agent_ids.push(agent_id!(variant.agent_type, format!("saturation-{n}")));
    }

    SaturationIterationContext {
        user,
        component,
        agent_ids,
        base_memory_bytes: config.length,
        env_id: env.id,
    }
}

async fn warmup(variant: &SaturationVariant, context: &SaturationIterationContext) {
    let Some(allocate_method) = variant.allocate_method else {
        // Echo variants: nothing to pre-load; the agent's natural footprint is
        // established on first invocation.
        return;
    };

    async {
        let base = context.base_memory_bytes;
        let result_futures = context
            .agent_ids
            .iter()
            .enumerate()
            .map(move |(idx, agent_id)| async move {
                let user_clone = context.user.clone();
                let bytes = agent_memory_bytes(idx, base);
                invoke_and_await_agent(
                    &user_clone,
                    &context.component,
                    agent_id,
                    allocate_method,
                    data_value!(bytes),
                )
                .await
            })
            .collect::<Vec<_>>();
        let _ = result_futures.join().await;
    }
    .instrument(tracing::info_span!(
        "warmup_allocate_memory",
        agent_count = context.agent_ids.len()
    ))
    .await;
}

async fn run(
    variant: &SaturationVariant,
    context: &SaturationIterationContext,
    recorder: BenchmarkRecorder,
) {
    let agent_count = context.agent_ids.len();
    let deadline = Instant::now() + RUN_DURATION;

    let result_futures = context
        .agent_ids
        .iter()
        .enumerate()
        .map(|(idx, agent_id)| {
            let recorder = recorder.clone();
            async move {
                let user_clone = context.user.clone();

                tokio::time::sleep(agent_stagger(idx)).await;

                let mut calls = 0u64;
                while Instant::now() < deadline {
                    let result = invoke_and_await_agent(
                        &user_clone,
                        &context.component,
                        agent_id,
                        variant.active_method,
                        (variant.active_params)(),
                    )
                    .await;
                    result.record(&recorder, "", idx.to_string().as_str());
                    calls += 1;
                    tokio::time::sleep(IDLE_GAP).await;
                }
                calls
            }
        })
        .collect::<Vec<_>>();

    let started = Instant::now();
    let per_agent_calls = result_futures.join().await;
    let elapsed = started.elapsed();

    // Aggregate sustained throughput over the fixed run window. Across `size`
    // steps, this reveals where added active agents stop adding throughput
    // (memory saturation / eviction churn dominates) — the knee we are after.
    let total_calls: u64 = per_agent_calls.iter().sum();
    let secs = elapsed.as_secs_f64();
    if secs > 0.0 {
        let ops_per_sec = (total_calls as f64 / secs).round() as u64;
        info!(
            "saturation: {agent_count} agents, {total_calls} calls in {secs:.1}s = {ops_per_sec} ops/sec"
        );
        recorder.count(
            &ResultKey::primary("saturation-throughput-ops-per-sec"),
            ops_per_sec,
        );
    }
}

async fn cleanup_iteration(context: SaturationIterationContext) {
    let agent_ids: Vec<AgentId> = context
        .agent_ids
        .iter()
        .filter_map(|agent_id| AgentId::from_agent_id(context.component.id, agent_id).ok())
        .collect();
    delete_workers(&context.user, &agent_ids).await;
    cleanup_user_state(&context.user, &context.env_id).await;
}

/// Generates a `Benchmark` impl wrapper for a saturation variant.
macro_rules! saturation_benchmark {
    ($ty:ident, $bench_name:literal, $variant:expr, $description:literal) => {
        pub struct $ty {
            config: RunConfig,
        }

        #[async_trait]
        impl Benchmark for $ty {
            type BenchmarkContext = SaturationBenchmarkContext;
            type IterationContext = SaturationIterationContext;

            fn name() -> &'static str {
                $bench_name
            }

            fn description() -> &'static str {
                indoc! { $description }
            }

            async fn create_benchmark_context(
                mode: &TestMode,
                verbosity: Level,
                cluster_size: usize,
                disable_compilation_cache: bool,
                otlp: bool,
            ) -> Self::BenchmarkContext {
                create_context(
                    mode,
                    verbosity,
                    cluster_size,
                    disable_compilation_cache,
                    otlp,
                )
                .await
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
                setup_iteration(&$variant, &self.config, benchmark_context).await
            }

            async fn warmup(
                &self,
                _benchmark_context: &Self::BenchmarkContext,
                context: &Self::IterationContext,
            ) {
                warmup(&$variant, context).await
            }

            async fn run(
                &self,
                _benchmark_context: &Self::BenchmarkContext,
                context: &Self::IterationContext,
                recorder: BenchmarkRecorder,
            ) {
                run(&$variant, context, recorder).await
            }

            async fn cleanup_iteration(
                &self,
                _benchmark_context: &Self::BenchmarkContext,
                context: Self::IterationContext,
            ) {
                cleanup_iteration(context).await
            }
        }
    };
}

saturation_benchmark!(
    ThroughputSaturationCounters,
    "throughput-saturation-counters",
    COUNTERS_VARIANT,
    "Ramps `size` active agents that each retain a deterministic, per-agent-distinct
    synthetic memory footprint (controlled by `length`) and do CPU work, measuring
    sustained throughput to locate the memory-saturation knee."
);

saturation_benchmark!(
    ThroughputSaturationEchoRust,
    "throughput-saturation-echo-rust",
    ECHO_RUST_VARIANT,
    "Ramps `size` actively-invoked Rust `echo` agents to find how many fit resident
    per pod before eviction churn craters throughput. The per-agent footprint is the
    agent's natural memory (no synthetic allocation)."
);

saturation_benchmark!(
    ThroughputSaturationEchoTs,
    "throughput-saturation-echo-ts",
    ECHO_TS_VARIANT,
    "Ramps `size` actively-invoked TypeScript `echo` agents to find how many fit
    resident per pod before eviction churn craters throughput. The per-agent
    footprint is the agent's natural memory, including the QuickJS runtime."
);
