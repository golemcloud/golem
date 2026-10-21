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

#[cfg(test)]
use super::AttachmentRead;
use super::{
    AcceptedAttachmentWriteEnd, Arc, AtomicBool, AttachmentCharge, AttachmentConsumer,
    AttachmentController, AttachmentMode, AttachmentObserver, AttachmentProducer,
    AttachmentReservation, AttachmentStreamProducer, AttachmentTerminalPublication,
    AttachmentTerminalSnapshot, AttachmentWriteBody, AttachmentWriter, BufferedChunk,
    ByteAttachment, ByteStreamCloseCause, ByteStreamFailure, Bytes, GuestAttachmentWatcher,
    MemoryGrant, Ordering, PreparedAttachmentTerminal, PreparedAttachmentWrite,
    RawAttachmentStreamProducer, StagedAttachmentWrite, StreamWriteError, ToolAttachmentId,
    ToolAttachmentMetadata, WeakAttachmentController, close_cause_eq,
};
use crate::durable_host::tool::durability;

impl AttachmentProducer {
    pub(in crate::durable_host::tool) fn retire(mut self) {
        self.abandon_on_drop = false;
    }

    /// Reserve at guest initiation, before durable admission can reorder concurrent calls.
    pub(crate) fn next_operation_ordinal(&mut self) -> u64 {
        self.writer().next_operation_ordinal()
    }

    pub(crate) fn controller(&self) -> AttachmentController {
        AttachmentController {
            attachment: self.attachment.clone(),
        }
    }

    pub(crate) fn writer(&self) -> AttachmentWriter {
        AttachmentWriter {
            attachment: self.attachment.clone(),
            next_operation_ordinal: self.next_operation_ordinal.clone(),
        }
    }

    pub(crate) async fn write(&self, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        self.attachment.write(bytes, None).await
    }

    pub(crate) fn finish(&self) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Finished)
    }

    pub(crate) fn fail(&self, failure: ByteStreamFailure) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(
                ByteAttachment::sanitize_failure(failure),
            ))
    }

    #[cfg(test)]
    pub(crate) fn cancel(&self) -> Result<(), StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled))
    }

    pub(crate) fn abandon_unconfigured(&self) -> Result<bool, StreamWriteError> {
        self.attachment
            .terminate_unconfigured(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned))
    }

    pub(crate) fn reject_unconfigured(&self) -> Result<bool, StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Failed(
                "tool invocation rejected".to_string(),
            )))?;
        Ok(self.attachment.publish_no_body_terminal())
    }

    #[cfg(test)]
    pub(crate) fn configure_live(&self) -> bool {
        self.attachment.configure(AttachmentMode::Live)
    }

    #[cfg(test)]
    pub(crate) fn configure_completion(&self) -> bool {
        self.attachment
            .configure(AttachmentMode::Completion { published: false })
    }

    #[cfg(test)]
    pub(crate) fn publish_completion(&self) -> bool {
        self.attachment.publish_completion()
    }

    #[cfg(test)]
    pub(crate) fn observer(&self) -> AttachmentObserver {
        AttachmentObserver {
            attachment: self.attachment.clone(),
            guest_watcher: None,
        }
    }
}

impl AttachmentWriter {
    pub(crate) fn next_operation_ordinal(&self) -> u64 {
        self.next_operation_ordinal
            .try_update(Ordering::AcqRel, Ordering::Acquire, |ordinal| {
                ordinal.checked_add(1)
            })
            .expect("attachment operation overflow")
    }

    pub(crate) fn controller(&self) -> AttachmentController {
        AttachmentController {
            attachment: self.attachment.clone(),
        }
    }

    pub(crate) fn identity(&self) -> Option<ToolAttachmentId> {
        self.attachment.state.lock().unwrap().identity
    }

    pub(crate) fn effective_terminal(failure: Option<ByteStreamFailure>) -> ByteStreamCloseCause {
        match failure {
            None => ByteStreamCloseCause::Finished,
            Some(failure) => {
                ByteStreamCloseCause::Failed(ByteAttachment::sanitize_failure(failure))
            }
        }
    }

    pub(crate) fn prepare_terminal(
        &self,
        cause: ByteStreamCloseCause,
    ) -> Result<Result<PreparedAttachmentTerminal, ()>, StreamWriteError> {
        let operation = self.attachment.begin_producer_operation()?;
        let state = self.attachment.state.lock().unwrap();
        if let Some(selected) = &state.terminal {
            return if close_cause_eq(selected, &cause) {
                Ok(Err(()))
            } else {
                Err(StreamWriteError::Closed(selected.clone()))
            };
        }
        drop(state);
        Ok(Ok(PreparedAttachmentTerminal {
            operation,
            cause,
            host_resource_exhausted: false,
        }))
    }

    pub(crate) fn set_identity(&self, identity: ToolAttachmentId) {
        let mut state = self.attachment.state.lock().unwrap();
        assert!(state.identity.is_none());
        state.identity = Some(identity);
    }

    pub(crate) async fn prepare_write(
        &self,
        bytes: Vec<u8>,
    ) -> Result<Option<PreparedAttachmentWrite>, StreamWriteError> {
        self.attachment.prepare_write(bytes).await
    }

    pub(crate) async fn write(&self, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        self.attachment.write(bytes, None).await
    }

    /// Owns regenerated storage before a historical call can await its Start claim.
    /// Only crossing its accepted End permits publication; indexing it grants no queue credit.
    pub(in crate::durable_host::tool) fn stage_historical_write(
        &self,
        bytes: AttachmentWriteBody,
        source_end: AcceptedAttachmentWriteEnd,
    ) -> anyhow::Result<StagedAttachmentWrite> {
        let attachment = &self.attachment;
        let mut state = attachment.state.lock().unwrap();
        anyhow::ensure!(!state.owner_fenced, "historical attachment owner is fenced");
        let published = Arc::new(AtomicBool::new(false));
        let length = bytes.len();
        if state.observed_writes.contains(&source_end.0) || !state.consumer_active {
            return Ok(StagedAttachmentWrite {
                attachment: attachment.clone(),
                published,
                length,
            });
        }
        let (bytes, charge) = match bytes {
            AttachmentWriteBody::Inline(bytes) => {
                let bytes = bytes.into_boxed_slice().into_vec();
                anyhow::ensure!(
                    attachment
                        .reconstruction_bytes
                        .load(Ordering::Acquire)
                        .saturating_add(bytes.capacity() as u64)
                        <= attachment.limit.saturating_mul(2) as u64,
                    "historical attachment exceeds its queue and offered-item allocation bound"
                );
                anyhow::ensure!(
                    !attachment.memory.tracking_enabled(),
                    "historical attachment reconstruction started after live activation"
                );
                let reservation = attachment
                    .memory
                    .historical_charges
                    .reserve(bytes.capacity() as u64)
                    .ok_or_else(|| {
                        anyhow::anyhow!("historical attachment activation is pending")
                    })?;
                let mut charge = AttachmentCharge::new(
                    AttachmentReservation {
                        grant: MemoryGrant::inert(bytes.capacity() as u64),
                        historical_reservation: Some(reservation),
                    },
                    attachment.charged_bytes.clone(),
                );
                attachment
                    .reconstruction_bytes
                    .fetch_add(bytes.capacity() as u64, Ordering::AcqRel);
                charge.reconstruction_bytes = Some(attachment.reconstruction_bytes.clone());
                (AttachmentWriteBody::Inline(bytes), Some(charge))
            }
            recorded @ AttachmentWriteBody::Recorded { .. } => (recorded, None),
        };
        let position = state
            .chunks
            .iter()
            .position(|chunk| chunk.source_end.is_some_and(|end| end.0 > source_end.0))
            .unwrap_or(state.chunks.len());
        state.buffered_bytes += bytes.len();
        state.chunks.insert(
            position,
            BufferedChunk {
                bytes,
                charge,
                published: published.clone(),
                source_end: Some(source_end),
            },
        );
        Ok(StagedAttachmentWrite {
            attachment: attachment.clone(),
            published,
            length,
        })
    }

    pub(crate) fn finish(&self) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Finished)
    }

    pub(crate) fn fail(&self, failure: ByteStreamFailure) -> Result<(), StreamWriteError> {
        let _operation = self.attachment.begin_producer_operation()?;
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(
                ByteAttachment::sanitize_failure(failure),
            ))
    }
}

impl Drop for AttachmentProducer {
    fn drop(&mut self) {
        if self.abandon_on_drop {
            self.attachment.producer_abandon();
        } else {
            self.attachment.state.lock().unwrap().producer_active = false;
            self.attachment.changed.notify_waiters();
        }
    }
}

impl AttachmentConsumer {
    pub(crate) fn controller(&self) -> AttachmentController {
        AttachmentController {
            attachment: self.attachment.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn configure_live(&self) -> bool {
        self.attachment.configure(AttachmentMode::Live)
    }

    #[cfg(test)]
    pub(crate) fn configure_completion(&self) -> bool {
        self.attachment
            .configure(AttachmentMode::Completion { published: false })
    }

    pub(crate) fn into_stream_producer(self) -> AttachmentStreamProducer {
        AttachmentStreamProducer {
            consumer: Some(self),
            in_flight_charge: None,
            pending_chunk: None,
            finished: false,
        }
    }

    pub(crate) fn into_raw_stream_producer(self) -> RawAttachmentStreamProducer {
        RawAttachmentStreamProducer {
            source: self.into_stream_producer(),
            remainder: Bytes::new(),
            finished: false,
        }
    }

    #[cfg(test)]
    pub(crate) async fn read_next(&self) -> AttachmentRead {
        loop {
            let notified = self.attachment.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mut pending_chunk = None;
            let read = {
                let mut state = self.attachment.state.lock().unwrap();
                let chunks_visible = matches!(state.mode, AttachmentMode::Live)
                    || matches!(state.mode, AttachmentMode::Completion { published: true });
                let terminal_visible = !state.terminal_pending
                    && (chunks_visible || matches!(state.mode, AttachmentMode::TerminalOnly));
                if state
                    .chunks
                    .front()
                    .is_some_and(|chunk| !chunk.published.load(Ordering::Acquire))
                {
                    None
                } else if chunks_visible && !state.chunks.is_empty() {
                    let chunk = state.chunks.pop_front().unwrap();
                    state.buffered_bytes -= chunk.bytes.len();
                    state.delivered_bytes = state
                        .delivered_bytes
                        .saturating_add(chunk.bytes.len() as u64);
                    pending_chunk = Some(chunk);
                    None
                } else if !terminal_visible {
                    None
                } else {
                    match state.terminal.clone() {
                        Some(ByteStreamCloseCause::Finished) => Some(AttachmentRead::End),
                        Some(ByteStreamCloseCause::Failed(failure))
                            if !state.terminal_item_delivered =>
                        {
                            state.terminal_item_delivered = true;
                            Some(AttachmentRead::Item(Err(failure)))
                        }
                        Some(ByteStreamCloseCause::Failed(_))
                        | Some(ByteStreamCloseCause::ConsumerCancelled) => {
                            Some(AttachmentRead::End)
                        }
                        None => None,
                    }
                }
            };
            if let Some(chunk) = pending_chunk {
                self.attachment.changed.notify_waiters();
                let _charge = chunk.charge;
                return AttachmentRead::Item(Ok(chunk
                    .bytes
                    .into_bytes()
                    .await
                    .expect("test attachment body hydration failed")));
            }
            if let Some(read) = read {
                self.attachment.changed.notify_waiters();
                return read;
            }
            notified.as_mut().await;
        }
    }
}

impl Drop for AttachmentConsumer {
    fn drop(&mut self) {
        self.attachment.consumer_cancel();
    }
}

impl Drop for GuestAttachmentWatcher {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().guest_watchers -= 1;
    }
}

impl AttachmentObserver {
    pub(crate) fn into_guest_watcher(mut self) -> Self {
        self.attachment.state.lock().unwrap().guest_watchers += 1;
        self.guest_watcher = Some(Arc::new(GuestAttachmentWatcher(self.attachment.clone())));
        self
    }

    pub(crate) fn identity(&self) -> Option<ToolAttachmentId> {
        self.attachment.state.lock().unwrap().identity
    }

    pub(crate) async fn wait_terminal(&self) -> ByteStreamCloseCause {
        self.attachment.wait_terminal().await
    }

    #[cfg(test)]
    pub(crate) fn terminal(&self) -> Option<ByteStreamCloseCause> {
        let state = self.attachment.state.lock().unwrap();
        (!state.terminal_pending)
            .then(|| state.terminal.clone())
            .flatten()
    }

    pub(crate) fn terminal_snapshot(&self) -> Option<AttachmentTerminalSnapshot> {
        let state = self.attachment.state.lock().unwrap();
        if state.terminal_pending {
            return None;
        }
        state
            .terminal
            .clone()
            .map(|_cause| AttachmentTerminalSnapshot {
                #[cfg(test)]
                cause: _cause,
                host_resource_exhausted: state.host_resource_exhausted,
            })
    }
}

impl WeakAttachmentController {
    pub(crate) fn upgrade(&self) -> Option<AttachmentController> {
        self.0
            .upgrade()
            .map(|attachment| AttachmentController { attachment })
    }
}

impl AttachmentController {
    pub(crate) fn downgrade(&self) -> WeakAttachmentController {
        WeakAttachmentController(Arc::downgrade(&self.attachment))
    }

    pub(crate) fn has_open_endpoints(&self) -> bool {
        let state = self.attachment.state.lock().unwrap();
        state.producer_active || state.consumer_active || state.guest_watchers != 0
    }

    pub(crate) fn restore_control(
        &self,
        transition: &golem_common::model::oplog::attachment::ToolAttachmentControl,
        index: golem_common::model::OplogIndex,
        pending: bool,
    ) -> anyhow::Result<()> {
        use golem_common::model::oplog::attachment::{
            ToolAttachmentControl, ToolAttachmentTerminal,
        };
        match transition {
            ToolAttachmentControl::TerminalSelected(terminal) => {
                let (cause, host) = match terminal {
                    ToolAttachmentTerminal::Application(cause) => {
                        (durability::cause_from_model(cause.clone()), false)
                    }
                    ToolAttachmentTerminal::HostResourceExhausted => (
                        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
                        true,
                    ),
                };
                self.restore_terminal(cause, host, index, pending)
            }
            ToolAttachmentControl::ReaderDiscarded => {
                self.attachment.consumer_cancel();
                Ok(())
            }
        }
    }

    pub(crate) fn restore_terminal(
        &self,
        cause: ByteStreamCloseCause,
        host_resource_exhausted: bool,
        end: golem_common::model::OplogIndex,
        pending: bool,
    ) -> anyhow::Result<()> {
        let mut state = self.attachment.state.lock().unwrap();
        anyhow::ensure!(!state.owner_fenced, "historical attachment owner is fenced");
        if let Some(selected) = state.terminal_end {
            anyhow::ensure!(
                selected == end
                    && state
                        .terminal
                        .as_ref()
                        .is_some_and(|selected| close_cause_eq(selected, &cause))
                    && state.host_resource_exhausted == host_resource_exhausted,
                "recorded terminal conflicts with an existing durable selection"
            );
        }
        // A journal delivery may precede regeneration of the terminal's producer.
        state.terminal = Some(cause);
        state.terminal_pending = pending;
        state.owner_terminal_reserved = false;
        state.terminal_item_delivered = state.observed_terminal == Some(end);
        state.host_resource_exhausted = host_resource_exhausted;
        state.terminal_end = Some(end);
        let reader = state.reader_waker.take();
        drop(state);
        self.attachment.changed.notify_waiters();
        self.attachment.terminal_changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        Ok(())
    }

    pub(crate) fn accept_cancellation<R>(
        &self,
        enqueue: impl FnOnce(AttachmentTerminalPublication) -> R,
    ) -> Option<R> {
        self.accept_application_terminal(
            ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled),
            enqueue,
        )
    }

    pub(in crate::durable_host::tool) fn accept_application_terminal<R>(
        &self,
        cause: ByteStreamCloseCause,
        enqueue: impl FnOnce(AttachmentTerminalPublication) -> R,
    ) -> Option<R> {
        let mut state = self.attachment.state.lock().unwrap();
        if state.terminal.is_some() || state.owner_fenced {
            return None;
        }
        state.terminal = Some(cause);
        state.terminal_pending = true;
        Some(enqueue(AttachmentTerminalPublication {
            attachment: self.attachment.clone(),
            _operation: None,
        }))
    }

    pub(in crate::durable_host::tool) fn producer_disposal_owned_elsewhere(&self) -> bool {
        let state = self.attachment.state.lock().unwrap();
        state.owner_fenced
            || state.owner_terminal_reserved
            || (state.terminal_end.is_none()
                && !state.terminal_pending
                && matches!(
                    state.terminal,
                    Some(ByteStreamCloseCause::ConsumerCancelled)
                ))
    }

    /// Keeps producer destruction from selecting abandonment before the owner commits its End.
    pub(crate) fn reserve_owner_terminal(&self, failure: ByteStreamFailure) {
        let mut state = self.attachment.state.lock().unwrap();
        if state.identity.is_none() || state.owner_fenced || state.terminal.is_some() {
            return;
        }
        state.host_resource_exhausted = matches!(failure, ByteStreamFailure::ResourceExhausted);
        state.terminal = Some(ByteStreamCloseCause::Failed(failure));
        state.terminal_pending = true;
        state.owner_terminal_reserved = true;
    }

    /// Accepts an owner reservation without replacing an earlier producer selection.
    pub(crate) fn accept_owner_terminal<R>(
        &self,
        failure: ByteStreamFailure,
        enqueue: impl FnOnce(
            golem_common::model::oplog::attachment::ToolAttachmentTerminal,
            AttachmentTerminalPublication,
        ) -> R,
    ) -> Option<R> {
        use golem_common::model::oplog::attachment::ToolAttachmentTerminal;

        let mut state = self.attachment.state.lock().unwrap();
        if state.owner_fenced
            || state.terminal_end.is_some()
            || (state.terminal_pending && !state.owner_terminal_reserved)
        {
            return None;
        }
        state.owner_terminal_reserved = false;
        if state.terminal.is_none() {
            state.host_resource_exhausted = matches!(failure, ByteStreamFailure::ResourceExhausted);
            state.terminal = Some(ByteStreamCloseCause::Failed(failure));
        }
        let terminal = if state.host_resource_exhausted {
            ToolAttachmentTerminal::HostResourceExhausted
        } else {
            ToolAttachmentTerminal::Application(durability::cause_to_model(
                state.terminal.as_ref().expect("owner selected a terminal"),
            ))
        };
        state.terminal_pending = true;
        Some(enqueue(
            terminal,
            AttachmentTerminalPublication {
                attachment: self.attachment.clone(),
                _operation: None,
            },
        ))
    }

    pub(crate) async fn await_terminal_publication(
        &self,
        expected: Option<golem_common::model::OplogIndex>,
    ) -> anyhow::Result<()> {
        loop {
            let changed = self.attachment.terminal_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.attachment.state.lock().unwrap();
                anyhow::ensure!(
                    !state.owner_fenced,
                    "attachment owner was fenced during terminal publication"
                );
                if !state.terminal_pending
                    && let Some(end) = state.terminal_end
                {
                    anyhow::ensure!(
                        expected.is_none_or(|expected| expected == end),
                        "attachment restored a different durable terminal"
                    );
                    return Ok(());
                }
            }
            changed.await;
        }
    }

    pub(in crate::durable_host::tool) fn observe_terminal(
        &self,
        end: golem_common::model::OplogIndex,
    ) {
        let mut state = self.attachment.state.lock().unwrap();
        state.observed_terminal = Some(end);
        if state.terminal_end == Some(end) {
            state.terminal_item_delivered = true;
        }
    }

    pub(in crate::durable_host::tool) fn observe_write(
        &self,
        end: AcceptedAttachmentWriteEnd,
        length: usize,
    ) {
        let mut state = self.attachment.state.lock().unwrap();
        if state.observed_writes.insert(end.0) {
            state.delivered_bytes = state.delivered_bytes.saturating_add(length as u64);
        }
        if let Some(position) = state
            .chunks
            .iter()
            .position(|chunk| chunk.source_end == Some(end))
        {
            let chunk = state.chunks.remove(position).unwrap();
            state.buffered_bytes -= chunk.bytes.len();
        }
        drop(state);
        self.attachment.changed.notify_waiters();
    }

    pub(crate) async fn prepare_live_memory_accounting(&self) -> bool {
        self.attachment.prepare_live_memory_accounting().await
    }

    pub(crate) fn commit_live_memory_accounting(&self) {
        self.attachment.commit_live_memory_accounting();
    }

    pub(crate) fn complete_rejected_live_memory_accounting(&self) {
        self.attachment.complete_rejected_live_memory_accounting();
    }

    pub(crate) fn abort_prepared_live_memory_accounting(&self) {
        self.attachment.memory.abort_tracking();
        self.attachment.changed.notify_waiters();
    }

    pub(crate) fn configure_live(&self) -> bool {
        self.attachment.configure(AttachmentMode::Live)
    }

    pub(crate) fn configure_completion(&self) -> bool {
        self.attachment
            .configure(AttachmentMode::Completion { published: false })
    }

    pub(crate) fn publish_completion(&self) -> bool {
        self.attachment.publish_completion()
    }

    pub(crate) fn publish_no_body_terminal(&self) -> bool {
        self.attachment.publish_no_body_terminal()
    }

    pub(in crate::durable_host::tool) fn prepare_no_body_terminal(&self) {
        self.attachment.memory.abort_tracking();
        let reader = {
            let mut state = self.attachment.state.lock().unwrap();
            if state.owner_fenced {
                return;
            }
            if matches!(state.mode, AttachmentMode::Pending) {
                state.chunks.clear();
                state.buffered_bytes = 0;
                state.mode = AttachmentMode::TerminalOnly;
            }
            state.reader_waker.take()
        };
        self.attachment.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    pub(crate) fn cancel(&self) -> Result<(), StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled))
    }

    pub(crate) fn fence_owner(&self) {
        self.attachment.fence_owner();
    }

    pub(crate) fn host_fail(&self, failure: ByteStreamFailure) -> Result<(), StreamWriteError> {
        self.attachment
            .select_terminal(ByteStreamCloseCause::Failed(
                ByteAttachment::sanitize_failure(failure),
            ))
    }

    pub(crate) fn observer(&self) -> AttachmentObserver {
        AttachmentObserver {
            attachment: self.attachment.clone(),
            guest_watcher: None,
        }
    }

    pub(crate) fn metadata(&self) -> ToolAttachmentMetadata {
        self.attachment.metadata()
    }

    #[cfg(test)]
    pub(crate) fn live_memory_accounting_state(&self) -> (bool, bool) {
        (
            self.attachment.memory.tracking_enabled(),
            self.attachment.memory.tracking_pending(),
        )
    }
}
