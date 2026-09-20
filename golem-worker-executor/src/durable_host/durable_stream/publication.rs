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

use super::index::validate_items_payload;
use super::*;

// This is a producer-wide budget rather than a per-stream budget. In particular, creating many
// streams cannot multiply the amount of payload retained by a worker.
pub(super) const COMMITTED_RETENTION_MAX_ENTRIES: usize = 4096;
const COMMITTED_RETENTION_MAX_BYTES: usize = 32 * 1024 * 1024;
pub(super) const STREAM_SEGMENT_MAX_EVENTS: usize = 256;
pub(super) const STREAM_SEGMENT_TARGET_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
pub(super) struct CommittedEventRetention {
    pub(super) batches: VecDeque<RetainedCommittedBatch>,
    pub(super) entries: usize,
    pub(super) bytes: usize,
}

pub(super) struct RetainedCommittedBatch {
    pub(super) events: RetainedCommittedEvents,
    encoded_event_bytes: Option<usize>,
    pub(super) retained_bytes: usize,
}

pub(super) enum RetainedCommittedEvents {
    Shared(Arc<CommittedProducerStreamEvent>),
    Packed {
        stream_id: StreamId,
        first_sequence: u64,
        first_offset: StreamOffset,
        bytes: Arc<Vec<u8>>,
        batch_end: StreamOffset,
        nested_handles: Arc<Vec<DurableStreamHandle>>,
        nested_references: Arc<Vec<StreamRecordReference>>,
    },
}

impl RetainedCommittedBatch {
    fn stream_id(&self) -> StreamId {
        match &self.events {
            RetainedCommittedEvents::Shared(event) => event.stream_id,
            RetainedCommittedEvents::Packed { stream_id, .. } => *stream_id,
        }
    }

    pub(super) fn first_sequence(&self) -> u64 {
        match &self.events {
            RetainedCommittedEvents::Shared(event) => event.producer_sequence,
            RetainedCommittedEvents::Packed { first_sequence, .. } => *first_sequence,
        }
    }

    fn len(&self) -> usize {
        match &self.events {
            RetainedCommittedEvents::Shared(_) => 1,
            RetainedCommittedEvents::Packed { bytes, .. } => bytes.len(),
        }
    }

    fn offset_at(&self, index: usize) -> Option<StreamOffset> {
        match &self.events {
            RetainedCommittedEvents::Shared(event) => (index == 0).then_some(event.offset),
            RetainedCommittedEvents::Packed {
                first_offset,
                bytes,
                ..
            } => {
                if index >= bytes.len() {
                    return None;
                }
                let sub_index = first_offset
                    .sub_index()
                    .checked_add(u32::try_from(index).ok()?)?;
                Some(StreamOffset::new(
                    first_offset.producer_oplog_index(),
                    sub_index,
                ))
            }
        }
    }

    fn index_of(&self, stream_id: StreamId, offset: StreamOffset) -> Option<usize> {
        if self.stream_id() != stream_id {
            return None;
        }
        match &self.events {
            RetainedCommittedEvents::Shared(event) => (event.offset == offset).then_some(0),
            RetainedCommittedEvents::Packed {
                first_offset,
                bytes,
                ..
            } => {
                if offset.producer_oplog_index() != first_offset.producer_oplog_index() {
                    return None;
                }
                let index = offset.sub_index().checked_sub(first_offset.sub_index())? as usize;
                (index < bytes.len()).then_some(index)
            }
        }
    }
}

pub(super) fn encoded_event_bytes(event: &CommittedProducerStreamEvent) -> usize {
    golem_common::serialization::serialize(event)
        .expect("committed stream event serialization cannot fail")
        .len()
}

impl DurableStreamStore {
    /// Returns bytes retained only as a disposable live-publication optimization.
    pub(crate) fn retained_payload_bytes(
        payload: &StreamItemsPayload,
    ) -> Result<usize, StreamStoreError> {
        validate_items_payload(payload)?;
        Ok(match payload {
            StreamItemsPayload::Values(values) => values.iter().map(Vec::len).sum::<usize>() * 2,
            StreamItemsPayload::PackedU8(bytes) => {
                bytes.len() * (std::mem::size_of::<CommittedProducerStreamEvent>() + 2)
            }
        })
    }

    pub(super) fn retain_committed_events(&self, events: &[CommittedProducerStreamEvent]) {
        // Only newly committed writes enter retention; replay repairs publish directly to the bus.
        if events.is_empty() {
            return;
        }
        let mut retained = self
            .committed_retention
            .lock()
            .expect("durable stream committed retention lock poisoned");
        let packed = events.iter().enumerate().all(|(index, event)| {
            let Some(sequence) = events[0].producer_sequence.checked_add(index as u64) else {
                return false;
            };
            let Some(sub_index) = u32::try_from(index)
                .ok()
                .and_then(|index| events[0].offset.sub_index().checked_add(index))
            else {
                return false;
            };
            matches!(
                event.payload,
                CommittedProducerStreamEventPayload::PackedU8(_)
            ) && event.stream_id == events[0].stream_id
                && event.producer_sequence == sequence
                && event.offset
                    == StreamOffset::new(events[0].offset.producer_oplog_index(), sub_index)
                && event.packed_u8_batch_end == events[0].packed_u8_batch_end
                && event.terminal_author.is_none()
                && event.nested_handles == events[0].nested_handles
                && event.nested_references == events[0].nested_references
        }) && events[0].packed_u8_batch_end == events.last().map(|event| event.offset);
        let batches = if packed {
            let bytes: Vec<_> = events
                .iter()
                .map(|event| match event.payload {
                    CommittedProducerStreamEventPayload::PackedU8(byte) => byte,
                    _ => unreachable!(),
                })
                .collect();
            let nested_handles = events[0].nested_handles.clone();
            let nested_references = events[0].nested_references.clone();
            vec![RetainedCommittedBatch {
                encoded_event_bytes: None,
                retained_bytes: std::mem::size_of::<RetainedCommittedBatch>()
                    .saturating_add(bytes.capacity())
                    .saturating_add(4 * std::mem::size_of::<usize>())
                    .saturating_add(
                        nested_handles
                            .capacity()
                            .saturating_mul(std::mem::size_of::<DurableStreamHandle>()),
                    ),
                events: RetainedCommittedEvents::Packed {
                    stream_id: events[0].stream_id,
                    first_sequence: events[0].producer_sequence,
                    first_offset: events[0].offset,
                    bytes: Arc::new(bytes),
                    batch_end: events[0]
                        .packed_u8_batch_end
                        .expect("materialized packed batch has an end offset"),
                    nested_handles: Arc::new(nested_handles),
                    nested_references: Arc::new(nested_references),
                },
            }]
        } else {
            // Ordinary values and terminals retain their payload behind a shared allocation.
            events
                .iter()
                .filter_map(|event| {
                    let encoded_event_bytes = encoded_event_bytes(event);
                    let retained_bytes = encoded_event_bytes
                        .saturating_add(std::mem::size_of::<RetainedCommittedBatch>())
                        .saturating_add(std::mem::size_of::<CommittedProducerStreamEvent>());
                    if retained_bytes > COMMITTED_RETENTION_MAX_BYTES {
                        return None;
                    }
                    Some(RetainedCommittedBatch {
                        encoded_event_bytes: Some(encoded_event_bytes),
                        retained_bytes,
                        events: RetainedCommittedEvents::Shared(Arc::new(event.clone())),
                    })
                })
                .collect()
        };
        for batch in batches {
            retained.entries = retained.entries.saturating_add(1);
            retained.bytes = retained.bytes.saturating_add(batch.retained_bytes);
            retained.batches.push_back(batch);
        }
        while retained.entries > COMMITTED_RETENTION_MAX_ENTRIES
            || retained.bytes > COMMITTED_RETENTION_MAX_BYTES
        {
            let Some(evicted) = retained.batches.pop_front() else {
                break;
            };
            retained.entries = retained.entries.saturating_sub(1);
            retained.bytes = retained.bytes.saturating_sub(evicted.retained_bytes);
        }
    }

    pub(super) fn retained_segment(
        &self,
        stream_id: StreamId,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Option<Vec<CommittedProducerStreamEvent>> {
        let retained = self
            .committed_retention
            .lock()
            .expect("durable stream committed retention lock poisoned");
        let cursor = after.and_then(|after| {
            retained
                .batches
                .iter()
                .find_map(|batch| batch.index_of(stream_id, after).map(|index| (batch, index)))
        });
        if after.is_some() && cursor.is_none() {
            return None;
        }
        let mut result = Vec::new();
        let mut bytes = 0usize;
        let mut expected_sequence = cursor
            .and_then(|(batch, index)| batch.first_sequence().checked_add(index as u64 + 1))
            .unwrap_or(0);
        let mut found_start = false;
        for batch in &retained.batches {
            if batch.stream_id() != stream_id {
                continue;
            }
            let batch_end_sequence = batch.first_sequence().saturating_add(batch.len() as u64);
            if batch_end_sequence <= expected_sequence {
                continue;
            }
            if batch.first_sequence() > expected_sequence {
                return None;
            }
            found_start = true;
            let start = usize::try_from(expected_sequence - batch.first_sequence()).ok()?;
            match &batch.events {
                RetainedCommittedEvents::Shared(event) => {
                    if through.is_some_and(|through| event.offset > through) {
                        break;
                    }
                    let event_bytes = batch
                        .encoded_event_bytes
                        .expect("ordinary retained event has an encoded size");
                    if !result.is_empty()
                        && bytes.saturating_add(event_bytes) > STREAM_SEGMENT_TARGET_BYTES
                    {
                        break;
                    }
                    bytes = bytes.saturating_add(event_bytes);
                    expected_sequence = expected_sequence.saturating_add(1);
                    result.push((**event).clone());
                }
                RetainedCommittedEvents::Packed {
                    stream_id,
                    first_sequence,
                    bytes: packed_bytes,
                    batch_end,
                    nested_handles,
                    nested_references,
                    ..
                } => {
                    let available = packed_bytes.len().saturating_sub(start);
                    let mut count = available.min(STREAM_SEGMENT_MAX_EVENTS - result.len());
                    if let Some(through) = through {
                        let through_index = batch.index_of(*stream_id, through);
                        if through < batch.offset_at(start)? {
                            break;
                        }
                        if let Some(through_index) = through_index {
                            count = count.min(through_index.saturating_sub(start) + 1);
                        }
                    }
                    if count == 0 {
                        break;
                    }
                    let representative = CommittedProducerStreamEvent {
                        stream_id: *stream_id,
                        producer_sequence: u64::MAX,
                        offset: StreamOffset::new(
                            batch.offset_at(start)?.producer_oplog_index(),
                            u32::MAX,
                        ),
                        packed_u8_batch_end: Some(*batch_end),
                        terminal_author: None,
                        nested_handles: (**nested_handles).clone(),
                        nested_references: (**nested_references).clone(),
                        payload: CommittedProducerStreamEventPayload::PackedU8(u8::MAX),
                    };
                    let event_bound = encoded_event_bytes(&representative);
                    if !result.is_empty() || bytes > 0 {
                        count = count.min(
                            STREAM_SEGMENT_TARGET_BYTES
                                .saturating_sub(bytes)
                                .checked_div(event_bound)
                                .unwrap_or(0),
                        );
                    } else if event_bound.saturating_mul(count) > STREAM_SEGMENT_TARGET_BYTES {
                        count = (STREAM_SEGMENT_TARGET_BYTES / event_bound)
                            .max(1)
                            .min(count);
                    }
                    for index in start..start + count {
                        result.push(CommittedProducerStreamEvent {
                            stream_id: *stream_id,
                            producer_sequence: first_sequence + index as u64,
                            offset: batch.offset_at(index)?,
                            packed_u8_batch_end: Some(*batch_end),
                            terminal_author: None,
                            nested_handles: (**nested_handles).clone(),
                            nested_references: (**nested_references).clone(),
                            payload: CommittedProducerStreamEventPayload::PackedU8(
                                packed_bytes[index],
                            ),
                        });
                    }
                    bytes = bytes.saturating_add(event_bound.saturating_mul(count));
                    expected_sequence = expected_sequence.saturating_add(count as u64);
                }
            }
            if result.last().is_some_and(|event| event.is_terminal())
                || result.len() >= STREAM_SEGMENT_MAX_EVENTS
                || bytes >= STREAM_SEGMENT_TARGET_BYTES
                || result
                    .last()
                    .is_some_and(|event| through == Some(event.offset))
            {
                break;
            }
        }
        if !found_start || result.is_empty() && through.is_none() {
            None
        } else {
            Some(result)
        }
    }

    pub(super) fn enqueue_events(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        events: Vec<CommittedProducerStreamEvent>,
        replayed: bool,
    ) -> Result<PublicationReceipt, StreamStoreError> {
        let bus = self.bus(stream_id)?;
        if !replayed {
            self.retain_committed_events(&events);
            bus.notify_committed();
        }
        if events.len() == 1 && events[0].is_terminal() {
            let event = events.into_iter().next().unwrap();
            let receipt =
                bus.defer_terminal(event.offset, replayed, self.terminal_progress.clone())?;
            bus.try_publish_deferred_terminal(self.queued_terminal(stream_id, event.offset))?;
            self.start_terminal_dispatcher();
            return Ok(receipt);
        }
        Ok(bus.enqueue_batch(
            events
                .into_iter()
                .map(|event| DurableLiveStreamEvent {
                    offset: event.offset,
                    payload: event,
                })
                .collect(),
            replayed,
            context.map(|context| {
                context.assert_owner(self);
                context.publication_keepalive()
            }),
        ))
    }

    fn queued_terminal(
        &self,
        stream_id: StreamId,
        offset: StreamOffset,
    ) -> QueuedDurableEvent<CommittedProducerStreamEvent> {
        let oplog = self.oplog.clone();
        let lineage = self.fork_lineage.clone();
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let owner_fingerprint = self.producer_fingerprint;
        QueuedDurableEvent::Terminal {
            offset,
            load: Arc::new(move |offset| {
                let oplog = oplog.clone();
                let lineage = lineage.clone();
                let owner = owner.clone();
                Box::pin(async move {
                    metadata::read_terminal_event(
                        oplog.as_ref(),
                        &lineage,
                        &owner,
                        owner_fingerprint,
                        stream_id,
                        offset,
                    )
                    .await
                    .map_err(|_| DurableLiveStreamBusError::PublicationAborted)
                })
            }),
        }
    }

    fn start_terminal_dispatcher(&self) {
        if self
            .terminal_dispatcher_started
            .swap(true, Ordering::AcqRel)
        {
            return;
        }
        let producer = self.self_weak.clone();
        let progress = self.terminal_progress.clone();
        tokio::spawn(async move {
            loop {
                let changed = progress.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let mut cursor = None;
                let mut pending = false;
                loop {
                    let Some(producer) = producer.upgrade() else {
                        return;
                    };
                    let page: Vec<_> = producer
                        .buses
                        .read()
                        .expect("durable stream bus map lock poisoned")
                        .range((
                            cursor.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded),
                            std::ops::Bound::Unbounded,
                        ))
                        .take(32)
                        .map(|(id, bus)| (*id, bus.clone()))
                        .collect();
                    let last_page = page.len() < 32;
                    for (stream_id, bus) in page {
                        cursor = Some(stream_id);
                        if !bus.has_pending_deferred_terminal() {
                            continue;
                        }
                        let result = async {
                            producer.ensure_healthy()?;
                            if let Some(offset) = bus.ready_deferred_terminal()? {
                                bus.try_publish_deferred_terminal(
                                    producer.queued_terminal(stream_id, offset),
                                )?;
                            }
                            Ok::<(), StreamStoreError>(())
                        }
                        .await;
                        if result.is_err() {
                            producer.poison();
                            bus.fail_deferred_terminal(
                                DurableLiveStreamBusError::PublicationAborted,
                            );
                        }
                        pending |= bus.has_pending_deferred_terminal();
                    }
                    if last_page {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                if pending {
                    // A transient bus-state lock need not generate a reader notification.
                    tokio::select! {
                        _ = &mut changed => {},
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
                    }
                } else {
                    changed.await;
                }
            }
        });
    }

    pub(super) async fn wait_for_publication(
        &self,
        context: Option<&StreamWriteContext>,
        publication: PublicationReceipt,
    ) -> Result<(), StreamStoreError> {
        if let Some(context) = context {
            context.assert_owner(self);
            context.defer_publication(publication);
        } else {
            publication
                .await
                .map_err(|_| DurableLiveStreamBusError::PublicationAborted)??;
        }
        Ok(())
    }

    pub(super) fn bus(
        &self,
        stream_id: StreamId,
    ) -> Result<Arc<DurableLiveStreamBus<CommittedProducerStreamEvent>>, StreamStoreError> {
        self.buses
            .read()
            .expect("durable stream bus map lock poisoned")
            .get(&stream_id)
            .cloned()
            .ok_or(StreamStoreError::UnknownStream(stream_id))
    }

    pub(super) async fn stream_bus(
        &self,
        stream: StreamId,
    ) -> Result<Arc<DurableLiveStreamBus<CommittedProducerStreamEvent>>, StreamStoreError> {
        let _index = self
            .index_for([ProducerMetadataKey::Stream(stream)])
            .await?;
        self.bus(stream)
    }

    pub(super) async fn publish_repair(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        events: Vec<CommittedProducerStreamEvent>,
    ) -> Result<(), StreamStoreError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let publication = self.enqueue_events(context, stream_id, events, true)?;
        drop(index);
        self.wait_for_publication(context, publication).await?;
        Ok(())
    }
}
