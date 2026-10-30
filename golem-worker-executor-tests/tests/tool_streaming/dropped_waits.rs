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

//! Tool bodies that drop a pending wait. Whatever the oplog holds of the dropped wait, the
//! agent's next rebuild replays the body.

use super::*;
use test_r::test;

/// An agent that has the `clock-race` tool, on its own executor.
struct ClockToolOwner {
    context: TestContext,
    environment_state: Arc<TestEnvironmentStateService>,
    executor: TestWorkerExecutor,
    caller_component: golem_common::model::component::ComponentDto,
    agent: golem_common::model::agent::ParsedAgentId,
    worker_id: golem_common::model::AgentId,
}

impl ClockToolOwner {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        provider: &PrecompiledComponent,
        caller: &PrecompiledComponent,
        name: &str,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let environment_state = Arc::new(TestEnvironmentStateService::default());
        let executor = start_with_overrides(
            deps,
            &context,
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
        let caller_component = executor
            .component_dep(&context.default_environment_id, caller)
            .store()
            .await?;
        let metadata = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
            false,
            true,
        )
        .await?;
        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            Some(deployment_state(
                context.account_id,
                provider_component.id,
                provider_component.revision,
                "golem-it:tool-streaming-rust-provider",
                "ToolStreamingCaller",
                metadata.tools,
            )),
        );
        let agent = agent_id!("ToolStreamingCaller", name);
        let worker_id = executor
            .start_agent(&caller_component.id, agent.clone())
            .await?;
        Ok(Self {
            context,
            environment_state,
            executor,
            caller_component,
            agent,
            worker_id,
        })
    }

    /// A client's call of a `clock-race` command, with no stream attached. Returns the number
    /// that the command returned.
    async fn call_clock_tool(
        &self,
        command: &str,
        input: &[(&str, SchemaValue)],
    ) -> anyhow::Result<u64> {
        let fingerprint = self
            .executor
            .get_worker_metadata(&self.worker_id)
            .await?
            .fingerprint;
        let schema_of = |value: &SchemaValue| match value {
            SchemaValue::List { .. } => SchemaType::list(SchemaType::u64()),
            _ => SchemaType::u64(),
        };
        let input = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(
                input
                    .iter()
                    .map(|(name, value)| golem_common::schema::NamedFieldType {
                        name: name.to_string(),
                        body: schema_of(value),
                        metadata: Default::default(),
                    })
                    .collect(),
            )),
            SchemaValue::Record {
                fields: input.iter().map(|(_, value)| value.clone()).collect(),
            },
        );
        let output = self
            .executor
            .invoke_external_tool(
                &self.worker_id,
                fingerprint,
                IdempotencyKey::fresh(),
                ToolName::try_from("clock-race").unwrap(),
                vec![command.to_string()],
                input,
                InvocationContextStack::fresh(),
                Principal::GolemUser(GolemUserPrincipal {
                    account_id: self.context.account_id,
                }),
                None,
            )
            .await?;
        let AgentInvocationResult::ExternalTool { result: Ok(result) } = output.result else {
            anyhow::bail!("expected a successful `clock-race {command}`, got {output:?}");
        };
        match result.result.map(|value| value.into_parts().1) {
            Some(SchemaValue::U64(value)) => Ok(value),
            other => anyhow::bail!("expected a number from `clock-race {command}`, got {other:?}"),
        }
    }

    /// Replaces the executor by a new one that has no agent in memory.
    async fn restart_the_executor(
        self,
        deps: &WorkerExecutorTestDependencies,
    ) -> anyhow::Result<Self> {
        let Self {
            context,
            environment_state,
            executor,
            caller_component,
            agent,
            worker_id,
        } = self;
        drop(executor);
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                environment_state_service: Some(environment_state.clone()),
                ..Default::default()
            },
        )
        .await?;
        Ok(Self {
            context,
            environment_state,
            executor,
            caller_component,
            agent,
            worker_id,
        })
    }

    /// The agent's next call loads the agent, and the load replays every earlier call. The
    /// replay must not fail, and the next call must run.
    async fn the_next_call_runs_after_the_replay(&self) -> anyhow::Result<()> {
        let next_call = tokio::time::timeout(
            Duration::from_secs(60),
            self.executor.invoke_and_await_agent(
                &self.caller_component,
                &self.agent,
                "replay_probe",
                data_value!(),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("waited 60 s for the call after the restart"));
        // A replay that failed shows in the agent before it shows in the next call.
        let status = self
            .executor
            .get_worker_metadata(&self.worker_id)
            .await?
            .status;
        anyhow::ensure!(
            status != AgentStatus::Failed,
            "the agent failed after the restart: {:?}",
            next_call
        );
        let oplog = self
            .executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?;
        anyhow::ensure!(
            oplog
                .iter()
                .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
            "the replay of the earlier call must not fail: {:?}",
            oplog
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
                .collect::<Vec<_>>()
        );
        assert_eq!(next_call??.into_typed::<String>()?, "replayed");
        Ok(())
    }
}

const WAIT_FOR: &str = "monotonic-clock::wait-for";
const WAIT_UNTIL: &str = "monotonic-clock::wait-until";

fn clock_seconds(secs: &[u64]) -> SchemaValue {
    SchemaValue::List {
        elements: secs.iter().map(|secs| SchemaValue::U64(*secs)).collect(),
    }
}

/// What the oplog holds of one wait: the kind of its terminal and, for each clock read it made,
/// the kind of the read's terminal.
#[derive(Debug, PartialEq)]
struct RecordedWait {
    terminal: Option<&'static str>,
    reads: Vec<Option<&'static str>>,
}

impl ClockToolOwner {
    /// The agent's calls of the wait named `function` in oplog order.
    async fn recorded_waits(&self, function: &str) -> anyhow::Result<Vec<RecordedWait>> {
        self.executor.commit_oplog(&self.worker_id).await?;
        let oplog = self
            .executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?;
        let terminal_of = |start: OplogIndex| {
            oplog.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::End(end) if end.start_index == start => Some("End"),
                PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start => {
                    Some("Cancelled")
                }
                _ => None,
            })
        };
        Ok(oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name.ends_with(function))
            })
            .map(|wait| RecordedWait {
                terminal: terminal_of(wait.oplog_index),
                reads: oplog
                    .iter()
                    .filter(|read| {
                        matches!(&read.entry, PublicOplogEntry::Start(start)
                            if start.parent_start_index == Some(wait.oplog_index))
                    })
                    .map(|read| terminal_of(read.oplog_index))
                    .collect(),
            })
            .collect())
    }
}

/// The body races a 1 s wait against a 60 s wait and returns when the first ends. The 60 s wait
/// is still pending then, with its clock read recorded.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn a_body_that_dropped_a_wait_after_its_clock_read_is_replayed_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let owner =
        ClockToolOwner::start(last_unique_id, deps, provider, caller, "dropped-wait").await?;
    let winner = owner
        .call_clock_tool("race-p3-sleeps", &[("secs", clock_seconds(&[1, 60]))])
        .await?;
    assert_eq!(winner, 1);
    // The waits start in their own host tasks, so the oplog can hold their `Start`s in any order.
    let waits = owner.recorded_waits(WAIT_FOR).await?;
    assert!(
        waits.contains(&RecordedWait {
            terminal: Some("Cancelled"),
            reads: vec![Some("End")],
        }),
        "the 60 s wait must be cancelled after its clock read: {waits:?}"
    );

    let owner = owner.restart_the_executor(deps).await?;
    owner.the_next_call_runs_after_the_replay().await
}

impl ClockToolOwner {
    /// A client's call of `command`, whose body starts a 60 s wait, lets the host run for many
    /// turns, drops the wait while it is held at `stage` of the call named `function`, and then
    /// makes a checkpoint call of `checkpoint`.
    async fn call_a_body_that_drops_a_wait_held_at(
        &self,
        command: &str,
        function: &str,
        stage: ReplayAdmissionStage,
        checkpoint: &str,
    ) -> anyhow::Result<()> {
        let mut gate =
            self.executor
                .gate_next_replay_access_admission(&self.worker_id, function, stage);
        let mut checkpoint_gate = self.executor.gate_next_replay_access_admission(
            &self.worker_id,
            checkpoint,
            ReplayAdmissionStage::BeforeLiveStart,
        );
        let turns = 2_000;
        let input = [
            ("secs", SchemaValue::U64(60)),
            ("turns", SchemaValue::U64(turns)),
        ];
        let call = self.call_clock_tool(command, &input);
        // The body makes its checkpoint call only after it dropped the wait, so the host work held
        // at the gate continues after the drop.
        let hold = async {
            tokio::time::timeout(Duration::from_secs(60), gate.entered())
                .await
                .map_err(|_| anyhow::anyhow!("the wait did not reach {function} {stage:?}"))?;
            tokio::time::timeout(Duration::from_secs(60), checkpoint_gate.entered())
                .await
                .map_err(|_| anyhow::anyhow!("the body did not reach its {checkpoint} call"))?;
            gate.release();
            checkpoint_gate.release();
            Ok::<_, anyhow::Error>(())
        };
        let (returned, held) = tokio::join!(call, hold);
        held?;
        assert_eq!(returned?, turns);
        Ok(())
    }
}

/// The body returns before the wait's `Start` is appended. The wait still records its `Start` and
/// its `Cancelled`.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn a_body_that_dropped_a_wait_before_its_start_is_replayed_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let owner = ClockToolOwner::start(
        last_unique_id,
        deps,
        provider,
        caller,
        "dropped-before-start",
    )
    .await?;
    owner
        .call_a_body_that_drops_a_wait_held_at(
            "drop-p3-sleep-early",
            WAIT_FOR,
            ReplayAdmissionStage::BeforeLiveStart,
            WAIT_UNTIL,
        )
        .await?;
    assert_eq!(
        owner.recorded_waits(WAIT_FOR).await?,
        [RecordedWait {
            terminal: Some("Cancelled"),
            reads: vec![],
        }]
    );

    let owner = owner.restart_the_executor(deps).await?;
    owner.the_next_call_runs_after_the_replay().await
}

/// The body returns while the clock read of its wait is held after its `Start`, so the read is
/// cancelled with the wait.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn a_body_that_dropped_a_wait_during_its_clock_read_is_replayed_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let owner = ClockToolOwner::start(
        last_unique_id,
        deps,
        provider,
        caller,
        "dropped-during-read",
    )
    .await?;
    owner
        .call_a_body_that_drops_a_wait_held_at(
            "drop-p3-sleep-early",
            "monotonic-clock::now",
            ReplayAdmissionStage::AfterDeferredStart,
            WAIT_UNTIL,
        )
        .await?;
    assert_eq!(
        owner.recorded_waits(WAIT_FOR).await?,
        [RecordedWait {
            terminal: Some("Cancelled"),
            reads: vec![Some("Cancelled")],
        }]
    );

    let owner = owner.restart_the_executor(deps).await?;
    owner.the_next_call_runs_after_the_replay().await
}

/// A `wait-until` starts through the generic accessor path, not through the start of
/// `wait-for`. The body returns before the wait's `Start` is appended, and the wait still records
/// its `Start` and its `Cancelled`.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn a_body_that_dropped_a_wait_until_before_its_start_is_replayed_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let owner = ClockToolOwner::start(
        last_unique_id,
        deps,
        provider,
        caller,
        "dropped-until-before-start",
    )
    .await?;
    owner
        .call_a_body_that_drops_a_wait_held_at(
            "drop-p3-sleep-until-early",
            WAIT_UNTIL,
            ReplayAdmissionStage::BeforeLiveStart,
            WAIT_FOR,
        )
        .await?;
    assert_eq!(
        owner.recorded_waits(WAIT_UNTIL).await?,
        [RecordedWait {
            terminal: Some("Cancelled"),
            reads: vec![],
        }]
    );

    let owner = owner.restart_the_executor(deps).await?;
    owner.the_next_call_runs_after_the_replay().await
}

/// The body drops a `wait-until` in the same turn as it calls it, before the host function makes
/// any progress. The wait still records its `Start` and its `Cancelled`.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn a_body_that_dropped_a_wait_until_in_its_call_is_replayed_after_a_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let owner = ClockToolOwner::start(
        last_unique_id,
        deps,
        provider,
        caller,
        "dropped-until-in-call",
    )
    .await?;
    let turns = owner
        .call_clock_tool(
            "drop-p3-sleep-until-early",
            &[
                ("secs", SchemaValue::U64(60)),
                ("turns", SchemaValue::U64(0)),
            ],
        )
        .await?;
    assert_eq!(turns, 0);
    assert_eq!(
        owner.recorded_waits(WAIT_UNTIL).await?,
        [RecordedWait {
            terminal: Some("Cancelled"),
            reads: vec![],
        }]
    );

    let owner = owner.restart_the_executor(deps).await?;
    owner.the_next_call_runs_after_the_replay().await
}
