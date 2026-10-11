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

use crate::durable_host::durability::ClassifiedHostError;
use crate::durable_host::schema_value_stream::StoreValueResolver;
use crate::durable_host::stream_bus::{
    LiveStreamEventPayload, LiveStreamPublishError, LiveStreamPublisher, LiveStreamReceiveError,
    PrimaryLiveStreamSubscriber, ReservedPrimaryLiveStreamSubscriber, live_output_stream_bus,
};
use crate::worker::suspension::{ExternalActivity, RuntimeSource};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::wit::wire::SchemaValueTree;
use golem_schema::schema::wit::{decode_value_with, encode_value_with_streams};
use golem_schema::schema::{SchemaValue, SchemaValueStream};
use golem_service_base::error::worker_executor::InterruptKind;
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

    pub(crate) fn abort(&self) {
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
    runtime_source: Option<RuntimeSource>,
}

impl LiveStreamEndpoint {
    pub(crate) fn runtime_source(&self) -> Option<RuntimeSource> {
        self.runtime_source.clone()
    }

    pub(crate) fn external_activity(&self) -> Option<ExternalActivity> {
        self.runtime_source
            .as_ref()
            .map(RuntimeSource::external_activity)
    }

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

pub(crate) type FrontendInterrupt = Pin<Box<dyn Future<Output = InterruptKind> + Send>>;

#[cfg(test)]
pub(super) fn output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    interrupt: FrontendInterrupt,
) -> Result<(LiveOutputConsumer, SchemaValueStream), String> {
    accounted_output_stream_pair(capacity, runtime_teardown, None, interrupt)
}

pub(super) fn accounted_output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    runtime_source: Option<RuntimeSource>,
    interrupt: FrontendInterrupt,
) -> Result<(LiveOutputConsumer, SchemaValueStream), String> {
    let cancellation = CancellationToken::new();
    let lifecycle = Arc::new(SourceLifecycle::new(cancellation.clone()));
    let (publisher, primary) = live_output_stream_bus(capacity, cancellation)
        .map_err(|error| format!("failed to create live output stream bus: {error:?}"))?;
    let endpoint = LiveStreamEndpoint {
        primary: Some(primary),
        lifecycle: lifecycle.clone(),
        runtime_source: runtime_source.clone(),
    };
    #[cfg(feature = "test-utils")]
    tracing::debug!(
        lifecycle = Arc::as_ptr(&lifecycle) as usize,
        teardown_probe = Arc::as_ptr(&runtime_teardown) as *const () as usize,
        event = "created",
        "LiveOutputConsumer.handoff"
    );
    Ok((
        LiveOutputConsumer {
            publisher,
            lifecycle,
            pending: None,
            pending_failure: None,
            terminal_requested: false,
            runtime_teardown,
            interrupt,
            runtime_source,
        },
        SchemaValueStream::from_host_endpoint(endpoint),
    ))
}

#[cfg(test)]
pub(super) fn byte_output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    interrupt: FrontendInterrupt,
) -> Result<(LiveByteOutputConsumer, SchemaValueStream), String> {
    let (consumer, stream) = output_stream_pair(capacity, runtime_teardown, interrupt)?;
    Ok((LiveByteOutputConsumer(consumer), stream))
}

pub(super) fn accounted_byte_output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    runtime_source: Option<RuntimeSource>,
    interrupt: FrontendInterrupt,
) -> Result<(LiveByteOutputConsumer, SchemaValueStream), String> {
    let (consumer, stream) =
        accounted_output_stream_pair(capacity, runtime_teardown, runtime_source, interrupt)?;
    Ok((LiveByteOutputConsumer(consumer), stream))
}

pub(super) struct LiveByteOutputConsumer(LiveOutputConsumer);

impl<D> StreamConsumer<D> for LiveByteOutputConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.0.pending.is_some() {
            return self.0.poll_pending(cx);
        }
        if finish {
            self.0.begin_terminal_publication();
            return self.0.poll_pending(cx);
        }

        let mut source = source.as_direct(store);
        let count = source.remaining().len().min(64 * 1024);
        if count == 0 {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let bytes = source.remaining()[..count].to_vec();
        source.mark_read(count);
        let publisher = self.0.publisher.clone();
        self.0.pending = Some(Box::pin(async move {
            let mut offset = 0;
            for byte in bytes {
                offset = publisher.publish_item(SchemaValue::U8(byte)).await?;
            }
            Ok(offset)
        }));
        self.0.poll_pending(cx)
    }
}

pub(crate) fn relay_stream_pair(
    capacity: usize,
) -> Result<(LiveStreamPublisher<SchemaValue>, LiveStreamEndpoint), String> {
    let cancellation = CancellationToken::new();
    let lifecycle = Arc::new(SourceLifecycle::new(cancellation.clone()));
    let (publisher, primary) = live_output_stream_bus(capacity, cancellation)
        .map_err(|error| format!("failed to create relay stream bus: {error:?}"))?;
    Ok((
        publisher,
        LiveStreamEndpoint {
            primary: Some(primary),
            lifecycle,
            runtime_source: None,
        },
    ))
}

#[cfg(test)]
pub(crate) fn test_output_stream_pair(
    capacity: usize,
) -> Result<(LiveStreamPublisher<SchemaValue>, LiveStreamEndpoint), String> {
    relay_stream_pair(capacity)
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
    interrupt: FrontendInterrupt,
    runtime_source: Option<RuntimeSource>,
}

impl LiveOutputConsumer {
    fn begin_terminal_publication(&mut self) {
        #[cfg(feature = "test-utils")]
        let lifecycle = Arc::as_ptr(&self.lifecycle) as usize;
        #[cfg(feature = "test-utils")]
        let teardown_probe = Arc::as_ptr(&self.runtime_teardown) as *const () as usize;
        #[cfg(feature = "test-utils")]
        tracing::debug!(
            lifecycle,
            teardown_probe,
            event = "finish_requested",
            "LiveOutputConsumer.handoff"
        );
        self.terminal_requested = true;
        let publisher = self.publisher.clone();
        self.pending = Some(Box::pin(async move {
            let result = publisher.publish_end().await;
            #[cfg(feature = "test-utils")]
            tracing::debug!(
                lifecycle,
                teardown_probe,
                event = "finish_publication_completed",
                ok = result.is_ok(),
                "LiveOutputConsumer.handoff"
            );
            result
        }));
    }

    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<wasmtime::Result<StreamResult>> {
        let result = match self.pending.as_mut() {
            Some(pending) => match pending.as_mut().poll(cx) {
                Poll::Pending => {
                    // Consumed items remain owned by pending publication and Drop. Terminal
                    // publication is mandatory even when guest observation is interrupted.
                    if !self.terminal_requested
                        && let Poll::Ready(kind) = self.interrupt.as_mut().poll(cx)
                    {
                        #[cfg(feature = "test-utils")]
                        tracing::debug!(
                            lifecycle = Arc::as_ptr(&self.lifecycle) as usize,
                            teardown_probe = Arc::as_ptr(&self.runtime_teardown) as *const () as usize,
                            event = "pending_publication_interrupt_ready",
                            interrupt_kind = ?std::mem::discriminant(&kind),
                            "LiveOutputConsumer.handoff"
                        );
                        return Poll::Ready(Err(wasmtime::Error::from_anyhow(kind.into())));
                    }
                    return Poll::Pending;
                }
                Poll::Ready(result) => result,
            },
            None => return Poll::Ready(Ok(StreamResult::Completed)),
        };
        #[cfg(feature = "test-utils")]
        tracing::debug!(
            lifecycle = Arc::as_ptr(&self.lifecycle) as usize,
            teardown_probe = Arc::as_ptr(&self.runtime_teardown) as *const () as usize,
            event = "publication_ready",
            terminal_requested = self.terminal_requested,
            ok = result.is_ok(),
            closed = matches!(&result, Err(LiveStreamPublishError::Closed)),
            "LiveOutputConsumer.handoff"
        );
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
        #[cfg(feature = "test-utils")]
        let lifecycle_id = Arc::as_ptr(&self.lifecycle) as usize;
        #[cfg(feature = "test-utils")]
        let teardown_probe = Arc::as_ptr(&self.runtime_teardown) as *const () as usize;
        #[cfg(feature = "test-utils")]
        tracing::debug!(
            lifecycle = lifecycle_id,
            teardown_probe,
            event = "drop_enter",
            pending = self.pending.is_some(),
            terminal_requested = self.terminal_requested,
            "LiveOutputConsumer.handoff"
        );
        let pending = self.pending.take();
        let terminal_requested = self.terminal_requested;
        let publisher = self.publisher.clone();
        let lifecycle = self.lifecycle.clone();
        let runtime_teardown = self.runtime_teardown.clone();
        let activity = self
            .runtime_source
            .as_ref()
            .map(RuntimeSource::external_activity);
        tokio::spawn(async move {
            let _activity = activity;
            if let Some(pending) = pending {
                let result = pending.await;
                #[cfg(feature = "test-utils")]
                tracing::debug!(
                    lifecycle = lifecycle_id,
                    teardown_probe,
                    event = "drop_pending_completed",
                    ok = result.is_ok(),
                    "LiveOutputConsumer.handoff"
                );
                let _ = result;
            }
            if !terminal_requested {
                tokio::task::yield_now().await;
                let teardown = runtime_teardown();
                #[cfg(feature = "test-utils")]
                tracing::debug!(
                    lifecycle = lifecycle_id,
                    teardown_probe,
                    event = "drop_teardown_decision",
                    teardown,
                    "LiveOutputConsumer.handoff"
                );
                if teardown {
                    lifecycle.abort();
                    #[cfg(feature = "test-utils")]
                    tracing::debug!(
                        lifecycle = lifecycle_id,
                        teardown_probe,
                        event = "drop_aborted",
                        "LiveOutputConsumer.handoff"
                    );
                } else {
                    #[cfg(feature = "test-utils")]
                    tracing::debug!(
                        lifecycle = lifecycle_id,
                        teardown_probe,
                        event = "drop_publication_enter",
                        "LiveOutputConsumer.handoff"
                    );
                    let result = publisher.publish_end().await;
                    #[cfg(feature = "test-utils")]
                    tracing::debug!(
                        lifecycle = lifecycle_id,
                        teardown_probe,
                        event = "drop_publication_completed",
                        ok = result.is_ok(),
                        "LiveOutputConsumer.handoff"
                    );
                    let _ = result;
                    lifecycle.finish();
                }
            } else {
                lifecycle.finish();
                #[cfg(feature = "test-utils")]
                tracing::debug!(
                    lifecycle = lifecycle_id,
                    teardown_probe,
                    event = "drop_terminal_finished",
                    "LiveOutputConsumer.handoff"
                );
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

type InputEvent = Option<
    Result<crate::durable_host::stream_bus::LiveStreamEvent<SchemaValue>, LiveStreamReceiveError>,
>;
type ReceiveFuture = Pin<
    Box<
        dyn Future<
                Output = (
                    PrimaryLiveStreamSubscriber<SchemaValue>,
                    FrontendInterrupt,
                    Result<InputEvent, InterruptKind>,
                ),
            > + Send,
    >,
>;

pub(super) async fn receive_input_event(
    mut subscriber: PrimaryLiveStreamSubscriber<SchemaValue>,
    cancelled: CancellationToken,
    mut interrupt: FrontendInterrupt,
) -> (
    PrimaryLiveStreamSubscriber<SchemaValue>,
    FrontendInterrupt,
    Result<InputEvent, InterruptKind>,
) {
    let event = tokio::select! {
        biased;
        event = subscriber.recv() => Ok(Some(event)),
        kind = &mut interrupt => Err(kind),
        _ = cancelled.cancelled() => Ok(None),
    };
    (subscriber, interrupt, event)
}

pub(super) struct LiveInputProducer {
    subscriber: Option<PrimaryLiveStreamSubscriber<SchemaValue>>,
    pending: Option<ReceiveFuture>,
    lifecycle: Arc<SourceLifecycle>,
    finished: bool,
    interrupt: Option<FrontendInterrupt>,
}

impl LiveInputProducer {
    pub(super) fn new(endpoint: LiveStreamEndpoint, interrupt: FrontendInterrupt) -> Self {
        let lifecycle = endpoint.lifecycle.clone();
        Self {
            subscriber: Some(endpoint.activate()),
            pending: None,
            lifecycle,
            finished: false,
            interrupt: Some(interrupt),
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
            let interrupt = self
                .interrupt
                .take()
                .expect("live input interrupt is missing");
            self.pending = Some(Box::pin(receive_input_event(
                subscriber, cancelled, interrupt,
            )));
        }
        let (subscriber, interrupt, event) = match self.pending.as_mut().unwrap().as_mut().poll(cx)
        {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        self.pending = None;
        self.subscriber = Some(subscriber);
        self.interrupt = Some(interrupt);
        let event = match event {
            Ok(event) => event,
            Err(kind) => {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::from_anyhow(kind.into())));
            }
        };

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
                LiveStreamEventPayload::ClassifiedError { kind, message } => {
                    self.finished = true;
                    self.lifecycle.finish();
                    Poll::Ready(Err(wasmtime::Error::from_anyhow(anyhow::Error::new(
                        ClassifiedHostError { kind, message },
                    ))))
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
    #[timeout("5s")]
    async fn quiet_live_input_observes_typed_stop_without_terminal() {
        let (_publisher, endpoint) = relay_stream_pair(1).unwrap();
        let lifecycle = endpoint.lifecycle();
        let (stop, signal) = tokio::sync::oneshot::channel();
        let kind = golem_service_base::error::worker_executor::InterruptKind::Suspend(
            golem_common::model::Timestamp::now_utc(),
        );
        let mut receive = Box::pin(receive_input_event(
            endpoint.activate(),
            lifecycle.cancelled.clone(),
            Box::pin(async move { signal.await.unwrap() }),
        ));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(receive.as_mut().poll(&mut cx).is_pending());
        stop.send(kind).unwrap();
        let (_subscriber, _signal, result) =
            tokio::time::timeout(Duration::from_millis(100), receive)
                .await
                .expect("quiet live demand must observe the published stop");
        assert_eq!(result.unwrap_err(), kind);
        assert!(!lifecycle.is_aborted());
    }

    #[test]
    #[timeout("5s")]
    async fn bounded_live_publication_observation_retains_consumed_item() {
        let (mut consumer, stream) =
            output_stream_pair(1, Arc::new(|| true), Box::pin(std::future::pending())).unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let lifecycle = endpoint.lifecycle();
        let mut primary = endpoint.activate();
        consumer
            .publisher
            .publish_item(SchemaValue::U8(11))
            .await
            .unwrap();
        let publisher = consumer.publisher.clone();
        consumer.pending = Some(Box::pin(async move {
            publisher.publish_item(SchemaValue::U8(22)).await
        }));
        let (stop, signal) = tokio::sync::oneshot::channel();
        let kind = golem_service_base::error::worker_executor::InterruptKind::Suspend(
            golem_common::model::Timestamp::now_utc(),
        );
        consumer.interrupt = Box::pin(async move { signal.await.unwrap() });
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(consumer.poll_pending(&mut cx).is_pending());
        stop.send(kind).unwrap();
        let error = tokio::time::timeout(
            Duration::from_millis(100),
            std::future::poll_fn(|cx| consumer.poll_pending(cx)),
        )
        .await
        .expect("bounded publication observation must stop")
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<golem_service_base::error::worker_executor::InterruptKind>(),
            Some(&kind)
        );
        assert!(consumer.pending.is_some());
        drop(consumer);
        assert!(!lifecycle.finished.load(Ordering::Acquire));
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(11))
        ));
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(22))
        ));
        lifecycle.cancelled().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), primary.recv())
                .await
                .is_err()
        );
    }

    #[test]
    #[timeout("5s")]
    async fn bounded_byte_frontend_stop_retains_already_consumed_batch() {
        struct Observed(LiveByteOutputConsumer, Arc<tokio::sync::Notify>);
        impl<D> StreamConsumer<D> for Observed {
            type Item = u8;
            fn poll_consume(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                store: StoreContextMut<D>,
                source: Source<u8>,
                finish: bool,
            ) -> Poll<wasmtime::Result<StreamResult>> {
                let result = Pin::new(&mut self.0).poll_consume(cx, store, source, finish);
                if result.is_pending() {
                    self.1.notify_one();
                }
                result
            }
        }
        let (stop, signal) = tokio::sync::oneshot::channel();
        let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
        let (consumer, stream) = byte_output_stream_pair(
            1,
            Arc::new(|| true),
            Box::pin(async move { signal.await.unwrap() }),
        )
        .unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let lifecycle = endpoint.lifecycle();
        let mut primary = endpoint.activate();
        let pending = Arc::new(tokio::sync::Notify::new());
        let mut config = wasmtime::Config::new();
        config.concurrency_support(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        let observed = store
            .run_concurrent(async |accessor| -> wasmtime::Result<()> {
                accessor.with(|mut store| {
                    let reader =
                        wasmtime::component::StreamReader::new(&mut store, vec![11u8, 22])?;
                    reader.pipe(&mut store, Observed(consumer, pending.clone()))
                })?;
                pending.notified().await;
                stop.send(kind).unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(())
            })
            .await
            .expect_err("bounded byte publication observation must stop before the primary drains");
        assert_eq!(observed.downcast_ref::<InterruptKind>(), Some(&kind));
        drop(store);
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(11))
        ));
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(22))
        ));
        lifecycle.cancelled().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), primary.recv())
                .await
                .is_err()
        );
    }

    #[test]
    #[timeout("5s")]
    async fn ready_item_and_mandatory_terminal_keep_publication_ownership() {
        let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
        let (mut consumer, stream) =
            output_stream_pair(1, Arc::new(|| false), Box::pin(std::future::ready(kind))).unwrap();
        let mut primary = stream
            .take_host_endpoint::<LiveStreamEndpoint>()
            .unwrap()
            .activate();
        let publisher = consumer.publisher.clone();
        consumer.pending = Some(Box::pin(async move {
            publisher.publish_item(SchemaValue::U8(17)).await
        }));
        assert!(matches!(
            std::future::poll_fn(|cx| consumer.poll_pending(cx))
                .await
                .unwrap(),
            StreamResult::Completed
        ));
        consumer.begin_terminal_publication();
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(
            consumer.poll_pending(&mut cx).is_pending(),
            "terminal publication is not an interruptible observation"
        );
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(17))
        ));
        assert!(matches!(
            std::future::poll_fn(|cx| consumer.poll_pending(cx))
                .await
                .unwrap(),
            StreamResult::Cancelled
        ));
        assert!(matches!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::End
        ));
    }

    #[test]
    #[timeout("5s")]
    async fn ready_live_item_wins_over_published_stop() {
        let (publisher, endpoint) = relay_stream_pair(1).unwrap();
        let cancelled = endpoint.lifecycle().cancelled.clone();
        let subscriber = endpoint.activate();
        publisher.publish_item(SchemaValue::U8(17)).await.unwrap();
        let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
        let (_subscriber, _signal, event) =
            receive_input_event(subscriber, cancelled, Box::pin(std::future::ready(kind))).await;
        assert!(matches!(
            event.unwrap().unwrap().unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(17))
        ));
    }

    #[test]
    #[timeout("5s")]
    async fn byte_output_preserves_binary_values_across_bounded_publications() {
        struct BytesProducer(Option<bytes::Bytes>);

        impl<D> StreamProducer<D> for BytesProducer {
            type Item = u8;
            type Buffer = bytes::Bytes;

            fn poll_produce<'a>(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                store: StoreContextMut<'a, D>,
                mut destination: Destination<'a, Self::Item, Self::Buffer>,
                finish: bool,
            ) -> Poll<wasmtime::Result<StreamResult>> {
                if finish {
                    return Poll::Ready(Ok(StreamResult::Cancelled));
                }
                if destination.remaining(store) == Some(0) {
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                match self.0.take() {
                    Some(bytes) => {
                        destination.set_buffer(bytes);
                        Poll::Ready(Ok(StreamResult::Completed))
                    }
                    None => Poll::Ready(Ok(StreamResult::Dropped)),
                }
            }
        }

        let mut config = wasmtime::Config::new();
        config.concurrency_support(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        let expected = vec![0, 255, 128, 1, 17, 0, 252];
        let reader = wasmtime::component::StreamReader::new(
            &mut store,
            BytesProducer(Some(expected.clone().into())),
        )
        .unwrap();
        let (consumer, stream) =
            byte_output_stream_pair(2, Arc::new(|| false), Box::pin(std::future::pending()))
                .unwrap();
        let mut primary = stream
            .take_host_endpoint::<LiveStreamEndpoint>()
            .unwrap()
            .activate();
        let actual = store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<u8>> {
                accessor.with(|mut store| reader.pipe(&mut store, consumer))?;
                let mut bytes = Vec::new();
                loop {
                    let event = primary.recv().await.unwrap();
                    assert_eq!(event.offset, bytes.len() as u64);
                    match event.payload {
                        LiveStreamEventPayload::Item(SchemaValue::U8(byte)) => bytes.push(byte),
                        LiveStreamEventPayload::End => break,
                        other => panic!("unexpected byte output event: {other:?}"),
                    }
                }
                Ok(bytes)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    #[timeout("2s")]
    async fn normal_output_finish_publishes_end_before_finishing_lifecycle() {
        let (mut consumer, stream) =
            output_stream_pair(4, Arc::new(|| false), Box::pin(std::future::pending())).unwrap();
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
        let (consumer, stream) =
            output_stream_pair(4, Arc::new(|| false), Box::pin(std::future::pending())).unwrap();
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
        let (consumer, stream) =
            output_stream_pair(4, Arc::new(|| true), Box::pin(std::future::pending())).unwrap();
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
