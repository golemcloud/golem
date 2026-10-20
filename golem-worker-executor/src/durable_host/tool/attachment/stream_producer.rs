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
    AsContextMut, AttachmentCharge, AttachmentMode, AttachmentReservationOutcome,
    AttachmentStreamBuffer, AttachmentStreamProducer, AttachmentStreamRead, AttachmentWriteBody,
    BufferedChunk, ByteStreamCloseCause, ByteStreamFailure, Bytes, Context, Destination, Ordering,
    OwnedSemaphorePermit, Pin, Poll, RawAttachmentStreamProducer, StreamProducer, StreamResult,
    VecBuffer, WriteBuffer,
};

impl AttachmentWriteBody {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Inline(bytes) => bytes.len(),
            Self::Recorded { length, .. } => *length,
        }
    }

    pub(super) async fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Inline(bytes) => Ok(bytes),
            Self::Recorded { length, hydrate } => {
                let bytes = hydrate.await?;
                anyhow::ensure!(bytes.len() == length, "attachment body length changed");
                Ok(bytes)
            }
        }
    }
}

impl AttachmentStreamBuffer {
    pub(in crate::durable_host::tool) fn new(
        item: Result<Vec<u8>, ByteStreamFailure>,
        replay_slot: Option<OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            buffer: VecBuffer::from(vec![item]),
            replay_slot,
        }
    }
}

// SAFETY: VecBuffer owns initialization and transfer of every item; the lease only
// limits hydration while this adapter still owns the payload.
unsafe impl WriteBuffer<Result<Vec<u8>, ByteStreamFailure>> for AttachmentStreamBuffer {
    fn remaining(&self) -> &[Result<Vec<u8>, ByteStreamFailure>] {
        self.buffer.remaining()
    }

    fn skip(&mut self, count: usize) {
        self.buffer.skip(count);
        if self.buffer.remaining().is_empty() {
            self.replay_slot.take();
        }
    }

    fn take(
        &mut self,
        count: usize,
        fun: &mut dyn FnMut(&[std::mem::MaybeUninit<Result<Vec<u8>, ByteStreamFailure>>]),
    ) {
        self.buffer.take(count, fun);
        if self.buffer.remaining().is_empty() {
            self.replay_slot.take();
        }
    }
}

impl AttachmentStreamProducer {
    /// Dequeues one item while retaining its memory charge until the next poll or drop.
    /// An adapter must finish using that item before polling for another one.
    pub(in crate::durable_host::tool) fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<wasmtime::Result<AttachmentStreamRead>> {
        let attachment = self
            .consumer
            .as_ref()
            .expect("attachment stream producer lost its consumer")
            .attachment
            .clone();
        let (owner_fenced, consumer_active) = {
            let mut state = attachment.state.lock().unwrap();
            if self.pending_chunk.is_some() {
                state.reader_waker = Some(cx.waker().clone());
            }
            (state.owner_fenced, state.consumer_active)
        };
        if owner_fenced {
            self.pending_chunk = None;
            self.in_flight_charge = None;
            self.finished = true;
            return Poll::Ready(Err(wasmtime::Error::msg(
                "owner generation fenced tool attachment",
            )));
        }
        if !consumer_active {
            self.pending_chunk = None;
            self.in_flight_charge = None;
            self.finished = true;
            return Poll::Ready(Ok(AttachmentStreamRead::Terminal {
                result: StreamResult::Cancelled,
                terminal_index: None,
            }));
        }
        if let Some(pending) = self.pending_chunk.as_mut() {
            let result = std::task::ready!(pending.as_mut().poll(cx));
            self.pending_chunk = None;
            let (owner_fenced, consumer_active) = {
                let mut state = attachment.state.lock().unwrap();
                state.reader_waker.take();
                (state.owner_fenced, state.consumer_active)
            };
            if owner_fenced {
                self.in_flight_charge = None;
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "owner generation fenced tool attachment",
                )));
            }
            if !consumer_active {
                self.in_flight_charge = None;
                self.finished = true;
                return Poll::Ready(Ok(AttachmentStreamRead::Terminal {
                    result: StreamResult::Cancelled,
                    terminal_index: None,
                }));
            }
            return Poll::Ready(
                result
                    .map(|(item, charge)| {
                        self.in_flight_charge = charge;
                        item
                    })
                    .map_err(wasmtime::Error::from_anyhow),
            );
        }
        drop(self.in_flight_charge.take());
        if self.finished {
            return Poll::Ready(Ok(AttachmentStreamRead::Terminal {
                result: StreamResult::Dropped,
                terminal_index: None,
            }));
        }
        enum Produced {
            Chunk(BufferedChunk),
            Item(AttachmentStreamRead, Option<AttachmentCharge>),
            Terminal(StreamResult, Option<golem_common::model::OplogIndex>),
            OwnerFenced,
            Pending,
        }

        let produced = {
            let mut state = attachment.state.lock().unwrap();
            let chunks_visible = matches!(state.mode, AttachmentMode::Live)
                || matches!(state.mode, AttachmentMode::Completion { published: true });
            let terminal_visible = !state.terminal_pending
                && (chunks_visible || matches!(state.mode, AttachmentMode::TerminalOnly));
            if state.owner_fenced {
                Produced::OwnerFenced
            } else if state
                .chunks
                .front()
                .is_some_and(|chunk| !chunk.published.load(Ordering::Acquire))
            {
                state.reader_waker = Some(cx.waker().clone());
                Produced::Pending
            } else if chunks_visible && !state.chunks.is_empty() {
                let chunk = state.chunks.pop_front().unwrap();
                state.buffered_bytes -= chunk.bytes.len();
                state.delivered_bytes = state
                    .delivered_bytes
                    .saturating_add(chunk.bytes.len() as u64);
                Produced::Chunk(chunk)
            } else if !terminal_visible {
                state.reader_waker = Some(cx.waker().clone());
                Produced::Pending
            } else {
                match state.terminal.clone() {
                    Some(ByteStreamCloseCause::Finished) => {
                        Produced::Terminal(StreamResult::Dropped, state.terminal_end)
                    }
                    Some(ByteStreamCloseCause::Failed(failure))
                        if !state.terminal_item_delivered =>
                    {
                        state.terminal_item_delivered = true;
                        Produced::Item(
                            AttachmentStreamRead::Failure {
                                failure,
                                terminal_index: state.terminal_end,
                            },
                            None,
                        )
                    }
                    Some(ByteStreamCloseCause::Failed(_)) => {
                        Produced::Terminal(StreamResult::Dropped, state.terminal_end)
                    }
                    Some(ByteStreamCloseCause::ConsumerCancelled) => {
                        Produced::Terminal(StreamResult::Cancelled, state.terminal_end)
                    }
                    None => {
                        state.reader_waker = Some(cx.waker().clone());
                        Produced::Pending
                    }
                }
            }
        };

        match produced {
            Produced::Chunk(chunk) => {
                let owner = attachment.clone();
                self.pending_chunk = Some(Box::pin(async move {
                    let mut charge = chunk.charge;
                    if matches!(chunk.bytes, AttachmentWriteBody::Recorded { .. }) {
                        loop {
                            let changed = owner.changed.notified();
                            tokio::pin!(changed);
                            changed.as_mut().enable();
                            anyhow::ensure!(
                                !owner.state.lock().unwrap().owner_fenced,
                                "historical attachment owner is fenced"
                            );
                            match owner.memory.reserve(chunk.bytes.len()).await {
                                AttachmentReservationOutcome::Reserved(reservation) => {
                                    charge = Some(AttachmentCharge::new(
                                        reservation,
                                        owner.charged_bytes.clone(),
                                    ));
                                    break;
                                }
                                AttachmentReservationOutcome::Pending => changed.await,
                                AttachmentReservationOutcome::Rejected => {
                                    anyhow::bail!("attachment hydration memory admission rejected")
                                }
                            }
                        }
                    }
                    let item = AttachmentStreamRead::Chunk {
                        bytes: chunk.bytes.into_bytes().await?,
                        source_end: chunk.source_end,
                    };
                    Ok((item, charge))
                }));
                attachment.changed.notify_waiters();
                self.poll_read(cx)
            }
            Produced::Item(item, charge) => {
                self.in_flight_charge = charge;
                attachment.changed.notify_waiters();
                Poll::Ready(Ok(item))
            }
            Produced::Terminal(result, terminal_index) => {
                self.finished = true;
                Poll::Ready(Ok(AttachmentStreamRead::Terminal {
                    result,
                    terminal_index,
                }))
            }
            Produced::OwnerFenced => {
                self.finished = true;
                Poll::Ready(Err(wasmtime::Error::msg(
                    "owner generation fenced tool attachment",
                )))
            }
            Produced::Pending => Poll::Pending,
        }
    }
}

impl<D> StreamProducer<D> for AttachmentStreamProducer {
    type Item = Result<Vec<u8>, ByteStreamFailure>;
    type Buffer = AttachmentStreamBuffer;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        drop(self.in_flight_charge.take());
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        match std::task::ready!(self.poll_read(cx))? {
            AttachmentStreamRead::Chunk { bytes, .. } => {
                dst.set_buffer(AttachmentStreamBuffer::new(Ok(bytes), None));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            AttachmentStreamRead::Failure { failure, .. } => {
                dst.set_buffer(AttachmentStreamBuffer::new(Err(failure), None));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            AttachmentStreamRead::Terminal { result, .. } => Poll::Ready(Ok(result)),
        }
    }
}

impl<D> StreamProducer<D> for RawAttachmentStreamProducer {
    type Item = u8;
    type Buffer = Bytes;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.remainder.is_empty() && self.source.pending_chunk.is_none() {
            drop(self.source.in_flight_charge.take());
        }
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let attachment = self
            .source
            .consumer
            .as_ref()
            .expect("raw attachment stream producer lost its consumer")
            .attachment
            .clone();
        let remaining = dst.remaining(store.as_context_mut());
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if remaining == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        if !self.remainder.is_empty() {
            if attachment.state.lock().unwrap().owner_fenced {
                self.finished = true;
                self.remainder = Bytes::new();
                self.source.in_flight_charge = None;
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "owner generation fenced tool attachment",
                )));
            }
            let length = remaining
                .unwrap_or(self.remainder.len())
                .min(self.remainder.len());
            dst.set_buffer(self.remainder.split_to(length));
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        match std::task::ready!(self.source.poll_read(cx))? {
            AttachmentStreamRead::Chunk { bytes, .. } => {
                self.remainder = Bytes::from(bytes);
                let length = remaining
                    .unwrap_or(self.remainder.len())
                    .min(self.remainder.len());
                dst.set_buffer(self.remainder.split_to(length));
                attachment.changed.notify_waiters();
                Poll::Ready(Ok(StreamResult::Completed))
            }
            AttachmentStreamRead::Terminal { result, .. } => {
                self.finished = true;
                Poll::Ready(Ok(result))
            }
            AttachmentStreamRead::Failure { failure, .. } => {
                self.finished = true;
                Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "underlying tool stdout failed: {failure:?}"
                ))))
            }
        }
    }
}
