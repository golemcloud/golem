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

use super::{
    AccessClaimOptions, CallReplayOutcome, Cancellable, CompletionDelivery,
    DeferredCallReplayOutcome, DurableCallSession, NotCancellable,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::durability::{DurableCallTrapContext, TerminalCallError};
use crate::durable_host::tail_work::TailActivity;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{DurableFunctionType, HostPayloadPair, OplogIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{Accessor, HasData};

/// The single buffered demand allowed between a guest-facing stream producer and its durable
/// driver. The producer also permits only one in-flight reply, so this bounds both queued demand
/// and produced data without reading ahead from the live source.
pub(crate) const DEMAND_STREAM_CAPACITY: usize = 1;

pub(crate) fn demand_channel<T>() -> (mpsc::Sender<T>, mpsc::Receiver<T>) {
    mpsc::channel(DEMAND_STREAM_CAPACITY)
}

/// One child result in a [`DurableDemandStream`]. A live item still owns the child session so the
/// adapter can read and encode its protocol-specific frame before persisting it. A replayed item
/// carries the recorded response and, where the adapter's protocol has one, its deferred
/// guest-delivery boundary.
///
/// Transient: adapters destructure it immediately, so boxing the live session would only add one
/// allocation per streamed item.
#[allow(clippy::large_enum_variant)]
pub(crate) enum DurableDemandItem<Pair: HostPayloadPair> {
    Live(DurableCallSession<Pair, NotCancellable>),
    Replayed {
        response: Pair::Resp,
        delivery: CompletionDelivery,
    },
}

/// Whether this adapter's oplog protocol records the guest-delivery disposition of each child.
/// TCP child sequences are markerless; P3 HTTP records markers and needs replay-discard parking.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DemandDeliveryMode {
    Markerless,
    Deferred,
}

/// Durable parent/child protocol shared by byte streams whose source advances only on guest
/// demand. It owns the batched parent, creates exactly one durable child per requested item, and
/// exposes the parent terminal only as a consuming operation so no children can be started after
/// it.
///
/// This is intentionally concrete to durable demand streams rather than a universal host-stream
/// abstraction. Adapters continue to own live I/O, frame encoding, cancellation, terminal result
/// handling, and resource cleanup.
pub(crate) struct DurableDemandStream<Parent, Child, Demand>
where
    Parent: HostPayloadPair,
    Child: HostPayloadPair,
{
    parent: DurableCallSession<Parent, Cancellable>,
    demands: mpsc::Receiver<Demand>,
    observational_owner: Option<OplogIndex>,
    delivery_mode: DemandDeliveryMode,
    _child: std::marker::PhantomData<Child>,
}

impl<Parent, Child, Demand> DurableDemandStream<Parent, Child, Demand>
where
    Parent: HostPayloadPair,
    Child: HostPayloadPair,
{
    pub(crate) async fn start<T, D, Ctx, F>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        demands: mpsc::Receiver<Demand>,
        claim_options: AccessClaimOptions,
        delivery_mode: DemandDeliveryMode,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(super::AccessStartContext) -> Result<Parent::Req, WorkerExecutorError>,
    {
        let observational_owner = claim_options.observational_owner;
        let parent = DurableCallSession::<Parent, Cancellable>::start_access_with_options(
            store,
            get_ctx,
            DurableFunctionType::WriteRemoteBatched(None),
            claim_options,
            build_request,
        )
        .await?;
        Ok(Self {
            parent,
            demands,
            observational_owner,
            delivery_mode,
            _child: std::marker::PhantomData,
        })
    }

    pub(crate) fn is_live(&self) -> bool {
        self.parent.is_live()
    }

    pub(crate) fn begin_index(&self) -> OplogIndex {
        self.parent.begin_index()
    }

    pub(crate) fn trap_context(&self) -> DurableCallTrapContext {
        self.parent.trap_context()
    }

    pub(crate) fn parent_mut(&mut self) -> &mut DurableCallSession<Parent, Cancellable> {
        &mut self.parent
    }

    pub(crate) async fn next_demand(&mut self, activity: &TailActivity) -> Option<Demand> {
        activity.park(self.demands.recv()).await
    }

    pub(crate) fn abandon_for_trap(&mut self) {
        self.parent.abandon_for_trap();
    }

    pub(crate) fn trap(&mut self, error: impl Into<anyhow::Error>) -> anyhow::Error {
        self.parent.trap(error)
    }

    pub(crate) async fn next<T, D, Ctx>(
        &self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Child::Req,
    ) -> Result<DurableDemandItem<Child>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let child = DurableCallSession::<Child, NotCancellable>::start_access_with_options(
            store,
            get_ctx,
            DurableFunctionType::WriteRemoteBatched(Some(self.parent.begin_index())),
            AccessClaimOptions {
                observational_owner: self.observational_owner,
                ..Default::default()
            },
            async move |_| Ok(request),
        )
        .await?;

        if child.is_live() {
            Ok(DurableDemandItem::Live(child))
        } else {
            match self.delivery_mode {
                DemandDeliveryMode::Markerless => {
                    match child.replay_access(store, get_ctx).await? {
                        CallReplayOutcome::Replayed(response) => Ok(DurableDemandItem::Replayed {
                            response,
                            delivery: CompletionDelivery::unarmed(),
                        }),
                        CallReplayOutcome::Incomplete(mut child) => {
                            child.abandon_for_trap();
                            Err(incomplete_child_error())
                        }
                    }
                }
                DemandDeliveryMode::Deferred => {
                    match child.replay_access_deferred(store, get_ctx).await? {
                        DeferredCallReplayOutcome::Replayed(response, delivery) => {
                            Ok(DurableDemandItem::Replayed { response, delivery })
                        }
                        DeferredCallReplayOutcome::Incomplete(mut child) => {
                            child.abandon_for_trap();
                            Err(incomplete_child_error())
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn complete_item<T, D, Ctx>(
        &self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        child: DurableCallSession<Child, NotCancellable>,
        response: Child::Resp,
    ) -> Result<CompletionDelivery, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        match self.delivery_mode {
            DemandDeliveryMode::Markerless => child
                .complete_access(store, get_ctx, response)
                .await
                .map(|_| CompletionDelivery::unarmed()),
            DemandDeliveryMode::Deferred => child
                .complete_access_deferred(store, get_ctx, response, None)
                .await
                .map(|(_, delivery)| delivery),
        }
    }

    pub(crate) async fn finish<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Parent::Resp,
    ) -> Result<Parent::Resp, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.parent.finish_access(store, get_ctx, response).await
    }

    pub(crate) async fn finish_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Parent::Resp,
    ) -> Result<(Parent::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.parent
            .finish_access_deferred(store, get_ctx, response)
            .await
    }
}

fn incomplete_child_error() -> WorkerExecutorError {
    WorkerExecutorError::unexpected_oplog_entry(
        "completed batched demand-stream child",
        "incomplete batched demand-stream child".to_string(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DemandDelivery {
    Delivered,
    Abandoned,
}

/// Crosses the one fallible boundary between a persisted child terminal and its guest-facing
/// stream producer. Live delivery records delivered/discarded disposition when the adapter's
/// protocol has delivery markers; replay of a discarded child parks until the deterministic guest
/// drops the corresponding demand and never re-delivers the item.
pub(crate) async fn deliver_demand<R>(
    activity: &TailActivity,
    mut demand: oneshot::Sender<R>,
    reply: R,
    mut delivery: CompletionDelivery,
) -> Result<DemandDelivery, WorkerExecutorError> {
    if delivery.is_replay_discarded() {
        tracing::debug!(
            "recorded demand-stream completion was discarded before delivery; parking until the \
             replayed guest drops the stream demand"
        );
        activity.park(demand.closed()).await;
        return Ok(DemandDelivery::Abandoned);
    }

    delivery.prepare_delivery().await?;
    if demand.send(reply).is_ok() {
        delivery.delivered();
        Ok(DemandDelivery::Delivered)
    } else {
        tracing::debug!(
            "demand-stream item persisted but the guest dropped the stream before delivery"
        );
        delivery.discarded().await?;
        Ok(DemandDelivery::Abandoned)
    }
}

pub(crate) fn closed_demand<R>() -> oneshot::Sender<R> {
    let (tx, rx) = oneshot::channel();
    drop(rx);
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    async fn demand_channel_preserves_order_and_allows_only_one_queued_demand() {
        let (tx, mut rx) = demand_channel();

        tx.try_send(1).expect("the first demand must fit");
        assert!(matches!(
            tx.try_send(2),
            Err(mpsc::error::TrySendError::Full(2))
        ));
        assert_eq!(rx.recv().await, Some(1));

        tx.try_send(2).expect("capacity must be released by recv");
        assert_eq!(rx.recv().await, Some(2));
    }
}
