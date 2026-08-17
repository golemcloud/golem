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

use crate::durable_host::schema_value_stream::StoreValueResolver;
use crate::workerctx::WorkerCtx;
use golem_schema::schema::wit::wire::SchemaValueTree;
use golem_schema::schema::wit::{decode_value_with, encode_value_with_streams};
use golem_schema::schema::{SchemaValue, SchemaValueStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, Source, StreamConsumer, StreamProducer, StreamResult};

/// Tracks source endpoints created by one live streaming invocation. The
/// invocation keeps its Store event loop running until every source has
/// reached end-of-stream or the downstream reader has detached.
#[derive(Debug)]
pub(crate) struct LiveStreamTracker {
    pub(super) active: AtomicUsize,
    changed: tokio::sync::Notify,
    cancelled: tokio_util::sync::CancellationToken,
}

impl LiveStreamTracker {
    pub(crate) fn new(cancelled: tokio_util::sync::CancellationToken) -> Self {
        Self {
            active: AtomicUsize::new(0),
            changed: tokio::sync::Notify::new(),
            cancelled,
        }
    }

    fn add_source(&self) {
        self.active.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_waiters();
    }

    fn source_finished(&self) {
        let previous = self.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "live stream source count underflow");
        self.changed.notify_waiters();
    }

    pub(crate) async fn wait_for_sources(&self) {
        loop {
            let changed = self.changed.notified();
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }

    async fn cancelled(&self) {
        self.cancelled.cancelled().await;
    }

    pub(crate) fn cancellation_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancelled.clone()
    }
}

#[derive(Debug, Default)]
pub(super) struct SourceLifecycle {
    pub(super) finished: AtomicBool,
    finished_notify: tokio::sync::Notify,
    trackers: Mutex<Vec<Arc<LiveStreamTracker>>>,
    cancelled: tokio_util::sync::CancellationToken,
}

impl SourceLifecycle {
    fn attach(self: &Arc<Self>, tracker: Arc<LiveStreamTracker>) {
        let mut trackers = self
            .trackers
            .lock()
            .expect("stream lifecycle mutex poisoned");
        if self.finished.load(Ordering::Acquire)
            || trackers
                .iter()
                .any(|current| Arc::ptr_eq(current, &tracker))
        {
            return;
        }
        tracker.add_source();
        trackers.push(tracker.clone());
        let lifecycle = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tracker.cancelled() => {
                    lifecycle.cancelled.cancel();
                    lifecycle.finish();
                }
                _ = lifecycle.wait_finished() => {}
            }
        });
    }

    async fn wait_finished(&self) {
        loop {
            let finished = self.finished_notify.notified();
            if self.finished.load(Ordering::Acquire) {
                return;
            }
            finished.await;
        }
    }

    fn finish(&self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        self.finished_notify.notify_waiters();
        let trackers = std::mem::take(
            &mut *self
                .trackers
                .lock()
                .expect("stream lifecycle mutex poisoned"),
        );
        for tracker in trackers {
            tracker.source_finished();
        }
    }
}

pub(super) struct RelayReceiver {
    pub(super) demand_tx: mpsc::Sender<()>,
    pub(super) item_rx: mpsc::Receiver<Result<SchemaValue, String>>,
    lifecycle: Arc<SourceLifecycle>,
}

impl RelayReceiver {
    pub(super) fn attach(&self, tracker: Arc<LiveStreamTracker>) {
        self.lifecycle.attach(tracker);
    }

    pub(super) async fn next(&mut self) -> Result<Option<SchemaValue>, String> {
        tokio::select! {
            result = self.demand_tx.send(()) => {
                if result.is_err() {
                    return match self.item_rx.recv().await {
                        Some(Ok(value)) => Ok(Some(value)),
                        Some(Err(error)) => Err(error),
                        None => Ok(None),
                    };
                }
            }
            _ = self.lifecycle.cancelled.cancelled() => {
                self.lifecycle.finish();
                return Err("live streaming invocation was cancelled".to_string());
            }
        }
        tokio::select! {
            item = self.item_rx.recv() => match item {
                Some(Ok(value)) => Ok(Some(value)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            },
            _ = self.lifecycle.cancelled.cancelled() => {
                self.lifecycle.finish();
                Err("live streaming invocation was cancelled".to_string())
            }
        }
    }
}

impl Drop for RelayReceiver {
    fn drop(&mut self) {
        self.lifecycle.finish();
    }
}

pub(super) struct RelayConsumer {
    demand_rx: mpsc::Receiver<()>,
    item_tx: mpsc::Sender<Result<SchemaValue, String>>,
    lifecycle: Arc<SourceLifecycle>,
    has_demand: bool,
}

pub(super) struct RelayPeer {
    pub(super) demand_rx: mpsc::Receiver<()>,
    pub(super) item_tx: mpsc::Sender<Result<SchemaValue, String>>,
    pub(super) lifecycle: Arc<SourceLifecycle>,
}

impl Drop for RelayConsumer {
    fn drop(&mut self) {
        self.lifecycle.finish();
    }
}

pub(super) fn relay_pair(
    tracker: Option<Arc<LiveStreamTracker>>,
) -> (RelayConsumer, SchemaValueStream) {
    let (peer, stream) = relay_endpoint_pair(tracker);
    (
        RelayConsumer {
            demand_rx: peer.demand_rx,
            item_tx: peer.item_tx,
            lifecycle: peer.lifecycle,
            has_demand: false,
        },
        stream,
    )
}

pub(super) fn relay_endpoint_pair(
    tracker: Option<Arc<LiveStreamTracker>>,
) -> (RelayPeer, SchemaValueStream) {
    let (demand_tx, demand_rx) = mpsc::channel(1);
    let (item_tx, item_rx) = mpsc::channel(1);
    let lifecycle = Arc::new(SourceLifecycle::default());
    if let Some(tracker) = tracker {
        lifecycle.attach(tracker);
    }
    let receiver = RelayReceiver {
        demand_tx,
        item_rx,
        lifecycle: lifecycle.clone(),
    };
    (
        RelayPeer {
            demand_rx,
            item_tx,
            lifecycle: lifecycle.clone(),
        },
        SchemaValueStream::from_host_endpoint(receiver),
    )
}

impl<Ctx: WorkerCtx> StreamConsumer<Ctx> for RelayConsumer {
    type Item = SchemaValueTree;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<Ctx>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish || self.item_tx.is_closed() {
            self.lifecycle.finish();
            return Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Dropped
            }));
        }

        if !self.has_demand {
            match self.demand_rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(())) => self.has_demand = true,
                Poll::Ready(None) => {
                    self.lifecycle.finish();
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }

        let mut item = None;
        source.read(&mut store, &mut item)?;
        let Some(item) = item else {
            return Poll::Ready(Ok(StreamResult::Completed));
        };

        let decoded = {
            let mut resolver = StoreValueResolver::new(&mut store);
            decode_value_with(item, &mut resolver).map_err(|error| error.to_string())
        };
        self.has_demand = false;
        match self.item_tx.try_send(decoded) {
            Ok(()) => Poll::Ready(Ok(StreamResult::Completed)),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.lifecycle.finish();
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Err(mpsc::error::TrySendError::Full(_)) => Poll::Ready(Err(wasmtime::Error::msg(
                "schema value stream relay accepted an item without matching demand",
            ))),
        }
    }
}

pub(super) struct RelayProducer {
    receiver: RelayReceiver,
    demand_pending: bool,
    finished: bool,
}

impl RelayProducer {
    pub(super) fn new(receiver: RelayReceiver) -> Self {
        Self {
            receiver,
            demand_pending: false,
            finished: false,
        }
    }
}

impl<Ctx: WorkerCtx> StreamProducer<Ctx> for RelayProducer {
    type Item = SchemaValueTree;
    type Buffer = Option<SchemaValueTree>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, Ctx>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if finish {
            self.finished = true;
            self.receiver.lifecycle.finish();
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        if !self.demand_pending {
            match self.receiver.demand_tx.try_send(()) {
                Ok(()) => self.demand_pending = true,
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.finished = true;
                    self.receiver.lifecycle.finish();
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Err(mpsc::error::TrySendError::Full(_)) => self.demand_pending = true,
            }
        }

        match self.receiver.item_rx.poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                self.finished = true;
                self.receiver.lifecycle.finish();
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                self.receiver.lifecycle.finish();
                Poll::Ready(Err(wasmtime::Error::msg(error)))
            }
            Poll::Ready(Some(Ok(value))) => {
                self.demand_pending = false;
                let encoded = {
                    let mut resolver = StoreValueResolver::new(&mut store);
                    encode_value_with_streams(&value, &mut resolver)
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))?
                };
                destination.set_buffer(Some(encoded));
                Poll::Ready(Ok(StreamResult::Completed))
            }
        }
    }
}
