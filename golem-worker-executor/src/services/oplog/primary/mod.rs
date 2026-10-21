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

use crate::metrics::oplog::{
    record_oplog_call, record_oplog_epoch_fence, record_oplog_storage_retry,
};
use crate::metrics::storage::{
    STORAGE_TYPE_OPLOG, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::model::ExecutionStatus;
use crate::services::oplog::reader::{
    OplogReadSource, UnreadableOplog, checked_range_end, exact_from_source,
};
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, OpenOplogs, Oplog,
    OplogAddReceipt, OplogCloseCompletion, OplogConstructor, OplogError, OplogFence,
    OplogLifecycleGuard, OplogService, OrderedOplogStart, PendingUpload,
    RawOplogPayloadDownloadError, ReservedPayload, ReservedRawStartBuilder, StagePublication,
    decode_scan_cursor, next_scan_cursor, record_epoch_verdict, record_owning_epoch,
    refuse_if_fenced, retry_scan_storage_op,
};
use crate::services::shutdown::join_or_park_at_runtime_shutdown;
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::model::RetryConfig;
use golem_common::model::ShardEpoch;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatusRecord, DurableStreamSessionStatus,
    OwnedAgentId, ScanCursor,
};
use golem_common::read_only_lock;
use golem_common::retries::get_delay;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace,
};
use std::cmp::{max, min};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, warn};

mod constructor;
mod handle;
mod reader;
mod service;
mod state;
mod storage;

use reader::read_persisted_oplog_entries;
use storage::{
    retry_maintenance_storage_op, retry_oplog_append, retry_storage_op, retry_storage_op_result,
};

enum SerializedOplogAppend {
    Entry((u64, Bytes)),
    Batch(Arc<[(u64, Bytes)]>),
}

/// Whether an open still has to record the epoch it asserts, or its caller did so already.
#[derive(Clone)]
enum EpochRecord {
    /// The open records it, and takes the refusal it gets.
    Pending,
    /// A create recorded it before the first entry went in, with this refusal.
    Recorded(Option<OplogFence>),
}

/// The primary oplog service implementation, suitable for direct use (top level of a multi-layered setup).
///
/// Stores and retrieves individual oplog entries from the `IndexedStorage` implementation configured for
/// the executor.
#[derive(Clone)]
pub struct PrimaryOplogService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_operations_before_commit_ephemeral: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    oplogs: OpenOplogs,
    stream_session_index: Arc<std::sync::OnceLock<Arc<super::StreamSessionIndexService>>>,
}

#[derive(Clone)]
struct CreateOplogConstructor {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    key: String,
    last_oplog_idx: Option<OplogIndex>,
    /// `last_oplog_idx` was read before this constructor claims the epoch, so it may be behind
    /// entries an executor at an older epoch committed in between.
    reconcile_last_index: bool,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    fingerprint: AgentFingerprint,
    stream_session_index: Option<Arc<super::StreamSessionIndexService>>,
    shard_epoch: Option<ShardEpoch>,
    epoch_record: EpochRecord,
}

/// The primary oplog behind an actor boundary.
///
/// A dedicated tokio task (the actor) exclusively owns [`PrimaryOplogState`]; every `Oplog`
/// method sends an [`OplogJob`] over an unbounded channel and awaits the job's oneshot reply.
/// There is deliberately **no shared lock** between callers.
///
/// This shape is required for deadlock freedom, not just style. `Oplog` methods are awaited from
/// two kinds of callers that must never block each other through shared ownership:
///
/// * futures polled by wasmtime's store event loop (concurrent p3 durable host calls), and
/// * host code running on wasm fibers that suspend while *keeping the store* (async libcalls such
///   as the `memory.grow` resource limiter, and non-concurrent `wrap_async`-style host functions
///   like the p2 stdio streams).
///
/// While such a fiber is suspended, the event loop cannot poll any store-polled future
/// (wasmtime's documented store-blocking limitation, wasmtime#11869/#11870). With a shared lock —
/// even a lock whose critical sections are purely synchronous — a store-polled future that is
/// queued on the lock becomes its next owner on FIFO handoff and then cannot run until the event
/// loop polls it, so a fiber queued behind it deadlocks the whole store. With the actor, callers
/// only await oneshot completions, which never make an unpolled caller the owner of anything, so
/// fiber-side waits always make progress as long as the actor task (polled directly by tokio)
/// does.
///
/// The actor must therefore never await anything that is produced by a store event loop; it only
/// performs storage I/O.
///
/// ORDERING (Start determinism): `add_start_with_reserved_raw_payload` relies on jobs being
/// processed in the order they were enqueued. `mpsc::UnboundedSender::send` is synchronous, and
/// callers enqueue as their first non-awaiting step, so enqueue order equals the order in which
/// concurrent durable calls initiated their operation — the same guarantee the previous
/// FIFO-fair mutex provided via `lock()` acquisition order.
struct PrimaryOplog {
    jobs: tokio::sync::mpsc::UnboundedSender<OplogJob>,
    closed: OplogCloseCompletion,
    tasks: super::WorkerTasks,
    retired: AtomicBool,
    key: String,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    fingerprint: AgentFingerprint,
    /// The epoch the actor's state asserts on every append, copied here so that reading it does
    /// not have to go through the actor. Fixed for the oplog's lifetime.
    shard_epoch: Option<ShardEpoch>,
    /// The inline payload threshold of the actor's state, copied here so that an inline upload
    /// does not wait behind the actor's queued commits. Fixed for the oplog's lifetime.
    max_payload_size: usize,
    /// The refusal the actor's state has latched, shared so the handle can report it.
    fence: Arc<std::sync::OnceLock<OplogFence>>,
    stream_session_index: Option<Arc<super::StreamSessionIndexService>>,
    close: Mutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

/// A request processed by the [`PrimaryOplog`] actor task, which exclusively owns the oplog
/// state. Mutating jobs that grow the buffer past the commit threshold run the resulting commit
/// inside the actor before replying, preserving the pre-actor behavior where `add` blocked the
/// caller on a threshold-triggered commit.
enum OplogJob {
    Close,
    Add {
        entry: OplogEntry,
        done: tokio::sync::oneshot::Sender<Result<OplogIndex, OplogError>>,
    },
    AddDurableStreamBatch {
        make_batch: DurableStreamBatchBuilder,
        done: tokio::sync::oneshot::Sender<Result<Vec<(OplogIndex, OplogEntry)>, OplogError>>,
    },
    AddPair {
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        done: tokio::sync::oneshot::Sender<Result<(OplogIndex, OplogIndex), OplogError>>,
    },
    AddStart {
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, OplogError>>,
    },
    AddIndexedStart {
        build_request: IndexedReservedStartBuilder,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, OplogError>>,
    },
    Commit {
        level: CommitLevel,
        done: tokio::sync::oneshot::Sender<Result<BTreeMap<OplogIndex, OplogEntry>, OplogError>>,
    },
    Flush {
        done: tokio::sync::oneshot::Sender<()>,
    },
    DropPrefix {
        last_dropped_id: OplogIndex,
        done: tokio::sync::oneshot::Sender<Result<u64, String>>,
    },
    CurrentIndex {
        done: tokio::sync::oneshot::Sender<OplogIndex>,
    },
    RawDurableStreamSessionStatus {
        session_key: golem_common::model::durable_stream::StreamSessionKey,
        done: tokio::sync::oneshot::Sender<RawSessionLookup>,
    },
    CompleteRawDurableStreamSessionStatus {
        session_key: golem_common::model::durable_stream::StreamSessionKey,
        expected_watermark: OplogIndex,
        expected_committed: OplogIndex,
        status: Box<Result<Option<DurableStreamSessionStatus>, String>>,
        done: tokio::sync::oneshot::Sender<Option<super::RawDurableStreamSessionStatus>>,
    },
    LastAddedNonHintEntry {
        done: tokio::sync::oneshot::Sender<Option<OplogIndex>>,
    },
    /// Snapshots the state needed for reads and replica waits; the caller performs the storage
    /// I/O itself, off the actor, so large reads do not head-of-line block writes.
    Reader {
        done: tokio::sync::oneshot::Sender<OplogReader>,
    },
    /// Snapshots the state needed for payload blob uploads/downloads.
    BlobContext {
        done: tokio::sync::oneshot::Sender<OplogBlobContext>,
    },
}

struct RawSessionLookup {
    watermark: OplogIndex,
    committed: OplogIndex,
    buffer: VecDeque<OplogEntry>,
    cached: Option<Result<Option<DurableStreamSessionStatus>, String>>,
}

/// Snapshot of the state needed to upload/download oplog payload blobs outside the actor.
struct OplogBlobContext {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    max_payload_size: usize,
}

/// A snapshot of [`PrimaryOplogState`] sufficient to serve reads. The actor hands this snapshot
/// to the caller, which performs the read I/O itself, off the actor task — so large reads never
/// head-of-line block writes (see [`OplogJob::Reader`]). The buffer snapshot keeps both single and
/// batched reads' visibility of not-yet-committed entries.
struct OplogReader {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: RetryConfig,
    namespace: IndexedStorageNamespace,
    key: String,
    last_committed_idx: OplogIndex,
    buffer: VecDeque<OplogEntry>,
    replicas: u8,
}

struct PrimaryOplogState {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    namespace: IndexedStorageNamespace,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    last_reported_commit_idx: OplogIndex,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    fingerprint: AgentFingerprint,
    account_id: AccountId,
    account_id_label: String,
    environment_id_label: String,
    last_added_non_hint_entry: Option<OplogIndex>,
    /// In-flight external payload uploads started by [`PrimaryOplogState::reserve_raw_payload`] but
    /// not yet known to be durable. The commit barrier in `append` waits on these before persisting
    /// any buffered entries, so no committed entry can reference a not-yet-written blob.
    pending_uploads: Vec<PendingUpload>,
    durable_stream_sessions: super::raw_session::RawSessionCache,
    /// The shard epoch this executor held for the agent's shard when the oplog was opened, and
    /// the one every append asserts.
    ///
    /// Cached at open rather than read per write: one live oplog is one ownership generation, and
    /// this is the value its metadata row was written with. A renewal never changes it - an epoch
    /// only moves when the shard changes owner, and then this oplog is the losing side.
    shard_epoch: Option<ShardEpoch>,
    /// Set once a write has been refused, or at open when the epoch record already belonged to a
    /// newer owner. Every later write fails on it immediately: the oplog is another executor's
    /// now, so there is nothing to be gained by asking the storage again. Shared with the handle,
    /// which answers [`Oplog::fence`] from it without a round trip through the actor.
    fence: Arc<std::sync::OnceLock<OplogFence>>,
}
