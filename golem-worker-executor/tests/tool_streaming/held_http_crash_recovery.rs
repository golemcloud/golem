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

//! A crash while a P3 HTTP GET is in flight, after a completed durable wait and alongside a
//! pending long durable watchdog, must replay by resending the GET and complete the original
//! invocation. The same sequence runs as an agent method, as a tool provided by the owner's
//! component, and as a tool provided by a separate component. Controls drop either the watchdog
//! or the initial wait.
//!
//! Replay claims each `wait-for` `Start` by the invocation id of the guest call that recorded it.
//! In the natural order the initial wait's host task usually reaches its claim first; one case
//! holds it back so that the watchdog claims first, which must not change which `Start` each
//! wait receives.
//!
//! `HELD_HTTP_CRASH_REPETITIONS` repeats every case on fresh owners in one executor.

use super::*;
use anyhow::Context;
use golem_common::model::AgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_worker_executor_test_utils::{ReplayAdmissionGateHandle, ReplayAdmissionStage};
use std::time::Instant;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);

const NAP_MS: u32 = 300;
const WATCHDOG_MS: u32 = 600_000;
const CHECKPOINT_RESPONSE: &str = "checkpoint";
const FIRST_GET_TIMEOUT: Duration = Duration::from_secs(30);
const RESENT_GET_TIMEOUT: Duration = Duration::from_secs(30);
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(60);
const IN_FLIGHT_START_TIMEOUT: Duration = Duration::from_secs(10);
const REPLAY_GATE_TIMEOUT: Duration = Duration::from_secs(30);

/// The order in which the two replayed `wait-for` calls reach their `Start` claims.
#[derive(Clone, Copy, Debug)]
enum ClaimOrder {
    /// Host scheduling decides.
    Natural,
    /// The first wait to reach its claim (the initial wait in guest call order) is held until the
    /// second one (the watchdog) has claimed its `Start`.
    WatchdogFirst,
}

fn repetitions() -> usize {
    std::env::var("HELD_HTTP_CRASH_REPETITIONS")
        .ok()
        .map(|value| {
            value
                .parse()
                .expect("HELD_HTTP_CRASH_REPETITIONS is a positive number")
        })
        .unwrap_or(1)
}

/// Holds every `GET /checkpoint` until released and records each arrival.
struct HeldCheckpointServer {
    port: u16,
    started: Instant,
    arrivals: tokio::sync::watch::Receiver<Vec<Duration>>,
    release: tokio::sync::watch::Sender<bool>,
    _task: tokio_util::task::AbortOnDropHandle<()>,
}

#[derive(Clone)]
struct HeldCheckpointState {
    started: Instant,
    arrivals: Arc<tokio::sync::watch::Sender<Vec<Duration>>>,
    release: tokio::sync::watch::Receiver<bool>,
}

async fn held_checkpoint(State(state): State<HeldCheckpointState>) -> &'static str {
    let elapsed = state.started.elapsed();
    state
        .arrivals
        .send_modify(|arrivals| arrivals.push(elapsed));
    let mut release = state.release.clone();
    release
        .wait_for(|released| *released)
        .await
        .expect("checkpoint release sender is alive");
    CHECKPOINT_RESPONSE
}

impl HeldCheckpointServer {
    async fn start() -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let started = Instant::now();
        let (arrivals_tx, arrivals) = tokio::sync::watch::channel(Vec::new());
        let (release, release_rx) = tokio::sync::watch::channel(false);
        let app = Router::new()
            .route("/checkpoint", get(held_checkpoint))
            .with_state(HeldCheckpointState {
                started,
                arrivals: Arc::new(arrivals_tx),
                release: release_rx,
            });
        let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve held checkpoint requests");
        }));
        Ok(Self {
            port,
            started,
            arrivals,
            release,
            _task: task,
        })
    }

    fn authority(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn arrival_count(&self) -> usize {
        self.arrivals.borrow().len()
    }

    async fn wait_for_arrivals(&mut self, count: usize, timeout: Duration) -> anyhow::Result<()> {
        let arrivals = &mut self.arrivals;
        let reached = tokio::time::timeout(timeout, async move {
            arrivals
                .wait_for(|arrivals| arrivals.len() >= count)
                .await
                .map(|_| ())
        })
        .await;
        match reached {
            Ok(result) => Ok(result?),
            Err(_) => anyhow::bail!(
                "GET /checkpoint #{count} did not arrive within {timeout:?}; observed {}",
                self.arrival_count()
            ),
        }
    }

    fn release(&self) {
        self.release.send_replace(true);
    }

    fn observations(&self) -> String {
        format!(
            "server uptime {:?}; released {}; GET /checkpoint arrivals at {:?}",
            self.started.elapsed(),
            *self.release.borrow(),
            *self.arrivals.borrow()
        )
    }
}

#[derive(Clone, Copy, Debug)]
enum Form {
    AgentMethod,
    OwnerComponentTool,
    SeparateComponentTool,
}

struct Fixture {
    executor: TestWorkerExecutor,
    owner_component: ComponentDto,
    owner_type: &'static str,
    clock_race: Option<golem_common::schema::tool::Tool>,
    principal: Principal,
}

async fn fixture(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    form: Form,
) -> anyhow::Result<Fixture> {
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let (owner_component, owner_type) = match form {
        Form::AgentMethod | Form::OwnerComponentTool => {
            (provider_component.clone(), "NapFetchOwner")
        }
        Form::SeparateComponentTool => (
            executor
                .component_dep(&context.default_environment_id, caller)
                .store()
                .await?,
            "ToolStreamingCaller",
        ),
    };
    let clock_race = match form {
        Form::AgentMethod => None,
        Form::OwnerComponentTool | Form::SeparateComponentTool => {
            let metadata = extract_component_metadata(
                &deps
                    .component_directory
                    .join(format!("{}.wasm", provider.wasm_name)),
                false,
                true,
            )
            .await?;
            let clock_race_name = ToolName::try_from("clock-race").unwrap();
            let clock_race = metadata
                .tools
                .iter()
                .find(|tool| {
                    tool.commands
                        .nodes
                        .first()
                        .is_some_and(|root| root.name == clock_race_name.as_str())
                })
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("provider does not export clock-race"))?;
            environment_state.set_tool_deployment(
                context.default_environment_id,
                owner_component.id,
                owner_component.revision,
                Some(deployment_state(
                    context.account_id,
                    provider_component.id,
                    provider_component.revision,
                    "golem-it:tool-streaming-rust-provider",
                    owner_type,
                    metadata.tools,
                )),
            );
            Some(clock_race)
        }
    };
    Ok(Fixture {
        executor,
        owner_component,
        owner_type,
        clock_race,
        principal: Principal::GolemUser(GolemUserPrincipal {
            account_id: context.account_id,
        }),
    })
}

fn clock_race_input(
    definition: &golem_common::schema::tool::Tool,
    command: &str,
    values: Vec<(&str, SchemaValue)>,
) -> anyhow::Result<TypedSchemaValue> {
    let command_index = definition
        .command_index_by_path(&[command.to_string()])
        .ok_or_else(|| anyhow::anyhow!("clock-race has no `{command}` command"))?;
    let schema = definition.canonical_input_record_schema(command_index)?;
    let SchemaType::Record { fields, .. } = &schema.root else {
        anyhow::bail!("clock-race `{command}` input is not a record");
    };
    let normalize = |name: &str| name.replace(['-', '_'], "").to_lowercase();
    let mut values = values
        .into_iter()
        .map(|(name, value)| (normalize(name), value))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = Vec::with_capacity(fields.len());
    for field in fields {
        ordered.push(values.remove(&normalize(&field.name)).ok_or_else(|| {
            anyhow::anyhow!("no value for clock-race `{command}` field `{}`", field.name)
        })?);
    }
    anyhow::ensure!(
        values.is_empty(),
        "unused clock-race `{command}` inputs: {:?}",
        values.keys().collect::<Vec<_>>()
    );
    Ok(TypedSchemaValue::new(
        schema,
        SchemaValue::Record { fields: ordered },
    ))
}

fn external_tool_string(output: AgentInvocationResult) -> anyhow::Result<String> {
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = output else {
        anyhow::bail!("expected a successful external tool result, got {output:?}");
    };
    match result.result.map(|value| value.into_parts().1) {
        Some(SchemaValue::String(value)) => Ok(value),
        other => anyhow::bail!("expected a string tool result, got {other:?}"),
    }
}

impl Fixture {
    async fn start_owner(&self, name: &str) -> anyhow::Result<(ParsedAgentId, AgentId)> {
        let agent = agent_id!(self.owner_type, name.to_string());
        let worker_id = self
            .executor
            .start_agent(&self.owner_component.id, agent.clone())
            .await?;
        Ok((agent, worker_id))
    }

    async fn invoke_tool(
        &self,
        worker_id: &AgentId,
        key: IdempotencyKey,
        command: &str,
        input: TypedSchemaValue,
    ) -> anyhow::Result<String> {
        let fingerprint = self
            .executor
            .get_worker_metadata(worker_id)
            .await?
            .fingerprint;
        let output = self
            .executor
            .invoke_external_tool(
                worker_id,
                fingerprint,
                key,
                ToolName::try_from("clock-race").unwrap(),
                vec![command.to_string()],
                input,
                InvocationContextStack::fresh(),
                self.principal.clone(),
                None,
            )
            .await?;
        external_tool_string(output.result)
    }

    fn nap_then_fetch_input(
        &self,
        nap_ms: u32,
        authority: &str,
        watchdog_ms: u32,
    ) -> anyhow::Result<TypedSchemaValue> {
        clock_race_input(
            self.clock_race.as_ref().expect("tool form has clock-race"),
            "nap-then-fetch",
            vec![
                ("nap_ms", SchemaValue::U32(nap_ms)),
                ("authority", SchemaValue::String(authority.to_string())),
                ("watchdog_ms", SchemaValue::U32(watchdog_ms)),
            ],
        )
    }

    /// Starts `nap_then_fetch` without awaiting its result.
    async fn trigger(
        &self,
        form: Form,
        agent: &ParsedAgentId,
        worker_id: &AgentId,
        key: &IdempotencyKey,
        nap_ms: u32,
        authority: &str,
        watchdog_ms: u32,
    ) -> anyhow::Result<Option<tokio::task::JoinHandle<anyhow::Result<String>>>> {
        match form {
            Form::AgentMethod => {
                self.executor
                    .invoke_agent_with_key(
                        &self.owner_component,
                        agent,
                        key,
                        "nap_then_fetch",
                        data_value!(nap_ms, authority.to_string(), watchdog_ms),
                    )
                    .await?;
                Ok(None)
            }
            Form::OwnerComponentTool | Form::SeparateComponentTool => {
                let input = self.nap_then_fetch_input(nap_ms, authority, watchdog_ms)?;
                let executor = self.executor.clone();
                let worker_id = worker_id.clone();
                let key = key.clone();
                let principal = self.principal.clone();
                Ok(Some(tokio::spawn(async move {
                    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
                    let output = executor
                        .invoke_external_tool(
                            &worker_id,
                            fingerprint,
                            key,
                            ToolName::try_from("clock-race").unwrap(),
                            vec!["nap-then-fetch".to_string()],
                            input,
                            InvocationContextStack::fresh(),
                            principal,
                            None,
                        )
                        .await?;
                    external_tool_string(output.result)
                })))
            }
        }
    }

    /// Awaits the result recorded under `key`, attaching to the original invocation.
    async fn await_original(
        &self,
        form: Form,
        agent: &ParsedAgentId,
        worker_id: &AgentId,
        key: &IdempotencyKey,
        nap_ms: u32,
        authority: &str,
        watchdog_ms: u32,
    ) -> anyhow::Result<String> {
        match form {
            Form::AgentMethod => Ok(self
                .executor
                .invoke_and_await_agent_with_key(
                    &self.owner_component,
                    agent,
                    key,
                    "nap_then_fetch",
                    data_value!(nap_ms, authority.to_string(), watchdog_ms),
                )
                .await?
                .into_typed::<String>()?),
            Form::OwnerComponentTool | Form::SeparateComponentTool => {
                let input = self.nap_then_fetch_input(nap_ms, authority, watchdog_ms)?;
                self.invoke_tool(worker_id, key.clone(), "nap-then-fetch", input)
                    .await
            }
        }
    }

    async fn follow_up(
        &self,
        form: Form,
        agent: &ParsedAgentId,
        worker_id: &AgentId,
    ) -> anyhow::Result<String> {
        match form {
            Form::AgentMethod => Ok(self
                .executor
                .invoke_and_await_agent(&self.owner_component, agent, "follow_up", data_value!())
                .await?
                .into_typed::<String>()?),
            Form::OwnerComponentTool | Form::SeparateComponentTool => {
                let input = clock_race_input(
                    self.clock_race.as_ref().expect("tool form has clock-race"),
                    "follow-up",
                    Vec::new(),
                )?;
                self.invoke_tool(worker_id, IdempotencyKey::fresh(), "follow-up", input)
                    .await
            }
        }
    }

    async fn diagnostics(
        &self,
        worker_id: &AgentId,
        server: &HeldCheckpointServer,
        pre_crash_oplog: &Option<Vec<PublicOplogEntryWithIndex>>,
    ) -> String {
        let mut report = String::new();
        match self.executor.get_worker_metadata(worker_id).await {
            Ok(metadata) => report.push_str(&format!(
                "worker {worker_id}: status {:?}, retry count {}, pending invocations {}, last error kind {:?}, last error {:?}\n",
                metadata.status,
                metadata.retry_count,
                metadata.pending_invocation_count,
                metadata.last_error_kind,
                metadata.last_error,
            )),
            Err(error) => report.push_str(&format!("worker metadata unavailable: {error:#}\n")),
        }
        report.push_str(&server.observations());
        report.push('\n');
        if let Some(oplog) = pre_crash_oplog {
            report.push_str("oplog at the crash:\n");
            for entry in oplog {
                report.push_str(&format!("  {entry:?}\n"));
            }
        }
        match self
            .executor
            .get_oplog(worker_id, OplogIndex::INITIAL)
            .await
        {
            Ok(oplog) => {
                report.push_str("oplog now:\n");
                for entry in oplog {
                    report.push_str(&format!("  {entry:?}\n"));
                }
            }
            Err(error) => report.push_str(&format!("oplog unavailable: {error:#}\n")),
        }
        report
    }

    /// Commits the owner oplog until it holds the `http::client::send` `Start` of the held GET
    /// without a terminal, and checks the surrounding window: the initial wait completed and was
    /// delivered, and the watchdog wait is still open.
    async fn persist_in_flight_window(
        &self,
        worker_id: &AgentId,
        nap_ms: u32,
        watchdog_ms: u32,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let deadline = Instant::now() + IN_FLIGHT_START_TIMEOUT;
        loop {
            self.executor.commit_oplog(worker_id).await?;
            let oplog = self
                .executor
                .get_oplog(worker_id, OplogIndex::INITIAL)
                .await?;
            let terminated = |start: OplogIndex| {
                oplog.iter().any(|entry| match &entry.entry {
                    PublicOplogEntry::End(end) => end.start_index == start,
                    PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
                    _ => false,
                })
            };
            let delivered = |start: OplogIndex| {
                oplog.iter().any(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivered) if delivered.start_index == start)
                })
            };
            let starts = |function_name: &str| {
                oplog
                    .iter()
                    .filter(|entry| {
                        matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == function_name)
                    })
                    .map(|entry| entry.oplog_index)
                    .collect::<Vec<_>>()
            };
            let sends = starts("http::client::send");
            if let [send] = sends.as_slice()
                && !terminated(*send)
            {
                let waits = starts("clocks::monotonic-clock::wait-for");
                let open_waits = waits.iter().filter(|wait| !terminated(**wait)).count();
                let completed_waits = waits
                    .iter()
                    .filter(|wait| **wait < *send && terminated(**wait) && delivered(**wait))
                    .count();
                let expected_open = usize::from(watchdog_ms > 0);
                let expected_completed = usize::from(nap_ms > 0);
                // Without an initial wait the send and the watchdog start concurrently, so the
                // watchdog's `Start` can still be on its way to the oplog.
                if open_waits == expected_open && completed_waits == expected_completed {
                    return Ok(oplog);
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "in-flight window has {open_waits} open and {completed_waits} completed waits, \
                     expected {expected_open} open and {expected_completed} completed"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            anyhow::ensure!(
                sends.len() <= 1 && Instant::now() < deadline,
                "the held GET has no single unterminated `http::client::send` Start: {sends:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Arms the replay gates of [`ClaimOrder::WatchdogFirst`] before the crash, so that no
    /// replayed wait can pass its claim before the gates are in place.
    fn arm_watchdog_first_gates(&self, worker_id: &AgentId) -> WatchdogFirstGates {
        let wait = |stage| {
            self.executor.gate_next_replay_access_admission(
                worker_id,
                "monotonic-clock::wait-for",
                stage,
            )
        };
        WatchdogFirstGates {
            first_wait: wait(ReplayAdmissionStage::BeforeScope),
            second_wait: wait(ReplayAdmissionStage::BeforeScope),
            second_wait_claimed: wait(ReplayAdmissionStage::AfterStart),
        }
    }

    async fn crash_while_get_is_held(
        &self,
        form: Form,
        owner_name: &str,
        nap_ms: u32,
        watchdog_ms: u32,
        claim_order: ClaimOrder,
    ) -> anyhow::Result<()> {
        let mut server = HeldCheckpointServer::start().await?;
        let authority = server.authority();
        let (agent, worker_id) = self.start_owner(owner_name).await?;
        let key = IdempotencyKey::fresh();
        let mut pre_crash_oplog = None;
        let mut original_waiter = None;

        let outcome = async {
            original_waiter = self
                .trigger(
                    form,
                    &agent,
                    &worker_id,
                    &key,
                    nap_ms,
                    &authority,
                    watchdog_ms,
                )
                .await?;
            server.wait_for_arrivals(1, FIRST_GET_TIMEOUT).await?;
            pre_crash_oplog = Some(
                self.persist_in_flight_window(&worker_id, nap_ms, watchdog_ms)
                    .await?,
            );
            let gates = match claim_order {
                ClaimOrder::Natural => None,
                ClaimOrder::WatchdogFirst => Some(self.arm_watchdog_first_gates(&worker_id)),
            };
            self.executor.simulated_crash(&worker_id).await?;
            if let Some(gates) = gates {
                gates.release_watchdog_first().await?;
            }
            server
                .wait_for_arrivals(2, RESENT_GET_TIMEOUT)
                .await
                .context("replay did not resend the held GET")?;
            server.release();

            let result = tokio::time::timeout(
                COMPLETION_TIMEOUT,
                self.await_original(
                    form,
                    &agent,
                    &worker_id,
                    &key,
                    nap_ms,
                    &authority,
                    watchdog_ms,
                ),
            )
            .await
            .context("original invocation did not complete")??;
            anyhow::ensure!(
                result == format!("200 {CHECKPOINT_RESPONSE}"),
                "original invocation returned {result:?}"
            );
            anyhow::ensure!(
                server.arrival_count() == 2,
                "expected the held GET and exactly one resend, observed {}",
                server.arrival_count()
            );

            let follow_up =
                tokio::time::timeout(COMPLETION_TIMEOUT, self.follow_up(form, &agent, &worker_id))
                    .await
                    .context("follow-up invocation did not complete")??;
            anyhow::ensure!(
                follow_up == "settled",
                "follow-up invocation returned {follow_up:?}"
            );
            let status = self.executor.get_worker_metadata(&worker_id).await?.status;
            anyhow::ensure!(
                status != AgentStatus::Failed,
                "owner is {status:?} after a successful follow-up"
            );

            // A restart after settlement replays the completed history, including the resent GET.
            self.executor.simulated_crash(&worker_id).await?;
            let replayed_follow_up =
                tokio::time::timeout(COMPLETION_TIMEOUT, self.follow_up(form, &agent, &worker_id))
                    .await
                    .context("follow-up invocation after a settled restart did not complete")??;
            anyhow::ensure!(
                replayed_follow_up == "settled",
                "follow-up invocation after a settled restart returned {replayed_follow_up:?}"
            );
            anyhow::ensure!(
                server.arrival_count() == 2,
                "replay of the completed GET must not resend it, observed {} GETs",
                server.arrival_count()
            );
            let status = self.executor.get_worker_metadata(&worker_id).await?.status;
            anyhow::ensure!(
                status != AgentStatus::Failed,
                "owner is {status:?} after a settled restart"
            );
            self.executor.check_oplog_is_queryable(&worker_id).await?;
            let oplog = self
                .executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await?;
            let sends = oplog
                .iter()
                .filter(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "http::client::send")
                })
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>();
            let [send] = sends.as_slice() else {
                anyhow::bail!("the resent GET must complete under the recorded Start, found sends at {sends:?}");
            };
            let send_ends = oplog
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == *send))
                .count();
            anyhow::ensure!(
                send_ends == 1,
                "the recorded GET Start at {send} has {send_ends} End entries"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;

        if let Err(error) = outcome {
            let report = self
                .diagnostics(&worker_id, &server, &pre_crash_oplog)
                .await;
            let original = match original_waiter {
                Some(waiter) if waiter.is_finished() => format!("{:?}", waiter.await),
                Some(waiter) => {
                    waiter.abort();
                    "still pending".to_string()
                }
                None => "awaited by idempotency key".to_string(),
            };
            eprintln!(
                "held HTTP crash case {form:?} nap={nap_ms} watchdog={watchdog_ms} {claim_order:?} owner={owner_name} failed: {error:#}\noriginal waiter: {original}\n{report}"
            );
            return Err(error.context(format!(
                "{form:?} nap={nap_ms} watchdog={watchdog_ms} {claim_order:?} owner={owner_name}; original waiter: {original}\n{report}"
            )));
        }
        if let Some(waiter) = original_waiter {
            // The crash may end the first in-process waiter; the key lookup above is the
            // authoritative result of the original invocation.
            let _ = waiter.await;
        }
        Ok(())
    }
}

struct WatchdogFirstGates {
    first_wait: ReplayAdmissionGateHandle,
    second_wait: ReplayAdmissionGateHandle,
    /// While the first wait is held before its claim, the only wait that can claim a `Start` is
    /// the second one.
    second_wait_claimed: ReplayAdmissionGateHandle,
}

impl WatchdogFirstGates {
    async fn release_watchdog_first(mut self) -> anyhow::Result<()> {
        tokio::time::timeout(REPLAY_GATE_TIMEOUT, self.first_wait.entered())
            .await
            .context("the first replayed wait did not reach its claim")?;
        tokio::time::timeout(REPLAY_GATE_TIMEOUT, self.second_wait.entered())
            .await
            .context("the second replayed wait did not reach its claim")?;
        self.second_wait.release();
        tokio::time::timeout(REPLAY_GATE_TIMEOUT, self.second_wait_claimed.entered())
            .await
            .context("the second replayed wait did not claim its Start")?;
        self.first_wait.release();
        self.second_wait_claimed.release();
        Ok(())
    }
}

async fn run_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    form: Form,
    nap_ms: u32,
    watchdog_ms: u32,
    claim_order: ClaimOrder,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let fixture = fixture(deps, &context, provider, caller, form).await?;
    let repetitions = repetitions();
    let mut failures = Vec::new();
    for repetition in 0..repetitions {
        let owner_name =
            format!("held-http-{form:?}-{nap_ms}-{watchdog_ms}-{claim_order:?}-{repetition}");
        if let Err(error) = fixture
            .crash_while_get_is_held(form, &owner_name, nap_ms, watchdog_ms, claim_order)
            .await
        {
            failures.push(format!("{error:#}"));
        }
    }
    eprintln!(
        "held HTTP crash case {form:?} nap={nap_ms} watchdog={watchdog_ms} {claim_order:?}: {} of {repetitions} repetitions failed",
        failures.len()
    );
    anyhow::ensure!(
        failures.is_empty(),
        "{} of {repetitions} repetitions failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn agent_method_resends_held_get_after_crash_with_pending_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::AgentMethod,
        NAP_MS,
        WATCHDOG_MS,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn agent_method_resends_held_get_when_the_watchdog_claims_first(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::AgentMethod,
        NAP_MS,
        WATCHDOG_MS,
        ClaimOrder::WatchdogFirst,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn owner_component_tool_resends_held_get_when_the_watchdog_claims_first(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::OwnerComponentTool,
        NAP_MS,
        WATCHDOG_MS,
        ClaimOrder::WatchdogFirst,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn owner_component_tool_resends_held_get_after_crash_with_pending_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::OwnerComponentTool,
        NAP_MS,
        WATCHDOG_MS,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn separate_component_tool_resends_held_get_after_crash_with_pending_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::SeparateComponentTool,
        NAP_MS,
        WATCHDOG_MS,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn agent_method_resends_held_get_after_crash_without_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::AgentMethod,
        NAP_MS,
        0,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn owner_component_tool_resends_held_get_after_crash_without_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::OwnerComponentTool,
        NAP_MS,
        0,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn separate_component_tool_resends_held_get_after_crash_without_watchdog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::SeparateComponentTool,
        NAP_MS,
        0,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn agent_method_resends_held_get_after_crash_without_initial_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::AgentMethod,
        0,
        WATCHDOG_MS,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn owner_component_tool_resends_held_get_after_crash_without_initial_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::OwnerComponentTool,
        0,
        WATCHDOG_MS,
        ClaimOrder::Natural,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("30m")]
async fn separate_component_tool_resends_held_get_after_crash_without_initial_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_case(
        last_unique_id,
        deps,
        provider,
        caller,
        Form::SeparateComponentTool,
        0,
        WATCHDOG_MS,
        ClaimOrder::Natural,
    )
    .await
}
