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

/// The Store's fixed replay role and its own live-continuation gate. Clones retain the same
/// gate so Store-independent continuations observe publication by the admitting Store.
#[derive(Clone, Debug)]
pub(super) enum StoreReplayState {
    Primary,
    CompletedEntityReplay,
    IncompleteEntityReplay(EntityContinuation),
    LiveEntity,
}

#[derive(Clone, Debug)]
pub(super) struct EntityContinuation {
    live_published: Arc<AtomicBool>,
}

impl StoreReplayState {
    pub(super) fn new(mode: Option<InvocationExecutionMode>) -> Self {
        match mode {
            None => Self::Primary,
            Some(InvocationExecutionMode::ReplayingCompleted) => Self::CompletedEntityReplay,
            Some(InvocationExecutionMode::ReplayingIncomplete) => {
                Self::IncompleteEntityReplay(EntityContinuation {
                    live_published: Arc::new(AtomicBool::new(false)),
                })
            }
            Some(InvocationExecutionMode::Live) => Self::LiveEntity,
        }
    }

    pub(super) fn initial_entity_mode(&self) -> Option<InvocationExecutionMode> {
        match self {
            Self::Primary => None,
            Self::CompletedEntityReplay => Some(InvocationExecutionMode::ReplayingCompleted),
            Self::IncompleteEntityReplay(_) => Some(InvocationExecutionMode::ReplayingIncomplete),
            Self::LiveEntity => Some(InvocationExecutionMode::Live),
        }
    }

    pub(super) fn role(&self) -> ReplayToLiveRole {
        match self {
            Self::Primary => ReplayToLiveRole::PrimaryAgent,
            _ => ReplayToLiveRole::NonPrimary,
        }
    }

    pub(super) fn is_incomplete_entity(&self) -> bool {
        matches!(self, Self::IncompleteEntityReplay(_))
    }

    pub(super) fn continued_live(&self) -> bool {
        matches!(self, Self::IncompleteEntityReplay(gate) if gate.live_published.load(Ordering::Acquire))
    }

    pub(super) fn is_live(&self, primary_live_published: bool) -> bool {
        match self {
            Self::Primary => primary_live_published,
            Self::CompletedEntityReplay => false,
            Self::IncompleteEntityReplay(_) => self.continued_live(),
            Self::LiveEntity => true,
        }
    }

    #[cfg(test)]
    pub(super) fn incomplete_entity_for_test(live_published: Arc<AtomicBool>) -> Self {
        Self::IncompleteEntityReplay(EntityContinuation { live_published })
    }

    fn finalization(
        &self,
        tool_entity: bool,
        operation: Option<tool::operation::OwnerToolOperation>,
    ) -> Result<LiveFinalization, WorkerExecutorError> {
        match self {
            Self::Primary => Ok(LiveFinalization::Primary),
            // A completed parent can settle an incomplete child without becoming live itself.
            Self::CompletedEntityReplay => Ok(LiveFinalization::CompletedEntityReplay),
            Self::IncompleteEntityReplay(continuation) if tool_entity => {
                Ok(LiveFinalization::IncompleteTool {
                    continuation: continuation.clone(),
                    operation: operation.ok_or_else(|| {
                        WorkerExecutorError::runtime(
                            "Incomplete tool entity has no live-admission operation installed",
                        )
                    })?,
                })
            }
            Self::IncompleteEntityReplay(continuation) => {
                Ok(LiveFinalization::IncompleteBody(continuation.clone()))
            }
            Self::LiveEntity => Ok(LiveFinalization::AlreadyLiveEntity),
        }
    }
}

pub(super) enum LiveFinalization {
    Primary,
    CompletedEntityReplay,
    IncompleteBody(EntityContinuation),
    IncompleteTool {
        continuation: EntityContinuation,
        operation: tool::operation::OwnerToolOperation,
    },
    AlreadyLiveEntity,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(in crate::durable_host) async fn begin_switch_to_live(
        &self,
    ) -> Result<BeginReplayToLive, WorkerExecutorError> {
        begin_replay_to_live(
            &self.state.store_replay_state,
            matches!(
                self.runtime,
                OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
            ),
            self.entity_tool_operation(),
            &self.public_state,
            &self.linear_memory,
            &self.state.replay_state,
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
        let store_replay_state = self.state.store_replay_state.clone();
        let rejected = self.rejects_live_continuation_at_replay_tail()
            || (!store_replay_state.is_incomplete_entity() && !replay_ended);
        let replay_state = self.state.replay_state.clone();
        let public_state = self.public_state.clone();
        let linear_memory = self.linear_memory.clone();
        let tool_entity = matches!(
            self.runtime,
            OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
        );
        let tool_operation = self.entity_tool_operation();
        move || {
            let expected = expected.clone();
            let replay_state = replay_state.clone();
            let public_state = public_state.clone();
            let linear_memory = linear_memory.clone();
            let tool_operation = tool_operation.clone();
            let store_replay_state = store_replay_state.clone();
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
                        &store_replay_state,
                        tool_entity,
                        tool_operation,
                        &public_state,
                        &linear_memory,
                        &replay_state,
                    )
                    .await
                } else {
                    begin_local_live_continuation(
                        &store_replay_state,
                        tool_entity,
                        tool_operation,
                        &public_state,
                        &linear_memory,
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
        match self.finalization {
            LiveFinalization::Primary => ReplayToLiveRole::PrimaryAgent,
            _ => ReplayToLiveRole::NonPrimary,
        }
    }

    pub(crate) async fn finish_at(
        self,
        boundary: Option<
            golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal,
        >,
    ) -> Result<FinishReplayToLive, WorkerExecutorError> {
        match self.finalization {
            LiveFinalization::IncompleteTool {
                continuation,
                operation,
            } => {
                match operation
                    .activate_live_attachment_memory_accounting_at(boundary)
                    .await
                {
                    tool::operation::ToolLiveAdmissionOutcome::Admitted => {
                        continuation.live_published.store(true, Ordering::Release);
                    }
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
            LiveFinalization::IncompleteBody(continuation) => {
                continuation.live_published.store(true, Ordering::Release);
            }
            LiveFinalization::Primary
            | LiveFinalization::CompletedEntityReplay
            | LiveFinalization::AlreadyLiveEntity => {}
        }
        self.linear_memory.activate_live_usage_if_guest_body();
        Ok(FinishReplayToLive::Live)
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
    store_replay_state: &StoreReplayState,
    tool_entity: bool,
    tool_operation: Option<tool::operation::OwnerToolOperation>,
    public_state: &PublicDurableWorkerState<Ctx>,
    linear_memory: &LinearMemoryTracker,
    replay_state: &ReplayState,
) -> Result<BeginReplayToLive, WorkerExecutorError> {
    await_incomplete_tool_attachment_reconstruction(
        store_replay_state.is_incomplete_entity(),
        tool_entity,
        tool_operation.as_ref(),
        replay_state,
    )
    .await?;
    activate_incomplete_entity_linear_memory(
        store_replay_state.is_incomplete_entity(),
        public_state,
        linear_memory,
    )
    .await?;
    let finalization = store_replay_state.finalization(tool_entity, tool_operation)?;
    let outcome = match store_replay_state {
        StoreReplayState::Primary => replay_state.publish_primary_live(linear_memory).await?,
        _ => {
            let replay_target = replay_state.exhaust_replay_prefix().await?;
            linear_memory.switch_to_live();
            ReplayToLiveOutcome::Live { replay_target }
        }
    };
    match outcome {
        ReplayToLiveOutcome::ReplayResumed => Ok(BeginReplayToLive::ReplayResumed),
        ReplayToLiveOutcome::Live { replay_target } => {
            Ok(BeginReplayToLive::Pending(PendingReplayToLive {
                replay_target,
                finalization,
                linear_memory: linear_memory.clone(),
            }))
        }
    }
}

pub(in crate::durable_host) async fn begin_local_live_continuation<Ctx: WorkerCtx>(
    store_replay_state: &StoreReplayState,
    tool_entity: bool,
    tool_operation: Option<tool::operation::OwnerToolOperation>,
    public_state: &PublicDurableWorkerState<Ctx>,
    linear_memory: &LinearMemoryTracker,
    replay_state: &ReplayState,
    replay_target: OplogIndex,
) -> Result<PendingReplayToLive, WorkerExecutorError> {
    await_incomplete_tool_attachment_reconstruction(
        store_replay_state.is_incomplete_entity(),
        tool_entity,
        tool_operation.as_ref(),
        replay_state,
    )
    .await?;
    activate_incomplete_entity_linear_memory(
        store_replay_state.is_incomplete_entity(),
        public_state,
        linear_memory,
    )
    .await?;
    let finalization = store_replay_state.finalization(tool_entity, tool_operation)?;
    linear_memory.switch_to_live();
    Ok(PendingReplayToLive {
        replay_target,
        finalization,
        linear_memory: linear_memory.clone(),
    })
}
