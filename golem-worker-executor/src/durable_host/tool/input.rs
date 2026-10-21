// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::attachment::{
    AttachmentConsumer, AttachmentController, AttachmentStreamBuffer, AttachmentStreamProducer,
    AttachmentStreamRead,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::LiveCallPermit;
use crate::durable_host::durability::{DurableCallTrapContext, InFunctionRetryHost};
use crate::durable_host::input_observation::{
    EnqueueInputObservation, InputObservationDelivery, InputReadObserver,
};
use crate::preview2::golem::tool::host::ByteStreamFailure;
use crate::services::HasWorker;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::workerctx::WorkerCtx;
use golem_common::model::durable_stream::StreamConsumerObservation;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::oplog::attachment::{
    ToolAttachmentId, ToolAttachmentReadObservation, ToolAttachmentReadOutcome,
};
use golem_common::model::{IdempotencyKey, OplogIndex, Timestamp};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use wasmtime::AsContextMut;
use wasmtime::component::{
    Accessor, AccessorTask, Destination, StreamProducer, StreamResult, TerminalConsumption,
};

struct PreparedRead {
    item: AttachmentStreamRead,
    replay_slot: Option<OwnedSemaphorePermit>,
    observer: Option<InputReadObserver>,
    observation: StreamConsumerObservation,
    from_journal: bool,
    recorded_index: Option<OplogIndex>,
}

/// Keeps the consumer binding across invocations while obtaining physical attribution from
/// the Store at each read. The getter borrows only the current poll's Store window.
pub(super) struct AttachmentInputProducer<U: 'static, Ctx: WorkerCtx> {
    source: AttachmentStreamProducer,
    controller: AttachmentController,
    attachment: ToolAttachmentId,
    invocation: IdempotencyKey,
    getter: fn(&mut U) -> &mut DurableWorkerCtx<Ctx>,
    ordinal: Arc<AtomicU64>,
    attempt: Arc<AtomicU64>,
    active_attempt: bool,
    pending: Option<oneshot::Receiver<Result<PreparedRead, WorkerExecutorError>>>,
    replay_slot: Arc<Semaphore>,
    finished: bool,
}

impl<U: 'static, Ctx: WorkerCtx> AttachmentInputProducer<U, Ctx> {
    pub(super) fn new(
        consumer: AttachmentConsumer,
        getter: fn(&mut U) -> &mut DurableWorkerCtx<Ctx>,
        ctx: &DurableWorkerCtx<Ctx>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            attachment: consumer
                .controller()
                .observer()
                .identity()
                .expect("durable attachment identity"),
            invocation: ctx
                .state
                .get_current_idempotency_key()
                .ok_or_else(|| anyhow::anyhow!("attachment consumer has no invocation"))?,
            controller: consumer.controller(),
            source: consumer.into_stream_producer(),
            getter,
            ordinal: Arc::new(AtomicU64::new(0)),
            attempt: Arc::new(AtomicU64::new(0)),
            active_attempt: false,
            pending: None,
            replay_slot: Arc::new(Semaphore::new(1)),
            finished: false,
        })
    }
}

impl<U: 'static, Ctx: WorkerCtx> StreamProducer<U> for AttachmentInputProducer<U, Ctx> {
    type Item = Result<Vec<u8>, ByteStreamFailure>;
    type Buffer = AttachmentStreamBuffer;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<'a, U>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if (self.getter)(store.data_mut())
            .state
            .durability_is_suppressed()
        {
            return Pin::new(&mut self.source).poll_produce(cx, store, destination, finish);
        }
        let host = destination.remaining(&mut store).is_none();
        let ctx = (self.getter)(store.data_mut());
        let observation = if host {
            StreamConsumerObservation::Host
        } else {
            StreamConsumerObservation::Guest {
                parent_start_index: ctx.entity_parent_start_index(),
            }
        };
        let ordinal = self.ordinal.load(Ordering::Acquire);
        let recorded = ctx.state.replay_state.tool_attachment_observation(
            self.attachment,
            &self.invocation,
            ordinal,
        );
        if finish {
            if self.active_attempt
                && let Some((_, Some((index, expected)))) = recorded
                && self.attempt.load(Ordering::Acquire) - 1 == expected
            {
                return Poll::Ready(Err(ctx
                    .state
                    .replay_state
                    .fail_stream_observation(index)
                    .into()));
            }
            self.active_attempt = false;
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if destination.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if !self.active_attempt {
            self.active_attempt = true;
            self.attempt.fetch_add(1, Ordering::AcqRel);
        }
        if let Some((_, Some((index, expected)))) = recorded {
            let actual = self.attempt.load(Ordering::Acquire) - 1;
            if actual < expected {
                return Poll::Pending;
            }
            if actual != expected {
                let ctx = (self.getter)(store.data_mut());
                return Poll::Ready(Err(ctx
                    .state
                    .replay_state
                    .fail_stream_observation(index)
                    .into()));
            }
        }
        let mut retained_item = None;
        let prepared = loop {
            if self.pending.is_none() {
                let live = if recorded.is_none() {
                    Some(match retained_item.take() {
                        Some(item) => item,
                        None => std::task::ready!(self.source.poll_read(cx))?,
                    })
                } else {
                    None
                };
                let ctx = (self.getter)(store.data_mut());
                let delivery = InputObservationDelivery {
                    replay: ctx.state.replay_state.clone(),
                    guest_order: ctx.guest_read_order(),
                    runtime: ctx.runtime_suspension.clone(),
                    activity: ctx.tail_work_tracker().activity(),
                    runtime_teardown: ctx.stream_runtime_teardown_probe(),
                    cleanup_sink: ctx
                        .state
                        .dropped_call_event_sender()
                        .expect("worker cleanup queue"),
                    trap_context: DurableCallTrapContext {
                        retry_from: InFunctionRetryHost::current_retry_point(ctx),
                    },
                    live_call_permit: LiveCallPermit::new(ctx.state.live_host_call_counter()),
                };
                let (result, pending) = oneshot::channel();
                let task = PrepareAttachmentObservation {
                    attachment: self.attachment,
                    invocation: self.invocation.clone(),
                    ordinal,
                    attempt: self.attempt.clone(),
                    parent: ctx.entity_parent_start_index(),
                    observation: observation.clone(),
                    recorded,
                    live,
                    replay_slot: self.replay_slot.clone(),
                    capacity: self.controller.metadata().capacity_bytes,
                    delivery,
                    worker: ctx.public_state.worker(),
                    oplog: ctx.state.oplog.clone(),
                    result,
                };
                store.as_context_mut().spawn(task);
                self.pending = Some(pending);
            }
            let prepared = std::task::ready!(Pin::new(self.pending.as_mut().unwrap()).poll(cx));
            self.pending = None;
            let prepared = prepared.map_err(|_| {
                wasmtime::Error::msg("attachment observation preparation stopped")
            })??;
            if prepared.observation == observation {
                break prepared;
            }
            if prepared.recorded_index.is_some() || !host || prepared.observer.is_none() {
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "attachment read changed its retained observation boundary",
                )));
            }
            prepared.observer.unwrap()(TerminalConsumption::NotDelivered)?;
            retained_item = Some(prepared.item);
        };
        let PreparedRead {
            item,
            replay_slot,
            observer,
            from_journal,
            ..
        } = prepared;
        let ordinal = self.ordinal.clone();
        let controller = self.controller.clone();
        let source = match &item {
            AttachmentStreamRead::Chunk { source_end, bytes } => {
                source_end.map(|source| (source, bytes.len()))
            }
            _ => None,
        };
        let terminal = match &item {
            AttachmentStreamRead::Failure { terminal_index, .. } => *terminal_index,
            _ => None,
        };
        let observe = move |consumption| -> Result<(), WorkerExecutorError> {
            if consumption == TerminalConsumption::Delivered {
                if from_journal {
                    if let Some((source, length)) = source {
                        controller.observe_write(source, length);
                    }
                    if let Some(terminal) = terminal {
                        controller.observe_terminal(terminal);
                    }
                }
                ordinal.fetch_add(1, Ordering::AcqRel);
            }
            if let Some(observer) = observer {
                observer(consumption)?;
            }
            Ok(())
        };
        if host {
            observe(TerminalConsumption::Delivered)?;
        } else {
            destination.register_read_observer(&mut store, move |consumption| {
                let _ = observe(consumption);
            })?;
        }
        self.active_attempt = false;
        match item {
            AttachmentStreamRead::Chunk { bytes, .. } => {
                destination.set_buffer(AttachmentStreamBuffer::new(Ok(bytes), replay_slot))
            }
            AttachmentStreamRead::Failure { failure, .. } => {
                destination.set_buffer(AttachmentStreamBuffer::new(Err(failure), replay_slot))
            }
            AttachmentStreamRead::Terminal { result, .. } => {
                self.finished = true;
                return Poll::Ready(Ok(result));
            }
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct PrepareAttachmentObservation<Ctx: WorkerCtx> {
    attachment: ToolAttachmentId,
    invocation: IdempotencyKey,
    ordinal: u64,
    attempt: Arc<AtomicU64>,
    parent: Option<OplogIndex>,
    observation: StreamConsumerObservation,
    recorded: Option<(OplogIndex, Option<(OplogIndex, u64)>)>,
    live: Option<AttachmentStreamRead>,
    replay_slot: Arc<Semaphore>,
    capacity: usize,
    delivery: InputObservationDelivery,
    worker: Arc<crate::worker::Worker<Ctx>>,
    oplog: Arc<dyn crate::services::oplog::Oplog>,
    result: oneshot::Sender<Result<PreparedRead, WorkerExecutorError>>,
}

impl<U: 'static, Ctx: WorkerCtx> AccessorTask<U> for PrepareAttachmentObservation<Ctx> {
    async fn run(self, _accessor: &Accessor<U>) -> wasmtime::Result<()> {
        self.prepare().await
    }
}

impl<Ctx: WorkerCtx> PrepareAttachmentObservation<Ctx> {
    async fn prepare(self) -> wasmtime::Result<()> {
        let recorded_index = self
            .recorded
            .and_then(|(_, boundary)| boundary.map(|(index, _)| index));
        let replay_slot = if self.recorded.is_some() {
            Some(self.replay_slot.acquire_owned().await?)
        } else {
            None
        };
        let (item, outcome) = if let Some((original, boundary)) = self.recorded {
            let (_, read, payload) = self
                .delivery
                .replay
                .hydrate_tool_attachment_observation(
                    boundary.map_or(original, |(index, _)| index),
                    self.capacity as u64,
                )
                .await?;
            if let Some((index, _)) = boundary {
                let entry = self.oplog.read(index).await?;
                if read.observation != self.observation
                    || entry.entity_parent_start_index() != self.parent
                {
                    return Err(self.delivery.replay.fail_stream_observation(index).into());
                }
            }
            let item = match &read.outcome {
                ToolAttachmentReadOutcome::TypedChunk(end) => AttachmentStreamRead::Chunk {
                    bytes: payload.expect("validated data observation"),
                    source_end: Some(*end),
                },
                ToolAttachmentReadOutcome::Eof { terminal_index } => {
                    AttachmentStreamRead::Terminal {
                        result: StreamResult::Dropped,
                        terminal_index: Some(*terminal_index),
                    }
                }
                ToolAttachmentReadOutcome::Error {
                    terminal_index,
                    failure,
                } => AttachmentStreamRead::Failure {
                    failure: super::durability::failure_from_model(failure.clone()),
                    terminal_index: Some(*terminal_index),
                },
                ToolAttachmentReadOutcome::RawBytes { .. } => {
                    return Err(wasmtime::Error::msg(
                        "typed attachment read has a raw-byte observation",
                    ));
                }
            };
            (item, read.outcome)
        } else {
            let item = self.live.expect("live attachment read");
            let outcome = match &item {
                AttachmentStreamRead::Chunk { source_end, .. } => {
                    ToolAttachmentReadOutcome::TypedChunk(source_end.ok_or_else(|| {
                        wasmtime::Error::msg("attachment chunk has no durable acceptance")
                    })?)
                }
                AttachmentStreamRead::Failure {
                    failure,
                    terminal_index,
                } => ToolAttachmentReadOutcome::Error {
                    terminal_index: terminal_index.ok_or_else(|| {
                        wasmtime::Error::msg("attachment failure has no durable terminal")
                    })?,
                    failure: super::durability::failure_to_model(failure),
                },
                AttachmentStreamRead::Terminal { terminal_index, .. } => {
                    ToolAttachmentReadOutcome::Eof {
                        terminal_index: terminal_index.ok_or_else(|| {
                            wasmtime::Error::msg("attachment EOF has no durable terminal")
                        })?,
                    }
                }
            };
            (item, outcome)
        };
        let payload = match &item {
            AttachmentStreamRead::Chunk { bytes, .. } if self.recorded.is_none() => Some(
                self.oplog
                    .upload_payload(bytes)
                    .await
                    .map_err(wasmtime::Error::msg)?,
            ),
            _ => None,
        };
        let observation = ToolAttachmentReadObservation {
            attachment: self.attachment,
            logical_invocation: self.invocation,
            observation: self.observation.clone(),
            ordinal: self.ordinal,
            read_attempt_ordinal: 0,
            outcome,
            reobserves: self
                .recorded
                .filter(|(_, boundary)| boundary.is_none())
                .map(|(original, _)| original),
        };
        let prepare_live = async move {
            let enqueue: EnqueueInputObservation = Box::new(move || {
                let mut observation = observation;
                observation.read_attempt_ordinal = self.attempt.load(Ordering::Acquire) - 1;
                let receipt = self.oplog.enqueue_add(OplogEntry::ToolAttachmentRead {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: self.parent,
                    observation: Box::new(observation),
                    payload,
                });
                Ok(Box::pin(async move {
                    receipt.await?;
                    self.worker
                        .commit_oplog_and_update_state(CommitLevel::Always)
                        .await?;
                    Ok(())
                }))
            });
            Ok(enqueue)
        };
        if self.observation == StreamConsumerObservation::Host {
            self.delivery
                .consume_host(recorded_index, prepare_live)
                .await?;
            let _ = self.result.send(Ok(PreparedRead {
                item,
                replay_slot,
                observer: None,
                observation: self.observation,
                from_journal: self.recorded.is_some(),
                recorded_index,
            }));
            return Ok(());
        }
        let (prepared, observer) = oneshot::channel();
        let teardown = self.delivery.runtime_teardown.clone();
        let settled = self
            .delivery
            .prepare(recorded_index, prepare_live, prepared)
            .await?;
        let observer = observer
            .await
            .map_err(|_| wasmtime::Error::msg("attachment observer preparation stopped"))??;
        if let Err(Ok(PreparedRead { observer, .. })) = self.result.send(Ok(PreparedRead {
            item,
            replay_slot,
            observer: Some(observer),
            observation: self.observation,
            from_journal: self.recorded.is_some(),
            recorded_index,
        })) && !teardown()
            && let Some(observer) = observer
        {
            observer(TerminalConsumption::NotDelivered)?;
        }
        let _ = settled.await;
        Ok(())
    }
}

pub(crate) struct NativeAttachmentInput {
    source: AttachmentStreamProducer,
    controller: AttachmentController,
    binding: Option<(ToolAttachmentId, IdempotencyKey)>,
    ordinal: u64,
    attempt: Arc<AtomicU64>,
    pending: Option<futures::future::BoxFuture<'static, anyhow::Result<PreparedRead>>>,
    replay_slot: Arc<Semaphore>,
    finished: bool,
}

impl NativeAttachmentInput {
    pub(crate) fn new<Ctx: WorkerCtx>(
        consumer: AttachmentConsumer,
        ctx: &DurableWorkerCtx<Ctx>,
    ) -> anyhow::Result<Self> {
        let controller = consumer.controller();
        let binding = controller
            .observer()
            .identity()
            .map(|attachment| {
                ctx.state
                    .get_current_idempotency_key()
                    .map(|invocation| (attachment, invocation))
                    .ok_or_else(|| anyhow::anyhow!("attachment consumer has no invocation"))
            })
            .transpose()?;
        Ok(Self {
            source: consumer.into_stream_producer(),
            controller,
            binding,
            ordinal: 0,
            attempt: Arc::new(AtomicU64::new(0)),
            pending: None,
            replay_slot: Arc::new(Semaphore::new(1)),
            finished: false,
        })
    }

    pub(crate) async fn read<Ctx: WorkerCtx>(
        &mut self,
        ctx: &mut impl crate::durable_host::concurrent::DurableContext<Ctx>,
    ) -> anyhow::Result<Option<Result<Vec<u8>, ByteStreamFailure>>> {
        if self.finished {
            return Ok(None);
        }
        let (suppressed, replay) = ctx.with_ctx(|ctx| {
            (
                ctx.state.durability_is_suppressed(),
                ctx.state.replay_state.clone(),
            )
        });
        let binding = self.binding.as_ref().filter(|_| !suppressed);
        let item = if let Some((attachment, invocation)) = binding {
            if self.pending.is_none() {
                let recorded =
                    replay.tool_attachment_observation(*attachment, invocation, self.ordinal);
                let attempt = self.attempt.fetch_add(1, Ordering::AcqRel);
                if let Some((_, Some((index, expected)))) = recorded {
                    if attempt < expected {
                        std::future::pending::<()>().await;
                    }
                    if attempt != expected {
                        return Err(replay.fail_stream_observation(index).into());
                    }
                }
                let live = if recorded.is_none() {
                    Some(std::future::poll_fn(|cx| self.source.poll_read(cx)).await?)
                } else {
                    None
                };
                let (result, prepared) = oneshot::channel();
                let preparation = ctx.with_ctx(|ctx| PrepareAttachmentObservation {
                    attachment: *attachment,
                    invocation: invocation.clone(),
                    ordinal: self.ordinal,
                    attempt: self.attempt.clone(),
                    parent: ctx.entity_parent_start_index(),
                    observation: StreamConsumerObservation::Host,
                    recorded,
                    live,
                    replay_slot: self.replay_slot.clone(),
                    capacity: self.controller.metadata().capacity_bytes,
                    delivery: InputObservationDelivery {
                        replay: ctx.state.replay_state.clone(),
                        guest_order: None,
                        runtime: ctx.runtime_suspension.clone(),
                        activity: ctx.tail_work_tracker().activity(),
                        runtime_teardown: ctx.stream_runtime_teardown_probe(),
                        cleanup_sink: ctx
                            .state
                            .dropped_call_event_sender()
                            .expect("worker cleanup queue"),
                        trap_context: DurableCallTrapContext {
                            retry_from: InFunctionRetryHost::current_retry_point(ctx),
                        },
                        live_call_permit: LiveCallPermit::new(ctx.state.live_host_call_counter()),
                    },
                    worker: ctx.public_state.worker(),
                    oplog: ctx.state.oplog.clone(),
                    result,
                });
                let trap_context = preparation.delivery.trap_context;
                self.pending = Some(Box::pin(async move {
                    let result = async move {
                        preparation.prepare().await?;
                        Ok::<_, anyhow::Error>(prepared.await??)
                    }
                    .await;
                    result.map_err(|error| {
                        crate::durable_host::durability::mark_durable_call_trap_context(
                            error,
                            trap_context,
                        )
                    })
                }));
            }
            // A cancelled host await retains both the taken item and any accepted append.
            let prepared = self.pending.as_mut().unwrap().await;
            self.pending = None;
            let prepared = prepared?;
            if prepared.from_journal {
                match &prepared.item {
                    AttachmentStreamRead::Chunk {
                        source_end: Some(source),
                        bytes,
                    } => self.controller.observe_write(*source, bytes.len()),
                    AttachmentStreamRead::Failure {
                        terminal_index: Some(index),
                        ..
                    } => self.controller.observe_terminal(*index),
                    _ => {}
                }
            }
            self.ordinal += 1;
            prepared.item
        } else {
            std::future::poll_fn(|cx| self.source.poll_read(cx)).await?
        };
        match item {
            AttachmentStreamRead::Chunk { bytes, .. } => Ok(Some(Ok(bytes))),
            AttachmentStreamRead::Failure { failure, .. } => Ok(Some(Err(failure))),
            AttachmentStreamRead::Terminal { .. } => {
                self.finished = true;
                Ok(None)
            }
        }
    }
}
