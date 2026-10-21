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

mod endpoints;
mod memory;
mod publication;
mod queue;
mod stream_producer;

use queue::close_cause_eq;

use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError,
};
use crate::services::active_agents::{ActiveAgents, MemoryGrant};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::oplog::attachment::{AcceptedAttachmentWriteEnd, ToolAttachmentId};
use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio::sync::{Notify, OwnedSemaphorePermit};
use wasmtime::AsContextMut;
use wasmtime::component::{Destination, StreamProducer, StreamResult, VecBuffer, WriteBuffer};

type MemoryReservation = Pin<Box<dyn Future<Output = Option<MemoryGrant>> + Send + 'static>>;

#[derive(Clone)]
pub(crate) struct AttachmentMemory {
    reserve_tracked: Arc<dyn Fn(u64) -> MemoryReservation + Send + Sync>,
    tracking_enabled: Arc<AtomicBool>,
    tracking_pending: Arc<AtomicBool>,
    prepared_grant: Arc<Mutex<Option<MemoryGrant>>>,
    historical_charges: Arc<HistoricalAttachmentCharges>,
    live_activation: Arc<tokio::sync::Mutex<()>>,
}

struct HistoricalAttachmentCharges {
    state: Mutex<HistoricalAttachmentChargeState>,
}

struct HistoricalAttachmentChargeState {
    bytes: u64,
    accepting: bool,
    upgrade_grant: Option<MemoryGrant>,
}

struct HistoricalAttachmentReservation {
    charges: Option<Arc<HistoricalAttachmentCharges>>,
    bytes: u64,
}

struct AttachmentTrackingRollback<'a> {
    memory: &'a AttachmentMemory,
    changed: &'a Notify,
    armed: bool,
}

enum AttachmentReservationOutcome {
    Reserved(AttachmentReservation),
    Pending,
    Rejected,
}

struct AttachmentReservation {
    grant: MemoryGrant,
    historical_reservation: Option<HistoricalAttachmentReservation>,
}

struct BufferedChunk {
    bytes: AttachmentWriteBody,
    charge: Option<AttachmentCharge>,
    published: Arc<AtomicBool>,
    source_end: Option<AcceptedAttachmentWriteEnd>,
}

pub(super) struct StagedAttachmentWrite {
    attachment: Arc<ByteAttachment>,
    published: Arc<AtomicBool>,
    length: usize,
}

pub(super) enum AttachmentWriteBody {
    Inline(Vec<u8>),
    Recorded {
        length: usize,
        hydrate: Pin<Box<dyn Future<Output = anyhow::Result<Vec<u8>>> + Send>>,
    },
}

struct AttachmentCharge {
    grant: MemoryGrant,
    charged_bytes: Arc<AtomicU64>,
    historical_charges: Option<Arc<HistoricalAttachmentCharges>>,
    reconstruction_bytes: Option<Arc<AtomicU64>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttachmentMode {
    Pending,
    Live,
    Completion { published: bool },
    TerminalOnly,
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolAttachmentModeMetadata {
    Pending,
    Live,
    CompletionStaged,
    CompletionPublished,
    TerminalOnly,
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolAttachmentTerminalMetadata {
    Finished,
    Cancelled,
    Abandoned,
    ResourceExhausted,
    Failed,
    ConsumerCancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolAttachmentMetadata {
    pub mode: ToolAttachmentModeMetadata,
    pub capacity_bytes: usize,
    pub accepted_bytes: u64,
    pub delivered_bytes: u64,
    pub buffered_bytes: usize,
    pub charged_bytes: u64,
    pub backpressured: bool,
    pub terminal_selected: bool,
    pub terminal: Option<ToolAttachmentTerminalMetadata>,
    pub owner_fenced: bool,
    pub host_resource_exhausted: bool,
    pub producer_operation_active: bool,
    pub producer_active: bool,
    pub consumer_active: bool,
}

pub(super) fn terminal_metadata(cause: &ByteStreamCloseCause) -> ToolAttachmentTerminalMetadata {
    match cause {
        ByteStreamCloseCause::Finished => ToolAttachmentTerminalMetadata::Finished,
        ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled) => {
            ToolAttachmentTerminalMetadata::Cancelled
        }
        ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned) => {
            ToolAttachmentTerminalMetadata::Abandoned
        }
        ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted) => {
            ToolAttachmentTerminalMetadata::ResourceExhausted
        }
        ByteStreamCloseCause::Failed(ByteStreamFailure::Failed(_)) => {
            ToolAttachmentTerminalMetadata::Failed
        }
        ByteStreamCloseCause::ConsumerCancelled => {
            ToolAttachmentTerminalMetadata::ConsumerCancelled
        }
    }
}

struct AttachmentState {
    identity: Option<ToolAttachmentId>,
    mode: AttachmentMode,
    chunks: VecDeque<BufferedChunk>,
    observed_writes: BTreeSet<golem_common::model::OplogIndex>,
    observed_terminal: Option<golem_common::model::OplogIndex>,
    buffered_bytes: usize,
    accepted_completion_bytes: usize,
    accepted_bytes: u64,
    delivered_bytes: u64,
    terminal: Option<ByteStreamCloseCause>,
    terminal_pending: bool,
    owner_terminal_reserved: bool,
    terminal_end: Option<golem_common::model::OplogIndex>,
    owner_fenced: bool,
    host_resource_exhausted: bool,
    terminal_item_delivered: bool,
    producer_active: bool,
    consumer_active: bool,
    guest_watchers: usize,
    reader_waker: Option<Waker>,
}

struct ByteAttachment {
    limit: usize,
    memory: AttachmentMemory,
    charged_bytes: Arc<AtomicU64>,
    reconstruction_bytes: Arc<AtomicU64>,
    producer_operation: AtomicBool,
    state: Mutex<AttachmentState>,
    changed: Notify,
    terminal_changed: Notify,
}

#[derive(Debug, Eq, PartialEq)]
enum WritePlan {
    Wait,
    Discard,
    ResourceExhausted,
    Accept,
}

struct ProducerOperation(Arc<ByteAttachment>);

/// A nonempty write with an admission decision, but no accepted stream effect yet.
pub(crate) struct PreparedAttachmentWrite {
    operation: Option<ProducerOperation>,
    content: Option<PreparedWriteContent>,
}

enum PreparedWriteContent {
    Bytes {
        bytes: Vec<u8>,
        charge: AttachmentCharge,
    },
    ResourceExhausted,
}

pub(crate) enum AttachmentWriteAcceptance<R = AttachmentWritePublication> {
    Accepted(R),
    ResourceExhausted(PreparedAttachmentTerminal),
    Discarded,
    Retry,
}

/// Owns the accepted slot until publication. Dropping it releases the slot's memory without
/// selecting a stream terminal; the caller must fail the owner on continuation failure.
pub(crate) struct AttachmentWritePublication {
    operation: ProducerOperation,
    published: Arc<AtomicBool>,
}

pub(crate) struct PreparedAttachmentTerminal {
    operation: ProducerOperation,
    cause: ByteStreamCloseCause,
    host_resource_exhausted: bool,
}

pub(crate) struct AttachmentTerminalPublication {
    attachment: Arc<ByteAttachment>,
    _operation: Option<ProducerOperation>,
}

pub(crate) struct AttachmentProducer {
    attachment: Arc<ByteAttachment>,
    next_operation_ordinal: Arc<AtomicU64>,
    abandon_on_drop: bool,
}

#[derive(Clone)]
pub(crate) struct AttachmentWriter {
    attachment: Arc<ByteAttachment>,
    next_operation_ordinal: Arc<AtomicU64>,
}

pub(crate) struct AttachmentConsumer {
    attachment: Arc<ByteAttachment>,
}

#[derive(Clone)]
pub(crate) struct AttachmentObserver {
    attachment: Arc<ByteAttachment>,
    guest_watcher: Option<Arc<GuestAttachmentWatcher>>,
}

struct GuestAttachmentWatcher(Arc<ByteAttachment>);

/// Operation-owned control plane for an attachment. It carries no producer or consumer role, so
/// retaining it for owner fencing and delayed completion publication cannot keep either endpoint
/// artificially open.
#[derive(Clone)]
pub(crate) struct AttachmentController {
    attachment: Arc<ByteAttachment>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakAttachmentController(std::sync::Weak<ByteAttachment>);

#[derive(Default)]
pub(crate) struct AttachmentStreamBuffer {
    buffer: VecBuffer<Result<Vec<u8>, ByteStreamFailure>>,
    replay_slot: Option<OwnedSemaphorePermit>,
}

type PendingAttachmentChunk = futures::future::BoxFuture<
    'static,
    anyhow::Result<(AttachmentStreamRead, Option<AttachmentCharge>)>,
>;

pub(crate) struct AttachmentStreamProducer {
    consumer: Option<AttachmentConsumer>,
    in_flight_charge: Option<AttachmentCharge>,
    pending_chunk: Option<PendingAttachmentChunk>,
    finished: bool,
}

pub(super) enum AttachmentStreamRead {
    Chunk {
        bytes: Vec<u8>,
        source_end: Option<AcceptedAttachmentWriteEnd>,
    },
    Failure {
        failure: ByteStreamFailure,
        terminal_index: Option<golem_common::model::OplogIndex>,
    },
    Terminal {
        result: StreamResult,
        terminal_index: Option<golem_common::model::OplogIndex>,
    },
}

pub(crate) struct RawAttachmentStreamProducer {
    source: AttachmentStreamProducer,
    remainder: Bytes,
    finished: bool,
}

pub(crate) fn attachment_pair(
    limit: usize,
    memory: AttachmentMemory,
) -> (AttachmentProducer, AttachmentConsumer, AttachmentObserver) {
    let attachment = ByteAttachment::new(limit, memory);
    (
        AttachmentProducer {
            attachment: attachment.clone(),
            next_operation_ordinal: Arc::new(AtomicU64::new(0)),
            abandon_on_drop: true,
        },
        AttachmentConsumer {
            attachment: attachment.clone(),
        },
        AttachmentObserver {
            attachment,
            guest_watcher: None,
        },
    )
}

pub(crate) fn discard_producer(memory: AttachmentMemory) -> AttachmentProducer {
    AttachmentProducer {
        attachment: ByteAttachment::discard(memory),
        next_operation_ordinal: Arc::new(AtomicU64::new(0)),
        abandon_on_drop: true,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AttachmentTerminalSnapshot {
    #[cfg(test)]
    pub(crate) cause: ByteStreamCloseCause,
    pub(crate) host_resource_exhausted: bool,
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct AttachmentActivity {
    pub(crate) producer_active: bool,
    pub(crate) consumer_active: bool,
    pub(crate) buffered_bytes: usize,
    pub(crate) charged_bytes: u64,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum AttachmentRead {
    Item(Result<Vec<u8>, ByteStreamFailure>),
    End,
}

#[cfg(test)]
mod tests;
