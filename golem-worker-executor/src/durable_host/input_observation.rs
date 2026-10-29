// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::concurrent::{DropEvent, LiveCallPermit, MarkerReceipt};
use super::durability::DurableCallTrapContext;
use super::replay_state::{GuestReadOrder, ReplayState};
use super::tail_work::TailActivity;
use futures::future::BoxFuture;
use golem_common::model::OplogIndex;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::TerminalConsumption;

pub(super) type InputReadObserver =
    Box<dyn FnOnce(TerminalConsumption) -> Result<(), WorkerExecutorError> + Send>;

/// Calling this closure reserves the observation's ordered append position synchronously.
/// Its receipt joins persistence and publication without keeping the guest Store borrowed.
pub(super) type EnqueueInputObservation = Box<
    dyn FnOnce() -> Result<BoxFuture<'static, Result<(), WorkerExecutorError>>, WorkerExecutorError>
        + Send,
>;

/// Delivery ownership shared by native input adapters, independent of their journal format.
pub(super) struct InputObservationDelivery {
    pub replay: ReplayState,
    pub guest_order: Option<GuestReadOrder>,
    pub runtime: Option<Arc<crate::worker::suspension::RuntimeStore>>,
    pub activity: TailActivity,
    pub runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    pub cleanup_sink: mpsc::UnboundedSender<DropEvent>,
    pub trap_context: DurableCallTrapContext,
    pub live_call_permit: LiveCallPermit,
}

impl InputObservationDelivery {
    /// The caller validates logical input identity before supplying a retained boundary.
    /// Live preparation must not reserve an oplog position; only consumption does that.
    pub async fn prepare(
        self,
        recorded_index: Option<OplogIndex>,
        prepare_live: impl Future<Output = Result<EnqueueInputObservation, WorkerExecutorError>>,
        result: oneshot::Sender<Result<InputReadObserver, WorkerExecutorError>>,
    ) -> wasmtime::Result<oneshot::Receiver<()>> {
        self.prepare_inner(recorded_index, prepare_live, result, None)
            .await
    }

    /// A host receive takes ownership of the logical item before handing it to its transport.
    /// Unlike the synchronous guest callback, this boundary can wait for the commit result.
    pub async fn consume_host(
        self,
        recorded_index: Option<OplogIndex>,
        prepare_live: impl Future<Output = Result<EnqueueInputObservation, WorkerExecutorError>>,
    ) -> wasmtime::Result<()> {
        let (prepared, observer) = oneshot::channel();
        let (completed, completion) = oneshot::channel();
        let _settled = self
            .prepare_inner(recorded_index, prepare_live, prepared, Some(completed))
            .await?;
        let observer = observer
            .await
            .map_err(|_| wasmtime::Error::msg("host observation preparation stopped"))??;
        observer(TerminalConsumption::Delivered)?;
        completion
            .await
            .map_err(|_| wasmtime::Error::msg("host observation commit stopped"))??;
        Ok(())
    }

    async fn prepare_inner(
        self,
        recorded_index: Option<OplogIndex>,
        prepare_live: impl Future<Output = Result<EnqueueInputObservation, WorkerExecutorError>>,
        mut result: oneshot::Sender<Result<InputReadObserver, WorkerExecutorError>>,
        completed: Option<oneshot::Sender<Result<(), WorkerExecutorError>>>,
    ) -> wasmtime::Result<oneshot::Receiver<()>> {
        let (settled, observation_settled) = oneshot::channel();
        let prepare = async {
            let prepared = if let Some(index) = recorded_index {
                let barrier = if completed.is_some() {
                    self.replay.await_stream_observation(index).await?
                } else {
                    self.replay
                        .await_guest_stream_observation(index, self.guest_order.clone())
                        .await?
                };
                (Some(barrier), None)
            } else {
                self.replay
                    .await_natural_tail_end(Some(&self.activity))
                    .await?;
                (None, Some(prepare_live.await?))
            };
            if completed.is_none()
                && let Some(runtime) = &self.runtime
            {
                self.activity.park(runtime.await_guest_blocked()).await;
            }
            Ok::<_, WorkerExecutorError>(prepared)
        };
        let (barrier, prepared) = tokio::select! {
            biased;
            _ = result.closed() => {
                if let Some(index) = recorded_index
                    && !(self.runtime_teardown)()
                {
                    return Err(self.replay.fail_stream_observation(index).into());
                }
                return Ok(observation_settled);
            },
            outcome = prepare => outcome?,
        };
        let observer: InputReadObserver = if let Some(barrier) = barrier {
            Box::new(move |consumption| {
                let _ownership = (self.activity, self.live_call_permit, settled);
                if consumption == TerminalConsumption::NotDelivered {
                    let error = self.replay.fail_stream_observation(recorded_index.unwrap());
                    barrier.fail(
                        "cancelled native stream read cannot retain its recorded guest delivery",
                    );
                    return Err(error);
                }
                barrier.acknowledge();
                if let Some(completed) = completed {
                    let _ = completed.send(Ok(()));
                }
                Ok(())
            })
        } else {
            // Preparation is complete. Only future guest demand can reserve the append;
            // retaining an unconsumed item must not prevent invocation settlement.
            let tracker = self.activity.tracker();
            drop(self.activity);
            Box::new(move |consumption| {
                if consumption == TerminalConsumption::NotDelivered {
                    return Ok(());
                }
                let activity = tracker.activity();
                let receipt = prepared.unwrap()();
                let (done, receiver) = oneshot::channel();
                tokio::spawn(async move {
                    let outcome = match receipt {
                        Ok(receipt) => receipt.await,
                        Err(error) => Err(error),
                    };
                    drop(activity);
                    if let Some(completed) = completed {
                        let _ = completed.send(outcome.clone());
                    }
                    let _ = done.send(outcome);
                    let _ = settled.send(());
                });
                let _ = self.cleanup_sink.send(DropEvent::AwaitCompletionMarker {
                    receipt: Some(MarkerReceipt::pending(receiver)),
                    trap_context: self.trap_context,
                    live_call_permit: Some(self.live_call_permit),
                });
                Ok(())
            })
        };
        match result.send(Ok(observer)) {
            Err(Ok(observer)) if !(self.runtime_teardown)() => {
                observer(TerminalConsumption::NotDelivered)?;
            }
            Err(Err(error)) => return Err(error.into()),
            _ => {}
        }
        Ok(observation_settled)
    }
}
