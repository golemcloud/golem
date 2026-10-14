// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::attachment::{
    AttachmentController, AttachmentObserver, AttachmentWriteAcceptance, AttachmentWriter,
    PreparedAttachmentTerminal,
};
use crate::durable_host::concurrent::{
    AccessClaimOptions, Cancellable, DeferredCallReplayOutcome, DurableCallSession,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError,
};
use crate::services::oplog::OplogOps;
use crate::services::{HasActiveAgents, HasWorker};
use crate::workerctx::{ReplayAdmissionStage, WorkerCtx};
use golem_common::model::Timestamp;
use golem_common::model::oplog::attachment as model;
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
use wasmtime::component::{Accessor, AccessorTask, HasData, HasSelf};

struct RestoreAttachmentWrite {
    writer: AttachmentWriter,
    bytes: Vec<u8>,
    start: golem_common::model::OplogIndex,
    replay: crate::durable_host::replay_state::ReplayState,
    activity: crate::durable_host::tail_work::TailActivity,
    permit: crate::durable_host::concurrent::LiveCallPermit,
    result: tokio::sync::oneshot::Sender<Result<(), WorkerExecutorError>>,
}

impl<U: 'static, D: HasData> AccessorTask<U, D> for RestoreAttachmentWrite {
    async fn run(self, _accessor: &Accessor<U, D>) -> wasmtime::Result<()> {
        let _activity = self.activity;
        let _permit = self.permit;
        let result = async {
            let source = self
                .replay
                .await_tool_attachment_acceptance(self.start)
                .await?;
            self.replay
                .validate_tool_attachment_regenerated_bytes(source, &self.bytes)
                .await?;
            self.writer
                .restore_write(self.bytes, source)
                .await
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "cannot restore accepted attachment at {}: {error}",
                        self.start
                    ))
                })?;
            self.replay
                .complete_tool_attachment_reconstruction(self.start)?;
            Ok::<_, WorkerExecutorError>(())
        }
        .await;
        let _ = self.result.send(result.clone());
        result.map_err(Into::into)
    }
}

pub(super) async fn write<Pair, U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    operation_ordinal: u64,
    bytes: Vec<u8>,
) -> anyhow::Result<Result<(), StreamWriteError>>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = HostRequestToolAttachmentWrite,
            Resp = HostResponseToolAttachmentWrite,
        >,
{
    let Some(identity) = writer.identity() else {
        return Ok(writer.write(bytes).await);
    };
    let request = ToolAttachmentWriteRequest::new(identity, operation_ordinal, &bytes);
    let host_request = HostRequestToolAttachmentWrite {
        request: request.clone(),
    };
    let mut call = DurableCallSession::<Pair, Cancellable>::start_access_with_options(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteLocal,
        AccessClaimOptions {
            request_identity: Some(host_request.clone().into()),
            ..Default::default()
        },
        async move |_| Ok(host_request),
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
                    let (result, restored) = tokio::sync::oneshot::channel();
                    let task = accessor.with(|mut access| {
                        let ctx = access.get();
                        RestoreAttachmentWrite {
                            writer,
                            bytes,
                            start,
                            replay: ctx.state.replay_state.clone(),
                            activity: ctx.tail_work_tracker().activity(),
                            permit: crate::durable_host::concurrent::LiveCallPermit::new(
                                ctx.state.live_host_call_counter(),
                            ),
                            result,
                        }
                    });
                    accessor.spawn(task);
                    restored.await??;
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
        let failed = publication_failure(accessor, controller.clone(), Pair::FQFN)?;
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
                        Pair::FQFN,
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

async fn complete_resource_exhaustion<Pair, U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut call: DurableCallSession<Pair, Cancellable>,
    prepared: PreparedAttachmentTerminal,
    controller: AttachmentController,
) -> anyhow::Result<Result<(), StreamWriteError>>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = HostRequestToolAttachmentWrite,
            Resp = HostResponseToolAttachmentWrite,
        >,
{
    let failed = publication_failure(accessor, controller, Pair::FQFN)?;
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

pub(super) async fn finish<Pair, U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    operation_ordinal: u64,
) -> anyhow::Result<Result<(), StreamWriteError>>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = golem_common::model::oplog::HostRequestToolAttachmentFinish,
            Resp = golem_common::model::oplog::HostResponseToolAttachmentTerminalResult,
        >,
{
    let Some(attachment) = writer.identity() else {
        return Ok(writer.finish());
    };
    terminal::<Pair, U, Ctx>(
        accessor,
        writer,
        controller,
        golem_common::model::oplog::HostRequestToolAttachmentFinish {
            attachment,
            operation_ordinal,
        },
        None,
    )
    .await
}

pub(super) async fn fail<Pair, U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    operation_ordinal: u64,
    reason: ByteStreamFailure,
) -> anyhow::Result<Result<(), StreamWriteError>>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = golem_common::model::oplog::HostRequestToolAttachmentFail,
            Resp = golem_common::model::oplog::HostResponseToolAttachmentTerminalResult,
        >,
{
    let Some(attachment) = writer.identity() else {
        return Ok(writer.fail(reason));
    };
    terminal::<Pair, U, Ctx>(
        accessor,
        writer,
        controller,
        golem_common::model::oplog::HostRequestToolAttachmentFail {
            attachment,
            operation_ordinal,
            reason: failure_to_model(&reason),
        },
        Some(reason),
    )
    .await
}

pub(super) async fn wait<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    observer: AttachmentObserver,
    operation_ordinal: u64,
) -> anyhow::Result<ByteStreamCloseCause> {
    use crate::durable_host::concurrent::CallReplayOutcome;
    use golem_common::model::oplog::host_functions::GolemToolStdinWait;
    use golem_common::model::oplog::{
        HostRequestToolAttachmentEndpoint, HostResponseToolAttachmentClosed,
    };

    let attachment = observer
        .identity()
        .expect("stdin observer has a constructor identity");
    let request = HostRequestToolAttachmentEndpoint {
        attachment,
        operation_ordinal,
    };
    let mut call =
        DurableCallSession::<GolemToolStdinWait, Cancellable>::start_access_with_options(
            accessor,
            accessor.getter(),
            DurableFunctionType::ReadLocal,
            AccessClaimOptions {
                request_identity: Some(request.clone().into()),
                ..Default::default()
            },
            async move |_| Ok(request),
        )
        .await?;
    if !call.is_live() {
        match call.replay_access(accessor, accessor.getter()).await? {
            CallReplayOutcome::Replayed(response) => return Ok(cause_from_model(response.cause)),
            CallReplayOutcome::Incomplete(live) => call = live,
        }
    }
    let response = call
        .run_live_action_access(accessor, accessor.getter(), async move || {
            Ok::<_, anyhow::Error>(HostResponseToolAttachmentClosed {
                cause: cause_to_model(&observer.wait_terminal().await),
            })
        })
        .await?;
    Ok(cause_from_model(response.cause))
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
    Pair::Req: Clone,
{
    use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentTerminalOutcome;
    use golem_common::model::oplog::{HostRequest, HostResponseToolAttachmentTerminalResult};
    let attachment = writer.identity().expect("durable terminal has an identity");
    let cause = AttachmentWriter::effective_terminal(reason.clone());
    let effective = model::ToolAttachmentTerminal::Application(cause_to_model(&cause));
    let expected: HostRequest = request.clone().into();
    let mut call = DurableCallSession::<Pair, Cancellable>::start_access_with_options(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteLocal,
        AccessClaimOptions {
            request_identity: Some(expected.clone()),
            ..Default::default()
        },
        async move |_| Ok(request),
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
        if recorded != expected {
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
            let failed = publication_failure(accessor, controller, Pair::FQFN)?;
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

pub(super) fn failure_to_model(failure: &ByteStreamFailure) -> model::ByteStreamFailure {
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

pub(super) fn install_cancellation_effect<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    operation: &super::operation::OwnerToolOperation,
) {
    use crate::durable_host::concurrent::{
        DropEvent, LiveCallPermit, MarkerReceipt, prepare_access_terminal_effect,
    };
    use crate::durable_host::durability::{DurableCallTrapContext, InFunctionRetryHost};
    use crate::services::oplog::CommitLevel;

    let context = PublicationFailureContext::new(ctx);
    let oplog = ctx.state.oplog.clone();
    let replay = ctx.state.replay_state.clone();
    let mode = ctx.entity_invocation_scope().map(|scope| scope.mode());
    let local_live = ctx.state.local_live_tail();
    let suppressed = ctx.state.durability_is_suppressed();
    let parent = ctx.entity_parent_start_index();
    let tracker = ctx.tail_work_tracker();
    let live_calls = ctx.state.live_host_call_counter();
    let cleanup = ctx
        .state
        .dropped_call_event_sender()
        .expect("worker cleanup queue");
    let trap = DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
        in_atomic_region: InFunctionRetryHost::in_atomic_region(ctx),
    };
    operation.set_cancellation_effect(Box::new(move |operation, controller| {
        let identity = controller.observer().identity();
        if suppressed || identity.is_none() {
            let _ = controller.cancel();
            return;
        }
        if !crate::durable_host::store_is_live(
            mode,
            local_live.load(std::sync::atomic::Ordering::Acquire),
            replay.is_live_published(),
        ) {
            return;
        }
        let activity = tracker.activity();
        let permit = LiveCallPermit::new(live_calls.clone());
        let setup = (|| {
            let recording = context.worker.runtime_append_tasks().register().ok_or(
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Jump,
                },
            )?;
            let worker = context.worker.clone();
            let failure = context.clone();
            let failed_controller = controller.clone();
            let failed_operation = operation.clone();
            prepare_access_terminal_effect(
                &context.worker.tasks,
                recording,
                async move {
                    worker
                        .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                        .await
                        .map(|_| ())
                        .map_err(WorkerExecutorError::from)
                },
                |publication: super::attachment::AttachmentTerminalPublication, index| {
                    publication.publish(index)
                },
                move |error| {
                    failure.fail(
                        error,
                        Some(failed_operation),
                        failed_controller,
                        "tool attachment cancellation",
                    )
                },
            )
        })();
        let terminal = match setup {
            Ok((handoff, terminal)) => {
                let accepted = controller.accept_cancellation(|publication| {
                    (
                        publication,
                        oplog.enqueue_add(OplogEntry::ToolAttachmentControl {
                            timestamp: Timestamp::now_utc(),
                            entity_parent_start_index: parent,
                            attachment: identity.unwrap(),
                            transition: model::ToolAttachmentControl::TerminalSelected(
                                model::ToolAttachmentTerminal::Application(
                                    model::ByteStreamCloseCause::Failed(
                                        model::ByteStreamFailure::Cancelled,
                                    ),
                                ),
                            ),
                        }),
                    )
                });
                let Some(accepted) = accepted else {
                    return;
                };
                assert!(handoff.send(accepted).is_ok());
                terminal
            }
            Err(error) => {
                let failed = context.clone().fail(
                    error.clone(),
                    Some(operation.clone()),
                    controller.clone(),
                    "tool attachment cancellation",
                );
                tokio::spawn(failed);
                tokio::spawn(async move { Err(error) })
            }
        };
        let (done, receipt) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = terminal.await.unwrap_or_else(|error| {
                Err(WorkerExecutorError::runtime(format!(
                    "attachment cancellation recorder stopped: {error}"
                )))
            });
            drop(activity);
            let _ = done.send(result);
        });
        let _ = cleanup.send(DropEvent::AwaitCompletionMarker {
            receipt: Some(MarkerReceipt::pending(receipt)),
            trap_context: trap,
            live_call_permit: Some(permit),
        });
    }));
}

fn publication_failure<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    controller: AttachmentController,
    function: &'static str,
) -> anyhow::Result<
    impl FnOnce(WorkerExecutorError) -> Pin<Box<dyn Future<Output = ()> + Send>>
    + Send
    + 'static
    + use<U, Ctx>,
> {
    let (context, operation) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            PublicationFailureContext::new(ctx),
            ctx.entity_tool_operation(),
        )
    });
    Ok(move |error| context.fail(error, operation, controller, function))
}

struct PublicationFailureContext<Ctx: WorkerCtx> {
    worker: std::sync::Arc<crate::worker::Worker<Ctx>>,
    execution: std::sync::Arc<crate::worker::instance::OwnerExecution>,
    owner_id: golem_common::model::OwnedAgentId,
    hook: Option<std::sync::Arc<dyn crate::workerctx::ReplayAdmissionHook>>,
}

impl<Ctx: WorkerCtx> Clone for PublicationFailureContext<Ctx> {
    fn clone(&self) -> Self {
        Self {
            worker: self.worker.clone(),
            execution: self.execution.clone(),
            owner_id: self.owner_id.clone(),
            hook: self.hook.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> PublicationFailureContext<Ctx> {
    fn new(ctx: &DurableWorkerCtx<Ctx>) -> Self {
        Self {
            worker: ctx.public_state.worker(),
            execution: ctx.owner_execution.clone(),
            owner_id: ctx.state.owned_agent_id.clone(),
            hook: ctx.replay_admission_hook(),
        }
    }

    fn fail(
        self,
        error: WorkerExecutorError,
        operation: Option<super::operation::OwnerToolOperation>,
        controller: AttachmentController,
        function: &'static str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let Self {
            worker,
            execution,
            owner_id,
            hook,
        } = self;
        controller.fence_owner();
        let operations = execution.tool_operations();
        let continuation = operations.track_failure_continuation();
        Box::pin(async move {
            let _continuation = continuation;
            if let Some(hook) = hook {
                hook.before_replay_access_start(
                    function,
                    ReplayAdmissionStage::BeforeToolAttachmentFailureElection,
                )
                .await;
            }
            let active = worker
                .active_agents()
                .try_get_active_agent(&owner_id)
                .await
                .filter(|active| std::sync::Arc::ptr_eq(&active.execution(), &execution));
            let cleanup = if let Some(operation) = &operation {
                operation.select_infrastructure(error).await;
                operation.claim_owner_failure_cleanup()
            } else {
                operations.select_primary_infrastructure(error).await
            };
            if let Some(cleanup) = cleanup {
                let winner = operations
                    .selected_owner_failure()
                    .expect("failure cleanup has an owner winner");
                let interrupt = worker.current_execution_interrupt_signal();
                let primary_interrupt = operation.is_none().then(|| interrupt.clone()).flatten();
                let cleanup_operations = operations.clone();
                let cleanup = operations.start_owner_failure_cleanup(
                    cleanup,
                    async move {
                        if let Some(active) = active {
                            active.fence_entity_bodies(winner).await;
                        } else {
                            cleanup_operations.close_failed_attachments();
                            cleanup_operations.drain_owner_failure_lanes().await;
                        }
                        if let Some(operation) = operation {
                            operation.settle().await;
                        }
                        Ok(())
                    },
                    move || {
                        if let Some(interrupt) = interrupt {
                            let _ = interrupt.send(InterruptKind::Interrupt(Timestamp::now_utc()));
                        }
                    },
                );
                if let Some(interrupt) = primary_interrupt {
                    let _ = interrupt.send(InterruptKind::Interrupt(Timestamp::now_utc()));
                }
                let _ = cleanup.await;
            }
        })
    }
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
        model::StreamWriteError::Closed(cause) => StreamWriteError::Closed(cause_from_model(cause)),
    }
}

pub(super) fn cause_from_model(cause: model::ByteStreamCloseCause) -> ByteStreamCloseCause {
    match cause {
        model::ByteStreamCloseCause::Finished => ByteStreamCloseCause::Finished,
        model::ByteStreamCloseCause::ConsumerCancelled => ByteStreamCloseCause::ConsumerCancelled,
        model::ByteStreamCloseCause::Failed(failure) => {
            ByteStreamCloseCause::Failed(failure_from_model(failure))
        }
    }
}

pub(super) fn failure_from_model(failure: model::ByteStreamFailure) -> ByteStreamFailure {
    match failure {
        model::ByteStreamFailure::Cancelled => ByteStreamFailure::Cancelled,
        model::ByteStreamFailure::Abandoned => ByteStreamFailure::Abandoned,
        model::ByteStreamFailure::ResourceExhausted => ByteStreamFailure::ResourceExhausted,
        model::ByteStreamFailure::Failed(message) => ByteStreamFailure::Failed(message),
    }
}
