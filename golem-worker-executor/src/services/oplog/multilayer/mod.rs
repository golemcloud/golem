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

use crate::metrics::storage::{
    STORAGE_TYPE_OPLOG_ARCHIVE, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::model::ExecutionStatus;
use crate::services::oplog::ephemeral::EphemeralOplog;
use crate::services::oplog::multilayer::BackgroundTransferMessage::{
    TransferFromLower, TransferFromPrimary,
};
use crate::services::oplog::reader::{OplogRead, OplogReadError, OplogReadSource, UnreadableOplog};
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, OpenOplogs, Oplog,
    OplogAddReceipt, OplogCloseCompletion, OplogConstructor, OplogError, OplogFence,
    OplogLifecycleGuard, OplogService, OrderedOplogStart, RawOplogPayloadDownloadError,
    ReservedRawStartBuilder, StagePublication, decode_scan_cursor, downcast_oplog,
    first_scan_cursor,
};
use crate::storage::indexed::{IndexedStorageError, IndexedStorageMetaNamespace};
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::model::ShardEpoch;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{
    AtomicOplogIndex, OplogEntry, OplogIndex, PayloadId, RawOplogPayload,
};
use golem_common::model::{AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor};
use golem_common::read_only_lock;
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use nonempty_collections::NEVec;
use std::cmp::min;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::Sender;
use tracing::{Instrument, Level, debug, info, warn};

pub(crate) type TransferFiber = Arc<Mutex<TransferFiberState>>;

/// A failed index or existence read of the archive `level` of the agent.
fn unreadable_archive(
    owned_agent_id: &OwnedAgentId,
    level: usize,
    error: OplogArchiveError,
) -> UnreadableOplog {
    UnreadableOplog::new(
        &owned_agent_id.agent_id,
        OplogReadError::of_archive(OplogReadSource::Archive(level), error),
    )
}

/// A failed call to an archive layer, and whether a later call can pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OplogArchiveError {
    /// The storage did not answer, after its retries. A later call can pass.
    Unavailable(String),
    /// Stored archive data does not decode.
    Corruption(String),
    /// Any other failure. A later call gives the same answer.
    Failed(String),
}

impl OplogArchiveError {
    /// The failure of an indexed storage call, described by `details`. A transient or
    /// indeterminate error is `Unavailable`; every other error is `Failed`.
    pub fn of_indexed(error: &IndexedStorageError, details: impl Into<String>) -> Self {
        match error {
            IndexedStorageError::Transient(_) | IndexedStorageError::Indeterminate(_) => {
                Self::Unavailable(details.into())
            }
            IndexedStorageError::Conflict(_)
            | IndexedStorageError::InvalidResume(_)
            | IndexedStorageError::Other(_)
            | IndexedStorageError::Fenced { .. } => Self::Failed(details.into()),
        }
    }

    /// The failure of a blob storage call, described by `details`, as
    /// [`golem_service_base::storage::blob::BlobFailure`] classifies it. A transient error, and a
    /// storage that refuses the call until an operator fixes it, is `Unavailable`: the oplog data
    /// itself is not at fault, so a later call can pass. A permanent error is `Failed`.
    pub fn of_blob(error: &anyhow::Error, details: impl Into<String>) -> Self {
        use golem_service_base::storage::blob::BlobFailure;
        match BlobFailure::of(error) {
            BlobFailure::Transient | BlobFailure::Refused(_) => Self::Unavailable(details.into()),
            BlobFailure::Permanent => Self::Failed(details.into()),
        }
    }

    /// The same failure, with its description given by `describe`.
    pub fn map_details(self, describe: impl FnOnce(String) -> String) -> Self {
        match self {
            Self::Unavailable(details) => Self::Unavailable(describe(details)),
            Self::Corruption(details) => Self::Corruption(describe(details)),
            Self::Failed(details) => Self::Failed(describe(details)),
        }
    }

    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

impl Display for OplogArchiveError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(details) | Self::Corruption(details) | Self::Failed(details) => {
                f.write_str(details)
            }
        }
    }
}

impl std::error::Error for OplogArchiveError {}

/// A read that did not give its entries, keeping whether a later read can pass.
impl From<OplogReadError> for OplogArchiveError {
    fn from(error: OplogReadError) -> Self {
        if error.is_transient() {
            Self::Unavailable(error.to_string())
        } else if matches!(error, OplogReadError::Corruption { .. }) {
            Self::Corruption(error.to_string())
        } else {
            Self::Failed(error.to_string())
        }
    }
}

pub type OplogArchiveResult<T> = Result<T, OplogArchiveError>;
type TransferFibers = Arc<Mutex<HashMap<AgentId, Weak<Mutex<TransferFiberState>>>>>;
type ArchiveFailures = Arc<Mutex<HashMap<(OwnedAgentId, ArchiveSource), ArchiveFailureState>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ArchiveSource {
    Primary,
    Lower(usize),
}

const ARCHIVE_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
const ARCHIVE_RETRY_MAX_DELAY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy)]
struct ArchiveFailureState {
    failures: u32,
    retry_at: Instant,
}

fn archive_retry_delay(failures: u32) -> Duration {
    ARCHIVE_RETRY_BASE_DELAY
        .saturating_mul(1 << failures.saturating_sub(1).min(9))
        .min(ARCHIVE_RETRY_MAX_DELAY)
}

pub(crate) struct TransferFiberState {
    transfer_fiber: Option<tokio::task::AbortHandle>,
    closed: Option<OplogCloseCompletion>,
    cancelled: bool,
}

pub(crate) fn new_transfer_fiber() -> TransferFiber {
    Arc::new(Mutex::new(TransferFiberState {
        transfer_fiber: None,
        closed: None,
        cancelled: false,
    }))
}

#[async_trait]
pub trait OplogArchiveService: Debug + Send + Sync {
    /// Opens an oplog archive for reading and writing.
    ///
    /// With `Some(shard_epoch)` the archive records that epoch as its writer generation, as the
    /// primary oplog does at open, and asserts it on every write; a newer owner's record refuses
    /// them. `None` records and asserts nothing: for read-only opens, and for executors without a
    /// shard assignment.
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        shard_epoch: Option<ShardEpoch>,
    ) -> OplogArchiveResult<Arc<dyn OplogArchive + Send + Sync>>;

    /// Opens a new, known-empty archive without probing persistent storage. The epoch as for
    /// [`Self::open`].
    async fn open_fresh(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        shard_epoch: Option<ShardEpoch>,
    ) -> OplogArchiveResult<Arc<dyn OplogArchive + Send + Sync>>;

    /// Deletes the oplog archive for a worker completely, together with any writer generation
    /// recorded for it.
    async fn delete(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<()>;

    /// Reads the entries physically present in this archive within the requested range.
    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError>;

    /// Checks if an oplog archive exists for a worker
    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<bool>;

    /// Scans the oplog archive for all workers belonging to the given component, in a paginated way.
    ///
    /// `modes` selects which agent modes to scan. `Some(mode)` scans only that mode;
    /// `None` scans both modes (durable and ephemeral). When scanning multiple modes the
    /// active mode is encoded into the returned `ScanCursor` so the caller resumes the same
    /// pagination correctly.
    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError>;

    /// Gets the last stored oplog entry's id in the archive
    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<OplogIndex>;

    /// The meta-namespace whose keys list every agent this archive holds entries for, or `None`
    /// when the storage cannot list them, as for blob-backed archives.
    fn scan_namespace(&self, _agent_mode: AgentMode) -> Option<IndexedStorageMetaNamespace> {
        None
    }
}

/// Interface for secondary oplog archives - requires less functionality than the primary archive
#[async_trait]
pub trait OplogArchive: Debug {
    /// Reads the entries physically present in this archive within the requested range.
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError>;

    /// Append a new chunk of entries to the oplog.
    /// Returns the number of compressed bytes written to storage, `OplogError::Fenced` when a newer
    /// owner recorded its epoch on this archive, or `OplogError::Maintenance` for a storage failure
    /// the archive transfer retries later. A refusal latches, so every later write through this
    /// handle is refused too.
    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> Result<u64, OplogError>;

    /// Verifies that transferred entries can be read from persistent storage without consulting
    /// this archive handle's cache.
    async fn verify_persisted(
        &self,
        entries: &[(OplogIndex, OplogEntry)],
    ) -> OplogArchiveResult<()>;

    /// Gets the last appended chunk's last index
    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex>;

    /// Drop a chunk of entries from the beginning of the oplog. Returns the number of entries
    /// dropped, or an error as for [`Self::append`], dropping nothing when refused.
    ///
    /// This should only be called _after_ `append` succeeded in the archive below this one
    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError>;

    /// Gets the total number of entries in this oplog archive
    async fn length(&self) -> OplogArchiveResult<u64>;

    /// Gets the last index in this oplog archive
    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex>;

    /// The refusal this archive has latched, if a newer owner's generation turned one of its
    /// writes away. Every later write through this handle fails on it.
    fn fence(&self) -> Option<OplogFence> {
        None
    }
}

/// The first refusal any of `layers` has latched.
pub(crate) fn layers_fence(
    layers: &NEVec<Arc<dyn OplogArchive + Send + Sync>>,
) -> Option<OplogFence> {
    layers.iter().find_map(|layer| layer.fence())
}

/// Wraps an `OplogArchive` to record storage metrics on writes.
#[derive(Debug)]
pub struct InstrumentedOplogArchive {
    inner: Arc<dyn OplogArchive + Send + Sync>,
    account_id: AccountId,
    environment_id: EnvironmentId,
}

#[derive(Debug)]
pub struct MultiLayerOplogService {
    pub primary: Arc<dyn OplogService>,
    pub lower: NEVec<Arc<dyn OplogArchiveService>>,

    oplogs: OpenOplogs,
    transfer_fibers: TransferFibers,
    archive_failures: ArchiveFailures,

    entry_count_limit: u64,
    max_operations_before_commit_ephemeral: u64,
}

#[derive(Clone)]
struct CreateOplogConstructor {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    initial_entry: Option<OplogEntry>,
    primary: Arc<dyn OplogService>,
    service: MultiLayerOplogService,
    last_oplog_index: Option<OplogIndex>,
    fresh: bool,
    initial_worker_metadata: AgentMetadata,
    last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
    execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    shard_epoch: Option<ShardEpoch>,
}

pub struct MultiLayerOplog {
    owned_agent_id: OwnedAgentId,
    #[allow(dead_code)] // retained for diagnostics and future use
    agent_mode: AgentMode,
    primary: Arc<dyn Oplog>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    retired: Arc<AtomicBool>,
    multi_layer_oplog_service: MultiLayerOplogService,
    transfer_fiber: TransferFiber,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    last_reported_commit_index: AtomicOplogIndex,
    last_transfer_point: AtomicOplogIndex,
    close_fn: Mutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

#[derive(Debug)]
pub enum BackgroundTransferMessage {
    TransferFromPrimary {
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog>>,
        done: Option<Sender<Result<(), OplogError>>>,
        transfer_origin: TraceOrigin,
    },
    TransferFromLower {
        source: usize,
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog>>,
        done: Option<Sender<Result<(), OplogError>>>,
        drain: bool,
        transfer_origin: TraceOrigin,
    },
}

/// Wraps an open oplog archive to track the number of items written and automatically
/// scheduling transfers to lower levels when the limit is reached
#[derive(Debug)]
pub struct WrappedOplogArchive {
    layer: usize,
    archive: Arc<dyn OplogArchive + Send + Sync>,
    entry_count: AtomicU64,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    owned_agent_id: OwnedAgentId,
    multi_layer_oplog_service: MultiLayerOplogService,
    entry_count_limit: u64,
}

struct BackgroundTransferFromPrimary {
    last_transferred_idx: OplogIndex,
    primary: Arc<dyn Oplog>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
}

struct BackgroundTransferBetweenLowers {
    last_transferred_idx: OplogIndex,
    source_layer: Arc<dyn OplogArchive + Send + Sync>,
    target_layer: Arc<dyn OplogArchive + Send + Sync>,
}

mod archive;
mod constructor;
mod handle;
mod service;
mod transfer;

use transfer::BackgroundTransfer;
pub(crate) use transfer::transfer_between_lower_layers;

#[cfg(test)]
#[path = "tests.rs"]
mod transfer_lifecycle_tests;
