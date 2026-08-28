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
    PrimaryLiveStreamSubscriber, ReservedPrimaryLiveStreamSubscriber, live_output_stream_bus,
};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::wit::wire::SchemaValueTree;
use golem_schema::schema::wit::{decode_value_with, encode_value_with_streams};
use golem_schema::schema::{SchemaValue, SchemaValueStream};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use tokio_util::sync::CancellationToken;
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, Source, StreamConsumer, StreamProducer, StreamResult};

#[derive(Debug)]
pub(crate) struct SourceLifecycle {
    pub(super) finished: AtomicBool,
    cancelled: CancellationToken,
}

impl SourceLifecycle {
    fn new(cancelled: CancellationToken) -> Self {
        Self {
            finished: AtomicBool::new(false),
            cancelled,
        }
    }

    fn abort(&self) {
        self.cancelled.cancel();
        self.finish();
    }

    pub(crate) fn is_aborted(&self) -> bool {
        self.cancelled.is_cancelled()
    }

    pub(crate) async fn cancelled(&self) {
        self.cancelled.cancelled().await;
    }

    pub(crate) fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }
}

pub(crate) struct LiveStreamEndpoint {
    primary: Option<ReservedPrimaryLiveStreamSubscriber<SchemaValue>>,
    lifecycle: Arc<SourceLifecycle>,
}

impl LiveStreamEndpoint {
    pub(crate) fn lifecycle(&self) -> Arc<SourceLifecycle> {
        self.lifecycle.clone()
    }

    pub(crate) fn activate(mut self) -> PrimaryLiveStreamSubscriber<SchemaValue> {
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

pub(super) fn output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
) -> Result<(LiveOutputConsumer, SchemaValueStream), String> {
    let cancellation = CancellationToken::new();
    let lifecycle = Arc::new(SourceLifecycle::new(cancellation.clone()));
    let (publisher, primary) = live_output_stream_bus(capacity, cancellation)
        .map_err(|error| format!("failed to create live output stream bus: {error:?}"))?;
    let endpoint = LiveStreamEndpoint {
        primary: Some(primary),
        lifecycle: lifecycle.clone(),
    };
    Ok((
        LiveOutputConsumer {
            publisher,
            lifecycle,
            pending: None,
            pending_failure: None,
            terminal_requested: false,
            runtime_teardown,
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
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
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
        let runtime_teardown = self.runtime_teardown.clone();
        tokio::spawn(async move {
            if let Some(pending) = pending {
                let _ = pending.await;
            }
            if !terminal_requested {
                tokio::task::yield_now().await;
                if runtime_teardown() {
                    lifecycle.abort();
                } else {
                    let _ = publisher.publish_end().await;
                    lifecycle.finish();
                }
            } else {
                lifecycle.finish();
            }
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
                LiveStreamEventPayload::End => {
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
    use std::time::Duration;
    use test_r::{test, timeout};

    #[test]
    #[timeout("2s")]
    async fn normal_output_finish_publishes_end_before_finishing_lifecycle() {
        let (mut consumer, stream) = output_stream_pair(4, Arc::new(|| false)).unwrap();
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
    }

    #[test]
    #[timeout("2s")]
    async fn output_drop_during_running_invocation_publishes_end() {
        let (consumer, stream) = output_stream_pair(4, Arc::new(|| false)).unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let lifecycle = endpoint.lifecycle();
        let mut primary = endpoint.activate();

        drop(consumer);

        assert!(matches!(
            primary.recv().await.unwrap(),
            crate::durable_host::stream_bus::LiveStreamEvent {
                offset: 0,
                payload: LiveStreamEventPayload::End,
            }
        ));
        tokio::time::timeout(Duration::from_millis(20), async {
            while !lifecycle.finished.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(lifecycle.finished.load(Ordering::Acquire));
        assert!(!lifecycle.is_aborted());
    }

    #[test]
    #[timeout("2s")]
    async fn output_runtime_teardown_aborts_without_publishing_a_terminal() {
        let (consumer, stream) = output_stream_pair(4, Arc::new(|| true)).unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let lifecycle = endpoint.lifecycle();
        let mut primary = endpoint.activate();

        drop(consumer);

        lifecycle.cancelled.cancelled().await;
        assert!(lifecycle.is_aborted());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), primary.recv())
                .await
                .is_err()
        );
    }
}
