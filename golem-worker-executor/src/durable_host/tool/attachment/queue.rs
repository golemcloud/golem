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
use super::AttachmentActivity;
use super::{
    AcceptedAttachmentWriteEnd, Arc, AtomicBool, AtomicU64, AttachmentCharge, AttachmentMemory,
    AttachmentMode, AttachmentReservationOutcome, AttachmentState, AttachmentTrackingRollback,
    AttachmentWriteAcceptance, BTreeSet, ByteAttachment, ByteStreamCloseCause, ByteStreamFailure,
    MemoryGrant, Mutex, Notify, Ordering, PreparedAttachmentWrite, PreparedWriteContent,
    ProducerOperation, StreamWriteError, ToolAttachmentMetadata, ToolAttachmentModeMetadata,
    VecDeque, WritePlan, terminal_metadata,
};

impl ByteAttachment {
    pub(in crate::durable_host::tool) fn new(limit: usize, memory: AttachmentMemory) -> Arc<Self> {
        Arc::new(Self {
            limit,
            memory,
            charged_bytes: Arc::new(AtomicU64::new(0)),
            reconstruction_bytes: Arc::new(AtomicU64::new(0)),
            producer_operation: AtomicBool::new(false),
            state: Mutex::new(AttachmentState {
                identity: None,
                mode: AttachmentMode::Pending,
                chunks: VecDeque::new(),
                observed_writes: BTreeSet::new(),
                observed_terminal: None,
                buffered_bytes: 0,
                accepted_completion_bytes: 0,
                accepted_bytes: 0,
                delivered_bytes: 0,
                terminal: None,
                terminal_pending: false,
                owner_terminal_reserved: false,
                terminal_end: None,
                owner_fenced: false,
                host_resource_exhausted: false,
                terminal_item_delivered: false,
                producer_active: true,
                consumer_active: true,
                guest_watchers: 0,
                reader_waker: None,
            }),
            changed: Notify::new(),
            terminal_changed: Notify::new(),
        })
    }

    pub(super) fn discard(memory: AttachmentMemory) -> Arc<Self> {
        Arc::new(Self {
            limit: 0,
            memory,
            charged_bytes: Arc::new(AtomicU64::new(0)),
            reconstruction_bytes: Arc::new(AtomicU64::new(0)),
            producer_operation: AtomicBool::new(false),
            state: Mutex::new(AttachmentState {
                identity: None,
                mode: AttachmentMode::Discard,
                chunks: VecDeque::new(),
                observed_writes: BTreeSet::new(),
                observed_terminal: None,
                buffered_bytes: 0,
                accepted_completion_bytes: 0,
                accepted_bytes: 0,
                delivered_bytes: 0,
                terminal: None,
                terminal_pending: false,
                owner_terminal_reserved: false,
                terminal_end: None,
                owner_fenced: false,
                host_resource_exhausted: false,
                terminal_item_delivered: false,
                producer_active: true,
                consumer_active: false,
                guest_watchers: 0,
                reader_waker: None,
            }),
            changed: Notify::new(),
            terminal_changed: Notify::new(),
        })
    }

    pub(super) fn begin_producer_operation(
        self: &Arc<Self>,
    ) -> Result<ProducerOperation, StreamWriteError> {
        self.producer_operation
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ProducerOperation(self.clone()))
            .map_err(|_| StreamWriteError::ConcurrentOperation)
    }

    pub(super) fn configure(&self, mode: AttachmentMode) -> bool {
        let reader = {
            let mut state = self.state.lock().unwrap();
            if state.mode != AttachmentMode::Pending {
                return false;
            }
            if mode == AttachmentMode::Live {
                state.accepted_completion_bytes = 0;
            }
            state.mode = mode;
            state.reader_waker.take()
        };
        tracing::debug!(attachment = ?(self as *const Self), ?mode, "configured tool attachment");
        self.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        true
    }

    pub(super) async fn prepare_live_memory_accounting(&self) -> bool {
        let _activation = self.memory.live_activation.lock().await;
        if self.memory.tracking_enabled() {
            return true;
        }
        if self.memory.tracking_pending() {
            return self.memory.prepared_grant.lock().unwrap().is_some();
        }

        self.memory.begin_tracking();
        self.changed.notify_waiters();
        let mut rollback = AttachmentTrackingRollback {
            memory: &self.memory,
            changed: &self.changed,
            armed: true,
        };
        let mut grant = MemoryGrant::inert(0);
        loop {
            let historical_bytes = self.memory.historical_charges.bytes();
            if historical_bytes <= grant.bytes() {
                self.memory.prepare_grant(grant);
                rollback.disarm();
                return true;
            }
            let Some(additional) = self
                .memory
                .reserve_tracked(historical_bytes - grant.bytes())
                .await
            else {
                rollback.disarm();
                return false;
            };
            grant.merge(additional);
        }
    }

    pub(super) fn commit_live_memory_accounting(&self) {
        if self.memory.tracking_enabled() {
            return;
        }
        self.memory.commit_tracking();
        self.changed.notify_waiters();
    }

    pub(super) fn complete_rejected_live_memory_accounting(&self) {
        {
            let mut state = self.state.lock().unwrap();
            if matches!(
                state.mode,
                AttachmentMode::Pending
                    | AttachmentMode::Live
                    | AttachmentMode::Completion { published: false }
            ) {
                state.chunks.clear();
                state.buffered_bytes = 0;
                state.accepted_completion_bytes = 0;
            }
        }
        let _ = self.select_terminal_with_origin(
            ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
            true,
        );
        self.publish_no_body_terminal();
    }

    pub(super) fn publish_completion(&self) -> bool {
        let reader = {
            let mut state = self.state.lock().unwrap();
            let AttachmentMode::Completion { published: false } = state.mode else {
                return false;
            };
            if state.terminal.is_none() {
                return false;
            }
            state.mode = AttachmentMode::Completion { published: true };
            state.reader_waker.take()
        };
        self.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        true
    }

    pub(super) fn publish_no_body_terminal(&self) -> bool {
        self.memory.abort_tracking();
        let reader = {
            let mut state = self.state.lock().unwrap();
            if state.owner_fenced || state.terminal.is_none() {
                return false;
            }
            match state.mode {
                AttachmentMode::Pending => {
                    state.chunks.clear();
                    state.buffered_bytes = 0;
                    state.mode = AttachmentMode::TerminalOnly;
                }
                AttachmentMode::Completion { published: false } => {
                    state.mode = AttachmentMode::Completion { published: true };
                }
                AttachmentMode::Live
                | AttachmentMode::Completion { published: true }
                | AttachmentMode::TerminalOnly
                | AttachmentMode::Discard => {}
            }
            state.reader_waker.take()
        };
        self.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        true
    }

    pub(super) fn select_terminal(
        &self,
        cause: ByteStreamCloseCause,
    ) -> Result<(), StreamWriteError> {
        self.select_terminal_with_origin(cause, false)
    }

    pub(super) fn select_terminal_with_origin(
        &self,
        cause: ByteStreamCloseCause,
        host_resource_exhausted: bool,
    ) -> Result<(), StreamWriteError> {
        let terminal = terminal_metadata(&cause);
        let reader = {
            let mut state = self.state.lock().unwrap();
            if let Some(selected) = &state.terminal {
                return if close_cause_eq(selected, &cause) {
                    Ok(())
                } else {
                    Err(StreamWriteError::Closed(selected.clone()))
                };
            }
            if matches!(cause, ByteStreamCloseCause::ConsumerCancelled) {
                state.chunks.clear();
                state.buffered_bytes = 0;
            }
            state.terminal = Some(cause);
            state.host_resource_exhausted = host_resource_exhausted;
            state.reader_waker.take()
        };
        tracing::debug!(
            attachment = ?(self as *const Self),
            ?terminal,
            host_resource_exhausted,
            "Selected tool attachment terminal"
        );
        self.changed.notify_waiters();
        self.terminal_changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        Ok(())
    }

    pub(super) fn fence_owner(&self) {
        self.memory.abort_tracking();
        let (reader, selected_terminal) = {
            let mut state = self.state.lock().unwrap();
            state.owner_fenced = true;
            state.owner_terminal_reserved = false;
            state.chunks.clear();
            state.buffered_bytes = 0;
            let selected_terminal = state.terminal.is_none() || state.terminal_pending;
            if selected_terminal {
                state.terminal = Some(ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled));
                state.terminal_pending = false;
                state.terminal_end = None;
                state.host_resource_exhausted = false;
            }
            (state.reader_waker.take(), selected_terminal)
        };
        self.changed.notify_waiters();
        if selected_terminal {
            self.terminal_changed.notify_waiters();
        }
        tracing::debug!(
            attachment = ?(self as *const Self),
            selected_terminal,
            "Fenced tool attachment with owner generation"
        );
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    pub(super) fn terminate_unconfigured(
        &self,
        cause: ByteStreamCloseCause,
    ) -> Result<bool, StreamWriteError> {
        let (reader, selected_terminal, result) = {
            let mut state = self.state.lock().unwrap();
            if state.mode != AttachmentMode::Pending || !state.chunks.is_empty() {
                return Ok(false);
            }
            let (selected_terminal, result) = if let Some(selected) = &state.terminal {
                (
                    false,
                    if close_cause_eq(selected, &cause) {
                        Ok(())
                    } else {
                        Err(StreamWriteError::Closed(selected.clone()))
                    },
                )
            } else {
                state.terminal = Some(cause);
                (true, Ok(()))
            };
            state.mode = AttachmentMode::TerminalOnly;
            (state.reader_waker.take(), selected_terminal, result)
        };
        self.changed.notify_waiters();
        if selected_terminal {
            self.terminal_changed.notify_waiters();
        }
        if let Some(reader) = reader {
            reader.wake();
        }
        result.map(|()| true)
    }

    pub(super) async fn write(
        self: &Arc<Self>,
        bytes: Vec<u8>,
        source_end: Option<AcceptedAttachmentWriteEnd>,
    ) -> Result<(), StreamWriteError> {
        let Some(mut prepared) = self.prepare_write(bytes).await? else {
            return Ok(());
        };
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match prepared.accept(std::convert::identity)? {
                AttachmentWriteAcceptance::Accepted(publication) => {
                    publication.publish(source_end);
                    return Ok(());
                }
                AttachmentWriteAcceptance::ResourceExhausted(prepared) => {
                    prepared.publish_local()?;
                    return Err(StreamWriteError::Closed(ByteStreamCloseCause::Failed(
                        ByteStreamFailure::ResourceExhausted,
                    )));
                }
                AttachmentWriteAcceptance::Discarded => return Ok(()),
                AttachmentWriteAcceptance::Retry => notified.as_mut().await,
            }
        }
    }

    pub(super) async fn prepare_write(
        self: &Arc<Self>,
        bytes: Vec<u8>,
    ) -> Result<Option<PreparedAttachmentWrite>, StreamWriteError> {
        let operation = self.begin_producer_operation()?;
        if bytes.is_empty() {
            return Ok(None);
        }

        let len = bytes.len();
        if len > self.limit {
            {
                let state = self.state.lock().unwrap();
                if let Some(cause) = &state.terminal {
                    return Err(StreamWriteError::Closed(cause.clone()));
                }
                if state.mode == AttachmentMode::Discard {
                    return Ok(None);
                }
            }
            return Ok(Some(PreparedAttachmentWrite {
                operation: Some(operation),
                content: Some(PreparedWriteContent::ResourceExhausted),
            }));
        }
        let bytes = bytes.into_boxed_slice().into_vec();
        let allocation_bytes = bytes.capacity();
        let charge = loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let plan = {
                let state = self.state.lock().unwrap();
                if let Some(cause) = &state.terminal {
                    return Err(StreamWriteError::Closed(cause.clone()));
                }
                if self.memory.tracking_pending() {
                    WritePlan::Wait
                } else {
                    match state.mode {
                        AttachmentMode::Discard => WritePlan::Discard,
                        AttachmentMode::Pending | AttachmentMode::Live => WritePlan::Accept,
                        AttachmentMode::Completion { .. } => {
                            if len > self.limit.saturating_sub(state.accepted_completion_bytes) {
                                WritePlan::ResourceExhausted
                            } else {
                                WritePlan::Accept
                            }
                        }
                        AttachmentMode::TerminalOnly => {
                            unreachable!("terminal-only attachments always have a terminal")
                        }
                    }
                }
            };

            match plan {
                WritePlan::Discard => return Ok(None),
                WritePlan::ResourceExhausted => {
                    return Ok(Some(PreparedAttachmentWrite {
                        operation: Some(operation),
                        content: Some(PreparedWriteContent::ResourceExhausted),
                    }));
                }
                WritePlan::Accept => {}
                WritePlan::Wait => {
                    notified.as_mut().await;
                    continue;
                }
            }

            let reservation = self.memory.reserve(allocation_bytes);
            tokio::pin!(reservation);
            let charge = tokio::select! {
                charge = &mut reservation => charge,
                () = &mut notified => continue,
            };
            let reservation = match charge {
                AttachmentReservationOutcome::Reserved(reservation) => reservation,
                AttachmentReservationOutcome::Pending => continue,
                AttachmentReservationOutcome::Rejected => {
                    return Ok(Some(PreparedAttachmentWrite {
                        operation: Some(operation),
                        content: Some(PreparedWriteContent::ResourceExhausted),
                    }));
                }
            };
            break AttachmentCharge::new(reservation, self.charged_bytes.clone());
        };

        Ok(Some(PreparedAttachmentWrite {
            operation: Some(operation),
            content: Some(PreparedWriteContent::Bytes { bytes, charge }),
        }))
    }

    pub(super) async fn wait_terminal(&self) -> ByteStreamCloseCause {
        loop {
            let notified = self.terminal_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let terminal = {
                let state = self.state.lock().unwrap();
                (!state.terminal_pending)
                    .then(|| state.terminal.clone())
                    .flatten()
            };
            if let Some(cause) = terminal {
                return cause;
            }
            notified.as_mut().await;
        }
    }

    pub(super) fn consumer_cancel(&self) {
        let (reader, selected_terminal) = {
            let mut state = self.state.lock().unwrap();
            state.consumer_active = false;
            state.chunks.clear();
            state.buffered_bytes = 0;
            let selected_terminal = if state.terminal.is_none() {
                state.terminal = Some(ByteStreamCloseCause::ConsumerCancelled);
                true
            } else {
                false
            };
            (state.reader_waker.take(), selected_terminal)
        };
        self.changed.notify_waiters();
        if selected_terminal {
            self.terminal_changed.notify_waiters();
        }
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    pub(super) fn producer_abandon(&self) {
        let _ = self.select_terminal(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned));
        self.state.lock().unwrap().producer_active = false;
        self.changed.notify_waiters();
    }

    pub(super) fn sanitize_failure(failure: ByteStreamFailure) -> ByteStreamFailure {
        match failure {
            ByteStreamFailure::Failed(_) => {
                ByteStreamFailure::Failed("stream producer failed".to_string())
            }
            other => other,
        }
    }

    #[cfg(test)]
    pub(super) fn activity(&self) -> AttachmentActivity {
        let state = self.state.lock().unwrap();
        AttachmentActivity {
            producer_active: state.producer_active,
            consumer_active: state.consumer_active,
            buffered_bytes: state.buffered_bytes,
            charged_bytes: self.charged_bytes.load(Ordering::Acquire),
        }
    }

    pub(super) fn metadata(&self) -> ToolAttachmentMetadata {
        let state = self.state.lock().unwrap();
        let terminal = state.terminal.as_ref().map(terminal_metadata);
        ToolAttachmentMetadata {
            mode: match state.mode {
                AttachmentMode::Pending => ToolAttachmentModeMetadata::Pending,
                AttachmentMode::Live => ToolAttachmentModeMetadata::Live,
                AttachmentMode::Completion { published: false } => {
                    ToolAttachmentModeMetadata::CompletionStaged
                }
                AttachmentMode::Completion { published: true } => {
                    ToolAttachmentModeMetadata::CompletionPublished
                }
                AttachmentMode::TerminalOnly => ToolAttachmentModeMetadata::TerminalOnly,
                AttachmentMode::Discard => ToolAttachmentModeMetadata::Discard,
            },
            capacity_bytes: self.limit,
            accepted_bytes: state.accepted_bytes,
            delivered_bytes: state.delivered_bytes,
            buffered_bytes: state.buffered_bytes,
            charged_bytes: self.charged_bytes.load(Ordering::Acquire),
            backpressured: matches!(state.mode, AttachmentMode::Live)
                && state.buffered_bytes >= self.limit
                && state.terminal.is_none()
                && state.producer_active
                && state.consumer_active,
            terminal_selected: state.terminal.is_some(),
            terminal,
            owner_fenced: state.owner_fenced,
            host_resource_exhausted: state.host_resource_exhausted,
            producer_operation_active: self.producer_operation.load(Ordering::Acquire),
            producer_active: state.producer_active,
            consumer_active: state.consumer_active,
        }
    }
}

pub(super) fn failure_eq(left: &ByteStreamFailure, right: &ByteStreamFailure) -> bool {
    match (left, right) {
        (ByteStreamFailure::Cancelled, ByteStreamFailure::Cancelled)
        | (ByteStreamFailure::Abandoned, ByteStreamFailure::Abandoned)
        | (ByteStreamFailure::ResourceExhausted, ByteStreamFailure::ResourceExhausted) => true,
        (ByteStreamFailure::Failed(left), ByteStreamFailure::Failed(right)) => left == right,
        _ => false,
    }
}

pub(super) fn close_cause_eq(left: &ByteStreamCloseCause, right: &ByteStreamCloseCause) -> bool {
    match (left, right) {
        (ByteStreamCloseCause::Finished, ByteStreamCloseCause::Finished)
        | (ByteStreamCloseCause::ConsumerCancelled, ByteStreamCloseCause::ConsumerCancelled) => {
            true
        }
        (ByteStreamCloseCause::Failed(left), ByteStreamCloseCause::Failed(right)) => {
            failure_eq(left, right)
        }
        _ => false,
    }
}
