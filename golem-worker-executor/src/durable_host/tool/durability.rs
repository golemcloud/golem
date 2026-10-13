// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::attachment::{
    AttachmentController, AttachmentWriteAcceptance, AttachmentWriter, PreparedAttachmentTerminal,
};
use crate::durable_host::concurrent::{Cancellable, DeferredCallReplayOutcome, DurableCallSession};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError,
};
use crate::services::oplog::OplogOps;
use crate::services::{HasActiveAgents, HasWorker};
use crate::workerctx::{ReplayAdmissionStage, WorkerCtx};
use golem_common::model::Timestamp;
use golem_common::model::oplog::attachment as model;
use golem_common::model::oplog::host_functions::GolemToolOutputWrite;
use golem_common::model::oplog::payload::tool_attachment::{
    ToolAttachmentWriteOutcome, ToolAttachmentWriteRequest,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestToolAttachmentWrite, HostResponseToolAttachmentWrite,
    OplogEntry,
};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::future::Future;
use std::pin::Pin;
use wasmtime::component::{Accessor, HasSelf};

pub(super) async fn write<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    bytes: Vec<u8>,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    let Some(identity) = writer.identity() else {
        return Ok(writer.write(bytes).await);
    };
    let request = ToolAttachmentWriteRequest::new(identity, &bytes);
    let mut call = DurableCallSession::<GolemToolOutputWrite, Cancellable>::start_access(
        accessor,
        accessor.getter(),
        HostRequestToolAttachmentWrite {
            request: request.clone(),
        },
        DurableFunctionType::WriteLocal,
    )
    .await?;
    let start = call.start_index();
    if !call.is_live() {
        let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
        let OplogEntry::Start {
            request: Some(recorded),
            ..
        } = oplog.read(start).await
        else {
            return Err(call.trap(anyhow::anyhow!("tool output write has no recorded request")));
        };
        let recorded = HostRequestToolAttachmentWrite::try_from(
            oplog
                .download_payload(recorded)
                .await
                .map_err(anyhow::Error::msg)?,
        )
        .map_err(anyhow::Error::msg)?;
        if recorded.request != request {
            return Err(call.trap(anyhow::anyhow!(
                "regenerated tool output differs from its recorded write at {start}"
            )));
        }
        match call
            .replay_access_deferred(accessor, accessor.getter())
            .await?
        {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                anyhow::ensure!(
                    response.outcome.validate(&request),
                    "invalid tool output write outcome at {start}"
                );
                if response.outcome == ToolAttachmentWriteOutcome::Accepted {
                    let source_end = accessor.with(|mut access| {
                        access
                            .get()
                            .state
                            .replay_state
                            .accepted_tool_attachment_write_end(start)
                    })?;
                    writer
                        .restore_write(bytes, source_end)
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "cannot restore accepted tool output at {start}: {error:?}"
                            )
                        })?;
                    accessor.with(|mut access| {
                        access
                            .get()
                            .state
                            .replay_state
                            .complete_tool_attachment_reconstruction(start)
                    });
                } else if response.outcome == ToolAttachmentWriteOutcome::HostResourceExhausted {
                    let end = accessor.with(|mut access| {
                        access
                            .get()
                            .state
                            .replay_state
                            .selected_tool_attachment_terminal_end(
                                start,
                                identity,
                                &model::ToolAttachmentTerminal::HostResourceExhausted,
                            )
                    })?;
                    writer.restore_terminal(
                        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
                        true,
                        end,
                    )?;
                }
                let result = response.outcome.result().map_err(write_error_from_model);
                if delivery.is_replay_discarded() {
                    std::future::pending::<()>().await;
                }
                delivery.deliver_at_accessor_terminal(accessor).await?;
                return Ok(result);
            }
            DeferredCallReplayOutcome::Incomplete(live) => call = live,
        }
    }
    if !call.is_persisted() {
        let result = writer.write(bytes).await;
        let outcome = outcome_for_result(&result, request.length == 0);
        let (_, delivery) = call
            .complete_access_deferred(
                accessor,
                accessor.getter(),
                HostResponseToolAttachmentWrite { outcome },
            )
            .await?;
        delivery.deliver_at_accessor_terminal(accessor).await?;
        return Ok(result);
    }
    let mut prepared = match writer.prepare_write(bytes).await {
        Ok(Some(prepared)) => prepared,
        result => {
            let result = result.map(|_| ());
            let outcome = outcome_for_result(&result, request.length == 0);
            let (_, delivery) = call
                .complete_access_deferred(
                    accessor,
                    accessor.getter(),
                    HostResponseToolAttachmentWrite { outcome },
                )
                .await?;
            delivery.deliver_at_accessor_terminal(accessor).await?;
            return Ok(result);
        }
    };
    loop {
        prepared.wait_ready().await;
        let mut selected = None;
        let failed = publication_failure(accessor, controller.clone())?;
        let interrupt = accessor.with(|mut access| access.get().create_interrupt_signal());
        let completing = call.complete_access_deferred_with_effect(
            accessor,
            accessor.getter(),
            HostResponseToolAttachmentWrite {
                outcome: ToolAttachmentWriteOutcome::Accepted,
            },
            |_, enqueue| {
                selected = Some(prepared.accept(enqueue));
            },
            |publication, end| publication.publish(Some(model::AcceptedAttachmentWriteEnd(end))),
            failed,
        );
        let completed = tokio::select! {
            completed = completing => completed?,
            interruption = interrupt => return Err(interruption.into()),
        };
        match completed {
            Ok((_, delivery)) => {
                if let Some(hook) = accessor.with(|mut access| access.get().replay_admission_hook())
                {
                    let interrupt =
                        accessor.with(|mut access| access.get().create_interrupt_signal());
                    tokio::select! {
                        _ = hook.before_replay_access_start(
                        "golem::tool::streams::tool-output-writer::write",
                        ReplayAdmissionStage::AfterToolOutputAccepted,
                        ) => {},
                        interruption = interrupt => return Err(interruption.into()),
                    }
                }
                delivery.deliver_at_accessor_terminal(accessor).await?;
                return Ok(Ok(()));
            }
            Err((unfinished, _)) => call = unfinished,
        }
        let result = match selected.expect("acceptance callback must select an outcome") {
            Ok(AttachmentWriteAcceptance::Retry) => continue,
            Ok(AttachmentWriteAcceptance::ResourceExhausted(prepared)) => {
                return complete_resource_exhaustion(accessor, call, prepared, controller).await;
            }
            Ok(AttachmentWriteAcceptance::Discarded) => Ok(()),
            Err(error) => Err(error),
            Ok(AttachmentWriteAcceptance::Accepted(())) => {
                unreachable!("accepted write owns its End")
            }
        };
        let outcome = outcome_for_result(&result, false);
        let (_, delivery) = call
            .complete_access_deferred(
                accessor,
                accessor.getter(),
                HostResponseToolAttachmentWrite { outcome },
            )
            .await?;
        delivery.deliver_at_accessor_terminal(accessor).await?;
        return Ok(result);
    }
}

async fn complete_resource_exhaustion<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut call: DurableCallSession<GolemToolOutputWrite, Cancellable>,
    prepared: PreparedAttachmentTerminal,
    controller: AttachmentController,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    let failed = publication_failure(accessor, controller)?;
    let mut selection = None;
    let interrupt = accessor.with(|mut access| access.get().create_interrupt_signal());
    let completing = call.complete_access_deferred_with_effect(
        accessor,
        accessor.getter(),
        HostResponseToolAttachmentWrite {
            outcome: ToolAttachmentWriteOutcome::HostResourceExhausted,
        },
        |_, enqueue| selection = Some(prepared.accept(enqueue)),
        |publication, end| publication.publish(end),
        failed,
    );
    let completed = tokio::select! {
        completed = completing => completed?,
        interruption = interrupt => return Err(interruption.into()),
    };
    let exhausted = StreamWriteError::Closed(ByteStreamCloseCause::Failed(
        ByteStreamFailure::ResourceExhausted,
    ));
    match completed {
        Ok((_, delivery)) => {
            delivery.deliver_at_accessor_terminal(accessor).await?;
            return Ok(Err(exhausted));
        }
        Err((unfinished, _)) => call = unfinished,
    }
    let error = match selection.expect("exhaustion acceptance must select an outcome") {
        Ok(None) => exhausted,
        Err(error) => error,
        Ok(Some(())) => unreachable!("selected exhaustion owns its End"),
    };
    let (_, delivery) = call
        .complete_access_deferred(
            accessor,
            accessor.getter(),
            HostResponseToolAttachmentWrite {
                outcome: ToolAttachmentWriteOutcome::Rejected(error_to_model(&error)),
            },
        )
        .await?;
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(Err(error))
}

pub(super) async fn finish<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    let Some(attachment) = writer.identity() else {
        return Ok(writer.finish());
    };
    terminal::<golem_common::model::oplog::host_functions::GolemToolOutputFinish, U, Ctx>(
        accessor,
        writer,
        controller,
        golem_common::model::oplog::HostRequestToolAttachmentEndpoint { attachment },
        None,
    )
    .await
}

pub(super) async fn fail<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    reason: ByteStreamFailure,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    let Some(attachment) = writer.identity() else {
        return Ok(writer.fail(reason));
    };
    terminal::<golem_common::model::oplog::host_functions::GolemToolOutputFail, U, Ctx>(
        accessor,
        writer,
        controller,
        golem_common::model::oplog::HostRequestToolAttachmentFail {
            attachment,
            reason: failure_to_model(&reason),
        },
        Some(reason),
    )
    .await
}

async fn terminal<Pair, U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    request: Pair::Req,
    reason: Option<ByteStreamFailure>,
) -> anyhow::Result<Result<(), StreamWriteError>>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Resp = golem_common::model::oplog::HostResponseToolAttachmentTerminalResult,
        >,
{
    use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentTerminalOutcome;
    use golem_common::model::oplog::{HostRequest, HostResponseToolAttachmentTerminalResult};
    let attachment = writer.identity().expect("durable terminal has an identity");
    let cause = AttachmentWriter::effective_terminal(reason.clone());
    let effective = model::ToolAttachmentTerminal::Application(cause_to_model(&cause));
    let mut call = DurableCallSession::<Pair, Cancellable>::start_access(
        accessor,
        accessor.getter(),
        request,
        DurableFunctionType::WriteLocal,
    )
    .await?;
    let start = call.start_index();
    if !call.is_live() {
        let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
        let OplogEntry::Start {
            request: Some(recorded),
            ..
        } = oplog.read(start).await
        else {
            return Err(call.trap(anyhow::anyhow!(
                "tool output terminal has no recorded request"
            )));
        };
        let recorded = oplog
            .download_payload(recorded)
            .await
            .map_err(anyhow::Error::msg)?;
        let matches = match (recorded, &reason) {
            (HostRequest::ToolAttachmentEndpoint(recorded), None) => {
                recorded.attachment == attachment
            }
            (HostRequest::ToolAttachmentFail(recorded), Some(reason)) => {
                recorded.attachment == attachment && recorded.reason == failure_to_model(reason)
            }
            _ => false,
        };
        if !matches {
            return Err(call.trap(anyhow::anyhow!(
                "tool output terminal request differs at {start}"
            )));
        }
        match call
            .replay_access_deferred(accessor, accessor.getter())
            .await?
        {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                if let ToolAttachmentTerminalOutcome::Selected(selected) = &response.outcome {
                    anyhow::ensure!(
                        selected == &effective,
                        "invalid selected tool output terminal at {start}"
                    );
                    let end = accessor.with(|mut access| {
                        access
                            .get()
                            .state
                            .replay_state
                            .selected_tool_attachment_terminal_end(start, attachment, selected)
                    })?;
                    writer.restore_terminal(cause, false, end)?;
                }
                let result = response.outcome.result().map_err(write_error_from_model);
                if delivery.is_replay_discarded() {
                    std::future::pending::<()>().await;
                }
                delivery.deliver_at_accessor_terminal(accessor).await?;
                return Ok(result);
            }
            DeferredCallReplayOutcome::Incomplete(live) => call = live,
        }
    }
    let prepared = writer.prepare_terminal(cause);
    let outcome = match prepared {
        Ok(Ok(prepared)) if call.is_persisted() => {
            let failed = publication_failure(accessor, controller)?;
            let mut selection = None;
            let interrupt = accessor.with(|mut access| access.get().create_interrupt_signal());
            let completing = call.complete_access_deferred_with_effect(
                accessor,
                accessor.getter(),
                HostResponseToolAttachmentTerminalResult {
                    outcome: ToolAttachmentTerminalOutcome::Selected(effective),
                },
                |_, enqueue| {
                    selection = Some(prepared.accept(enqueue));
                },
                |publication, end| publication.publish(end),
                failed,
            );
            let completed = tokio::select! {
                completed = completing => completed?,
                interruption = interrupt => return Err(interruption.into()),
            };
            match completed {
                Ok((_, delivery)) => {
                    delivery.deliver_at_accessor_terminal(accessor).await?;
                    return Ok(Ok(()));
                }
                Err((unfinished, _)) => call = unfinished,
            }
            match selection.expect("terminal acceptance must select an outcome") {
                Ok(None) => ToolAttachmentTerminalOutcome::Unchanged,
                Err(error) => ToolAttachmentTerminalOutcome::Rejected(error_to_model(&error)),
                Ok(Some(())) => unreachable!("selected terminal owns its End"),
            }
        }
        Ok(Ok(prepared)) => {
            let result = prepared.publish_local();
            match result {
                Ok(()) => ToolAttachmentTerminalOutcome::Selected(effective),
                Err(error) => ToolAttachmentTerminalOutcome::Rejected(error_to_model(&error)),
            }
        }
        Ok(Err(())) => ToolAttachmentTerminalOutcome::Unchanged,
        Err(error) => ToolAttachmentTerminalOutcome::Rejected(error_to_model(&error)),
    };
    let result = outcome.result().map_err(write_error_from_model);
    let (_, delivery) = call
        .complete_access_deferred(
            accessor,
            accessor.getter(),
            HostResponseToolAttachmentTerminalResult { outcome },
        )
        .await?;
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(result)
}

fn failure_to_model(failure: &ByteStreamFailure) -> model::ByteStreamFailure {
    match failure {
        ByteStreamFailure::Cancelled => model::ByteStreamFailure::Cancelled,
        ByteStreamFailure::Abandoned => model::ByteStreamFailure::Abandoned,
        ByteStreamFailure::ResourceExhausted => model::ByteStreamFailure::ResourceExhausted,
        ByteStreamFailure::Failed(message) => model::ByteStreamFailure::Failed(message.clone()),
    }
}

fn cause_to_model(cause: &ByteStreamCloseCause) -> model::ByteStreamCloseCause {
    match cause {
        ByteStreamCloseCause::Finished => model::ByteStreamCloseCause::Finished,
        ByteStreamCloseCause::ConsumerCancelled => model::ByteStreamCloseCause::ConsumerCancelled,
        ByteStreamCloseCause::Failed(failure) => {
            model::ByteStreamCloseCause::Failed(failure_to_model(failure))
        }
    }
}

fn error_to_model(error: &StreamWriteError) -> model::StreamWriteError {
    match error {
        StreamWriteError::ConcurrentOperation => model::StreamWriteError::ConcurrentOperation,
        StreamWriteError::Closed(cause) => model::StreamWriteError::Closed(cause_to_model(cause)),
    }
}

fn publication_failure<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    controller: AttachmentController,
) -> anyhow::Result<
    impl FnOnce(WorkerExecutorError) -> Pin<Box<dyn Future<Output = ()> + Send>>
    + Send
    + 'static
    + use<U, Ctx>,
> {
    let (worker, operation, execution, owner_id) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.public_state.worker(),
            ctx.entity_tool_operation(),
            ctx.owner_execution.clone(),
            ctx.state.owned_agent_id.clone(),
        )
    });
    let operation = operation
        .ok_or_else(|| anyhow::anyhow!("tool output writer has no owning tool operation"))?;
    Ok(move |error| -> Pin<Box<dyn Future<Output = ()> + Send>> {
        controller.fence_owner();
        Box::pin(async move {
            let operations = execution.tool_operations();
            let active = worker
                .active_agents()
                .try_get_active_agent(&owner_id)
                .await
                .filter(|active| std::sync::Arc::ptr_eq(&active.execution(), &execution));
            operation.select_infrastructure(error).await;
            if let Some(cleanup) = operation.claim_owner_failure_cleanup() {
                let winner = operations
                    .selected_owner_failure()
                    .expect("failure cleanup has an owner winner");
                let interrupt = worker.current_execution_interrupt_signal();
                let cleanup_operations = operations.clone();
                let _ = operations
                    .start_owner_failure_cleanup(
                        cleanup,
                        async move {
                            if let Some(active) = active {
                                active.fence_entity_bodies(winner).await;
                            } else {
                                cleanup_operations.close_failed_attachments();
                                cleanup_operations.drain_owner_failure_lanes().await;
                            }
                            operation.settle().await;
                            Ok(())
                        },
                        move || {
                            if let Some(interrupt) = interrupt {
                                let _ =
                                    interrupt.send(InterruptKind::Interrupt(Timestamp::now_utc()));
                            }
                        },
                    )
                    .await;
            }
        })
    })
}

fn outcome_for_result(
    result: &Result<(), StreamWriteError>,
    empty: bool,
) -> ToolAttachmentWriteOutcome {
    match result {
        Ok(()) if empty => ToolAttachmentWriteOutcome::Empty,
        Ok(()) => ToolAttachmentWriteOutcome::Discarded,
        Err(error) => ToolAttachmentWriteOutcome::Rejected(error_to_model(error)),
    }
}

fn write_error_from_model(error: model::StreamWriteError) -> StreamWriteError {
    match error {
        model::StreamWriteError::ConcurrentOperation => StreamWriteError::ConcurrentOperation,
        model::StreamWriteError::Closed(cause) => StreamWriteError::Closed(match cause {
            model::ByteStreamCloseCause::Finished => ByteStreamCloseCause::Finished,
            model::ByteStreamCloseCause::ConsumerCancelled => {
                ByteStreamCloseCause::ConsumerCancelled
            }
            model::ByteStreamCloseCause::Failed(failure) => {
                ByteStreamCloseCause::Failed(match failure {
                    model::ByteStreamFailure::Cancelled => ByteStreamFailure::Cancelled,
                    model::ByteStreamFailure::Abandoned => ByteStreamFailure::Abandoned,
                    model::ByteStreamFailure::ResourceExhausted => {
                        ByteStreamFailure::ResourceExhausted
                    }
                    model::ByteStreamFailure::Failed(message) => ByteStreamFailure::Failed(message),
                })
            }
        }),
    }
}
