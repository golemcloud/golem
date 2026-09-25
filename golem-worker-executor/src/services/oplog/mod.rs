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

use crate::model::ExecutionStatus;
use crate::services::stream_session_index::StreamSessionIndexService;
use crate::storage::indexed::{IndexedStorageError, ScanResume};
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
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostResponse, OplogEntry, OplogIndex, OplogPayload,
    PayloadId, RawOplogPayload, UpdateDescription,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationResult, AgentMetadata, AgentStatusRecord,
    DurableStreamSessionStatus, OwnedAgentId, ScanCursor, ShardEpoch, Timestamp,
};
use golem_common::read_only_lock;
use golem_common::retries::get_delay;
use golem_common::serialization::serialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use serde::{Deserialize, Serialize};

pub use ephemeral::EphemeralOplog;
pub use multilayer::{MultiLayerOplog, MultiLayerOplogService, OplogArchive, OplogArchiveService};
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
mod multilayer;
pub mod plugin;
mod primary;
pub mod rate_limited;
mod raw_session;
mod reader;

#[cfg(test)]
pub(crate) use reader::{OplogReadSource, checked_range_end, exact_from_source, fail_stop};

#[cfg(test)]
pub mod tests;

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

    /// Publishes a fully committed stage if no primary oplog exists. The caller must stop
    /// and drop its staged writer first. `false` means a competing target exists; errors may
    /// have indeterminate outcomes and must be reconciled using the target's fork provenance.
    async fn publish_staged(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _stage_id: uuid::Uuid,
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
    ) -> Arc<dyn Oplog>;

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
    ) -> Arc<dyn Oplog>;

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
    ) -> Arc<dyn Oplog>;

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex;

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
    ) -> BTreeMap<OplogIndex, OplogEntry>;

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
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_exact(owned_agent_id, agent_mode, idx, n).await
    }

    /// Checks whether the oplog exists in the oplog, without opening it
    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool;

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
}

const SCAN_CURSOR_PREFIX: &str = "gsc1_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OplogScanState {
    pub(crate) layer: usize,
    pub(crate) mode: AgentMode,
    pub(crate) resume: Option<ScanResume>,
}

pub(crate) fn decode_scan_cursor(
    cursor: &ScanCursor,
    modes: Option<AgentMode>,
) -> Result<OplogScanState, WorkerExecutorError> {
    if cursor.is_finished() {
        return Ok(OplogScanState {
            layer: 0,
            mode: modes.unwrap_or(AgentMode::Durable),
            resume: None,
        });
    }

    let encoded = cursor
        .as_str()
        .strip_prefix(SCAN_CURSOR_PREFIX)
        .ok_or_else(|| WorkerExecutorError::invalid_request("Invalid agent scan cursor version"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| WorkerExecutorError::invalid_request("Invalid agent scan cursor encoding"))?;
    let state: OplogScanState = serde_json::from_slice(&bytes)
        .map_err(|_| WorkerExecutorError::invalid_request("Invalid agent scan cursor payload"))?;

    if let Some(mode) = modes
        && state.mode != mode
    {
        return Err(WorkerExecutorError::invalid_request(
            "Agent scan cursor does not match the requested agent mode",
        ));
    }

    Ok(state)
}

fn encode_scan_cursor(state: OplogScanState) -> Result<ScanCursor, WorkerExecutorError> {
    let bytes = serde_json::to_vec(&state).map_err(|error| {
        WorkerExecutorError::unknown(format!("Failed to encode agent scan cursor: {error}"))
    })?;
    Ok(ScanCursor::new(format!(
        "{SCAN_CURSOR_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    )))
}

pub(crate) fn first_scan_cursor(
    layer: usize,
    modes: Option<AgentMode>,
) -> Result<ScanCursor, WorkerExecutorError> {
    encode_scan_cursor(OplogScanState {
        layer,
        mode: modes.unwrap_or(AgentMode::Durable),
        resume: None,
    })
}

pub(crate) fn next_scan_cursor(
    state: OplogScanState,
    modes: Option<AgentMode>,
    resume: Option<ScanResume>,
) -> Result<ScanCursor, WorkerExecutorError> {
    match resume {
        Some(resume) => encode_scan_cursor(OplogScanState {
            resume: Some(resume),
            ..state
        }),
        None if modes.is_none() && state.mode == AgentMode::Durable => {
            encode_scan_cursor(OplogScanState {
                mode: AgentMode::Ephemeral,
                resume: None,
                ..state
            })
        }
        None => Ok(ScanCursor::default()),
    }
}

pub(crate) async fn retry_scan_storage_op<T, F, Fut>(
    retry_config: &golem_common::model::RetryConfig,
    op_name: &str,
    target: &str,
    mut op: F,
) -> Result<T, WorkerExecutorError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(value) => return Ok(value),
            Err(IndexedStorageError::InvalidResume(message)) => {
                return Err(WorkerExecutorError::invalid_request(message));
            }
            Err(IndexedStorageError::Transient(message)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    crate::metrics::oplog::record_oplog_storage_retry(op_name);
                    tracing::warn!(
                        op = op_name,
                        key = target,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error, retrying: {message}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{target}' after {attempts} attempts: Transient storage error: {message}"
                    );
                }
            }
            Err(error) => {
                panic!("Indexed storage operation '{op_name}' failed for key '{target}': {error}");
            }
        }
    }
}

/// A handle to an external blob upload that [`Oplog::add_start_with_reserved_raw_payload`] started
/// (spawned) but which may not have finished yet.
///
/// Cloneable so the same upload can be awaited by both the call that initiated it (before it
/// appends its `End`) and the commit barrier in the leaf oplog's `append` (before any entry
/// referencing the blob is persisted to indexed storage). It is `Send`, so a durable-call start can
/// return it and hold it across awaits. A `None` inner means the payload was stored inline, or
/// eagerly uploaded and already durable, so there is nothing to wait for.
#[derive(Clone)]
pub struct PendingUpload {
    inner: Option<Shared<BoxFuture<'static, Result<(), String>>>>,
}

impl PendingUpload {
    /// A payload that is already durable (inline, or an eager upload): waiting is a no-op.
    pub fn already_durable() -> Self {
        Self { inner: None }
    }

    /// A payload whose blob upload is in flight on the given shared future.
    pub fn spawned(upload: Shared<BoxFuture<'static, Result<(), String>>>) -> Self {
        Self {
            inner: Some(upload),
        }
    }

    /// Waits for the blob upload to finish, returning its result (`Ok(())` if there was nothing to
    /// wait for).
    pub async fn wait(&self) -> Result<(), String> {
        match &self.inner {
            Some(upload) => upload.clone().await,
            None => Ok(()),
        }
    }
}

impl Debug for PendingUpload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingUpload")
            .field("pending", &self.inner.is_some())
            .finish()
    }
}

/// A zero-sized, `!Send` token carried inside [`ReservedPayload`] to protect the leaf oplog's
/// `Start`-ordering critical section at compile time.
///
/// [`Oplog::add_start_with_reserved_raw_payload`] must, within a single non-yielding step of the
/// leaf oplog's actor job handler, reserve the request payload, build the `Start`, and assign its
/// index (`push`) with **no `.await` in between** — otherwise a concurrent call's `Start` could
/// interleave and break initiation-order determinism. The leaf implementation holds this guard
/// across that whole window. Because it is
/// `!Send`, holding it across an `.await` makes the enclosing (`async_trait`, hence `Send`-bound)
/// future fail to compile, so a future refactor that turns one of those synchronous steps into an
/// awaited one is rejected by the compiler instead of silently reordering. Never add a manual
/// `Send`/`Sync` impl.
#[must_use = "the reserve guard must be held until the Start is pushed, then dropped"]
pub struct ReserveGuard {
    _not_send: PhantomData<*const ()>,
}

/// The result of [`PrimaryOplogState::reserve_raw_payload`]: a payload reference whose (possibly
/// large) blob upload has been *started* but not awaited, the [`PendingUpload`] tracking that
/// upload, and a [`ReserveGuard`] guarding the no-`.await` window up to the `Start` `push`.
#[must_use = "a reserved payload must be turned into a Start within the same non-yielding actor step"]
pub struct ReservedPayload {
    pub raw: RawOplogPayload,
    pub pending: PendingUpload,
    pub guard: ReserveGuard,
}

impl ReservedPayload {
    /// Builds a reserve result. Called by the leaf oplog from inside its actor task.
    pub fn new(raw: RawOplogPayload, pending: PendingUpload) -> Self {
        Self {
            raw,
            pending,
            guard: ReserveGuard {
                _not_send: PhantomData,
            },
        }
    }
}

/// The result of [`Oplog::add_start_with_reserved_raw_payload`]: the appended `Start`, its assigned
/// index, and the [`PendingUpload`] tracking its (possibly deferred) request-payload blob upload.
#[must_use = "the pending upload must be awaited before the matching End/Cancelled is appended"]
pub struct OrderedOplogStart {
    /// The index assigned to the appended `Start` entry.
    pub index: OplogIndex,
    /// The `Start` entry exactly as appended. Wrapper layers that mirror the buffered entries (e.g.
    /// the plugin-forwarding oplog) need it because the entry is built deep in the leaf from the
    /// reserved payload reference.
    pub entry: OplogEntry,
    /// Tracks the request payload's durable write. The caller must [`PendingUpload::wait`] on it
    /// before appending the matching `End`/`Cancelled`; the leaf oplog's `append` commit barrier is
    /// the backstop. A no-op for inline or eagerly-uploaded payloads.
    pub pending_upload: PendingUpload,
}

pub enum DurableStreamOplogRecord {
    Registered(Option<OplogIndex>, Box<StreamRegisteredRecord>),
    Items(Option<OplogIndex>, StreamItemsRecord),
    End(Option<OplogIndex>, StreamEndRecord),
    Cancel(Option<OplogIndex>, StreamCancelRecord),
    Session(Option<OplogIndex>, Box<StreamSessionRecord>),
    InlineEntry(OplogEntry),
}

impl DurableStreamOplogRecord {
    fn serialize(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Registered(_, record) => serialize(record),
            Self::Items(_, record) => serialize(record),
            Self::End(_, record) => serialize(record),
            Self::Cancel(_, record) => serialize(record),
            Self::Session(_, record) => serialize(record),
            Self::InlineEntry(_) => Ok(Vec::new()),
        }
    }

    fn into_entry(self, raw: RawOplogPayload) -> Result<OplogEntry, String> {
        match self {
            Self::Registered(entity_parent_start_index, record) => {
                Ok(OplogEntry::stream_registered(
                    entity_parent_start_index,
                    raw.into_payload_with_cache(Arc::from(record))?,
                ))
            }
            Self::Items(entity_parent_start_index, record) => Ok(OplogEntry::stream_items(
                entity_parent_start_index,
                raw.into_payload_with_cache(Arc::new(record))?,
            )),
            Self::End(entity_parent_start_index, record) => Ok(OplogEntry::stream_end(
                entity_parent_start_index,
                raw.into_payload_with_cache(Arc::new(record))?,
            )),
            Self::Cancel(entity_parent_start_index, record) => Ok(OplogEntry::stream_cancel(
                entity_parent_start_index,
                raw.into_payload_with_cache(Arc::new(record))?,
            )),
            Self::Session(entity_parent_start_index, record) => Ok(OplogEntry::stream_session(
                entity_parent_start_index,
                raw.into_payload_with_cache(Arc::from(record))?,
            )),
            Self::InlineEntry(entry) => Ok(entry),
        }
    }

    pub fn into_inline_entry(self) -> OplogEntry {
        match self {
            Self::Registered(entity_parent_start_index, record) => OplogEntry::stream_registered(
                entity_parent_start_index,
                OplogPayload::Inline(record),
            ),
            Self::Items(entity_parent_start_index, record) => OplogEntry::stream_items(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
            ),
            Self::End(entity_parent_start_index, record) => OplogEntry::stream_end(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
            ),
            Self::Cancel(entity_parent_start_index, record) => OplogEntry::stream_cancel(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
            ),
            Self::Session(entity_parent_start_index, record) => {
                OplogEntry::stream_session(entity_parent_start_index, OplogPayload::Inline(record))
            }
            Self::InlineEntry(entry) => entry,
        }
    }
}

pub type DurableStreamBatchBuilder =
    Box<dyn FnOnce(OplogIndex) -> Vec<DurableStreamOplogRecord> + Send>;

pub type ReservedRawStartBuilder =
    Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>;

pub type IndexedReservedStartBuilder =
    Box<dyn FnOnce(OplogIndex) -> Result<(Vec<u8>, ReservedRawStartBuilder), String> + Send>;

/// Why an oplog write was refused by the storage: the shard epoch this executor asserted is
/// behind the one recorded for the oplog, because another executor owns the shard now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OplogFence {
    pub agent_id: AgentId,
    pub expected_epoch: ShardEpoch,
    pub actual_epoch: Option<ShardEpoch>,
}

/// Why an oplog write failed without taking the executor down.
///
/// A `Fenced` write is not a storage failure - the storage is healthy and refused the write on
/// purpose - so it is returned rather than retried or panicked on, and the worker that hit it is
/// stopped and left to the shard's new owner. A storage failure never reaches this type: it keeps
/// its fail-stop semantics inside the oplog implementation. `Payload` is an entry whose payload the
/// caller-supplied builder could not produce - it failed to serialize, or was too large - and the
/// add failures tests inject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OplogError {
    Fenced(OplogFence),
    Payload(String),
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
        }
    }
}

impl std::error::Error for OplogError {}

/// A single oplog append that has already been synchronously enqueued in the oplog's ordering
/// domain. Creating this receipt reserves the entry's position; awaiting it returns the assigned
/// index after the append finishes.
pub type OplogAddReceipt = BoxFuture<'static, Result<OplogIndex, OplogError>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawDurableStreamSessionStatus {
    pub watermark: OplogIndex,
    pub status: Result<Option<DurableStreamSessionStatus>, String>,
}

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Any + Debug + Send + Sync {
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

    /// Commits the buffered entries to the oplog
    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError>;

    /// Returns the current oplog index
    async fn current_oplog_index(&self) -> OplogIndex;

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
    async fn read_exact(&self, oplog_index: OplogIndex, n: u64)
    -> BTreeMap<OplogIndex, OplogEntry>;

    /// Reads the part of the requested range physically present in this oplog.
    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_exact(oplog_index, n).await
    }

    /// Reads the entry at the given oplog index.
    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.read_exact(oplog_index, 1)
            .await
            .remove(&oplog_index)
            .unwrap_or_else(|| {
                panic!("Missing oplog entry {oplog_index} after an exact single-entry read")
            })
    }

    /// Gets the total number of entries in the oplog
    async fn length(&self) -> u64;

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

    /// Atomically appends a `Start` entry and a second entry (its `End` or
    /// `Cancelled`) that references the `Start`'s `OplogIndex`.
    ///
    /// `make_second` builds the second entry from the freshly assigned `Start`
    /// index (a durable call is identified by the `OplogIndex` of its `Start`).
    /// Used by the sequential adapter (the p2 durability path, see
    /// [`OplogOps::add_completed_host_call`]) to write a matched host-call
    /// `Start`/`End` pair atomically.
    ///
    /// Implementations must ensure no other writer can interleave between the
    /// two appends and that no commit threshold check fires between them, so
    /// the pair is never split across a commit/crash boundary. This is a
    /// required method (no default) deliberately: a default `add`-twice
    /// composition would silently violate that atomicity for any implementor
    /// that forgot to override it.
    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> Result<(OplogIndex, OplogIndex), OplogError>;

    /// The shard epoch this oplog's writes assert, or `None` for an ephemeral oplog, which nothing
    /// fences - nor the archive layers behind it.
    ///
    /// Only the primary oplog knows it, so a wrapper answers from the oplog it wraps.
    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.inner().and_then(|inner| inner.shard_epoch())
    }

    /// The refusal this oplog has latched, if the storage has turned one of its writes away:
    /// every later write fails on it, so the handle is finished. Answered without a round trip,
    /// so the open-oplog cache can decline to hand a finished handle to a new opener.
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

async fn deserialize_oplog_payload<T: BinaryCodec + Send + 'static>(
    bytes: Vec<u8>,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        golem_common::serialization::try_deserialize(&bytes)?.ok_or_else(|| {
            "oplog payload has an unsupported or missing serialization version".into()
        })
    })
    .await
    .map_err(|error| format!("oplog payload deserialization task failed: {error}"))?
}

#[async_trait]
pub trait OplogOps: Oplog {
    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload<T: BinaryCodec + Debug + Clone + PartialEq + Sync>(
        &self,
        data: &T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self.upload_raw_payload(bytes).await?;
        let cached = Arc::new(data.clone());
        let payload = raw_payload.into_payload_with_cache(cached)?;
        Ok(payload)
    }

    /// Uploads an owned oplog payload and moves it into the in-memory cache.
    async fn upload_payload_owned<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync>(
        &self,
        data: T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self.upload_raw_payload(bytes).await?;
        let payload = raw_payload.into_payload_with_cache(Arc::new(data))?;
        Ok(payload)
    }

    /// Downloads a big oplog payload by its reference
    async fn download_payload<
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    >(
        &self,
        payload: OplogPayload<T>,
    ) -> Result<T, String> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::SerializedInline { bytes, .. } => deserialize_oplog_payload(bytes).await,
            OplogPayload::External {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                let bytes = self.download_raw_payload(payload_id, md5_hash).await?;
                deserialize_oplog_payload(bytes).await
            }
        }
    }

    /// Typed convenience wrapper over [`Oplog::add_start_with_reserved_raw_payload`]: serializes
    /// `request`, then reserves its payload and appends the `Start` (built by `build_start` from the
    /// payload reference), returning the `Start`'s index and the [`PendingUpload`] tracking the
    /// request blob's durable write. It inherits the underlying raw method's ordering contract
    /// (initiation order on replayable oplogs; none on ephemeral).
    ///
    /// The caller must `wait` on the returned [`PendingUpload`] before appending the call's
    /// `End`/`Cancelled` (so an upload failure surfaces at the call), with the leaf oplog's `append`
    /// commit barrier as the backstop. The ordering and durability contract lives entirely in
    /// [`Oplog::add_start_with_reserved_raw_payload`]; this wrapper only adds typed
    /// (de)serialization.
    async fn add_start_with_reserved_payload<T>(
        &self,
        request: T,
        build_start: impl FnOnce(OplogPayload<T>) -> OplogEntry + Send + 'static,
    ) -> Result<(OplogIndex, PendingUpload), OplogError>
    where
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    {
        let bytes = serialize(&request)?;
        let cached = Arc::new(request);
        let ordered = self
            .add_start_with_reserved_raw_payload(
                bytes,
                Box::new(move |raw| {
                    let payload = raw.into_payload_with_cache(cached)?;
                    Ok(build_start(payload))
                }),
            )
            .await?;
        Ok((ordered.index, ordered.pending_upload))
    }

    /// Typed convenience wrapper over
    /// [`Oplog::add_start_with_indexed_reserved_raw_payload`]. The request builder receives the
    /// exact index that will identify the durable host call.
    async fn add_start_with_indexed_reserved_payload<T>(
        &self,
        build_request: impl FnOnce(OplogIndex) -> Result<T, String> + Send + 'static,
        build_start: impl FnOnce(OplogPayload<T>) -> OplogEntry + Send + 'static,
    ) -> Result<(OplogIndex, PendingUpload), OplogError>
    where
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    {
        let ordered = self
            .add_start_with_indexed_reserved_raw_payload(Box::new(move |start_index| {
                let request = build_request(start_index)?;
                let bytes = serialize(&request)?;
                let cached = Arc::new(request);
                Ok((
                    bytes,
                    Box::new(move |raw| {
                        let payload = raw.into_payload_with_cache(cached)?;
                        Ok(build_start(payload))
                    }),
                ))
            }))
            .await?;
        Ok((ordered.index, ordered.pending_upload))
    }

    /// Sequential adapter that persists a completed durable host call as a matched
    /// `Start`/`End` pair.
    /// Returns `(start_idx, end_idx)`. A durable call is identified by the
    /// `OplogIndex` of its `Start`, and the `End` references it via
    /// `start_index`. The two entries are appended atomically via
    /// [`Oplog::add_pair`] so no other writer can interleave between them and
    /// the pair is never split across a commit/crash boundary.
    ///
    /// This is the durability primitive of the sequential (p2) host-call path, a permanent
    /// coexistence path (Rust's std imports p2), not a legacy one. Unlike the concurrent (p3)
    /// path — which orders `Start` eagerly via
    /// [`Oplog::add_start_with_reserved_raw_payload`] before the side effect runs — the
    /// sequential path runs at most one host call at a time per worker, so it can record the
    /// whole call after the effect completed, as one atomic pair.
    ///
    /// `parent_start_index` is the `Start` index of the enclosing durable scope (if any). This is
    /// an explicit parameter because the oplog cannot see the worker state's open scopes, and
    /// because the parent must be the call's own enclosing scope, not whichever sibling scope
    /// happens to be temporally open. The caller derives it explicitly from the call.
    async fn add_completed_host_call(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
    ) -> Result<(OplogIndex, OplogIndex), OplogError> {
        let request_payload: OplogPayload<HostRequest> = self.upload_payload(request).await?;
        let response_payload: OplogPayload<HostResponse> = self.upload_payload(response).await?;
        let now = Timestamp::now_utc();
        let start = OplogEntry::Start {
            timestamp: now,
            parent_start_index,
            function_name,
            invocation_id: None,
            observational_owner: None,
            request: Some(request_payload),
            durable_function_type: function_type,
        };
        let (start_idx, end_idx) = self
            .add_pair(
                start,
                Box::new(move |start_index| OplogEntry::End {
                    timestamp: now,
                    start_index,
                    response: Some(response_payload),
                    forced_commit: false,
                }),
            )
            .await?;
        Ok((start_idx, end_idx))
    }

    async fn add_agent_invocation_started(
        &self,
        invocation: AgentInvocation,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogEntry, OplogError> {
        let entry = self
            .agent_invocation_started_entry(invocation, wallet_pin)
            .await?;
        self.add(entry.clone()).await?;
        Ok(entry)
    }

    async fn add_agent_invocation_started_with_index(
        &self,
        invocation: AgentInvocation,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogIndex, OplogError> {
        let entry = self
            .agent_invocation_started_entry(invocation, wallet_pin)
            .await?;
        self.add(entry).await
    }

    async fn agent_invocation_started_entry(
        &self,
        invocation: AgentInvocation,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogEntry, String> {
        let (idempotency_key, invocation_payload, ctx) = invocation.into_parts();
        let payload = self.upload_payload_owned(invocation_payload).await?;
        let invocation_context = ctx.to_oplog_data();
        Ok(OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            idempotency_key,
            payload,
            trace_id: ctx.trace_id,
            trace_states: ctx.trace_states,
            invocation_context,
            wallet_pin: Box::new(wallet_pin),
        })
    }

    async fn add_agent_invocation_finished(
        &self,
        result: &AgentInvocationResult,
        method_name: Option<String>,
        consumed_fuel: u64,
        component_revision: ComponentRevision,
    ) -> Result<OplogIndex, OplogError> {
        let consumed_fuel = if consumed_fuel > i64::MAX as u64 {
            i64::MAX
        } else {
            consumed_fuel as i64
        };

        let payload = self.upload_payload(result).await?;
        let entry = OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            result: payload,
            method_name,
            consumed_fuel,
            component_revision,
        };
        self.add(entry).await
    }

    async fn create_snapshot_based_update_description(
        &self,
        target_revision: ComponentRevision,
        payload: Vec<u8>,
        mime_type: String,
    ) -> Result<UpdateDescription, String> {
        let payload = self.upload_payload_owned(payload).await?;
        Ok(UpdateDescription::SnapshotBased {
            target_revision,
            payload,
            mime_type,
        })
    }

    async fn get_upload_description_payload(
        &self,
        description: UpdateDescription,
    ) -> Result<Option<(Vec<u8>, String)>, String> {
        match description {
            UpdateDescription::SnapshotBased {
                payload, mime_type, ..
            } => {
                let bytes = self.download_payload(payload).await?;
                Ok(Some((bytes, mime_type)))
            }
            UpdateDescription::Automatic { .. } => Ok(None),
        }
    }
}

#[async_trait]
impl<O: Oplog + ?Sized> OplogOps for O {}

#[async_trait]
pub trait OplogServiceOps: OplogService {
    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload<T: BinaryCodec + Debug + Clone + PartialEq + Sync>(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: &T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self
            .upload_raw_payload(owned_agent_id, agent_mode, bytes)
            .await?;
        let cached = Arc::new(data.clone());
        let payload = raw_payload.into_payload_with_cache(cached)?;
        Ok(payload)
    }

    /// Downloads a big oplog payload by its reference
    async fn download_payload<
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    >(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload: OplogPayload<T>,
    ) -> Result<T, String> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::SerializedInline { bytes, .. } => deserialize_oplog_payload(bytes).await,
            OplogPayload::External {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                let bytes = self
                    .download_raw_payload(owned_agent_id, agent_mode, payload_id, md5_hash)
                    .await?;
                deserialize_oplog_payload(bytes).await
            }
        }
    }
}

#[async_trait]
impl<O: OplogService + ?Sized> OplogServiceOps for O {}

pub type OplogCloseCompletion = Shared<BoxFuture<'static, Result<(), String>>>;

struct OpenOplogEntry {
    oplog: Weak<dyn Oplog>,
    closed: OplogCloseCompletion,
    /// The epoch the opener that constructed this handle asked it to assert.
    requested_epoch: Option<ShardEpoch>,
}

type OplogSlot = Arc<Mutex<Option<OpenOplogEntry>>>;

/// Exclusive ownership of a logical oplog's cold lifecycle. The primary service owns the slot;
/// wrapper construction uses the same guard, including when no primary handle exists yet.
pub struct OplogLifecycleGuard {
    agent_id: AgentId,
    slot: Option<OwnedMutexGuard<Option<OpenOplogEntry>>>,
    owner: OpenOplogs,
}

impl OplogLifecycleGuard {
    pub fn assert_agent(&self, agent_id: &AgentId) {
        assert_eq!(&self.agent_id, agent_id);
    }
}

impl Drop for OplogLifecycleGuard {
    fn drop(&mut self) {
        let guard = self.slot.take().unwrap();
        let slot = OwnedMutexGuard::mutex(&guard).clone();
        drop(guard);
        self.owner.release_if_unused(&self.agent_id, &slot);
    }
}

#[derive(Clone)]
pub struct OpenOplogs {
    oplogs: Cache<AgentId, (), OplogSlot, ()>,
}

impl OpenOplogs {
    pub fn new(name: &'static str) -> Self {
        Self {
            oplogs: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                name,
            ),
        }
    }

    async fn slot(&self, agent_id: &AgentId) -> OplogSlot {
        self.oplogs
            .get_or_insert_simple(agent_id, || async { Ok(Arc::new(Mutex::new(None))) })
            .await
            .unwrap()
    }

    pub async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        let slot = self.slot(agent_id).await.lock_owned().await;
        OplogLifecycleGuard {
            agent_id: agent_id.clone(),
            slot: Some(slot),
            owner: self.clone(),
        }
    }

    fn release_if_unused(&self, agent_id: &AgentId, slot: &OplogSlot) {
        // Every waiter and live handle owns a slot reference. Removing the last unused slot
        // cannot split waiters between two locks or let an old handle evict its replacement.
        self.oplogs.remove_if_cached_sync(agent_id, |current| {
            Arc::ptr_eq(current, slot)
                && Arc::strong_count(current) == 2
                && current.try_lock().is_ok_and(|entry| {
                    entry
                        .as_ref()
                        .is_none_or(|entry| entry.closed.peek().is_some())
                })
        });
    }

    pub async fn get_or_open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        agent_id: &AgentId,
        constructor: impl OplogConstructor,
    ) -> Arc<dyn Oplog> {
        lifecycle.assert_agent(agent_id);
        let requested_epoch = constructor.shard_epoch();
        let slot = self.slot(agent_id).await;
        let is_primary = Arc::ptr_eq(
            &slot,
            OwnedMutexGuard::mutex(lifecycle.slot.as_ref().unwrap()),
        );
        // Wrapper slots are only reached under the primary lifecycle guard. Their nested
        // locks protect cached handles during construction, not independent lifecycles.
        let mut wrapper_slot = if is_primary {
            None
        } else {
            Some(slot.lock().await)
        };
        let cached = if let Some(wrapper) = &wrapper_slot {
            wrapper.as_ref()
        } else {
            lifecycle.slot.as_ref().unwrap().as_ref()
        };
        match cached.and_then(|entry| entry.oplog.upgrade()) {
            Some(oplog) if !oplog.is_retired() => {
                let opened_with = cached.and_then(|entry| entry.requested_epoch);
                if can_reuse(&*oplog, opened_with, requested_epoch) {
                    return oplog;
                }
                // Replaced without waiting for it to close: see `can_reuse`.
            }
            live => {
                if let Some(oplog) = live {
                    oplog.retire();
                }
                if let Some(cached) = cached {
                    // Completion, including an error, proves the old layer no longer owns running
                    // work. The new attempt reloads persisted state rather than inheriting the old
                    // error.
                    let _ = cached.closed.clone().await;
                }
            }
        }
        let owner = self.clone();
        let close_agent_id = agent_id.clone();
        let close_slot = slot.clone();
        let close = Box::new(move || owner.release_if_unused(&close_agent_id, &close_slot));
        let oplog = constructor.create_oplog(lifecycle, close).await;
        let closed = oplog.closed();
        let entry = Some(OpenOplogEntry {
            oplog: Arc::downgrade(&oplog),
            closed: closed.clone(),
            requested_epoch,
        });
        if let Some(wrapper) = &mut wrapper_slot {
            **wrapper = entry;
        } else {
            **lifecycle.slot.as_mut().unwrap() = entry;
        }
        // Retain the cache slot until asynchronous last-handle cleanup finishes, even when
        // nobody reopens it. A cold open that wins this race awaits the same completion above.
        let owner = self.clone();
        let agent_id = agent_id.clone();
        let cleanup_slot = slot.clone();
        tokio::spawn(async move {
            let _ = closed.await;
            owner.release_if_unused(&agent_id, &cleanup_slot);
        });
        oplog
    }
}

/// Whether a live cached handle can be handed to an opener asking for `requested`. It cannot,
/// and is replaced, when:
/// - it is fenced: the storage refused one of its writes, so every later one is refused too;
/// - or it belongs to an older ownership generation: `requested` is newer than the epoch it was
///   opened with (`None`, an ephemeral open, is older than any epoch) and it really asserts that
///   epoch. An ephemeral handle opened with an epoch asserts none, so it is reused at any epoch;
///   one opened with `None` is replaced by any open that asserts an epoch.
///
/// A replaced handle may still be held by a worker that is stopping, so nobody waits for it to
/// close; it keeps any background work, such as an archive transfer, until its holder drops it.
/// An equal or older request gets the cached handle, so no two live handles assert one epoch.
fn can_reuse(
    oplog: &dyn Oplog,
    opened_with: Option<ShardEpoch>,
    requested: Option<ShardEpoch>,
) -> bool {
    let fenced = oplog.fence().is_some();
    let older_generation = requested > opened_with && oplog.shard_epoch() == opened_with;
    !fenced && !older_generation
}

impl Debug for OpenOplogs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenOplogs").finish()
    }
}

#[async_trait]
pub trait OplogConstructor: Send {
    async fn create_oplog(
        self,
        lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog>;

    /// The epoch the oplog this constructor builds is asked to assert, or `None` for an ephemeral
    /// one. The open-oplog cache compares it with the epoch a cached
    /// handle was opened with, so it has no default: a layer that left it out would hand an
    /// older generation's handle to every newer opener.
    fn shard_epoch(&self) -> Option<ShardEpoch>;
}
