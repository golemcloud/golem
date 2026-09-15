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

use super::index::{attachment_slot, validate_attachment_epoch, validate_version};
use super::publication::{
    STREAM_SEGMENT_MAX_EVENTS, STREAM_SEGMENT_TARGET_BYTES, encoded_event_bytes,
};
use super::*;

impl DurableStreamStore {
    #[tracing::instrument(
        name = "durable_stream.catch_up",
        skip_all,
        fields(stream_id = %handle.stream_id, has_cursor = after.is_some())
    )]
    /// Creates a reader that catches up from committed oplog history before joining live events.
    pub(crate) async fn catch_up(
        self: &Arc<Self>,
        handle: DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<DurableCatchUpReader, StreamStoreError> {
        self.validate_handle(&handle).await?;
        let bus = self.stream_bus(handle.stream_id).await?;
        self.validate_cursor(handle.stream_id, after).await?;
        let subscription = bus.subscribe().await?;
        let history = match subscription.high_water {
            Some(high_water) => match self.read_segment(&handle, after, Some(high_water)).await {
                Ok(history) => history,
                Err(error) => {
                    bus.unsubscribe(subscription.reader_id()).await;
                    crate::metrics::durable_stream::record_live_join_rejected();
                    return Err(error);
                }
            },
            None => Vec::new(),
        };
        let join_high_water = subscription.high_water;
        crate::metrics::durable_stream::record_catch_up(history.len());
        tracing::debug!(
            stream_id = %handle.stream_id,
            catch_up_events = history.len(),
            has_cursor = after.is_some(),
            has_join_high_water = join_high_water.is_some(),
            "Durable stream reader joined committed history to live tail"
        );
        Ok(DurableCatchUpReader {
            bus,
            subscription: Some(subscription),
            history_source: join_high_water.map(|_| (self.clone(), handle)),
            history: history.into(),
            join_high_water,
            last_delivered: after,
            terminal_delivered: false,
        })
    }

    /// Verifies that a handle names this producer, fingerprint, stream, and schema.
    pub(crate) async fn validate_handle(
        &self,
        handle: &DurableStreamHandle,
    ) -> Result<(), StreamStoreError> {
        validate_version(handle.format_version)?;
        let index = self
            .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
            .await?;
        if index
            .registrations
            .get(&handle.stream_id)
            .is_some_and(|record| &record.handle == handle)
        {
            Ok(())
        } else {
            Err(StreamStoreError::InvalidHandle)
        }
    }

    /// Returns whether the handle is bound to this exact durable producer identity.
    pub(crate) fn owns_handle_identity(&self, handle: &DurableStreamHandle) -> bool {
        handle.producer_environment_id == self.environment_id
            && handle.producer == self.producer
            && handle.expected_producer_fingerprint == self.producer_fingerprint
    }

    pub(super) async fn validate_cursor(
        &self,
        stream_id: StreamId,
        after: Option<StreamOffset>,
    ) -> Result<(), StreamStoreError> {
        let Some(after) = after else {
            return Ok(());
        };
        let index = self
            .index_for([
                ProducerMetadataKey::Stream(stream_id),
                ProducerMetadataKey::Position(stream_id, after.producer_oplog_index()),
            ])
            .await?;
        let high_water = index
            .streams
            .get(&stream_id)
            .and_then(|stream| stream.last_offset);
        if high_water.is_none_or(|high_water| after > high_water)
            || !after.producer_oplog_index().is_defined()
        {
            return Err(StreamStoreError::CursorUnavailable);
        }
        if index.streams[&stream_id].terminal && high_water == Some(after) {
            return Ok(());
        }
        let valid = index
            .batch_positions
            .get(&(stream_id, after.producer_oplog_index()))
            .is_some_and(|(_, count)| u64::from(after.sub_index()) < *count);
        if valid {
            Ok(())
        } else {
            Err(StreamStoreError::CursorUnavailable)
        }
    }
}

#[async_trait]
impl StreamSegmentSource for DurableStreamStore {
    #[tracing::instrument(name = "durable_stream.read_segment", level = "debug", skip_all)]
    async fn read_segment(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        self.validate_handle(handle).await?;
        for offset in [after, through].into_iter().flatten() {
            StreamOffset::from_bytes(*offset.as_bytes())
                .map_err(|error| StreamStoreError::InvalidOffset(error.to_string()))?;
            self.validate_cursor(handle.stream_id, Some(offset)).await?;
        }
        if after
            .zip(through)
            .is_some_and(|(after, through)| after > through)
        {
            return Err(StreamStoreError::InvalidOffset(
                "catch-up end precedes its cursor".into(),
            ));
        }
        if after.is_some() && after == through {
            return Ok(Vec::new());
        }
        if let Some(events) = self.retained_segment(handle.stream_id, after, through) {
            return Ok(events);
        }
        let (mut sequence, sequence_limit, terminal) = {
            let index = self
                .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
                .await?;
            let stream = &index.streams[&handle.stream_id];
            (
                stream.first_sequence.unwrap_or(0),
                stream.next_sequence,
                stream.terminal,
            )
        };
        let mut enclosing_batch = None;
        if let Some(after) = after {
            let index = self
                .index_for([
                    ProducerMetadataKey::Stream(handle.stream_id),
                    ProducerMetadataKey::Position(handle.stream_id, after.producer_oplog_index()),
                ])
                .await?;
            let stream = &index.streams[&handle.stream_id];
            if stream.terminal && stream.last_offset == Some(after) {
                return Ok(Vec::new());
            }
            let (first, count) = index
                .batch_positions
                .get(&(handle.stream_id, after.producer_oplog_index()))
                .copied()
                .ok_or(StreamStoreError::CursorUnavailable)?;
            let cursor_sequence = first
                .checked_add(u64::from(after.sub_index()))
                .ok_or(StreamStoreError::CounterOverflow)?;
            if u64::from(after.sub_index()) >= count {
                return Err(StreamStoreError::CursorUnavailable);
            }
            let batch_end = first
                .checked_add(count)
                .ok_or(StreamStoreError::CounterOverflow)?;
            sequence = cursor_sequence
                .checked_add(1)
                .ok_or(StreamStoreError::CounterOverflow)?;
            enclosing_batch =
                (sequence < batch_end).then_some((after.producer_oplog_index(), first));
        }
        let mut result = Vec::new();
        let mut result_bytes = 0usize;
        let mut locators = BTreeMap::new();
        loop {
            if result.len() >= STREAM_SEGMENT_MAX_EVENTS {
                return Ok(result);
            }
            if sequence >= sequence_limit && !terminal {
                return Ok(result);
            }
            if sequence >= sequence_limit {
                let index = self.index_for_terminal([], handle.stream_id).await?;
                if let Some(event) = index.streams[&handle.stream_id].terminal_event.clone()
                    && after.is_none_or(|after| event.offset > after)
                    && through.is_none_or(|through| event.offset <= through)
                    && result.len() < STREAM_SEGMENT_MAX_EVENTS
                    && (result.is_empty()
                        || result_bytes.saturating_add(encoded_event_bytes(&event))
                            <= STREAM_SEGMENT_TARGET_BYTES)
                {
                    result.push(event);
                }
                return Ok(result);
            }
            let (oplog_index, batch_first) = if let Some(enclosing) = enclosing_batch.take() {
                enclosing
            } else {
                if !locators.contains_key(&sequence) {
                    let locator_frontier =
                        125usize.min(sequence_limit.saturating_sub(sequence) as usize);
                    if self.control_metadata_provider.get().is_some() {
                        let mut index = self
                            .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
                            .await?;
                        // Replace the locator window without evicting its stream/session
                        // dependencies and loading the same window twice.
                        if !index.complete_for_deletion {
                            index.loaded_metadata.retain(|key| match key {
                                ProducerMetadataKey::Batch(stream, _)
                                | ProducerMetadataKey::Position(stream, _) => {
                                    *stream != handle.stream_id
                                }
                                _ => true,
                            });
                            index
                                .streams
                                .get_mut(&handle.stream_id)
                                .unwrap()
                                .batches
                                .clear();
                            index
                                .batch_positions
                                .retain(|(stream, _), _| *stream != handle.stream_id);
                        }
                    }
                    let mut metadata_keys = Vec::with_capacity(locator_frontier + 1);
                    metadata_keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                    metadata_keys.extend((0..locator_frontier).filter_map(|distance| {
                        sequence.checked_add(distance as u64).map(|candidate| {
                            ProducerMetadataKey::Batch(handle.stream_id, candidate)
                        })
                    }));
                    let index = self.index_for(metadata_keys).await?;
                    let stream = index
                        .streams
                        .get(&handle.stream_id)
                        .ok_or(StreamStoreError::UnknownStream(handle.stream_id))?;
                    locators.extend(
                        stream
                            .batches
                            .range(sequence..sequence.saturating_add(locator_frontier as u64))
                            .map(|(&first, &oplog_index)| (first, oplog_index)),
                    );
                }
                let oplog_index = locators.remove(&sequence).ok_or_else(|| {
                    StreamStoreError::CorruptHistory(
                        "stream metadata is missing a batch locator".into(),
                    )
                })?;
                (oplog_index, sequence)
            };
            let record = self.read_item_batch(oplog_index).await?;
            if record.stream_id != handle.stream_id || record.first_sequence != batch_first {
                return Err(StreamStoreError::CorruptHistory(
                    "stream batch locator identifies a different batch".into(),
                ));
            }
            let events = self
                .materialize_item_batch_range(
                    &record,
                    sequence,
                    STREAM_SEGMENT_MAX_EVENTS.saturating_sub(result.len()),
                )
                .await?;
            if sequence < record.first_sequence
                || sequence
                    >= record
                        .first_sequence
                        .saturating_add(record.offsets.len() as u64)
            {
                return Err(StreamStoreError::CorruptHistory(
                    "stream batch does not contain the requested sequence".into(),
                ));
            }
            let mut advanced = false;
            for event in events {
                if event.producer_sequence != sequence {
                    return Err(StreamStoreError::CorruptHistory(
                        "stream batch event sequence is not contiguous".into(),
                    ));
                }
                if through.is_some_and(|through| event.offset > through) {
                    return Ok(result);
                }
                let bytes = encoded_event_bytes(&event);
                if !result.is_empty()
                    && (result.len() >= STREAM_SEGMENT_MAX_EVENTS
                        || result_bytes.saturating_add(bytes) > STREAM_SEGMENT_TARGET_BYTES)
                {
                    return Ok(result);
                }
                sequence = event.producer_sequence + 1;
                advanced = true;
                result_bytes = result_bytes.saturating_add(bytes);
                result.push(event);
                if result
                    .last()
                    .is_some_and(|event| Some(event.offset) == through)
                {
                    return Ok(result);
                }
            }
            if !advanced {
                return Err(StreamStoreError::CorruptHistory(
                    "stream batch locator did not advance the requested sequence".into(),
                ));
            }
        }
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for DurableStreamStore {
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, StreamStoreError> {
        DurableStreamStore::journal_lag_events(self, handle, after).await
    }

    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        now_millis: u64,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        if attachment.stream_id != handle.stream_id {
            return Err(StreamStoreError::InvalidHandle);
        }
        let index = self
            .index_for([ProducerMetadataKey::Attachment(
                attachment.attachment_id,
                attachment.stream_id,
                attachment.consumer_environment_id,
                attachment.consumer.clone(),
            )])
            .await?;
        index.validate_attachment_key(
            attachment,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        let indexed_attachment = index
            .attachments
            .get(&attachment_slot(attachment))
            .ok_or(StreamStoreError::InvalidAttachmentState)?;
        validate_attachment_epoch(indexed_attachment, attachment)?;
        if indexed_attachment.key != *attachment {
            return Err(StreamStoreError::AttachmentConflict);
        }
        match indexed_attachment.state {
            IndexedStreamAttachmentState::Active {
                lease_expires_at_millis,
                ..
            } if now_millis < lease_expires_at_millis => {}
            IndexedStreamAttachmentState::Active { .. } => {
                return Err(StreamStoreError::LeaseExpired);
            }
            _ => return Err(StreamStoreError::InvalidAttachmentState),
        }
        drop(index);
        self.read_segment(handle, after, through).await
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        now_millis: u64,
        after: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        let started = std::time::Instant::now();
        let events = self
            .read_attached_segment(attachment, handle, now_millis, after, None)
            .await?;
        if !events.is_empty() {
            return Ok(events);
        }
        let bus = self.stream_bus(handle.stream_id).await?;
        let subscription = bus.subscribe().await?;
        let mut reader = DurableCatchUpReader {
            bus,
            subscription: Some(subscription),
            history_source: None,
            history: VecDeque::new(),
            join_high_water: None,
            last_delivered: after,
            terminal_delivered: false,
        };
        let events = self
            .read_attached_segment(attachment, handle, now_millis, after, None)
            .await?;
        if !events.is_empty() {
            return Ok(events);
        }
        if let Ok(event) =
            tokio::time::timeout(std::time::Duration::from_secs(1), reader.next()).await
        {
            event?;
        }
        // The attachment may have been renewed, finalized, or replaced while the long poll was
        // asleep. Re-read it authoritatively and drain the full available segment rather than
        // returning only the bus event that happened to wake this waiter.
        let now_millis = now_millis.saturating_add(started.elapsed().as_millis() as u64);
        self.read_attached_segment(attachment, handle, now_millis, after, None)
            .await
    }
}

/// Resumable reader that deduplicates committed history against the subscribed live tail.
pub(crate) struct DurableCatchUpReader {
    pub(super) bus: Arc<DurableLiveStreamBus<CommittedProducerStreamEvent>>,
    pub(super) subscription: Option<DurableLiveStreamSubscription<CommittedProducerStreamEvent>>,
    pub(super) history_source: Option<(Arc<DurableStreamStore>, DurableStreamHandle)>,
    pub(super) history: VecDeque<CommittedProducerStreamEvent>,
    pub(super) join_high_water: Option<StreamOffset>,
    pub(super) last_delivered: Option<StreamOffset>,
    pub(super) terminal_delivered: bool,
}

impl DurableCatchUpReader {
    /// Returns the next ordered event, advancing by durable offset rather than connection state.
    pub(crate) async fn next(
        &mut self,
    ) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        if self.terminal_delivered {
            return Ok(None);
        }
        self.bus.ensure_available()?;
        if self.history.is_empty() {
            if self.last_delivered >= self.join_high_water {
                self.history_source = None;
            }
            if let Some((source, handle)) = &self.history_source {
                self.history = source
                    .read_segment(handle, self.last_delivered, self.join_high_water)
                    .await?
                    .into();
                if self.history.is_empty() {
                    return Err(StreamStoreError::CorruptHistory(
                        "stream replay ended before its captured high-water mark".into(),
                    ));
                }
            }
        }
        self.bus.ensure_available()?;
        if let Some(event) = self.history.pop_front() {
            let event = self.deliver(event)?;
            self.release_subscription_if_complete();
            return Ok(Some(event));
        }
        loop {
            let Some(subscription) = self.subscription.as_mut() else {
                return Err(DurableLiveStreamBusError::PublicationAborted.into());
            };
            let event = subscription.recv().await?;
            if self
                .join_high_water
                .is_some_and(|high_water| event.offset <= high_water)
                || self
                    .last_delivered
                    .is_some_and(|last_delivered| event.offset <= last_delivered)
            {
                continue;
            }
            let event = self.deliver(event.payload)?;
            self.release_subscription_if_complete();
            return Ok(Some(event));
        }
    }

    fn release_subscription_if_complete(&mut self) {
        if self.terminal_delivered {
            self.history_source = None;
            self.release_subscription_in_background();
        }
    }

    fn release_subscription_in_background(&mut self) {
        let Some(subscription) = self.subscription.take() else {
            return;
        };
        let reader_id = subscription.reader_id();
        drop(subscription);
        let bus = self.bus.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                bus.unsubscribe(reader_id).await;
            });
        }
    }

    fn deliver(
        &mut self,
        event: CommittedProducerStreamEvent,
    ) -> Result<CommittedProducerStreamEvent, StreamStoreError> {
        if self
            .last_delivered
            .is_some_and(|last_delivered| event.offset <= last_delivered)
        {
            return Err(StreamStoreError::CorruptHistory(
                "catch-up reader observed a non-increasing offset".to_string(),
            ));
        }
        self.last_delivered = Some(event.offset);
        self.terminal_delivered = event.is_terminal();
        Ok(event)
    }
}

impl Drop for DurableCatchUpReader {
    fn drop(&mut self) {
        self.release_subscription_in_background();
    }
}
