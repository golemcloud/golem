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

use super::*;

impl std::fmt::Debug for DroppedDurableInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DroppedDurableInput")
            .field("transport_stream_id", &self.transport_stream_id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl DroppedDurableInput {
    /// A replayed drop must not wait for guest progress, including when the original guest
    /// retained the endpoint forever. The context owns this task until replay settles or the
    /// resident runtime is discarded, just as it owns tool producer disposal.
    pub(crate) fn defer_if_replaying<Ctx: WorkerCtx>(
        &self,
        ctx: &DurableWorkerCtx<Ctx>,
    ) -> Result<bool, WorkerExecutorError> {
        if ctx.is_live() {
            return Ok(false);
        }
        let Some(tasks) = &ctx.producer_disposal_tasks else {
            // Failure cleanup already took and aborted the task group. Resource destruction
            // must not restart source cancellation while the Store is being disposed.
            // True means the caller must skip inline cancellation, not that it was recorded.
            return Ok(true);
        };
        let cancellation = self.clone();
        let replay = ctx.state.replay_state.clone();
        let operation = ctx.entity_tool_operation();
        let boundary = self.disposal_boundary()?;
        let rejected = ctx.rejects_live_continuation_at_replay_tail();
        let transition = ctx.live_continuation_at_replay_tail_factory(
            true,
            "durable stream guest-drop cancellation".to_string(),
        );
        let cleanup = ctx
            .state
            .dropped_call_event_sender()
            .expect("worker cleanup queue");
        let trap_context = DurableCallTrapContext {
            retry_from: InFunctionRetryHost::current_retry_point(ctx),
        };
        let interrupt = ctx.create_interrupt_signal();
        tasks.spawner().spawn(async move {
            let cancel = async {
                if let Some(index) = cancellation.unreproduced_guest_observation(&replay).await? {
                    return Err(replay.fail_stream_observation(index));
                }
                loop {
                    if cancellation
                        .replay_disposal_is_complete(operation.as_ref(), &boundary, rejected)
                        .await?
                    {
                        return Ok(());
                    }
                    replay.await_natural_tail_end(None).await?;
                    if cancellation
                        .replay_disposal_is_complete(operation.as_ref(), &boundary, rejected)
                        .await?
                    {
                        return Ok(());
                    }
                    match transition().await? {
                        BeginReplayToLive::ReplayResumed => continue,
                        BeginReplayToLive::Pending(pending) => {
                            pending
                                .finish_at(Some(boundary.clone()))
                                .await?
                                .require_live()?;
                            break;
                        }
                    }
                }
                // The Store applies replay events before forwarding the cancellation. The
                // native task owns only the passive wait, never Store-local replay state.
                cleanup
                    .send(DropEvent::CancelDroppedDurableInput {
                        cancellation: Box::new(cancellation),
                    })
                    .map_err(|_| WorkerExecutorError::runtime("durable input cleanup queue closed"))
            };
            let result = tokio::select! {
                result = cancel => result,
                kind = interrupt => return Err(WorkerExecutorError::Interrupted { kind }),
            };
            if let Err(error) = result {
                cleanup
                    .send(DropEvent::DurableInputDisposalFailed {
                        error,
                        trap_context,
                    })
                    .map_err(|_| {
                        WorkerExecutorError::runtime("durable input cleanup queue closed")
                    })?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    pub(super) async fn unreproduced_guest_observation(
        &self,
        replay: &ReplayState,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        let inspect = async {
            let binding = self
                .streams
                .binding(self.transport_stream_id)
                .ok_or_else(|| {
                    format!("unknown durable input binding {}", self.transport_stream_id)
                })?;
            let reader = self
                .streams
                .current_control_metadata()
                .await?
                .reader_id(&binding)?;
            for index in self.streams.consumer_history_positions(reader).await? {
                if index > replay.replay_target() || replay.is_in_skipped_region(index).await? {
                    continue;
                }
                let record = self.streams.session_record_at(index).await?;
                let missing = match record {
                    StreamSessionRecord::ConsumerItemValue(record) => {
                        matches!(record.observation, StreamConsumerObservation::Guest { .. })
                            && record.consumer_read_ordinal + record.logical_item_count() as u64
                                > self.consumer_read_ordinal
                    }
                    StreamSessionRecord::ConsumerTerminal(record) => {
                        matches!(record.observation, StreamConsumerObservation::Guest { .. })
                            && record.consumer_read_ordinal >= self.consumer_read_ordinal
                    }
                    _ => false,
                };
                if missing {
                    return Ok::<_, SessionError>(Some(index));
                }
            }
            Ok(None)
        };
        inspect
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))
    }

    pub(super) fn disposal_boundary(
        &self,
    ) -> Result<SerializableToolAdmissionRefusal, WorkerExecutorError> {
        let binding = self
            .streams
            .binding(self.transport_stream_id)
            .ok_or_else(|| {
                WorkerExecutorError::runtime(format!(
                    "unknown durable input binding {}",
                    self.transport_stream_id
                ))
            })?;
        Ok(
            SerializableToolAdmissionRefusal::BeforeDurableInputDisposal {
                session: self.streams.session_reference.clone(),
                binding: Box::new(binding),
                role: self.role,
            },
        )
    }

    pub(super) async fn replay_disposal_is_complete(
        &self,
        operation: Option<&crate::durable_host::tool::operation::OwnerToolOperation>,
        boundary: &SerializableToolAdmissionRefusal,
        rejected: bool,
    ) -> Result<bool, WorkerExecutorError> {
        let recorded_refusal = operation.is_some_and(|operation| {
            operation
                .expected_admission_refusal()
                .is_some_and(|(expected, _)| expected == *boundary)
        });
        // Owner finalization may have supplied a source terminal after this exact disposal
        // was refused. That terminal does not erase the recorded admission decision.
        if !recorded_refusal
            && self
                .is_recorded()
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        {
            return Ok(true);
        }
        if let Some(operation) = operation {
            operation.reject_missing_replay_admission(boundary).await?;
        }
        if rejected {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "durable stream guest-drop cancellation",
                "no recorded cancellation during completed entity replay",
            ));
        }
        Ok(false)
    }

    pub(crate) async fn is_recorded(&self) -> Result<bool, SessionError> {
        let binding = self
            .streams
            .binding(self.transport_stream_id)
            .ok_or_else(|| format!("unknown durable input binding {}", self.transport_stream_id))?;
        let mapping = self
            .streams
            .mapping(self.transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {}", self.transport_stream_id))?;
        self.streams.validate_recovered_mapping(&mapping).await?;
        if self
            .streams
            .current_control_metadata()
            .await?
            .cancellation_intent(&binding.source)
            .is_some()
        {
            return Ok(true);
        }
        // Session finalization writes source terminals, not consumer cancellation intents.
        // Inspect the exact validated source without consuming any guest observation. An
        // unrelated session's Finished record cannot establish that this source is closed.
        if self.streams.producer.owns_handle_identity(&mapping.handle) {
            return self
                .streams
                .producer
                .stream_head(&mapping.handle)
                .await
                .map(|head| head.closed)
                .map_err(SessionError::from);
        }
        self.streams.has_journaled_consumer_terminal(&mapping).await
    }

    /// Cancels the durable source of a readable stream end the guest dropped without draining.
    ///
    /// The cleanup is best effort: once the session's attachment has been fenced (the invocation
    /// finished or another attempt took over, which is also what a replaying guest observes when
    /// it drops the same reader again), the cancellation is no longer ours to author and is
    /// skipped instead of failing the worker.
    pub async fn cancel(&self) -> Result<(), SessionError> {
        match self
            .streams
            .cancel_stream(
                self.transport_stream_id,
                self.role,
                StreamCancelReason::GuestDrop,
                Some("guest dropped its durable readable stream end".to_string()),
                None,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if error.starts_with("StaleEpoch:") => {
                tracing::debug!(
                    transport_stream_id = self.transport_stream_id,
                    error = %error,
                    "Skipping cancellation of dropped durable stream"
                );
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    transport_stream_id = self.transport_stream_id,
                    role = ?self.role,
                    error = %error,
                    "Cancellation of dropped durable stream failed"
                );
                Err(error)
            }
        }
    }
}

impl DurableInputDropTask {
    async fn run<Ctx: WorkerCtx>(self, accessor: &Accessor<Ctx>) -> wasmtime::Result<()> {
        let result = cancel_dropped_durable_input_access(
            accessor,
            DurableWorkerCtxView::durable_ctx_mut,
            &self.cancellation,
        )
        .await
        .map_err(SessionError::from);
        let _ = self.result.send(result);
        Ok(())
    }
}

pub(super) fn durable_stream_cancel_error(
    role: StreamCancelRole,
    reason: StreamCancelReason,
    details: Option<String>,
) -> anyhow::Error {
    anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message: format!(
            "durable stream cancelled ({role:?}, {reason:?}): {}",
            details.unwrap_or_default()
        ),
    })
}

impl DurableInputProducer {
    /// Explicit guest resource drop also cancels endpoints that were never unwrapped.
    /// A transferred endpoint has already been taken from the affine stream cell.
    pub(crate) fn drop_unread(
        stream: SchemaValueStream,
        drop_event_sink: mpsc::UnboundedSender<DropEvent>,
        runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        if stream
            .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
            .is_ok()
            && let Ok(endpoint) = stream.take_host_endpoint::<DurableInputEndpoint>()
        {
            drop(Self::new(endpoint).with_drop_cleanup(drop_event_sink, runtime_teardown));
        }
    }

    /// Creates a producer for an already materialized session endpoint.
    pub fn new(endpoint: DurableInputEndpoint) -> Self {
        Self {
            guest_read_ordinal: Arc::new(AtomicU64::new(endpoint.consumer_read_ordinal)),
            input: endpoint,
            pending: None,
            pending_activity: None,
            live_admission: None,
            pending_drop: None,
            pending_guest_value: None,
            guest_delivery: None,
            pending_observation: None,
            finished: false,
            dropping: false,
            drop_event_sink: None,
            runtime_teardown: Arc::new(|| false),
            producer_cancel: None,
            pending_past_a_completed_record: false,
        }
    }

    /// Reports the reason of a cancel by the input's producer through `reason`. The reader still
    /// reads the end of the input; a reader whose ABI has a failure item can read the reason
    /// as one.
    pub(crate) fn with_producer_cancel_observer(
        mut self,
        reason: Arc<OnceLock<StreamCancelReason>>,
    ) -> Self {
        self.producer_cancel = Some(reason);
        self
    }

    /// Tells the producer when its Store is torn down, without cleanup on drop: a producer that
    /// is dropped unread then cancels nothing.
    pub(crate) fn with_runtime_teardown(
        mut self,
        runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        self.runtime_teardown = runtime_teardown;
        self
    }

    /// Lets a read past the recorded consumer journal stay pending in the replay of a completed
    /// entity body, where every other reader reports a divergence.
    ///
    /// A body may give up a read that never got anything. A record's position is reserved when
    /// the reader takes the item, and the body's result is recorded after that, so a read
    /// without a record got nothing in the first run. In the replay it waits until the body
    /// gives it up again; a request to finish is answered as in the first run.
    pub(crate) fn with_pending_reads_past_a_completed_record(mut self) -> Self {
        self.pending_past_a_completed_record = true;
        self
    }

    /// Installs cancellation cleanup to run when the guest drops unread input.
    pub(crate) fn with_drop_cleanup(
        mut self,
        drop_event_sink: mpsc::UnboundedSender<DropEvent>,
        runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        self.drop_event_sink = Some(drop_event_sink);
        self.runtime_teardown = runtime_teardown;
        self
    }

    #[cfg(test)]
    pub(super) fn begin_receive(&mut self) {
        self.pending = Some(self.input.receive(None, None));
    }

    pub(super) fn finish_receive(
        &mut self,
        result: Result<DurableInputRead, DurableReceiveError>,
    ) -> anyhow::Result<DurableInputEvent> {
        self.pending = None;
        self.pending_activity = None;
        let mut read = result.map_err(|error| match error {
            DurableReceiveError::Session(error) => error.into_trap(),
            DurableReceiveError::Interrupt(kind) => anyhow::Error::new(kind),
        })?;
        self.input.complete_receive(&mut read);
        self.pending_observation = read.observation;
        let event = read.event.ok_or_else(|| {
            anyhow::anyhow!("durable input stream source closed without a terminal event")
        })?;
        match event.payload {
            CommittedProducerStreamEventPayload::Value(bytes) => {
                let value = ProtoSchemaValue::decode(bytes.as_slice())
                    .map_err(|error| anyhow::anyhow!("invalid durable stream value: {error}"))?;
                decode_recursive_stream_value(value, |stream_id, _| {
                    read.endpoints
                        .remove(&stream_id)
                        .map(SchemaValueStream::from_host_endpoint)
                        .ok_or_else(|| format!("unknown nested stream reference {stream_id}"))
                })
                .map(DurableInputEvent::Item)
                .map_err(anyhow::Error::msg)
            }
            CommittedProducerStreamEventPayload::PackedU8(byte) => {
                Ok(DurableInputEvent::Item(SchemaValue::U8(byte)))
            }
            CommittedProducerStreamEventPayload::End(StreamEndResult::Ok) => {
                self.finished = true;
                Ok(DurableInputEvent::End)
            }
            CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(error)) => Err(
                anyhow::anyhow!("durable stream ended with error context: {error:?}"),
            ),
            CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputProducer | StreamCancelRole::OutputProducer,
                reason,
                ..
            } => {
                if let Some(observer) = &self.producer_cancel {
                    let _ = observer.set(reason);
                }
                self.finished = true;
                Ok(DurableInputEvent::End)
            }
            CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputConsumer | StreamCancelRole::OutputConsumer,
                ..
            } => {
                self.finished = true;
                Ok(DurableInputEvent::Cancelled)
            }
            CommittedProducerStreamEventPayload::Cancel {
                role: role @ StreamCancelRole::System,
                reason,
                details,
            } => Err(durable_stream_cancel_error(role, reason, details)),
        }
    }

    /// Receives a host-side value through the same consumer-journal path used by the guest ABI.
    pub(crate) async fn receive_value(
        &mut self,
        admission: Option<&mpsc::UnboundedSender<DurableInputReceiveAdmission>>,
    ) -> anyhow::Result<Option<DurableInputEvent>> {
        if self.finished {
            return Ok(Some(DurableInputEvent::End));
        }
        if self.pending.is_none() {
            let guard = if let Some(admission) = admission {
                let (result, response) = oneshot::channel();
                if admission
                    .send(DurableInputReceiveAdmission {
                        opens_source: self.input.opens_source(),
                        ordinal: self.input.consumer_read_ordinal,
                        boundary: self
                            .input
                            .admission_refusal(StreamConsumerObservation::Host)?,
                        result,
                    })
                    .is_err()
                {
                    return Ok(None);
                }
                let Ok(result) = response.await else {
                    return Ok(None);
                };
                Some(result?)
            } else {
                #[cfg(not(test))]
                anyhow::bail!("durable projection has no store admission");
                #[cfg(test)]
                None
            };
            self.pending = Some(self.input.receive(guard, None));
        }
        let result = std::future::poll_fn(|cx| {
            self.pending
                .as_mut()
                .expect("durable receive is missing")
                .as_mut()
                .poll(cx)
        })
        .await;
        let result = self.finish_receive(result);
        if result.is_err() {
            self.finished = true;
        }
        result.map(Some)
    }

    pub(crate) fn abort_for_teardown(&mut self) {
        self.finished = true;
        self.pending = None;
        self.pending_activity = None;
        self.input.reader = None;
    }
}

impl Drop for DurableInputProducer {
    fn drop(&mut self) {
        if self.finished || (self.runtime_teardown)() {
            return;
        }
        let Some(drop_event_sink) = &self.drop_event_sink else {
            return;
        };
        let role = match self.input.role.direction() {
            SessionStreamRole::Input => StreamCancelRole::InputConsumer,
            SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
            SessionStreamRole::ToolStdin
            | SessionStreamRole::ToolStdout
            | SessionStreamRole::ToolStderr => unreachable!(),
        };
        let _ = drop_event_sink.send(DropEvent::CancelDroppedDurableInput {
            cancellation: Box::new(DroppedDurableInput {
                streams: self.input.streams.clone(),
                transport_stream_id: self.input.transport_stream_id,
                consumer_read_ordinal: self.guest_read_ordinal.load(Ordering::Acquire),
                role,
            }),
        });
    }
}

impl DurableInputProducer {
    fn poll_guest_value<Ctx: WorkerCtx>(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContextMut<'_, Ctx>,
        capacity: usize,
        finish: bool,
    ) -> Poll<wasmtime::Result<(DurableInputItem, Option<StreamReadObserver>)>> {
        if finish {
            // Cancelling this native read does not close the durable source. Keep its
            // receive and prepared observation for the next read of the same producer.
            return Poll::Ready(Ok((
                DurableInputItem::Terminal(StreamResult::Cancelled),
                None,
            )));
        }
        if self.guest_delivery.is_none() {
            if capacity == 0 {
                return Poll::Ready(Ok((
                    DurableInputItem::Terminal(StreamResult::Completed),
                    None,
                )));
            }
            let item = std::task::ready!(self.poll_value(cx, store, false, Some(capacity)))?;
            let Some(observation) = self.pending_observation.take() else {
                return Poll::Ready(Ok((item, None)));
            };
            let ctx = store.data().durable_ctx();
            let (result, receiver) = oneshot::channel();
            let task = PrepareInputObservation {
                observation,
                streams: self.input.streams.clone(),
                replay: ctx.state.replay_state.clone(),
                guest_order: ctx.guest_read_order(),
                runtime: ctx.runtime_suspension.clone(),
                activity: ctx.tail_work_tracker().activity(),
                result,
                runtime_teardown: self.runtime_teardown.clone(),
                cleanup_sink: ctx
                    .state
                    .dropped_call_event_sender()
                    .expect("worker cleanup queue"),
                trap_context: DurableCallTrapContext {
                    retry_from: InFunctionRetryHost::current_retry_point(ctx),
                },
                live_call_permit: LiveCallPermit::new(ctx.state.live_host_call_counter()),
            };
            self.pending_guest_value = Some(item);
            store
                .as_context_mut()
                .spawn(async move |store| task.run(store).await)?;
            self.guest_delivery = Some(receiver);
        }
        let result = std::task::ready!(Pin::new(self.guest_delivery.as_mut().unwrap()).poll(cx));
        self.guest_delivery = None;
        let observer = result.map_err(|_| {
            WorkerExecutorError::runtime("durable stream read delivery task ended without a result")
        })??;
        Poll::Ready(Ok((
            self.pending_guest_value.take().unwrap(),
            Some(self.track_guest_consumption(observer)),
        )))
    }

    pub(super) fn track_guest_consumption(
        &self,
        observer: StreamReadObserver,
    ) -> StreamReadObserver {
        let consumed = self.guest_read_ordinal.clone();
        let next = self.input.consumer_read_ordinal;
        Box::new(move |consumption| {
            let delivered = consumption == wasmtime::component::TerminalConsumption::Delivered;
            observer(consumption)?;
            if delivered {
                consumed.store(next, Ordering::Release);
            }
            Ok(())
        })
    }

    fn poll_value<Ctx: WorkerCtx>(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContextMut<'_, Ctx>,
        finish: bool,
        guest_capacity: Option<usize>,
    ) -> Poll<wasmtime::Result<DurableInputItem>> {
        let guest = guest_capacity.is_some();
        if self.finished {
            return Poll::Ready(Ok(DurableInputItem::Terminal(StreamResult::Dropped)));
        }
        if finish {
            if !self.dropping {
                self.dropping = true;
                self.pending = None;
                self.pending_activity = None;
                let role = match self.input.role.direction() {
                    SessionStreamRole::Input => StreamCancelRole::InputConsumer,
                    SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
                    SessionStreamRole::ToolStdin
                    | SessionStreamRole::ToolStdout
                    | SessionStreamRole::ToolStderr => unreachable!(),
                };
                let (result, receiver) = oneshot::channel();
                let task = DurableInputDropTask {
                    cancellation: DroppedDurableInput {
                        streams: self.input.streams.clone(),
                        transport_stream_id: self.input.transport_stream_id,
                        consumer_read_ordinal: self.guest_read_ordinal.load(Ordering::Acquire),
                        role,
                    },
                    result,
                };
                store
                    .as_context_mut()
                    .spawn(async move |store| task.run(store).await)?;
                self.pending_drop = Some(Box::pin(async move {
                    receiver
                        .await
                        .map_err(|_| "durable input drop task ended without a result".to_string())?
                }));
            }
            match self
                .pending_drop
                .as_mut()
                .expect("drop cancellation is missing")
                .as_mut()
                .poll(cx)
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    return Poll::Ready(Err(wasmtime::Error::from_anyhow(error.into_trap())));
                }
                Poll::Ready(Ok(_)) => {
                    self.finished = true;
                    self.pending_drop = None;
                    self.input.reader = None;
                    return Poll::Ready(Ok(DurableInputItem::Terminal(StreamResult::Cancelled)));
                }
            }
        }
        if self.pending.is_none() {
            if self.input.opens_source() && self.live_admission.is_none() {
                let ctx = store.data().durable_ctx();
                if !ctx.is_live() {
                    if self.pending_past_a_completed_record
                        && ctx.rejects_live_continuation_at_replay_tail()
                    {
                        // Nothing wakes this read: the body gives it up, as it did when it
                        // was recorded.
                        return Poll::Pending;
                    }
                    let observation = if guest {
                        StreamConsumerObservation::Guest {
                            parent_start_index: ctx.entity_parent_start_index(),
                        }
                    } else {
                        StreamConsumerObservation::Host
                    };
                    let boundary = self.input.admission_refusal(observation)?;
                    let (result, receiver) = oneshot::channel();
                    let task = DurableInputLiveAdmission::<Ctx> {
                        replay_state: ctx.state.replay_state.clone(),
                        activity: ctx.tail_work_tracker().activity(),
                        boundary,
                        result,
                        _ctx: PhantomData,
                    };
                    store
                        .as_context_mut()
                        .spawn(async move |store| task.run(store).await)?;
                    self.live_admission = Some(receiver);
                }
            }
            if let Some(admission) = &mut self.live_admission {
                match Pin::new(admission).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(Ok(()))) => self.live_admission = None,
                    Poll::Ready(result) => {
                        self.finished = true;
                        let error = match result {
                            Ok(Err(error)) => error,
                            Err(_) => WorkerExecutorError::runtime(
                                "durable input live admission ended without a result",
                            ),
                            Ok(Ok(())) => unreachable!(),
                        };
                        return Poll::Ready(Err(error.into()));
                    }
                }
            }
            let live_call = LiveCallPermit::new(
                store
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .live_host_call_counter(),
            );
            let ctx = store.data().durable_ctx();
            let suspension = ctx.runtime_suspension.as_ref().map(|runtime| {
                let activity = Arc::new(OnceLock::new());
                self.pending_activity = Some(activity.clone());
                DurableReadWait {
                    runtime: runtime.clone(),
                    activity,
                    grace: ctx.state.config.suspend.wait_suspend_grace,
                    recheck: ctx.state.config.suspend.rpc_resume_after,
                    interrupt: ctx.create_interrupt_signal(),
                }
            });
            self.pending = Some(self.input.receive_observed(
                Some(ReceiveGuard {
                    _live_call: live_call,
                    interrupt: Some(ctx.create_interrupt_signal()),
                }),
                suspension,
                if guest {
                    StreamConsumerObservation::Guest {
                        parent_start_index: ctx.entity_parent_start_index(),
                    }
                } else {
                    StreamConsumerObservation::Host
                },
                guest_capacity.unwrap_or(1),
            ));
        }
        if let Some(binding) = &self.pending_activity
            && let Some(activity) = store.runtime_activity()
        {
            // The receive can survive cancellation of its original native transfer.
            // Later reads must not bind it to a second activity.
            let _ = binding.set(activity);
        }
        let receive_result = match self.pending.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        let guest_bytes = receive_result
            .as_ref()
            .ok()
            .and_then(|read| read.guest_bytes.clone());
        let value = match self.finish_receive(receive_result) {
            Ok(DurableInputEvent::Item(value)) => value,
            Ok(DurableInputEvent::End) => {
                return Poll::Ready(Ok(DurableInputItem::Terminal(StreamResult::Dropped)));
            }
            // A consumer cancel that this read did not request (`finish` returned above), such
            // as the end of the invocation that owns an unread input, ends the stream for the
            // reader. Wasmtime accepts `Cancelled` only as the answer to `finish`.
            Ok(DurableInputEvent::Cancelled) => {
                return Poll::Ready(Ok(DurableInputItem::Terminal(StreamResult::Dropped)));
            }
            Err(error) => {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::from_anyhow(error)));
            }
        };
        Poll::Ready(Ok(match guest_bytes {
            Some(bytes) => DurableInputItem::Bytes(bytes),
            None => DurableInputItem::Value(value),
        }))
    }
}

impl<Ctx: WorkerCtx> StreamProducer<Ctx> for DurableInputProducer {
    type Item = wire::SchemaValueTree;
    type Buffer = Option<wire::SchemaValueTree>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, Ctx>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let (item, observer) = if let Some(capacity) = destination.remaining(&mut store) {
            std::task::ready!(self.poll_guest_value(cx, &mut store, capacity.min(1), finish))?
        } else {
            (
                std::task::ready!(self.poll_value(cx, &mut store, finish, None))?,
                None,
            )
        };
        if let Some(observer) = observer {
            destination.register_read_observer(&mut store, move |consumption| {
                // A mismatched replay consumption already poisons the shared cursor.
                let _ = observer(consumption);
            })?;
        }
        let value = match item {
            DurableInputItem::Value(value) => value,
            DurableInputItem::Bytes(_) => unreachable!("item streams consume one value per read"),
            DurableInputItem::Terminal(result) => return Poll::Ready(Ok(result)),
        };
        let encoded = {
            let mut resolver = StoreValueResolver::new(&mut store);
            match encode_value_with_streams(&value, &mut resolver) {
                Ok(encoded) => encoded,
                Err(error) => {
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(error.to_string())));
                }
            }
        };
        destination.set_buffer(Some(encoded));
        Poll::Ready(Ok(StreamResult::Completed))
    }

    fn try_into(mut me: Pin<Box<Self>>, ty: TypeId) -> Result<Box<dyn Any>, Pin<Box<Self>>> {
        let producer = me.as_ref().get_ref();
        if ty == TypeId::of::<ForwardedDurableInput>()
            && producer.input.forwarded_handle().is_ok()
            && producer.pending.is_none()
            && producer.live_admission.is_none()
            && producer.pending_drop.is_none()
            && producer.guest_delivery.is_none()
            && producer.pending_guest_value.is_none()
            && !producer.finished
        {
            let handle = producer.input.handle.clone();
            let origin = producer.input.streams.clone();
            let reader_id = producer.input.reader_id.clone();
            me.as_mut().get_mut().finished = true;
            Ok(Box::new(ForwardedDurableInput {
                handle,
                origin,
                reader_id,
            }))
        } else {
            Err(me)
        }
    }
}

impl<Ctx: WorkerCtx> StreamProducer<Ctx> for DurableByteInputProducer {
    type Item = u8;
    type Buffer = bytes::Bytes;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, Ctx>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let (item, observer) = if let Some(capacity) = destination.remaining(&mut store) {
            std::task::ready!(self.0.poll_guest_value(cx, &mut store, capacity, finish))?
        } else {
            (
                std::task::ready!(self.0.poll_value(cx, &mut store, finish, None))?,
                None,
            )
        };
        if let Some(observer) = observer {
            destination.register_read_observer(&mut store, move |consumption| {
                // A mismatched replay consumption already poisons the shared cursor.
                let _ = observer(consumption);
            })?;
        }
        match item {
            DurableInputItem::Bytes(bytes) => {
                destination.set_buffer(bytes);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            DurableInputItem::Value(SchemaValue::U8(byte)) => {
                destination.set_buffer(bytes::Bytes::copy_from_slice(&[byte]));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            DurableInputItem::Value(_) => {
                self.0.finished = true;
                Poll::Ready(Err(wasmtime::Error::msg(
                    "tool stdin contains a non-byte value",
                )))
            }
            DurableInputItem::Terminal(result) => Poll::Ready(Ok(result)),
        }
    }
}

/// Maps one read of a durable byte input to the stdin of a tool body.
///
/// A cancel by the input's producer with the reason `cancelled` is not an error of the stream:
/// the body reads it as one `cancelled` item, and then the stream ends. Every other end of the
/// input, including a producer cancel with another reason, is a clean end without an item.
fn tool_stdin_delivery(
    item: DurableInputItem,
    cancelled_by_producer: bool,
) -> wasmtime::Result<ToolStdinDelivery> {
    Ok(match item {
        DurableInputItem::Bytes(bytes) => ToolStdinDelivery::Chunk(bytes.to_vec()),
        DurableInputItem::Value(SchemaValue::U8(byte)) => ToolStdinDelivery::Chunk(vec![byte]),
        DurableInputItem::Value(_) => {
            return Err(wasmtime::Error::msg("tool stdin contains a non-byte value"));
        }
        DurableInputItem::Terminal(StreamResult::Dropped) if cancelled_by_producer => {
            ToolStdinDelivery::Cancelled
        }
        DurableInputItem::Terminal(result) => ToolStdinDelivery::End(result),
    })
}

impl DurableToolStdinProducer {
    pub(crate) fn new(
        endpoint: DurableInputEndpoint,
        runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        let producer_cancel = Arc::new(OnceLock::new());
        Self {
            input: DurableInputProducer::new(endpoint)
                .with_runtime_teardown(runtime_teardown)
                .with_producer_cancel_observer(producer_cancel.clone())
                .with_pending_reads_past_a_completed_record(),
            producer_cancel,
            ended: false,
        }
    }
}

impl<Ctx: WorkerCtx> StreamProducer<Ctx> for DurableToolStdinProducer {
    type Item = Result<Vec<u8>, ByteStreamFailure>;
    type Buffer = Option<Self::Item>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, Ctx>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.ended {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let (item, observer) = match destination.remaining(&mut store) {
            Some(0) if !finish => return Poll::Ready(Ok(StreamResult::Completed)),
            Some(_) => std::task::ready!(self.input.poll_guest_value(
                cx,
                &mut store,
                MAX_PACKED_U8_STREAM_ITEM_SIZE,
                finish
            ))?,
            None => (
                std::task::ready!(self.input.poll_value(cx, &mut store, finish, None))?,
                None,
            ),
        };
        if let Some(observer) = observer {
            destination.register_read_observer(&mut store, move |consumption| {
                // A mismatched replay consumption already poisons the shared cursor.
                let _ = observer(consumption);
            })?;
        }
        let cancelled_by_producer = matches!(
            self.producer_cancel.get(),
            Some(StreamCancelReason::Cancelled)
        );
        match tool_stdin_delivery(item, cancelled_by_producer) {
            Ok(ToolStdinDelivery::Chunk(bytes)) => {
                destination.set_buffer(Some(Ok(bytes)));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Ok(ToolStdinDelivery::Cancelled) => {
                self.ended = true;
                destination.set_buffer(Some(Err(ByteStreamFailure::Cancelled)));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Ok(ToolStdinDelivery::End(result)) => Poll::Ready(Ok(result)),
            Err(error) => {
                self.input.finished = true;
                Poll::Ready(Err(error))
            }
        }
    }
}
