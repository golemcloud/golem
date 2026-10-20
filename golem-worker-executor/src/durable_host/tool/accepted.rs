// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::{
    AcceptedToolCall, AgentEntity, Arc, AttachmentController, ByteStreamFailure,
    DeferredAdmissionCleanup, EntityBodyOutcome, EntityCallMode, EntityInvocationDescriptor,
    EntityInvocationDurability, FailedRetainedEntityResources, HasActiveAgents, HasWorker,
    HostResponseEntityInvocation, IncompleteLiveRepairBeforeBody, InterruptKind,
    InvocationExecutionMode, Mutex, OwnedToolEndpoints, RecordedEntityTerminal,
    SerializableEntityBodyExecution, SerializableToolOperationTerminal, SerializableToolRpcError,
    SessionError, ToolAttachmentCounterparty, ToolBodyStdin, ToolCaller, ToolExecution,
    ToolInvokeResponse, ToolOutputWriterEntry, ToolSidecarBody, ToolSidecarInvocation, WorkerCtx,
    WorkerExecutorError, anyhow, attachment, decode_tool_terminal, durability,
    encode_tool_terminal, invoke_native_tool, oneshot, operation, publish_output_completions,
    resource_exhausted_after_body, resource_exhausted_terminal, resource_exhausted_without_body,
    streams, terminal_from_response,
};
#[cfg(test)]
use super::{SkippedToolAttachmentEndpoints, ToolStdinEntry};

pub(super) fn recorded_tool_body_is_skipped(
    recorded: &RecordedEntityTerminal,
) -> Result<bool, WorkerExecutorError> {
    terminal_from_response(match recorded {
        RecordedEntityTerminal::Completed(response)
        | RecordedEntityTerminal::Cancelled(response) => response,
    })
    .map(|terminal| terminal.body_execution == SerializableEntityBodyExecution::Skipped)
}

pub(super) fn skipped_attachment_failure(
    terminal: &SerializableToolOperationTerminal,
) -> ByteStreamFailure {
    match &terminal.result {
        Err(SerializableToolRpcError::ResourceExhausted(_)) => ByteStreamFailure::ResourceExhausted,
        Err(SerializableToolRpcError::Cancelled) => ByteStreamFailure::Cancelled,
        Err(SerializableToolRpcError::RemoteInternalError(details))
            if terminal.body_execution == SerializableEntityBodyExecution::Trapped =>
        {
            ByteStreamFailure::Failed(details.clone())
        }
        _ => ByteStreamFailure::Failed("tool body was skipped during replay".to_string()),
    }
}

#[cfg(test)]
impl SkippedToolAttachmentEndpoints {
    pub(super) fn controllers(
        &self,
    ) -> (
        Option<AttachmentController>,
        Option<AttachmentController>,
        Option<AttachmentController>,
    ) {
        (
            self.stdin.as_ref().map(ToolStdinEntry::controller),
            self.stdout.as_ref().map(ToolOutputWriterEntry::controller),
            self.stderr.as_ref().map(ToolOutputWriterEntry::controller),
        )
    }
}

/// Completes the attachment effects of an already recorded operation terminal. A retained
/// skipped-body End is authoritative even when a crash or cut removed its following controls.
pub(super) async fn complete_owner_attachment_terminals<Ctx: WorkerCtx>(
    accessor: &ToolCaller<Ctx>,
    operation: &operation::OwnerToolOperation,
    start: golem_common::model::OplogIndex,
    controllers: [Option<&AttachmentController>; 3],
    failure: ByteStreamFailure,
) -> anyhow::Result<()> {
    use crate::durable_host::concurrent::{LiveCallPermit, prepare_access_terminal_effect};
    use crate::services::oplog::CommitLevel;
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::oplog::attachment::ToolAttachmentControl;

    for controller in controllers.into_iter().flatten() {
        let worker = accessor.primary.clone();
        let oplog = accessor.settlement.oplog.clone();
        let replay = accessor.settlement.replay.clone();
        let live = accessor.is_live();
        let suppressed = accessor.suppressed;
        let failure_context = accessor.publication_failure.clone();
        let activity = accessor.tracker.activity();
        let permit = LiveCallPermit::new(accessor.counter.clone());
        let Some(identity) = controller.observer().identity().filter(|_| !suppressed) else {
            let _ = controller.host_fail(failure.clone());
            continue;
        };
        if !live && let Some(index) = replay.await_tool_attachment_terminal(identity).await? {
            controller.await_terminal_publication(Some(index)).await?;
            continue;
        }
        let setup_failure = failure_context.clone();
        let commit_worker = worker.clone();
        let failed_controller = controller.clone();
        let failed_operation = operation.clone();
        let setup = (|| {
            let recording = worker.runtime_append_tasks().register().ok_or(
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                },
            )?;
            prepare_access_terminal_effect(
                &worker.tasks,
                recording,
                async move {
                    commit_worker
                        .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                        .await
                        .map(|_| ())
                        .map_err(WorkerExecutorError::from)
                },
                move |publication: attachment::AttachmentTerminalPublication, index| {
                    let (_activity, _permit) = (activity, permit);
                    publication.publish(index);
                },
                move |error| {
                    failure_context.fail(
                        error,
                        Some(failed_operation),
                        failed_controller,
                        "tool owner attachment terminal",
                    )
                },
            )
        })();
        let (handoff, receipt) = match setup {
            Ok(prepared) => prepared,
            Err(error) => {
                let cleanup = setup_failure.fail(
                    error.clone(),
                    Some(operation.clone()),
                    controller.clone(),
                    "tool owner attachment terminal",
                );
                tokio::spawn(cleanup);
                return Err(error.into());
            }
        };
        let accepted =
            controller.accept_owner_terminal(failure.clone(), |terminal, publication| {
                (
                    publication,
                    oplog.enqueue_add(OplogEntry::ToolAttachmentControl {
                        timestamp: golem_common::model::Timestamp::now_utc(),
                        entity_parent_start_index: Some(start),
                        attachment: identity,
                        transition: ToolAttachmentControl::TerminalSelected(terminal),
                    }),
                )
            });
        if let Some(accepted) = accepted {
            assert!(handoff.send(accepted).is_ok());
            receipt
                .await
                .map_err(|error| anyhow!("owner terminal recorder stopped: {error}"))??;
        } else {
            drop(handoff);
        }
        controller.await_terminal_publication(None).await?;
    }
    Ok(())
}

pub(super) async fn rollback_live_attachment_admission_rejection(
    operation: &operation::OwnerToolOperation,
) {
    if operation
        .take_live_attachment_admission_rejection()
        .await
        .is_some_and(|rejection| rejection.preselected)
    {
        operation
            .resolve_ordinary(
                Arc::new(resource_exhausted_terminal(
                    SerializableEntityBodyExecution::Executed,
                )),
                false,
            )
            .await;
    }
}

pub(super) fn publish_no_body_terminals(
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
    stderr: Option<&AttachmentController>,
) {
    for controller in stdin.into_iter().chain(stdout).chain(stderr) {
        controller.publish_no_body_terminal();
    }
}

pub(super) fn settle_resource_exhausted_admission(
    operation: &operation::OwnerToolOperation,
    filesystem: golem_common::model::entity::FilesystemCapability,
    deferred_admission: &Arc<operation::DeferredAdmissionTable>,
    deferred_cleanup: &mut Option<DeferredAdmissionCleanup>,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    start: golem_common::model::oplog::OplogIndex,
    call_mode: EntityCallMode,
) -> anyhow::Result<()> {
    if !operation.transition_admission(
        operation::BodyAdmissionState::Staging,
        operation::BodyAdmissionState::SettledWithoutBody,
    ) {
        return Err(anyhow!(
            "tool invocation was fenced before resource-exhausted no-body admission"
        ));
    }
    if filesystem == golem_common::model::entity::FilesystemCapability::Incapable {
        return Ok(());
    }
    if !deferred_admission.settle_staging(
        parent,
        start,
        operation::DeferredAdmissionReadiness::SettledWithoutBody,
    ) {
        return Err(anyhow!(
            "tool invocation lost its deferred no-body admission"
        ));
    }
    if call_mode == EntityCallMode::Synchronous
        && !deferred_admission.remove_settled_without_body(parent, start)
    {
        return Err(anyhow!(
            "synchronous tool invocation lost its no-body admission"
        ));
    }
    deferred_cleanup
        .as_mut()
        .expect("capable call must own deferred admission")
        .disarm();
    Ok(())
}

pub(super) async fn complete_resource_exhausted_without_body<Ctx>(
    accessor: &ToolCaller<Ctx>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
    stderr: Option<&AttachmentController>,
) -> anyhow::Result<ToolInvokeResponse>
where
    Ctx: WorkerCtx,
{
    let start = durability.scope().invocation_id().start_index();
    let _reconstruction = durability.historical_reconstruction_hold();
    let admission_rejection_preselected = operation
        .take_live_attachment_admission_rejection()
        .await
        .is_some_and(|rejection| rejection.preselected);
    if !admission_rejection_preselected && !operation.begin_ordinary() {
        return Err(anyhow!(
            "tool invocation was fenced before no-body completion"
        ));
    }
    let fallback_terminal = Arc::new(resource_exhausted_terminal(
        SerializableEntityBodyExecution::Skipped,
    ));
    let response = match resource_exhausted_without_body().await {
        Ok(response) => response,
        Err(error) => {
            operation.resolve_ordinary(fallback_terminal, false).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            return Err(error.into());
        }
    };
    let terminal = match terminal_from_response(&response) {
        Ok(terminal) => terminal,
        Err(error) => {
            operation.resolve_ordinary(fallback_terminal, false).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            return Err(error.into());
        }
    };
    let outcome = durability
        .complete_without_body_owned(&accessor.settlement, response)
        .await;
    match outcome {
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(
            response,
            _,
        )) => {
            operation.resolve_ordinary(terminal, true).await;
            let terminal = terminal_from_response(&response)?;
            let failure = skipped_attachment_failure(&terminal);
            complete_owner_attachment_terminals(
                accessor,
                &operation,
                start,
                [stdin, stdout, stderr],
                failure.clone(),
            )
            .await?;
            for controller in stdin.into_iter().chain(stdout).chain(stderr) {
                if matches!(failure, ByteStreamFailure::ResourceExhausted) {
                    controller.complete_rejected_live_memory_accounting();
                }
            }
            publish_no_body_terminals(stdin, stdout, stderr);
            operation.settle().await;
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
            response,
            _,
        )) => {
            operation.resolve_ordinary(terminal, false).await;
            let _ = operation.begin_cancel();
            operation.resolve_cancel(true).await;
            let terminal = terminal_from_response(&response)?;
            let failure = skipped_attachment_failure(&terminal);
            complete_owner_attachment_terminals(
                accessor,
                &operation,
                start,
                [stdin, stdout, stderr],
                failure,
            )
            .await?;
            publish_no_body_terminals(stdin, stdout, stderr);
            operation.settle().await;
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Err(error) => {
            operation.resolve_ordinary(terminal, false).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            Err(error.into())
        }
    }
}

pub(super) async fn execute_recorded_skipped_tool_call<Ctx>(
    accessor: &ToolCaller<Ctx>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    endpoints: OwnedToolEndpoints,
    recorded: RecordedEntityTerminal,
    deferred_admission_inserted: bool,
    deferred_cleanup: &mut Option<DeferredAdmissionCleanup>,
) -> anyhow::Result<ToolInvokeResponse>
where
    Ctx: WorkerCtx,
{
    let start = durability.scope().invocation_id().start_index();
    let _reconstruction = durability.historical_reconstruction_hold();
    let response = match &recorded {
        RecordedEntityTerminal::Completed(response)
        | RecordedEntityTerminal::Cancelled(response) => response,
    };
    let terminal = terminal_from_response(response)?;
    if terminal.body_execution != SerializableEntityBodyExecution::Skipped {
        return Err(anyhow!(
            "recorded no-body tool replay selected an executed terminal"
        ));
    }
    let failure = skipped_attachment_failure(&terminal);
    let controllers = (
        endpoints.stdin.as_ref().and_then(ToolBodyStdin::controller),
        endpoints
            .stdout
            .as_ref()
            .map(ToolOutputWriterEntry::controller),
        endpoints
            .stderr
            .as_ref()
            .map(ToolOutputWriterEntry::controller),
    );
    if !operation.transition_admission(
        operation::BodyAdmissionState::Staging,
        operation::BodyAdmissionState::SettledWithoutBody,
    ) {
        return Err(anyhow!(
            "tool invocation was fenced before recorded no-body replay"
        ));
    }
    if deferred_admission_inserted {
        let parent = durability.parent().clone();
        let start = durability.scope().invocation_id().start_index();
        let settled = accessor.execution.deferred_tool_admission().settle_staging(
            &parent,
            start,
            operation::DeferredAdmissionReadiness::SettledWithoutBody,
        );
        if !settled {
            return Err(anyhow!(
                "recorded no-body replay lost its deferred admission"
            ));
        }
        deferred_cleanup
            .as_mut()
            .expect("eager deferred admission must own cleanup")
            .disarm();
    }
    let outcome = match recorded {
        RecordedEntityTerminal::Completed(response) => {
            if !operation.begin_ordinary() {
                return Err(anyhow!(
                    "tool invocation lost ordinary no-body replay arbitration"
                ));
            }
            let outcome = durability
                .complete_without_body_owned(&accessor.settlement, response)
                .await;
            match &outcome {
                Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(
                    _,
                    _,
                )) => {
                    operation.resolve_ordinary(terminal, true).await;
                }
                _ => operation.resolve_ordinary(terminal, false).await,
            }
            outcome
        }
        RecordedEntityTerminal::Cancelled(_) => {
            if !operation.begin_cancel() {
                return Err(anyhow!(
                    "tool invocation lost cancelled no-body replay arbitration"
                ));
            }
            let outcome = durability
                .cancel_without_body_owned(&accessor.settlement)
                .await;
            operation
                .resolve_cancel(matches!(
                    &outcome,
                    Ok(
                        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
                            _,
                            _
                        )
                    )
                ))
                .await;
            outcome
        }
    }?;
    complete_owner_attachment_terminals(
        accessor,
        &operation,
        start,
        [
            controllers.0.as_ref(),
            controllers.1.as_ref(),
            controllers.2.as_ref(),
        ],
        failure,
    )
    .await?;
    publish_no_body_terminals(
        controllers.0.as_ref(),
        controllers.1.as_ref(),
        controllers.2.as_ref(),
    );
    drop(endpoints);
    operation.settle().await;
    match outcome {
        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(response, _)
        | crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(response, _) => {
            decode_tool_terminal(*response).map_err(Into::into)
        }
    }
}

impl DeferredAdmissionCleanup {
    pub(super) fn disarm(&mut self) {
        self.start = None;
    }
}

impl Drop for DeferredAdmissionCleanup {
    fn drop(&mut self) {
        if let Some(start) = self.start.take() {
            self.table.remove(&self.parent, start);
        }
    }
}

pub(super) async fn cancel_tool_before_body<Ctx>(
    accessor: &ToolCaller<Ctx>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    deferred_admission: &Arc<operation::DeferredAdmissionTable>,
    deferred_cleanup: &mut Option<DeferredAdmissionCleanup>,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    start: golem_common::model::oplog::OplogIndex,
    admission: operation::BodyAdmissionState,
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
    stderr: Option<&AttachmentController>,
) -> anyhow::Result<ToolInvokeResponse>
where
    Ctx: WorkerCtx,
{
    let _reconstruction = durability.historical_reconstruction_hold();
    let expected_readiness = match admission {
        operation::BodyAdmissionState::Staging => operation::DeferredAdmissionReadiness::Staging,
        operation::BodyAdmissionState::Ready => operation::DeferredAdmissionReadiness::Ready,
        _ => unreachable!("no-body cancellation requires deferred admission"),
    };
    if !deferred_admission.settle_operation_without_body(
        parent,
        start,
        expected_readiness,
        &operation,
        admission,
    ) {
        if operation.admission_if_active() == Some(operation::BodyAdmissionState::Registered) {
            deferred_cleanup
                .as_mut()
                .expect("capable call must own deferred admission")
                .disarm();
            return cancel_registered_tool_before_body(
                accessor, durability, operation, stdin, stdout, stderr,
            )
            .await;
        }
        return Err(anyhow!(
            "cancelled tool invocation lost its deferred admission"
        ));
    }
    deferred_cleanup
        .as_mut()
        .expect("capable call must own deferred admission")
        .disarm();
    if !operation.begin_cancel() {
        return Err(anyhow!(
            "tool invocation lost cancellation terminal arbitration"
        ));
    }
    let outcome = durability
        .cancel_without_body_owned(&accessor.settlement)
        .await;
    operation
        .resolve_cancel(matches!(
            &outcome,
            Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(_, _))
        ))
        .await;
    let outcome = outcome?;
    complete_owner_attachment_terminals(
        accessor,
        &operation,
        start,
        [stdin, stdout, stderr],
        ByteStreamFailure::Cancelled,
    )
    .await?;
    publish_no_body_terminals(stdin, stdout, stderr);
    operation.settle().await;
    match outcome {
        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(response, _)
        | crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(response, _) => {
            decode_tool_terminal(*response).map_err(Into::into)
        }
    }
}

pub(super) async fn cancel_registered_tool_before_body<Ctx>(
    accessor: &ToolCaller<Ctx>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
    stderr: Option<&AttachmentController>,
) -> anyhow::Result<ToolInvokeResponse>
where
    Ctx: WorkerCtx,
{
    let start = durability.scope().invocation_id().start_index();
    let _reconstruction = durability.historical_reconstruction_hold();
    if !operation.begin_cancel() {
        return Err(anyhow!(
            "tool invocation lost cancellation terminal arbitration"
        ));
    }
    let outcome = durability
        .cancel_without_body_owned(&accessor.settlement)
        .await;
    operation
        .resolve_cancel(matches!(
            &outcome,
            Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(_, _))
        ))
        .await;
    let outcome = outcome?;
    complete_owner_attachment_terminals(
        accessor,
        &operation,
        start,
        [stdin, stdout, stderr],
        ByteStreamFailure::Cancelled,
    )
    .await?;
    publish_no_body_terminals(stdin, stdout, stderr);
    operation.settle().await;
    match outcome {
        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(response, _)
        | crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(response, _) => {
            decode_tool_terminal(*response).map_err(Into::into)
        }
    }
}

pub(super) async fn execute_accepted_tool_call<Ctx>(
    accessor: &ToolCaller<Ctx>,
    accepted: AcceptedToolCall,
    endpoints: OwnedToolEndpoints,
    execution: Option<&ToolExecution>,
    mut completed_supervisor_started: Option<oneshot::Sender<()>>,
) -> anyhow::Result<ToolInvokeResponse>
where
    Ctx: WorkerCtx,
{
    let replaying_completed = accepted.durability.scope().mode()
        == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted;
    let replaying_reconstruction = matches!(
        accepted.durability.scope().mode(),
        golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted
            | golem_common::model::entity::InvocationExecutionMode::ReplayingIncomplete
    );
    let mut reconstruction_hold = replaying_completed
        .then(|| accepted.durability.historical_reconstruction_hold())
        .flatten();
    let reconstruction_supervisor = replaying_reconstruction.then(oneshot::channel);
    let (inner_supervisor_started, mut inner_supervisor_ready) = match reconstruction_supervisor {
        Some((started, ready)) => (Some(started), Some(ready)),
        None => (None, None),
    };
    let cleanup_operation = accepted.operation.clone();
    let primary = accessor.primary.clone();
    let active_agents = primary.active_agents();
    let owner_id = primary.owned_agent_id().clone();
    let owner_execution = accessor.execution.clone();
    let owner_operations = owner_execution.tool_operations();
    let failed_resources = Arc::new(Mutex::new(None));
    let execution_future = Box::pin(execute_accepted_tool_call_inner(
        accessor,
        accepted,
        endpoints,
        execution,
        failed_resources.clone(),
        inner_supervisor_started,
    ));
    let execution_future = async {
        if execution.is_some_and(|execution| !execution.cancellable) {
            crate::durable_host::without_entity_cancellation(execution_future).await
        } else {
            execution_future.await
        }
    };
    tokio::pin!(execution_future);
    let mut result = if let Some(ready) = inner_supervisor_ready.as_mut() {
        tokio::select! {
            biased;
            started = ready => {
                if started.is_ok() {
                    if let Some(started) = completed_supervisor_started.take() {
                        let _ = started.send(());
                    }
                    drop(reconstruction_hold.take());
                }
                execution_future.await
            }
            result = &mut execution_future => result,
        }
    } else {
        execution_future.await
    };
    if let Err(error) = &mut result {
        rollback_live_attachment_admission_rejection(&cleanup_operation).await;
        let mut failed_resources = failed_resources.lock().unwrap().take();
        if let Some(failed_resources) = failed_resources.as_mut() {
            if !failed_resources.parent_end_attempted {
                if let Err(preparation_error) =
                    failed_resources.resources.prepare_parent_end().await
                    && owner_operations.selected_owner_failure().is_none()
                {
                    *error = preparation_error.into();
                }
                failed_resources.parent_end_attempted = true;
            }
            if let Err(disposal_error) =
                failed_resources.resources.release_for_owner_failure().await
                && owner_operations.selected_owner_failure().is_none()
            {
                *error = disposal_error.into();
            }
        }
        if matches!(
            cleanup_operation.winner_if_active(),
            Some(operation::ToolOperationWinner::SelectingCancelled)
        ) {
            cleanup_operation.resolve_cancel(false).await;
        }
        let active_agent = active_agents
            .try_get_active_agent(&owner_id)
            .await
            .filter(|active_agent| Arc::ptr_eq(&active_agent.execution(), &owner_execution));
        let requested_winner = match &active_agent {
            Some(_) => {
                let infrastructure = error
                    .downcast_ref::<WorkerExecutorError>()
                    .cloned()
                    .unwrap_or_else(|| WorkerExecutorError::runtime(error.to_string()));
                operation::OwnerFailureWinner::Infrastructure(infrastructure)
            }
            None => operation::OwnerFailureWinner::Lifecycle(InterruptKind::Interrupt(
                golem_common::model::Timestamp::now_utc(),
            )),
        };
        tracing::debug!(
            owner_id = %owner_id,
            owner_generation_active = active_agent.is_some(),
            requested_failure_kind = requested_winner.kind_label(),
            "Classifying accepted tool operation failure"
        );
        cleanup_operation.select_failure(requested_winner).await;
        let winner = owner_operations
            .selected_owner_failure()
            .expect("failed accepted tool operation must select an owner winner");
        let preserve_exact_trap = matches!(&winner, operation::OwnerFailureWinner::Trap(_));
        let owner_failure_cleanup = cleanup_operation.claim_owner_failure_cleanup();
        if let Some(owner_failure_cleanup) = owner_failure_cleanup {
            let interrupt_signal = primary.current_execution_interrupt_signal();
            let operations = owner_operations.clone();
            let cleanup = owner_operations.start_owner_failure_cleanup(
                owner_failure_cleanup,
                async move {
                    if let Some(active_agent) = active_agent {
                        active_agent.fence_entity_bodies(winner).await;
                    } else {
                        operations.close_failed_attachments();
                        operations.drain_owner_failure_lanes().await;
                    }
                    let settlement = match failed_resources {
                        Some(resources) => resources.resources.settle_after_parent_end().await,
                        None => Ok(()),
                    };
                    cleanup_operation.settle().await;
                    settlement
                },
                move || {
                    if let Some(signal) = interrupt_signal {
                        let _ = signal.send(InterruptKind::Interrupt(
                            golem_common::model::Timestamp::now_utc(),
                        ));
                    }
                },
            );
            if let Err(cleanup_error) = cleanup.await {
                *error = cleanup_error.into();
            }
        } else {
            if let Some(failed_resources) = failed_resources
                && let Err(settlement_error) =
                    failed_resources.resources.settle_after_parent_end().await
                && !preserve_exact_trap
            {
                *error = settlement_error.into();
            }
            cleanup_operation.settle().await;
        }
    }
    drop(reconstruction_hold);
    result
}

pub(super) async fn execute_accepted_tool_call_inner<Ctx>(
    accessor: &ToolCaller<Ctx>,
    accepted: AcceptedToolCall,
    endpoints: OwnedToolEndpoints,
    execution: Option<&ToolExecution>,
    failed_resources: Arc<Mutex<Option<FailedRetainedEntityResources>>>,
    mut completed_supervisor_started: Option<oneshot::Sender<()>>,
) -> anyhow::Result<ToolInvokeResponse>
where
    Ctx: WorkerCtx,
{
    let AcceptedToolCall {
        mut durability,
        operation,
        stdin: _,
        deferred_admission_inserted,
        mut incapable_lane_ticket,
        attachment_counterparty,
    } = accepted;
    let parent = durability.parent().clone();
    let start = durability.scope().invocation_id().start_index();
    let filesystem = durability.scope().activation().filesystem();
    let call_mode = durability.call_mode();
    let mut eager_deferred_cleanup = deferred_admission_inserted.then(|| {
        let table = accessor.execution.deferred_tool_admission();
        DeferredAdmissionCleanup {
            table,
            parent: parent.clone(),
            start: Some(start),
        }
    });
    if let Some(recorded) = durability
        .recorded_terminal_owned(&accessor.settlement)
        .await?
    {
        if recorded_tool_body_is_skipped(&recorded)? {
            return execute_recorded_skipped_tool_call(
                accessor,
                durability,
                operation,
                endpoints,
                recorded,
                deferred_admission_inserted,
                &mut eager_deferred_cleanup,
            )
            .await;
        }
        if let RecordedEntityTerminal::Completed(response) = &recorded
            && let Some(refusal) = terminal_from_response(response)?.admission_refusal.clone()
        {
            let replay = &accessor.settlement.replay;
            let (terminal_index, _) = replay
                .visible_terminal_record(start)
                .await?
                .ok_or_else(|| anyhow!("recorded tool terminal has no visible End"))?;
            operation.install_recorded_admission_refusal(refusal, terminal_index);
        }
    }
    let replaying_incomplete = durability.scope().mode()
        == golem_common::model::entity::InvocationExecutionMode::ReplayingIncomplete;
    let pre_body_live_admission_cancelled = match durability
        .enter_incomplete_live_repair_before_body_owned(&accessor.settlement)
        .await?
    {
        IncompleteLiveRepairBeforeBody::Ready(ready) => {
            durability = ready;
            false
        }
        IncompleteLiveRepairBeforeBody::Cancelled(cancelled) => {
            durability = cancelled;
            true
        }
    };
    let execution_mode = durability.scope().mode();
    let OwnedToolEndpoints {
        stdin,
        stdout,
        stderr,
    } = endpoints;
    let active_agents = accessor.primary.active_agents();
    let owner_id = accessor.primary.owned_agent_id().clone();
    let owner_component_metadata = accessor.metadata.clone();
    let owner_execution = accessor.execution.clone();
    let lane = owner_execution.lane();
    let owner_operations = owner_execution.tool_operations();
    let deferred_admission = owner_execution.deferred_tool_admission();
    let stdin_controller = stdin.as_ref().and_then(ToolBodyStdin::controller);
    let stdout_controller = stdout.as_ref().map(ToolOutputWriterEntry::controller);
    let stderr_controller = stderr.as_ref().map(ToolOutputWriterEntry::controller);
    let stdout_completion_only = stdout
        .as_ref()
        .is_some_and(ToolOutputWriterEntry::completion_only);
    let stderr_completion_only = stderr
        .as_ref()
        .is_some_and(ToolOutputWriterEntry::completion_only);
    // Guest counterparties need the same owner lane as a capable body. Session journals
    // progress independently, so their streams stay live while the body owns the lane.
    let stage_attachments = filesystem
        == golem_common::model::entity::FilesystemCapability::Capable
        && attachment_counterparty == ToolAttachmentCounterparty::Guest;
    let mut deferred_cleanup =
        if filesystem == golem_common::model::entity::FilesystemCapability::Capable {
            if !deferred_admission_inserted && !deferred_admission.insert(parent.clone(), start) {
                return Err(anyhow!(
                    "capable tool operation was already registered for deferred admission"
                ));
            }
            eager_deferred_cleanup.take().or_else(|| {
                Some(DeferredAdmissionCleanup {
                    table: deferred_admission.clone(),
                    parent: parent.clone(),
                    start: Some(start),
                })
            })
        } else {
            None
        };

    if pre_body_live_admission_cancelled {
        return Box::pin(cancel_tool_before_body(
            accessor,
            durability,
            operation,
            &deferred_admission,
            &mut deferred_cleanup,
            &parent,
            start,
            operation::BodyAdmissionState::Staging,
            stdin_controller.as_ref(),
            stdout_controller.as_ref(),
            stderr_controller.as_ref(),
        ))
        .await;
    }

    if filesystem == golem_common::model::entity::FilesystemCapability::Incapable
        && execution.is_some_and(ToolExecution::is_cancelled)
    {
        if !operation.transition_admission(
            operation::BodyAdmissionState::Staging,
            operation::BodyAdmissionState::SettledWithoutBody,
        ) {
            return Err(anyhow!(
                "tool invocation was fenced before pre-dispatch cancellation"
            ));
        }
        return cancel_registered_tool_before_body(
            accessor,
            durability,
            operation,
            stdin_controller.as_ref(),
            stdout_controller.as_ref(),
            stderr_controller.as_ref(),
        )
        .await;
    }

    if execution_mode == InvocationExecutionMode::Live {
        match operation.activate_live_attachment_memory_accounting().await {
            operation::ToolLiveAdmissionOutcome::Admitted => {}
            operation::ToolLiveAdmissionOutcome::ResourceExhausted => {
                settle_resource_exhausted_admission(
                    &operation,
                    filesystem,
                    &deferred_admission,
                    &mut deferred_cleanup,
                    &parent,
                    start,
                    call_mode,
                )?;
                return complete_resource_exhausted_without_body(
                    accessor,
                    durability,
                    operation,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                    stderr_controller.as_ref(),
                )
                .await;
            }
            operation::ToolLiveAdmissionOutcome::Cancelled => {
                if filesystem == golem_common::model::entity::FilesystemCapability::Capable {
                    return Box::pin(cancel_tool_before_body(
                        accessor,
                        durability,
                        operation,
                        &deferred_admission,
                        &mut deferred_cleanup,
                        &parent,
                        start,
                        operation::BodyAdmissionState::Staging,
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                        stderr_controller.as_ref(),
                    ))
                    .await;
                }
                if !operation.transition_admission(
                    operation::BodyAdmissionState::Staging,
                    operation::BodyAdmissionState::SettledWithoutBody,
                ) {
                    return Err(anyhow!(
                        "tool invocation was fenced before pre-dispatch cancellation"
                    ));
                }
                return Box::pin(cancel_registered_tool_before_body(
                    accessor,
                    durability,
                    operation,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                    stderr_controller.as_ref(),
                ))
                .await;
            }
            operation::ToolLiveAdmissionOutcome::Fenced => {
                return Err(anyhow!(
                    "tool invocation was fenced during live-memory admission"
                ));
            }
        }
    }

    if let Some(stdin) = &stdin_controller {
        if stage_attachments {
            stdin.configure_completion();
        } else {
            stdin.configure_live();
        }
    }
    if let Some(stdout) = &stdout_controller {
        if stage_attachments || stdout_completion_only {
            stdout.configure_completion();
        } else {
            stdout.configure_live();
        }
    }
    if let Some(stderr) = &stderr_controller {
        if stage_attachments || stderr_completion_only {
            stderr.configure_completion();
        } else {
            stderr.configure_live();
        }
    }
    match filesystem {
        golem_common::model::entity::FilesystemCapability::Incapable => {
            if !operation.transition_admission(
                operation::BodyAdmissionState::Staging,
                operation::BodyAdmissionState::Running,
            ) {
                return Err(anyhow!("tool invocation was fenced before dispatch"));
            }
        }
        golem_common::model::entity::FilesystemCapability::Capable => {
            if execution.is_some_and(ToolExecution::is_cancelled) {
                return cancel_tool_before_body(
                    accessor,
                    durability,
                    operation,
                    &deferred_admission,
                    &mut deferred_cleanup,
                    &parent,
                    start,
                    operation::BodyAdmissionState::Staging,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                    stderr_controller.as_ref(),
                )
                .await;
            }
            if stage_attachments && let Some(stdin) = &stdin_controller {
                let stdin_observer = stdin.observer();
                let terminal = match execution {
                    Some(execution) => {
                        tokio::select! {
                            terminal = stdin_observer.wait_terminal() => terminal,
                            _ = execution.cancelled() => {
                                return cancel_tool_before_body(
                                    accessor,
                                    durability,
                                    operation,
                                    &deferred_admission,
                                    &mut deferred_cleanup,
                                    &parent,
                                    start,
                                    operation::BodyAdmissionState::Staging,
                                    stdin_controller.as_ref(),
                                    stdout_controller.as_ref(),
                                    stderr_controller.as_ref(),
                                ).await;
                            }
                        }
                    }
                    None => stdin_observer.wait_terminal().await,
                };
                if execution.is_some_and(ToolExecution::is_cancelled) {
                    return cancel_tool_before_body(
                        accessor,
                        durability,
                        operation,
                        &deferred_admission,
                        &mut deferred_cleanup,
                        &parent,
                        start,
                        operation::BodyAdmissionState::Staging,
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                        stderr_controller.as_ref(),
                    )
                    .await;
                }
                let host_resource_exhausted = stdin
                    .observer()
                    .terminal_snapshot()
                    .is_some_and(|terminal| terminal.host_resource_exhausted);
                if host_resource_exhausted {
                    settle_resource_exhausted_admission(
                        &operation,
                        filesystem,
                        &deferred_admission,
                        &mut deferred_cleanup,
                        &parent,
                        start,
                        call_mode,
                    )?;
                    return complete_resource_exhausted_without_body(
                        accessor,
                        durability,
                        operation,
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                        stderr_controller.as_ref(),
                    )
                    .await;
                }
                tracing::debug!(
                    terminal = ?attachment::terminal_metadata(&terminal),
                    "Capable tool stdin staging reached its terminal"
                );
                stdin.publish_completion();
            }
            if !operation.transition_admission(
                operation::BodyAdmissionState::Staging,
                operation::BodyAdmissionState::Ready,
            ) || !deferred_admission.settle_staging(
                &parent,
                start,
                operation::DeferredAdmissionReadiness::Ready,
            ) {
                return Err(anyhow!(
                    "tool invocation was fenced before lane registration"
                ));
            }
            if call_mode == EntityCallMode::Synchronous {
                deferred_admission
                    .wait_and_register_cohort(
                        &parent,
                        operation::DeferredAdmissionCohort::ResultAwait(start),
                        &[start],
                        &owner_operations,
                        &lane,
                    )
                    .await?;
                deferred_cleanup
                    .as_mut()
                    .expect("capable call must own deferred admission")
                    .disarm();
            } else {
                if execution.is_none() {
                    return Err(anyhow!(
                        "detached capable tool operation has no execution owner"
                    ));
                }
                let execution = execution.expect("validated detached execution owner");
                let registered = tokio::select! {
                    registered = operation.wait_until_registered() => registered,
                    _ = execution.cancelled() => {
                        return if operation.admission_if_active()
                            == Some(operation::BodyAdmissionState::Ready)
                        {
                            cancel_tool_before_body(
                                accessor,
                                durability,
                                operation,
                                &deferred_admission,
                                &mut deferred_cleanup,
                                &parent,
                                start,
                                operation::BodyAdmissionState::Ready,
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                                stderr_controller.as_ref(),
                            ).await
                        } else {
                            deferred_cleanup
                                .as_mut()
                                .expect("capable call must own deferred admission")
                                .disarm();
                            cancel_registered_tool_before_body(
                                accessor,
                                durability,
                                operation,
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                                stderr_controller.as_ref(),
                            ).await
                        };
                    }
                };
                if !registered {
                    return Err(anyhow!("tool invocation was fenced before registration"));
                }
                deferred_cleanup
                    .as_mut()
                    .expect("capable call must own deferred admission")
                    .disarm();
            }
            let acquired = match execution {
                Some(execution) => {
                    tokio::select! {
                        biased;
                        _ = execution.cancelled() => {
                            return cancel_registered_tool_before_body(
                                accessor,
                                durability,
                                operation,
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                                stderr_controller.as_ref(),
                            ).await;
                        }
                        acquired = operation.acquire_registered_body() => acquired?,
                    }
                }
                None => operation.acquire_registered_body().await?,
            };
            if !acquired {
                return Err(anyhow!("tool invocation was fenced before lane grant"));
            }
            if execution.is_some_and(ToolExecution::is_cancelled) {
                return cancel_registered_tool_before_body(
                    accessor,
                    durability,
                    operation,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                    stderr_controller.as_ref(),
                )
                .await;
            }
        }
    }

    let active_agent = active_agents
        .try_get_active_agent(&owner_id)
        .await
        .filter(|active_agent| Arc::ptr_eq(&active_agent.execution(), &owner_execution))
        .ok_or_else(|| anyhow!("active owner disappeared before tool sidecar dispatch"))?;
    let context = operation.context();
    let position = durability.resolved_position().clone();
    let leaf_position = (position.plan().len() - 1) as u32;
    let AgentEntity::Tool(tool_name) = position
        .plan()
        .layer(leaf_position)
        .map_err(anyhow::Error::msg)?
        .activation()
        .entity()
    else {
        return Err(anyhow!("tool invocation plan has no terminal tool"));
    };
    let EntityInvocationDescriptor::Tool(descriptor) = &context.descriptor;
    let scope = durability.scope().clone();
    let result_streams = streams::session(&active_agent.primary(), &scope).await?;
    let caller_revision = accessor.revision;
    let has_input_streams =
        crate::durable_host::schema_value_stream::contains_stream(context.input.value());
    let input =
        streams::materialize_input(&result_streams, caller_revision, &context.input).await?;
    let sidecar = ToolSidecarInvocation {
        tool_name: tool_name.to_string(),
        position,
        command_path: descriptor.command_path.clone(),
        input,
        stdin,
        stdout,
        stderr,
        principal: context.principal.clone(),
        output_contract: descriptor.output_contract.clone(),
        result_streams: result_streams.clone(),
    };
    let operation_for_body = operation.clone();
    let terminal = Arc::new(std::sync::Mutex::new(None));
    let terminal_for_finalize = terminal.clone();
    let operation_for_finalize = operation.clone();
    let terminal_for_completed_failure = terminal.clone();
    let operation_for_completed_failure = operation.clone();
    let cancellation = execution
        .filter(|execution| execution.cancellable)
        .map(|execution| execution.cancel.clone());
    let scope_interest = durability.scope_interest();
    let finalize = move |result: Result<
        EntityBodyOutcome<HostResponseEntityInvocation>,
        WorkerExecutorError,
    >| {
        let operation = operation_for_finalize;
        async move {
            let _scope_interest = scope_interest;
            let result = match result {
                Ok(EntityBodyOutcome::Completed(response)) => Ok(response),
                Ok(EntityBodyOutcome::Cancelled) => return Ok(EntityBodyOutcome::Cancelled),
                Err(error) => Err(error),
            };
            let (result, admission_rejection_preselected) = match result {
                Err(error)
                    if crate::durable_host::is_tool_attachment_live_admission_rejection(&error) =>
                {
                    let rejection = operation
                        .take_live_attachment_admission_rejection()
                        .await
                        .ok_or_else(|| {
                            WorkerExecutorError::runtime(
                                "tool attachment admission rejection lost its operation marker",
                            )
                        })?;
                    (resource_exhausted_after_body().await, rejection.preselected)
                }
                Err(error) => {
                    rollback_live_attachment_admission_rejection(&operation).await;
                    (Err(error), false)
                }
                result => (result, false),
            };
            let result = match (result, operation.observed_admission_refusal()) {
                (Ok(response), Some(refusal)) => match terminal_from_response(&response) {
                    Ok(selected) => {
                        let mut selected = (*selected).clone();
                        selected.admission_refusal = Some(refusal);
                        encode_tool_terminal(
                            selected,
                            "failed to encode observed admission refusal",
                        )
                        .await
                    }
                    Err(error) => Err(error),
                },
                (result, _) => result,
            };
            if result.is_err() && admission_rejection_preselected {
                operation
                    .resolve_ordinary(
                        Arc::new(resource_exhausted_terminal(
                            SerializableEntityBodyExecution::Executed,
                        )),
                        false,
                    )
                    .await;
            }
            match &result {
                Ok(response) => {
                    let selected = match terminal_from_response(response) {
                        Ok(selected) => selected,
                        Err(error) => {
                            if admission_rejection_preselected {
                                operation
                                    .resolve_ordinary(
                                        Arc::new(resource_exhausted_terminal(
                                            SerializableEntityBodyExecution::Executed,
                                        )),
                                        false,
                                    )
                                    .await;
                            }
                            return Err(error);
                        }
                    };
                    if !admission_rejection_preselected && !operation.begin_ordinary() {
                        if operation.cancellation_selected_if_active() {
                            return Ok(EntityBodyOutcome::Cancelled);
                        }
                        return Err(WorkerExecutorError::runtime(
                            "tool operation terminal lost owner arbitration",
                        ));
                    }
                    *terminal_for_finalize.lock().unwrap() = Some(selected);
                }
                Err(error) => {
                    let _ = operation.select_infrastructure(error.clone()).await;
                }
            }
            result.map(EntityBodyOutcome::Completed)
        }
    };
    let body = match scope.activation().source() {
        golem_common::model::entity::EntityActivationSource::Component { .. } => {
            let invoke = ToolSidecarBody {
                invocation: sidecar,
                operation: operation_for_body,
                cancellation,
            };
            match filesystem {
                golem_common::model::entity::FilesystemCapability::Incapable => active_agent
                    .start_registered_entity_invocation(
                        scope,
                        owner_component_metadata,
                        call_mode,
                        incapable_lane_ticket
                            .take()
                            .expect("incapable tool body must own its admission ticket"),
                        invoke,
                        finalize,
                    ),
                golem_common::model::entity::FilesystemCapability::Capable => active_agent
                    .start_pre_acquired_entity_invocation(
                        scope,
                        owner_component_metadata,
                        call_mode,
                        invoke,
                        finalize,
                    ),
            }
        }
        golem_common::model::entity::EntityActivationSource::Host { .. } => {
            let worker = active_agent.primary();
            match filesystem {
                golem_common::model::entity::FilesystemCapability::Incapable => active_agent
                    .start_registered_native_entity_invocation(
                        scope,
                        call_mode,
                        incapable_lane_ticket
                            .take()
                            .expect("incapable native tool body must own its admission ticket"),
                        move |scope, registration, runner_abort| {
                            Box::pin(invoke_native_tool(
                                worker,
                                owner_component_metadata,
                                scope,
                                registration,
                                sidecar,
                                operation_for_body,
                                cancellation,
                                runner_abort,
                            ))
                        },
                        finalize,
                    ),
                golem_common::model::entity::FilesystemCapability::Capable => active_agent
                    .start_native_entity_invocation(
                        None,
                        scope,
                        call_mode,
                        move |scope, registration, runner_abort| {
                            Box::pin(invoke_native_tool(
                                worker,
                                owner_component_metadata,
                                scope,
                                registration,
                                sidecar,
                                operation_for_body,
                                cancellation,
                                runner_abort,
                            ))
                        },
                        finalize,
                    ),
            }
        }
    };
    let body = match body {
        Ok(body) => body,
        Err(error) => {
            rollback_live_attachment_admission_rejection(&operation).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            return Err(error.into());
        }
    };
    if replaying_incomplete && let Some(started) = completed_supervisor_started.take() {
        let _ = started.send(());
    }
    let outcome = durability
        .drive_owned(
            &accessor.settlement,
            body,
            execution
                .filter(|execution| execution.cancellable)
                .map(|execution| execution.cancel.clone()),
            move || {
                if let Some(started) = completed_supervisor_started {
                    let _ = started.send(());
                }
            },
            {
                let operation = operation.clone();
                move || {
                    operation.begin_cancel();
                }
            },
            move |error| async move {
                let terminal = terminal_for_completed_failure.lock().unwrap().take();
                if let Some(terminal) = terminal {
                    operation_for_completed_failure
                        .resolve_ordinary(terminal, false)
                        .await;
                } else {
                    rollback_live_attachment_admission_rejection(&operation_for_completed_failure)
                        .await;
                }
                let _ = operation_for_completed_failure
                    .select_infrastructure(error)
                    .await;
            },
        )
        .await;
    match outcome {
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(
            response,
            resources,
        )) => {
            let live_admission_rejected = operation.live_attachment_admission_was_rejected().await;
            let terminal = terminal.lock().unwrap().take().ok_or_else(|| {
                anyhow!("completed tool body did not select an operation terminal")
            })?;
            let trapped = terminal.body_execution == SerializableEntityBodyExecution::Trapped;
            let failure = skipped_attachment_failure(&terminal);
            let resources = match resources {
                Some(mut retained) => {
                    if let Err(error) = retained.prepare_parent_end().await {
                        operation.resolve_ordinary(terminal, false).await;
                        *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                            resources: retained,
                            parent_end_attempted: true,
                        });
                        return Err(error.into());
                    }
                    Some(retained)
                }
                None => None,
            };
            operation.resolve_ordinary(terminal, true).await;
            if live_admission_rejected || trapped {
                if let Err(error) = complete_owner_attachment_terminals(
                    accessor,
                    &operation,
                    start,
                    [
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                        stderr_controller.as_ref(),
                    ],
                    failure,
                )
                .await
                {
                    if let Some(retained) = resources {
                        *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                            resources: retained,
                            parent_end_attempted: true,
                        });
                    }
                    return Err(error);
                }
                if live_admission_rejected {
                    operation.complete_rejected_live_attachment_memory_accounting();
                }
            }
            if let Some(resources) = resources {
                resources.settle_after_parent_end().await?;
            }
            operation.settle().await;
            if has_input_streams
                || result_streams
                    .persisted_result()
                    .await
                    .map_err(SessionError::into_trap)?
                    .is_some()
            {
                result_streams
                    .complete()
                    .await
                    .map_err(SessionError::into_trap)?;
            }
            publish_output_completions(
                stage_attachments,
                stdout_completion_only,
                stderr_completion_only,
                stdout_controller.as_ref(),
                stderr_controller.as_ref(),
            );
            streams::restore_response(&result_streams, decode_tool_terminal(*response)?)
                .await
                .map_err(Into::into)
        }
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
            response,
            resources,
        )) => {
            let live_admission_rejected = operation.live_attachment_admission_was_rejected().await;
            let response_terminal = terminal_from_response(&response)?;
            let no_body =
                response_terminal.body_execution == SerializableEntityBodyExecution::Skipped;
            let selected_terminal = terminal.lock().unwrap().take();
            if let Some(terminal) = selected_terminal {
                operation.resolve_ordinary(terminal, false).await;
            }
            let _ = operation.begin_cancel();
            let resources = match resources {
                Some(mut retained) => {
                    if let Err(error) = retained.prepare_parent_end().await {
                        *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                            resources: retained,
                            parent_end_attempted: true,
                        });
                        return Err(error.into());
                    }
                    Some(retained)
                }
                None => None,
            };
            operation.resolve_cancel(true).await;
            if let Some(resources) = resources {
                resources.settle_after_parent_end().await?;
            }
            if live_admission_rejected {
                let failure = skipped_attachment_failure(&response_terminal);
                complete_owner_attachment_terminals(
                    accessor,
                    &operation,
                    start,
                    [
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                        stderr_controller.as_ref(),
                    ],
                    failure,
                )
                .await?;
            }
            if has_input_streams
                || result_streams
                    .persisted_result()
                    .await
                    .map_err(SessionError::into_trap)?
                    .is_some()
            {
                result_streams
                    .fail("tool invocation was cancelled".to_string())
                    .await
                    .map_err(SessionError::into_trap)?;
            }
            operation.settle().await;
            if no_body {
                publish_no_body_terminals(
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                    stderr_controller.as_ref(),
                );
            } else {
                publish_output_completions(
                    stage_attachments,
                    stdout_completion_only,
                    stderr_completion_only,
                    stdout_controller.as_ref(),
                    stderr_controller.as_ref(),
                );
            }
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Err(failure) => {
            let selected_terminal = terminal.lock().unwrap().take();
            if let Some(terminal) = selected_terminal {
                operation.resolve_ordinary(terminal, false).await;
            } else {
                rollback_live_attachment_admission_rejection(&operation).await;
            }
            if let Some(resources) = failure.resources {
                *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                    resources,
                    parent_end_attempted: false,
                });
            }
            Err(failure.error.into())
        }
    }
}
