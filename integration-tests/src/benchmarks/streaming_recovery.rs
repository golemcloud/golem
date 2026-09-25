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

//! Direct producer/session-index benchmarks, not guest consumer-journal reconstruction.

use crate::benchmarks::public_invocation::{
    DetachedSession, PublicInvocationSession, SessionCheckpoint, SessionEvents, StreamId,
};
use crate::benchmarks::{cleanup_account, cleanup_user_state, delete_workers};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use futures::future::try_join_all;
use golem_client::invocation_session::encode_generated_streamless_value;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::StreamSessionRecord;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_session_public::{
    PublicInvocationResult, PublicResumeOperation,
};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, IdempotencyKey, PromiseId};
use golem_common::schema::FromSchema;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::storage_metrics::{
    StorageMetricsClient, StorageOperation, StorageSnapshot,
};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkError, BenchmarkRecorder, BenchmarkResultValue, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use std::collections::BTreeSet;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::Level;

const PHASE: Duration = Duration::from_secs(120);
const PRE_DEMAND: Duration = Duration::from_secs(5);
const POST_COMPLETION: Duration = Duration::from_secs(35);

pub type StreamingRpcReconnect = StreamingRecovery<0>;
pub type StreamingRpcRecovery = StreamingRecovery<1>;
pub type StreamingRpcRecoverySiblings = StreamingRecovery<2>;
pub type StreamingRpcRecoveryNested = StreamingRecovery<3>;

pub struct StreamingRecovery<const CASE: u8> {
    config: RunConfig,
}

pub struct RecoveryContext {
    deps: BenchmarkTestDependencies,
    metrics: StorageMetricsClient,
}

pub struct RecoveryIteration {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    env: EnvironmentId,
    application: String,
    environment: String,
    deadline: tokio::time::Instant,
    warm_agent: Option<AgentId>,
    agents: Vec<ParsedAgentId>,
    gates: Vec<PromiseId>,
    expected: Vec<Vec<(u32, u32)>>,
    streams: Vec<Vec<StreamId>>,
    sessions: Mutex<Vec<PublicInvocationSession>>,
    detached: Mutex<Vec<DetachedSession>>,
    baseline: Option<StorageSnapshot>,
}

fn error(phase: &str, error: impl std::fmt::Display) -> BenchmarkError {
    BenchmarkError::new(phase, error)
}

fn public_value<T: golem_common::schema::IntoSchema + ?Sized>(
    value: &T,
) -> anyhow::Result<serde_json::Value> {
    let typed = golem_common::schema::try_into_typed_schema_value(value)?;
    encode_generated_streamless_value(typed.graph(), typed.value()).map_err(Into::into)
}

async fn phase<T>(
    name: &str,
    future: impl Future<Output = anyhow::Result<T>>,
) -> BenchmarkResultValue<T> {
    tokio::time::timeout(PHASE, future)
        .await
        .map_err(|e| error(name, e))?
        .map_err(|e| error(name, format!("{e:#}")))
}

fn session_key(report: &SessionCheckpoint) -> anyhow::Result<IdempotencyKey> {
    report
        .idempotency_key
        .clone()
        .context("missing accepted invocation key")
}

fn worker(iteration: &RecoveryIteration, index: usize) -> anyhow::Result<AgentId> {
    AgentId::from_agent_id(iteration.component.id, &iteration.agents[index])
        .map_err(anyhow::Error::msg)
}

fn finished_keys(history: &[PublicOplogEntryWithIndex]) -> anyhow::Result<Vec<IdempotencyKey>> {
    let mut keys = Vec::new();
    for entry in history {
        if let PublicOplogEntry::StreamSession(session) = &entry.entry
            && let StreamSessionRecord::Finished(finished) =
                StreamSessionRecord::from_value(session.record.value())
                    .map_err(anyhow::Error::msg)?
        {
            ensure!(finished.result.is_ok(), "producer session failed");
            keys.push(finished.session_key.idempotency_key().clone());
        }
    }
    Ok(keys)
}

async fn wait_finished(
    iteration: &RecoveryIteration,
    index: usize,
    expected: &[IdempotencyKey],
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    let id = worker(iteration, index)?;
    loop {
        let history = iteration.user.get_oplog(&id, OplogIndex::INITIAL).await?;
        let keys = finished_keys(&history)?;
        if expected.iter().all(|key| keys.contains(key)) {
            ensure!(
                keys.len() == expected.len(),
                "unexpected completed session count"
            );
            ensure!(
                keys.iter().collect::<BTreeSet<_>>().len() == keys.len(),
                "duplicate session finish"
            );
            return Ok(history);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn record_storage(
    after: &StorageSnapshot,
    before: Option<&StorageSnapshot>,
    recorder: &BenchmarkRecorder,
    phase: &str,
) -> anyhow::Result<()> {
    let mut delta = after.delta(before)?;
    // These selected API counters remain visible even when the entire series is absent.
    for entity in ["page", "session"] {
        delta
            .entry(StorageOperation {
                kind: "keyvalue".into(),
                operation: "get_many".into(),
                service: "stream_session_index".into(),
                api: "read_recovery".into(),
                entity: entity.into(),
            })
            .or_insert(0);
    }
    for (operation, count) in delta {
        recorder.count(
            &ResultKey::primary(format!(
                "storage-{phase}-{}-{}-{}-{}-{}",
                operation.kind,
                operation.operation,
                operation.service,
                operation.api,
                operation.entity
            )),
            count,
        );
    }
    Ok(())
}

fn elapsed(start: Instant, end: Option<Instant>, name: &str) -> anyhow::Result<Duration> {
    end.context(format!("missing {name} event"))?
        .checked_duration_since(start)
        .context(format!("{name} event precedes request"))
}

fn record_acceptance(
    report: &SessionCheckpoint,
    recorder: &BenchmarkRecorder,
) -> anyhow::Result<()> {
    let events = report.attempts.last().context("missing request timing")?;
    recorder.duration(
        &ResultKey::primary("session-accepted"),
        elapsed(events.sent, events.accepted, "acceptance")?,
    );
    Ok(())
}

fn record_first(events: &SessionEvents, recorder: &BenchmarkRecorder) -> anyhow::Result<()> {
    let accepted = events.accepted.context("missing acceptance event")?;
    let mapped = elapsed(accepted, events.result, "result mapping")?;
    let first = elapsed(events.sent, events.first_item, "first resumed item")?;
    ensure!(
        events.first_item >= events.result,
        "output preceded result mapping"
    );
    recorder.duration(&ResultKey::primary("result-mapped"), mapped);
    recorder.duration(&ResultKey::primary("first-resumed-item"), first);
    Ok(())
}

fn validate_report(
    report: &SessionCheckpoint,
    before: &SessionCheckpoint,
    topology: Topology,
    ids: &[StreamId],
    expected: &[(u32, u32)],
) -> anyhow::Result<()> {
    report.ensure_success()?;
    ensure!(
        roots(report)? == roots(before)?,
        "durable root mapping changed"
    );
    ensure!(
        leaves(report, topology)? == ids,
        "durable leaf mapping changed"
    );
    ensure!(
        report.outputs.len()
            == if topology == Topology::Nested {
                4
            } else {
                expected.len()
            },
        "unexpected output count"
    );
    ensure!(ids.len() == expected.len(), "expectation arity mismatch");
    for (id, (domain, length)) in ids.iter().zip(expected) {
        validate_values(report, id, *domain, *length)?;
        let prefix = before
            .outputs
            .get(id)
            .context("missing saved prefix")?
            .items
            .len();
        let output = report.outputs.get(id).context("missing final output")?;
        ensure!(
            prefix <= *length as usize,
            "saved prefix exceeds stream length"
        );
        ensure!(
            output.items[..prefix] == before.outputs[id].items,
            "saved prefix changed"
        );
    }
    Ok(())
}

fn validate_execution(
    history: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
) -> anyhow::Result<()> {
    let starts = history
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
        PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation)
                if &invocation.idempotency_key == key))
        })
        .count();
    ensure!(
        starts == 1,
        "expected one logical invocation start, got {starts}"
    );
    let mut current_is_target = false;
    let mut finishes = 0;
    for entry in history {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current_is_target = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation) if &invocation.idempotency_key == key);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current_is_target => {
                finishes += 1;
            }
            _ => {}
        }
    }
    ensure!(
        finishes == 1,
        "expected one logical invocation finish, got {finishes}"
    );
    ensure!(
        finished_keys(history)?
            .iter()
            .filter(|found| *found == key)
            .count()
            == 1,
        "expected one successful session finish"
    );
    Ok(())
}

fn session_records(history: &[PublicOplogEntryWithIndex]) -> Vec<PublicOplogEntry> {
    history
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::StreamSession(_)))
        .map(|entry| entry.entry.clone())
        .collect()
}

#[async_trait]
impl<const CASE: u8> Benchmark for StreamingRecovery<CASE> {
    type BenchmarkContext = RecoveryContext;
    type IterationContext = RecoveryIteration;

    fn name() -> &'static str {
        match CASE {
            0 => "streaming-rpc-reconnect",
            1 => "streaming-rpc-recovery",
            2 => "streaming-rpc-recovery-siblings",
            _ => "streaming-rpc-recovery-nested",
        }
    }

    fn description() -> &'static str {
        match CASE {
            0 => indoc! {
                "Measures a public WebSocket client reconnect to an older completed producer streaming
                session, without a guest caller or an executor restart. The client disconnects after
                `min(8, max(1, floor(length / 4)))` items, waits for producer completion, and creates
                `size` newer one-item sessions on the same producer before resuming the original
                invocation from its saved cursor. The `length` parameter is the original stream's
                item count.

                The benchmark records client-observed resume-request-to-acceptance, acceptance-to-result,
                request-to-first-resumed-item and request-to-completion latencies, plus resumed-item
                counts and coarse executor-wide logical storage-operation counts. It verifies the exact
                unread suffix without duplicate logical execution. A `size` of at least 129 additionally
                verifies that reconnect works after the session leaves recent status."
            },
            1 => indoc! {
                "Measures recovery of two active public WebSocket streaming invocations on separate
                producer agents, using client sessions rather than guest callers, after the single executor
                process is killed. Each producer has `size` completed historical sessions. Their
                measured streams contain `length` and `length + 8` items; each pauses after
                `min(8, max(1, floor(n / 4)))` items of its `n`-item stream until both takeover requests
                have been accepted after restart.

                The benchmark separately times executor shutdown, gRPC readiness and routing-table
                readiness. It records client-observed takeover-request-to-acceptance,
                acceptance-to-result, request-to-first-resumed-item (overall and per leaf), and
                request-to-completion latencies. First-item and completion timings include acceptance
                and gate-release delays, but exclude executor restart and the five-second pre-demand
                wait. Coarse executor-wide logical storage-operation counts cover startup, the
                five-second pre-demand window, resumption, completion and a 35-second post-completion
                window. It verifies exact unread suffixes, one logical execution of each measured
                invocation, and unchanged finalized session records after the observation window and
                a subsequent ordinary invocation."
            },
            2 => indoc! {
                "Measures recovery of one public WebSocket client-to-producer invocation with two
                active sibling output streams after the single executor process is killed. The producer has `size`
                completed historical sessions. The sibling streams contain `length` and `length + 3`
                items; each independently pauses after `min(8, max(1, floor(n / 4)))` items of its
                `n`-item stream until takeover has been accepted after restart.

                The benchmark separately times executor shutdown, gRPC readiness and routing-table
                readiness. It records client-observed takeover-request-to-acceptance,
                acceptance-to-result, request-to-first-resumed-item (overall and per leaf), and
                request-to-completion latencies. First-item and completion timings include acceptance
                and gate-release delays, but exclude executor restart and the five-second pre-demand
                wait. Coarse executor-wide logical storage-operation counts cover startup, the
                five-second pre-demand window, resumption, completion and a 35-second post-completion
                window. It verifies exact unread suffixes, one logical execution, and unchanged
                finalized session records after the observation window and a subsequent ordinary
                invocation."
            },
            _ => indoc! {
                "Measures recovery of a public WebSocket client-to-producer invocation with nested
                output streams after the single executor process is killed. The invocation returns two root streams
                that have already ended and introduced two unfinished child streams containing `length`
                and `length + 3` items. The producer has `size` completed historical sessions, and each
                child pauses after `min(8, max(1, floor(n / 4)))` items of its `n`-item stream until
                takeover has been accepted after restart.

                The benchmark separately times executor shutdown, gRPC readiness and routing-table
                readiness. It records client-observed takeover-request-to-acceptance,
                acceptance-to-result, request-to-first-resumed-item (overall and per leaf), and
                request-to-completion latencies. First-item and completion timings include acceptance
                and gate-release delays, but exclude executor restart and the five-second pre-demand
                wait. Coarse executor-wide logical storage-operation counts cover startup, the
                five-second pre-demand window, resumption, completion and a 35-second post-completion
                window. It verifies that terminal roots remain terminal, both child suffixes are exact,
                and the logical invocation is not executed again."
            },
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<RecoveryContext> {
        if !matches!(mode, TestMode::Spawned { .. }) || cluster_size != 1 {
            return Err(error(
                "setup-context",
                "streaming recovery benchmarks require spawned mode and one executor",
            ));
        }
        Ok(RecoveryContext {
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
            metrics: StorageMetricsClient::new(Duration::from_secs(10))
                .map_err(|e| error("setup-context", e))?,
        })
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        if CASE > 3 || config.length < 4 || config.length > ((u32::MAX - 100_000) / 3 - 8) as usize
        {
            return Err(error(
                "create",
                "invalid recovery case or length (requires >=4 and overflow-safe u32 values)",
            ));
        }
        Ok(Self { config })
    }

    async fn setup_iteration(
        &self,
        context: &RecoveryContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<RecoveryIteration> {
        let scenario_started = tokio::time::Instant::now();
        let user = phase("setup-user", context.deps.user()).await?;
        let environment = phase("setup-environment", user.app_and_env()).await;
        let (application, env) = match environment {
            Ok(value) => value,
            Err(e) => {
                cleanup_account(&user, &recorder).await;
                return Err(e);
            }
        };
        let component = phase(
            "setup-component",
            user.component(&env.id, "golem_it_agent_rpc_rust_release")
                .name("golem-it:agent-rpc-rust")
                .store(),
        )
        .await;
        let component = match component {
            Ok(value) => value,
            Err(e) => {
                cleanup_user_state(&user, &env.id, &recorder).await;
                return Err(e);
            }
        };
        let mut iteration = RecoveryIteration {
            user,
            component,
            env: env.id,
            application: application.name.0,
            environment: env.name.0,
            deadline: scenario_started + Duration::from_secs(690),
            warm_agent: None,
            agents: Vec::new(),
            gates: Vec::new(),
            expected: Vec::new(),
            streams: Vec::new(),
            sessions: Mutex::new(Vec::new()),
            detached: Mutex::new(Vec::new()),
            baseline: None,
        };
        // Leave enough of the outer five-minute setup guard to clean up partial seeding.
        let setup = tokio::time::timeout_at(
            scenario_started + Duration::from_secs(270),
            self.seed_and_checkpoint(context, &mut iteration, &recorder),
        )
        .await;
        let setup = setup
            .map_err(|e| error("setup-seeding", e))
            .and_then(|result| result);
        if let Err(e) = setup {
            if let Err(cleanup) = self
                .cleanup_iteration(context, iteration, recorder.clone())
                .await
            {
                recorder.failure(&cleanup.phase, cleanup.message);
            }
            Err(e)
        } else {
            Ok(iteration)
        }
    }

    async fn warmup(
        &self,
        _context: &RecoveryContext,
        _iteration: &RecoveryIteration,
    ) -> BenchmarkResultValue {
        Ok(())
    }

    async fn run(
        &self,
        context: &RecoveryContext,
        iteration: &RecoveryIteration,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        tokio::time::timeout_at(
            iteration.deadline,
            self.measure(context, iteration, &recorder),
        )
        .await
        .map_err(|e| error("whole-scenario", e))?
    }

    async fn cleanup_iteration(
        &self,
        _context: &RecoveryContext,
        iteration: RecoveryIteration,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        iteration.sessions.lock().await.clear();
        let mut workers = iteration.warm_agent.iter().cloned().collect::<Vec<_>>();
        for index in 0..iteration.agents.len() {
            match worker(&iteration, index) {
                Ok(id) => workers.push(id),
                Err(e) => recorder.failure(&ResultKey::primary("cleanup-agent-id"), e.to_string()),
            }
        }
        // Continue registry cleanup even when executor restart or worker deletion failed.
        if let Err(e) = tokio::time::timeout(
            Duration::from_secs(15),
            delete_workers(&iteration.user, &workers, &recorder),
        )
        .await
        {
            recorder.failure(&ResultKey::primary("cleanup-delete-worker"), e.to_string());
        }
        tokio::time::timeout(
            Duration::from_secs(15),
            cleanup_user_state(&iteration.user, &iteration.env, &recorder),
        )
        .await
        .map_err(|e| error("cleanup-iteration", e))?;
        Ok(())
    }

    async fn cleanup(context: RecoveryContext) -> BenchmarkResultValue {
        let stopped = phase(
            "cleanup-executors",
            context
                .deps
                .worker_executor_cluster()
                .kill_all_and_wait(tokio::time::Instant::now() + PHASE),
        )
        .await;
        context.deps.kill_all().await;
        stopped
    }
}

impl<const CASE: u8> StreamingRecovery<CASE> {
    fn topology(&self) -> Topology {
        match CASE {
            2 => Topology::Siblings,
            3 => Topology::Nested,
            _ => Topology::Flat,
        }
    }

    async fn seed_and_checkpoint(
        &self,
        context: &RecoveryContext,
        iteration: &mut RecoveryIteration,
        recorder: &BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let result: anyhow::Result<()> = async {
            let warm = agent_id!(
                "StreamingRpcTarget",
                format!("warm-{}", uuid::Uuid::new_v4())
            );
            iteration.warm_agent = Some(
                AgentId::from_agent_id(iteration.component.id, &warm)
                    .map_err(anyhow::Error::msg)?,
            );
            PublicInvocationSession::start(
                &context.deps,
                &iteration.user.token,
                &iteration.application,
                &iteration.environment,
                &warm,
                "benchmark_output",
                serde_json::json!({ "length": 1, "domain": 17 }),
                PHASE,
            )
            .await?
            .finish()
            .await?;
            for branch in 0..if CASE == 1 { 2 } else { 1 } {
                let agent = agent_id!("StreamingRpcTarget", uuid::Uuid::new_v4().to_string());
                iteration.agents.push(agent.clone());
                let length = self.config.length as u32;
                let mut keys = Vec::new();
                if CASE == 0 {
                    let checkpoint = PublicInvocationSession::start(
                        &context.deps,
                        &iteration.user.token,
                        &iteration.application,
                        &iteration.environment,
                        &agent,
                        "benchmark_output",
                        serde_json::json!({ "length": length, "domain": 701 }),
                        PHASE,
                    )
                    .await?
                    .disconnect_after((length / 4).clamp(1, 8) as usize)
                    .await?;
                    iteration
                        .streams
                        .push(leaves(&checkpoint.checkpoint, Topology::Flat)?);
                    iteration.expected.push(vec![(701, length)]);
                    keys.push(session_key(&checkpoint.checkpoint)?);
                    wait_finished(iteration, branch, &keys).await?;
                    iteration.detached.lock().await.push(checkpoint);
                } else {
                    ensure!(
                        iteration
                            .user
                            .invoke_and_await_agent(
                                &iteration.component,
                                &agent,
                                "ping",
                                data_value!()
                            )
                            .await?
                            .into_typed::<u64>()?
                            == 42,
                        "initialization ping failed"
                    );
                }
                for _ in 0..self.config.size {
                    let report = PublicInvocationSession::start(
                        &context.deps,
                        &iteration.user.token,
                        &iteration.application,
                        &iteration.environment,
                        &agent,
                        "benchmark_output",
                        serde_json::json!({ "length": 1, "domain": 23 }),
                        PHASE,
                    )
                    .await?
                    .finish()
                    .await?;
                    let ids = leaves(&report, Topology::Flat)?;
                    validate_values(&report, &ids[0], 23, 1)?;
                    keys.push(session_key(&report)?);
                }
                wait_finished(iteration, branch, &keys).await?;
                recorder.count(
                    &ResultKey::primary("seeded-sessions"),
                    self.config.size as u64,
                );
                if CASE != 0 {
                    let left = iteration
                        .user
                        .invoke_and_await_agent(
                            &iteration.component,
                            &agent,
                            "create_output_gate",
                            data_value!(),
                        )
                        .await?
                        .into_typed::<PromiseId>()?;
                    iteration.gates.push(left.clone());
                    let (method, input, expected) = if CASE == 1 {
                        let n = length + branch as u32 * 8;
                        let domain = 701 + branch as u32 * 10_000;
                        (
                            "benchmark_gated_output",
                            serde_json::json!({
                                "length": n,
                                "domain": domain,
                                "gate": public_value(&left)?,
                            }),
                            vec![(domain, n)],
                        )
                    } else {
                        let right = iteration
                            .user
                            .invoke_and_await_agent(
                                &iteration.component,
                                &agent,
                                "create_output_gate",
                                data_value!(),
                            )
                            .await?
                            .into_typed::<PromiseId>()?;
                        iteration.gates.push(right.clone());
                        (
                            if CASE == 2 {
                                "benchmark_gated_siblings"
                            } else {
                                "benchmark_gated_nested_siblings"
                            },
                            serde_json::json!({
                                "length": length,
                                "left_gate": public_value(&left)?,
                                "right_gate": public_value(&right)?,
                            }),
                            vec![(1000, length), (100_000, length + 3)],
                        )
                    };
                    let mut session = PublicInvocationSession::start(
                        &context.deps,
                        &iteration.user.token,
                        &iteration.application,
                        &iteration.environment,
                        &agent,
                        method,
                        input,
                        PHASE,
                    )
                    .await?;
                    iteration
                        .streams
                        .push(prefix(&mut session, self.topology(), &expected).await?);
                    iteration.expected.push(expected);
                    iteration.sessions.lock().await.push(session);
                }
            }
            iteration.baseline = Some(
                context
                    .metrics
                    .snapshot(context.deps.worker_executor_cluster().as_ref())
                    .await?,
            );
            Ok(())
        }
        .await;
        result.map_err(|e| error("setup-seeding", format!("{e:#}")))
    }

    async fn measure(
        &self,
        context: &RecoveryContext,
        iteration: &RecoveryIteration,
        recorder: &BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let cluster = context.deps.worker_executor_cluster();
        let mut baseline = iteration.baseline.clone();
        if CASE != 0 {
            let started = Instant::now();
            phase(
                "executor-stop",
                cluster.kill_all_and_wait(tokio::time::Instant::now() + PHASE),
            )
            .await?;
            if !cluster.all_reaped() {
                return Err(error("executor-stop", "executor exit not confirmed"));
            }
            recorder.duration(&ResultKey::primary("executor-stop"), started.elapsed());
            let sessions = std::mem::take(&mut *iteration.sessions.lock().await);
            *iteration.detached.lock().await = try_join_all(
                sessions
                    .into_iter()
                    .map(PublicInvocationSession::disconnect),
            )
            .await
            .map_err(|e| error("executor-stop", e))?;
            let started = Instant::now();
            phase("executor-grpc-ready", async {
                cluster.restart_all().await;
                Ok(())
            })
            .await?;
            recorder.duration(
                &ResultKey::primary("executor-grpc-ready"),
                started.elapsed(),
            );
            let started = Instant::now();
            phase("executor-routable", async {
                loop {
                    let table = context.deps.shard_manager().get_routing_table().await?;
                    let executors = cluster.to_vec();
                    if table.all().len() == executors.len()
                        && executors.iter().all(|executor| {
                            table
                                .all()
                                .iter()
                                .any(|pod| pod.port == executor.grpc_port())
                        })
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Ok(())
            })
            .await?;
            recorder.duration(&ResultKey::primary("executor-routable"), started.elapsed());
            let after = phase(
                "storage-startup",
                context.metrics.snapshot(cluster.as_ref()),
            )
            .await?;
            record_storage(&after, None, recorder, "startup")
                .map_err(|e| error("storage-startup", e))?;
            tokio::time::sleep(PRE_DEMAND).await;
            let observed = phase(
                "storage-pre-demand",
                context.metrics.snapshot(cluster.as_ref()),
            )
            .await?;
            record_storage(&observed, Some(&after), recorder, "pre-demand-5s")
                .map_err(|e| error("storage-pre-demand", e))?;
            baseline = Some(observed);
        }
        let checkpoints = std::mem::take(&mut *iteration.detached.lock().await);
        let resumed = phase(
            "session-accepted",
            try_join_all(checkpoints.iter().map(|checkpoint| async {
                let session = PublicInvocationSession::resume(
                    &context.deps,
                    &iteration.user.token,
                    checkpoint,
                    if CASE == 0 {
                        PublicResumeOperation::Resume
                    } else {
                        PublicResumeOperation::Takeover
                    },
                    PHASE,
                )
                .await?;
                record_acceptance(session.checkpoint(), recorder)?;
                Ok::<_, anyhow::Error>(session)
            })),
        )
        .await?;
        // Both public takeover requests are accepted before any gate can advance a suffix.
        phase("release-gates", async {
            for gate in &iteration.gates {
                iteration.user.complete_promise(gate, Vec::new()).await?;
            }
            Ok(())
        })
        .await?;
        let attachments = resumed.len();
        let (first_tx, mut first_rx) = tokio::sync::mpsc::channel(attachments.max(1));
        // Continue reading during scrapes: metrics must not delay receipt of completion.
        let readers = try_join_all(resumed.into_iter().map(|mut session| {
            let first_tx = first_tx.clone();
            async move {
                phase("first-resumed-item", async {
                    loop {
                        let events = session
                            .checkpoint()
                            .attempts
                            .last()
                            .context("missing request timing")?;
                        if events.first_item.is_some() {
                            record_first(events, recorder)?;
                            first_tx.send(()).await?;
                            break;
                        }
                        session.receive().await?;
                    }
                    Ok(())
                })
                .await?;
                phase("stream-complete", async {
                    let report = session.finish().await?;
                    let events = report.attempts.last().context("missing request timing")?;
                    recorder.duration(
                        &ResultKey::primary("stream-complete"),
                        elapsed(events.sent, events.completed, "completion")?,
                    );
                    Ok(report)
                })
                .await
            }
        }));
        drop(first_tx);
        let scrape = phase("storage-first-resumed-item", async {
            for _ in 0..attachments {
                first_rx.recv().await.context("first-item reader ended")?;
            }
            context.metrics.snapshot(cluster.as_ref()).await
        });
        let (reports, first) = tokio::try_join!(readers, scrape)?;
        record_storage(&first, baseline.as_ref(), recorder, "first-resumed-item")
            .map_err(|e| error("storage-first-resumed-item", e))?;
        let complete = phase(
            "storage-completion",
            context.metrics.snapshot(cluster.as_ref()),
        )
        .await?;
        record_storage(&complete, Some(&first), recorder, "completion")
            .map_err(|e| error("storage-completion", e))?;
        let histories = phase("correctness", async {
            let mut histories = Vec::new();
            for (index, report) in reports.iter().enumerate() {
                validate_report(
                    report,
                    &checkpoints[index].checkpoint,
                    self.topology(),
                    &iteration.streams[index],
                    &iteration.expected[index],
                )?;
                let history = iteration
                    .user
                    .get_oplog(&worker(iteration, index)?, OplogIndex::INITIAL)
                    .await?;
                validate_execution(&history, &session_key(report)?)?;
                let events = report.attempts.last().context("missing request timing")?;
                for (branch, id) in iteration.streams[index].iter().enumerate() {
                    recorder.duration(
                        &ResultKey::primary(format!(
                            "first-resumed-item-producer-{index}-leaf-{branch}"
                        )),
                        elapsed(
                            events.sent,
                            events.first_items.get(id).copied(),
                            "first resumed leaf",
                        )?,
                    );
                    recorder.count(
                        &ResultKey::primary(format!(
                            "resumed-items-producer-{index}-leaf-{branch}"
                        )),
                        (report.outputs[id].items.len()
                            - checkpoints[index].checkpoint.outputs[id].items.len())
                            as u64,
                    );
                }
                recorder.count(
                    &ResultKey::primary("output-items"),
                    report.item_count() as u64,
                );
                recorder.count(
                    &ResultKey::primary("output-terminals"),
                    report.outputs.len() as u64,
                );
                recorder.count(&ResultKey::primary("logical-invocation-starts"), 1);
                recorder.count(&ResultKey::primary("logical-invocation-finishes"), 1);
                recorder.count(&ResultKey::primary("successful-session-finishes"), 1);
                histories.push(history);
            }
            Ok(histories)
        })
        .await?;
        if CASE != 0 {
            let before_idle = phase(
                "storage-post-completion-start",
                context.metrics.snapshot(cluster.as_ref()),
            )
            .await?;
            tokio::time::sleep(POST_COMPLETION).await;
            let idle = phase(
                "storage-post-completion",
                context.metrics.snapshot(cluster.as_ref()),
            )
            .await?;
            record_storage(&idle, Some(&before_idle), recorder, "post-completion-35s")
                .map_err(|e| error("storage-post-completion", e))?;
            phase("retirement", async {
                for (index, history) in histories.iter().enumerate() {
                    let after = iteration
                        .user
                        .get_oplog(&worker(iteration, index)?, OplogIndex::INITIAL)
                        .await?;
                    ensure!(
                        session_records(history) == session_records(&after),
                        "retired sessions changed during observation window"
                    );
                    ensure!(
                        iteration
                            .user
                            .invoke_and_await_agent(
                                &iteration.component,
                                &iteration.agents[index],
                                "ping",
                                data_value!()
                            )
                            .await?
                            .into_typed::<u64>()?
                            == 42,
                        "ordinary demand failed"
                    );
                    let demanded = iteration
                        .user
                        .get_oplog(&worker(iteration, index)?, OplogIndex::INITIAL)
                        .await?;
                    ensure!(
                        session_records(history) == session_records(&demanded),
                        "ordinary demand changed retired sessions"
                    );
                    validate_execution(&demanded, &session_key(&reports[index])?)?;
                    recorder.count(&ResultKey::primary("retired-session-records-unchanged"), 1);
                }
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    Flat,
    Siblings,
    Nested,
}

fn reference(value: &serde_json::Value) -> anyhow::Result<StreamId> {
    value
        .get("$stream")
        .and_then(|stream| stream.get("streamToken"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .with_context(|| format!("expected public stream reference: {value}"))
}

pub fn roots(report: &SessionCheckpoint) -> anyhow::Result<Vec<StreamId>> {
    let Some(PublicInvocationResult::Value { value }) = report.result.as_ref() else {
        anyhow::bail!("expected method result");
    };
    match value {
        serde_json::Value::Array(values) => values.iter().map(reference).collect(),
        _ => Ok(vec![reference(value)?]),
    }
}

pub fn leaves(report: &SessionCheckpoint, topology: Topology) -> anyhow::Result<Vec<StreamId>> {
    let roots = roots(report)?;
    ensure!(
        roots.len() == if topology == Topology::Flat { 1 } else { 2 },
        "unexpected root count"
    );
    if topology != Topology::Nested {
        return Ok(roots);
    }
    roots
        .iter()
        .zip(["left", "right"])
        .map(|(root, label)| {
            let items = &report
                .outputs
                .get(root)
                .context("missing root output")?
                .items;
            ensure!(
                items.len() == 1,
                "root must introduce exactly one labelled child"
            );
            let value = &items[0].1;
            let actual = value.get("label").and_then(serde_json::Value::as_str);
            ensure!(actual == Some(label), "nested branch label changed");
            reference(
                value
                    .get("values")
                    .context("nested record omitted child reference")?,
            )
        })
        .collect()
}

fn validate_values(
    report: &SessionCheckpoint,
    id: &StreamId,
    domain: u32,
    length: u32,
) -> anyhow::Result<()> {
    let values = report
        .outputs
        .get(id)
        .context("missing output")?
        .items
        .iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let expected = (0..length)
        .map(|index| serde_json::json!(domain + 3 * index))
        .collect::<Vec<_>>();
    ensure!(
        values == expected.iter().collect::<Vec<_>>(),
        "stream {id:?} differs from expected domain {domain}, length {length}"
    );
    Ok(())
}

/// Observe every gated prefix, including root terminals for a nested graph, before a crash.
pub async fn prefix(
    session: &mut PublicInvocationSession,
    topology: Topology,
    expected: &[(u32, u32)],
) -> anyhow::Result<Vec<StreamId>> {
    loop {
        let report = session.checkpoint();
        let ready = report.result.is_some()
            && (topology != Topology::Nested
                || roots(report)?.iter().all(|root| {
                    report
                        .outputs
                        .get(root)
                        .is_some_and(|output| !output.items.is_empty() && output.terminal.is_some())
                }));
        if ready {
            let ids = leaves(report, topology)?;
            ensure!(ids.len() == expected.len(), "unexpected output topology");
            if ids.iter().zip(expected).all(|(id, (_, length))| {
                report
                    .outputs
                    .get(id)
                    .is_some_and(|output| output.items.len() == (length / 4).clamp(1, 8) as usize)
            }) {
                for (id, (domain, length)) in ids.iter().zip(expected) {
                    ensure!(
                        report
                            .outputs
                            .get(id)
                            .context("missing gated output")?
                            .terminal
                            .is_none(),
                        "gated child finalized before crash"
                    );
                    validate_values(report, id, *domain, (length / 4).clamp(1, 8))?;
                }
                return Ok(ids);
            }
        }
        session.receive().await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmarks::public_invocation::OutputObservation;
    use std::collections::BTreeMap;
    use test_r::test;

    #[test]
    fn resumed_timing_includes_acceptance_and_gate_delay() {
        let sent = Instant::now();
        let events = SessionEvents {
            sent,
            accepted: Some(sent + Duration::from_millis(70)),
            result: Some(sent + Duration::from_millis(90)),
            first_item: Some(sent + Duration::from_millis(430)),
            first_items: BTreeMap::new(),
            completed: Some(sent + Duration::from_millis(610)),
        };
        let recorder = BenchmarkRecorder::new();
        record_first(&events, &recorder).unwrap();
        assert_eq!(
            recorder.durations()[&ResultKey::primary("first-resumed-item")],
            vec![Duration::from_millis(430)]
        );
        assert_eq!(
            recorder.durations()[&ResultKey::primary("result-mapped")],
            vec![Duration::from_millis(20)]
        );
        let mut bad = events.clone();
        bad.result = None;
        let empty = BenchmarkRecorder::new();
        assert!(record_first(&bad, &empty).is_err());
        assert!(empty.durations().is_empty());
        bad.result = Some(sent + Duration::from_millis(440));
        assert!(record_first(&bad, &empty).is_err());
        assert!(empty.durations().is_empty());
        assert!(
            elapsed(
                sent + Duration::from_secs(1),
                events.completed,
                "completion"
            )
            .is_err()
        );
    }

    #[test]
    fn deterministic_values_reject_gaps_duplicates_and_wrong_branch() {
        let output = OutputObservation {
            items: [701, 704, 707]
                .into_iter()
                .enumerate()
                .map(|(sequence, value)| (sequence as u64, serde_json::json!(value)))
                .collect(),
            terminal: None,
        };
        let mut report = SessionCheckpoint::default();
        report.outputs.insert("stream".into(), output);
        validate_values(&report, &"stream".into(), 701, 3).unwrap();
        assert!(validate_values(&report, &"stream".into(), 10_701, 3).is_err());
        assert!(validate_values(&report, &"stream".into(), 701, 4).is_err());
        let output = report.outputs.get_mut("stream").unwrap();
        output.items[1] = output.items[0].clone();
        assert!(validate_values(&report, &"stream".into(), 701, 3).is_err());
    }

    fn reference_value(token: &str) -> serde_json::Value {
        serde_json::json!({ "$stream": { "streamToken": token } })
    }

    #[test]
    fn nested_labels_and_stream_tokens_define_the_leaf_topology() {
        let mut report = SessionCheckpoint::default();
        report.result = Some(PublicInvocationResult::Value {
            value: serde_json::json!([reference_value("root-left"), reference_value("root-right")]),
        });
        for (root, child, label) in [
            ("root-left", "child-left", "left"),
            ("root-right", "child-right", "right"),
        ] {
            report.outputs.insert(
                root.into(),
                OutputObservation {
                    items: vec![(
                        0,
                        serde_json::json!({
                            "label": label,
                            "values": reference_value(child)
                        }),
                    )],
                    terminal: None,
                },
            );
        }
        assert_eq!(
            leaves(&report, Topology::Nested).unwrap(),
            vec!["child-left", "child-right"]
        );
        let left = report.outputs.get_mut("root-left").unwrap();
        left.items[0].1["label"] = serde_json::json!("right");
        assert!(leaves(&report, Topology::Nested).is_err());
    }
}
