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

use crate::metrics::oplog::record_oplog_epoch_fence;
use crate::model::ExecutionStatus;
use crate::services::shutdown::join_or_park_at_runtime_shutdown;
use crate::services::stream_session_index::StreamSessionIndexService;
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageNamespace, ScanResume,
};
pub use crate::worker::tasks::WorkerTasks;
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
pub use blob::BlobOplogArchiveService;
pub use compressed::{CompressedOplogArchive, CompressedOplogArchiveService, CompressedOplogChunk};
use desert_rust::BinaryCodec;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::agent::AgentMode;
use golem_common::model::card::InvocationWalletPin;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::durable_stream::{
    StreamCancelRecord, StreamEndRecord, StreamItemsRecord, StreamRegisteredRecord,
    StreamSessionRecord,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, DurableInvocationId, DurableStreamEventSummary, FilesystemSnapshotName,
    HostRequest, HostResponse, OplogEntry, OplogIndex, OplogPayload, PayloadId, RawOplogPayload,
    UpdateDescription,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationResult, AgentMetadata, AgentStatusRecord,
    DurableStreamSessionStatus, IdempotencyKey, OwnedAgentId, ScanCursor, ShardEpoch, Timestamp,
};
use golem_common::read_only_lock;
use golem_common::retries::get_delay;
use golem_common::serialization::serialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use serde::{Deserialize, Serialize};

pub use ephemeral::EphemeralOplog;
pub use multilayer::{
    MultiLayerOplog, MultiLayerOplogService, OplogArchive, OplogArchiveError, OplogArchiveResult,
    OplogArchiveService,
};
pub use primary::PrimaryOplogService;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedMutexGuard};

mod blob;
mod compressed;
mod ephemeral;
mod fenced_stream;
mod multilayer;
pub mod plugin;
mod primary;
pub mod rate_limited;
mod raw_session;
mod reader;

mod epoch;
mod extensions;
mod open;
mod payload;
mod scan;

pub(crate) use epoch::{
    epoch_record_service, epoch_record_verdict, record_epoch_verdict, record_owning_epoch,
    refuse_if_fenced,
};
pub use extensions::{
    OplogOps, OplogPayloadDownloadError, OplogServiceOps, RawOplogPayloadDownloadError,
};
pub use open::{
    OpenOplogs, OplogCloseCompletion, OplogConstructor, OplogLifecycleGuard, OplogShutdownHandle,
};
pub use payload::{
    DurableStreamBatchBuilder, DurableStreamOplogRecord, IndexedReservedStartBuilder,
    OrderedOplogStart, PendingUpload, ReserveGuard, ReservedPayload, ReservedRawStartBuilder,
};
#[cfg(test)]
use scan::{OplogScanState, SCAN_CURSOR_PREFIX, encode_scan_cursor};
pub(crate) use scan::{
    decode_scan_cursor, first_scan_cursor, next_scan_cursor, retry_scan_storage_op,
};

#[cfg(test)]
pub use ephemeral::tests::fixture as gated_ephemeral_fixture;

pub use reader::{OplogReadError, UnreadableOplog};

pub(crate) use reader::OplogReadSource;

#[cfg(test)]
pub(crate) use reader::{checked_range_end, exact_from_source};

#[cfg(test)]
pub mod tests;

/// The permission to publish one staged oplog as the target of a fork. It is used once: a
/// publication takes it by value, and it can be neither cloned nor copied. Only the filesystem
/// snapshot service makes one, so each publication of a fork goes through the decision of that
/// service about the snapshots of the stage.
pub struct StagePublication {
    stage_id: uuid::Uuid,
}

impl StagePublication {
    /// The permission to publish the stage `stage_id`.
    pub(crate) fn new(
        stage_id: uuid::Uuid,
        _key: crate::services::agent_filesystem_snapshots::PublicationKey,
    ) -> Self {
        Self { stage_id }
    }

    /// The stage that the permission publishes.
    pub(crate) fn stage_id(&self) -> uuid::Uuid {
        self.stage_id
    }

    /// The permission to publish the stage `stage_id`, for the tests of the oplog.
    #[cfg(test)]
    pub(crate) fn for_tests(stage_id: uuid::Uuid) -> Self {
        Self { stage_id }
    }
}

/// Whether an archive step returns once its transfer is queued or once the transfer has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveWait {
    Queued,
    /// Holds the agent's oplog lifecycle lock until the transfer finishes.
    Finished,
}

/// A top-level service for managing worker oplogs
///
/// For write access an oplog has to be opened with the `open` function (or if it doesn't exist,
/// created with the `create` function), which returns an implementation of the `Oplog` trait
/// providing synchronized access to the worker's oplog.
///
/// The following implementations are provided:
/// - `PrimaryOplogService` - based on the configured indexed storage, directly stores oplog entries.
///    This should always be the top-level implementation even in case of multi-layering.
/// - `CompressedOplogService` - uses the configured indexed storage, but stores oplog entries in
///    compressed chunks. Reads a whole chunk in memory when accessed. Should not be used on top level.
/// - `MultiLayerOplogService` - a service that can be used to stack multiple oplog services on each
///    other. Old entries are moved down the stack based on configurable conditions.
///
#[async_trait]
pub trait OplogService: Debug + Send + Sync {
    /// Locks cold lifecycle operations for the entire logical oplog stack.
    /// Wrappers delegate to their inner service; normal oplog operations do not take this lock.
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard;

    /// Installs the shared index after the complete oplog layer stack has been constructed.
    /// Primary actors need the index, but reconstruction must read through the outer service so
    /// archived entries and payloads remain visible. Constructing an index from primary storage
    /// alone would bypass those layers. The index therefore holds only a Weak reference back to
    /// the completed service; this dependency does not form an owning Arc cycle. Installation is
    /// single-shot so actors and worker-status persistence share the same index instance.
    fn set_stream_session_index(&self, index: Arc<StreamSessionIndexService>);

    fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>>;

    /// Creates an empty, hidden oplog with one writer and a fresh per-attempt stage id.
    /// It bypasses visible oplog caches, archives and derived session indexes. Payloads use
    /// the final agent's blob namespace so publication needs no payload rewrite.
    async fn create_staged(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _stage_id: uuid::Uuid,
        _initial_worker_metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        Err("staged oplogs are unsupported by this oplog service".to_string())
    }

    /// Checks whether one particular hidden stage still awaits publication.
    async fn staged_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<bool, String>;

    /// Publishes a fully committed stage, the stage of `publication`, if no primary oplog exists.
    /// The caller must stop and drop its staged writer first. `false` means a competing target
    /// exists; errors may have indeterminate outcomes and must be reconciled using the target's
    /// fork provenance.
    async fn publish_staged(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _publication: StagePublication,
        _expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        Err("staged oplogs are unsupported by this oplog service".to_string())
    }

    /// Removes only this attempt's hidden index, never the target's shared payload namespace.
    async fn discard_staged(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        Err("staged oplogs are unsupported by this oplog service".to_string())
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError>;

    /// Creates an oplog whose absence has already been established by the caller.
    ///
    /// Implementations may use this guarantee to initialize storage cursors and counters from
    /// the empty state without probing persistence. Callers must not use this for identities that
    /// may already have an oplog.
    async fn create_fresh(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError>;

    /// Opens an existing oplog for the given worker.
    ///
    /// `last_oplog_index` controls how the oplog's internal write cursor is initialized:
    /// - `None` — the implementation resolves the last index from storage at construction time.
    ///   This is the recommended default for production callers, as it avoids TOCTOU races
    ///   between reading the index and opening the oplog.
    /// - `Some(idx)` — uses the provided index. This is intended for outer layers (e.g.
    ///   `MultiLayerOplogService`) that have already resolved the correct global last index
    ///   across all layers and need to pass it down to inner layers.
    async fn open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError>;

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog>;

    /// The last index for archive maintenance: every storage failure is returned, not only an
    /// unreadable archive.
    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        Ok(self
            .get_last_index(owned_agent_id, agent_mode)
            .await
            .map_err(|error| error.to_string())?)
    }

    /// Deletes the agent's oplog, in every layer. With `expected_epoch` - the epoch the caller's
    /// own handle asserts - only while that is still the epoch recorded for the oplog and this
    /// executor recorded it: otherwise nothing is deleted and the delete is refused with
    /// [`OplogError::Fenced`], as a write at that epoch would be. `None` is an ephemeral oplog,
    /// deleted unconditionally: nothing fences it or the archive layers behind it.
    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError>;

    /// Confirms `expected_epoch` still owns the agent's oplog without writing anything else: the
    /// compare-and-set an open makes, at the epoch the caller already holds. Refused with
    /// [`OplogError::Fenced`] once another executor's epoch is recorded, so state the oplog does not
    /// carry is removed only while the oplog is still this executor's.
    async fn assert_owning_epoch(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: ShardEpoch,
    ) -> Result<(), OplogError>;

    /// Reads exactly `n` contiguous entries starting at `idx`.
    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>;

    /// Reads the part of the requested range physically present in this service.
    ///
    /// Composite services use this to fold their sources. Standalone services use the exact
    /// logical read by default.
    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_exact(owned_agent_id, agent_mode, idx, n).await
    }

    /// Checks whether the oplog exists in the oplog, without opening it
    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog>;

    /// Existence for archive maintenance: every storage failure is returned, not only an
    /// unreadable archive.
    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, String> {
        Ok(self
            .exists(owned_agent_id, agent_mode)
            .await
            .map_err(|error| error.to_string())?)
    }

    /// Scans the oplog for all workers belonging to the given component, in a paginated way.
    ///
    /// `modes` selects which agent modes to scan. `Some(mode)` scans only that mode;
    /// `None` scans both modes (durable and ephemeral). When scanning multiple modes the
    /// active mode is encoded into the returned `ScanCursor` so the caller resumes the same
    /// pagination correctly.
    ///
    /// Pages can be empty. This operation is slow and is not locking the oplog.
    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError>;

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String>;

    async fn download_raw_payload_classified(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.download_raw_payload(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
            .map_err(|message| RawOplogPayloadDownloadError::Backend(anyhow::Error::msg(message)))
    }
}

/// Level of commit guarantees
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CommitLevel {
    /// Always commit immediately and do not return until it is done
    Always,
    /// Flush and report entries, allowing ephemeral storage writes to finish asynchronously.
    /// Durable oplogs still wait for persistence. Explicit protocol barriers use `Always`.
    Deferred,
    /// Only commit immediately if the worker is durable
    DurableOnly,
    /// Commit nothing: only report the entries the oplog committed by itself since the last
    /// report, on its commit threshold or a replica wait. Buffered entries stay buffered.
    ReportOnly,
}

/// How many batches of entries that an oplog committed by itself it keeps in memory for the next
/// report. Beyond it, the primary oplog drops its oldest batch, which the report reads back from
/// storage, and the ephemeral oplog drops a later one, which leaves a gap for the status fold.
pub(crate) const MAX_RETAINED_RECEIPT_BATCHES: usize = 32;

/// The callback an oplog calls after it committed entries by itself, so that their owner can
/// collect them with [`CommitLevel::ReportOnly`]. The oplog holds it weakly and does not call it
/// after its owner dropped it.
pub(crate) type AutoCommitCallback = dyn Fn() + Send + Sync;

#[derive(Clone, Default)]
pub(crate) struct AutoCommitListener(Arc<std::sync::Mutex<Option<Weak<AutoCommitCallback>>>>);

impl AutoCommitListener {
    fn set(&self, listener: Weak<AutoCommitCallback>) {
        *self.0.lock().unwrap() = Some(listener);
    }

    /// Calls the listener. Returns false when there is no live listener, so nobody will collect
    /// the entries of the commit with [`CommitLevel::ReportOnly`] soon.
    fn notify(&self) -> bool {
        let listener = self.0.lock().unwrap().as_ref().and_then(Weak::upgrade);
        match listener {
            Some(listener) => {
                listener();
                true
            }
            None => false,
        }
    }
}

/// Why an oplog write was refused by the storage: the shard epoch this executor asserted is
/// behind the one recorded for the oplog, because another executor owns the shard now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OplogFence {
    pub agent_id: AgentId,
    pub expected_epoch: ShardEpoch,
    pub actual_epoch: Option<ShardEpoch>,
}

impl OplogFence {
    /// The fence a refused storage write reports for `agent_id`. Only a fence comes back from
    /// the fenceable storage retries: every other storage failure panics inside them.
    pub(crate) fn refused(agent_id: AgentId, error: IndexedStorageError) -> Self {
        match error {
            IndexedStorageError::Fenced {
                expected, actual, ..
            } => OplogFence {
                agent_id,
                expected_epoch: expected,
                actual_epoch: actual,
            },
            other => unreachable!("a fenceable storage write failed without a fence: {other}"),
        }
    }
}

/// Why an oplog operation failed without taking the executor down.
///
/// A `Fenced` write is not a storage failure - the storage is healthy and refused the write on
/// purpose - so it is returned rather than retried or panicked on, and the worker that hit it is
/// stopped and left to the shard's new owner. Storage failures on execution-critical reads and
/// writes keep their fail-stop semantics inside the oplog implementation. Fallible archive
/// maintenance returns `Maintenance`, allowing the fenced cleanup to be retried without stopping
/// the executor. `Payload` is an entry whose payload the caller-supplied builder could not produce.
/// `ArchiveWrite` is a write to an archive layer that failed after its retries: it is latched, so
/// the agent stops, and only that agent. `Unreadable` is a commit whose entries were stored, but
/// whose status could not be rebuilt because the oplog does not read: the agent stops with the
/// cause, and only that agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OplogError {
    Fenced(OplogFence),
    Payload(String),
    Maintenance(String),
    ArchiveWrite(ArchiveWriteFailure),
    Unreadable(UnreadableOplog),
}

/// A write of the oplog of `agent_id` to the archive layer `level` that failed after its retries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveWriteFailure {
    pub agent_id: AgentId,
    pub level: usize,
    pub details: String,
}

impl Display for ArchiveWriteFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to write oplog data of {} to archive layer {}: {}",
            self.agent_id, self.level, self.details
        )
    }
}

/// An archive layer that did not store the oplog fails the agent for good.
impl From<ArchiveWriteFailure> for WorkerExecutorError {
    fn from(failure: ArchiveWriteFailure) -> Self {
        WorkerExecutorError::oplog_failed(
            failure.agent_id,
            format!(
                "failed to write oplog data to archive layer {}: {}",
                failure.level, failure.details
            ),
        )
    }
}

impl OplogError {
    /// The failure of the agent that this error is: an archive layer that did not store the oplog,
    /// or an oplog that does not read. It is `OplogFailed` for good, or `RecoveryRequired` when a
    /// later read can pass. A fence, which moves the agent to the new owner of its shard, and a
    /// write that failed alone give `None`.
    pub fn agent_failure(&self) -> Option<WorkerExecutorError> {
        match self {
            Self::ArchiveWrite(failure) => Some(failure.clone().into()),
            Self::Unreadable(unreadable) => Some(unreadable.clone().into()),
            Self::Fenced(_) | Self::Payload(_) | Self::Maintenance(_) => None,
        }
    }
}

impl From<String> for OplogError {
    fn from(details: String) -> Self {
        OplogError::Payload(details)
    }
}

impl From<OplogError> for WorkerExecutorError {
    fn from(error: OplogError) -> Self {
        match error {
            OplogError::Fenced(fence) => WorkerExecutorError::oplog_fenced(
                fence.agent_id,
                fence.expected_epoch.0,
                fence.actual_epoch.map(|epoch| epoch.0),
            ),
            OplogError::Payload(details) => WorkerExecutorError::runtime(details),
            OplogError::Maintenance(details) => WorkerExecutorError::runtime(details),
            OplogError::ArchiveWrite(failure) => WorkerExecutorError::from(failure),
            OplogError::Unreadable(unreadable) => WorkerExecutorError::from(unreadable),
        }
    }
}

impl Display for OplogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            OplogError::Fenced(fence) => write!(
                f,
                "oplog write for {} fenced: asserted shard epoch {}, stored {}",
                fence.agent_id,
                fence.expected_epoch,
                fence
                    .actual_epoch
                    .map(|epoch| epoch.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
            OplogError::Payload(details) => write!(f, "oplog payload error: {details}"),
            OplogError::Maintenance(details) => write!(f, "oplog maintenance error: {details}"),
            OplogError::ArchiveWrite(failure) => write!(f, "{failure}"),
            OplogError::Unreadable(unreadable) => write!(f, "{unreadable}"),
        }
    }
}

impl std::error::Error for OplogError {}

/// A write after which the agent cannot go on. Every other `OplogError` stays fail-stop.
///
/// `Fenced` (the shard has a new owner) and `ArchiveWrite` (an archive layer of an ephemeral oplog
/// did not store its entries after its retries) are latched on the oplog: every later add and
/// commit fails the same way, so a path that cannot report the refusal leaves it to the next write
/// that can.
///
/// `Unreadable` is not latched. Only the commit of the status task gives it, after the entries of
/// that commit were stored, when the status could not be rebuilt because the oplog does not read.
/// A later write can succeed, so the caller that receives it fails the agent with the cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OplogRefusal {
    Fenced(OplogFence),
    ArchiveWrite(ArchiveWriteFailure),
    Unreadable(UnreadableOplog),
}

impl OplogRefusal {
    /// The fence, when the refusal means that the shard has a new owner.
    pub fn fence(&self) -> Option<&OplogFence> {
        match self {
            Self::Fenced(fence) => Some(fence),
            Self::ArchiveWrite(_) | Self::Unreadable(_) => None,
        }
    }
}

impl OplogError {
    /// Splits the refusal off a write error; every other error is returned as it is.
    pub fn into_refusal(self) -> Result<OplogRefusal, OplogError> {
        match self {
            Self::Fenced(fence) => Ok(OplogRefusal::Fenced(fence)),
            Self::ArchiveWrite(failure) => Ok(OplogRefusal::ArchiveWrite(failure)),
            Self::Unreadable(unreadable) => Ok(OplogRefusal::Unreadable(unreadable)),
            error @ (Self::Payload(_) | Self::Maintenance(_)) => Err(error),
        }
    }
}

impl From<OplogRefusal> for OplogError {
    fn from(refusal: OplogRefusal) -> Self {
        match refusal {
            OplogRefusal::Fenced(fence) => Self::Fenced(fence),
            OplogRefusal::ArchiveWrite(failure) => Self::ArchiveWrite(failure),
            OplogRefusal::Unreadable(unreadable) => Self::Unreadable(unreadable),
        }
    }
}

impl From<OplogRefusal> for WorkerExecutorError {
    fn from(refusal: OplogRefusal) -> Self {
        OplogError::from(refusal).into()
    }
}

impl Display for OplogRefusal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", OplogError::from(self.clone()))
    }
}

/// A single oplog append that has already been synchronously enqueued in the oplog's ordering
/// domain. Creating this receipt reserves the entry's position; awaiting it returns the assigned
/// index after the append finishes.
pub type OplogAddReceipt = BoxFuture<'static, Result<OplogIndex, OplogError>>;
pub type OplogAddPairReceipt = BoxFuture<'static, Result<(OplogIndex, OplogIndex), OplogError>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawDurableStreamSessionStatus {
    pub watermark: OplogIndex,
    pub status: Result<Option<DurableStreamSessionStatus>, String>,
}

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Any + Debug + Send + Sync {
    /// A non-owning, exact-generation executor-shutdown capability. The synchronous fence is
    /// installed before executor-owned tasks are cancelled; after those tasks stop, the close
    /// action joins every captured oplog layer even if the outer handle has already disappeared.
    fn executor_shutdown_handle(&self) -> OplogShutdownHandle {
        self.inner().map_or_else(
            || OplogShutdownHandle::from_completion(self.closed()),
            |inner| inner.executor_shutdown_handle(),
        )
    }

    /// Retires this open handle after its worker's durable state has been deleted.
    ///
    /// Cached implementations unregister the exact handle and propagate retirement through
    /// wrapper layers. The retired object may remain alive through stale worker references, but
    /// it must no longer be returned when a new worker with the same identity opens its oplog.
    fn retire(&self) {}

    /// Consulted only while holding the cold lifecycle lock, never on the append/read path.
    fn is_retired(&self) -> bool {
        false
    }

    /// Completion of this layer's owned work after its last handle is dropped or retired.
    fn closed(&self) -> OplogCloseCompletion {
        futures::future::ready(Ok(())).boxed().shared()
    }

    /// Root tasks share the open oplog's lifetime even when its cached worker shell changes.
    /// Wrappers delegate this reference to their leaf; in-memory test oplogs need no owner.
    fn task_owner(&self) -> Option<&WorkerTasks> {
        None
    }

    /// Stops owned work without removing persisted history. Callers first stop/drain users and
    /// hold the logical lifecycle guard until work finishes and storage removal completes.
    /// Every layer is joined even when cleanup reports an error.
    async fn stop_and_wait(&self) -> Result<(), String> {
        let tasks_result = if let Some(tasks) = self.task_owner() {
            tasks.stop_and_wait().await
        } else {
            Ok(())
        };
        self.retire();
        let result = self.closed().await;
        if let Some(inner) = self.inner() {
            let inner_result = inner.stop_and_wait().await;
            return tasks_result.and(result).and(inner_result);
        }
        tasks_result.and(result)
    }

    /// Adds a single entry to the oplog (possibly buffered), and returns its index
    async fn add(&self, entry: OplogEntry) -> Result<OplogIndex, OplogError> {
        self.enqueue_add(entry).await
    }

    /// Synchronously enqueues a single entry before returning its asynchronous completion receipt.
    ///
    /// This is used at synchronous guest-observation boundaries where deferring the enqueue until
    /// an async task is polled would allow later guest operations to overtake the observed event.
    /// Replayable implementations and wrapper layers must reserve the entry in the same ordering
    /// domain as [`Self::add`] before this method returns; they must not implement it by merely
    /// boxing an unpolled call to `add`.
    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt;

    /// Atomically appends producer-stream records built from the first assigned index. A production
    /// leaf externalizes each large record before committing any entry, and commit-threshold checks
    /// run only after the complete batch is buffered. The checked default is for serialized test
    /// oplogs only.
    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        let first_index = self.current_oplog_index().await.next();
        let records = make_batch(first_index);
        let mut result = Vec::with_capacity(records.len());
        for record in records {
            let expected_index = result
                .last()
                .map_or(first_index, |(index, _): &(OplogIndex, OplogEntry)| {
                    index.next()
                });
            let entry = record.into_inline_entry();
            let index = self.add(entry.clone()).await?;
            assert_eq!(
                index, expected_index,
                "oplog add_durable_stream_batch default observed a concurrent writer"
            );
            result.push((index, entry));
        }
        Ok(result)
    }

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the layer below this one
    ///
    /// Returns the number of dropped entries.
    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64;

    async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, String> {
        Ok(self.drop_prefix(last_dropped_id).await)
    }

    /// Commits the buffered entries to the oplog
    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError>;

    /// Registers the callback that the leaf oplog calls after it committed entries by itself.
    /// The owner then collects the entries with [`CommitLevel::ReportOnly`]. A new registration
    /// replaces the earlier one.
    fn set_auto_commit_listener(&self, listener: Weak<AutoCommitCallback>) {
        if let Some(inner) = self.inner() {
            inner.set_auto_commit_listener(listener);
        }
    }

    /// Returns the current oplog index
    async fn current_oplog_index(&self) -> OplogIndex;

    async fn try_current_oplog_index(&self) -> Result<OplogIndex, String> {
        Ok(self.current_oplog_index().await)
    }

    /// Returns actor-ordered lifecycle metadata including buffered raw appends. Absence is proven
    /// through the returned watermark; storage failures must not be reported as absence.
    async fn raw_durable_stream_session_status(
        &self,
        _session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> RawDurableStreamSessionStatus {
        RawDurableStreamSessionStatus {
            watermark: self.current_oplog_index().await,
            status: Err("raw stream session metadata is unavailable".into()),
        }
    }

    /// Returns the index of the last non-hint entry which was added in this session with `add`. If
    /// there is no such entry, returns `None`.
    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex>;

    /// Waits until indexed store writes all changes into at least `replicas` replicas (or the maximum
    /// available).
    /// Returns true if the maximum possible number of replicas is reached within the timeout,
    /// otherwise false.
    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<bool, OplogError>;

    /// Reads exactly `n` contiguous entries starting at `oplog_index`.
    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>;

    /// Reads the part of the requested range physically present in this oplog.
    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_exact(oplog_index, n).await
    }

    /// The part of the range present in this oplog, for archive maintenance: every storage failure
    /// is returned, not only an unreadable archive.
    async fn try_read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        Ok(self
            .read_source(oplog_index, n)
            .await
            .map_err(|error| error.to_string())?)
    }

    /// Reads the entry at the given oplog index.
    async fn read(&self, oplog_index: OplogIndex) -> Result<OplogEntry, UnreadableOplog> {
        Ok(self
            .read_exact(oplog_index, 1)
            .await?
            .remove(&oplog_index)
            .unwrap_or_else(|| {
                panic!("Missing oplog entry {oplog_index} after an exact single-entry read")
            }))
    }

    /// Gets the total number of entries in the oplog
    async fn length(&self) -> Result<u64, UnreadableOplog>;

    /// The length for archive maintenance: every storage failure is returned, not only an
    /// unreadable archive.
    async fn try_length(&self) -> Result<u64, String> {
        Ok(self.length().await.map_err(|error| error.to_string())?)
    }

    /// Adds an entry to the oplog and immediately commits it
    async fn add_and_commit(&self, entry: OplogEntry) -> Result<OplogIndex, OplogError> {
        let index = self.add(entry).await?;
        self.commit(CommitLevel::Always).await?;
        Ok(index)
    }

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String>;

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.download_raw_payload(payload_id, md5_hash)
            .await
            .map_err(|message| RawOplogPayloadDownloadError::Backend(anyhow::Error::msg(message)))
    }

    /// Reserves a reference for a (possibly large) `serialized_request` payload, builds the call's
    /// `Start` from that reference with the **synchronous** `build_start`, and appends it — all so
    /// that the `Start` is ordered (its index assigned) in initiation order *before* the
    /// (potentially slow, big) request upload finishes.
    ///
    /// This is the single ordering primitive for concurrent durable host calls. Its contract is
    /// that, for any concurrent calls reaching it, the order in which their `Start` entries are
    /// assigned indices matches the order in which they entered this method — so replay sees a
    /// deterministic interleaving. Leaf implementations guarantee this by reserving the payload,
    /// running `build_start`, and assigning the index within a single non-yielding step of their
    /// actor's job handler, with **no `.await` in between** (a deferred upload is *started* there
    /// but not awaited). Wrapper implementations must delegate without introducing an `.await`
    /// before the inner call assigns the index (e.g. rate-limiting back-pressure must happen
    /// *after* delegation, not before).
    ///
    /// The initiation-order guarantee is scoped to **replayable** (durable) oplogs — it exists
    /// solely so replay can deterministically match concurrent calls back to their `Start`
    /// entries. The ephemeral oplog is never replayed, so it deliberately does not provide the
    /// guarantee: it uploads the payload eagerly (awaiting it) before appending the `Start`, and
    /// concurrent calls' `Start` entries may interleave in upload-completion order. Do not copy
    /// the ephemeral implementation into a replayable oplog, and do not "fix" the ephemeral one
    /// by adding ordering machinery it cannot need.
    ///
    /// For replayable oplogs this method is `async` only because it round-trips through the leaf
    /// oplog's actor and/or delegates through wrapper layers — *not* because it awaits the upload
    /// (only the ephemeral implementation awaits its eager upload here). The returned
    /// [`OrderedOplogStart::pending_upload`] tracks the request blob's durable write; the caller
    /// must [`PendingUpload::wait`] on it before appending the matching `End`/`Cancelled`, with the
    /// leaf oplog's `append` commit barrier as the backstop (so no committed entry references a
    /// not-yet-written blob).
    ///
    /// This is a required method (no default) deliberately: a default `reserve` + `add` composition
    /// would reintroduce an `.await` between reserving and ordering the `Start`, silently breaking
    /// the determinism contract for any implementor that forgot to override it.
    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError>;

    /// Like [`Self::add_start_with_reserved_raw_payload`], but builds the request after the leaf
    /// oplog has assigned the exact `Start` index. The leaf must invoke `build_request` and append
    /// the resulting `Start` in the same serialized writer step, so the supplied index is exactly
    /// the one returned in [`OrderedOplogStart`].
    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError>;

    /// Synchronously enqueues an atomic `Start` and second entry (its `End` or
    /// `Cancelled`) that references the `Start`'s `OplogIndex`, returning their asynchronous
    /// completion receipt.
    ///
    /// `make_second` builds the second entry from the freshly assigned `Start`
    /// index (a durable call is identified by the `OplogIndex` of its `Start`).
    /// Used by the sequential adapter (the p2 durability path, see
    /// [`OplogOps::add_completed_host_call`]) to write a matched host-call
    /// `Start`/`End` pair atomically.
    ///
    /// Implementations must reserve one writer job before returning, in the same ordering domain
    /// as [`Self::enqueue_add`]. Dropping the receipt must not cancel that job. No other writer may
    /// interleave between the two appends and no commit threshold check may fire between them, so
    /// the pair is never split across a commit/crash boundary. This is a
    /// required method (no default) deliberately: a default `add`-twice
    /// composition would silently violate that atomicity for any implementor
    /// that forgot to override it.
    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> OplogAddPairReceipt;

    /// Async convenience for [`Self::enqueue_add_pair`].
    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> Result<(OplogIndex, OplogIndex), OplogError> {
        self.enqueue_add_pair(start, make_second).await
    }

    /// The shard epoch this oplog's primary writes assert, or `None` without a primary oplog. An
    /// ephemeral oplog has none: its archive layers assert the epoch it was opened with, and a
    /// refusal there is reported by [`Self::fence`].
    ///
    /// Only the primary oplog knows it, so a wrapper answers from the oplog it wraps.
    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.inner().and_then(|inner| inner.shard_epoch())
    }

    /// The refusal this oplog has latched, if the storage has turned one of its writes away:
    /// every later write fails on it, so the handle is finished. Answered without a round trip,
    /// so the open-oplog cache can decline to hand a finished handle to a new opener, and a write
    /// known to be refused is not started: a remote side effect, or the plugin forwarder's flush
    /// and checkpoint.
    fn fence(&self) -> Option<OplogFence> {
        self.inner().and_then(|inner| inner.fence())
    }

    /// Returns the inner oplog wrapped by this implementation, if any.
    /// Wrapper oplogs should override this to enable generic traversal of the
    /// oplog composition chain (used by `downcast_oplog`).
    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        None
    }
}

pub(crate) fn downcast_oplog<T: Oplog>(oplog: &Arc<dyn Oplog>) -> Option<Arc<T>> {
    let mut current = oplog.clone();
    loop {
        if current.deref().type_id() == TypeId::of::<T>() {
            let raw: *const dyn Oplog = Arc::into_raw(current);
            let raw: *const T = raw.cast();
            return Some(unsafe { Arc::from_raw(raw) });
        }
        {
            let inner = current.inner()?;
            current = inner
        }
    }
}
