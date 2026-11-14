// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use crate::durable_host::stream_outcome::encode_outcome;
use wasmtime::component::{FutureProducer, FutureReader, TerminalConsumption};

struct InputCompletion {
    authored_by: StreamTerminalAuthor,
    terminal: StreamTerminal,
    source_offset: StreamOffset,
    ordinal: u64,
}

pub(super) struct InputCompletionSender {
    sender: oneshot::Sender<InputCompletion>,
    pending: Option<InputCompletion>,
}

impl InputCompletionSender {
    pub(super) fn terminal(
        &mut self,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
        source_offset: StreamOffset,
        ordinal: u64,
    ) {
        self.pending = Some(InputCompletion {
            authored_by,
            terminal,
            source_offset,
            ordinal,
        });
    }

    pub(super) fn deliver(self) {
        if let Some(completion) = self.pending {
            let _ = self.sender.send(completion);
        }
    }
}

struct InputOutcomeProducer {
    completion: Option<oneshot::Receiver<InputCompletion>>,
    streams: StreamSession,
    reader_id: DurableReaderIdentity,
    delivery: Option<oneshot::Receiver<Result<StreamReadObserver, WorkerExecutorError>>>,
    outcome: Option<wire::StreamOutcome>,
    observer: Arc<std::sync::Mutex<Option<StreamReadObserver>>>,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl DurableInputProducer {
    /// Pairs this reader with a completion that is gated by actual EOF consumption.
    pub fn with_completion<Ctx: WorkerCtx>(
        mut self,
        store: &mut StoreContextMut<'_, Ctx>,
    ) -> wasmtime::Result<(Self, FutureReader<wire::StreamOutcome>)> {
        let (sender, completion) = oneshot::channel();
        let observer: Arc<std::sync::Mutex<Option<StreamReadObserver>>> = Default::default();
        let mut future = FutureReader::new(
            &mut *store,
            InputOutcomeProducer {
                completion: Some(completion),
                streams: self.input.streams.clone(),
                reader_id: self.input.reader_id.clone(),
                delivery: None,
                outcome: None,
                observer: observer.clone(),
                runtime_teardown: self.runtime_teardown.clone(),
            },
        )?;
        future.register_terminal_observer(&mut *store, move |consumption| {
            if let Some(observer) = observer.lock().expect("input outcome observer lock").take() {
                let _ = observer(if consumption == TerminalConsumption::Delivered {
                    consumption
                } else {
                    TerminalConsumption::NotDelivered
                });
            }
        })?;
        self.completion = Some(InputCompletionSender {
            sender,
            pending: None,
        });
        Ok((self, future))
    }
}

impl StreamSession {
    async fn completion_observation(
        &self,
        mut record: StreamConsumerTerminalRecord,
    ) -> Result<PendingConsumerObservation, SessionError> {
        let positions = self
            .current_control_metadata()
            .await?
            .consumer_outcome_positions(record.reader_id)
            .to_vec();
        let mut recorded: Option<ConsumerObservations> = None;
        for index in positions {
            let StreamSessionRecord::ConsumerOutcome(original) =
                self.session_record_at(index).await?
            else {
                continue;
            };
            if original.consumer_read_ordinal != record.consumer_read_ordinal {
                continue;
            }
            let observation = StreamSessionRecord::ConsumerOutcome(record.clone());
            let source = StreamSessionRecord::ConsumerOutcome(original.clone());
            let original_index = original.reobserves.unwrap_or(index);
            let logical = if original.reobserves.is_some() {
                self.session_record_at(original_index).await?
            } else {
                source.clone()
            };
            if (original.reobserves.is_some() && original_index >= index)
                || !source.matches_reobserved(&logical)
                || !observation.matches_reobserved(&logical)
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "unchanged stream outcome",
                    format!("outcome differs from observation at {index}"),
                )
                .into());
            }
            let boundaries = recorded.get_or_insert_with(|| ConsumerObservations {
                original_index,
                boundaries: Vec::new(),
            });
            if boundaries.original_index != original_index {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "one logical stream outcome",
                    format!("conflicting outcome at {index}"),
                )
                .into());
            }
            boundaries.boundaries.push((index, original.observation));
        }
        record.reobserves = None;
        Ok(PendingConsumerObservation {
            record: StreamSessionRecord::ConsumerOutcome(record),
            recorded,
            introduced: None,
        })
    }
}

impl<Ctx: WorkerCtx> FutureProducer<Ctx> for InputOutcomeProducer {
    type Item = wire::StreamOutcome;

    fn poll_produce(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<Ctx>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        if finish {
            return Poll::Ready(Ok(None));
        }
        if self.delivery.is_none() {
            let Some(receiver) = self.completion.as_mut() else {
                return Poll::Pending;
            };
            let completion = match Pin::new(receiver).poll(cx) {
                Poll::Pending => return Poll::Pending,
                // Dropping items early is a consumer cancel, not a producer outcome.
                Poll::Ready(Err(_)) => {
                    self.completion = None;
                    return Poll::Pending;
                }
                Poll::Ready(Ok(completion)) => completion,
            };
            self.completion = None;
            self.outcome = Some(encode_outcome(&completion.terminal)?);
            let ctx = store.data().durable_ctx();
            let (result, delivery) = oneshot::channel();
            let reader_id = self.reader_id.clone();
            let streams = self.streams.clone();
            let observation = StreamConsumerObservation::Guest {
                parent_start_index: ctx.entity_parent_start_index(),
            };
            let replay = ctx.state.replay_state.clone();
            let guest_order = ctx.guest_read_order();
            let runtime = ctx.runtime_suspension.clone();
            let activity = ctx.tail_work_tracker().activity();
            let runtime_teardown = self.runtime_teardown.clone();
            let cleanup_sink = ctx
                .state
                .dropped_call_event_sender()
                .expect("worker cleanup queue");
            let trap_context = DurableCallTrapContext {
                retry_from: InFunctionRetryHost::current_retry_point(ctx),
            };
            let live_call_permit = LiveCallPermit::new(ctx.state.live_host_call_counter());
            store.as_context_mut().spawn(async move |accessor| {
                let record = StreamConsumerTerminalRecord {
                    format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                    observation,
                    reobserves: None,
                    session_key: streams.session_reference.clone(),
                    reader_id: reader_id.await.map_err(wasmtime::Error::msg)?,
                    source_offset: completion.source_offset,
                    consumer_read_ordinal: completion.ordinal,
                    authored_by: completion.authored_by,
                    terminal: completion.terminal,
                };
                let observation = streams.completion_observation(record).await?;
                PrepareInputObservation {
                    observation,
                    streams,
                    replay,
                    guest_order,
                    runtime,
                    activity,
                    result,
                    runtime_teardown,
                    cleanup_sink,
                    trap_context,
                    live_call_permit,
                }
                .run(accessor)
                .await
            })?;
            self.delivery = Some(delivery);
        }
        let observer = std::task::ready!(Pin::new(self.delivery.as_mut().unwrap()).poll(cx))
            .map_err(|_| wasmtime::Error::msg("stream outcome preparation stopped"))??;
        *self.observer.lock().expect("input outcome observer lock") = Some(observer);
        Poll::Ready(Ok(self.outcome.take()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::backpressured_session_input;
    use super::*;
    use test_r::{test, timeout};

    #[test]
    #[timeout("30s")]
    async fn input_completion_waits_for_eof_consumption_and_retains_the_terminal() {
        for terminal in [
            StreamTerminal::Finished,
            StreamTerminal::Failed(StreamError::Trapped("producer failed after item 1".into())),
            StreamTerminal::Failed(StreamError::Cancelled),
            StreamTerminal::Failed(StreamError::Abandoned),
            StreamTerminal::Failed(StreamError::Interrupted("source lost".into())),
            StreamTerminal::Failed(StreamError::ResourceExhausted("retention limit".into())),
            StreamTerminal::Failed(StreamError::Failed(Box::new(TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::u64()),
                SchemaValue::U64(917),
            )))),
        ] {
            for delivered in [false, true] {
                let (producer, streams, reader, handle) = backpressured_session_input().await;
                drop(reader);
                producer
                    .end(
                        None,
                        handle.stream_id,
                        1,
                        StreamTerminalAuthor::Host,
                        terminal.clone(),
                    )
                    .await
                    .unwrap();
                let endpoint = streams
                    .endpoint(handle, 0, SessionStreamRole::Input)
                    .await
                    .unwrap();
                let mut input = DurableInputProducer::new(endpoint);
                let (sender, mut completion) = oneshot::channel();
                input.completion = Some(InputCompletionSender {
                    sender,
                    pending: None,
                });
                assert!(matches!(
                    input.receive_value(None).await.unwrap(),
                    Some(DurableInputEvent::Item(SchemaValue::U8(1)))
                ));
                assert!(matches!(
                    completion.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                assert!(matches!(
                    input.receive_value(None).await.unwrap(),
                    Some(DurableInputEvent::Terminal { .. })
                ));
                assert!(
                    matches!(
                        completion.try_recv(),
                        Err(oneshot::error::TryRecvError::Empty)
                    ),
                    "preparing EOF must not release the outcome"
                );
                let observed = input.track_guest_consumption(Box::new(|_| Ok(())));
                observed(if delivered {
                    TerminalConsumption::Delivered
                } else {
                    TerminalConsumption::NotDelivered
                })
                .unwrap();
                if delivered {
                    let completion = completion.await.unwrap();
                    assert_eq!(completion.authored_by, StreamTerminalAuthor::Host);
                    assert_eq!(completion.terminal, terminal);
                    assert_eq!(completion.ordinal, 1);
                } else {
                    assert!(
                        completion.await.is_err(),
                        "a cancelled EOF read exposes no producer outcome"
                    );
                }
            }
        }
    }

    #[test]
    #[timeout("30s")]
    async fn input_completion_recovers_its_own_reobservation_boundaries() {
        let (producer, streams, reader, handle) = backpressured_session_input().await;
        drop(reader);
        let terminal = producer
            .end(
                None,
                handle.stream_id,
                1,
                StreamTerminalAuthor::Host,
                StreamTerminal::Failed(StreamError::Trapped("original failure".into())),
            )
            .await
            .unwrap()
            .value;
        let endpoint = streams
            .endpoint(handle, 0, SessionStreamRole::Input)
            .await
            .unwrap();
        let reader_id = endpoint.reader_id.clone().await.unwrap();
        let mut input = DurableInputProducer::new(endpoint);
        input.receive_value(None).await.unwrap();
        input.receive_value(None).await.unwrap();
        let record = StreamConsumerTerminalRecord {
            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
            observation: StreamConsumerObservation::Guest {
                parent_start_index: None,
            },
            reobserves: None,
            session_key: streams.session_reference.clone(),
            reader_id,
            source_offset: terminal,
            consumer_read_ordinal: 1,
            authored_by: StreamTerminalAuthor::Host,
            terminal: StreamTerminal::Failed(StreamError::Trapped("original failure".into())),
        };
        assert!(
            streams
                .completion_observation(record.clone())
                .await
                .unwrap()
                .recorded
                .is_none()
        );
        producer
            .append_session_record(None, StreamSessionRecord::ConsumerOutcome(record.clone()))
            .await
            .unwrap();
        let original = streams.oplog.current_oplog_index().await;
        let mut repeated = record.clone();
        repeated.reobserves = Some(original);
        repeated.observation = StreamConsumerObservation::Guest {
            parent_start_index: Some(OplogIndex::from_u64(99)),
        };
        producer
            .append_session_record(None, StreamSessionRecord::ConsumerOutcome(repeated.clone()))
            .await
            .unwrap();
        let repeated_index = streams.oplog.current_oplog_index().await;
        let mut requested = record.clone();
        requested.observation = repeated.observation.clone();
        let observed = streams
            .completion_observation(requested)
            .await
            .unwrap()
            .recorded
            .unwrap();
        assert_eq!(observed.original_index, original);
        assert_eq!(
            observed.boundaries,
            vec![
                (original, record.observation.clone()),
                (repeated_index, repeated.observation)
            ]
        );
        let mut changed = record;
        changed.terminal = StreamTerminal::Finished;
        assert!(streams.completion_observation(changed).await.is_err());
    }
}
