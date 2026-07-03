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
use async_trait::async_trait;
pub use blob::BlobOplogArchiveService;
pub use compressed::{CompressedOplogArchive, CompressedOplogArchiveService, CompressedOplogChunk};
use desert_rust::BinaryCodec;
use futures::future::{BoxFuture, Shared};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostResponse, OplogEntry, OplogIndex, OplogPayload,
    PayloadId, PersistenceLevel, RawOplogPayload, UpdateDescription,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationResult, AgentMetadata, AgentStatusRecord,
    OwnedAgentId, ScanCursor, Timestamp,
};
use golem_common::read_only_lock;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;

pub use ephemeral::EphemeralOplog;
pub use multilayer::{MultiLayerOplog, MultiLayerOplogService, OplogArchiveService};
pub use primary::PrimaryOplogService;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

mod blob;
mod compressed;
mod ephemeral;
mod multilayer;
pub mod plugin;
mod primary;
pub mod rate_limited;

#[cfg(test)]
pub mod tests;

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
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
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
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog>;

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex;

    async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode);

    async fn read(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Reads an inclusive range of entries from the oplog
    async fn read_range(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        start_idx: OplogIndex,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        assert!(
            start_idx <= last_idx,
            "Invalid range passed to OplogService::read_range: start_idx = {start_idx}, last_idx = {last_idx}"
        );

        self.read(
            owned_agent_id,
            agent_mode,
            start_idx,
            Into::<u64>::into(last_idx) - Into::<u64>::into(start_idx) + 1,
        )
        .await
    }

    async fn read_prefix(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_range(owned_agent_id, agent_mode, OplogIndex::INITIAL, last_idx)
            .await
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
    /// Only commit immediately if the worker is durable
    DurableOnly,
}

/// High bit of `ScanCursor.cursor` used to encode the active `AgentMode` phase
/// when `scan_for_component` is invoked with `modes = None` (scan both modes).
///
/// When the bit is `0`, the active phase scans `AgentMode::Durable`. When the
/// bit is `1`, the active phase scans `AgentMode::Ephemeral`. The remaining
/// 63 bits hold the actual storage cursor value, which is more than enough for
/// any indexed-storage backend's cursor.
///
/// This bit is disjoint from `ScanCursor.layer`, which encodes the
/// `MultiLayerOplogService` layer being scanned.
pub(crate) const SCAN_CURSOR_EPHEMERAL_BIT: u64 = 1u64 << 63;
pub(crate) const SCAN_CURSOR_VALUE_MASK: u64 = !SCAN_CURSOR_EPHEMERAL_BIT;

/// Decodes a multi-mode scan cursor.
///
/// Returns `(active_mode, next_mode)` where:
/// - `active_mode` is the mode that should be scanned in this call.
/// - `next_mode` is `Some(mode)` if there is another phase to continue with
///   when the active phase finishes (cursor reaches `0`), and `None` if there
///   is nothing else to scan after this phase.
///
/// When `modes = Some(m)`, only that single mode is scanned (no phase
/// transition). When `modes = None`, the durable mode is scanned first, then
/// the ephemeral mode.
pub(crate) fn scan_modes(
    modes: Option<AgentMode>,
    raw_cursor: u64,
) -> (AgentMode, Option<AgentMode>) {
    match modes {
        Some(mode) => (mode, None),
        None => {
            if raw_cursor & SCAN_CURSOR_EPHEMERAL_BIT == 0 {
                (AgentMode::Durable, Some(AgentMode::Ephemeral))
            } else {
                (AgentMode::Ephemeral, None)
            }
        }
    }
}

/// Strips the mode-encoding high bit from a raw cursor and returns the actual
/// storage cursor value to pass to the indexed storage backend.
pub(crate) fn cursor_value(raw_cursor: u64) -> u64 {
    raw_cursor & SCAN_CURSOR_VALUE_MASK
}

/// Builds the next `ScanCursor` to return from `scan_for_component`.
///
/// - `next_cursor_val` is the storage-level cursor returned by the backend
///   (already free of mode-encoding bits).
/// - `active_mode` is the mode that was just scanned.
/// - `next_mode` is the mode to continue with after the active phase finishes
///   (as returned by `scan_modes`).
/// - `layer` is preserved from the input cursor.
pub(crate) fn next_scan_cursor(
    next_cursor_val: u64,
    active_mode: AgentMode,
    next_mode: Option<AgentMode>,
    layer: usize,
) -> ScanCursor {
    let value = next_cursor_val & SCAN_CURSOR_VALUE_MASK;
    if value == 0 {
        // Active phase finished. If there is a next mode, switch to it; otherwise emit cursor 0.
        match next_mode {
            Some(AgentMode::Ephemeral) => ScanCursor {
                cursor: SCAN_CURSOR_EPHEMERAL_BIT,
                layer,
            },
            Some(AgentMode::Durable) => ScanCursor { cursor: 0, layer },
            None => ScanCursor { cursor: 0, layer },
        }
    } else {
        // Active phase still running. Re-encode the active mode so subsequent
        // calls resume with the same active mode.
        let bit = match active_mode {
            AgentMode::Durable => 0,
            AgentMode::Ephemeral => SCAN_CURSOR_EPHEMERAL_BIT,
        };
        ScanCursor {
            cursor: value | bit,
            layer,
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
/// [`Oplog::add_start_with_reserved_raw_payload`] must, under a single held state lock, reserve the
/// request payload, build the `Start`, and assign its index (`push`) with **no `.await` in
/// between** — otherwise a concurrent call's `Start` could interleave and break initiation-order
/// determinism. The leaf implementation holds this guard across that whole window. Because it is
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
#[must_use = "a reserved payload must be turned into a Start under the same held lock"]
pub struct ReservedPayload {
    pub raw: RawOplogPayload,
    pub pending: PendingUpload,
    pub guard: ReserveGuard,
}

impl ReservedPayload {
    /// Builds a reserve result. Called by the leaf oplog while holding its state lock.
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

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Any + Debug + Send + Sync {
    /// Adds a single entry to the oplog (possibly buffered), and returns its index
    async fn add(&self, entry: OplogEntry) -> OplogIndex;

    /// A variant of add that can inject failures in tests. TO BE REMOVED
    async fn fallible_add(&self, entry: OplogEntry) -> Result<(), String> {
        self.add(entry).await;
        Ok(())
    }

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the layer below this one
    ///
    /// Returns the number of dropped entries.
    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64;

    /// Commits the buffered entries to the oplog
    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Returns the current oplog index
    async fn current_oplog_index(&self) -> OplogIndex;

    /// Returns the index of the last non-hint entry which was added in this session with `add`. If
    /// there is no such entry, returns `None`.
    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex>;

    /// Waits until indexed store writes all changes into at least `replicas` replicas (or the maximum
    /// available).
    /// Returns true if the maximum possible number of replicas is reached within the timeout,
    /// otherwise false.
    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool;

    /// Reads the entry at the given oplog index
    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry;

    /// Reads the entry at the given oplog index
    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Gets the total number of entries in the oplog
    async fn length(&self) -> u64;

    /// Adds an entry to the oplog and immediately commits it
    async fn add_and_commit(&self, entry: OplogEntry) -> OplogIndex {
        let index = self.add(entry).await;
        self.commit(CommitLevel::Always).await;
        index
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
    /// running `build_start`, and assigning the index under a single held state lock with **no
    /// `.await` in between** (a deferred upload is *started* there but not awaited). Wrapper
    /// implementations must delegate without introducing an `.await` before the inner call assigns
    /// the index (e.g. rate-limiting back-pressure must happen *after* delegation, not before).
    ///
    /// This method is itself `async` only because it takes the leaf state lock and/or delegates
    /// through wrapper layers — *not* because it awaits the upload. The returned
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
        build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
    ) -> Result<OrderedOplogStart, String>;

    /// Switched to a different persistence level. This can be used as an optimization hint in the implementations.
    async fn switch_persistence_level(&self, mode: PersistenceLevel);

    /// Atomically appends a `Start` entry and a second entry (its `End` or
    /// `Cancelled`) that references the `Start`'s `OplogIndex`.
    ///
    /// `make_second` builds the second entry from the freshly assigned `Start`
    /// index (a durable call is identified by the `OplogIndex` of its `Start`).
    /// Used by the legacy adapter to write a matched host-call `Start`/`End`
    /// pair atomically. The default implementation just calls
    /// `add` twice; concrete implementations must override to ensure no other
    /// writer can interleave between the two appends and that no commit
    /// threshold check fires between them, so the pair is never split across a
    /// crash boundary.
    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        let first_idx = self.add(start).await;
        let second = make_second(first_idx);
        let second_idx = self.add(second).await;
        (first_idx, second_idx)
    }

    /// Like [`add_pair`](Self::add_pair) but for two already-built entries, returning a
    /// `Result` so test wrappers can inject a write failure on either entry. The default
    /// delegates to `add_pair`, inheriting its atomic buffering, so the two entries are
    /// never split by a commit-threshold check or a crash boundary.
    async fn fallible_add_pair(
        &self,
        first: OplogEntry,
        second: OplogEntry,
    ) -> Result<(OplogIndex, OplogIndex), String> {
        let (first_idx, second_idx) = self.add_pair(first, Box::new(move |_| second)).await;
        Ok((first_idx, second_idx))
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
        match current.inner() {
            Some(inner) => current = inner,
            None => return None,
        }
    }
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

    /// Downloads a big oplog payload by its reference
    async fn download_payload<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync>(
        &self,
        payload: OplogPayload<T>,
    ) -> Result<T, String> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::SerializedInline { bytes, .. } => deserialize(&bytes),
            OplogPayload::External {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                let bytes = self.download_raw_payload(payload_id, md5_hash).await?;
                deserialize(&bytes)
            }
        }
    }

    /// Typed convenience wrapper over [`Oplog::add_start_with_reserved_raw_payload`]: serializes
    /// `request`, then reserves its payload and appends the `Start` (built by `build_start` from the
    /// payload reference) in initiation order, returning the `Start`'s index and the
    /// [`PendingUpload`] tracking the request blob's durable write.
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
    ) -> Result<(OplogIndex, PendingUpload), String>
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

    /// Legacy adapter that persists a completed durable host call as a matched
    /// `Start`/`End` pair.
    /// Returns `(start_idx, end_idx)`. A durable call is identified by the
    /// `OplogIndex` of its `Start`, and the `End` references it via
    /// `start_index`. The two entries are appended atomically via
    /// [`Oplog::add_pair`] so no other writer can interleave between them and
    /// the pair is never split across a commit/crash boundary.
    ///
    /// This will eventually be replaced with a recorder/`CallHandle` based API
    /// that captures `Start` eagerly (before the side effect) and `End` (or
    /// `Cancelled`) when the call completes.
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
    ) -> Result<(OplogIndex, OplogIndex), String> {
        let request_payload: OplogPayload<HostRequest> = self.upload_payload(request).await?;
        let response_payload: OplogPayload<HostResponse> = self.upload_payload(response).await?;
        let now = Timestamp::now_utc();
        let start = OplogEntry::Start {
            timestamp: now,
            parent_start_index,
            function_name,
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
            .await;
        Ok((start_idx, end_idx))
    }

    async fn add_agent_invocation_started(
        &self,
        invocation: AgentInvocation,
    ) -> Result<OplogEntry, String> {
        let (idempotency_key, invocation_payload, ctx) = invocation.into_parts();
        let payload = self.upload_payload(&invocation_payload).await?;
        let trace_id = ctx.trace_id.clone();
        let trace_states = ctx.trace_states.clone();
        let invocation_context = ctx.to_oplog_data();
        let entry = OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            idempotency_key,
            payload,
            trace_id,
            trace_states,
            invocation_context,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn add_agent_invocation_finished(
        &self,
        result: &AgentInvocationResult,
        method_name: Option<String>,
        consumed_fuel: u64,
        component_revision: ComponentRevision,
    ) -> Result<OplogEntry, String> {
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
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn create_snapshot_based_update_description(
        &self,
        target_revision: ComponentRevision,
        payload: Vec<u8>,
        mime_type: String,
    ) -> Result<UpdateDescription, String> {
        let payload = self.upload_payload(&payload).await?;
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
    async fn download_payload<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync>(
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
            OplogPayload::SerializedInline { bytes, .. } => deserialize(&bytes),
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
                deserialize(&bytes)
            }
        }
    }
}

#[async_trait]
impl<O: OplogService + ?Sized> OplogServiceOps for O {}

#[derive(Clone)]
struct OpenOplogEntry {
    pub oplog: Weak<dyn Oplog>,
    pub initial: Arc<AtomicBool>,
}

impl OpenOplogEntry {
    pub fn new(oplog: Arc<dyn Oplog>) -> Self {
        Self {
            oplog: Arc::downgrade(&oplog),
            initial: Arc::new(AtomicBool::new(true)),
        }
    }
}

#[derive(Clone)]
pub struct OpenOplogs {
    oplogs: Cache<AgentId, (), OpenOplogEntry, ()>,
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

    pub async fn get_or_open(
        &self,
        agent_id: &AgentId,
        constructor: impl OplogConstructor + 'static,
    ) -> Arc<dyn Oplog> {
        loop {
            let constructor_clone = constructor.clone();
            let close = Box::new(self.oplogs.create_weak_remover(agent_id.clone()));

            let entry = self
                .oplogs
                .get_or_insert(
                    agent_id,
                    || (),
                    async |_| {
                        let result = constructor_clone.create_oplog(close).await;

                        // Temporarily increasing ref count because we want to store a weak pointer
                        // but not drop it before we re-gain a strong reference when got out of the cache
                        let result = unsafe {
                            let ptr = Arc::into_raw(result);
                            Arc::increment_strong_count(ptr);
                            Arc::from_raw(ptr)
                        };
                        Ok(OpenOplogEntry::new(result))
                    },
                )
                .await
                .unwrap();
            if let Some(oplog) = entry.oplog.upgrade() {
                let oplog = if entry.initial.swap(false, Ordering::AcqRel) {
                    unsafe {
                        let ptr = Arc::into_raw(oplog);
                        Arc::decrement_strong_count(ptr);
                        Arc::from_raw(ptr)
                    }
                } else {
                    oplog
                };

                break oplog;
            } else {
                self.oplogs.remove(agent_id).await;
                continue;
            }
        }
    }
}

impl Debug for OpenOplogs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenOplogs").finish()
    }
}

#[async_trait]
pub trait OplogConstructor: Clone + Send {
    async fn create_oplog(self, close: Box<dyn FnOnce() + Send + Sync>) -> Arc<dyn Oplog>;
}
