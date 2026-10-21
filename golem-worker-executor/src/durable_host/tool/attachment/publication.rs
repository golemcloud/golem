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

use super::{
    AcceptedAttachmentWriteEnd, Arc, AtomicBool, AttachmentMode, AttachmentTerminalPublication,
    AttachmentWriteAcceptance, AttachmentWriteBody, AttachmentWritePublication, BufferedChunk,
    ByteStreamCloseCause, ByteStreamFailure, Ordering, PreparedAttachmentTerminal,
    PreparedAttachmentWrite, PreparedWriteContent, ProducerOperation, StagedAttachmentWrite,
    StreamWriteError, close_cause_eq,
};

impl StagedAttachmentWrite {
    pub(in crate::durable_host::tool) fn publish(self) -> anyhow::Result<()> {
        let mut state = self.attachment.state.lock().unwrap();
        anyhow::ensure!(!state.owner_fenced, "historical attachment owner is fenced");
        state.accepted_bytes = state.accepted_bytes.saturating_add(self.length as u64);
        if matches!(
            state.mode,
            AttachmentMode::Pending | AttachmentMode::Completion { .. }
        ) {
            state.accepted_completion_bytes += self.length;
        }
        self.published.store(true, Ordering::Release);
        let reader = state.reader_waker.take();
        drop(state);
        self.attachment.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
        Ok(())
    }
}

impl Drop for StagedAttachmentWrite {
    fn drop(&mut self) {
        if self.published.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.attachment.state.lock().unwrap();
        let removed = if let Some(position) = state
            .chunks
            .iter()
            .position(|chunk| Arc::ptr_eq(&chunk.published, &self.published))
        {
            let chunk = state.chunks.remove(position).unwrap();
            state.buffered_bytes -= chunk.bytes.len();
            Some(chunk)
        } else {
            None
        };
        let reader = removed.as_ref().and_then(|_| state.reader_waker.take());
        drop(state);
        drop(removed);
        self.attachment.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

impl PreparedAttachmentWrite {
    /// Revalidates and installs the ordered slot before transferring publication ownership
    /// to `enqueue_end` under the attachment lock. The capability must retain the publication
    /// guard, must be synchronous, and must not reenter the attachment.
    pub(crate) fn accept<R>(
        &mut self,
        enqueue_end: impl FnOnce(AttachmentWritePublication) -> R,
    ) -> Result<AttachmentWriteAcceptance<R>, StreamWriteError> {
        let attachment = self
            .operation
            .as_ref()
            .expect("write already accepted")
            .0
            .clone();
        let mut state = attachment.state.lock().unwrap();
        if let Some(cause) = &state.terminal {
            return Err(StreamWriteError::Closed(cause.clone()));
        }
        if state.mode == AttachmentMode::Discard {
            return Ok(AttachmentWriteAcceptance::Discarded);
        }
        let len = match self.content.as_ref().expect("write already accepted") {
            PreparedWriteContent::Bytes { bytes, .. } => bytes.len(),
            PreparedWriteContent::ResourceExhausted => return Ok(self.exhausted()),
        };
        if attachment.memory.tracking_pending() {
            return Ok(AttachmentWriteAcceptance::Retry);
        }
        match state.mode {
            AttachmentMode::Discard => return Ok(AttachmentWriteAcceptance::Discarded),
            AttachmentMode::Pending | AttachmentMode::Live => {
                if len > attachment.limit.saturating_sub(state.buffered_bytes) {
                    return Ok(AttachmentWriteAcceptance::Retry);
                }
            }
            AttachmentMode::Completion { .. } => {
                if len
                    > attachment
                        .limit
                        .saturating_sub(state.accepted_completion_bytes)
                {
                    return Ok(self.exhausted());
                }
            }
            AttachmentMode::TerminalOnly => {
                unreachable!("terminal-only attachments always have a terminal")
            }
        }
        let published = Arc::new(AtomicBool::new(false));
        let PreparedWriteContent::Bytes { bytes, charge } = self.content.take().unwrap() else {
            unreachable!("exhausted writes cannot accept bytes")
        };
        state.chunks.push_back(BufferedChunk {
            bytes: AttachmentWriteBody::Inline(bytes),
            charge: Some(charge),
            published: published.clone(),
            source_end: None,
        });
        state.buffered_bytes += len;
        state.accepted_bytes = state.accepted_bytes.saturating_add(len as u64);
        if matches!(
            state.mode,
            AttachmentMode::Pending | AttachmentMode::Completion { .. }
        ) {
            state.accepted_completion_bytes += len;
        }
        let accepted = enqueue_end(AttachmentWritePublication {
            operation: self.operation.take().unwrap(),
            published,
        });
        drop(state);
        Ok(AttachmentWriteAcceptance::Accepted(accepted))
    }

    pub(super) fn exhausted<R>(&mut self) -> AttachmentWriteAcceptance<R> {
        self.content.take();
        AttachmentWriteAcceptance::ResourceExhausted(PreparedAttachmentTerminal {
            operation: self.operation.take().expect("write already accepted"),
            cause: ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
            host_resource_exhausted: true,
        })
    }

    /// Waits for a possible acceptance outcome without accepting the write. `accept` must
    /// still revalidate after this wait.
    pub(crate) async fn wait_ready(&self) {
        let attachment = &self.operation.as_ref().expect("write already accepted").0;
        let len = match self.content.as_ref().expect("write already accepted") {
            PreparedWriteContent::Bytes { bytes, .. } => bytes.len(),
            PreparedWriteContent::ResourceExhausted => return,
        };
        loop {
            let notified = attachment.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let ready = {
                let state = attachment.state.lock().unwrap();
                state.terminal.is_some()
                    || (!attachment.memory.tracking_pending()
                        && (matches!(
                            state.mode,
                            AttachmentMode::Discard | AttachmentMode::Completion { .. }
                        ) || len <= attachment.limit.saturating_sub(state.buffered_bytes)))
            };
            if ready {
                return;
            }
            notified.as_mut().await;
        }
    }
}

impl AttachmentWritePublication {
    pub(crate) fn publish(self, source_end: Option<AcceptedAttachmentWriteEnd>) {
        let attachment = &self.operation.0;
        let reader = {
            let mut state = attachment.state.lock().unwrap();
            if let Some(chunk) = state.chunks.back_mut() {
                // The publication guard still owns the sole producer operation.
                assert!(Arc::ptr_eq(&chunk.published, &self.published));
                chunk.source_end = source_end;
            }
            self.published.store(true, Ordering::Release);
            state.reader_waker.take()
        };
        attachment.changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

impl Drop for AttachmentWritePublication {
    fn drop(&mut self) {
        if self.published.load(Ordering::Acquire) {
            return;
        }
        let attachment = &self.operation.0;
        let mut state = attachment.state.lock().unwrap();
        if let Some(index) = state
            .chunks
            .iter()
            .position(|chunk| Arc::ptr_eq(&chunk.published, &self.published))
        {
            let chunk = state.chunks.remove(index).unwrap();
            state.buffered_bytes -= chunk.bytes.len();
        }
    }
}

impl PreparedAttachmentTerminal {
    pub(crate) fn publish_local(self) -> Result<(), StreamWriteError> {
        self.operation
            .0
            .select_terminal_with_origin(self.cause, self.host_resource_exhausted)
    }

    pub(crate) fn accept<R>(
        self,
        enqueue: impl FnOnce(AttachmentTerminalPublication) -> R,
    ) -> Result<Option<R>, StreamWriteError> {
        let attachment = self.operation.0.clone();
        let mut state = attachment.state.lock().unwrap();
        if let Some(selected) = &state.terminal {
            return if close_cause_eq(selected, &self.cause) {
                Ok(None)
            } else {
                Err(StreamWriteError::Closed(selected.clone()))
            };
        }
        state.terminal = Some(self.cause);
        state.host_resource_exhausted = self.host_resource_exhausted;
        state.terminal_pending = true;
        let result = enqueue(AttachmentTerminalPublication {
            attachment: attachment.clone(),
            _operation: Some(self.operation),
        });
        drop(state);
        Ok(Some(result))
    }
}

impl AttachmentTerminalPublication {
    pub(crate) fn publish(self, end: golem_common::model::OplogIndex) {
        let attachment = &self.attachment;
        let mut state = attachment.state.lock().unwrap();
        if state.owner_fenced {
            return;
        }
        state.terminal_end = Some(end);
        state.terminal_pending = false;
        let reader = state.reader_waker.take();
        drop(state);
        attachment.changed.notify_waiters();
        attachment.terminal_changed.notify_waiters();
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

impl Drop for ProducerOperation {
    fn drop(&mut self) {
        self.0.producer_operation.store(false, Ordering::Release);
    }
}
