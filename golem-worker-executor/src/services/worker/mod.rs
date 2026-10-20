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

mod identity;
mod running_workers;
mod status;
mod worker_service;

#[cfg(test)]
use running_workers::{StaleReason, stale_reason};
#[cfg(test)]
use status::*;

use super::component::ComponentService;
use super::golem_config::GolemConfig;
use super::{HasComponentService, HasConfig, HasOplogService};
use crate::durable_host::durable_stream::metadata::{ProducerMetadataKey, ProducerMetadataRow};
use crate::durable_host::durable_stream::{SessionControlMetadata, SessionError};
use crate::metrics::workers::{
    record_agent_identity_resolution, record_derived_cache_publication_failed,
    record_stale_running_worker, record_status_cache_publication, record_worker_call,
};
use crate::services::oplog::{OplogLifecycleGuard, OplogService};
use crate::services::shard::ShardService;
use crate::services::stream_session_index::StreamSessionIndexService;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::snapshot_selection::kept_rejections;
use crate::worker::status::calculate_last_known_status_with_checkpoint_reader;
use crate::worker::status::fold_invocation_result_entries;
use async_trait::async_trait;
use golem_common::base_model::durable_stream::StreamSessionKey;
use golem_common::model::agent::{AgentMode, ParsedAgentId};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatus, AgentStatusRecord,
    DurableStreamPublicBinding, DurableStreamSessionStatus, FailedUpdateRecord, IdempotencyKey,
    InvocationResultMembership, OwnedAgentId, ReceivedCardTransferIndex, ReceivedCardTransferState,
    ShardEpoch, ShardId, SuccessfulUpdateRecord,
};
use golem_common::serialization::{deserialize, serialize, try_deserialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::{debug, error};

pub(crate) const DERIVED_CACHE_EXPIRY: std::time::Duration =
    std::time::Duration::from_secs(7 * 24 * 60 * 60);
const INVOCATION_RESULT_INDEX_METADATA_FIELD: &str = "metadata";
const INVOCATION_RESULT_INDEX_FIELD_PREFIX: &str = "ir:";

/// The rejected automatic snapshot entries to store: `current` and `new` together, as
/// [`kept_rejections`] keeps them for `status`, in index order. `None` when they equal `current`,
/// so nothing needs a write.
fn rejections_to_store(
    current: Vec<OplogIndex>,
    status: &AgentStatusRecord,
    new: &HashSet<OplogIndex>,
) -> Option<Vec<OplogIndex>> {
    let current = current
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let kept = kept_rejections(status, current.iter().chain(new).copied());
    (kept != current).then(|| kept.into_iter().collect())
}

fn invocation_result_index_field(key: &IdempotencyKey) -> String {
    format!("{INVOCATION_RESULT_INDEX_FIELD_PREFIX}{}", key.value)
}

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct InvocationResultIndexMetadata {
    pub covered_through: OplogIndex,
    pub revert_generation: u64,
    pub current_idempotency_key: Option<IdempotencyKey>,
    pub cancelled_idempotency_key: Option<IdempotencyKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
struct PersistedInvocationResult {
    // Catch-up is serialized per agent and clears the whole hash before advancing to a newer
    // revert generation, so a complete same-generation index cannot retain older-generation
    // fields.
    revert_generation: u64,
    oplog_index: OplogIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationResultIndexLookup {
    Found(OplogIndex),
    DefinitiveMiss,
    Incomplete,
}

/// The result of computing a status cache write: `(fields_to_set, field_names_to_delete)`.
type StatusFieldWrites = (Vec<(String, Vec<u8>)>, Vec<String>);

pub struct DurableStreamRecoveryMetadata {
    pub(crate) covered_through: OplogIndex,
    pub(crate) sessions: Vec<(StreamSessionKey, SessionControlMetadata)>,
    pub(crate) consumer_deleting:
        Option<golem_common::model::durable_stream::StreamConsumerDeletingRecord>,
}

/// The potentially large parts of an [`AgentStatusRecord`] that are stored separately from `core`. They are
/// taken out of the record (`mem::take`) before serializing `core`, so this never clones the large
/// fields.
struct SplitStatusParts {
    invocation_results: InvocationResultMembership,
    received_card_transfers: ReceivedCardTransferIndex,
    failed_updates: Vec<FailedUpdateRecord>,
    successful_updates: Vec<SuccessfulUpdateRecord>,
    skipped_regions: DeletedRegions,
    deleted_regions: DeletedRegions,
}

#[derive(Debug, Clone)]
pub struct GetWorkerMetadataResult {
    // Status of the worker at the time of the create oplog entry
    pub initial_worker_metadata: AgentMetadata,
    // Last known cached status of the worker. Might be outdated
    pub last_known_status: Option<AgentStatusRecord>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAgentIdentity {
    pub agent_mode: AgentMode,
    pub fingerprint: AgentFingerprint,
    pub create_entry: OplogEntry,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
struct CachedAgentMode {
    agent_mode: AgentMode,
    fingerprint: AgentFingerprint,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
struct RunningWorker {
    owned_agent_id: OwnedAgentId,
    fingerprint: AgentFingerprint,
}

/// Requests the delete of the filesystem snapshots of a dead incarnation of an agent, whose
/// recovery-index member is stale.
pub type OnStale<'a> = &'a (dyn Fn(&OwnedAgentId, AgentFingerprint) + Send + Sync);

/// Service for persisting the current set of Golem workers represented by their metadata
#[async_trait]
pub trait WorkerService: Send + Sync {
    /// Loads the worker's initial metadata and last known cached status.
    ///
    /// `Ok(None)` means the worker has no oplog (it does not exist). Oplog storage failures follow
    /// the oplog service's fail-stop policy; other metadata and cache failures are returned.
    async fn get(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<GetWorkerMetadataResult>, WorkerExecutorError>;

    /// Enumerates the workers this executor must recover, per assigned shard.
    ///
    /// Returns `Err` when the recovery index or a worker behind it cannot be read. Each member
    /// whose agent has no oplog, or whose agent now has another incarnation, is stale: the scan
    /// calls `on_stale` with it and then removes it from the index. A scan that fails later keeps
    /// the requests that it made, and a member whose removal failed stays for the next scan.
    async fn get_running_workers_in_shards(
        &self,
        on_stale: OnStale<'_>,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError>;

    /// Deletes the worker: its cached status, its indexes, its oplog and its entry in the recovery
    /// index, in that order.
    ///
    /// Returns `Err` when the storage could not be reached. Safe to run again, and it is: by
    /// `Worker::delete`'s staged deletion and by the worker service's re-dispatch. The oplog goes
    /// after everything it can rebuild, so a run that stops part-way leaves the oplog the next one
    /// re-derives the rest from. Only the recovery-index entry follows it; one a crash leaves
    /// behind names no oplog, and the next recovery scan drops it.
    ///
    /// `expected_epoch` is the epoch the caller's oplog handle asserts, confirmed before anything is
    /// removed; refused, nothing is removed and the result is [`WorkerExecutorError::OplogFenced`],
    /// because the agent's state belongs to the shard's new owner. `None` is an ephemeral oplog,
    /// which nothing fences.
    ///
    /// `after_oplog_delete` is called with `fingerprint` right after the oplog delete succeeded,
    /// or after it was skipped because the stored identity belongs to another incarnation or is
    /// gone. A call that fails before that point does not call it.
    async fn remove(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        expected_epoch: Option<ShardEpoch>,
        after_oplog_delete: &(dyn Fn(AgentFingerprint) + Send + Sync),
    ) -> Result<(), WorkerExecutorError>;

    /// Deletes every cached status blob for the worker (live cache, clean checkpoint, the legacy
    /// key and the dedicated `agent_mode` key), leaving the oplog untouched.
    async fn remove_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError>;

    /// Gives the stored indexes of the rejected automatic snapshot entries of the incarnation
    /// `fingerprint`, or an empty set when none is stored.
    async fn get_rejected_periodic_snapshots(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
    ) -> Result<HashSet<OplogIndex>, WorkerExecutorError> {
        Ok(HashSet::new())
    }

    /// Adds `oplog_indexes` to the stored rejected automatic snapshot entries of the incarnation
    /// `fingerprint`, and keeps only the entries that a start of `status` can still select, as
    /// [`kept_rejections`] says. So the stored set holds at most two entries. Each set of one
    /// incarnation is apart from the sets of the others. A write that changes nothing is skipped.
    async fn reject_periodic_snapshots(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _status: &AgentStatusRecord,
        _oplog_indexes: &HashSet<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        Err(WorkerExecutorError::runtime(
            "snapshot rejection storage is unavailable",
        ))
    }

    async fn lookup_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        key: &IdempotencyKey,
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError>;

    async fn lookup_durable_stream_public_binding(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        public_session_id: &str,
    ) -> Result<Option<DurableStreamPublicBinding>, SessionError>;

    /// Reads per-session metadata through a captured persisted horizon, independently of status
    /// publication. Payloads remain in the oplog and are addressed by index.
    async fn lookup_durable_stream_control_metadata(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _key: &StreamSessionKey,
    ) -> Result<SessionControlMetadata, SessionError> {
        Err("durable stream control metadata is unavailable".into())
    }

    async fn lookup_durable_stream_producer_metadata(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _keys: Vec<ProducerMetadataKey>,
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), SessionError> {
        Err("durable stream producer metadata is unavailable".into())
    }

    async fn read_durable_stream_consumer_page(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _key: &StreamSessionKey,
        _reader: golem_common::model::durable_stream::LocalStreamReaderId,
        _page: u64,
    ) -> Result<Vec<OplogIndex>, SessionError> {
        Err("durable stream consumer index is unavailable".into())
    }

    async fn lookup_durable_stream_resume_offset(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _key: &StreamSessionKey,
        _attempt: golem_common::model::durable_stream::AttemptId,
    ) -> Result<Option<OplogIndex>, SessionError> {
        Err("durable stream resume index is unavailable".into())
    }

    async fn lookup_durable_stream_recovery_metadata(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
    ) -> Result<DurableStreamRecoveryMetadata, SessionError> {
        Err("durable stream recovery index is unavailable".into())
    }

    async fn catch_up_invocation_result_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _status: &AgentStatusRecord,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn lookup_invocation_result_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _status: &AgentStatusRecord,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationResultIndexLookup, String> {
        Ok(InvocationResultIndexLookup::Incomplete)
    }

    /// Resolves the authoritative `Create` entry. The cached mode only selects which oplog
    /// namespace to probe first and is never proof that a worker still exists.
    async fn resolve_agent_identity(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError>;

    /// Writes the cached status *blob* for the worker (no `RunningWorkers` index maintenance).
    ///
    /// The cached `AgentStatusRecord` is stored split across several fields of a per-agent hash
    /// (see [`KeyValueStorageNamespace::AgentStatus`]): a small `core`, the bounded `membership`,
    /// the `regions`, the `updates`, and one field per received card transfer. Only fields that
    /// changed are written. The complete invocation-result index is maintained in its dedicated
    /// namespace.
    ///
    /// `previous_status` is the status currently held in the cache (i.e. the last value
    /// successfully written). When provided, the delta of changed fields is computed against it.
    /// Pass `None` on cold paths (worker create, recompute after a cache miss, detach reload) where
    /// the previously stored fields are reconciled by reading them back.
    ///
    /// On success the (reassembled) status is returned so the caller can use it as the baseline
    /// for the next delta. The `AgentMode` is read from `status_value.agent_mode`. Cached status is
    /// only written for durable workers; for ephemeral workers this is a no-op (returning the
    /// passed status unchanged).
    ///
    /// Returns `Err` instead of panicking so the background flusher can re-queue the worker on a
    /// transient storage failure.
    async fn write_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError>;

    /// Reads the worker's *clean* status checkpoint, if any.
    ///
    /// The checkpoint is a full `AgentStatusRecord` written only at structurally clean boundaries
    /// (snapshot save / throttled idle) and stored in its own per-agent hash
    /// (see [`KeyValueStorageNamespace::AgentStatusCheckpoint`]). Because it is never advanced into
    /// an open jump region, it serves as a fold baseline for status recompute that predates any
    /// later jump, avoiding a full re-read of the oplog from index 1.
    ///
    /// Returns `Ok(None)` on a cache miss or stale format, and `Err` when the storage could not be
    /// read. The transient `agent_mode` field (not part of the persisted `core`) is restored from
    /// `agent_mode`.
    async fn read_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        agent_mode: AgentMode,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError>;

    /// Writes the worker's *clean* status checkpoint blob.
    ///
    /// Same split layout and delta semantics as [`write_cached_status`](Self::write_cached_status),
    /// but targeting the [`KeyValueStorageNamespace::AgentStatusCheckpoint`] hash. No-op for
    /// ephemeral workers (returning the value unchanged). Returns the reassembled record so the
    /// caller can use it as the baseline for the next delta. Returns `Err` instead of panicking so
    /// callers can treat checkpoint writes as best-effort (the oplog remains the source of truth).
    async fn write_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_checkpoint: Option<&AgentStatusRecord>,
        checkpoint: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError>;

    /// Updates the `RunningWorkers` recovery index for the worker according to `status_value`.
    ///
    /// This is the authoritative index consulted on crash/reshard recovery to decide which workers
    /// to resume, so it is always maintained synchronously (never deferred to the background
    /// flusher). The worker is added when [`should_track_for_assignment_recovery`] holds and
    /// removed otherwise. No-op for ephemeral workers.
    ///
    /// Returns the storage error if the index could not be updated. No caller may carry on as if
    /// the worker were tracked: an agent missing from this index is an agent that a crash or a
    /// reshard will not resume, so it silently stops running with nothing recording that it should
    /// be. [`update_cached_status`](Self::update_cached_status) fails the operation it was part of,
    /// and the hot path (`AgentStatusFlusher::on_status_changed`) treats it as fatal.
    async fn set_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status_value: &AgentStatusRecord,
    ) -> Result<(), String>;

    /// Removes the recovery-index member of the incarnation `fingerprint` of the agent when it
    /// is stale: when the agent has no oplog, or its oplog belongs to another incarnation. It
    /// calls `on_stale` before the removal, so a removal that fails leaves the member for the next
    /// scan. Gives `true` when the member was stale, and `false` when the incarnation is the
    /// stored one.
    async fn remove_if_stale(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        on_stale: OnStale<'_>,
    ) -> Result<bool, WorkerExecutorError>;

    /// Convenience cold-path helper that updates the recovery index *and* writes the blob in one
    /// call. Hot paths use [`set_assignment_tracking`](Self::set_assignment_tracking) (index)
    /// together with the background flusher (blob) instead.
    ///
    /// The index is updated first and its failure is not swallowed. Both callers go on to make the
    /// worker runnable, and a worker that runs while missing from `RunningWorkers` is one that no
    /// crash or reshard will resume - reporting success here would hand the caller that state
    /// silently.
    ///
    /// Both failures are returned rather than fatal. Both callers reach this from a path that can
    /// return an error, and a storage blip here would otherwise abort an executor holding live
    /// agents - forcing every one of them to replay from oplog or snapshot, which is far more
    /// expensive than failing the one operation that could not write.
    async fn update_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<(), WorkerExecutorError> {
        self.set_assignment_tracking(owned_agent_id, fingerprint, &status_value)
            .await
            .map_err(WorkerExecutorError::runtime)?;
        self.write_cached_status(owned_agent_id, fingerprint, previous_status, status_value)
            .await
            .map(|_| ())
    }
}

#[derive(Clone)]
pub struct DefaultWorkerService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    shard_service: Arc<dyn ShardService>,
    oplog_service: Arc<dyn OplogService>,
    component_service: Arc<dyn ComponentService>,
    config: Arc<GolemConfig>,
    lifecycle_gates: Arc<AgentLifecycleGates>,
    stream_session_index: Arc<StreamSessionIndexService>,
    invocation_result_index_locks: Arc<StdMutex<HashMap<OwnedAgentId, Weak<AsyncMutex<()>>>>>,
}

#[derive(Default)]
struct AgentLifecycleGates {
    gates: StdMutex<HashMap<OwnedAgentId, Weak<RwLock<()>>>>,
}

struct AgentLifecycleGate {
    owned_agent_id: OwnedAgentId,
    gate: Arc<RwLock<()>>,
    registry: Arc<AgentLifecycleGates>,
}

impl Drop for AgentLifecycleGate {
    fn drop(&mut self) {
        let mut gates = self.registry.gates.lock().unwrap();
        if Arc::strong_count(&self.gate) == 1
            && gates
                .get(&self.owned_agent_id)
                .is_some_and(|registered| registered.ptr_eq(&Arc::downgrade(&self.gate)))
        {
            gates.remove(&self.owned_agent_id);
        }
    }
}

impl AgentLifecycleGates {
    fn acquire(self: &Arc<Self>, owned_agent_id: &OwnedAgentId) -> AgentLifecycleGate {
        let mut gates = self.gates.lock().unwrap();
        let gate = gates
            .get(owned_agent_id)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let gate = Arc::new(RwLock::new(()));
                gates.insert(owned_agent_id.clone(), Arc::downgrade(&gate));
                gate
            });

        AgentLifecycleGate {
            owned_agent_id: owned_agent_id.clone(),
            gate,
            registry: self.clone(),
        }
    }
}

struct InvocationResultIndexLock {
    registry: Arc<StdMutex<HashMap<OwnedAgentId, Weak<AsyncMutex<()>>>>>,
    owned_agent_id: OwnedAgentId,
    inner: Arc<AsyncMutex<()>>,
}

impl Drop for InvocationResultIndexLock {
    fn drop(&mut self) {
        let mut locks = self.registry.lock().unwrap();
        if Arc::strong_count(&self.inner) == 1
            && locks
                .get(&self.owned_agent_id)
                .is_some_and(|registered| registered.ptr_eq(&Arc::downgrade(&self.inner)))
        {
            locks.remove(&self.owned_agent_id);
        }
    }
}

impl DefaultWorkerService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        oplog_service: Arc<dyn OplogService>,
        component_service: Arc<dyn ComponentService>,
        config: Arc<GolemConfig>,
    ) -> Self {
        let stream_session_index = oplog_service.stream_session_index().unwrap_or_else(|| {
            let index = Arc::new(StreamSessionIndexService::new(
                key_value_storage.clone(),
                Arc::downgrade(&oplog_service),
            ));
            oplog_service.set_stream_session_index(index.clone());
            index
        });
        Self {
            key_value_storage,
            shard_service,
            oplog_service,
            component_service,
            config,
            lifecycle_gates: Arc::new(AgentLifecycleGates::default()),
            stream_session_index,
            invocation_result_index_locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    fn invocation_result_index_lock(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> InvocationResultIndexLock {
        let mut locks = self.invocation_result_index_locks.lock().unwrap();
        let inner = if let Some(lock) = locks.get(owned_agent_id).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(AsyncMutex::new(()));
            locks.insert(owned_agent_id.clone(), Arc::downgrade(&lock));
            lock
        };
        InvocationResultIndexLock {
            registry: self.invocation_result_index_locks.clone(),
            owned_agent_id: owned_agent_id.clone(),
            inner,
        }
    }

    fn lifecycle_gate(&self, owned_agent_id: &OwnedAgentId) -> AgentLifecycleGate {
        self.lifecycle_gates.acquire(owned_agent_id)
    }

    fn invocation_result_index_namespace(
        agent_id: &AgentId,
        fingerprint: AgentFingerprint,
    ) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentInvocationResultIndex {
            agent_id: agent_id.clone(),
            fingerprint,
        }
    }

    fn rejected_periodic_snapshots_namespace(agent_id: &AgentId) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
            agent_id: agent_id.clone(),
        }
    }

    fn rejected_periodic_snapshots_field(fingerprint: AgentFingerprint) -> String {
        fingerprint.0.to_string()
    }

    async fn remove_all_fields(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
        description: &str,
    ) -> Result<(), WorkerExecutorError> {
        let fields = self
            .key_value_storage
            .with("worker", "remove")
            .keys(namespace.clone())
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to list {description} fields for {owned_agent_id}: {err}"
                ))
            })?;
        if !fields.is_empty() {
            self.key_value_storage
                .with("worker", "remove")
                .del_many(namespace, fields.into())
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to remove {description} for {owned_agent_id}: {err}"
                    ))
                })?;
        }
        Ok(())
    }
}

impl HasOplogService for DefaultWorkerService {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        self.oplog_service.clone()
    }
}

impl HasConfig for DefaultWorkerService {
    fn config(&self) -> Arc<GolemConfig> {
        self.config.clone()
    }
}

impl HasComponentService for DefaultWorkerService {
    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }
}

#[cfg(test)]
pub(crate) mod session_index_tests;

#[cfg(test)]
mod tests;
