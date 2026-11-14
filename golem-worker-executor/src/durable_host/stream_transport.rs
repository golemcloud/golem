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
    live_stream_bus,
};
use crate::worker::suspension::{ExternalActivity, RuntimeSource};
use crate::workerctx::WorkerCtx;
use golem_common::base_model::durable_stream::{StreamError, StreamTerminal, StreamTerminalAuthor};
use golem_schema::schema::wit::wire::{SchemaValueTree, StreamOutcome};
use golem_schema::schema::wit::{decode_value_with, encode_value_with_streams};
use golem_schema::schema::{SchemaValue, SchemaValueStream};
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Destination, FutureProducer, FutureReader, Source, StreamConsumer, StreamProducer,
    StreamResult, TerminalConsumption,
};

#[derive(Debug)]
pub(crate) struct SourceLifecycle {
    pub(super) finished: AtomicBool,
    cancelled: CancellationToken,
    acceptance_cancelled: CancellationToken,
    acceptance: Mutex<Option<NativeAcceptance>>,
    acceptance_changed: futures::task::AtomicWaker,
}

#[derive(Debug, Clone, Copy)]
struct NativeAcceptance {
    replay_items: u64,
    control_limit: Option<u64>,
}

impl SourceLifecycle {
    fn new(cancelled: CancellationToken) -> Self {
        Self {
            finished: AtomicBool::new(false),
            cancelled,
            acceptance_cancelled: CancellationToken::new(),
            acceptance: Mutex::new(None),
            acceptance_changed: futures::task::AtomicWaker::new(),
        }
    }

    pub(crate) fn source_cancellation(&self) -> CancellationToken {
        self.acceptance_cancelled.clone()
    }

    /// Installs the native acceptance boundary before the durable drain activates its source.
    pub(crate) fn configure_native_acceptance(
        &self,
        replay_items: u64,
        control_limit: Option<u64>,
    ) {
        *self
            .acceptance
            .lock()
            .expect("native acceptance lock poisoned") = Some(NativeAcceptance {
            replay_items,
            control_limit,
        });
        self.acceptance_changed.wake();
    }

    fn activate_native_acceptance(&self) {
        self.acceptance
            .lock()
            .expect("native acceptance lock poisoned")
            .get_or_insert(NativeAcceptance {
                replay_items: 0,
                control_limit: None,
            });
        self.acceptance_changed.wake();
    }

    fn poll_native_allowance(&self, accepted: u64, cx: &mut Context<'_>) -> Poll<u64> {
        self.acceptance_changed.register(cx.waker());
        if self.finished.load(Ordering::Acquire) {
            return Poll::Ready(0);
        }
        let acceptance = *self
            .acceptance
            .lock()
            .expect("native acceptance lock poisoned");
        let Some(acceptance) = acceptance else {
            return Poll::Pending;
        };
        // A reconstructed source must reproduce its prefix even when control already won.
        // Count native takes, not drain progress: the frontend can run ahead of publication.
        let limit = acceptance
            .control_limit
            .map_or(u64::MAX, |limit| limit.saturating_sub(accepted));
        Poll::Ready(if self.acceptance_cancelled.is_cancelled() {
            limit.min(acceptance.replay_items.saturating_sub(accepted))
        } else {
            limit
        })
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
        self.acceptance_changed.wake();
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
        self.lifecycle.activate_native_acceptance();
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
    accounted_output_stream_pair(capacity, runtime_teardown, None, interrupt, None)
}

pub(super) fn accounted_output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    runtime_source: Option<RuntimeSource>,
    interrupt: FrontendInterrupt,
    producer_outcome: Option<Arc<crate::durable_host::stream_outcome::ProducerOutcome>>,
) -> Result<(LiveOutputConsumer, SchemaValueStream), String> {
    let cancellation = CancellationToken::new();
    let lifecycle = Arc::new(SourceLifecycle::new(cancellation.clone()));
    let retained_producer = producer_outcome.clone();
    let (publisher, primary) = live_stream_bus(capacity, move || {
        cancellation.cancel();
        if let Some(producer) = &retained_producer {
            producer.close_items();
        }
    })
    .map_err(|error| format!("failed to create live output stream bus: {error:?}"))?;
    if let Some(producer) = &producer_outcome {
        producer.bind_publication(publisher.clone(), lifecycle.clone());
    }
    let endpoint = LiveStreamEndpoint {
        primary: Some(primary),
        lifecycle: lifecycle.clone(),
        runtime_source: runtime_source.clone(),
    };
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
            accepted_items: 0,
            producer_outcome,
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

#[cfg(test)]
pub(super) fn accounted_byte_output_stream_pair(
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    runtime_source: Option<RuntimeSource>,
    interrupt: FrontendInterrupt,
) -> Result<(LiveByteOutputConsumer, SchemaValueStream), String> {
    let (consumer, stream) =
        accounted_output_stream_pair(capacity, runtime_teardown, runtime_source, interrupt, None)?;
    Ok((LiveByteOutputConsumer(consumer), stream))
}

#[cfg(test)]
pub(super) struct LiveByteOutputConsumer(LiveOutputConsumer);

#[cfg(test)]
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
        let allowance = match self
            .0
            .lifecycle
            .poll_native_allowance(self.0.accepted_items, cx)
        {
            Poll::Pending if finish => {
                self.0.begin_terminal_publication();
                return self.0.poll_pending(cx);
            }
            Poll::Pending => {
                if let Poll::Ready(kind) = self.0.interrupt.as_mut().poll(cx) {
                    return Poll::Ready(Err(wasmtime::Error::from_anyhow(kind.into())));
                }
                return Poll::Pending;
            }
            Poll::Ready(0) => return Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Ready(allowance) => allowance,
        };
        if finish {
            self.0.begin_terminal_publication();
            return self.0.poll_pending(cx);
        }

        let mut source = source.as_direct(store);
        let count = source
            .remaining()
            .len()
            .min(64 * 1024)
            .min(usize::try_from(allowance).unwrap_or(usize::MAX));
        if count == 0 {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let bytes = source.remaining()[..count].to_vec();
        source.mark_read(count);
        self.0.accepted_items += count as u64;
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
    accounted_relay_stream_pair(capacity, None)
}

pub(super) fn accounted_relay_stream_pair(
    capacity: usize,
    runtime_source: Option<RuntimeSource>,
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
            runtime_source,
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
    accepted_items: u64,
    producer_outcome: Option<Arc<crate::durable_host::stream_outcome::ProducerOutcome>>,
}

impl LiveOutputConsumer {
    fn begin_terminal_publication(&mut self) {
        self.terminal_requested = true;
        if let Some(outcome) = &self.producer_outcome {
            outcome.close_items();
            self.pending = Some(Box::pin(async { Ok(0) }));
            return;
        }
        let publisher = self.publisher.clone();
        self.pending = Some(Box::pin(async move {
            let result = publisher.publish_end().await;
            tracing::trace!(
                succeeded = result.is_ok(),
                "Live output terminal publication completed"
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
                        tracing::trace!(
                            interrupt_kind = ?kind,
                            "Live output observation interrupted"
                        );
                        return Poll::Ready(Err(wasmtime::Error::from_anyhow(kind.into())));
                    }
                    return Poll::Pending;
                }
                Poll::Ready(result) => result,
            },
            None => return Poll::Ready(Ok(StreamResult::Completed)),
        };
        tracing::trace!(
            terminal_requested = self.terminal_requested,
            succeeded = result.is_ok(),
            closed = matches!(&result, Err(LiveStreamPublishError::Closed)),
            "Live output publication completed"
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
        if let Some(outcome) = &self.producer_outcome {
            // Accepted items already belong to the native publication lane. Closing items
            // never selects an outcome; the invocation scope still owns absent outcomes.
            outcome.close_items();
            return;
        }
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
                tracing::trace!(
                    succeeded = result.is_ok(),
                    "Retained live output publication completed"
                );
                let _ = result;
            }
            if !terminal_requested {
                tokio::task::yield_now().await;
                let teardown = runtime_teardown();
                if teardown {
                    lifecycle.abort();
                } else {
                    let result = publisher.publish_end().await;
                    tracing::trace!(
                        succeeded = result.is_ok(),
                        "Dropped live output terminal publication completed"
                    );
                    let _ = result;
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
        match self
            .lifecycle
            .poll_native_allowance(self.accepted_items, cx)
        {
            Poll::Pending if finish => {
                self.begin_terminal_publication();
                return self.poll_pending(cx);
            }
            Poll::Pending => {
                if let Poll::Ready(kind) = self.interrupt.as_mut().poll(cx) {
                    return Poll::Ready(Err(wasmtime::Error::from_anyhow(kind.into())));
                }
                return Poll::Pending;
            }
            Poll::Ready(0) => return Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Ready(_) => {}
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
        self.accepted_items += 1;

        let decoded = {
            let mut resolver = StoreValueResolver::new(&mut store);
            decode_value_with(item, &mut resolver).map_err(|error| error.to_string())
        };
        let publisher = self.publisher.clone();
        match decoded {
            Ok(value) => {
                self.pending = Some(match &self.producer_outcome {
                    Some(outcome) => {
                        let completion = outcome.publish_item(value);
                        Box::pin(async move {
                            completion
                                .await
                                .unwrap_or(Err(LiveStreamPublishError::Closed))
                        })
                    }
                    None => Box::pin(async move { publisher.publish_item(value).await }),
                });
            }
            Err(error) => {
                if self.producer_outcome.is_some() {
                    return Poll::Ready(Err(wasmtime::Error::from(
                        wasmtime::Trap::UnreachableCodeReached,
                    )
                    .context(format!("invalid schema stream item: {error}"))));
                }
                self.pending_failure = Some(error.clone());
                self.terminal_requested = true;
                self.pending = Some(Box::pin(async move {
                    publisher
                        .publish_terminal(
                            StreamTerminalAuthor::Protocol,
                            StreamTerminal::Failed(StreamError::Interrupted(error)),
                        )
                        .await
                }));
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
    completion: Option<oneshot::Sender<StreamTerminal>>,
}

struct LiveInputOutcomeProducer(Option<oneshot::Receiver<StreamTerminal>>);

impl<D: Send + 'static> FutureProducer<D> for LiveInputOutcomeProducer {
    type Item = StreamOutcome;

    fn poll_produce(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: StoreContextMut<D>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        if finish {
            return Poll::Ready(Ok(None));
        }
        let Some(receiver) = self.0.as_mut() else {
            return Poll::Pending;
        };
        let result = std::task::ready!(Pin::new(receiver).poll(cx));
        self.0 = None;
        match result {
            Ok(terminal) => Poll::Ready(
                crate::durable_host::stream_outcome::encode_outcome(&terminal).map(Some),
            ),
            // An early reader drop is not a producer outcome.
            Err(_) => Poll::Pending,
        }
    }
}

impl LiveInputProducer {
    pub(super) fn new<D: Send + 'static>(
        store: &mut impl wasmtime::AsContextMut<Data = D>,
        endpoint: LiveStreamEndpoint,
        interrupt: FrontendInterrupt,
    ) -> wasmtime::Result<(Self, FutureReader<StreamOutcome>)> {
        let lifecycle = endpoint.lifecycle.clone();
        let (sender, receiver) = oneshot::channel();
        let outcome = FutureReader::new(store, LiveInputOutcomeProducer(Some(receiver)))?;
        Ok((
            Self {
                subscriber: Some(endpoint.activate()),
                pending: None,
                lifecycle,
                finished: false,
                interrupt: Some(interrupt),
                completion: Some(sender),
            },
            outcome,
        ))
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
                LiveStreamEventPayload::Terminal { terminal, .. } => {
                    self.finished = true;
                    self.lifecycle.finish();
                    if let Some(completion) = self.completion.take() {
                        if destination.remaining(&mut store).is_some() {
                            destination.register_read_observer(&mut store, move |consumption| {
                                if consumption == TerminalConsumption::Delivered {
                                    let _ = completion.send(terminal);
                                }
                            })?;
                        } else {
                            let _ = completion.send(terminal);
                        }
                    }
                    Poll::Ready(Ok(StreamResult::Dropped))
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
    use golem_common::base_model::durable_stream::StreamTerminalAuthor;
    use std::time::Duration;
    use test_r::{test, timeout};

    #[test]
    fn native_acceptance_preserves_only_the_recorded_control_prefix() {
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        for cancel_before_configuration in [false, true] {
            let lifecycle = SourceLifecycle::new(CancellationToken::new());
            let cancelled = lifecycle.source_cancellation();
            assert!(lifecycle.poll_native_allowance(0, &mut cx).is_pending());
            if cancel_before_configuration {
                cancelled.cancel();
            }
            lifecycle.configure_native_acceptance(3, Some(3));
            lifecycle.activate_native_acceptance();
            // The allowance caps a byte batch as well as successive scalar takes, even
            // when the drain has not yet validated any of the accepted prefix.
            assert_eq!(lifecycle.poll_native_allowance(0, &mut cx), Poll::Ready(3));
            assert_eq!(lifecycle.poll_native_allowance(2, &mut cx), Poll::Ready(1));
            assert_eq!(lifecycle.poll_native_allowance(3, &mut cx), Poll::Ready(0));
            cancelled.cancel();
            assert_eq!(lifecycle.poll_native_allowance(2, &mut cx), Poll::Ready(1));
            assert_eq!(lifecycle.poll_native_allowance(3, &mut cx), Poll::Ready(0));
        }

        let lifecycle = SourceLifecycle::new(CancellationToken::new());
        lifecycle.configure_native_acceptance(3, None);
        lifecycle.activate_native_acceptance();
        // A Source terminal must still be reproduced, not converted into a native drop.
        assert_eq!(
            lifecycle.poll_native_allowance(3, &mut cx),
            Poll::Ready(u64::MAX)
        );
        lifecycle.source_cancellation().cancel();
        assert_eq!(lifecycle.poll_native_allowance(2, &mut cx), Poll::Ready(1));
        assert_eq!(lifecycle.poll_native_allowance(3, &mut cx), Poll::Ready(0));

        let lifecycle = SourceLifecycle::new(CancellationToken::new());
        lifecycle.activate_native_acceptance();
        assert_eq!(
            lifecycle.poll_native_allowance(0, &mut cx),
            Poll::Ready(u64::MAX)
        );
        lifecycle.source_cancellation().cancel();
        assert_eq!(lifecycle.poll_native_allowance(0, &mut cx), Poll::Ready(0));
    }

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
            LiveStreamEventPayload::Terminal {
                authored_by: StreamTerminalAuthor::Guest,
                terminal: StreamTerminal::Finished,
            }
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
                        LiveStreamEventPayload::Terminal {
                            terminal: StreamTerminal::Finished,
                            ..
                        } => break,
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
                payload: LiveStreamEventPayload::Terminal {
                    authored_by: StreamTerminalAuthor::Guest,
                    terminal: StreamTerminal::Finished,
                },
                ..
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
                payload: LiveStreamEventPayload::Terminal {
                    authored_by: StreamTerminalAuthor::Guest,
                    terminal: StreamTerminal::Finished,
                },
                ..
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
