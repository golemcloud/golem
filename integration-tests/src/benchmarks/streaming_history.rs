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

use crate::benchmarks::public_invocation::{PublicInvocationSession, SessionCheckpoint};
use crate::benchmarks::{cleanup_user_state, delete_workers};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use golem_common::data_value;
use golem_common::model::AgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::SchemaValue;
use golem_test_framework::benchmark::storage_metrics::{StorageMetricsClient, StorageSnapshot};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tracing::Level;

const PHASE_DEADLINE: Duration = Duration::from_secs(120);
const METRICS_DEADLINE: Duration = Duration::from_secs(10);
const MAX_BOUNDED_SESSION_INDEX_READS: u64 = 128;
const MEASURED_DOMAIN: u32 = 700_000;

pub struct StreamingRpcHistory {
    config: RunConfig,
}

pub struct StreamingRpcColdIndexed {
    inner: StreamingRpcCold,
}

struct StreamingRpcCold {
    config: RunConfig,
}

pub struct StreamingHistoryContext {
    deps: BenchmarkTestDependencies,
}

pub struct StreamingHistoryIteration {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    caller: ParsedAgentId,
    measured_agents: Vec<AgentId>,
    environment_id: EnvironmentId,
    length: u32,
    storage_before: StorageSnapshot,
}

pub struct StreamingColdContext {
    deps: BenchmarkTestDependencies,
}

pub struct StreamingColdIteration {
    user: TestUserContext<BenchmarkTestDependencies>,
    application: String,
    environment: String,
    target: ParsedAgentId,
    measured_agents: Vec<AgentId>,
    environment_id: EnvironmentId,
    length: u32,
    post_restart: StorageSnapshot,
}

fn benchmark_error(phase: &str, error: impl std::fmt::Display) -> BenchmarkError {
    BenchmarkError::new(phase, error)
}

fn spawned_only(mode: &TestMode, name: &str) -> BenchmarkResultValue {
    if matches!(mode, TestMode::Spawned { .. }) {
        Ok(())
    } else {
        Err(BenchmarkError::new(
            "create",
            format!("{name} requires spawned PostgreSQL benchmark mode"),
        ))
    }
}

async fn create_context(
    mode: &TestMode,
    verbosity: Level,
    cluster_size: usize,
    disable_compilation_cache: bool,
    otlp: bool,
) -> BenchmarkResultValue<BenchmarkTestDependencies> {
    Ok(BenchmarkTestDependencies::new(
        mode,
        verbosity,
        cluster_size,
        disable_compilation_cache,
        otlp,
    )
    .await)
}

async fn metrics_snapshot(
    deps: &BenchmarkTestDependencies,
    phase: &str,
) -> BenchmarkResultValue<StorageSnapshot> {
    let client = StorageMetricsClient::new(METRICS_DEADLINE)
        .map_err(|error| benchmark_error(phase, error))?;
    client
        .snapshot(deps.worker_executor_cluster().as_ref())
        .await
        .map_err(|error| benchmark_error(phase, error))
}

fn storage_counts(
    phase: &str,
    before: &StorageSnapshot,
    after: &StorageSnapshot,
    recorder: &BenchmarkRecorder,
) -> BenchmarkResultValue {
    let delta = after
        .delta(Some(before))
        .map_err(|error| benchmark_error(&format!("storage-{phase}"), error))?;
    let mut coarse = BTreeMap::<(&str, &str), u64>::new();
    for (operation, count) in &delta {
        *coarse
            .entry((operation.kind.as_str(), operation.operation.as_str()))
            .or_default() += count;
        if operation.service == "stream_session_index" {
            recorder.count(
                &ResultKey::secondary(format!(
                    "storage-{phase}-{}-{}-{}-{}",
                    operation.kind, operation.operation, operation.api, operation.entity
                )),
                *count,
            );
        }
    }
    for (kind, operation) in [
        ("keyvalue", "get"),
        ("keyvalue", "get_many"),
        ("keyvalue", "set_many"),
        ("keyvalue", "compare_and_set_many"),
        ("indexed", "read"),
    ] {
        recorder.count(
            &ResultKey::primary(format!("storage-{phase}-{kind}-{operation}")),
            coarse.get(&(kind, operation)).copied().unwrap_or(0),
        );
    }
    Ok(())
}

fn ensure_stream_index_reads_are_bounded(
    before: &StorageSnapshot,
    after: &StorageSnapshot,
    maximum: u64,
) -> anyhow::Result<()> {
    let calls = after
        .delta(Some(before))?
        .into_iter()
        .filter(|(operation, _)| {
            operation.service == "stream_session_index"
                && matches!(
                    operation.operation.as_str(),
                    "get" | "get_many" | "read" | "first" | "last" | "scan"
                )
        })
        .try_fold(0u64, |total, (_, count)| {
            total.checked_add(count).context("storage count overflow")
        })?;
    ensure!(
        calls <= maximum,
        "stream-session index lookup used {calls} logical storage calls; maximum is {maximum}"
    );
    Ok(())
}

async fn component_and_environment(
    deps: &BenchmarkTestDependencies,
) -> anyhow::Result<(
    TestUserContext<BenchmarkTestDependencies>,
    ComponentDto,
    EnvironmentId,
    String,
    String,
)> {
    let user = deps.user().await?;
    let (application, environment) = user.app_and_env().await?;
    let component = user
        .component(&environment.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    Ok((
        user,
        component,
        environment.id,
        application.name.0,
        environment.name.0,
    ))
}

async fn invoke_caller(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    caller: &ParsedAgentId,
    method: &str,
    input: golem_common::schema::TypedSchemaValue,
) -> anyhow::Result<Vec<SchemaValue>> {
    tokio::time::timeout(
        PHASE_DEADLINE,
        user.invoke_and_await_agent(component, caller, method, input),
    )
    .await
    .with_context(|| format!("{method} deadline"))??
    .into_return_value()
    .map(|value| vec![value])
    .map_or_else(|| Ok(Vec::new()), Ok)
}

fn streaming_caller_result(
    value: &[SchemaValue],
    expected_chunks: u32,
) -> anyhow::Result<(Duration, Duration)> {
    let [SchemaValue::Record { fields }] = value else {
        anyhow::bail!("expected one streaming benchmark result record, got {value:?}");
    };
    let [
        SchemaValue::U64(first),
        SchemaValue::U64(total),
        SchemaValue::U32(chunks),
    ] = fields.as_slice()
    else {
        anyhow::bail!("unexpected streaming benchmark fields: {fields:?}");
    };
    ensure!(*chunks == expected_chunks, "guest chunk count changed");
    ensure!(*first <= *total, "first chunk arrived after completion");
    Ok((Duration::from_nanos(*first), Duration::from_nanos(*total)))
}

async fn seed_direct_session(
    deps: &BenchmarkTestDependencies,
    user: &TestUserContext<BenchmarkTestDependencies>,
    application: &str,
    environment: &str,
    target: &ParsedAgentId,
    domain: u32,
) -> anyhow::Result<()> {
    let report = PublicInvocationSession::start(
        deps,
        &user.token,
        application,
        environment,
        target,
        "benchmark_output",
        serde_json::json!({ "length": 1, "domain": domain }),
        PHASE_DEADLINE,
    )
    .await?
    .finish()
    .await?;
    validate_direct_output(&report, 1, domain)?;
    Ok(())
}

fn validate_direct_output(
    report: &SessionCheckpoint,
    length: u32,
    domain: u32,
) -> anyhow::Result<()> {
    report.ensure_success()?;
    ensure!(report.outputs.len() == 1, "expected exactly one output");
    let output = report.outputs.values().next().context("missing output")?;
    ensure!(
        output.items.len() == length as usize,
        "output item count changed"
    );
    let values = output
        .items
        .iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let expected = (0..length)
        .map(|index| serde_json::json!(domain + index * 3))
        .collect::<Vec<_>>();
    ensure!(
        values == expected.iter().collect::<Vec<_>>(),
        "deterministic stream values changed"
    );

    for (sequence, (actual, _)) in output.items.iter().enumerate() {
        ensure!(
            *actual == sequence as u64,
            "output producer sequence changed"
        );
    }
    let Some((
        sequence,
        golem_common::model::invocation_session_public::PublicOutputStreamOutcome::Ok,
    )) = &output.terminal
    else {
        anyhow::bail!("expected exactly one successful output terminal");
    };
    ensure!(
        *sequence == u64::from(length),
        "terminal producer sequence changed"
    );
    Ok(())
}

fn record_session_timings(
    report: &SessionCheckpoint,
    recorder: &BenchmarkRecorder,
) -> anyhow::Result<()> {
    let events = report.attempts.first().context("missing session events")?;
    let accepted = events.accepted.context("missing acceptance timestamp")?;
    let result = events.result.context("missing result timestamp")?;
    let first = events.first_item.context("missing first-item timestamp")?;
    let completed = events.completed.context("missing completion timestamp")?;
    ensure!(events.sent <= accepted, "acceptance preceded request");
    ensure!(accepted <= result, "result preceded acceptance");
    ensure!(result <= first, "first item preceded stream mapping result");
    ensure!(first <= completed, "completion preceded first item");
    recorder.duration(
        &ResultKey::primary("session-accepted"),
        accepted.duration_since(events.sent),
    );
    recorder.duration(
        &ResultKey::primary("result-mapped"),
        result.duration_since(accepted),
    );
    recorder.duration(
        &ResultKey::primary("first-item"),
        first.duration_since(events.sent),
    );
    recorder.duration(
        &ResultKey::primary("stream-complete"),
        completed.duration_since(events.sent),
    );
    Ok(())
}

async fn stop_executors(
    deps: &BenchmarkTestDependencies,
    recorder: Option<&BenchmarkRecorder>,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let cluster = deps.worker_executor_cluster();
    cluster
        .kill_all_and_wait(tokio::time::Instant::now() + PHASE_DEADLINE)
        .await?;
    ensure!(cluster.all_reaped(), "executor exit was not confirmed");
    if let Some(recorder) = recorder {
        recorder.duration(&ResultKey::primary("executor-stop"), started.elapsed());
    }
    Ok(())
}

async fn restart_executors(
    deps: &BenchmarkTestDependencies,
    recorder: Option<&BenchmarkRecorder>,
) -> anyhow::Result<()> {
    let grpc_started = Instant::now();
    tokio::time::timeout(PHASE_DEADLINE, deps.worker_executor_cluster().restart_all())
        .await
        .context("executor gRPC readiness deadline")?;
    if let Some(recorder) = recorder {
        recorder.duration(
            &ResultKey::primary("executor-grpc-ready"),
            grpc_started.elapsed(),
        );
    }

    let routable_started = Instant::now();
    tokio::time::timeout(PHASE_DEADLINE, async {
        loop {
            let table = deps.shard_manager().get_routing_table().await?;
            let executors = deps.worker_executor_cluster().to_vec();
            if table.all().len() == executors.len()
                && executors.iter().all(|executor| {
                    table
                        .all()
                        .iter()
                        .any(|pod| pod.port == executor.grpc_port())
                })
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("executor routability deadline")??;
    if let Some(recorder) = recorder {
        recorder.duration(
            &ResultKey::primary("executor-routable"),
            routable_started.elapsed(),
        );
    }
    Ok(())
}

#[async_trait]
impl Benchmark for StreamingRpcHistory {
    type BenchmarkContext = StreamingHistoryContext;
    type IterationContext = StreamingHistoryIteration;

    fn name() -> &'static str {
        "streaming-rpc-history"
    }

    fn description() -> &'static str {
        indoc! {
            "Measures a warm streaming RPC after creating completed session history on the same
            caller and producer agents. The `size` parameter is the number of completed one-chunk
            sessions created before the measurement. The `length` parameter is the number of 4 KiB
            chunks in the measured stream.

            The benchmark records end-to-end latency of the outer caller invocation and
            guest-observed times from issuing the producer RPC to the first chunk and to stream
            completion. It also records seeded-session, stream-item and terminal counts, plus
            coarse executor-wide logical storage-operation counts around the measured invocation.
            History creation is excluded from the invocation and guest timings."
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        spawned_only(mode, Self::name())?;
        Ok(StreamingHistoryContext {
            deps: create_context(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await?,
        })
    }

    async fn cleanup(context: Self::BenchmarkContext) -> BenchmarkResultValue {
        context.deps.kill_all().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        u32::try_from(config.length)
            .map_err(|error| benchmark_error("create", error))
            .and_then(|length| {
                if length == 0 {
                    Err(BenchmarkError::new("create", "length must be positive"))
                } else {
                    Ok(())
                }
            })?;
        Ok(Self { config })
    }

    async fn setup_iteration(
        &self,
        context: &Self::BenchmarkContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        let (user, component, environment_id, _, _) = component_and_environment(&context.deps)
            .await
            .map_err(|error| benchmark_error("setup", error))?;
        let warm = golem_common::agent_id!(
            "StreamingRpcCaller",
            format!("warm-{}", uuid::Uuid::new_v4())
        );
        let warm_result = invoke_caller(
            &user,
            &component,
            &warm,
            "benchmark_producer",
            data_value!(1u32, 4096u32),
        )
        .await
        .map_err(|error| benchmark_error("setup-warm-cache", error))?;
        streaming_caller_result(&warm_result, 1)
            .map_err(|error| benchmark_error("setup-warm-cache-correctness", error))?;

        let name = format!("history-{}", uuid::Uuid::new_v4());
        let caller = golem_common::agent_id!("StreamingRpcCaller", name.clone());
        let target = golem_common::agent_id!("StreamingRpcTarget", name);
        let initialized = invoke_caller(
            &user,
            &component,
            &caller,
            "call_stream_free",
            data_value!(),
        )
        .await
        .map_err(|error| benchmark_error("setup-initialize", error))?;
        if initialized != vec![SchemaValue::U64(1)] {
            return Err(BenchmarkError::new(
                "setup-initialize-correctness",
                "non-streaming initialization changed",
            ));
        }

        for _ in 0..self.config.size {
            let seeded = invoke_caller(
                &user,
                &component,
                &caller,
                "benchmark_producer",
                data_value!(1u32, 4096u32),
            )
            .await
            .map_err(|error| benchmark_error("setup-seed", error))?;
            streaming_caller_result(&seeded, 1)
                .map_err(|error| benchmark_error("setup-seed-correctness", error))?;
        }
        recorder.count(
            &ResultKey::primary("seeded-sessions"),
            self.config.size as u64,
        );
        let storage_before = metrics_snapshot(&context.deps, "setup-storage-snapshot").await?;
        let measured_agents = [&caller, &target]
            .into_iter()
            .map(|agent| AgentId::from_agent_id(component.id, agent).map_err(anyhow::Error::msg))
            .collect::<anyhow::Result<Vec<_>>>()
            .map_err(|error| benchmark_error("setup-agent-id", error))?;
        Ok(StreamingHistoryIteration {
            user,
            component,
            caller,
            measured_agents,
            environment_id,
            length: self.config.length as u32,
            storage_before,
        })
    }

    async fn warmup(
        &self,
        _context: &Self::BenchmarkContext,
        _iteration: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        Ok(())
    }

    async fn run(
        &self,
        context: &Self::BenchmarkContext,
        iteration: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let started = Instant::now();
        let measured = invoke_caller(
            &iteration.user,
            &iteration.component,
            &iteration.caller,
            "benchmark_producer",
            data_value!(iteration.length, 4096u32),
        )
        .await
        .map_err(|error| benchmark_error("stream-complete", error))?;
        recorder.duration(&ResultKey::primary("invocation"), started.elapsed());
        let (first, total) = streaming_caller_result(&measured, iteration.length)
            .map_err(|error| benchmark_error("correctness", error))?;
        recorder.duration(&ResultKey::primary("guest-time-to-first-chunk"), first);
        recorder.duration(&ResultKey::secondary("guest-stream-total"), total);
        recorder.count(
            &ResultKey::primary("stream-items"),
            u64::from(iteration.length),
        );
        recorder.count(&ResultKey::primary("stream-terminals"), 1);
        let storage_after = metrics_snapshot(&context.deps, "storage-measured").await?;
        storage_counts(
            "measured",
            &iteration.storage_before,
            &storage_after,
            &recorder,
        )
    }

    async fn cleanup_iteration(
        &self,
        _context: &Self::BenchmarkContext,
        iteration: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        delete_workers(&iteration.user, &iteration.measured_agents, &recorder).await;
        cleanup_user_state(&iteration.user, &iteration.environment_id, &recorder).await;
        Ok(())
    }
}

macro_rules! cold_benchmark {
    ($type:ty, $name:literal, $description:expr) => {
        #[async_trait]
        impl Benchmark for $type {
            type BenchmarkContext = StreamingColdContext;
            type IterationContext = StreamingColdIteration;

            fn name() -> &'static str {
                $name
            }

            fn description() -> &'static str {
                $description
            }

            async fn create_benchmark_context(
                mode: &TestMode,
                verbosity: Level,
                cluster_size: usize,
                disable_compilation_cache: bool,
                otlp: bool,
            ) -> BenchmarkResultValue<Self::BenchmarkContext> {
                spawned_only(mode, Self::name())?;
                Ok(StreamingColdContext {
                    deps: create_context(
                        mode,
                        verbosity,
                        cluster_size,
                        disable_compilation_cache,
                        otlp,
                    )
                    .await?,
                })
            }

            async fn cleanup(context: Self::BenchmarkContext) -> BenchmarkResultValue {
                context.deps.kill_all().await;
                Ok(())
            }

            async fn create(mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
                spawned_only(mode, Self::name())?;
                let length = u32::try_from(config.length)
                    .map_err(|error| benchmark_error("create", error))?;
                if length == 0 {
                    return Err(BenchmarkError::new("create", "length must be positive"));
                }
                Ok(Self {
                    inner: StreamingRpcCold { config },
                })
            }

            async fn setup_iteration(
                &self,
                context: &Self::BenchmarkContext,
                recorder: BenchmarkRecorder,
            ) -> BenchmarkResultValue<Self::IterationContext> {
                self.inner.setup_iteration(context, recorder).await
            }

            async fn warmup(
                &self,
                _context: &Self::BenchmarkContext,
                _iteration: &Self::IterationContext,
            ) -> BenchmarkResultValue {
                Ok(())
            }

            async fn run(
                &self,
                context: &Self::BenchmarkContext,
                iteration: &Self::IterationContext,
                recorder: BenchmarkRecorder,
            ) -> BenchmarkResultValue {
                self.inner.run(context, iteration, recorder).await
            }

            async fn cleanup_iteration(
                &self,
                _context: &Self::BenchmarkContext,
                iteration: Self::IterationContext,
                recorder: BenchmarkRecorder,
            ) -> BenchmarkResultValue {
                delete_workers(&iteration.user, &iteration.measured_agents, &recorder).await;
                cleanup_user_state(&iteration.user, &iteration.environment_id, &recorder).await;
                Ok(())
            }
        }
    };
}

cold_benchmark!(
    StreamingRpcColdIndexed,
    "streaming-rpc-cold-indexed",
    indoc! {
        "Measures the first new public WebSocket client-to-producer streaming invocation, without
        a guest caller, after restarting all executors while retaining the target agent's persisted stream-session
        index. The `size` parameter is the number of completed sessions created on the target before
        the restart. The `length` parameter is the number of deterministic u32 items produced by the
        measured stream.

        The benchmark separately times executor shutdown, restart to gRPC readiness, and the
        subsequent wait for routing-table readiness. It records client-observed request-to-acceptance,
        acceptance-to-result, request-to-first-item and request-to-completion latencies, plus coarse
        executor-wide logical storage-operation counts. These are protocol milestones, not isolated
        reconstruction or replay timings; acceptance alone does not establish that replay has completed."
    }
);

impl StreamingRpcCold {
    async fn setup_iteration(
        &self,
        context: &StreamingColdContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<StreamingColdIteration> {
        let (user, component, environment_id, application, environment) =
            component_and_environment(&context.deps)
                .await
                .map_err(|error| benchmark_error("setup", error))?;
        let warm = golem_common::agent_id!(
            "StreamingRpcTarget",
            format!("warm-{}", uuid::Uuid::new_v4())
        );
        seed_direct_session(
            &context.deps,
            &user,
            &application,
            &environment,
            &warm,
            900_000,
        )
        .await
        .map_err(|error| benchmark_error("setup-warm-cache", error))?;

        let target = golem_common::agent_id!(
            "StreamingRpcTarget",
            format!("cold-{}", uuid::Uuid::new_v4())
        );
        let ping = tokio::time::timeout(
            PHASE_DEADLINE,
            user.invoke_and_await_agent(&component, &target, "ping", data_value!()),
        )
        .await
        .context("target initialization deadline")
        .and_then(|result| result)
        .and_then(|value| value.into_typed::<u64>())
        .map_err(|error| benchmark_error("setup-initialize", error))?;
        if ping != 42 {
            return Err(BenchmarkError::new(
                "setup-initialize-correctness",
                "target ping changed",
            ));
        }

        let agent_id = AgentId::from_agent_id(component.id, &target)
            .map_err(|error| benchmark_error("setup-agent-id", error))?;
        for index in 0..self.config.size {
            seed_direct_session(
                &context.deps,
                &user,
                &application,
                &environment,
                &target,
                10_000
                    + u32::try_from(index).map_err(|error| benchmark_error("setup-seed", error))?
                        * 10,
            )
            .await
            .map_err(|error| benchmark_error("setup-seed", error))?;
        }
        recorder.count(
            &ResultKey::primary("seeded-sessions"),
            self.config.size as u64,
        );
        let storage_before = metrics_snapshot(&context.deps, "setup-storage-snapshot").await?;
        stop_executors(&context.deps, Some(&recorder))
            .await
            .map_err(|error| benchmark_error("executor-stop", error))?;
        restart_executors(&context.deps, Some(&recorder))
            .await
            .map_err(|error| benchmark_error("executor-restart", error))?;
        let post_restart = metrics_snapshot(&context.deps, "storage-startup").await?;
        storage_counts("startup", &storage_before, &post_restart, &recorder)?;
        Ok(StreamingColdIteration {
            user,
            application,
            environment,
            target,
            measured_agents: vec![agent_id],
            environment_id,
            length: self.config.length as u32,
            post_restart,
        })
    }

    async fn run(
        &self,
        context: &StreamingColdContext,
        iteration: &StreamingColdIteration,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let mut session = PublicInvocationSession::start(
            &context.deps,
            &iteration.user.token,
            &iteration.application,
            &iteration.environment,
            &iteration.target,
            "benchmark_output",
            serde_json::json!({
                "length": iteration.length,
                "domain": MEASURED_DOMAIN,
            }),
            PHASE_DEADLINE,
        )
        .await
        .map_err(|error| benchmark_error("session-accepted", error))?;
        let accepted_storage = metrics_snapshot(&context.deps, "storage-accepted").await?;
        storage_counts(
            "accepted",
            &iteration.post_restart,
            &accepted_storage,
            &recorder,
        )?;
        loop {
            let first_item = session
                .checkpoint()
                .attempts
                .first()
                .ok_or_else(|| BenchmarkError::new("first-item", "missing session attempt"))?
                .first_item;
            if first_item.is_some() {
                break;
            }
            session
                .receive()
                .await
                .map_err(|error| benchmark_error("first-item", error))?;
        }
        let first_storage = metrics_snapshot(&context.deps, "storage-first").await?;
        storage_counts("first", &accepted_storage, &first_storage, &recorder)?;
        let report = session
            .finish()
            .await
            .map_err(|error| benchmark_error("stream-complete", error))?;
        validate_direct_output(&report, iteration.length, MEASURED_DOMAIN)
            .map_err(|error| benchmark_error("correctness", error))?;
        record_session_timings(&report, &recorder)
            .map_err(|error| benchmark_error("correctness-timing", error))?;
        recorder.count(
            &ResultKey::primary("stream-items"),
            u64::from(iteration.length),
        );
        recorder.count(&ResultKey::primary("stream-terminals"), 1);
        let complete_storage = metrics_snapshot(&context.deps, "storage-completion").await?;
        storage_counts("completion", &first_storage, &complete_storage, &recorder)?;
        ensure_stream_index_reads_are_bounded(
            &iteration.post_restart,
            &complete_storage,
            MAX_BOUNDED_SESSION_INDEX_READS,
        )
        .map_err(|error| benchmark_error("correctness-index-bounded", error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_test_framework::benchmark::storage_metrics::StorageOperation;
    use test_r::test;

    #[test]
    fn streaming_caller_validation_requires_exact_count_and_ordered_timings() {
        let value = vec![SchemaValue::Record {
            fields: vec![
                SchemaValue::U64(5),
                SchemaValue::U64(8),
                SchemaValue::U32(2),
            ],
        }];
        assert_eq!(
            streaming_caller_result(&value, 2).unwrap(),
            (Duration::from_nanos(5), Duration::from_nanos(8))
        );
        assert!(streaming_caller_result(&value, 1).is_err());
        let reversed = vec![SchemaValue::Record {
            fields: vec![
                SchemaValue::U64(9),
                SchemaValue::U64(8),
                SchemaValue::U32(2),
            ],
        }];
        assert!(streaming_caller_result(&reversed, 2).is_err());
    }

    #[test]
    fn storage_operation_identity_retains_api_and_entity_labels() {
        let left = StorageOperation {
            kind: "keyvalue".into(),
            operation: "get_many".into(),
            service: "stream_session_index".into(),
            api: "read_recovery".into(),
            entity: "page".into(),
        };
        let mut right = left.clone();
        right.api = "lookup_producer".into();
        assert_ne!(left, right);
    }
}
