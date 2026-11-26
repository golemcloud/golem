// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::durable_host::concurrent::{
    AccessClaimOptions, CompletionDelivery, DeferredCallReplayOutcome, DurableCallSession,
    InvocationIdSource, LeaveIncompleteOnDrop, ReplayAccessStartOutcome,
    resolve_current_observational_owner,
};
use crate::durable_host::durability::GuestTaskContext;
use crate::durable_host::p3::{DurableP3, durable_worker_ctx};
use crate::durable_host::tail_work::TailActivity;
use crate::services::oplog::OplogOps;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::payload::HostPayloadPair;
use golem_common::model::oplog::payload::types::SerializableP3HttpWriteOutcome;
use golem_common::model::oplog::{
    DurableFunctionType, DurableInvocationId, HostRequestP3HttpRequestBodyWrite,
    HostResponseP3HttpRequestBodyWrite, OplogEntry,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::any::Any;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{
    Access, Accessor, GuestWriteInterceptor, Lift, Source, StreamResult, TerminalConsumption,
    VecBuffer, WriteBuffer,
};
use wasmtime::{AsContextMut, StoreContextMut};

struct WriteDeliveryGuard(Option<CompletionDelivery>);

impl WriteDeliveryGuard {
    fn observe(mut self, consumption: TerminalConsumption) {
        let delivery = self.0.take().unwrap();
        match consumption {
            TerminalConsumption::Delivered => delivery.delivered(),
            TerminalConsumption::NotDelivered => drop(delivery),
            TerminalConsumption::Superseded => {
                tracing::error!("HTTP write terminal observer was superseded");
                delivery.suppress();
            }
        }
    }
}

impl Drop for WriteDeliveryGuard {
    fn drop(&mut self) {
        if let Some(delivery) = self.0.take() {
            delivery.suppress();
        }
    }
}

struct WriteDemand {
    offered: usize,
    invocation_id: InvocationIdSource,
    plan: oneshot::Sender<WritePlan>,
    accepted: oneshot::Receiver<SerializableP3HttpWriteOutcome>,
    delivery: oneshot::Sender<WriteDeliveryGuard>,
}

enum WritePlan {
    Live,
    Replay(SerializableP3HttpWriteOutcome),
}

enum WriteState<T> {
    Idle,
    Preparing {
        offered: usize,
        plan: oneshot::Receiver<WritePlan>,
        accepted: Option<oneshot::Sender<SerializableP3HttpWriteOutcome>>,
    },
    Accepting {
        offered: usize,
        accepted: Option<oneshot::Sender<SerializableP3HttpWriteOutcome>>,
    },
    Replaying {
        outcome: SerializableP3HttpWriteOutcome,
        buffer: VecBuffer<T>,
        forward_empty: bool,
    },
    Delivering {
        result: StreamResult,
    },
}

pub(super) struct HttpRequestWriteInterceptor<T> {
    demands: mpsc::Sender<WriteDemand>,
    state: WriteState<T>,
    delivery: Option<oneshot::Receiver<WriteDeliveryGuard>>,
    identify: fn(
        &mut dyn Any,
        Option<&GuestTaskContext>,
    ) -> wasmtime::Result<Option<DurableInvocationId>>,
    write_ready: fn(&mut dyn Any) -> wasmtime::Result<()>,
    issued: Option<InvocationIdSource>,
}

fn stream_result(outcome: &SerializableP3HttpWriteOutcome) -> (StreamResult, usize) {
    match outcome {
        SerializableP3HttpWriteOutcome::Completed(n) => (StreamResult::Completed, *n as usize),
        SerializableP3HttpWriteOutcome::Dropped(n) => (StreamResult::Dropped, *n as usize),
        SerializableP3HttpWriteOutcome::Cancelled(n) => (StreamResult::Cancelled, *n as usize),
    }
}

impl<D: 'static, T: Lift + Send + Sync + Unpin + 'static> GuestWriteInterceptor<D, T>
    for HttpRequestWriteInterceptor<T>
{
    fn write_started(
        self: Pin<&mut Self>,
        mut store: StoreContextMut<'_, D>,
    ) -> wasmtime::Result<()> {
        let task_context = store
            .as_context_mut()
            .guest_task_context::<GuestTaskContext>()?;
        let this = self.get_mut();
        this.issued = Some(InvocationIdSource::Reserved((this.identify)(
            store.data_mut(),
            task_context.as_deref(),
        )?));
        Ok(())
    }

    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'_, D>,
        mut source: Source<'_, T>,
        finish: bool,
        native: &mut dyn FnMut(
            &mut Context<'_>,
            StoreContextMut<'_, D>,
            Source<'_, T>,
            bool,
        ) -> Poll<wasmtime::Result<StreamResult>>,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                WriteState::Idle => {
                    let offered = source.remaining(&mut store);
                    let (plan_tx, plan) = oneshot::channel();
                    let (accepted, accepted_rx) = oneshot::channel();
                    let (delivery_tx, delivery) = oneshot::channel();
                    this.demands
                        .try_send(WriteDemand {
                            offered,
                            invocation_id: this.issued.take().ok_or_else(|| {
                                wasmtime::Error::msg(
                                    "a native write issue must precede its first poll",
                                )
                            })?,
                            plan: plan_tx,
                            accepted: accepted_rx,
                            delivery: delivery_tx,
                        })
                        .map_err(|error| {
                            wasmtime::Error::msg(format!("HTTP write recorder stopped: {error}"))
                        })?;
                    this.state = WriteState::Preparing {
                        offered,
                        plan,
                        accepted: Some(accepted),
                    };
                    this.delivery = Some(delivery);
                    // Both live and replay asynchronous writes use the scheduled path.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                WriteState::Preparing {
                    offered,
                    plan,
                    accepted,
                } => {
                    let plan = ready!(Pin::new(plan).poll(cx))
                        .map_err(|_| wasmtime::Error::msg("HTTP write preparation stopped"))?;
                    this.state = match plan {
                        WritePlan::Live => WriteState::Accepting {
                            offered: *offered,
                            accepted: accepted.take(),
                        },
                        WritePlan::Replay(outcome) => {
                            let (_, count) = stream_result(&outcome);
                            if count > *offered {
                                return Poll::Ready(Err(wasmtime::Error::msg(
                                    "recorded HTTP write exceeds offered items",
                                )));
                            }
                            let mut items = Vec::with_capacity(count);
                            source.read_count(&mut store, &mut items, count)?;
                            if items.len() != count {
                                return Poll::Ready(Err(wasmtime::Error::msg(
                                    "HTTP write prefix was not reconstructed",
                                )));
                            }
                            WriteState::Replaying {
                                forward_empty: count == 0
                                    && matches!(
                                        outcome,
                                        SerializableP3HttpWriteOutcome::Completed(_)
                                    ),
                                outcome,
                                buffer: items.into(),
                            }
                        }
                    };
                }
                WriteState::Accepting { offered, accepted } => {
                    let result = ready!(native(
                        cx,
                        store.as_context_mut(),
                        source.reborrow(),
                        finish
                    ))?;
                    let count = (*offered - source.remaining(&mut store)) as u64;
                    let outcome = match result {
                        StreamResult::Completed => SerializableP3HttpWriteOutcome::Completed(count),
                        StreamResult::Dropped => SerializableP3HttpWriteOutcome::Dropped(count),
                        StreamResult::Cancelled => SerializableP3HttpWriteOutcome::Cancelled(count),
                    };
                    accepted.take().unwrap().send(outcome).map_err(|_| {
                        wasmtime::Error::msg("HTTP write recorder dropped acceptance")
                    })?;
                    this.state = WriteState::Delivering { result };
                }
                WriteState::Replaying {
                    outcome,
                    buffer,
                    forward_empty,
                } => {
                    if *forward_empty || !buffer.remaining().is_empty() {
                        let result = ready!(native(
                            cx,
                            store.as_context_mut(),
                            source.with_buffer(buffer),
                            false
                        ))?;
                        *forward_empty = false;
                        if matches!(result, StreamResult::Dropped) && !buffer.remaining().is_empty()
                        {
                            return Poll::Ready(Err(wasmtime::Error::msg(
                                "native HTTP reader dropped a recorded accepted prefix",
                            )));
                        }
                        if !buffer.remaining().is_empty() {
                            cx.waker().wake_by_ref();
                            return Poll::Pending;
                        }
                    }
                    let (result, _) = stream_result(outcome);
                    if matches!(result, StreamResult::Cancelled) && !finish {
                        return Poll::Pending;
                    }
                    this.state = WriteState::Delivering { result };
                }
                WriteState::Delivering { result } => {
                    let delivery = this
                        .delivery
                        .as_mut()
                        .ok_or_else(|| wasmtime::Error::msg("HTTP write delivery missing"))?;
                    let guard = ready!(Pin::new(delivery).poll(cx));
                    this.delivery = None;
                    let guard =
                        guard.map_err(|_| wasmtime::Error::msg("HTTP write delivery stopped"))?;
                    source.register_write_observer(&mut store, move |consumption| {
                        guard.observe(consumption)
                    })?;
                    (this.write_ready)(store.data_mut())?;
                    let result = *result;
                    this.state = WriteState::Idle;
                    return Poll::Ready(Ok(result));
                }
            }
        }
    }
}

pub(super) fn write_interceptor<Ctx, U, Pair, T>(
    store: &mut Access<U, DurableP3<Ctx>>,
) -> wasmtime::Result<HttpRequestWriteInterceptor<T>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    Pair: HostPayloadPair<
            Req = HostRequestP3HttpRequestBodyWrite,
            Resp = HostResponseP3HttpRequestBodyWrite,
        >,
    T: Send + 'static,
{
    let (demands, receive) = mpsc::channel(1);
    let (captured, activity) = {
        let mut access = store.as_context_mut();
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        (
            ctx.attribution.capture(),
            Arc::new(ctx.tail_work_tracker().activity()),
        )
    };
    store.spawn(async move |accessor| {
        captured
            .run(record_writes::<Ctx, U, Pair>(accessor, receive, activity))
            .await
    })?;
    Ok(HttpRequestWriteInterceptor {
        demands,
        state: WriteState::Idle,
        delivery: None,
        identify: |data, task_context| {
            let ctx = data
                .downcast_mut::<Ctx>()
                .ok_or_else(|| {
                    wasmtime::Error::msg(
                        "HTTP write interceptor store data must be its worker context",
                    )
                })?
                .durable_ctx_mut();
            let owner = resolve_current_observational_owner(ctx, task_context)?;
            if owner.is_some()
                || task_context.and_then(GuestTaskContext::current).is_some()
                || ctx.state.snapshotting_mode
            {
                return Ok(None);
            }
            Ok(ctx
                .invocation_anchor()
                .map(|root| ctx.guest_imports.reserve_builtin_identity(root)))
        },
        write_ready: |data| {
            let ctx = data
                .downcast_mut::<Ctx>()
                .ok_or_else(|| wasmtime::Error::msg("HTTP write interceptor context mismatch"))?
                .durable_ctx_mut();
            if let Some(hook) = ctx.replay_admission_hook() {
                hook.http_request_write_ready(Pair::FQFN);
            }
            Ok(())
        },
        issued: None,
    })
}

async fn record_writes<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    mut demands: mpsc::Receiver<WriteDemand>,
    activity: Arc<TailActivity>,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: 'static,
    Pair: HostPayloadPair<
            Req = HostRequestP3HttpRequestBodyWrite,
            Resp = HostResponseP3HttpRequestBodyWrite,
        >,
{
    let mut sequence = 0;
    while let Some(demand) = activity.park(demands.recv()).await {
        let invocation_id = demand.invocation_id;
        let write_sequence = sequence;
        sequence += 1;
        let claim =
            DurableCallSession::<Pair, LeaveIncompleteOnDrop>::claim_replay_access_with_options(
                accessor,
                durable_worker_ctx::<Ctx, U>,
                DurableFunctionType::ReadLocal,
                AccessClaimOptions {
                    invocation_id,
                    deferred_observation: true,
                    ..Default::default()
                },
            )
            .await?;
        let mut token = if let ReplayAccessStartOutcome::Claimed(handle) = claim {
            let oplog = accessor.with(|mut access| {
                durable_worker_ctx::<Ctx, U>(access.data_mut())
                    .state
                    .oplog
                    .clone()
            });
            let entry = oplog.read(handle.start_index()).await?;
            let OplogEntry::Start {
                request: Some(recorded),
                ..
            } = entry
            else {
                return Err(wasmtime::Error::msg("HTTP write Start has no request"));
            };
            let recorded = HostRequestP3HttpRequestBodyWrite::try_from(
                oplog
                    .download_payload(recorded)
                    .await
                    .map_err(wasmtime::Error::msg)?,
            )
            .map_err(wasmtime::Error::msg)?;
            if recorded.sequence != write_sequence || recorded.offered != demand.offered as u64 {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    handle.start_index(),
                    format!(
                        "HTTP write sequence {}, offered {}",
                        recorded.sequence, recorded.offered,
                    ),
                    format!(
                        "HTTP write sequence {write_sequence}, offered {}",
                        demand.offered
                    ),
                )
                .into());
            }
            if stream_result(&recorded.outcome).1 > demand.offered {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    handle.start_index(),
                    format!("HTTP write outcome {:?}", recorded.outcome),
                    format!("HTTP write offered {} items", demand.offered),
                )
                .into());
            }
            if demand
                .plan
                .send(WritePlan::Replay(recorded.outcome))
                .is_err()
            {
                return Ok(());
            }
            let replay = handle
                .replay_access_deferred(accessor, durable_worker_ctx::<Ctx, U>)
                .await?;
            match replay {
                DeferredCallReplayOutcome::Replayed(_, token) => token,
                DeferredCallReplayOutcome::Incomplete(handle) => {
                    handle
                        .complete_access_deferred(
                            accessor,
                            durable_worker_ctx::<Ctx, U>,
                            HostResponseP3HttpRequestBodyWrite {},
                        )
                        .await?
                        .1
                }
            }
        } else {
            if demand.plan.send(WritePlan::Live).is_err() {
                return Ok(());
            }
            let Ok(outcome) = activity.park(demand.accepted).await else {
                return Ok(());
            };
            let handle =
                DurableCallSession::<Pair, LeaveIncompleteOnDrop>::start_access_with_options(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    DurableFunctionType::ReadLocal,
                    AccessClaimOptions {
                        invocation_id,
                        ..Default::default()
                    },
                    HostRequestP3HttpRequestBodyWrite {
                        sequence: write_sequence,
                        offered: demand.offered as u64,
                        outcome,
                    },
                )
                .await?;
            handle
                .complete_access_deferred(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpRequestBodyWrite {},
                )
                .await?
                .1
        };
        let mut delivery_tx = demand.delivery;
        if token.is_replay_discarded() {
            activity.park(delivery_tx.closed()).await;
            continue;
        }
        let prepared = tokio::select! {
            biased;
            _ = delivery_tx.closed() => continue,
            result = token.prepare_delivery(Some(&activity)) => result,
        };
        if let Err(error) = prepared {
            token.suppress();
            return Err(error.into());
        }
        let _ = delivery_tx.send(WriteDeliveryGuard(Some(token)));
    }
    Ok(())
}
