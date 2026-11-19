// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::response_body::{HttpFutureDelivery, HttpFutureDeliveryGuard};
use crate::durable_host::concurrent::{
    AccessClaimOptions, DeferredCallReplayOutcome, DurableCallSession, InvocationIdSource,
    LeaveIncompleteOnDrop, ReplayAccessStartOutcome,
};
use crate::durable_host::p3::{DurableP3, durable_worker_ctx};
use crate::durable_host::tail_work::TailActivity;
use crate::services::oplog::OplogOps;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::payload::HostPayloadPair;
use golem_common::model::oplog::payload::types::SerializableP3HttpWriteOutcome;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestP3HttpRequestBodyWrite, HostResponseP3HttpRequestBodyWrite,
    OplogEntry,
};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{
    Access, Accessor, GuestWriteInterceptor, Lift, Source, StreamResult, VecBuffer, WriteBuffer,
};
use wasmtime::{AsContextMut, StoreContextMut};

struct WriteDemand {
    offered: usize,
    plan: oneshot::Sender<WritePlan>,
    accepted: oneshot::Receiver<SerializableP3HttpWriteOutcome>,
    delivery: oneshot::Sender<HttpFutureDeliveryGuard>,
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
        delivery: oneshot::Receiver<HttpFutureDeliveryGuard>,
    },
    Accepting {
        offered: usize,
        accepted: Option<oneshot::Sender<SerializableP3HttpWriteOutcome>>,
        delivery: oneshot::Receiver<HttpFutureDeliveryGuard>,
    },
    Replaying {
        outcome: SerializableP3HttpWriteOutcome,
        buffer: VecBuffer<T>,
        forward_empty: bool,
        delivery: oneshot::Receiver<HttpFutureDeliveryGuard>,
    },
    Delivering {
        result: StreamResult,
        delivery: oneshot::Receiver<HttpFutureDeliveryGuard>,
    },
}

pub(super) struct HttpRequestWriteInterceptor<T> {
    demands: mpsc::Sender<WriteDemand>,
    state: WriteState<T>,
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
                        delivery,
                    };
                    // Both live and replay asynchronous writes use the scheduled path.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                WriteState::Preparing {
                    offered,
                    plan,
                    accepted,
                    delivery,
                } => {
                    let plan = ready!(Pin::new(plan).poll(cx))
                        .map_err(|_| wasmtime::Error::msg("HTTP write preparation stopped"))?;
                    let (_, replacement) = oneshot::channel();
                    let delivery = std::mem::replace(delivery, replacement);
                    this.state = match plan {
                        WritePlan::Live => WriteState::Accepting {
                            offered: *offered,
                            accepted: accepted.take(),
                            delivery,
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
                                delivery,
                            }
                        }
                    };
                }
                WriteState::Accepting {
                    offered,
                    accepted,
                    delivery,
                } => {
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
                    let (_, replacement) = oneshot::channel();
                    this.state = WriteState::Delivering {
                        result,
                        delivery: std::mem::replace(delivery, replacement),
                    };
                }
                WriteState::Replaying {
                    outcome,
                    buffer,
                    forward_empty,
                    delivery,
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
                    let (_, replacement) = oneshot::channel();
                    this.state = WriteState::Delivering {
                        result,
                        delivery: std::mem::replace(delivery, replacement),
                    };
                }
                WriteState::Delivering { result, delivery } => {
                    let guard = ready!(Pin::new(delivery).poll(cx))
                        .map_err(|_| wasmtime::Error::msg("HTTP write delivery stopped"))?;
                    source.register_write_observer(&mut store, move |consumption| {
                        guard.observe(consumption)
                    })?;
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
    invocation_id: InvocationIdSource,
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
            .run(record_writes::<Ctx, U, Pair>(
                accessor,
                receive,
                invocation_id,
                activity,
            ))
            .await
    })?;
    Ok(HttpRequestWriteInterceptor {
        demands,
        state: WriteState::Idle,
    })
}

async fn record_writes<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    mut demands: mpsc::Receiver<WriteDemand>,
    invocation_id: InvocationIdSource,
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
                return Err(wasmtime::Error::msg(format!(
                    "HTTP write replay request differs: recorded {recorded:?}, sequence {write_sequence}, offered {}",
                    demand.offered,
                )));
            }
            if stream_result(&recorded.outcome).1 > demand.offered {
                return Err(wasmtime::Error::msg(
                    "recorded HTTP write exceeds offered items",
                ));
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
        let delivery = HttpFutureDelivery::new();
        delivery.arm(token)?;
        let _ = delivery_tx.send(HttpFutureDeliveryGuard::new(delivery));
    }
    Ok(())
}
