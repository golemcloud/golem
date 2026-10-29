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

use super::*;

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(in crate::durable_host) async fn begin_switch_to_live(
        &self,
    ) -> Result<BeginReplayToLive, WorkerExecutorError> {
        begin_replay_to_live(
            self.state.entity_execution_mode == Some(InvocationExecutionMode::ReplayingIncomplete),
            matches!(
                self.runtime,
                OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
            ),
            self.entity_tool_operation(),
            &self.public_state,
            &self.linear_memory,
            &self.state.replay_state,
            if self.runtime == OwnerRuntime::Agent {
                ReplayToLiveRole::PrimaryAgent
            } else {
                ReplayToLiveRole::NonPrimary
            },
            self.state.local_live_tail(),
        )
        .await
    }

    async fn finish_switch_to_live_at(
        &mut self,
        pending: PendingReplayToLive,
        boundary: Option<
            golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal,
        >,
    ) -> Result<FinishReplayToLive, WorkerExecutorError> {
        let role = pending.role();
        let outcome = pending.finish_at(boundary).await?;
        if outcome == FinishReplayToLive::Live && role == ReplayToLiveRole::PrimaryAgent {
            self.process_pending_replay_events().await?;
        }
        Ok(outcome)
    }

    pub(crate) fn rejects_live_continuation_at_replay_tail(&self) -> bool {
        let mode = self.entity_invocation_scope().map(|scope| scope.mode());
        mode == Some(InvocationExecutionMode::ReplayingCompleted)
            || (mode != Some(InvocationExecutionMode::ReplayingIncomplete)
                && self.runtime != OwnerRuntime::Agent)
    }

    pub(crate) fn prepare_live_continuation_at_replay_tail(
        &self,
        replay_ended: bool,
        expected: String,
    ) -> impl Future<Output = Result<BeginReplayToLive, WorkerExecutorError>> + Send + 'static + use<Ctx>
    {
        self.live_continuation_at_replay_tail_factory(replay_ended, expected)()
    }

    pub(crate) fn live_continuation_at_replay_tail_factory(
        &self,
        replay_ended: bool,
        expected: String,
    ) -> impl Fn() -> futures::future::BoxFuture<
        'static,
        Result<BeginReplayToLive, WorkerExecutorError>,
    >
    + Send
    + Sync
    + 'static
    + use<Ctx> {
        let mode = self.entity_invocation_scope().map(|scope| scope.mode());
        let replaying_incomplete_entity =
            mode == Some(InvocationExecutionMode::ReplayingIncomplete);
        let rejected = self.rejects_live_continuation_at_replay_tail()
            || (!replaying_incomplete_entity && !replay_ended);
        let replay_state = self.state.replay_state.clone();
        let public_state = self.public_state.clone();
        let linear_memory = self.linear_memory.clone();
        let tool_entity = matches!(
            self.runtime,
            OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
        );
        let tool_operation = self.entity_tool_operation();
        let local_live_tail = self.state.local_live_tail();
        let role = if self.runtime == OwnerRuntime::Agent {
            ReplayToLiveRole::PrimaryAgent
        } else {
            ReplayToLiveRole::NonPrimary
        };
        move || {
            let expected = expected.clone();
            let replay_state = replay_state.clone();
            let public_state = public_state.clone();
            let linear_memory = linear_memory.clone();
            let tool_operation = tool_operation.clone();
            let local_live_tail = local_live_tail.clone();
            Box::pin(async move {
                if rejected {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        expected,
                        format!(
                            "replay continuation at {} is valid only for the primary agent replay tail or an incomplete entity",
                            replay_state.last_replayed_index()
                        ),
                    ));
                }
                if replay_ended {
                    begin_replay_to_live(
                        replaying_incomplete_entity,
                        tool_entity,
                        tool_operation,
                        &public_state,
                        &linear_memory,
                        &replay_state,
                        role,
                        local_live_tail,
                    )
                    .await
                } else {
                    begin_local_live_continuation(
                        replaying_incomplete_entity,
                        tool_entity,
                        tool_operation,
                        &public_state,
                        &linear_memory,
                        role,
                        local_live_tail,
                        &replay_state,
                        replay_state.replay_target(),
                    )
                    .await
                    .map(BeginReplayToLive::Pending)
                }
            })
        }
    }

    pub(crate) async fn continue_live_at_replay_tail(
        &mut self,
        replay_ended: bool,
        expected: String,
    ) -> Result<bool, WorkerExecutorError> {
        self.continue_live_at_replay_tail_with_refusal(replay_ended, expected, None)
            .await
    }

    pub(crate) async fn continue_live_at_replay_tail_with_refusal(
        &mut self,
        replay_ended: bool,
        expected: String,
        boundary: Option<
            golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal,
        >,
    ) -> Result<bool, WorkerExecutorError> {
        let pending = match self
            .prepare_live_continuation_at_replay_tail(replay_ended, expected)
            .await?
        {
            BeginReplayToLive::ReplayResumed => return Ok(false),
            BeginReplayToLive::Pending(pending) => pending,
        };
        self.finish_switch_to_live_at(pending, boundary)
            .await?
            .require_live()?;
        Ok(true)
    }

    pub(crate) async fn get_oplog_entry_or_continue_live(
        &mut self,
        request: SerializableToolPositionalRequest,
    ) -> anyhow::Result<Option<(OplogIndex, OplogEntry)>> {
        concurrent::drain_queued_dropped_call_events(self).await?;
        let expected = request.expected_description();
        loop {
            if self.is_live() {
                return Ok(None);
            }
            if let Some(hook) = &self.replay_admission_hook {
                hook.before_positional_replay_read(expected);
            }
            match self
                .state
                .replay_state
                .get_oplog_entry_or_replay_end_with_admission(
                    self.entity_parent_start_index(),
                    self.entity_tool_operation()
                        .map(|operation| (operation, request)),
                )
                .await?
            {
                PositionalRead::Entry(index, entry) => {
                    return Ok(Some((index, entry)));
                }
                PositionalRead::ReplayEnded => {
                    if self
                        .continue_live_at_replay_tail_with_refusal(
                            true,
                            expected.to_string(),
                            Some(SerializableToolAdmissionRefusal::BeforePositional { request }),
                        )
                        .await?
                    {
                        return Ok(None);
                    }
                }
            }
        }
    }

    pub(in crate::durable_host) async fn switch_to_live(
        &mut self,
    ) -> Result<(), WorkerExecutorError> {
        self.switch_to_live_at(None).await
    }

    pub(in crate::durable_host) async fn switch_to_live_at(
        &mut self,
        boundary: Option<
            golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal,
        >,
    ) -> Result<(), WorkerExecutorError> {
        let pending = match self.begin_switch_to_live().await? {
            BeginReplayToLive::ReplayResumed => {
                return Err(WorkerExecutorError::runtime(
                    "replay target grew while the primary runtime was settling",
                ));
            }
            BeginReplayToLive::Pending(pending) => pending,
        };
        self.finish_switch_to_live_at(pending, boundary)
            .await?
            .require_live()
    }
}

impl FinishReplayToLive {
    pub(crate) fn require_live(self) -> Result<(), WorkerExecutorError> {
        match self {
            Self::Live => Ok(()),
            Self::AttachmentAdmissionRejected => {
                Err(tool_attachment_live_admission_rejected_error())
            }
            Self::Cancelled => Err(tool_attachment_live_admission_cancelled_error()),
        }
    }
}

impl PendingReplayToLive {
    #[allow(dead_code)] // The fixed transition target remains available for diagnostics.
    pub(crate) fn replay_target(&self) -> OplogIndex {
        self.replay_target
    }

    pub(in crate::durable_host) fn role(&self) -> ReplayToLiveRole {
        self.role
    }

    pub(crate) async fn finish_at(
        self,
        boundary: Option<
            golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal,
        >,
    ) -> Result<FinishReplayToLive, WorkerExecutorError> {
        if self.replaying_incomplete_entity && self.tool_entity {
            let operation = self.tool_operation.ok_or_else(|| {
                WorkerExecutorError::runtime(
                    "Incomplete tool entity has no live-admission operation installed",
                )
            })?;
            match operation
                .activate_live_attachment_memory_accounting_at(boundary)
                .await
            {
                tool::operation::ToolLiveAdmissionOutcome::Admitted => {}
                tool::operation::ToolLiveAdmissionOutcome::ResourceExhausted => {
                    return Ok(FinishReplayToLive::AttachmentAdmissionRejected);
                }
                tool::operation::ToolLiveAdmissionOutcome::Cancelled => {
                    return Ok(FinishReplayToLive::Cancelled);
                }
                tool::operation::ToolLiveAdmissionOutcome::Fenced => {
                    return Err(WorkerExecutorError::runtime(
                        "Tool operation was fenced during live-memory admission",
                    ));
                }
            }
        }

        if self.role == ReplayToLiveRole::NonPrimary {
            self.local_live_tail.store(true, Ordering::Release);
        }
        self.linear_memory.activate_live_usage_if_guest_body();
        Ok(FinishReplayToLive::Live)
    }
}

pub(in crate::durable_host) fn store_is_live(
    entity_execution_mode: Option<InvocationExecutionMode>,
    local_live_tail: bool,
    shared_live_published: bool,
) -> bool {
    match entity_execution_mode {
        Some(InvocationExecutionMode::ReplayingCompleted) => false,
        Some(InvocationExecutionMode::ReplayingIncomplete | InvocationExecutionMode::Live) => {
            local_live_tail
        }
        None => shared_live_published,
    }
}

pub(crate) fn tool_attachment_live_admission_rejected_error() -> WorkerExecutorError {
    WorkerExecutorError::runtime(TOOL_ATTACHMENT_LIVE_ADMISSION_REJECTED)
}

pub(crate) fn is_tool_attachment_live_admission_rejection(error: &WorkerExecutorError) -> bool {
    matches!(
        error,
        WorkerExecutorError::Runtime { details }
            if details == TOOL_ATTACHMENT_LIVE_ADMISSION_REJECTED
    )
}

pub(crate) fn tool_attachment_live_admission_cancelled_error() -> WorkerExecutorError {
    WorkerExecutorError::runtime(TOOL_ATTACHMENT_LIVE_ADMISSION_CANCELLED)
}

async fn activate_incomplete_entity_linear_memory<Ctx: WorkerCtx>(
    replaying_incomplete_entity: bool,
    public_state: &PublicDurableWorkerState<Ctx>,
    linear_memory: &LinearMemoryTracker,
) -> Result<(), WorkerExecutorError> {
    if !replaying_incomplete_entity {
        return Ok(());
    }

    let active_agents = public_state.worker().active_agents();
    linear_memory
        .activate_live_reservation(move |bytes| {
            let active_agents = active_agents.clone();
            async move { active_agents.acquire_memory(bytes).await }
        })
        .await;
    Ok(())
}

async fn await_incomplete_tool_attachment_reconstruction(
    replaying_incomplete_entity: bool,
    tool_entity: bool,
    operation: Option<&tool::operation::OwnerToolOperation>,
    replay: &ReplayState,
) -> Result<(), WorkerExecutorError> {
    if replaying_incomplete_entity && tool_entity {
        let invocation = operation
            .and_then(|operation| operation.invocation_id())
            .ok_or_else(|| {
                WorkerExecutorError::runtime("Incomplete tool entity has no accepted operation")
            })?;
        replay
            .await_tool_attachment_reconstruction(invocation.start_index())
            .await?;
    }
    Ok(())
}

pub(in crate::durable_host) async fn begin_replay_to_live<Ctx: WorkerCtx>(
    replaying_incomplete_entity: bool,
    tool_entity: bool,
    tool_operation: Option<tool::operation::OwnerToolOperation>,
    public_state: &PublicDurableWorkerState<Ctx>,
    linear_memory: &LinearMemoryTracker,
    replay_state: &ReplayState,
    role: ReplayToLiveRole,
    local_live_tail: Arc<AtomicBool>,
) -> Result<BeginReplayToLive, WorkerExecutorError> {
    await_incomplete_tool_attachment_reconstruction(
        replaying_incomplete_entity,
        tool_entity,
        tool_operation.as_ref(),
        replay_state,
    )
    .await?;
    activate_incomplete_entity_linear_memory(
        replaying_incomplete_entity,
        public_state,
        linear_memory,
    )
    .await?;
    match replay_state.switch_to_live(linear_memory, role).await? {
        ReplayToLiveOutcome::ReplayResumed => Ok(BeginReplayToLive::ReplayResumed),
        ReplayToLiveOutcome::Live { replay_target } => {
            Ok(BeginReplayToLive::Pending(PendingReplayToLive {
                replay_target,
                role,
                replaying_incomplete_entity,
                tool_entity,
                tool_operation,
                local_live_tail,
                linear_memory: linear_memory.clone(),
            }))
        }
    }
}

pub(in crate::durable_host) async fn begin_local_live_continuation<Ctx: WorkerCtx>(
    replaying_incomplete_entity: bool,
    tool_entity: bool,
    tool_operation: Option<tool::operation::OwnerToolOperation>,
    public_state: &PublicDurableWorkerState<Ctx>,
    linear_memory: &LinearMemoryTracker,
    role: ReplayToLiveRole,
    local_live_tail: Arc<AtomicBool>,
    replay_state: &ReplayState,
    replay_target: OplogIndex,
) -> Result<PendingReplayToLive, WorkerExecutorError> {
    await_incomplete_tool_attachment_reconstruction(
        replaying_incomplete_entity,
        tool_entity,
        tool_operation.as_ref(),
        replay_state,
    )
    .await?;
    activate_incomplete_entity_linear_memory(
        replaying_incomplete_entity,
        public_state,
        linear_memory,
    )
    .await?;
    linear_memory.switch_to_live();
    Ok(PendingReplayToLive {
        replay_target,
        role,
        replaying_incomplete_entity,
        tool_entity,
        tool_operation,
        local_live_tail,
        linear_memory: linear_memory.clone(),
    })
}
