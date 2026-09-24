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
use crate::invocation_session::{InvocationSession, SessionCheckpoint};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::{ResumeOperation, invocation_response};
use golem_common::data_value;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::StreamOffset;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId};
use golem_common::schema::SchemaValue;
use golem_test_framework::benchmark::session_index::SpawnedSessionIndexControl;
use golem_test_framework::benchmark::storage_metrics::{StorageMetricsClient, StorageSnapshot};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
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
    inner: StreamingRpcCold<false>,
}

pub struct StreamingRpcColdRebuild {
    inner: StreamingRpcCold<true>,
}

struct StreamingRpcCold<const REBUILD: bool> {
    config: RunConfig,
    mode: TestMode,
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
    component: ComponentDto,
    target: ParsedAgentId,
    owned_target: OwnedAgentId,
    measured_agents: Vec<AgentId>,
    environment_id: EnvironmentId,
    length: u32,
    seeded_keys: Vec<IdempotencyKey>,
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
)> {
    let user = deps.user().await?;
    let (_, environment) = user.app_and_env().await?;
    let component = user
        .component(&environment.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    Ok((user, component, environment.id))
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
    component: &ComponentDto,
    target: &ParsedAgentId,
    domain: u32,
) -> anyhow::Result<IdempotencyKey> {
    let report = InvocationSession::start(
        deps,
        component,
        target,
        "benchmark_output",
        data_value!(1u32, domain),
        PHASE_DEADLINE,
    )
    .await?
    .finish()
    .await?;
    validate_direct_output(&report, 1, domain)?;
    Ok(report
        .acceptance
        .as_ref()
        .and_then(|accepted| accepted.idempotency_key.clone())
        .context("seed session omitted idempotency key")?
        .into())
}

fn offset(bytes: &[u8]) -> anyhow::Result<StreamOffset> {
    let bytes: [u8; 24] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("stream offset has {} bytes", bytes.len()))?;
    StreamOffset::from_bytes(bytes).map_err(anyhow::Error::msg)
}

fn validate_direct_output(
    report: &SessionCheckpoint,
    length: u32,
    domain: u32,
) -> anyhow::Result<()> {
    report.ensure_success()?;
    ensure!(report.outputs.len() == 1, "expected exactly one output");
    let accepted = report.acceptance.as_ref().context("missing acceptance")?;
    let output = report.outputs.values().next().context("missing output")?;
    ensure!(
        output.items.len() == length as usize,
        "output item count changed"
    );
    let values = output
        .values()?
        .into_iter()
        .map(|value| SchemaValue::try_from(value).map_err(anyhow::Error::msg))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let expected = (0..length)
        .map(|index| SchemaValue::U32(domain + index * 3))
        .collect::<Vec<_>>();
    ensure!(values == expected, "deterministic stream values changed");

    let mut previous = None;
    for (sequence, item) in output.items.iter().enumerate() {
        ensure!(
            item.producer_sequence == sequence as u64,
            "output producer sequence changed"
        );
        ensure!(item.epoch == accepted.epoch, "output epoch changed");
        let current = offset(&item.durable_offset)?;
        ensure!(
            previous.is_none_or(|previous| previous < current),
            "output offsets are not strictly increasing"
        );
        previous = Some(current);
    }
    let Some(invocation_response::Response::OutputEnd(end)) = output
        .terminal
        .as_ref()
        .and_then(|terminal| terminal.response.as_ref())
    else {
        anyhow::bail!("expected exactly one successful output terminal");
    };
    ensure!(
        end.producer_sequence == u64::from(length),
        "terminal producer sequence changed"
    );
    ensure!(end.epoch == accepted.epoch, "terminal epoch changed");
    let terminal = offset(&end.durable_offset)?;
    ensure!(
        previous.is_none_or(|previous| previous < terminal),
        "terminal offset does not follow output items"
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

async fn inspect_index(
    mode: &TestMode,
    deps: &BenchmarkTestDependencies,
    target: &OwnedAgentId,
    sessions: &[IdempotencyKey],
    delete: bool,
) -> anyhow::Result<()> {
    let mut control = SpawnedSessionIndexControl::acquire(mode, deps).await?;
    control.validate(target, sessions).await?;
    if delete {
        control.delete(target, sessions).await?;
    }
    drop(control);
    Ok(())
}

async fn validate_rebuilt_index(
    mode: &TestMode,
    deps: &BenchmarkTestDependencies,
    target: &OwnedAgentId,
    sessions: &[IdempotencyKey],
) -> anyhow::Result<()> {
    stop_executors(deps, None).await?;
    let inspected = inspect_index(mode, deps, target, sessions, false).await;
    let restarted = restart_executors(deps, None).await;
    inspected.and(restarted)
}

#[async_trait]
impl Benchmark for StreamingRpcHistory {
    type BenchmarkContext = StreamingHistoryContext;
    type IterationContext = StreamingHistoryIteration;

    fn name() -> &'static str {
        "streaming-rpc-history"
    }

    fn description() -> &'static str {
        "Warm ordinary streaming RPC with one caller/producer pair and exactly size completed one-chunk prior sessions. length is measured 4 KiB chunks; guest first chunk and completion exclude seeding."
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
        let (user, component, environment_id) = component_and_environment(&context.deps)
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
    ($type:ty, $rebuild:literal, $name:literal, $description:literal) => {
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
                    inner: StreamingRpcCold {
                        config,
                        mode: mode.clone(),
                    },
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
    false,
    "streaming-rpc-cold-indexed",
    "First direct trusted streaming demand reconstructs one cold target with a valid persisted session index. size is completed target-owned history; length is deterministic output items. Acceptance is routing/admission, not by itself proof that guest replay completed."
);

cold_benchmark!(
    StreamingRpcColdRebuild,
    true,
    "streaming-rpc-cold-rebuild",
    "First direct trusted streaming demand reconstructs one cold target after deleting only its PostgreSQL derived session index. size is completed target-owned history; length is deterministic output items. Acceptance is routing/admission, not by itself proof that guest replay completed."
);

impl<const REBUILD: bool> StreamingRpcCold<REBUILD> {
    async fn setup_iteration(
        &self,
        context: &StreamingColdContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<StreamingColdIteration> {
        let (user, component, environment_id) = component_and_environment(&context.deps)
            .await
            .map_err(|error| benchmark_error("setup", error))?;
        let warm = golem_common::agent_id!(
            "StreamingRpcTarget",
            format!("warm-{}", uuid::Uuid::new_v4())
        );
        seed_direct_session(&context.deps, &component, &warm, 900_000)
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
        let owned_target = OwnedAgentId::new(environment_id, &agent_id);
        let mut seeded_keys = Vec::with_capacity(self.config.size);
        for index in 0..self.config.size {
            seeded_keys.push(
                seed_direct_session(
                    &context.deps,
                    &component,
                    &target,
                    10_000
                        + u32::try_from(index)
                            .map_err(|error| benchmark_error("setup-seed", error))?
                            * 10,
                )
                .await
                .map_err(|error| benchmark_error("setup-seed", error))?,
            );
        }
        recorder.count(
            &ResultKey::primary("seeded-sessions"),
            seeded_keys.len() as u64,
        );
        let storage_before = metrics_snapshot(&context.deps, "setup-storage-snapshot").await?;
        stop_executors(&context.deps, Some(&recorder))
            .await
            .map_err(|error| benchmark_error("executor-stop", error))?;
        inspect_index(
            &self.mode,
            &context.deps,
            &owned_target,
            &seeded_keys,
            REBUILD,
        )
        .await
        .map_err(|error| benchmark_error("setup-index", error))?;
        restart_executors(&context.deps, Some(&recorder))
            .await
            .map_err(|error| benchmark_error("executor-restart", error))?;
        let post_restart = metrics_snapshot(&context.deps, "storage-startup").await?;
        storage_counts("startup", &storage_before, &post_restart, &recorder)?;
        Ok(StreamingColdIteration {
            user,
            component,
            target,
            owned_target,
            measured_agents: vec![agent_id],
            environment_id,
            length: self.config.length as u32,
            seeded_keys,
            post_restart,
        })
    }

    async fn run(
        &self,
        context: &StreamingColdContext,
        iteration: &StreamingColdIteration,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let mut session = InvocationSession::start(
            &context.deps,
            &iteration.component,
            &iteration.target,
            "benchmark_output",
            data_value!(iteration.length, MEASURED_DOMAIN),
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
        if !REBUILD {
            ensure_stream_index_reads_are_bounded(
                &iteration.post_restart,
                &complete_storage,
                MAX_BOUNDED_SESSION_INDEX_READS,
            )
            .map_err(|error| benchmark_error("correctness-index-bounded", error))?;
        }

        let measured_key: IdempotencyKey = report
            .acceptance
            .as_ref()
            .and_then(|accepted| accepted.idempotency_key.clone())
            .context("measured session omitted idempotency key")
            .map_err(|error| benchmark_error("correctness", error))?
            .into();
        let mut all_sessions = iteration.seeded_keys.clone();
        all_sessions.push(measured_key);

        if REBUILD {
            let item_count = report.item_count();
            let second = InvocationSession::resume(
                &context.deps,
                report,
                ResumeOperation::Resume,
                PHASE_DEADLINE,
            )
            .await
            .map_err(|error| benchmark_error("second-lookup", error))?
            .finish()
            .await
            .map_err(|error| benchmark_error("second-lookup", error))?;
            second
                .ensure_success()
                .map_err(|error| benchmark_error("second-lookup-correctness", error))?;
            if second.item_count() != item_count || second.attempts.len() != 2 {
                return Err(BenchmarkError::new(
                    "second-lookup-correctness",
                    "finalized lookup repeated output or omitted its second attempt",
                ));
            }
            let second_storage = metrics_snapshot(&context.deps, "storage-second-lookup").await?;
            ensure_stream_index_reads_are_bounded(
                &complete_storage,
                &second_storage,
                MAX_BOUNDED_SESSION_INDEX_READS,
            )
            .map_err(|error| benchmark_error("second-lookup-bounded", error))?;
            storage_counts(
                "second-lookup",
                &complete_storage,
                &second_storage,
                &recorder,
            )?;
        }

        validate_rebuilt_index(
            &self.mode,
            &context.deps,
            &iteration.owned_target,
            &all_sessions,
        )
        .await
        .map_err(|error| benchmark_error("correctness-index", error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::common::Uuid;
    use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoValue;
    use golem_api_grpc::proto::golem::worker::{
        DurableStreamHandle, DurableStreamMapping, InvocationAccepted, InvocationResponse,
        InvocationSessionCompletion, InvocationSessionResult, OutputStreamEnd, OutputStreamItem,
        StreamMappingRole, invocation_session_completion,
    };
    use golem_test_framework::benchmark::storage_metrics::StorageOperation;
    use test_r::test;

    fn canonical_offset(index: u64, sub_index: u32) -> Vec<u8> {
        StreamOffset::new(
            golem_common::model::oplog::OplogIndex::from_u64(index),
            sub_index,
        )
        .as_bytes()
        .to_vec()
    }

    fn valid_report() -> SessionCheckpoint {
        let id = Uuid {
            high_bits: 1,
            low_bits: 2,
        };
        let mut report = SessionCheckpoint {
            acceptance: Some(InvocationAccepted {
                epoch: 7,
                ..Default::default()
            }),
            result: Some(InvocationSessionResult::default()),
            completion: Some(InvocationSessionCompletion {
                outcome: Some(invocation_session_completion::Outcome::Success(
                    Default::default(),
                )),
            }),
            ..Default::default()
        };
        report.mappings.insert(
            1,
            DurableStreamMapping {
                transport_stream_id: 1,
                handle: Some(DurableStreamHandle {
                    stream_id: Some(id),
                    ..Default::default()
                }),
                role: StreamMappingRole::Output as i32,
                ..Default::default()
            },
        );
        let mut observation = crate::invocation_session::OutputObservation::default();
        observation.items = (0..2)
            .map(|sequence| OutputStreamItem {
                producer_sequence: sequence,
                durable_stream_id: Some(id),
                durable_offset: canonical_offset(10 + sequence, 0),
                epoch: 7,
                value: Some(
                    ProtoValue::try_from(SchemaValue::U32(100 + sequence as u32 * 3)).unwrap(),
                ),
                ..Default::default()
            })
            .collect();
        observation.terminal = Some(InvocationResponse {
            response: Some(invocation_response::Response::OutputEnd(OutputStreamEnd {
                producer_sequence: 2,
                durable_stream_id: Some(id),
                durable_offset: canonical_offset(12, 0),
                epoch: 7,
                ..Default::default()
            })),
        });
        report.outputs.insert((1, 2), observation);
        report
    }

    #[test]
    fn direct_output_validation_rejects_value_offset_sequence_and_terminal_drift() {
        let report = valid_report();
        assert!(validate_direct_output(&report, 2, 100).is_ok());

        let mut wrong_value = report.clone();
        wrong_value.outputs.get_mut(&(1, 2)).unwrap().items[1].value =
            Some(ProtoValue::try_from(SchemaValue::U32(104)).unwrap());
        assert!(validate_direct_output(&wrong_value, 2, 100).is_err());

        let mut duplicate_offset = report.clone();
        let first = duplicate_offset.outputs[&(1, 2)].items[0]
            .durable_offset
            .clone();
        duplicate_offset.outputs.get_mut(&(1, 2)).unwrap().items[1].durable_offset = first;
        assert!(validate_direct_output(&duplicate_offset, 2, 100).is_err());

        let mut wrong_terminal = report.clone();
        let terminal = wrong_terminal
            .outputs
            .get_mut(&(1, 2))
            .unwrap()
            .terminal
            .as_mut()
            .unwrap();
        let Some(invocation_response::Response::OutputEnd(end)) = terminal.response.as_mut() else {
            unreachable!()
        };
        end.producer_sequence = 3;
        assert!(validate_direct_output(&wrong_terminal, 2, 100).is_err());
    }

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
