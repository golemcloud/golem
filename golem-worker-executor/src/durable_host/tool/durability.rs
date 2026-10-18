// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::attachment::{
    AttachmentController, AttachmentObserver, AttachmentWriteAcceptance, AttachmentWriteBody,
    AttachmentWriter, PreparedAttachmentTerminal, StagedAttachmentWrite,
};
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessorContext, Cancellable, CompletionDelivery,
    DeferredCallReplayOutcome, DurableAccess, DurableCallSession, DurableContext,
    PreparedContextStart, SharedDurableContext,
};
use crate::durable_host::durability::{
    InFunctionRetryHost, find_durable_call_trap_context, mark_durable_call_trap_context,
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

pub(crate) fn native_host_failure<Ctx: WorkerCtx>(
    context: &DurableWorkerCtx<Ctx>,
    error: anyhow::Error,
) -> impl Future<Output = WorkerExecutorError> + Send + 'static + use<Ctx> {
    let infrastructure = error.chain().find_map(|cause| {
        let error = cause.downcast_ref::<WorkerExecutorError>()?;
        (matches!(
            error,
            WorkerExecutorError::RecoveryRequired { .. }
                | WorkerExecutorError::UnexpectedOplogEntry { .. }
        ) || crate::durable_host::is_tool_attachment_live_admission_rejection(error))
        .then(|| error.clone())
    });
    let trap = crate::model::TrapType::from_error::<Ctx>(
        &error,
        InFunctionRetryHost::current_retry_point(context),
        context.agent_mode(),
    );
    let completed = context.entity_invocation_scope().is_some_and(|scope| {
        scope.mode() == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted
    });
    let operation = context.entity_tool_operation().filter(|_| !completed);
    let cancellation = context.entity_cancellation();
    async move {
        if let Some(error) = infrastructure {
            return error;
        }
        if let Some(operation) = operation {
            let locally_cancelled = cancellation.is_some_and(|token| token.is_cancelled())
                && operation.claim_local_cancellation_interruption();
            if !locally_cancelled {
                operation.select_trap(trap.clone()).await;
            }
        }
        trap.as_golem_error("")
            .unwrap_or_else(|| WorkerExecutorError::runtime("native tool invocation interrupted"))
    }
}

struct HistoricalAttachmentWriteJob<
    Pair: golem_common::model::oplog::HostPayloadPair,
    Ctx: WorkerCtx,
> {
    prepared: PreparedContextStart<Pair, Cancellable, Ctx>,
    restore: HistoricalAttachmentRestore,
}

struct HistoricalAttachmentRestore {
    request: HostRequestToolAttachmentWrite,
    descriptor: crate::durable_host::replay_state::HistoricalAttachmentWrite,
    staged: StagedAttachmentWrite,
    ranges: Vec<crate::durable_host::replay_state::AttachmentRangeValidation>,
    replay: crate::durable_host::replay_state::ReplayState,
    _activity: crate::durable_host::tail_work::TailActivity,
    _permit: crate::durable_host::concurrent::LiveCallPermit,
    result: tokio::sync::oneshot::Sender<Result<CompletionDelivery, WorkerExecutorError>>,
}

impl<Pair, Ctx: WorkerCtx> HistoricalAttachmentWriteJob<Pair, Ctx>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = HostRequestToolAttachmentWrite,
            Resp = HostResponseToolAttachmentWrite,
        >,
{
    fn prepare<T: 'static, D: HasData + ?Sized>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        writer: AttachmentWriter,
        request: HostRequestToolAttachmentWrite,
        bytes: Vec<u8>,
        descriptor: crate::durable_host::replay_state::HistoricalAttachmentWrite,
    ) -> anyhow::Result<(
        Self,
        tokio::sync::oneshot::Receiver<Result<CompletionDelivery, WorkerExecutorError>>,
    )> {
        let prepared = DurableCallSession::<Pair, Cancellable>::prepare_historical_context(
            store,
            get_ctx,
            DurableFunctionType::WriteLocal,
            AccessClaimOptions::default(),
        )?;
        let (replay, activity, permit) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.tail_work_tracker().activity(),
                crate::durable_host::concurrent::LiveCallPermit::new(
                    ctx.state.live_host_call_counter(),
                ),
            )
        });
        let ranges = replay.prepare_tool_attachment_range_validation(descriptor.end, &bytes)?;
        let body = if let Some(read) = descriptor.original_typed_read {
            let length = bytes.len();
            drop(bytes);
            AttachmentWriteBody::Recorded {
                length,
                hydrate: Box::pin(replay.deferred_tool_attachment_body(read, descriptor.end)),
            }
        } else {
            AttachmentWriteBody::Inline(bytes)
        };
        let staged = writer.stage_historical_write(body, descriptor.end)?;
        let (result, receiver) = tokio::sync::oneshot::channel();
        Ok((
            Self {
                prepared,
                restore: HistoricalAttachmentRestore {
                    request,
                    descriptor,
                    staged,
                    ranges,
                    replay,
                    _activity: activity,
                    _permit: permit,
                    result,
                },
            },
            receiver,
        ))
    }

    async fn run_native(
        self,
        mut context: SharedDurableContext<Ctx>,
    ) -> Result<(), WorkerExecutorError> {
        let call = DurableCallSession::<Pair, Cancellable>::start_prepared_context(
            &context,
            |ctx: &mut Ctx| ctx.durable_ctx_mut(),
            self.prepared,
            async |_| Ok(self.restore.request.clone()),
        )
        .await;
        self.restore.finish(&mut context, call).await
    }
}

impl<Pair, U: Send + 'static, Ctx: WorkerCtx> AccessorTask<U, HasSelf<DurableWorkerCtx<Ctx>>>
    for HistoricalAttachmentWriteJob<Pair, Ctx>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = HostRequestToolAttachmentWrite,
            Resp = HostResponseToolAttachmentWrite,
        > + Send
        + 'static,
{
    async fn run(
        self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> wasmtime::Result<()> {
        let call = DurableCallSession::<Pair, Cancellable>::start_prepared_context(
            accessor,
            accessor.getter(),
            self.prepared,
            async |_| Ok(self.restore.request.clone()),
        )
        .await;
        self.restore
            .finish(&mut AccessorContext::new(accessor, accessor.getter()), call)
            .await
            .map_err(Into::into)
    }
}

impl HistoricalAttachmentRestore {
    async fn finish<Pair, Ctx: WorkerCtx>(
        self,
        context: &mut impl DurableContext<Ctx>,
        call: Result<DurableCallSession<Pair, Cancellable>, WorkerExecutorError>,
    ) -> Result<(), WorkerExecutorError>
    where
        Pair: golem_common::model::oplog::HostPayloadPair<
                Req = HostRequestToolAttachmentWrite,
                Resp = HostResponseToolAttachmentWrite,
            >,
    {
        let result = async {
            let call = call?;
            if call.start_index() != self.descriptor.start || call.is_live() {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "the indexed historical attachment Start",
                    format!("claimed attachment Start {}", call.start_index()),
                ));
            }
            let DeferredCallReplayOutcome::Replayed(response, delivery) =
                call.replay_deferred(context).await?
            else {
                return Err(WorkerExecutorError::runtime(
                    "accepted attachment became incomplete",
                ));
            };
            if response.outcome != ToolAttachmentWriteOutcome::Accepted {
                return Err(WorkerExecutorError::runtime(
                    "accepted attachment changed its outcome",
                ));
            }
            let end = self
                .replay
                .await_tool_attachment_acceptance(self.descriptor.start)
                .await?;
            if end != self.descriptor.end {
                return Err(WorkerExecutorError::runtime(
                    "accepted attachment changed its End",
                ));
            }
            self.replay
                .validate_tool_attachment_ranges(self.ranges)
                .await?;
            self.staged
                .publish()
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            self.replay
                .complete_tool_attachment_reconstruction(self.descriptor.start)?;
            Ok(delivery)
        }
        .await;
        let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let _ = self.result.send(result);
        outcome
    }
}

fn historical_write<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    request: &ToolAttachmentWriteRequest,
) -> Result<Option<crate::durable_host::replay_state::HistoricalAttachmentWrite>, WorkerExecutorError>
{
    if ctx.is_live() || ctx.state.durability_is_suppressed() {
        Ok(None)
    } else {
        ctx.state
            .replay_state
            .historical_tool_attachment_write(request)
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
        > + Send
        + 'static,
{
    let Some(identity) = writer.identity() else {
        return Ok(writer.write(bytes).await);
    };
    let request = ToolAttachmentWriteRequest::new(identity, operation_ordinal, &bytes);
    let host_request = HostRequestToolAttachmentWrite {
        request: request.clone(),
    };
    if let Some(descriptor) =
        accessor.with(|mut access| historical_write(access.get(), &request))?
    {
        let (job, restored) = HistoricalAttachmentWriteJob::<Pair, Ctx>::prepare(
            accessor,
            accessor.getter(),
            writer,
            host_request,
            bytes,
            descriptor,
        )?;
        accessor.spawn(job);
        let delivery = restored.await??;
        if delivery.is_replay_discarded() {
            std::future::pending::<()>().await;
        }
        delivery.deliver_at_accessor_terminal(accessor).await?;
        return Ok(Ok(()));
    }
    let call = DurableCallSession::<Pair, Cancellable>::start_access_with_options(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteLocal,
        AccessClaimOptions::default(),
        host_request,
    )
    .await?;
    let (result, delivery) = write_started(
        &mut AccessorContext::new(accessor, accessor.getter()),
        call,
        writer,
        controller,
        request,
        bytes,
    )
    .await?;
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(result)
}

pub(crate) async fn write_native<Ctx: WorkerCtx>(
    ctx: &mut SharedDurableContext<Ctx>,
    writer: AttachmentWriter,
    tasks: &crate::worker::tasks::NativeReconstructionSpawner,
    bytes: Vec<u8>,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    use golem_common::model::oplog::host_functions::GolemToolOutputWrite;
    let Some(identity) = writer.identity() else {
        return Ok(writer.write(bytes).await);
    };
    let request =
        ToolAttachmentWriteRequest::new(identity, writer.next_operation_ordinal(), &bytes);
    let host_request = HostRequestToolAttachmentWrite {
        request: request.clone(),
    };
    if let Some(descriptor) = ctx.with_ctx(|ctx| historical_write(ctx, &request))? {
        let (job, restored) = HistoricalAttachmentWriteJob::<GolemToolOutputWrite, Ctx>::prepare(
            ctx,
            |ctx: &mut Ctx| ctx.durable_ctx_mut(),
            writer,
            host_request,
            bytes,
            descriptor,
        )?;
        tasks.spawn(job.run_native(ctx.clone()))?;
        let mut delivery = match restored.await {
            Ok(Ok(delivery)) => delivery,
            // The supervisor owns reconstruction errors even when the caller survives.
            _ => std::future::pending().await,
        };
        if delivery.is_replay_discarded() {
            std::future::pending::<()>().await;
        }
        delivery.prepare_delivery(None).await?;
        delivery.delivered();
        return Ok(Ok(()));
    }
    let call = DurableCallSession::<GolemToolOutputWrite, Cancellable>::start_context_with_options(
        ctx,
        |ctx: &mut Ctx| ctx.durable_ctx_mut(),
        DurableFunctionType::WriteLocal,
        AccessClaimOptions::default(),
        async move |_| Ok(host_request),
    )
    .await?;
    let trap = call.trap_context();
    let (result, mut delivery) = write_started(
        ctx,
        call,
        writer.clone(),
        writer.controller(),
        request,
        bytes,
    )
    .await
    .map_err(|error| {
        if find_durable_call_trap_context(&error).is_some() {
            error
        } else {
            mark_durable_call_trap_context(error, trap)
        }
    })?;
    delivery
        .prepare_delivery(None)
        .await
        .map_err(|error| mark_durable_call_trap_context(error.into(), trap))?;
    delivery.delivered();
    Ok(result)
}

pub(crate) async fn finish_native<Ctx: WorkerCtx>(
    ctx: &mut SharedDurableContext<Ctx>,
    writer: AttachmentWriter,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    use golem_common::model::oplog::host_functions::GolemToolOutputFinish;
    let Some(attachment) = writer.identity() else {
        return Ok(writer.finish());
    };
    let request = golem_common::model::oplog::HostRequestToolAttachmentFinish {
        attachment,
        operation_ordinal: writer.next_operation_ordinal(),
    };
    terminal_native::<GolemToolOutputFinish, Ctx>(ctx, writer, request, None).await
}

pub(crate) async fn fail_native<Ctx: WorkerCtx>(
    ctx: &mut SharedDurableContext<Ctx>,
    writer: AttachmentWriter,
    reason: ByteStreamFailure,
) -> anyhow::Result<Result<(), StreamWriteError>> {
    use golem_common::model::oplog::host_functions::GolemToolOutputFail;
    let Some(attachment) = writer.identity() else {
        return Ok(writer.fail(reason));
    };
    let request = golem_common::model::oplog::HostRequestToolAttachmentFail {
        attachment,
        operation_ordinal: writer.next_operation_ordinal(),
        reason: failure_to_model(&reason),
    };
    terminal_native::<GolemToolOutputFail, Ctx>(ctx, writer, request, Some(reason)).await
}

async fn terminal_native<Pair, Ctx: WorkerCtx>(
    ctx: &mut SharedDurableContext<Ctx>,
    writer: AttachmentWriter,
    request: Pair::Req,
    reason: Option<ByteStreamFailure>,
) -> anyhow::Result<Result<(), StreamWriteError>>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Resp = golem_common::model::oplog::HostResponseToolAttachmentTerminalResult,
        >,
    Pair::Req: Clone,
{
    let start_request = request.clone();
    let call = DurableCallSession::<Pair, Cancellable>::start_context_with_options(
        ctx,
        |ctx: &mut Ctx| ctx.durable_ctx_mut(),
        DurableFunctionType::WriteLocal,
        AccessClaimOptions::default(),
        async move |_| Ok(start_request),
    )
    .await?;
    let trap = call.trap_context();
    let (result, mut delivery) = terminal_started(
        ctx,
        call,
        writer.clone(),
        writer.controller(),
        request.into(),
        reason,
    )
    .await
    .map_err(|error| {
        if find_durable_call_trap_context(&error).is_some() {
            error
        } else {
            mark_durable_call_trap_context(error, trap)
        }
    })?;
    delivery
        .prepare_delivery(None)
        .await
        .map_err(|error| mark_durable_call_trap_context(error.into(), trap))?;
    delivery.delivered();
    Ok(result)
}

async fn write_started<Pair, Ctx: WorkerCtx>(
    context: &mut impl DurableContext<Ctx>,
    mut call: DurableCallSession<Pair, Cancellable>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    request: ToolAttachmentWriteRequest,
    bytes: Vec<u8>,
) -> anyhow::Result<(Result<(), StreamWriteError>, CompletionDelivery)>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = HostRequestToolAttachmentWrite,
            Resp = HostResponseToolAttachmentWrite,
        >,
{
    let identity = writer.identity().expect("durable write has an identity");
    let start = call.start_index();
    if !call.is_live() {
        let oplog = context.with_ctx(|ctx| ctx.state.oplog.clone());
        let OplogEntry::Start {
            request: Some(recorded),
            ..
        } = oplog.read(start).await.map_err(WorkerExecutorError::from)?
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
        match call.replay_deferred(context).await? {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                anyhow::ensure!(
                    response.outcome.validate(&request),
                    "invalid tool output write outcome at {start}"
                );
                anyhow::ensure!(
                    response.outcome != ToolAttachmentWriteOutcome::Accepted,
                    "accepted attachment write was not owned before its Start claim"
                );
                if response.outcome == ToolAttachmentWriteOutcome::HostResourceExhausted {
                    context.with_ctx(|ctx| {
                        ctx.state
                            .replay_state
                            .selected_tool_attachment_terminal_end(
                                start,
                                identity,
                                &model::ToolAttachmentTerminal::HostResourceExhausted,
                            )
                    })?;
                }
                let result = response.outcome.result().map_err(write_error_from_model);
                if delivery.is_replay_discarded() {
                    std::future::pending::<()>().await;
                }
                return Ok((result, delivery));
            }
            DeferredCallReplayOutcome::Incomplete(live) => call = live,
        }
    }
    if !call.is_persisted() {
        let result = writer.write(bytes).await;
        let outcome = outcome_for_result(&result, request.length == 0);
        let (_, delivery) = call
            .complete_deferred(context, HostResponseToolAttachmentWrite { outcome })
            .await?;
        return Ok((result, delivery));
    }
    let mut prepared = match writer.prepare_write(bytes).await {
        Ok(Some(prepared)) => prepared,
        result => {
            let result = result.map(|_| ());
            let outcome = outcome_for_result(&result, request.length == 0);
            let (_, delivery) = call
                .complete_deferred(context, HostResponseToolAttachmentWrite { outcome })
                .await?;
            return Ok((result, delivery));
        }
    };
    loop {
        prepared.wait_ready().await;
        let mut selected = None;
        let failed = publication_failure(context, controller.clone(), Pair::FQFN);
        let interrupt = context.with_ctx(|ctx| ctx.create_interrupt_signal());
        let completing = call.complete_deferred_with_effect(
            context,
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
                if let Some(hook) = context.with_ctx(|ctx| ctx.replay_admission_hook()) {
                    let interrupt = context.with_ctx(|ctx| ctx.create_interrupt_signal());
                    tokio::select! {
                        _ = hook.before_replay_access_start(
                        Pair::FQFN,
                        ReplayAdmissionStage::AfterToolOutputAccepted,
                        ) => {},
                        interruption = interrupt => return Err(interruption.into()),
                    }
                }
                return Ok((Ok(()), delivery));
            }
            Err((unfinished, _)) => call = unfinished,
        }
        let result = match selected.expect("acceptance callback must select an outcome") {
            Ok(AttachmentWriteAcceptance::Retry) => continue,
            Ok(AttachmentWriteAcceptance::ResourceExhausted(prepared)) => {
                return complete_resource_exhaustion(context, call, prepared, controller).await;
            }
            Ok(AttachmentWriteAcceptance::Discarded) => Ok(()),
            Err(error) => Err(error),
            Ok(AttachmentWriteAcceptance::Accepted(())) => {
                unreachable!("accepted write owns its End")
            }
        };
        let outcome = outcome_for_result(&result, false);
        let (_, delivery) = call
            .complete_deferred(context, HostResponseToolAttachmentWrite { outcome })
            .await?;
        return Ok((result, delivery));
    }
}

async fn complete_resource_exhaustion<Pair, Ctx: WorkerCtx>(
    context: &mut impl DurableContext<Ctx>,
    mut call: DurableCallSession<Pair, Cancellable>,
    prepared: PreparedAttachmentTerminal,
    controller: AttachmentController,
) -> anyhow::Result<(Result<(), StreamWriteError>, CompletionDelivery)>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Req = HostRequestToolAttachmentWrite,
            Resp = HostResponseToolAttachmentWrite,
        >,
{
    let failed = publication_failure(context, controller, Pair::FQFN);
    let mut selection = None;
    let interrupt = context.with_ctx(|ctx| ctx.create_interrupt_signal());
    let completing = call.complete_deferred_with_effect(
        context,
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
            return Ok((Err(exhausted), delivery));
        }
        Err((unfinished, _)) => call = unfinished,
    }
    let error = match selection.expect("exhaustion acceptance must select an outcome") {
        Ok(None) => exhausted,
        Err(error) => error,
        Ok(Some(())) => unreachable!("selected exhaustion owns its End"),
    };
    let (_, delivery) = call
        .complete_deferred(
            context,
            HostResponseToolAttachmentWrite {
                outcome: ToolAttachmentWriteOutcome::Rejected(error_to_model(&error)),
            },
        )
        .await?;
    Ok((Err(error), delivery))
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

    let Some(attachment) = observer.identity() else {
        return Ok(observer.wait_terminal().await);
    };
    let request = HostRequestToolAttachmentEndpoint {
        attachment,
        operation_ordinal,
    };
    let expected = request.clone().into();
    let mut call =
        DurableCallSession::<GolemToolStdinWait, Cancellable>::start_access_with_options(
            accessor,
            accessor.getter(),
            DurableFunctionType::ReadLocal,
            AccessClaimOptions::default(),
            request,
        )
        .await?;
    if !call.is_live() {
        let start = call.start_index();
        let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
        let OplogEntry::Start {
            request: Some(recorded),
            ..
        } = oplog.read(start).await.map_err(WorkerExecutorError::from)?
        else {
            return Err(call.trap(anyhow::anyhow!("tool stdin wait has no recorded request")));
        };
        let recorded = oplog
            .download_payload(recorded)
            .await
            .map_err(anyhow::Error::msg)?;
        if recorded != expected {
            return Err(call.trap(anyhow::anyhow!(
                "tool stdin wait request differs at {start}"
            )));
        }
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
    use golem_common::model::oplog::HostRequest;
    let expected: HostRequest = request.clone().into();
    let call = DurableCallSession::<Pair, Cancellable>::start_access_with_options(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteLocal,
        AccessClaimOptions::default(),
        request,
    )
    .await?;
    let (result, delivery) = terminal_started(
        &mut AccessorContext::new(accessor, accessor.getter()),
        call,
        writer,
        controller,
        expected,
        reason,
    )
    .await?;
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(result)
}

async fn terminal_started<Pair, Ctx: WorkerCtx>(
    context: &mut impl DurableContext<Ctx>,
    mut call: DurableCallSession<Pair, Cancellable>,
    writer: AttachmentWriter,
    controller: AttachmentController,
    expected: golem_common::model::oplog::HostRequest,
    reason: Option<ByteStreamFailure>,
) -> anyhow::Result<(Result<(), StreamWriteError>, CompletionDelivery)>
where
    Pair: golem_common::model::oplog::HostPayloadPair<
            Resp = golem_common::model::oplog::HostResponseToolAttachmentTerminalResult,
        >,
{
    use golem_common::model::oplog::HostResponseToolAttachmentTerminalResult;
    use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentTerminalOutcome;
    let attachment = writer.identity().expect("durable terminal has an identity");
    let cause = AttachmentWriter::effective_terminal(reason);
    let effective = model::ToolAttachmentTerminal::Application(cause_to_model(&cause));
    let start = call.start_index();
    if !call.is_live() {
        let oplog = context.with_ctx(|ctx| ctx.state.oplog.clone());
        let OplogEntry::Start {
            request: Some(recorded),
            ..
        } = oplog.read(start).await.map_err(WorkerExecutorError::from)?
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
        match call.replay_deferred(context).await? {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                if let ToolAttachmentTerminalOutcome::Selected(selected) = &response.outcome {
                    anyhow::ensure!(
                        selected == &effective,
                        "invalid selected tool output terminal at {start}"
                    );
                    context.with_ctx(|ctx| {
                        ctx.state
                            .replay_state
                            .selected_tool_attachment_terminal_end(start, attachment, selected)
                    })?;
                }
                let result = response.outcome.result().map_err(write_error_from_model);
                if delivery.is_replay_discarded() {
                    std::future::pending::<()>().await;
                }
                return Ok((result, delivery));
            }
            DeferredCallReplayOutcome::Incomplete(live) => call = live,
        }
    }
    let prepared = writer.prepare_terminal(cause);
    let outcome = match prepared {
        Ok(Ok(prepared)) if call.is_persisted() => {
            let failed = publication_failure(context, controller, Pair::FQFN);
            let mut selection = None;
            let interrupt = context.with_ctx(|ctx| ctx.create_interrupt_signal());
            let completing = call.complete_deferred_with_effect(
                context,
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
                    return Ok((Ok(()), delivery));
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
        .complete_deferred(
            context,
            HostResponseToolAttachmentTerminalResult { outcome },
        )
        .await?;
    Ok((result, delivery))
}

pub(super) fn failure_to_model(failure: &ByteStreamFailure) -> model::ByteStreamFailure {
    match failure {
        ByteStreamFailure::Cancelled => model::ByteStreamFailure::Cancelled,
        ByteStreamFailure::Abandoned => model::ByteStreamFailure::Abandoned,
        ByteStreamFailure::ResourceExhausted => model::ByteStreamFailure::ResourceExhausted,
        ByteStreamFailure::Failed(message) => model::ByteStreamFailure::Failed(message.clone()),
    }
}

pub(super) fn cause_to_model(cause: &ByteStreamCloseCause) -> model::ByteStreamCloseCause {
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

type ProducerTerminalRecorder = Box<
    dyn FnOnce() -> Result<
            Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
            WorkerExecutorError,
        > + Send
        + Sync,
>;

pub struct ProducerDisposal {
    controller: AttachmentController,
    primary_transition: bool,
    admission_error: Option<WorkerExecutorError>,
    record: Option<ProducerTerminalRecorder>,
    pending: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
    no_body: bool,
    _permit: Option<crate::durable_host::concurrent::LiveCallPermit>,
}

/// Outer completion owns admission joins; ordinary cleanup drains must remain reentrant.
pub(crate) async fn finish_producer_disposals<Ctx: WorkerCtx>(
    access: &mut impl DurableContext<Ctx>,
) -> Result<(), WorkerExecutorError> {
    access.drain().await.map_err(|error| error.source)?;
    let jobs = access.with_ctx(|ctx| {
        ctx.producer_disposal_tasks
            .as_ref()
            .map(|tasks| tasks.monitor())
    });
    if let Some(jobs) = jobs {
        jobs.drain().await?;
    }
    access.drain().await.map_err(|error| error.source)?;
    Ok(())
}

pub(crate) async fn abort_producer_disposals<Ctx: WorkerCtx>(
    access: &mut impl DurableContext<Ctx>,
) {
    if let Some(mut tasks) = access.with_ctx(|ctx| ctx.producer_disposal_tasks.take()) {
        tasks.abort_and_join().await;
    }
}

impl std::fmt::Debug for ProducerDisposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProducerDisposal")
            .field("primary_transition", &self.primary_transition)
            .field("admission_error", &self.admission_error)
            .field("no_body", &self.no_body)
            .finish_non_exhaustive()
    }
}

impl ProducerDisposal {
    pub(crate) async fn finish<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<(), WorkerExecutorError> {
        if self.pending.is_none() && access.with_ctx(|ctx| ctx.producer_disposal_tasks.is_none()) {
            return Ok(());
        }
        if let Some(error) = &self.admission_error {
            return Err(error.clone());
        }
        if self.primary_transition {
            access.process_replay_events().await?;
            self.primary_transition = false;
        }
        if self.pending.is_none() && self.controller.producer_disposal_owned_elsewhere() {
            return Ok(());
        }
        if let Some(record) = self.record.take() {
            self.pending = record()?;
            if self.pending.is_none() {
                // Another selector owns the terminal, or the consumer already closed locally.
                // Neither implies a durable publisher that this cleanup can safely await.
                return Ok(());
            }
        }
        if let Some(pending) = &mut self.pending {
            pending.await.map_err(|error| {
                WorkerExecutorError::runtime(format!("producer terminal recorder stopped: {error}"))
            })??;
            self.pending = None;
        }
        if self.no_body {
            self.controller.publish_no_body_terminal();
        }
        Ok(())
    }
}

pub(super) fn dispose_producer<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    producer: super::attachment::AttachmentProducer,
    failure: ByteStreamFailure,
    no_body: bool,
) -> anyhow::Result<()> {
    use crate::durable_host::concurrent::{
        DropEvent, LiveCallPermit, prepare_access_terminal_effect,
    };
    use crate::services::oplog::CommitLevel;

    let controller = producer.controller();
    let Some(identity) = producer
        .writer()
        .identity()
        .filter(|_| !ctx.state.durability_is_suppressed())
    else {
        if no_body {
            if matches!(failure, ByteStreamFailure::Abandoned) {
                let _ = producer.abandon_unconfigured();
            } else {
                let _ = producer.reject_unconfigured();
            }
        }
        return Ok(());
    };
    producer.retire();
    let Some(disposal_tasks) = &ctx.producer_disposal_tasks else {
        return Ok(());
    };
    let teardown = ctx.stream_runtime_teardown_probe();
    if teardown() {
        return Ok(());
    }
    let failure_context = PublicationFailureContext::new(ctx);
    let operation = ctx.entity_tool_operation();
    let replay = ctx.state.replay_state.clone();
    let live = ctx.is_live();
    let boundary = golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal::BeforeAttachmentDisposal {
        attachment: identity,
        terminal: model::ToolAttachmentTerminal::Application(cause_to_model(
            &ByteStreamCloseCause::Failed(failure.clone()),
        )),
    };
    let recorded_refusal = !live
        && operation.as_ref().is_some_and(|operation| {
            operation
                .expected_admission_refusal()
                .is_some_and(|(expected, _)| expected == boundary)
        });
    // Owner finalization can record a different terminal after refusing this disposal.
    if !recorded_refusal {
        if controller.producer_disposal_owned_elsewhere() {
            return Ok(());
        }
        if !live && replay.has_restorable_tool_attachment_terminal(identity) {
            if no_body {
                controller.prepare_no_body_terminal();
            }
            return Ok(());
        }
    }
    let oplog = ctx.state.oplog.clone();
    let parent = ctx.entity_parent_start_index();
    let live_calls = ctx.state.live_host_call_counter();
    let permit = live.then(|| LiveCallPermit::new(live_calls.clone()));
    let trap_context = crate::durable_host::durability::DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
    };
    let cleanup = ctx
        .state
        .dropped_call_event_sender()
        .expect("worker cleanup queue");
    let record: ProducerTerminalRecorder = Box::new({
        let controller = controller.clone();
        let failure_context = failure_context.clone();
        let operation = operation.clone();
        move || -> Result<_, WorkerExecutorError> {
            let recording = failure_context
                .worker
                .runtime_append_tasks()
                .register()
                .ok_or(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                })?;
            let worker = failure_context.worker.clone();
            let failed_controller = controller.clone();
            let published_controller = controller.clone();
            let (handoff, receipt) = prepare_access_terminal_effect(
                &worker.tasks,
                recording,
                {
                    let worker = worker.clone();
                    async move {
                        worker
                            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                            .await
                            .map(|_| ())
                            .map_err(WorkerExecutorError::from)
                    }
                },
                move |publication: super::attachment::AttachmentTerminalPublication, index| {
                    publication.publish(index);
                    if no_body {
                        published_controller.publish_no_body_terminal();
                    }
                },
                move |error| {
                    failure_context.fail(
                        error,
                        operation,
                        failed_controller,
                        "tool producer disposal",
                    )
                },
            )?;
            let cause = ByteStreamCloseCause::Failed(failure);
            let accepted = controller.accept_application_terminal(cause.clone(), |publication| {
                (
                    publication,
                    oplog.enqueue_add(OplogEntry::ToolAttachmentControl {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: parent,
                        attachment: identity,
                        transition: model::ToolAttachmentControl::TerminalSelected(
                            model::ToolAttachmentTerminal::Application(cause_to_model(&cause)),
                        ),
                    }),
                )
            });
            if let Some(accepted) = accepted {
                assert!(handoff.send(accepted).is_ok());
                Ok(Some(receipt))
            } else {
                drop(handoff);
                Ok(None)
            }
        }
    });
    // Live selection and enqueue happen before the resource drop returns. Missing-terminal
    // replay repair waits outside cleanup drains so historical producers can still run.
    let (record, pending) = if live {
        let Some(pending) = record()? else {
            return Ok(());
        };
        (None, Some(pending))
    } else {
        (Some(record), None)
    };
    let mut disposal = Box::new(ProducerDisposal {
        controller,
        primary_transition: false,
        admission_error: None,
        record,
        pending,
        no_body,
        _permit: permit,
    });
    if live {
        cleanup
            .send(DropEvent::DisposeToolProducer {
                disposal,
                trap_context,
            })
            .map_err(|_| anyhow::anyhow!("tool producer cleanup queue closed"))?;
    } else {
        let transition = ctx
            .live_continuation_at_replay_tail_factory(true, "tool producer disposal".to_string());
        let primary = ctx.runtime == golem_common::model::entity::OwnerRuntime::Agent;
        let rejected = ctx.rejects_live_continuation_at_replay_tail();
        let interrupt = ctx.create_interrupt_signal();
        disposal_tasks.spawner().spawn(async move {
            let admission = async {
                loop {
                    if !recorded_refusal && replay.has_restorable_tool_attachment_terminal(identity)
                    {
                        if no_body {
                            disposal.controller.prepare_no_body_terminal();
                        }
                        return Ok(false);
                    }
                    if let Some(operation) = &operation {
                        operation.reject_missing_replay_admission(&boundary).await?;
                    }
                    if rejected {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "tool producer disposal",
                            "no recorded terminal during completed entity replay",
                        ));
                    }
                    replay.await_natural_tail_end(None).await?;
                    if replay.has_restorable_tool_attachment_terminal(identity) {
                        continue;
                    }
                    let pending = match transition().await? {
                        crate::durable_host::BeginReplayToLive::Pending(pending) => pending,
                        crate::durable_host::BeginReplayToLive::ReplayResumed => continue,
                    };
                    pending
                        .finish_at(Some(boundary.clone()))
                        .await?
                        .require_live()?;
                    return Ok::<_, WorkerExecutorError>(true);
                }
            };
            let result = tokio::select! {
                result = admission => result,
                kind = interrupt => return Err(WorkerExecutorError::Interrupted { kind }),
            };
            if matches!(result, Ok(false)) {
                return Ok(());
            }
            disposal.primary_transition = primary && result.is_ok();
            disposal.admission_error = result.err();
            disposal._permit = Some(LiveCallPermit::new(live_calls));
            cleanup
                .send(DropEvent::DisposeToolProducer {
                    disposal,
                    trap_context,
                })
                .map_err(|_| WorkerExecutorError::runtime("tool producer cleanup queue closed"))?;
            Ok(())
        })?;
    }
    Ok(())
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
                    kind: InterruptKind::Restart,
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

fn publication_failure<Ctx: WorkerCtx, Access: DurableContext<Ctx>>(
    access: &mut Access,
    controller: AttachmentController,
    function: &'static str,
) -> impl FnOnce(WorkerExecutorError) -> Pin<Box<dyn Future<Output = ()> + Send>>
+ Send
+ 'static
+ use<Ctx, Access> {
    let (context, operation) = access.with_ctx(|ctx| {
        (
            PublicationFailureContext::new(ctx),
            ctx.entity_tool_operation(),
        )
    });
    move |error| context.fail(error, operation, controller, function)
}

pub(super) struct PublicationFailureContext<Ctx: WorkerCtx> {
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
    pub(super) fn new(ctx: &DurableWorkerCtx<Ctx>) -> Self {
        Self {
            worker: ctx.public_state.worker(),
            execution: ctx.owner_execution.clone(),
            owner_id: ctx.state.owned_agent_id.clone(),
            hook: ctx.replay_admission_hook(),
        }
    }

    pub(super) fn fail(
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
