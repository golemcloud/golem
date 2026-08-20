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
use crate::durable_host::stream_bus::{
    LiveStreamEventPayload, LiveStreamPublishError, LiveStreamPublisher, LiveStreamReceiveError,
    PrimaryLiveStreamSubscriber, ReservedPrimaryLiveStreamSubscriber, live_input_stream_bus,
    live_output_stream_bus,
};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::wit::wire::SchemaValueTree;
use golem_schema::schema::wit::{decode_value_with, encode_value_with_streams};
use golem_schema::schema::{SchemaValue, SchemaValueStream};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, Source, StreamConsumer, StreamProducer, StreamResult};

/// Tracks source endpoints created by one live streaming invocation. The
/// invocation keeps its Store event loop running until every source has
/// published its terminal or its primary reader has been lost.
#[derive(Debug)]
pub(crate) struct LiveStreamTracker {
    pub(super) active: AtomicUsize,
    changed: Notify,
    cancelled: CancellationToken,
    capacity: usize,
}

impl LiveStreamTracker {
    pub(crate) fn new(cancelled: CancellationToken, capacity: usize) -> Self {
        assert!(capacity > 0, "live stream bus capacity must be non-zero");
        Self {
            active: AtomicUsize::new(0),
            changed: Notify::new(),
            cancelled,
            capacity,
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

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.cancelled.clone()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }
}

#[derive(Debug)]
pub(super) struct SourceLifecycle {
    pub(super) finished: AtomicBool,
    finished_notify: Notify,
    trackers: Mutex<Vec<Arc<LiveStreamTracker>>>,
    cancelled: CancellationToken,
}

impl SourceLifecycle {
    fn new(cancelled: CancellationToken) -> Self {
        Self {
            finished: AtomicBool::new(false),
            finished_notify: Notify::new(),
            trackers: Mutex::new(Vec::new()),
            cancelled,
        }
    }

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

    pub(super) fn finish(&self) {
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

pub(super) struct LiveStreamEndpoint {
    primary: Option<ReservedPrimaryLiveStreamSubscriber<SchemaValue>>,
    publisher: LiveStreamPublisher<SchemaValue>,
    lifecycle: Arc<SourceLifecycle>,
}

impl LiveStreamEndpoint {
    pub(super) fn attach(&self, tracker: Arc<LiveStreamTracker>) {
        self.lifecycle.attach(tracker);
    }

    pub(super) fn lifecycle(&self) -> Arc<SourceLifecycle> {
        self.lifecycle.clone()
    }

    pub(super) fn publisher(&self) -> LiveStreamPublisher<SchemaValue> {
        self.publisher.clone()
    }

    pub(super) fn activate(mut self) -> PrimaryLiveStreamSubscriber<SchemaValue> {
        self.primary
            .take()
            .expect("live stream primary subscriber already activated")
            .activate()
    }
}

impl Drop for LiveStreamEndpoint {
    fn drop(&mut self) {
        if self.primary.is_some() {
            self.lifecycle.finish();
        }
    }
}

#[derive(Clone)]
pub(super) struct LiveStreamPeer {
    pub(super) publisher: LiveStreamPublisher<SchemaValue>,
    pub(super) primary_dropped: Arc<Notify>,
    pub(super) lifecycle: Arc<SourceLifecycle>,
}

pub(super) fn input_stream_pair(
    capacity: usize,
    invocation_cancellation: &CancellationToken,
) -> Result<(LiveStreamPeer, SchemaValueStream), String> {
    let stream_cancellation = invocation_cancellation.child_token();
    let primary_dropped = Arc::new(Notify::new());
    let lifecycle = Arc::new(SourceLifecycle::new(stream_cancellation.clone()));
    let (publisher, primary) =
        live_input_stream_bus(capacity, stream_cancellation, primary_dropped.clone())
            .map_err(|error| format!("failed to create live input stream bus: {error:?}"))?;
    let endpoint = LiveStreamEndpoint {
        primary: Some(primary),
        publisher: publisher.clone(),
        lifecycle: lifecycle.clone(),
    };
    Ok((
        LiveStreamPeer {
            publisher,
            primary_dropped,
            lifecycle,
        },
        SchemaValueStream::from_host_endpoint(endpoint),
    ))
}

pub(super) fn output_stream_pair(
    tracker: Option<Arc<LiveStreamTracker>>,
    capacity: usize,
) -> Result<(LiveOutputConsumer, SchemaValueStream), String> {
    let cancellation = tracker
        .as_ref()
        .map(|tracker| tracker.cancellation_token())
        .unwrap_or_default();
    let lifecycle = Arc::new(SourceLifecycle::new(cancellation.clone()));
    if let Some(tracker) = tracker {
        debug_assert_eq!(capacity, tracker.capacity());
        lifecycle.attach(tracker);
    }
    let (publisher, primary) = live_output_stream_bus(capacity, cancellation)
        .map_err(|error| format!("failed to create live output stream bus: {error:?}"))?;
    let endpoint = LiveStreamEndpoint {
        primary: Some(primary),
        publisher: publisher.clone(),
        lifecycle: lifecycle.clone(),
    };
    Ok((
        LiveOutputConsumer {
            publisher,
            lifecycle,
            pending: None,
            pending_failure: None,
            terminal_requested: false,
        },
        SchemaValueStream::from_host_endpoint(endpoint),
    ))
}

type PublicationFuture =
    Pin<Box<dyn Future<Output = Result<u64, LiveStreamPublishError>> + Send + 'static>>;

pub(super) struct LiveOutputConsumer {
    publisher: LiveStreamPublisher<SchemaValue>,
    lifecycle: Arc<SourceLifecycle>,
    pending: Option<PublicationFuture>,
    pending_failure: Option<String>,
    terminal_requested: bool,
}

impl LiveOutputConsumer {
    fn begin_terminal_publication(&mut self) {
        self.terminal_requested = true;
        let publisher = self.publisher.clone();
        self.pending = Some(Box::pin(async move { publisher.publish_end().await }));
    }

    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<wasmtime::Result<StreamResult>> {
        let result = match self.pending.as_mut() {
            Some(pending) => match pending.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(result) => result,
            },
            None => return Poll::Ready(Ok(StreamResult::Completed)),
        };
        self.pending = None;
        match result {
            Ok(_) => match self.pending_failure.take() {
                Some(_) => {
                    self.lifecycle.finish();
                    Poll::Ready(Ok(StreamResult::Dropped))
                }
                None if self.terminal_requested => {
                    self.lifecycle.finish();
                    Poll::Ready(Ok(StreamResult::Cancelled))
                }
                None => Poll::Ready(Ok(StreamResult::Completed)),
            },
            Err(LiveStreamPublishError::Closed) => {
                self.lifecycle.finish();
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Err(error) => {
                self.lifecycle.finish();
                Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "failed to publish live output stream event: {error:?}"
                ))))
            }
        }
    }
}

impl Drop for LiveOutputConsumer {
    fn drop(&mut self) {
        let pending = self.pending.take();
        let terminal_requested = self.terminal_requested;
        let publisher = self.publisher.clone();
        let lifecycle = self.lifecycle.clone();
        tokio::spawn(async move {
            if let Some(pending) = pending {
                let _ = pending.await;
            }
            if !terminal_requested {
                let _ = publisher.publish_end().await;
            }
            lifecycle.finish();
        });
    }
}

impl<Ctx: WorkerCtx> StreamConsumer<Ctx> for LiveOutputConsumer {
    type Item = SchemaValueTree;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<Ctx>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.pending.is_some() {
            return self.poll_pending(cx);
        }
        if finish {
            self.begin_terminal_publication();
            return self.poll_pending(cx);
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
        let publisher = self.publisher.clone();
        match decoded {
            Ok(value) => {
                self.pending = Some(Box::pin(async move { publisher.publish_item(value).await }));
            }
            Err(error) => {
                self.pending_failure = Some(error.clone());
                self.terminal_requested = true;
                self.pending = Some(Box::pin(
                    async move { publisher.publish_error(error).await },
                ));
            }
        }
        self.poll_pending(cx)
    }
}

type ReceiveFuture = Pin<
    Box<
        dyn Future<
                Output = (
                    PrimaryLiveStreamSubscriber<SchemaValue>,
                    Option<
                        Result<
                            crate::durable_host::stream_bus::LiveStreamEvent<SchemaValue>,
                            LiveStreamReceiveError,
                        >,
                    >,
                ),
            > + Send
            + 'static,
    >,
>;

async fn receive_input_event(
    mut subscriber: PrimaryLiveStreamSubscriber<SchemaValue>,
    cancelled: CancellationToken,
) -> (
    PrimaryLiveStreamSubscriber<SchemaValue>,
    Option<
        Result<
            crate::durable_host::stream_bus::LiveStreamEvent<SchemaValue>,
            LiveStreamReceiveError,
        >,
    >,
) {
    let event = tokio::select! {
        event = subscriber.recv() => Some(event),
        _ = cancelled.cancelled() => None,
    };
    (subscriber, event)
}

pub(super) struct LiveInputProducer {
    subscriber: Option<PrimaryLiveStreamSubscriber<SchemaValue>>,
    pending: Option<ReceiveFuture>,
    lifecycle: Arc<SourceLifecycle>,
    finished: bool,
}

impl LiveInputProducer {
    pub(super) fn new(endpoint: LiveStreamEndpoint) -> Self {
        let lifecycle = endpoint.lifecycle.clone();
        Self {
            subscriber: Some(endpoint.activate()),
            pending: None,
            lifecycle,
            finished: false,
        }
    }
}

impl Drop for LiveInputProducer {
    fn drop(&mut self) {
        self.lifecycle.finish();
    }
}

impl<Ctx: WorkerCtx> StreamProducer<Ctx> for LiveInputProducer {
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
            self.pending = None;
            self.subscriber = None;
            self.lifecycle.finish();
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        if self.pending.is_none() {
            let subscriber = self
                .subscriber
                .take()
                .expect("live input stream subscriber is missing");
            let cancelled = self.lifecycle.cancelled.clone();
            self.pending = Some(Box::pin(receive_input_event(subscriber, cancelled)));
        }
        let (subscriber, event) = match self.pending.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        self.pending = None;
        self.subscriber = Some(subscriber);

        match event {
            Some(Ok(event)) => match event.payload {
                LiveStreamEventPayload::Item(value) => {
                    let encoded = {
                        let mut resolver = StoreValueResolver::new(&mut store);
                        encode_value_with_streams(&value, &mut resolver)
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))?
                    };
                    destination.set_buffer(Some(encoded));
                    Poll::Ready(Ok(StreamResult::Completed))
                }
                LiveStreamEventPayload::End | LiveStreamEventPayload::Cancel(_) => {
                    self.finished = true;
                    self.lifecycle.finish();
                    Poll::Ready(Ok(StreamResult::Dropped))
                }
                LiveStreamEventPayload::Error(error) => {
                    self.finished = true;
                    self.lifecycle.finish();
                    Poll::Ready(Err(wasmtime::Error::msg(error)))
                }
            },
            Some(Err(LiveStreamReceiveError::Closed)) => {
                self.finished = true;
                self.lifecycle.finish();
                Poll::Ready(Err(wasmtime::Error::msg(
                    "live input stream closed without a terminal event",
                )))
            }
            Some(Err(LiveStreamReceiveError::Lagged(missed))) => {
                self.finished = true;
                self.lifecycle.finish();
                Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "live input stream lost {missed} events"
                ))))
            }
            None => {
                self.finished = true;
                self.subscriber = None;
                self.lifecycle.finish();
                Poll::Ready(Err(wasmtime::Error::msg(
                    "live streaming invocation was cancelled",
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::{test, timeout};

    #[test]
    #[timeout("2s")]
    async fn normal_output_finish_publishes_end_before_finishing_lifecycle() {
        let tracker = Arc::new(LiveStreamTracker::new(CancellationToken::new(), 4));
        let (mut consumer, stream) = output_stream_pair(Some(tracker.clone()), 4).unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let mut primary = endpoint.activate();

        consumer.begin_terminal_publication();
        let result = std::future::poll_fn(|cx| consumer.poll_pending(cx))
            .await
            .unwrap();

        assert!(matches!(result, StreamResult::Cancelled));
        assert!(consumer.lifecycle.finished.load(Ordering::Acquire));
        assert!(matches!(
            primary.recv().await.unwrap(),
            crate::durable_host::stream_bus::LiveStreamEvent {
                offset: 0,
                payload: LiveStreamEventPayload::End,
            }
        ));
        tracker.wait_for_sources().await;
    }

    #[test]
    #[timeout("2s")]
    async fn invocation_cancellation_wakes_a_guest_blocked_on_input() {
        let invocation_cancellation = CancellationToken::new();
        let session_cancellation = CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(invocation_cancellation.clone(), 4));
        let (peer, stream) = input_stream_pair(4, &session_cancellation).unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        endpoint.attach(tracker.clone());
        let primary = endpoint.activate();
        let blocked = tokio::spawn(receive_input_event(
            primary,
            peer.lifecycle.cancelled.clone(),
        ));
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());

        invocation_cancellation.cancel();

        let (primary, event) = blocked.await.unwrap();
        assert_eq!(event, None);
        drop(primary);
        peer.primary_dropped.notified().await;
        tracker.wait_for_sources().await;
        assert!(!session_cancellation.is_cancelled());
    }
}
