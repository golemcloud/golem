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

use crate::benchmarks::{cleanup_account, cleanup_user_state, delete_workers};
use crate::invocation_session::{InvocationSession, SessionCheckpoint, SessionEvents, StreamId};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use futures::future::try_join_all;
use golem_api_grpc::proto::golem::schema::{SchemaValue as ProtoValue, schema_value};
use golem_api_grpc::proto::golem::worker::{ResumeOperation, invocation_session_result};
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::StreamSessionRecord;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId, PromiseId};
use golem_common::schema::{FromSchema, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::session_index::inspect_live_session_index;
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
    mode: TestMode,
}

pub struct RecoveryContext {
    deps: BenchmarkTestDependencies,
    metrics: StorageMetricsClient,
}

pub struct RecoveryIteration {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    env: EnvironmentId,
    deadline: tokio::time::Instant,
    warm_agent: Option<AgentId>,
    agents: Vec<ParsedAgentId>,
    gates: Vec<PromiseId>,
    expected: Vec<Vec<(u32, u32)>>,
    streams: Vec<Vec<StreamId>>,
    sessions: Mutex<Vec<InvocationSession>>,
    detached: Mutex<Vec<SessionCheckpoint>>,
    baseline: Option<StorageSnapshot>,
}

fn error(phase: &str, error: impl std::fmt::Display) -> BenchmarkError {
    BenchmarkError::new(phase, error)
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
    Ok(IdempotencyKey::new(
        report
            .acceptance
            .as_ref()
            .and_then(|a| a.idempotency_key.as_ref())
            .context("missing accepted invocation key")?
            .value
            .clone(),
    ))
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
    let epoch = report
        .acceptance
        .as_ref()
        .context("missing acceptance")?
        .epoch;
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
            output
                .items
                .iter()
                .filter(|item| item.epoch == epoch)
                .count()
                == *length as usize - prefix,
            "resumed suffix length differs"
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
            0 => {
                "Resume an older completed direct producer session after min(8,max(1,length/4)) items; size is later one-item sessions, >=129 proves recent-status eviction. Exact cursors, suffix and one logical start; no executor restart."
            }
            1 => {
                "Hard-crash two direct producers with size completed sessions each; asymmetric length and length+8 outputs, checkpoints min(8,max(1,n/4)). Explicit Takeover before gates; 5s pre-demand/35s post-completion windows. Acceptance includes admission, not proof of replay completion; counters are coarse API-labelled windows, retirement checks are public lifecycle evidence, not zero-read proof."
            }
            2 => {
                "Hard-crash one direct producer session with two sibling outputs (length,length+3), independent checkpoints min(8,max(1,n/4)); size completed sessions. Takeover accepted before gates; 5s/35s observation windows. Acceptance and result mapping bracket admission/replay; public retirement evidence is not zero-read proof."
            }
            _ => {
                "Hard-crash one direct producer session with two terminal roots and labelled unfinished children (length,length+3), independent checkpoints min(8,max(1,n/4)); size completed sessions. Terminal-cursor Takeover accepted before gates; 5s/35s windows. Producer index, not consumer journal; public retirement evidence is not zero-read proof."
            }
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

    async fn create(mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        if CASE > 3 || config.length < 4 || config.length > ((u32::MAX - 100_000) / 3 - 8) as usize
        {
            return Err(error(
                "create",
                "invalid recovery case or length (requires >=4 and overflow-safe u32 values)",
            ));
        }
        Ok(Self {
            config,
            mode: mode.clone(),
        })
    }

    async fn setup_iteration(
        &self,
        context: &RecoveryContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<RecoveryIteration> {
        let scenario_started = tokio::time::Instant::now();
        let user = phase("setup-user", context.deps.user()).await?;
        let environment = phase("setup-environment", user.app_and_env()).await;
        let (_, env) = match environment {
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
            let warm = agent_id!("StreamingRpcTarget", format!("warm-{}", uuid::Uuid::new_v4()));
            iteration.warm_agent = Some(AgentId::from_agent_id(iteration.component.id, &warm).map_err(anyhow::Error::msg)?);
            InvocationSession::start(&context.deps, &iteration.component, &warm, "benchmark_output", data_value!(1u32, 17u32), PHASE).await?.finish().await?;
            for branch in 0..if CASE == 1 { 2 } else { 1 } {
                let agent = agent_id!("StreamingRpcTarget", uuid::Uuid::new_v4().to_string());
                iteration.agents.push(agent.clone());
                let length = self.config.length as u32;
                let mut keys = Vec::new();
                if CASE == 0 {
                    let checkpoint = InvocationSession::start(&context.deps, &iteration.component, &agent, "benchmark_output", data_value!(length, 701u32), PHASE).await?
                        .disconnect_after((length / 4).clamp(1, 8) as usize).await?;
                    iteration.streams.push(leaves(&checkpoint, Topology::Flat)?);
                    iteration.expected.push(vec![(701, length)]);
                    keys.push(session_key(&checkpoint)?);
                    wait_finished(iteration, branch, &keys).await?;
                    iteration.detached.lock().await.push(checkpoint);
                } else {
                    ensure!(iteration.user.invoke_and_await_agent(&iteration.component, &agent, "ping", data_value!()).await?.into_typed::<u64>()? == 42, "initialization ping failed");
                }
                for _ in 0..self.config.size {
                    let report = InvocationSession::start(&context.deps, &iteration.component, &agent, "benchmark_output", data_value!(1u32, 23u32), PHASE).await?.finish().await?;
                    let ids = leaves(&report, Topology::Flat)?;
                    validate_values(&report, &ids[0], 23, 1)?;
                    keys.push(session_key(&report)?);
                }
                let history = wait_finished(iteration, branch, &keys).await?;
                recorder.count(&ResultKey::primary("seeded-sessions"), self.config.size as u64);
                if CASE == 0 {
                    let horizon = history.last().context("missing producer history")?.oplog_index;
                    let owned = OwnedAgentId::new(iteration.env, &worker(iteration, branch)?);
                    loop {
                        let inspection = inspect_live_session_index(&self.mode, &context.deps, &owned, &keys).await?;
                        if inspection.coverage_present && keys.iter().all(|key| inspection.sessions_present.contains(key)) {
                            if self.config.size < 129 { break; }
                            if let Some(status) = inspection.status.filter(|status| status.oplog_idx >= horizon) {
                                ensure!(status.durable_stream_sessions.get(&keys[0]).is_none(), "original session still in recent status after 129 later completions");
                                recorder.count(&ResultKey::primary("original-evicted-from-recent-status"), 1);
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    recorder.count(&ResultKey::primary("original-present-in-physical-index"), 1);
                } else {
                    let left = iteration.user.invoke_and_await_agent(&iteration.component, &agent, "create_output_gate", data_value!()).await?.into_typed::<PromiseId>()?;
                    iteration.gates.push(left.clone());
                    let (method, input, expected) = if CASE == 1 {
                        let n = length + branch as u32 * 8;
                        let domain = 701 + branch as u32 * 10_000;
                        ("benchmark_gated_output", data_value!(n, domain, left), vec![(domain, n)])
                    } else {
                        let right = iteration.user.invoke_and_await_agent(&iteration.component, &agent, "create_output_gate", data_value!()).await?.into_typed::<PromiseId>()?;
                        iteration.gates.push(right.clone());
                        (if CASE == 2 { "benchmark_gated_siblings" } else { "benchmark_gated_nested_siblings" }, data_value!(length, left, right), vec![(1000, length), (100_000, length + 3)])
                    };
                    let mut session = InvocationSession::start(&context.deps, &iteration.component, &agent, method, input, PHASE).await?;
                    iteration.streams.push(prefix(&mut session, self.topology(), &expected).await?);
                    iteration.expected.push(expected);
                    iteration.sessions.lock().await.push(session);
                }
            }
            iteration.baseline = Some(context.metrics.snapshot(context.deps.worker_executor_cluster().as_ref()).await?);
            Ok(())
        }.await;
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
            *iteration.detached.lock().await = sessions
                .into_iter()
                .map(InvocationSession::disconnect)
                .collect();
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
                let session = InvocationSession::resume(
                    &context.deps,
                    checkpoint.clone(),
                    if CASE == 0 {
                        ResumeOperation::Resume
                    } else {
                        ResumeOperation::Takeover
                    },
                    PHASE,
                )
                .await?;
                record_acceptance(session.checkpoint(), recorder)?;
                Ok(session)
            })),
        )
        .await?;
        // Both acceptances and epoch checks are complete before any gate can advance a suffix.
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
                    &checkpoints[index],
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
                            - checkpoints[index].outputs[id].items.len())
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

fn reference(value: &ProtoValue, report: &SessionCheckpoint) -> anyhow::Result<StreamId> {
    let Some(schema_value::Value::StreamReference(reference)) = &value.value else {
        anyhow::bail!("expected stream reference: {value:?}");
    };
    let id = report
        .mappings
        .get(&reference.stream_id)
        .and_then(|mapping| mapping.handle.as_ref())
        .and_then(|handle| handle.stream_id)
        .context("missing stream mapping")?;
    Ok((id.high_bits, id.low_bits))
}

pub fn roots(report: &SessionCheckpoint) -> anyhow::Result<Vec<StreamId>> {
    let Some(invocation_session_result::Result::MethodResult(value)) = report
        .result
        .as_ref()
        .and_then(|result| result.result.as_ref())
    else {
        anyhow::bail!("expected method result");
    };
    match &value.value {
        Some(schema_value::Value::TupleValue(tuple)) => tuple
            .elements
            .iter()
            .map(|value| reference(value, report))
            .collect(),
        _ => Ok(vec![reference(value, report)?]),
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
            let value = items[0].value.as_ref().context("root omitted value")?;
            let Some(schema_value::Value::RecordValue(record)) = &value.value else {
                anyhow::bail!("expected labelled nested record");
            };
            ensure!(record.fields.len() == 2, "nested record arity");
            let actual =
                SchemaValue::try_from(record.fields[0].clone()).map_err(anyhow::Error::msg)?;
            ensure!(
                actual == SchemaValue::String(label.to_string()),
                "nested branch label changed"
            );
            // A retained nested item uses its own announcement, not a later attempt's channels.
            let Some(schema_value::Value::StreamReference(child)) = &record.fields[1].value else {
                anyhow::bail!("nested record omitted child reference");
            };
            let id = items[0]
                .new_stream_mappings
                .iter()
                .find(|mapping| mapping.transport_stream_id == child.stream_id)
                .and_then(|mapping| mapping.handle.as_ref())
                .and_then(|handle| handle.stream_id)
                .context("nested item omitted child mapping")?;
            Ok((id.high_bits, id.low_bits))
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
        .values()?
        .into_iter()
        .map(|value| SchemaValue::try_from(value).map_err(anyhow::Error::msg))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let expected = (0..length)
        .map(|index| SchemaValue::U32(domain + 3 * index))
        .collect::<Vec<_>>();
    ensure!(
        values == expected,
        "stream {id:?} differs from expected domain {domain}, length {length}"
    );
    Ok(())
}

/// Observe every gated prefix, including root terminals for a nested graph, before a crash.
pub async fn prefix(
    session: &mut InvocationSession,
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
    use crate::invocation_session::OutputObservation;
    use golem_api_grpc::proto::golem::common::Uuid;
    use golem_api_grpc::proto::golem::schema::{
        RecordValue, SchemaValueStreamReference, TupleValue,
    };
    use golem_api_grpc::proto::golem::worker::{
        DurableStreamHandle, DurableStreamMapping, InvocationSessionResult, OutputStreamItem,
    };
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
        let mut output = OutputObservation::default();
        output.items = [701, 704, 707]
            .into_iter()
            .map(|value| OutputStreamItem {
                value: Some(ProtoValue::try_from(SchemaValue::U32(value)).unwrap()),
                ..Default::default()
            })
            .collect();
        let mut report = SessionCheckpoint::default();
        report.outputs.insert((1, 2), output);
        validate_values(&report, &(1, 2), 701, 3).unwrap();
        assert!(validate_values(&report, &(1, 2), 10_701, 3).is_err());
        assert!(validate_values(&report, &(1, 2), 701, 4).is_err());
        let output = report.outputs.get_mut(&(1, 2)).unwrap();
        output.items[1] = output.items[0].clone();
        assert!(validate_values(&report, &(1, 2), 701, 3).is_err());
    }

    fn mapping(channel: u64, id: u64) -> DurableStreamMapping {
        DurableStreamMapping {
            transport_stream_id: channel,
            handle: Some(DurableStreamHandle {
                stream_id: Some(Uuid {
                    high_bits: 0,
                    low_bits: id,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn reference_value(channel: u64) -> ProtoValue {
        ProtoValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: channel },
            )),
        }
    }

    #[test]
    fn nested_labels_use_the_introducing_items_mapping_after_channel_renumbering() {
        let mut report = SessionCheckpoint {
            result: Some(InvocationSessionResult {
                result: Some(invocation_session_result::Result::MethodResult(
                    ProtoValue {
                        value: Some(schema_value::Value::TupleValue(TupleValue {
                            elements: vec![reference_value(8), reference_value(9)],
                        })),
                    },
                )),
                ..Default::default()
            }),
            mappings: BTreeMap::from([
                (8, mapping(8, 100)),
                (9, mapping(9, 200)),
                (3, mapping(3, 999)),
            ]),
            ..Default::default()
        };
        for (root, child, label) in [(100, 101, "left"), (200, 201, "right")] {
            let mut output = OutputObservation::default();
            output.items.push(OutputStreamItem {
                value: Some(ProtoValue {
                    value: Some(schema_value::Value::RecordValue(RecordValue {
                        fields: vec![
                            ProtoValue::try_from(SchemaValue::String(label.into())).unwrap(),
                            reference_value(3),
                        ],
                    })),
                }),
                new_stream_mappings: vec![mapping(3, child)],
                ..Default::default()
            });
            report.outputs.insert((0, root), output);
        }
        assert_eq!(
            leaves(&report, Topology::Nested).unwrap(),
            vec![(0, 101), (0, 201)]
        );
        let left = report.outputs.get_mut(&(0, 100)).unwrap();
        left.items[0].value = Some(ProtoValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![
                    ProtoValue::try_from(SchemaValue::String("right".into())).unwrap(),
                    reference_value(3),
                ],
            })),
        });
        assert!(leaves(&report, Topology::Nested).is_err());
    }
}
