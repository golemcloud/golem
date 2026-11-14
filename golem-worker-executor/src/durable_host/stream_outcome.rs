// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::durability::DurabilityHost;
use crate::durable_host::stream_bus::{LiveStreamPublishError, LiveStreamPublisher};
use crate::durable_host::stream_transport::SourceLifecycle;
use crate::worker::tasks::{
    JoinedChildren, NativeReconstructionMonitor, NativeReconstructionSpawner, WorkerTasks,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::durable_stream::{
    StreamError, StreamTerminal, StreamTerminalAuthor, StreamTerminalOrigin,
};
use golem_schema::schema::SchemaValue;
use golem_schema::schema::wit::{decode_typed, encode_typed, wire};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use wasmtime::StoreContextMut;
use wasmtime::component::{FutureConsumer, Source};

pub(crate) fn encode_outcome(terminal: &StreamTerminal) -> wasmtime::Result<wire::StreamOutcome> {
    Ok(match terminal {
        StreamTerminal::Finished => Ok(()),
        StreamTerminal::Failed(error) => Err(match error {
            StreamError::Failed(value) => wire::StreamError::Failed(
                encode_typed(value).map_err(|error| wasmtime::Error::msg(error.to_string()))?,
            ),
            StreamError::Trapped(details) => wire::StreamError::Trapped(details.clone()),
            StreamError::Cancelled => wire::StreamError::Cancelled,
            StreamError::Abandoned => wire::StreamError::Abandoned,
            StreamError::Interrupted(details) => wire::StreamError::Interrupted(details.clone()),
            StreamError::ResourceExhausted(details) => {
                wire::StreamError::ResourceExhausted(details.clone())
            }
        }),
        StreamTerminal::ConsumerCancelled { .. } => {
            return Err(wasmtime::Error::msg(
                "consumer cancellation is not a producer outcome",
            ));
        }
    })
}

pub(crate) fn supervise_schema_outputs<T: Send + 'static>(
    store: &mut impl wasmtime::AsContextMut<Data = T>,
    outputs: NativeReconstructionMonitor,
    runtime: Option<Arc<crate::worker::suspension::RuntimeStore>>,
    stop: CancellationToken,
) -> wasmtime::Result<()> {
    store.as_context_mut().spawn(async move |accessor| {
        // The supervisor only observes work already accounted by the native drains.
        if let (Some(runtime), Some(activity)) = (&runtime, accessor.runtime_activity()) {
            runtime.certify_root(activity);
        }
        tokio::select! {
            biased;
            _ = stop.cancelled() => Ok(()),
            error = outputs.wait_for_failure() => {
                Err(wasmtime::Error::from_anyhow(error.into()))
            }
        }
    })?;
    Ok(())
}

pub(crate) async fn drain_schema_outputs<Ctx: WorkerCtx>(
    accessor: &wasmtime::component::Accessor<Ctx>,
) -> Result<(), WorkerExecutorError> {
    let (outputs, interrupt, runtime) = accessor.with(|mut access| {
        let ctx = access.get().durable_ctx();
        Ok::<_, WorkerExecutorError>((
            ctx.schema_output_drain_monitor()?,
            ctx.create_interrupt_signal(),
            ctx.runtime_suspension.clone(),
        ))
    })?;
    let (sender, result) = oneshot::channel();
    // Native drainage must be a scheduler-owned task. An external wait in the root
    // alone looks deadlocked once all guest and Store tasks have exited.
    accessor
        .spawn(async move |accessor| {
            if let (Some(runtime), Some(activity)) = (&runtime, accessor.runtime_activity()) {
                runtime.certify_root(activity);
            }
            let _ = sender.send(outputs.drain_interruptibly(interrupt).await);
            Ok(())
        })
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    result.await.map_err(|_| {
        WorkerExecutorError::runtime("schema output drainage ended without a result")
    })?
}

/// Runtime ownership of schema producers, with active registrations for the current body.
/// Normal body release preserves local readers; runtime loss revokes every retained lane.
pub(crate) struct ProducerOutcomeScope {
    producers: Vec<Arc<ProducerOutcome>>,
    released: Vec<Weak<ProducerOutcome>>,
    projection_trap: watch::Sender<Option<String>>,
    tasks: WorkerTasks,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelectedProducerOutcome {
    pub authored_by: StreamTerminalAuthor,
    pub origin: StreamTerminalOrigin,
    pub terminal: StreamTerminal,
}

pub(crate) struct ProducerOutcome {
    selected: watch::Sender<Option<SelectedProducerOutcome>>,
    settled: CancellationToken,
    publication: Mutex<Option<ProducerPublication>>,
    tasks: WorkerTasks,
}

struct ProducerPublication {
    publisher: LiveStreamPublisher<SchemaValue>,
    sender: mpsc::UnboundedSender<ProducerPublicationCommand>,
    closed: bool,
    tasks: JoinedChildren<Result<(), LiveStreamPublishError>>,
    lifecycle: Arc<SourceLifecycle>,
}

impl Drop for ProducerPublication {
    fn drop(&mut self) {
        if !self.tasks.is_empty() {
            self.lifecycle.abort();
        }
    }
}

enum ProducerPublicationCommand {
    Item(
        Pin<Box<dyn Future<Output = Result<u64, LiveStreamPublishError>> + Send>>,
        oneshot::Sender<Result<u64, LiveStreamPublishError>>,
    ),
    Close,
}

impl Drop for ProducerOutcomeScope {
    fn drop(&mut self) {
        self.revoke();
    }
}

impl Default for ProducerOutcomeScope {
    fn default() -> Self {
        Self::new(WorkerTasks::default())
    }
}

impl ProducerOutcomeScope {
    pub(crate) fn revoke(&self) {
        for producer in self
            .producers
            .iter()
            .cloned()
            .chain(self.released.iter().filter_map(Weak::upgrade))
        {
            if let Some(publication) = producer.publication.lock().unwrap().take() {
                drop(publication);
            }
        }
    }

    pub(crate) fn new(tasks: WorkerTasks) -> Self {
        Self {
            producers: Vec::new(),
            released: Vec::new(),
            projection_trap: watch::channel(None).0,
            tasks,
        }
    }

    pub(crate) fn projection_trap(&self) -> watch::Receiver<Option<String>> {
        self.projection_trap.subscribe()
    }

    pub(crate) fn register(&mut self) -> Arc<ProducerOutcome> {
        let producer = Arc::new(ProducerOutcome {
            selected: watch::channel(None).0,
            settled: CancellationToken::new(),
            publication: Mutex::new(None),
            tasks: self.tasks.clone(),
        });
        self.producers.push(producer.clone());
        producer
    }

    /// Called only after the producing body's guest tasks and local tail work settle.
    pub(crate) fn settle(&mut self, result: Result<(), String>) {
        if let Err(details) = &result {
            self.projection_trap.send_replace(Some(details.clone()));
        }
        // Projections retained by an earlier body cannot inherit a later body's trap.
        self.projection_trap = watch::channel(None).0;
        for producer in &self.producers {
            producer.select(SelectedProducerOutcome {
                authored_by: StreamTerminalAuthor::Host,
                origin: StreamTerminalOrigin::Control,
                terminal: StreamTerminal::Failed(match &result {
                    Ok(()) => StreamError::Abandoned,
                    Err(details) => StreamError::Trapped(details.clone()),
                }),
            });
            producer.close_items();
            producer.settled.cancel();
        }
    }

    /// End the body without waiting for future local-reader demand.
    pub(crate) fn release(&mut self) {
        self.released
            .retain(|producer| producer.strong_count() != 0);
        self.released
            .extend(self.producers.drain(..).map(|producer| {
                assert!(
                    producer.settled.is_cancelled(),
                    "schema producer body has not settled"
                );
                Arc::downgrade(&producer)
            }));
    }

    #[cfg(test)]
    async fn finish_publications(&mut self) -> Result<(), String> {
        while let Some(producer) = self.producers.pop() {
            let publication = producer.publication.lock().unwrap().take();
            if let Some(mut publication) = publication {
                publication
                    .tasks
                    .join_next()
                    .await
                    .expect("schema publication task")
                    .map_err(|error| format!("schema stream publication task failed: {error}"))?
                    .map_err(|error| format!("schema stream publication failed: {error:?}"))?;
            }
        }
        Ok(())
    }
}

impl ProducerOutcome {
    pub(crate) fn bind_publication(
        &self,
        publisher: LiveStreamPublisher<SchemaValue>,
        lifecycle: Arc<SourceLifecycle>,
    ) {
        let mut publication = self.publication.lock().unwrap();
        assert!(publication.is_none(), "schema producer is already bound");
        let (sender, mut commands) = mpsc::unbounded_channel();
        let mut selected = self.selected.subscribe();
        let task_lifecycle = lifecycle.clone();
        let task_publisher = publisher.clone();
        let mut tasks = self.tasks.children();
        tasks.spawn(async move {
            let publish = async {
                while let Some(command) = commands.recv().await {
                    match command {
                        ProducerPublicationCommand::Item(publish, receipt) => {
                            let result = publish.await;
                            let _ = receipt.send(result);
                            match result {
                                Ok(_) => {}
                                Err(LiveStreamPublishError::Closed) => return Ok(()),
                                Err(error) => return Err(error),
                            }
                        }
                        ProducerPublicationCommand::Close => {
                            let outcome = loop {
                                if let Some(outcome) = selected.borrow_and_update().clone() {
                                    break outcome;
                                }
                                if selected.changed().await.is_err() {
                                    return Ok(());
                                }
                            };
                            match task_publisher
                                .publish_terminal_with_origin(
                                    outcome.authored_by,
                                    outcome.origin,
                                    outcome.terminal,
                                )
                                .await
                            {
                                Ok(_) | Err(LiveStreamPublishError::Closed) => return Ok(()),
                                Err(error) => return Err(error),
                            }
                        }
                    }
                }
                Ok(())
            };
            let result = tokio::select! {
                biased;
                _ = task_lifecycle.cancelled() => Ok(()),
                result = publish => result,
            };
            task_lifecycle.finish();
            result
        });
        *publication = Some(ProducerPublication {
            publisher,
            sender,
            closed: false,
            tasks,
            lifecycle,
        });
    }

    /// Ownership transfers before returning the receipt, independently of Store polling.
    /// The single bound consumer waits for each receipt before submitting another item.
    pub(crate) fn publish_item(
        &self,
        value: SchemaValue,
    ) -> oneshot::Receiver<Result<u64, LiveStreamPublishError>> {
        let (receipt, completion) = oneshot::channel();
        let publication = self.publication.lock().unwrap();
        let publication = publication.as_ref().expect("bound schema stream publisher");
        if !publication.closed {
            let publisher = publication.publisher.clone();
            let mut publish: Pin<
                Box<dyn Future<Output = Result<u64, LiveStreamPublishError>> + Send>,
            > = Box::pin(async move { publisher.publish_item(value).await });
            // Keep ready writes synchronous. A blocked write transfers to the native
            // owner before returning, so losing the Store cannot lose an accepted item.
            let publish = match publish
                .as_mut()
                .poll(&mut Context::from_waker(futures::task::noop_waker_ref()))
            {
                Poll::Ready(Ok(offset)) => {
                    let _ = receipt.send(Ok(offset));
                    return completion;
                }
                Poll::Ready(Err(error)) => Box::pin(std::future::ready(Err(error))),
                Poll::Pending => publish,
            };
            let _ = publication
                .sender
                .send(ProducerPublicationCommand::Item(publish, receipt));
        }
        completion
    }

    pub(crate) fn close_items(&self) {
        if let Some(publication) = self.publication.lock().unwrap().as_mut()
            && !publication.closed
        {
            publication.closed = true;
            let _ = publication.sender.send(ProducerPublicationCommand::Close);
        }
    }

    fn select(&self, outcome: SelectedProducerOutcome) {
        self.selected.send_if_modified(|selected| {
            if selected.is_some() {
                return false;
            }
            *selected = Some(outcome);
            true
        });
    }

    #[cfg(test)]
    async fn outcome(&self) -> SelectedProducerOutcome {
        let mut selected = self.selected.subscribe();
        loop {
            if let Some(outcome) = selected.borrow_and_update().clone() {
                return outcome;
            }
            // The sender is owned by self and cannot disappear during this borrow.
            selected.changed().await.expect("producer outcome sender");
        }
    }

    #[cfg(test)]
    fn settled(&self) -> CancellationToken {
        self.settled.clone()
    }

    pub(crate) fn consumer(self: &Arc<Self>) -> ProducerOutcomeConsumer {
        ProducerOutcomeConsumer(self.clone())
    }
}

pub(crate) struct ProducerOutcomeConsumer(Arc<ProducerOutcome>);

impl<D: Send + 'static> FutureConsumer<D> for ProducerOutcomeConsumer {
    type Item = Option<wire::StreamOutcome>;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<D>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        if finish {
            return Poll::Ready(Ok(()));
        }
        let mut value = None;
        source.read(&mut store, &mut value)?;
        if let Some(Some(value)) = value {
            let terminal = match value {
                Ok(()) => StreamTerminal::Finished,
                Err(wire::StreamError::Failed(value)) => StreamTerminal::Failed(
                    StreamError::Failed(Box::new(decode_typed(&value).map_err(|error| {
                        wasmtime::Error::from(wasmtime::Trap::UnreachableCodeReached)
                            .context(format!("invalid producer stream error: {error}"))
                    })?)),
                ),
                Err(_) => {
                    return Poll::Ready(Err(wasmtime::Error::from(
                        wasmtime::Trap::UnreachableCodeReached,
                    )
                    .context("producer supplied a host-only stream outcome")));
                }
            };
            self.0.select(SelectedProducerOutcome {
                authored_by: StreamTerminalAuthor::Guest,
                origin: StreamTerminalOrigin::Source,
                terminal,
            });
        }
        // None is absence, not success or abandonment. The scope owns settlement.
        Poll::Ready(Ok(()))
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn settle_schema_producers(&mut self, result: Result<(), String>) {
        self.schema_producers.settle(result);
        self.schema_producers.release();
    }

    pub(crate) fn schema_output_spawner(
        &self,
    ) -> Result<NativeReconstructionSpawner, WorkerExecutorError> {
        self.schema_output_tasks
            .lock()
            .unwrap()
            .as_ref()
            .map(|tasks| tasks.spawner())
            .ok_or_else(|| WorkerExecutorError::runtime("schema output task owner is closed"))
    }

    pub(crate) fn schema_output_drain_monitor(
        &self,
    ) -> Result<NativeReconstructionMonitor, WorkerExecutorError> {
        self.schema_output_tasks
            .lock()
            .unwrap()
            .as_ref()
            .map(|tasks| tasks.monitor())
            .ok_or_else(|| WorkerExecutorError::runtime("schema output task owner is closed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::{test, timeout};
    use wasmtime::AsContextMut;
    use wasmtime::component::FutureReader;

    #[test]
    #[timeout("30s")]
    async fn released_local_stream_retains_its_body_outcome_without_vetoing_suspension() {
        use crate::durable_host::stream_bus::LiveStreamEventPayload;
        use crate::durable_host::stream_transport::{
            LiveStreamEndpoint, accounted_output_stream_pair,
        };
        use crate::worker::suspension::OwnerSuspension;
        use crate::worker::suspension::tests::{blocked_timer, eligible_now};

        for activate_first in [false, true] {
            let owner = OwnerSuspension::new();
            let (runtime, _, _timer) = blocked_timer(
                &owner,
                std::time::Instant::now() + std::time::Duration::from_secs(60),
            );
            let tasks = WorkerTasks::default();
            let mut scope = ProducerOutcomeScope::new(tasks.clone());
            let producer = scope.register();
            let weak = Arc::downgrade(&producer);
            let (consumer, stream) = accounted_output_stream_pair(
                1,
                Arc::new(|| false),
                Some(runtime.source()),
                Box::pin(std::future::pending()),
                Some(producer.clone()),
            )
            .unwrap();
            let mut endpoint = Some(stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap());
            let mut source = activate_first.then(|| endpoint.take().unwrap().activate());
            let first = producer.publish_item(SchemaValue::U8(7));
            let pending = if activate_first {
                first.await.unwrap().unwrap();
                producer.publish_item(SchemaValue::U8(19))
            } else {
                first
            };
            scope.settle(Ok(()));
            scope.release();
            drop(consumer);
            drop(producer);
            // A later body must neither wait for nor replace this local stream's outcome.
            scope.register();
            scope.settle(Err("later body trap".into()));
            scope.release();
            assert!(eligible_now(&owner), "native delivery is not durable work");
            assert!(weak.upgrade().is_some(), "the reader retains the producer");
            let mut source = source
                .take()
                .unwrap_or_else(|| endpoint.take().unwrap().activate());
            let event = source.recv().await.unwrap();
            assert_eq!(
                event.payload,
                LiveStreamEventPayload::Item(SchemaValue::U8(7))
            );
            if activate_first {
                let event = source.recv().await.unwrap();
                assert_eq!(
                    event.payload,
                    LiveStreamEventPayload::Item(SchemaValue::U8(19))
                );
            }
            pending.await.unwrap().unwrap();
            let event = source.recv().await.unwrap();
            assert_eq!(event.terminal_origin, Some(StreamTerminalOrigin::Control));
            assert_eq!(
                event.payload,
                LiveStreamEventPayload::Terminal {
                    authored_by: StreamTerminalAuthor::Host,
                    terminal: StreamTerminal::Failed(StreamError::Abandoned),
                }
            );
            drop(source);
            tasks.stop_roots_and_wait().await;
            assert!(
                weak.upgrade().is_none(),
                "reader ownership must not form a cycle"
            );
        }
    }

    #[test]
    #[timeout("30s")]
    async fn runtime_loss_revokes_released_local_publication_without_replacing_its_outcome() {
        use crate::durable_host::stream_bus::LiveStreamEventPayload;
        use crate::durable_host::stream_transport::{
            LiveStreamEndpoint, accounted_output_stream_pair,
        };

        let tasks = WorkerTasks::default();
        let mut scope = ProducerOutcomeScope::new(tasks.clone());
        let producer = scope.register();
        let (consumer, stream) = accounted_output_stream_pair(
            1,
            Arc::new(|| false),
            None,
            Box::pin(std::future::pending()),
            Some(producer.clone()),
        )
        .unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let lifecycle = endpoint.lifecycle();
        let mut source = endpoint.activate();
        producer
            .publish_item(SchemaValue::U8(13))
            .await
            .unwrap()
            .unwrap();
        let pending = producer.publish_item(SchemaValue::U8(31));
        scope.settle(Ok(()));
        scope.release();
        let selected = producer.outcome().await;
        drop(scope);
        tasks.stop_roots_and_wait().await;
        assert!(pending.await.is_err());
        assert!(lifecycle.is_aborted());
        assert_eq!(producer.outcome().await, selected);
        assert_eq!(
            source.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::U8(13))
        );
        assert!(
            futures::poll!(Box::pin(source.recv())).is_pending(),
            "runtime loss must not publish a terminal"
        );
        drop(consumer);
    }

    #[test]
    #[timeout("30s")]
    async fn publication_scope_loss_and_cancelled_join_abort_without_selecting_fallback() {
        use crate::durable_host::stream_transport::{
            LiveStreamEndpoint, accounted_output_stream_pair,
        };

        for cancel_join in [false, true] {
            let tasks = WorkerTasks::default();
            let mut scope = ProducerOutcomeScope::new(tasks.clone());
            let producer = scope.register();
            let (consumer, stream) = accounted_output_stream_pair(
                1,
                Arc::new(|| false),
                None,
                Box::pin(std::future::pending()),
                Some(producer.clone()),
            )
            .unwrap();
            let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
            let lifecycle = endpoint.lifecycle();
            let _source = endpoint.activate();
            producer
                .publish_item(SchemaValue::U8(13))
                .await
                .unwrap()
                .unwrap();
            let pending = producer.publish_item(SchemaValue::U8(31));
            if cancel_join {
                let mut finished = Box::pin(scope.finish_publications());
                assert!(futures::poll!(&mut finished).is_pending());
                drop(finished);
            } else {
                drop(scope);
            }
            tasks.stop_roots_and_wait().await;
            assert!(pending.await.is_err());
            assert!(lifecycle.is_aborted());
            assert!(futures::poll!(Box::pin(producer.outcome())).is_pending());
            assert!(!producer.settled().is_cancelled());
            drop(consumer);
        }
    }

    #[test]
    #[timeout("30s")]
    async fn closed_items_wait_for_outcome_and_reject_further_admission() {
        use crate::durable_host::stream_bus::LiveStreamEventPayload;
        use crate::durable_host::stream_transport::{
            LiveStreamEndpoint, accounted_output_stream_pair,
        };

        let mut scope = ProducerOutcomeScope::default();
        let producer = scope.register();
        let (_consumer, stream) = accounted_output_stream_pair(
            1,
            Arc::new(|| false),
            None,
            Box::pin(std::future::pending()),
            Some(producer.clone()),
        )
        .unwrap();
        let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let mut source = endpoint.activate();
        producer.close_items();
        assert!(producer.publish_item(SchemaValue::U8(43)).await.is_err());
        assert!(futures::poll!(Box::pin(source.recv())).is_pending());
        scope.settle(Ok(()));
        let event = source.recv().await.unwrap();
        assert_eq!(event.offset, 0);
        assert_eq!(event.terminal_origin, Some(StreamTerminalOrigin::Control));
        assert_eq!(
            event.payload,
            LiveStreamEventPayload::Terminal {
                authored_by: StreamTerminalAuthor::Host,
                terminal: StreamTerminal::Failed(StreamError::Abandoned),
            }
        );
        scope.finish_publications().await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn accepted_items_and_explicit_outcomes_survive_lost_store_receipts_and_later_trap() {
        use crate::durable_host::stream_bus::LiveStreamEventPayload;
        use crate::durable_host::stream_transport::{
            LiveStreamEndpoint, accounted_output_stream_pair,
        };
        use golem_schema::schema::{SchemaGraph, SchemaType, TypedSchemaValue};

        let failure = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String("declared failure".into()),
        );
        for explicit in 0..3 {
            let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
            let engine = wasmtime::Engine::new(&config).unwrap();
            let mut store = wasmtime::Store::new(&engine, ());
            let mut scope = ProducerOutcomeScope::default();
            let producer = scope.register();
            let value = match explicit {
                0 => None,
                1 => Some(Ok(())),
                _ => Some(Err(wire::StreamError::Failed(
                    golem_schema::schema::wit::encode_typed(&failure).unwrap(),
                ))),
            };
            FutureReader::new(&mut store, async move { wasmtime::error::Ok(value) })
                .unwrap()
                .pipe(&mut store, producer.consumer())
                .unwrap();
            store
                .as_context_mut()
                .run_concurrent_and_settle(async |_| (), &mut |_| true)
                .await
                .unwrap();
            let (consumer, stream) = accounted_output_stream_pair(
                1,
                Arc::new(|| false),
                None,
                Box::pin(std::future::pending()),
                Some(producer.clone()),
            )
            .unwrap();
            let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
            let mut source = endpoint.activate();
            producer
                .publish_item(SchemaValue::U8(11))
                .await
                .unwrap()
                .unwrap();
            // Capacity is exhausted. The native lane must retain this accepted item even
            // when the Store-side observer and the source consumer disappear.
            drop(producer.publish_item(SchemaValue::U8(29)));
            drop(producer.publish_item(SchemaValue::U8(47)));
            drop(consumer);
            drop(store);
            scope.settle(Err("later trap".into()));
            let mut finished = Box::pin(scope.finish_publications());
            assert!(futures::poll!(&mut finished).is_pending());
            for (offset, value) in [(0, 11), (1, 29), (2, 47)] {
                let event = source.recv().await.unwrap();
                assert_eq!(event.offset, offset);
                assert_eq!(
                    event.payload,
                    LiveStreamEventPayload::Item(SchemaValue::U8(value))
                );
            }
            let event = source.recv().await.unwrap();
            assert_eq!(event.offset, 3);
            assert_eq!(
                event.payload,
                LiveStreamEventPayload::Terminal {
                    authored_by: if explicit == 0 {
                        StreamTerminalAuthor::Host
                    } else {
                        StreamTerminalAuthor::Guest
                    },
                    terminal: match explicit {
                        0 => StreamTerminal::Failed(StreamError::Trapped("later trap".into())),
                        1 => StreamTerminal::Finished,
                        _ => StreamTerminal::Failed(StreamError::Failed(Box::new(failure.clone()))),
                    },
                }
            );
            assert_eq!(
                event.terminal_origin,
                Some(if explicit == 0 {
                    StreamTerminalOrigin::Control
                } else {
                    StreamTerminalOrigin::Source
                })
            );
            finished.await.unwrap();
        }
    }

    #[test]
    #[timeout("30s")]
    async fn optional_producer_outcome_waits_for_settlement_and_preserves_explicit_outcomes() {
        use golem_schema::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
        let failure = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String("typed producer failure".into()),
        );
        for explicit in 0..3 {
            for trapped in [false, true] {
                let config =
                    golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
                let engine = wasmtime::Engine::new(&config).unwrap();
                let mut store = wasmtime::Store::new(&engine, ());
                let mut scope = ProducerOutcomeScope::default();
                let producer = scope.register();
                let value = match explicit {
                    0 => None,
                    1 => Some(Ok(())),
                    _ => Some(Err(wire::StreamError::Failed(
                        golem_schema::schema::wit::encode_typed(&failure).unwrap(),
                    ))),
                };
                let future =
                    FutureReader::new(&mut store, async move { wasmtime::error::Ok(value) })
                        .unwrap();
                future.pipe(&mut store, producer.consumer()).unwrap();
                store
                    .as_context_mut()
                    .run_concurrent_and_settle(async |_accessor| (), &mut |_| true)
                    .await
                    .unwrap();
                let expected = match explicit {
                    1 => StreamTerminal::Finished,
                    2 => StreamTerminal::Failed(StreamError::Failed(Box::new(failure.clone()))),
                    _ if trapped => {
                        StreamTerminal::Failed(StreamError::Trapped("post-root trap".into()))
                    }
                    _ => StreamTerminal::Failed(StreamError::Abandoned),
                };
                if explicit != 0 {
                    assert_eq!(producer.outcome().await.terminal, expected);
                } else {
                    assert!(futures::poll!(Box::pin(producer.outcome())).is_pending());
                }
                assert!(!producer.settled().is_cancelled());
                scope.settle(if trapped {
                    Err("post-root trap".into())
                } else {
                    Ok(())
                });
                let selected = producer.outcome().await;
                assert_eq!(selected.terminal, expected);
                assert_eq!(
                    selected.origin,
                    if explicit != 0 {
                        StreamTerminalOrigin::Source
                    } else {
                        StreamTerminalOrigin::Control
                    }
                );
            }
        }
    }

    #[test]
    #[timeout("30s")]
    async fn producer_scope_drop_and_sibling_failure_do_not_select_an_outcome() {
        let mut failed = ProducerOutcomeScope::default();
        let mut sibling = ProducerOutcomeScope::default();
        let failed_producer = failed.register();
        let sibling_producer = sibling.register();
        failed.settle(Err("isolated trap".into()));
        assert_eq!(
            failed_producer.outcome().await.terminal,
            StreamTerminal::Failed(StreamError::Trapped("isolated trap".into()))
        );
        drop(sibling);
        assert!(futures::poll!(Box::pin(sibling_producer.outcome())).is_pending());
        assert!(!sibling_producer.settled().is_cancelled());
    }

    #[test]
    #[timeout("30s")]
    async fn unresolved_producer_future_does_not_preempt_a_later_producer_trap() {
        let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
        let engine = wasmtime::Engine::new(&config).unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        let mut scope = ProducerOutcomeScope::default();
        let producer = scope.register();
        FutureReader::new(
            &mut store,
            std::future::pending::<wasmtime::Result<Option<wire::StreamOutcome>>>(),
        )
        .unwrap()
        .pipe(&mut store, producer.consumer())
        .unwrap();
        store
            .as_context_mut()
            .run_concurrent_and_settle(async |_| (), &mut |_| true)
            .await
            .unwrap();
        assert!(futures::poll!(Box::pin(producer.outcome())).is_pending());
        scope.settle(Err("trap with unresolved outcome".into()));
        assert_eq!(
            producer.outcome().await.terminal,
            StreamTerminal::Failed(StreamError::Trapped("trap with unresolved outcome".into()))
        );
    }

    #[test]
    #[timeout("30s")]
    async fn host_only_producer_outcomes_trap_instead_of_selecting_a_terminal() {
        for error in [
            wire::StreamError::Cancelled,
            wire::StreamError::Abandoned,
            wire::StreamError::Trapped("forged trap".into()),
            wire::StreamError::Interrupted("forged interruption".into()),
            wire::StreamError::ResourceExhausted("forged quota failure".into()),
        ] {
            let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
            let engine = wasmtime::Engine::new(&config).unwrap();
            let mut store = wasmtime::Store::new(&engine, ());
            let mut scope = ProducerOutcomeScope::default();
            let producer = scope.register();
            FutureReader::new(
                &mut store,
                async move { wasmtime::error::Ok(Some(Err(error))) },
            )
            .unwrap()
            .pipe(&mut store, producer.consumer())
            .unwrap();
            let error = store
                .as_context_mut()
                .run_concurrent_and_settle(async |_| (), &mut |_| true)
                .await
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<wasmtime::Trap>(),
                Some(&wasmtime::Trap::UnreachableCodeReached)
            );
            assert!(futures::poll!(Box::pin(producer.outcome())).is_pending());
        }
    }
}
