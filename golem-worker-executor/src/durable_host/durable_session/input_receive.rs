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

impl DurableStreamReader {
    pub(super) fn source_wait_can_suspend(&self) -> bool {
        match self {
            Self::Owned {
                source_wait_can_suspend,
                ..
            } => *source_wait_can_suspend,
            Self::Attached(_) => true,
        }
    }

    pub(super) fn journal_lag_sample_deadline(&mut self) -> &mut Instant {
        match self {
            Self::Owned {
                next_journal_lag_sample,
                ..
            } => next_journal_lag_sample,
            Self::Attached(reader) => &mut reader.next_journal_lag_sample,
        }
    }

    pub(super) async fn journal_lag_events(
        &self,
        after: Option<golem_common::model::durable_stream::StreamOffset>,
    ) -> Result<usize, StreamStoreError> {
        match self {
            Self::Owned { source, handle, .. } => source.journal_lag_events(handle, after).await,
            Self::Attached(reader) => {
                reader
                    .source
                    .journal_lag_events(&reader.handle, after)
                    .await
            }
        }
    }

    pub(super) async fn next(
        &mut self,
    ) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        let (event, foreign_binding) = match self {
            Self::Owned {
                reader,
                foreign_binding,
                ..
            } => (reader.next().await?, *foreign_binding),
            Self::Attached(reader) => (reader.next().await?, true),
        };
        Ok(event.map(|mut event| {
            if foreign_binding {
                event.nested_references = event
                    .nested_handles
                    .iter()
                    .cloned()
                    .map(StreamRecordReference::Foreign)
                    .collect();
            }
            event
        }))
    }
}

async fn record_source_journal_lag(
    reader: Option<&mut DurableStreamReader>,
    after: Option<golem_common::model::durable_stream::StreamOffset>,
    force: bool,
) {
    let lag = match reader {
        Some(reader) => {
            if !force && Instant::now() < *reader.journal_lag_sample_deadline() {
                return;
            }
            let result = reader.journal_lag_events(after).await;
            // Exact telemetry can require remote metadata IO; bound attempts, including failures.
            *reader.journal_lag_sample_deadline() = Instant::now() + Duration::from_millis(100);
            match result {
                Ok(lag) => lag,
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "Failed to sample durable consumer journal lag"
                    );
                    return;
                }
            }
        }
        None => 0,
    };
    crate::metrics::durable_stream::record_journal_lag(lag);
}

impl AttachedDurableCatchUpReader {
    async fn source_unavailable_overlay(
        &self,
    ) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        let Some((producer, reader_id)) = &self.consumer_producer else {
            return Ok(None);
        };
        let source_offset = producer
            .consumer_source_unavailable(&self.attachment, *reader_id)
            .await?;
        Ok(source_offset.map(|offset| CommittedProducerStreamEvent {
            stream_id: self.handle.stream_id,
            producer_sequence: 0,
            offset,
            packed_u8_batch_end: None,
            terminal_author: None,
            nested_handles: Vec::new(),
            nested_references: Vec::new(),
            payload: CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::System,
                reason: StreamCancelReason::SourceUnavailable,
                details: None,
            },
        }))
    }

    pub(super) async fn next(
        &mut self,
    ) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        loop {
            if let Some(event) = self.buffered.pop_front() {
                self.after = Some(event.offset);
                self.terminal = matches!(
                    event.payload,
                    CommittedProducerStreamEventPayload::End(_)
                        | CommittedProducerStreamEventPayload::Cancel { .. }
                );
                return Ok(Some(event));
            }
            if self.terminal {
                return Ok(None);
            }
            if let Some(event) = self.source_unavailable_overlay().await? {
                self.buffered.push_back(event);
                continue;
            }
            let events = match self
                .source
                .wait_for_attached_segment(
                    &self.attachment,
                    &self.handle,
                    Timestamp::now_utc().to_millis(),
                    self.after,
                )
                .await
            {
                Ok(events) => events,
                Err(error) => {
                    if let Some(event) = self.source_unavailable_overlay().await? {
                        self.buffered.push_back(event);
                        continue;
                    }
                    return Err(error);
                }
            };
            self.buffered.extend(events);
        }
    }
}

impl From<SessionError> for DurableReceiveError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<InterruptKind> for DurableReceiveError {
    fn from(kind: InterruptKind) -> Self {
        Self::Interrupt(kind)
    }
}

impl From<String> for DurableReceiveError {
    fn from(error: String) -> Self {
        Self::Session(error.into())
    }
}

impl From<&str> for DurableReceiveError {
    fn from(error: &str) -> Self {
        Self::Session(error.into())
    }
}

pub(super) async fn await_first_durable_source<F: Future + Send>(
    native: F,
    guard: &mut Option<ReceiveGuard>,
) -> Result<F::Output, InterruptKind>
where
    F::Output: Send,
{
    match guard.as_mut().and_then(|guard| guard.interrupt.as_mut()) {
        Some(interrupt) => tokio::select! {
            biased;
            result = native => Ok(result),
            kind = interrupt => Err(kind),
        },
        None => Ok(native.await),
    }
}

impl DurableReadWait {
    pub(super) async fn wait<F: Future + Send>(self, future: F) -> Result<F::Output, InterruptKind>
    where
        F::Output: Send,
    {
        let mut source = Some(Box::pin(future));
        let mut interrupt = Some(self.interrupt);
        let mut classified: Option<BoxFuture<'_, Result<F::Output, InterruptKind>>> = None;
        std::future::poll_fn(|cx| {
            // The initial synchronous producer poll precedes transfer admission. Bind on a
            // subsequent poll, without restarting the read or its logical grace period.
            if classified.is_none()
                && let Some(activity) = self.activity.get()
                && let Some(wait) = self.runtime.rpc_wait(*activity, self.grace, self.recheck)
            {
                let source = source.take().unwrap();
                classified = Some(Box::pin(
                    wait.wait_result(source, interrupt.take().unwrap()),
                ));
            }
            match classified.as_mut() {
                Some(wait) => wait.as_mut().poll(cx),
                None => match interrupt.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Ready(kind) => Poll::Ready(Err(kind)),
                    Poll::Pending => source.as_mut().unwrap().as_mut().poll(cx).map(Ok),
                },
            }
        })
        .await
    }
}

impl PrepareInputObservation {
    pub(super) async fn run<Ctx: WorkerCtx>(
        self,
        _accessor: &Accessor<Ctx>,
    ) -> wasmtime::Result<()> {
        let settled = self.prepare().await?;
        // The event loop must still see host work while the journal receipt settles.
        let _ = settled.await;
        Ok(())
    }
}

impl PrepareInputObservation {
    pub(super) async fn prepare(self) -> wasmtime::Result<oneshot::Receiver<()>> {
        let mut record = self.observation.record;
        let mut recorded_index = None;
        if let Some(recorded) = &self.observation.recorded {
            let original = self
                .streams
                .session_record_at(recorded.original_index)
                .await?;
            if !record.matches_reobserved(&original) {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "unchanged logical consumer input",
                    format!(
                        "read differs from original observation at {}",
                        recorded.original_index
                    ),
                )
                .into());
            }
            for (index, observation) in &recorded.boundaries {
                if self.replay.is_in_skipped_region(*index).await? {
                    continue;
                }
                let expected = match &record {
                    StreamSessionRecord::ConsumerItemValue(record) => &record.observation,
                    StreamSessionRecord::ConsumerTerminal(record) => &record.observation,
                    _ => unreachable!(),
                };
                if recorded_index.replace(*index).is_some() || expected != observation {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "one matching consumer observation boundary",
                        format!("conflicting consumer observation at {index}"),
                    )
                    .into());
                }
            }
            if recorded_index.is_none() {
                match &mut record {
                    StreamSessionRecord::ConsumerItemValue(record) => {
                        record.reobserves = Some(recorded.original_index);
                    }
                    StreamSessionRecord::ConsumerTerminal(record) => {
                        record.reobserves = Some(recorded.original_index);
                    }
                    _ => unreachable!(),
                }
            }
        }
        InputObservationDelivery {
            replay: self.replay,
            guest_order: self.guest_order,
            runtime: self.runtime,
            activity: self.activity,
            runtime_teardown: self.runtime_teardown,
            cleanup_sink: self.cleanup_sink,
            trap_context: self.trap_context,
            live_call_permit: self.live_call_permit,
        }
        .prepare(
            recorded_index,
            async move {
                let record = self
                    .streams
                    .producer
                    .prepare_consumer_observation(self.streams.entity_parent_start_index, record)
                    .await
                    .map_err(|error| {
                        SessionError::from(error)
                            .into_worker_executor_error(WorkerExecutorError::runtime)
                    })?;
                let enqueue: EnqueueInputObservation = Box::new(move || {
                    let receipt = record
                        .enqueue(self.observation.introduced)
                        .map_err(|error| {
                            SessionError::from(error)
                                .into_worker_executor_error(WorkerExecutorError::runtime)
                        })?;
                    Ok(Box::pin(async move {
                        receipt.await.map(|_| ()).map_err(|error| {
                            SessionError::from(error)
                                .into_worker_executor_error(WorkerExecutorError::runtime)
                        })
                    }))
                });
                Ok(enqueue)
            },
            self.result,
        )
        .await
    }
}

impl<Ctx: WorkerCtx> DurableInputLiveAdmission<Ctx> {
    pub(super) async fn run(self, accessor: &Accessor<Ctx>) -> wasmtime::Result<()> {
        let outcome = async {
            loop {
                let (operation, rejected) = accessor.with(|mut access| {
                    let ctx = access.get().durable_ctx_mut();
                    (
                        ctx.entity_tool_operation(),
                        ctx.rejects_live_continuation_at_replay_tail(),
                    )
                });
                if let Some(operation) = operation {
                    operation
                        .reject_missing_replay_admission(&self.boundary)
                        .await?;
                }
                if rejected {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "durable input observation",
                        "end of the recorded consumer journal during completed entity replay",
                    ));
                }
                self.replay_state
                    .await_natural_tail_end(Some(&self.activity))
                    .await?;
                let (transition, primary) = accessor.with(|mut access| {
                    let ctx = access.get().durable_ctx_mut();
                    (
                        ctx.prepare_live_continuation_at_replay_tail(
                            true,
                            "durable input stream source read".to_string(),
                        ),
                        ctx.runtime == OwnerRuntime::Agent,
                    )
                });
                let pending = match transition.await? {
                    BeginReplayToLive::ReplayResumed => continue,
                    BeginReplayToLive::Pending(pending) => pending,
                };
                finish_prepared_access_to_live_at(
                    pending,
                    primary,
                    accessor,
                    DurableWorkerCtxView::durable_ctx_mut,
                    Some(self.boundary.clone()),
                )
                .await?
                .require_live()?;
                return Ok(());
            }
        }
        .await;
        let _ = self.result.send(outcome);
        Ok(())
    }
}

impl DurableInputReceiveAdmission {
    pub(crate) async fn run<U: Send + 'static, Ctx: WorkerCtx>(
        self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> wasmtime::Result<()> {
        let mut result = self.result;
        let admit = async {
            if self.opens_source {
                loop {
                    let operation =
                        accessor.with(|mut access| access.get().entity_tool_operation());
                    if let Some(operation) = operation {
                        operation
                            .reject_missing_replay_admission(&self.boundary)
                            .await?;
                    }
                    let state = accessor.with(|mut access| {
                        let ctx = access.get();
                        if ctx.is_live() {
                            return Ok(None);
                        }
                        if ctx.rejects_live_continuation_at_replay_tail() {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                format!("durable input observation {}", self.ordinal),
                                "end of the recorded consumer journal during completed entity replay",
                            ));
                        }
                        Ok(Some((ctx.state.replay_state.clone(), ctx.tail_work_tracker().activity())))
                    })?;
                    let Some((replay, activity)) = state else {
                        break;
                    };
                    replay.await_natural_tail_end(Some(&activity)).await?;
                    let (transition, primary) = accessor.with(|mut access| {
                        let ctx = access.get();
                        (
                            ctx.prepare_live_continuation_at_replay_tail(
                                true,
                                "durable input stream source read".to_string(),
                            ),
                            ctx.runtime == OwnerRuntime::Agent,
                        )
                    });
                    match transition.await? {
                        BeginReplayToLive::ReplayResumed => continue,
                        BeginReplayToLive::Pending(pending) => {
                            finish_prepared_access_to_live_at(
                                pending,
                                primary,
                                accessor,
                                accessor.getter(),
                                Some(self.boundary.clone()),
                            )
                            .await?
                            .require_live()?;
                            break;
                        }
                    }
                }
            }
            Ok(accessor.with(|mut access| {
                let ctx = access.get();
                ReceiveGuard {
                    _live_call: LiveCallPermit::new(ctx.state.live_host_call_counter()),
                    interrupt: Some(ctx.create_interrupt_signal()),
                }
            }))
        };
        tokio::select! {
            biased;
            _ = result.closed() => {},
            outcome = admit => { let _ = result.send(outcome); }
        }
        Ok(())
    }
}

impl DurableInputEndpoint {
    pub(super) fn admission_refusal(
        &self,
        observation: StreamConsumerObservation,
    ) -> Result<SerializableToolAdmissionRefusal, WorkerExecutorError> {
        let binding = self
            .streams
            .binding(self.transport_stream_id)
            .or_else(|| {
                self.streams
                    .mapping(self.transport_stream_id)
                    .map(|mapping| StreamBindingRecord::foreign(&mapping))
            })
            .ok_or_else(|| WorkerExecutorError::runtime("durable input has no session binding"))?;
        Ok(SerializableToolAdmissionRefusal::BeforeInputRead {
            session: self.streams.session_reference.clone(),
            binding: Box::new(binding),
            consumer_read_ordinal: self.consumer_read_ordinal,
            observation,
        })
    }

    pub(super) fn opens_source(&self) -> bool {
        self.journal.is_empty() && self.reader.is_none() && !self.source_terminal
    }

    pub(super) fn receive(
        &mut self,
        guard: Option<ReceiveGuard>,
        suspension: Option<DurableReadWait>,
    ) -> DurableReceiveFuture {
        self.receive_observed(guard, suspension, StreamConsumerObservation::Host, 1)
    }

    pub(super) fn receive_observed(
        &mut self,
        guard: Option<ReceiveGuard>,
        suspension: Option<DurableReadWait>,
        observation: StreamConsumerObservation,
        capacity: usize,
    ) -> DurableReceiveFuture {
        let opens_source = self.opens_source();
        let mut reader = self.reader.take();
        let queued_event = self.journal.pop_front();
        let mut resident_journal =
            if capacity > 1 && matches!(observation, StreamConsumerObservation::Guest { .. }) {
                std::mem::take(&mut self.journal)
            } else {
                VecDeque::new()
            };
        let recorded_mappings = queued_event
            .as_ref()
            .and_then(|event| self.nested_mappings.remove(&event.offset))
            .unwrap_or_default();
        let source_after = self.source_after;
        let streams = self.streams.clone();
        let stream_id = self.handle.stream_id;
        let reader_id = self.reader_id.clone();
        let ordinal = self.consumer_read_ordinal;
        let recorded_observation = self.observations.remove(&ordinal);
        let role = self.role;
        Box::pin(async move {
            let mut guard = guard;
            let reader_id = reader_id.await?;
            let guest = matches!(observation, StreamConsumerObservation::Guest { .. });
            if !guest
                && let Some(recorded) = &recorded_observation
                && recorded
                    .boundaries
                    .iter()
                    .any(|(_, recorded)| *recorded != observation)
            {
                return Err(
                    SessionError::from(WorkerExecutorError::unexpected_oplog_entry(
                        "host projection observation",
                        format!("consumer observation {observation:?}"),
                    ))
                    .into(),
                );
            }
            let mut journaled = queued_event.is_some();
            let event = match queued_event {
                Some(event) => Some(event),
                None => {
                    if opens_source {
                        let mapping = streams
                            .current_control_metadata()
                            .await?
                            .reader_binding(reader_id)?
                            .clone();
                        let mapping = streams
                            .producer
                            .materialize_binding(&mapping)
                            .await
                            .map_err(SessionError::from)?;
                        reader = Some(streams.stream_reader(mapping, source_after).await?);
                        record_source_journal_lag(reader.as_mut(), source_after, true).await;
                    }
                    let result = match reader.as_mut() {
                        Some(reader) => {
                            let can_suspend = reader.source_wait_can_suspend();
                            let native = reader.next();
                            match suspension {
                                Some(wait) if can_suspend => wait.wait(native).await?,
                                _ => await_first_durable_source(native, &mut guard).await?,
                            }
                        }
                        None => Ok(None),
                    };
                    result.map_err(SessionError::from)?
                }
            };
            if event.as_ref().is_some_and(|event| {
                event.terminal_author.is_none()
                    && matches!(
                        &event.payload,
                        CommittedProducerStreamEventPayload::Cancel {
                            role: StreamCancelRole::System,
                            reason: StreamCancelReason::SourceUnavailable,
                            ..
                        }
                    )
            }) {
                journaled = true;
            }
            let mut endpoints = HashMap::new();
            let mut queued_events = VecDeque::<CommittedProducerStreamEvent>::new();
            let mut nested_mappings = Vec::new();
            let mut pending_observation = None;
            let mut guest_bytes = None;
            let mut introducing_index = None;
            if let Some(event) = &event {
                let record = match &event.payload {
                    CommittedProducerStreamEventPayload::Value(bytes) => {
                        let value = ProtoSchemaValue::decode(bytes.as_slice())
                            .map_err(|error| format!("invalid durable stream value: {error}"))?;
                        let handle_indices = preflight_proto_recursive_stream_value(&value)?;
                        if handle_indices.len() != event.nested_handles.len() {
                            return Err(
                                "nested durable input handle count does not match the canonical value"
                                    .to_string().into(),
                            );
                        }
                        for (position, (handle_index, handle)) in handle_indices
                            .into_iter()
                            .zip(event.nested_handles.iter().cloned())
                            .enumerate()
                        {
                            if handle_index != position as u64 {
                                return Err(format!(
                                    "invalid canonical nested input handle index {handle_index}"
                                )
                                .into());
                            }
                            let mapping = if journaled {
                                let mapping = recorded_mappings.get(position).ok_or_else(|| {
                                    "recorded nested durable input has no reader binding"
                                        .to_string()
                                })?;
                                if mapping.handle != handle || mapping.role != role {
                                    return Err(
                                        "recorded nested reader binding differs from its value"
                                            .into(),
                                    );
                                }
                                mapping.clone()
                            } else {
                                StreamSessionMappingRecord {
                                    transport_stream_id: streams.allocate_transport_stream_id()?,
                                    handle,
                                    role,
                                }
                            };
                            nested_mappings.push(mapping);
                        }
                        StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                            observation: observation.clone(),
                            reobserves: None,
                            session_key: streams.session_reference.clone(),
                            reader_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            value: bytes.clone(),
                            packed_u8: false,
                            recursive_mappings: nested_mappings
                                .iter()
                                .map(StreamBindingRecord::foreign)
                                .collect(),
                        })
                    }
                    CommittedProducerStreamEventPayload::PackedU8(byte) => {
                        let mut bytes = vec![*byte];
                        if (!journaled && !guest) || (guest && capacity > 1) {
                            let batch_end = event.packed_u8_batch_end.ok_or_else(|| {
                                "packed-u8 durable input event is missing its batch boundary"
                                    .to_string()
                            })?;
                            if batch_end.producer_oplog_index()
                                != event.offset.producer_oplog_index()
                                || batch_end.sub_index() < event.offset.sub_index()
                            {
                                return Err("packed-u8 durable input batch boundary is invalid"
                                    .to_string()
                                    .into());
                            }
                            while queued_events
                                .back()
                                .map(|event| event.offset)
                                .unwrap_or(event.offset)
                                != batch_end
                            {
                                if guest && bytes.len() >= capacity {
                                    break;
                                }
                                if bytes.len() >= MAX_PACKED_U8_STREAM_ITEM_SIZE {
                                    return Err(
                                        "packed-u8 durable input batch exceeds its size limit"
                                            .to_string()
                                            .into(),
                                    );
                                }
                                let next = if guest && journaled {
                                    resident_journal.pop_front()
                                } else {
                                    let next = reader
                                        .as_mut()
                                        .expect("durable input reader is missing")
                                        .next();
                                    let next = if guest {
                                        let Some(next) = next.now_or_never() else {
                                            break;
                                        };
                                        next
                                    } else {
                                        next.await
                                    };
                                    next.map_err(SessionError::from)?
                                }
                                .ok_or_else(|| {
                                    "packed-u8 durable input batch ended before its boundary"
                                        .to_string()
                                })?;
                                let expected_sub_index = event
                                    .offset
                                    .sub_index()
                                    .checked_add(bytes.len() as u32)
                                    .ok_or_else(|| {
                                        "packed-u8 consumer journal offset overflow".to_string()
                                    })?;
                                let next_byte = match &next.payload {
                                    CommittedProducerStreamEventPayload::PackedU8(byte)
                                        if next.offset.producer_oplog_index()
                                            == event.offset.producer_oplog_index()
                                            && next.offset.sub_index() == expected_sub_index
                                            && next.packed_u8_batch_end == Some(batch_end)
                                            && next.nested_handles.is_empty() =>
                                    {
                                        Some(*byte)
                                    }
                                    _ => None,
                                };
                                if let Some(byte) = next_byte {
                                    bytes.push(byte);
                                    queued_events.push_back(next);
                                } else {
                                    return Err("packed-u8 durable input batch is not contiguous"
                                        .to_string()
                                        .into());
                                }
                            }
                        }
                        if guest && capacity > 1 {
                            guest_bytes = Some(bytes::Bytes::copy_from_slice(&bytes));
                            queued_events.clear();
                        }
                        StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                            observation: observation.clone(),
                            reobserves: None,
                            session_key: streams.session_reference.clone(),
                            reader_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            value: bytes,
                            packed_u8: true,
                            recursive_mappings: Vec::new(),
                        })
                    }
                    CommittedProducerStreamEventPayload::End(result) => {
                        StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                            observation: observation.clone(),
                            reobserves: None,
                            session_key: streams.session_reference.clone(),
                            reader_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            terminal: StreamConsumerTerminal::End(result.clone()),
                        })
                    }
                    CommittedProducerStreamEventPayload::Cancel {
                        role,
                        reason,
                        details,
                    } => StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                        format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                        observation: observation.clone(),
                        reobserves: None,
                        session_key: streams.session_reference.clone(),
                        reader_id,
                        source_offset: event.offset,
                        consumer_read_ordinal: ordinal,
                        terminal: StreamConsumerTerminal::Cancel {
                            role: *role,
                            reason: *reason,
                            details: details.clone(),
                        },
                    }),
                };
                if guest {
                    // Only nested readers wait for the parent's oplog index.
                    let introduced = (!nested_mappings.is_empty()).then(|| {
                        let (introduced, index) = oneshot::channel::<Result<OplogIndex, String>>();
                        introducing_index = Some(
                            async move {
                                index.await.map_err(|_| {
                                    "parent stream item was not observed".to_string()
                                })?
                            }
                            .boxed()
                            .shared(),
                        );
                        introduced
                    });
                    pending_observation = Some(PendingConsumerObservation {
                        record,
                        recorded: recorded_observation,
                        introduced,
                    });
                } else if !journaled {
                    streams.append_record(None, record).await?;
                    streams.commit_consumer_journal().await?;
                    let committed_through = queued_events
                        .back()
                        .map(|event| event.offset)
                        .unwrap_or(event.offset);
                    record_source_journal_lag(
                        reader.as_mut(),
                        Some(committed_through),
                        event.is_terminal(),
                    )
                    .await;
                    tracing::debug!(
                        stream_id = %stream_id,
                        source_offset = %event.offset,
                        consumer_read_ordinal = ordinal,
                        logical_item_count = queued_events.len() + 1,
                        replayed = false,
                        "Durable consumer value journal committed before guest delivery"
                    );
                } else {
                    tracing::debug!(
                        stream_id = %stream_id,
                        source_offset = %event.offset,
                        consumer_read_ordinal = ordinal,
                        replayed = true,
                        "Durable consumer value journal replayed before guest delivery"
                    );
                }
            }
            for (position, mapping) in nested_mappings.into_iter().enumerate() {
                streams.insert_mapping(mapping.clone())?;
                let endpoint = if let Some(introducing_index) =
                    introducing_index.as_ref().filter(|_| !journaled)
                {
                    let introducing_index = introducing_index.clone();
                    DurableInputEndpoint {
                        reader: None,
                        journal: VecDeque::new(),
                        nested_mappings: HashMap::new(),
                        observations: HashMap::new(),
                        source_after: None,
                        source_terminal: false,
                        streams: streams.clone(),
                        transport_stream_id: mapping.transport_stream_id,
                        handle: mapping.handle,
                        consumer_read_ordinal: 0,
                        role: mapping.role,
                        reader_id: async move {
                            Ok(LocalStreamReaderId {
                                introducing_oplog_index: introducing_index.await?,
                                binding_slot: position as u32,
                            })
                        }
                        .boxed()
                        .shared(),
                    }
                } else {
                    streams.endpoint_for_mapping(mapping, 0).await?
                };
                endpoints.insert(position as u64, endpoint);
            }
            queued_events.append(&mut resident_journal);
            Ok(DurableInputRead {
                reader,
                event,
                endpoints,
                guest_bytes,
                #[cfg(test)]
                journaled,
                queued_events,
                observation: pending_observation,
            })
        })
    }

    pub(super) fn complete_receive(&mut self, read: &mut DurableInputRead) {
        self.reader = read.reader.take();
        self.journal.append(&mut read.queued_events);
        if read.event.is_some() {
            self.consumer_read_ordinal +=
                read.guest_bytes.as_ref().map_or(1, |bytes| bytes.len()) as u64;
        }
    }
}
