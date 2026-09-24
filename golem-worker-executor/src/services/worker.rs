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

use super::component::ComponentService;
use super::golem_config::GolemConfig;
use super::{HasComponentService, HasConfig, HasOplogService};
use crate::durable_host::durable_stream::SessionControlMetadata;
use crate::durable_host::durable_stream::metadata::{ProducerMetadataKey, ProducerMetadataRow};
use crate::metrics::workers::{
    record_agent_identity_resolution, record_derived_cache_publication_failed,
    record_stale_running_worker, record_status_cache_publication, record_worker_call,
};
use crate::services::oplog::{OplogError, OplogLifecycleGuard, OplogService};
use crate::services::shard::ShardService;
use crate::services::stream_session_index::StreamSessionIndexService;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
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

/// Hash field holding the small part of the cached `AgentStatusRecord`. Always present for a cached
/// status; its absence is treated as a cache miss.
const STATUS_CORE_FIELD: &str = "core";
/// Hash field holding the bounded invocation-result membership. Written only when it changes.
const STATUS_MEMBERSHIP_FIELD: &str = "membership";
/// Hash field holding `(skipped_regions, deleted_regions)`. Written only when the regions change.
const STATUS_REGIONS_FIELD: &str = "regions";
/// Hash field holding `(failed_updates, successful_updates)`. Written only when they change.
const STATUS_UPDATES_FIELD: &str = "updates";
/// Prefix for per-transfer target receipt fields (`tr:{transfer_id}` -> receipt identity).
const STATUS_RECEIVED_CARD_TRANSFER_PREFIX: &str = "tr:";
const INVOCATION_RESULT_INDEX_METADATA_FIELD: &str = "metadata";
const INVOCATION_RESULT_INDEX_FIELD_PREFIX: &str = "ir:";

fn status_received_card_transfer_field(transfer_id: &uuid::Uuid) -> String {
    format!("{STATUS_RECEIVED_CARD_TRANSFER_PREFIX}{transfer_id}")
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

/// Moves the separately persisted fields out of `status`, leaving the small `core` that is
/// serialized into the `core` field. Uses `mem::take`/`mem::replace`, so it does not clone the
/// potentially large updates, regions, or transfer index.
fn split_status(status: &mut AgentStatusRecord) -> SplitStatusParts {
    SplitStatusParts {
        invocation_results: std::mem::replace(
            &mut status.invocation_results,
            InvocationResultMembership::new(0, 1, 1),
        ),
        received_card_transfers: std::mem::take(&mut status.received_card_transfers),
        failed_updates: std::mem::take(&mut status.failed_updates),
        successful_updates: std::mem::take(&mut status.successful_updates),
        skipped_regions: std::mem::replace(&mut status.skipped_regions, DeletedRegions::new()),
        deleted_regions: std::mem::replace(&mut status.deleted_regions, DeletedRegions::new()),
    }
}

fn status_core(status: &AgentStatusRecord) -> AgentStatusRecord {
    AgentStatusRecord {
        status: status.status,
        last_error_kind: status.last_error_kind,
        skipped_regions: DeletedRegions::new(),
        overridden_retry_config: status.overridden_retry_config.clone(),
        pending_invocations: status.pending_invocations.clone(),
        pending_card_events: status.pending_card_events.clone(),
        pending_updates: status.pending_updates.clone(),
        failed_updates: Vec::new(),
        successful_updates: Vec::new(),
        invocation_results: InvocationResultMembership::new(0, 1, 1),
        received_card_transfers: ReceivedCardTransferIndex::default(),
        durable_stream_sessions: status.durable_stream_sessions.clone(),
        pending_durable_stream_cancellations: status.pending_durable_stream_cancellations.clone(),
        has_durable_stream_history: status.has_durable_stream_history,
        current_idempotency_key: status.current_idempotency_key.clone(),
        cancelled_idempotency_key: status.cancelled_idempotency_key.clone(),
        component_revision: status.component_revision,
        component_size: status.component_size,
        total_linear_memory_size: status.total_linear_memory_size,
        owned_resources: status.owned_resources.clone(),
        oplog_idx: status.oplog_idx,
        active_plugins: status.active_plugins.clone(),
        oplog_processor_checkpoints: status.oplog_processor_checkpoints.clone(),
        revoked_cards: status.revoked_cards.clone(),
        deleted_regions: DeletedRegions::new(),
        component_revision_for_replay: status.component_revision_for_replay,
        current_retry_state: status.current_retry_state.clone(),
        last_manual_update_snapshot_index: status.last_manual_update_snapshot_index,
        last_automatic_snapshot_index: status.last_automatic_snapshot_index,
        last_automatic_snapshot_timestamp: status.last_automatic_snapshot_timestamp,
        last_automatic_snapshot_component_revision: status
            .last_automatic_snapshot_component_revision,
        agent_mode: status.agent_mode,
        export_fork_admissions: status.export_fork_admissions.clone(),
    }
}

/// Computes the minimal set of hash-field writes/deletes needed to bring the cached status up to
/// date.
///
/// `core` must be the already-split (emptied) record. When `previous` is `Some`, the result is a
/// delta against it (this is the hot path, where `dels` is usually empty). When `previous` is
/// `None` (cold path: create / cache-miss recompute / detach reload), every part is written and
/// `existing_split_fields` is used to delete stale `tr:` fields.
///
/// `core` is always part of `sets` (it carries the `oplog_idx` marker), so the marker and every
/// written part advance together in one atomic `set_many` by the caller.
fn compute_status_field_writes(
    previous: Option<&AgentStatusRecord>,
    existing_split_fields: &[String],
    core: &AgentStatusRecord,
    parts: &SplitStatusParts,
) -> Result<StatusFieldWrites, String> {
    let mut sets: Vec<(String, Vec<u8>)> = Vec::new();
    let mut dels: Vec<String> = Vec::new();

    sets.push((STATUS_CORE_FIELD.to_string(), serialize(core)?));

    let membership_changed = match previous {
        Some(previous) => previous.invocation_results != parts.invocation_results,
        None => true,
    };
    if membership_changed {
        sets.push((
            STATUS_MEMBERSHIP_FIELD.to_string(),
            serialize(&parts.invocation_results)?,
        ));
    }

    let regions_changed = match previous {
        Some(previous) => {
            previous.skipped_regions != parts.skipped_regions
                || previous.deleted_regions != parts.deleted_regions
        }
        None => true,
    };
    if regions_changed {
        sets.push((
            STATUS_REGIONS_FIELD.to_string(),
            serialize(&(&parts.skipped_regions, &parts.deleted_regions))?,
        ));
    }

    let updates_changed = match previous {
        Some(previous) => {
            previous.failed_updates != parts.failed_updates
                || previous.successful_updates != parts.successful_updates
        }
        None => true,
    };
    if updates_changed {
        sets.push((
            STATUS_UPDATES_FIELD.to_string(),
            serialize(&(&parts.failed_updates, &parts.successful_updates))?,
        ));
    }

    match previous {
        Some(previous) => {
            for (transfer_id, state) in parts
                .received_card_transfers
                .changes_from(&previous.received_card_transfers)
            {
                let field = status_received_card_transfer_field(&transfer_id);
                match state {
                    Some(state) => sets.push((field, serialize(state)?)),
                    None => dels.push(field),
                }
            }
        }
        None => {
            let new_transfer_fields: HashSet<String> = parts
                .received_card_transfers
                .iter()
                .map(|(transfer_id, _)| status_received_card_transfer_field(transfer_id))
                .collect();
            for (transfer_id, state) in parts.received_card_transfers.iter() {
                sets.push((
                    status_received_card_transfer_field(transfer_id),
                    serialize(state)?,
                ));
            }

            for field in existing_split_fields {
                if field.starts_with(STATUS_RECEIVED_CARD_TRANSFER_PREFIX)
                    && !new_transfer_fields.contains(field)
                {
                    dels.push(field.clone());
                }
            }
        }
    }

    Ok((sets, dels))
}

/// Reassembles a cached [`AgentStatusRecord`] from the split hash fields. Returns `None` if the
/// `core` field is missing (cache miss) or any field fails to deserialize in the current format
/// (treated as a cache miss). `agent_mode` is `#[transient]` and not part of `core`, so the
/// returned record carries the `Durable` deserialization default; callers restore it.
fn reassemble_cached_status(
    fields: impl IntoIterator<Item = (String, bytes::Bytes)>,
) -> Option<AgentStatusRecord> {
    let mut core: Option<AgentStatusRecord> = None;
    let mut invocation_results: Option<InvocationResultMembership> = None;
    let mut regions: Option<(DeletedRegions, DeletedRegions)> = None;
    let mut updates: Option<(Vec<FailedUpdateRecord>, Vec<SuccessfulUpdateRecord>)> = None;
    let mut received_card_transfers = ReceivedCardTransferIndex::default();

    for (name, bytes) in fields {
        if name == STATUS_CORE_FIELD {
            core = Some(deserialize::<AgentStatusRecord>(&bytes).ok()?);
        } else if name == STATUS_MEMBERSHIP_FIELD {
            invocation_results = Some(deserialize::<InvocationResultMembership>(&bytes).ok()?);
        } else if name == STATUS_REGIONS_FIELD {
            regions = Some(deserialize::<(DeletedRegions, DeletedRegions)>(&bytes).ok()?);
        } else if name == STATUS_UPDATES_FIELD {
            updates = Some(
                deserialize::<(Vec<FailedUpdateRecord>, Vec<SuccessfulUpdateRecord>)>(&bytes)
                    .ok()?,
            );
        } else if let Some(transfer_id) = name.strip_prefix(STATUS_RECEIVED_CARD_TRANSFER_PREFIX) {
            let transfer_id = uuid::Uuid::parse_str(transfer_id).ok()?;
            let state = deserialize::<ReceivedCardTransferState>(&bytes).ok()?;
            received_card_transfers.insert(transfer_id, state);
        }
        // Unknown fields are ignored.
    }

    let mut status = core?;
    status.invocation_results = invocation_results?;
    if let Some((skipped_regions, deleted_regions)) = regions {
        status.skipped_regions = skipped_regions;
        status.deleted_regions = deleted_regions;
    }
    if let Some((failed_updates, successful_updates)) = updates {
        status.failed_updates = failed_updates;
        status.successful_updates = successful_updates;
    }
    status.received_card_transfers = received_card_transfers;
    Some(status)
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
    /// Returns `Err` when the recovery index itself could not be read; individual workers that
    /// cannot be loaded are skipped and logged, so one of them cannot block the rest.
    async fn get_running_workers_in_shards(
        &self,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError>;

    /// Deletes the worker: its oplog, its cached status and its entry in the recovery index.
    ///
    /// Returns `Err` when the storage could not be reached. Delete is not retried by the caller:
    /// a retry would re-run the oplog delete, so the error is reported instead.
    ///
    /// `expected_epoch` is the epoch the caller's oplog handle asserts. The oplog is deleted only
    /// while this executor still holds it at that epoch; otherwise nothing at all is removed and
    /// the result is [`WorkerExecutorError::OplogFenced`], because the agent's state belongs to
    /// the shard's new owner.
    async fn remove(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), WorkerExecutorError>;

    /// Deletes every cached status blob for the worker (live cache, clean checkpoint, the legacy
    /// key and the dedicated `agent_mode` key), leaving the oplog untouched.
    async fn remove_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError>;

    async fn get_rejected_periodic_snapshot_through(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        Ok(None)
    }

    async fn reject_periodic_snapshots_through(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _oplog_index: OplogIndex,
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
    ) -> Result<Option<DurableStreamSessionStatus>, String>;

    async fn lookup_durable_stream_public_binding(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        public_session_id: &str,
    ) -> Result<Option<DurableStreamPublicBinding>, String>;

    /// Reads per-session metadata through a captured persisted horizon, independently of status
    /// publication. Payloads remain in the oplog and are addressed by index.
    async fn lookup_durable_stream_control_metadata(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _key: &StreamSessionKey,
    ) -> Result<SessionControlMetadata, String> {
        Err("durable stream control metadata is unavailable".into())
    }

    async fn lookup_durable_stream_producer_metadata(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _keys: Vec<ProducerMetadataKey>,
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), String> {
        Err("durable stream producer metadata is unavailable".into())
    }

    async fn read_durable_stream_consumer_page(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _key: &StreamSessionKey,
        _reader: golem_common::model::durable_stream::LocalStreamReaderId,
        _page: u64,
    ) -> Result<Vec<OplogIndex>, String> {
        Err("durable stream consumer index is unavailable".into())
    }

    async fn lookup_durable_stream_resume_offset(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _key: &StreamSessionKey,
        _attempt: golem_common::model::durable_stream::AttemptId,
    ) -> Result<Option<OplogIndex>, String> {
        Err("durable stream resume index is unavailable".into())
    }

    async fn lookup_durable_stream_recovery_metadata(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
    ) -> Result<DurableStreamRecoveryMetadata, String> {
        Err("durable stream recovery index is unavailable".into())
    }

    async fn catch_up_invocation_result_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _status: &AgentStatusRecord,
    ) -> Result<(), String> {
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
    ) -> Result<AgentStatusRecord, String>;

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
    ) -> Result<AgentStatusRecord, String>;

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

    async fn remove_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), String>;

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
    ) -> Result<(), String> {
        self.set_assignment_tracking(owned_agent_id, fingerprint, &status_value)
            .await?;
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

    async fn enum_workers_at_key(
        &self,
        key: &str,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError> {
        record_worker_call("enum");

        // The index itself is not per-worker: without it there is no list of workers to recover,
        // so this failure is reported to the caller rather than silently yielding an empty shard.
        let value: Vec<RunningWorker> = self
            .key_value_storage
            .with_entity("worker", "enum", "agent_id")
            .members_of_set(KeyValueStorageNamespace::RunningWorkers, key)
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get worker ids from KV storage: {err}"
                ))
            })?;

        let mut workers = Vec::new();

        for running_worker in value {
            let owned_agent_id = &running_worker.owned_agent_id;
            // Only a worker that is genuinely gone may be skipped. The two outcomes are not the
            // same failure wearing different clothes:
            //
            // - `Ok(None)` means there is no oplog, so the index entry outlived the worker - a
            //   delete that raced this read. There is nothing to resume, and skipping is correct.
            // - `Err` means the entry may well exist and we simply could not read it. Skipping
            //   would leave a running agent suspended for as long as this executor owns the shard,
            //   which breaks the guarantee that a reshard or a crash does not stop a running
            //   worker - and it would do so silently. Failing the scan is the safe answer: a
            //   transient cause is already absorbed by the storage retry budget, and one that
            //   outlives it leaves this executor unable to do its job, so replacing it beats
            //   carrying on with an unknown number of agents stranded.
            let identity = self.resolve_agent_identity(owned_agent_id).await?;
            if identity
                .as_ref()
                .is_none_or(|identity| identity.fingerprint != running_worker.fingerprint)
            {
                let reason = if identity.is_none() {
                    "absent"
                } else {
                    "fingerprint_mismatch"
                };
                self.remove_running_worker_member(key, &running_worker)
                    .await?;
                record_stale_running_worker(reason);
                debug!("Skipping {owned_agent_id} during recovery: stale recovery-index member");
                continue;
            }

            match self.get(owned_agent_id).await {
                Ok(Some(metadata))
                    if metadata.initial_worker_metadata.fingerprint
                        == running_worker.fingerprint =>
                {
                    workers.push(metadata)
                }
                Ok(Some(_)) => {
                    self.remove_running_worker_member(key, &running_worker)
                        .await?;
                    record_stale_running_worker("fingerprint_mismatch");
                    debug!(
                        "Skipping {owned_agent_id} during recovery: stale recovery-index member"
                    );
                }
                Ok(None) => {
                    let current = self.resolve_agent_identity(owned_agent_id).await?;
                    if current
                        .as_ref()
                        .is_none_or(|identity| identity.fingerprint != running_worker.fingerprint)
                    {
                        let reason = if current.is_none() {
                            "absent"
                        } else {
                            "fingerprint_mismatch"
                        };
                        self.remove_running_worker_member(key, &running_worker)
                            .await?;
                        record_stale_running_worker(reason);
                    } else {
                        return Err(WorkerExecutorError::runtime(format!(
                            "failed to load metadata for existing {owned_agent_id} during recovery"
                        )));
                    }
                }
                Err(err) => {
                    return Err(WorkerExecutorError::runtime(format!(
                        "failed to read {owned_agent_id} during recovery, so it cannot be resumed: {err}"
                    )));
                }
            }
        }

        Ok(workers)
    }

    async fn remove_running_worker_member(
        &self,
        key: &str,
        running_worker: &RunningWorker,
    ) -> Result<(), WorkerExecutorError> {
        self.key_value_storage
            .with_entity("worker", "remove_stale", "agent_id")
            .remove_from_set(
                KeyValueStorageNamespace::RunningWorkers,
                key,
                running_worker,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to remove stale worker from the recovery index: {err}"
                ))
            })
    }

    /// Namespace holding the agent's split cached status (one per-agent hash whose fields are
    /// `core`, `membership`, `regions`, `updates`, and `tr:{transfer_id}`).
    fn status_namespace(
        agent_id: &AgentId,
        fingerprint: AgentFingerprint,
    ) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentStatus {
            agent_id: Arc::new(agent_id.clone()),
            fingerprint,
        }
    }

    /// Namespace holding the agent's *clean* status checkpoint. Same physical split layout as
    /// [`Self::status_namespace`], but written only at structurally clean boundaries (snapshot
    /// save / throttled idle) and never advanced by the background flusher, so it can serve as a
    /// fold baseline that always predates any later jump region.
    fn checkpoint_namespace(
        agent_id: &AgentId,
        fingerprint: AgentFingerprint,
    ) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentStatusCheckpoint {
            agent_id: Arc::new(agent_id.clone()),
            fingerprint,
        }
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

    /// Key holding only the worker's immutable `AgentMode`, stored separately from the status
    /// so `get_agent_mode` can resolve the oplog namespace without reading the whole
    /// `AgentStatusRecord`. Populated lazily on a `get_agent_mode` cache miss (durable workers
    /// only); never written on the per-commit hot path. The value never changes for the life of
    /// the worker. Lives in the `Worker` namespace (not `AgentStatus`) since it has an independent
    /// lifecycle from the status fields.
    fn agent_mode_key(agent_id: &AgentId) -> String {
        format!("worker:agent_mode:{}", agent_id.to_redis_key())
    }

    fn running_in_shard_key(shard_id: &ShardId) -> String {
        format!("worker:running_in_shard:{shard_id}")
    }

    /// Reads the cached `AgentStatusRecord` for `owned_agent_id`, if any, reassembling it from the
    /// split hash fields (`core`, `membership`, `regions`, `updates`, `tr:{transfer_id}`). Returns
    /// `None` if the `core` field is missing (cache miss) or any field cannot be deserialized in
    /// the current format (treated as a cache miss).
    ///
    /// `agent_mode` is `#[transient]` and not part of `core`, so the returned record carries the
    /// `Durable` deserialization default; callers must restore it from the authoritative source.
    async fn read_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError> {
        self.read_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id, fingerprint),
        )
        .await
    }

    /// Reads a split status record (live cache or checkpoint) from `namespace`, reassembling it
    /// from the `core` / `membership` / `regions` / `updates` / `tr:{transfer_id}` fields. Returns
    /// `None` if `core` is missing (cache miss / torn write) or any field cannot be deserialized in
    /// the current format.
    ///
    /// `agent_mode` is `#[transient]` and not part of `core`, so the returned record carries the
    /// `Durable` deserialization default; callers must restore it from the authoritative source.
    async fn read_split_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError> {
        // Single atomic read of every field of the per-agent status hash (`core`, `membership`,
        // `regions`, `updates`, `tr:{transfer_id}`). This is one round-trip (Redis `HGETALL`, a single
        // `SELECT ... WHERE namespace`, or one locked scan in memory) that observes a consistent
        // snapshot, so it cannot reassemble a torn, mixed-generation record. (A naive `keys` +
        // `get_many` would be two round-trips, leaving a window where a concurrent writer — the
        // background status flusher for the live cache, or the clean-checkpoint writer for the
        // checkpoint namespace — could add a new split field that the earlier `keys` did not list,
        // yielding a record at the newer `core.oplog_idx` missing derived status.)
        let fields = self
            .key_value_storage
            .with_entity("worker", "read_cached_status", "agent_status")
            .get_all_raw(namespace)
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get agent status for {owned_agent_id} from KV storage: {err}"
                ))
            })?;

        // No `core` field -> nothing cached (or a torn/partial write); treat as a cache miss.
        if !fields.iter().any(|(name, _)| name == STATUS_CORE_FIELD) {
            return Ok(None);
        }

        Ok(reassemble_cached_status(fields))
    }

    /// Writes the split status fields for an agent, sending only the parts that changed.
    ///
    /// `core` is always written (it carries the `oplog_idx` marker that versions the whole record).
    /// `membership`/`regions`/`updates` are written only when they differ from `previous_status`,
    /// and received card transfers are written per key (only newly added/changed keys).
    ///
    /// Publication compares the exact serialized baseline `core` and atomically applies all sets,
    /// stale-field deletions, and expiry. A mismatch reloads one complete hash snapshot and retries
    /// with a full reconciliation guarded by that snapshot's raw core. Thus an expired hash or a
    /// concurrent publisher can never leave auxiliary fields from a different core generation.
    async fn write_status_fields(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
        previous_status: Option<&AgentStatusRecord>,
        core: &AgentStatusRecord,
        parts: &SplitStatusParts,
    ) -> Result<(), String> {
        let cache_kind = match &namespace {
            KeyValueStorageNamespace::AgentStatus { .. } => "status",
            KeyValueStorageNamespace::AgentStatusCheckpoint { .. } => "checkpoint",
            _ => unreachable!("split status must use a status cache namespace"),
        };
        let (mut expected_core, mut existing_fields, mut use_delta) =
            if let Some(previous) = previous_status {
                let expected = serialize(&status_core(previous)).map_err(|err| {
                    format!("failed to serialize agent status for {owned_agent_id}: {err}")
                })?;
                (Some(expected), Vec::new(), true)
            } else {
                let snapshot = self
                    .key_value_storage
                    .with_entity("worker", "update_status", "agent_status")
                    .get_all_raw(namespace.clone())
                    .await
                    .map_err(|err| {
                        format!("failed to read agent status for {owned_agent_id}: {err}")
                    })?;
                let expected = snapshot
                    .iter()
                    .find(|(field, _)| field == STATUS_CORE_FIELD)
                    .map(|(_, value)| value.to_vec());
                let fields = snapshot.into_iter().map(|(field, _)| field).collect();
                (expected, fields, false)
            };

        loop {
            let baseline = use_delta.then_some(previous_status).flatten();
            let (sets, dels) = compute_status_field_writes(baseline, &existing_fields, core, parts)
                .map_err(|err| {
                    format!("failed to serialize agent status for {owned_agent_id}: {err}")
                })?;
            let pairs: Vec<(&str, &[u8])> = sets
                .iter()
                .map(|(field, bytes)| (field.as_str(), bytes.as_slice()))
                .collect();
            let deletions: Vec<&str> = dels.iter().map(String::as_str).collect();

            let published = self
                .key_value_storage
                .with_entity("worker", "update_status", "agent_status")
                .compare_and_mutate_many_raw(
                    namespace.clone(),
                    STATUS_CORE_FIELD,
                    expected_core.as_deref(),
                    &pairs,
                    &deletions,
                    DERIVED_CACHE_EXPIRY,
                )
                .await
                .map_err(|err| {
                    record_derived_cache_publication_failed(cache_kind);
                    format!("failed to set agent status in KV storage: {err}")
                })?;
            if published {
                record_status_cache_publication(
                    cache_kind,
                    if use_delta {
                        "delta_match"
                    } else {
                        "complete_reconciliation"
                    },
                );
                return Ok(());
            }

            if use_delta {
                record_status_cache_publication(cache_kind, "reconciliation_fallback");
            }

            // The remembered baseline is stale or the hash expired. Reconcile a complete record
            // against one atomic snapshot and guard that snapshot's exact core on publication.
            let snapshot = self
                .key_value_storage
                .with_entity("worker", "update_status", "agent_status")
                .get_all_raw(namespace.clone())
                .await
                .map_err(|err| {
                    format!("failed to reload agent status for {owned_agent_id}: {err}")
                })?;
            expected_core = snapshot
                .iter()
                .find(|(field, _)| field == STATUS_CORE_FIELD)
                .map(|(_, value)| value.to_vec());
            existing_fields = snapshot.into_iter().map(|(field, _)| field).collect();
            use_delta = false;
        }
    }

    /// Splits `status_value` and writes it to `namespace` (live cache or checkpoint), sending only
    /// the changed parts (delta against `previous_status` when provided). Returns the reassembled
    /// record so the caller can use it as the baseline for the next delta. No-op for ephemeral
    /// workers (their status is never persisted), returning the value unchanged.
    async fn write_split_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, String> {
        if status_value.agent_mode == AgentMode::Ephemeral {
            return Ok(status_value);
        }

        // Split the record: take the potentially large fields out so `core` stays small.
        // `split_status` moves the large fields out of `core` into `parts` (no clone).
        let mut core = status_value;
        let parts = split_status(&mut core);

        self.write_status_fields(owned_agent_id, namespace, previous_status, &core, &parts)
            .await?;

        // Reassemble the record (moving the parts back into `core`, no clone) so the caller gets
        // back a complete baseline for computing the next delta.
        let mut reassembled = core;
        reassembled.invocation_results = parts.invocation_results;
        reassembled.skipped_regions = parts.skipped_regions;
        reassembled.deleted_regions = parts.deleted_regions;
        reassembled.failed_updates = parts.failed_updates;
        reassembled.successful_updates = parts.successful_updates;
        reassembled.received_card_transfers = parts.received_card_transfers;
        Ok(reassembled)
    }

    /// Deletes every field of a split status hash, guarded by the observed core so a concurrent
    /// complete publication is either removed atomically or forces a retry.
    async fn remove_split_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let snapshot = self
                .key_value_storage
                .with_entity("worker", "remove", "agent_status")
                .get_all_raw(namespace.clone())
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to read cached status for {owned_agent_id} during removal: {err}"
                    ))
                })?;
            if snapshot.is_empty() {
                return Ok(());
            }
            let expected_core = snapshot
                .iter()
                .find(|(field, _)| field == STATUS_CORE_FIELD)
                .map(|(_, value)| &value[..]);
            let deletions: Vec<&str> = snapshot.iter().map(|(field, _)| field.as_str()).collect();
            if self
                .key_value_storage
                .with_entity("worker", "remove", "agent_status")
                .compare_and_mutate_many_raw(
                    namespace.clone(),
                    STATUS_CORE_FIELD,
                    expected_core,
                    &[],
                    &deletions,
                    DERIVED_CACHE_EXPIRY,
                )
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to remove cached status for {owned_agent_id}: {err}"
                    ))
                })?
            {
                return Ok(());
            }
        }
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

    /// Reads the dedicated mode hint, if present. Invalid formats are cache misses.
    async fn read_cached_agent_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<CachedAgentMode>, WorkerExecutorError> {
        let value = self
            .key_value_storage
            .with_entity("worker", "read_cached_agent_mode", "agent_mode")
            .get_raw(
                KeyValueStorageNamespace::Worker {
                    agent_id: Arc::new(owned_agent_id.agent_id()),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get agent mode for {owned_agent_id} from KV storage: {err}"
                ))
            })?;

        Ok(value
            .as_deref()
            .and_then(|bytes| try_deserialize(bytes).ok().flatten()))
    }

    /// Publishes the durable mode hint atomically with its expiry. This is best-effort because the
    /// resolved `Create` entry, not this key, is authoritative.
    async fn write_cached_agent_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
        cached: &CachedAgentMode,
    ) -> Result<(), String> {
        let value = serialize(cached)?;
        self.key_value_storage
            .with_entity("worker", "write_cached_agent_mode", "agent_mode")
            .set_raw_with_expiry(
                KeyValueStorageNamespace::Worker {
                    agent_id: Arc::new(owned_agent_id.agent_id()),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
                &value,
                DERIVED_CACHE_EXPIRY,
            )
            .await
            .map_err(|err| {
                record_derived_cache_publication_failed("mode_hint");
                format!("failed to set agent mode in KV storage: {err}")
            })
    }

    async fn remove_cached_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Result<(), String> {
        self.key_value_storage
            .with("worker", "remove_cached_agent_mode")
            .del(
                KeyValueStorageNamespace::Worker {
                    agent_id: Arc::new(owned_agent_id.agent_id()),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
            )
            .await
    }

    async fn remove_cached_agent_mode_if_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError> {
        let namespace = KeyValueStorageNamespace::Worker {
            agent_id: Arc::new(owned_agent_id.agent_id()),
        };
        let key = Self::agent_mode_key(&owned_agent_id.agent_id);
        let current = self
            .key_value_storage
            .with_entity("worker", "remove_cached_agent_mode", "agent_mode")
            .get_raw(namespace.clone(), &key)
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let Some(current) = current else {
            return Ok(());
        };
        let matches = try_deserialize::<CachedAgentMode>(&current)
            .ok()
            .flatten()
            .is_some_and(|cached| cached.fingerprint == fingerprint);
        if matches {
            self.key_value_storage
                .with_entity("worker", "remove_cached_agent_mode", "agent_mode")
                .compare_and_mutate_many_raw(
                    namespace,
                    &key,
                    Some(&current),
                    &[],
                    &[&key],
                    DERIVED_CACHE_EXPIRY,
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;
        }
        Ok(())
    }

    async fn read_agent_identity_in_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        let entry = self
            .oplog_service
            .read_source(owned_agent_id, agent_mode, OplogIndex::INITIAL, 1)
            .await
            .remove(&OplogIndex::INITIAL);
        match entry {
            None => Ok(None),
            Some(ref create_entry @ OplogEntry::Create { ref parameters, .. })
                if parameters.agent_mode == agent_mode =>
            {
                Ok(Some(ResolvedAgentIdentity {
                    agent_mode,
                    fingerprint: AgentFingerprint(parameters.instance_id),
                    create_entry: create_entry.clone(),
                }))
            }
            Some(OplogEntry::Create { .. }) => Err(WorkerExecutorError::runtime(format!(
                "agent {owned_agent_id} has a Create entry in the wrong oplog namespace"
            ))),
            Some(_) => Err(WorkerExecutorError::runtime(format!(
                "agent {owned_agent_id} has a malformed oplog without an initial Create entry"
            ))),
        }
    }

    async fn read_agent_identity_for_deletion(
        &self,
        owned_agent_id: &OwnedAgentId,
        deleting_mode: AgentMode,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        if let Some(identity) = self
            .read_agent_identity_in_mode(owned_agent_id, deleting_mode)
            .await?
        {
            return Ok(Some(identity));
        }
        let alternate_mode = match deleting_mode {
            AgentMode::Durable => AgentMode::Ephemeral,
            AgentMode::Ephemeral => AgentMode::Durable,
        };
        self.read_agent_identity_in_mode(owned_agent_id, alternate_mode)
            .await
    }

    pub(crate) fn should_track_for_assignment_recovery(status: &AgentStatusRecord) -> bool {
        matches!(
            status.status,
            AgentStatus::Running | AgentStatus::Retrying | AgentStatus::Interrupted
        ) || status.has_pending_work()
            || !status.pending_durable_stream_cancellations.is_empty()
    }
}

#[async_trait]
impl WorkerService for DefaultWorkerService {
    async fn lookup_durable_stream_producer_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        keys: Vec<ProducerMetadataKey>,
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), String> {
        self.stream_session_index
            .lookup_producer_metadata(owned_agent_id, agent_mode, fingerprint, keys)
            .await
    }

    async fn lookup_durable_stream_recovery_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<DurableStreamRecoveryMetadata, String> {
        self.stream_session_index
            .lookup_recovery_metadata(owned_agent_id, agent_mode, fingerprint)
            .await
    }

    async fn lookup_durable_stream_resume_offset(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
        attempt: golem_common::model::durable_stream::AttemptId,
    ) -> Result<Option<OplogIndex>, String> {
        self.stream_session_index
            .lookup_resume_offset(owned_agent_id, agent_mode, fingerprint, key, attempt)
            .await
    }

    async fn lookup_durable_stream_control_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
    ) -> Result<SessionControlMetadata, String> {
        self.stream_session_index
            .lookup_control_metadata(owned_agent_id, agent_mode, fingerprint, key)
            .await
    }

    async fn read_durable_stream_consumer_page(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
        reader: golem_common::model::durable_stream::LocalStreamReaderId,
        page: u64,
    ) -> Result<Vec<OplogIndex>, String> {
        self.stream_session_index
            .read_consumer_page(owned_agent_id, fingerprint, key, reader, page)
            .await
    }

    #[tracing::instrument(name = "worker_metadata.get", level = "debug", skip_all)]
    async fn get(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<GetWorkerMetadataResult>, WorkerExecutorError> {
        let lifecycle_gate = self.lifecycle_gate(owned_agent_id);
        let _lifecycle_guard = lifecycle_gate.gate.read().await;
        record_worker_call("get");

        let Some(identity) = self.resolve_agent_identity(owned_agent_id).await? else {
            return Ok(None);
        };
        let agent_mode = identity.agent_mode;
        let resolved_fingerprint = identity.fingerprint;
        let initial_oplog_entry = Some((OplogIndex::INITIAL, identity.create_entry));

        debug!("Found initial oplog entry for worker: {initial_oplog_entry:?}");

        match initial_oplog_entry {
            None => Ok(None),
            Some((
                _,
                OplogEntry::Create {
                    timestamp,
                    parameters,
                },
            )) => {
                let golem_common::model::oplog::CreateParameters {
                    agent_id,
                    owner_kind,
                    agent_mode: persisted_agent_mode,
                    component_revision,
                    env,
                    environment_id,
                    created_by,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins,
                    local_agent_config,
                    original_phantom_id,
                    instance_id,
                } = *parameters;
                owner_kind
                    .validate_instance_name(&agent_id.agent_id)
                    .unwrap_or_else(|error| {
                        panic!("invalid authoritative owner metadata for {owned_agent_id}: {error}")
                    });
                debug_assert_eq!(persisted_agent_mode, agent_mode);
                debug_assert_eq!(AgentFingerprint(instance_id), resolved_fingerprint);
                let agent_mode = persisted_agent_mode;
                let component_metadata = self
                    .component_service
                    .get_metadata(agent_id.component_id, Some(component_revision))
                    .await
                    .map_or_else(
                        |e| match e {
                            WorkerExecutorError::ComponentNotFound { .. } => Ok(None),
                            other => Err(other),
                        },
                        |v| Ok(Some(v)),
                    )
                    .unwrap_or_else(|err| {
                        panic!("failed to get component metadata for {owned_agent_id}: {err}")
                    });
                let Some(component_metadata) = component_metadata else {
                    return Ok(None);
                };
                let agent_type_name = (matches!(
                    owner_kind,
                    golem_common::model::agent::OwnerKind::ComponentAgent
                ) && component_metadata.metadata.is_agent())
                .then(|| ParsedAgentId::parse_agent_type_name(&agent_id.agent_id))
                .transpose()
                .unwrap_or_else(|error| {
                    panic!("invalid agent type in authoritative owner metadata for {owned_agent_id}: {error}")
                });

                let config = local_agent_config
                    .into_iter()
                    .map(|lac| {
                        lac.enrich_with_type(
                            &component_metadata.metadata,
                            owner_kind,
                            agent_type_name.as_ref(),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap_or_else(|err| {
                        panic!("failed enriching local agent config for {owned_agent_id}: {err}")
                    });

                let initial_worker_metadata = AgentMetadata {
                    agent_id,
                    owner_kind,
                    env,
                    config,
                    environment_id,
                    created_by,
                    created_by_email: component_metadata.account_email,
                    created_at: timestamp,
                    parent,
                    last_known_status: AgentStatusRecord {
                        component_revision,
                        component_revision_for_replay: component_revision,
                        component_size,
                        total_linear_memory_size: initial_total_linear_memory_size,
                        active_plugins: initial_active_plugins,
                        invocation_results: self.config.invocation_results.membership(),
                        export_fork_admissions: golem_common::model::ExportForkAdmissions {
                            owner_fingerprint: Some(AgentFingerprint(instance_id)),
                            ..Default::default()
                        },
                        agent_mode,
                        ..AgentStatusRecord::default()
                    },
                    original_phantom_id,
                    fingerprint: AgentFingerprint(instance_id),
                    agent_mode,
                };

                let fingerprint = AgentFingerprint(instance_id);
                let last_known_status = match self
                    .read_cached_status(owned_agent_id, fingerprint)
                    .await?
                {
                    Some(mut status) => {
                        // `agent_mode` is `#[transient]` and therefore not part of the status
                        // blob; restore it from the authoritative value resolved above so the
                        // returned record carries the correct mode instead of the `Durable`
                        // deserialization default.
                        status.agent_mode = agent_mode;
                        Some(status)
                    }
                    // No cached status (cache miss, missing for ephemeral workers, or stale
                    // format) -> recompute from oplog, preferring to fold forward from the clean
                    // checkpoint (if any) over a full re-read.
                    None => {
                        let last_known_status = calculate_last_known_status_with_checkpoint_reader(
                            self,
                            owned_agent_id,
                            agent_mode,
                            None,
                            || async {
                                // The checkpoint is only a fold baseline: a read failure here
                                // costs a full recompute, not correctness, and the function has
                                // no way to report it. The live cached status above is the read
                                // that propagates.
                                self.read_status_checkpoint(owned_agent_id, fingerprint, agent_mode)
                                    .await
                                    .unwrap_or_else(|err| {
                                        error!(
                                            "Failed to read the status checkpoint for {owned_agent_id}: {err}"
                                        );
                                        None
                                    })
                            },
                        )
                        .await;

                        let last_known_status = match last_known_status {
                            Ok(Some(status)) => status,
                            Ok(None) => return Ok(None),
                            Err(error) => {
                                tracing::error!(
                                    agent_id = %owned_agent_id,
                                    %error,
                                    "Failed to recompute cold worker status"
                                );
                                // The Create entry still proves the worker exists. Leave status
                                // unresolved so typed reconstruction callers report the failure,
                                // rather than treating a corrupt status payload as a missing worker.
                                return Ok(Some(GetWorkerMetadataResult {
                                    initial_worker_metadata,
                                    last_known_status: None,
                                }));
                            }
                        };

                        // Cold path: no in-memory previous, reconcile against stored fields.
                        self.write_cached_status(
                            owned_agent_id,
                            fingerprint,
                            None,
                            last_known_status.clone(),
                        )
                        .await
                        .map_err(WorkerExecutorError::runtime)?;

                        Some(last_known_status)
                    }
                };

                Ok(Some(GetWorkerMetadataResult {
                    initial_worker_metadata,
                    last_known_status,
                }))
            }
            Some(_) => panic!("Encountered malformed oplog without create oplog entry"),
        }
    }

    async fn get_running_workers_in_shards(
        &self,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError> {
        let shard_assignment = self.shard_service.try_get_current_assignment();
        let mut result: Vec<GetWorkerMetadataResult> = vec![];
        if let Some(shard_assignment) = shard_assignment {
            for shard_id in shard_assignment.shard_ids() {
                let key = Self::running_in_shard_key(&shard_id);
                let mut shard_worker = self.enum_workers_at_key(&key).await?;
                result.append(&mut shard_worker);
            }
        }
        Ok(result)
    }

    async fn remove(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), WorkerExecutorError> {
        lifecycle.assert_agent(&owned_agent_id.agent_id);
        let lifecycle_gate = self.lifecycle_gate(owned_agent_id);
        let _lifecycle_guard = lifecycle_gate.gate.write().await;
        record_worker_call("remove");
        let current_identity = self
            .read_agent_identity_for_deletion(owned_agent_id, agent_mode)
            .await?;
        let delete_current_oplog = match current_identity.as_ref() {
            Some(identity)
                if identity.fingerprint == fingerprint && identity.agent_mode != agent_mode =>
            {
                return Err(WorkerExecutorError::runtime(format!(
                    "agent {owned_agent_id} fingerprint matches the deleting incarnation but its mode does not"
                )));
            }
            Some(identity) => {
                identity.fingerprint == fingerprint && identity.agent_mode == agent_mode
            }
            None => false,
        };

        // The oplog first, so that a refusal leaves every other piece of the agent's state in place
        // too. Only this incarnation's: a recreated agent's oplog is not the deleting one's to remove.
        if delete_current_oplog {
            self.oplog_service
                .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
                .await
                .map_err(|error| match error {
                    OplogError::Fenced(fence) => WorkerExecutorError::oplog_fenced(
                        fence.agent_id,
                        fence.expected_epoch.0,
                        fence.actual_epoch.map(|epoch| epoch.0),
                    ),
                    other => WorkerExecutorError::runtime(other.to_string()),
                })?;
        }

        self.remove_cached_status(owned_agent_id, fingerprint)
            .await?;
        self.remove_all_fields(
            owned_agent_id,
            Self::invocation_result_index_namespace(&owned_agent_id.agent_id, fingerprint),
            "invocation-result index",
        )
        .await?;
        self.stream_session_index
            .clear(owned_agent_id, fingerprint)
            .await
            .map_err(WorkerExecutorError::runtime)?;
        self.key_value_storage
            .with("worker", "remove_rejected_periodic_snapshot")
            .del(
                Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
                &Self::rejected_periodic_snapshots_field(fingerprint),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assigment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);

        self.remove_running_worker_member(
            &Self::running_in_shard_key(&shard_id),
            &RunningWorker {
                owned_agent_id: owned_agent_id.clone(),
                fingerprint,
            },
        )
        .await
    }

    async fn remove_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError> {
        record_worker_call("remove_cached_status");
        self.remove_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id, fingerprint),
        )
        .await?;
        self.remove_split_status(
            owned_agent_id,
            Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
        )
        .await?;
        self.remove_cached_agent_mode_if_fingerprint(owned_agent_id, fingerprint)
            .await
    }

    async fn get_rejected_periodic_snapshot_through(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        let value: Option<Result<OplogIndex, String>> = self
            .key_value_storage
            .with_entity(
                "worker",
                "get_rejected_periodic_snapshot_through",
                "oplog_index",
            )
            .get_attempt_deserialize(
                Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
                &Self::rejected_periodic_snapshots_field(fingerprint),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;
        value.transpose().map_err(WorkerExecutorError::runtime)
    }

    async fn reject_periodic_snapshots_through(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        oplog_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        let namespace = Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id);
        let field = Self::rejected_periodic_snapshots_field(fingerprint);
        loop {
            let current = self
                .key_value_storage
                .with_entity("worker", "read_rejected_periodic_snapshot", "oplog_index")
                .get_raw(namespace.clone(), &field)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            if let Some(current) = &current {
                let current_index: OplogIndex =
                    deserialize(current).map_err(WorkerExecutorError::runtime)?;
                if current_index >= oplog_index {
                    return Ok(());
                }
            }
            let encoded = serialize(&oplog_index).map_err(WorkerExecutorError::runtime)?;
            let updated = self
                .key_value_storage
                .with_entity("worker", "reject_periodic_snapshots_through", "oplog_index")
                .compare_and_set_many_raw(
                    namespace.clone(),
                    &field,
                    current.as_deref(),
                    &[],
                    &[(field.as_str(), encoded.as_slice())],
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;
            if updated {
                return Ok(());
            }
        }
    }

    async fn lookup_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        key: &IdempotencyKey,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        if let Some(value) = status.durable_stream_sessions.get(key) {
            return Ok(Some(value.clone()));
        }
        if !status.durable_stream_sessions.has_history() {
            return Ok(None);
        }
        self.stream_session_index
            .lookup_persisted_offsets(
                owned_agent_id,
                agent_mode,
                fingerprint,
                status.oplog_idx,
                key,
            )
            .await
    }

    async fn lookup_durable_stream_public_binding(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        public_session_id: &str,
    ) -> Result<Option<DurableStreamPublicBinding>, String> {
        self.stream_session_index
            .lookup_public_binding(owned_agent_id, agent_mode, fingerprint, public_session_id)
            .await
    }

    async fn catch_up_invocation_result_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
    ) -> Result<(), String> {
        if agent_mode == AgentMode::Ephemeral {
            return Ok(());
        }

        let lock = self.invocation_result_index_lock(owned_agent_id);
        let _guard = lock.inner.lock().await;
        async {
            let namespace =
                Self::invocation_result_index_namespace(&owned_agent_id.agent_id, fingerprint);
            let status_generation = status.invocation_results.revert_generation();
            'reload: loop {
                let observed_metadata = self
                    .key_value_storage
                    .with_entity("worker", "read_invocation_result_index", "metadata")
                    .get_raw(
                        namespace.clone(),
                        INVOCATION_RESULT_INDEX_METADATA_FIELD,
                    )
                    .await?
                    .map(|value| value.to_vec());
                let persisted = observed_metadata
                    .as_deref()
                    .map(deserialize::<InvocationResultIndexMetadata>)
                    .transpose();
                let (mut metadata, mut expected_metadata) = match &persisted {
                    Ok(Some(metadata)) if metadata.revert_generation > status_generation => {
                        return Ok(());
                    }
                    Ok(Some(metadata)) if metadata.revert_generation == status_generation => {
                        if metadata.covered_through >= status.oplog_idx {
                            return Ok(());
                        }
                        (metadata.clone(), observed_metadata)
                    }
                    Ok(Some(_)) | Ok(None) | Err(_) => {
                        // Only a reset needs all mapping field names. Re-evaluate the generation
                        // from this atomic snapshot because another publisher may have advanced it
                        // after the metadata-only read above.
                        let snapshot = self
                            .key_value_storage
                            .with_entity("worker", "snapshot_invocation_result_index", "metadata")
                            .get_all_raw(namespace.clone())
                            .await?;
                        let snapshot_metadata = snapshot
                            .iter()
                            .find(|(field, _)| field == INVOCATION_RESULT_INDEX_METADATA_FIELD)
                            .map(|(_, value)| value.to_vec());
                        let snapshot_decoded = snapshot_metadata
                            .as_deref()
                            .map(deserialize::<InvocationResultIndexMetadata>)
                            .transpose();
                        match snapshot_decoded {
                            Ok(Some(metadata))
                                if metadata.revert_generation > status_generation =>
                            {
                                return Ok(());
                            }
                            Ok(Some(metadata))
                                if metadata.revert_generation == status_generation =>
                            {
                                if metadata.covered_through >= status.oplog_idx {
                                    return Ok(());
                                }
                                (metadata, snapshot_metadata)
                            }
                            Ok(Some(_)) | Ok(None) | Err(_) => {
                                let metadata = InvocationResultIndexMetadata {
                                    covered_through: OplogIndex::NONE,
                                    revert_generation: status_generation,
                                    current_idempotency_key: None,
                                    cancelled_idempotency_key: None,
                                };
                                let encoded = serialize(&metadata)?;
                                let deletions: Vec<&str> = snapshot
                                    .iter()
                                    .map(|(field, _)| field.as_str())
                                    .filter(|field| {
                                        *field != INVOCATION_RESULT_INDEX_METADATA_FIELD
                                    })
                                    .collect();
                                let installed = self
                                    .key_value_storage
                                    .with_entity(
                                        "worker",
                                        "reset_invocation_result_index",
                                        "metadata",
                                    )
                                    .compare_and_mutate_many_raw(
                                        namespace.clone(),
                                        INVOCATION_RESULT_INDEX_METADATA_FIELD,
                                        snapshot_metadata.as_deref(),
                                        &[(
                                            INVOCATION_RESULT_INDEX_METADATA_FIELD,
                                            encoded.as_slice(),
                                        )],
                                        &deletions,
                                        DERIVED_CACHE_EXPIRY,
                                    )
                                    .await
                                    .inspect_err(|_err| {
                                        record_derived_cache_publication_failed(
                                            "invocation_result_index",
                                        );
                                    })?;
                                if !installed {
                                    continue 'reload;
                                }
                                (metadata, Some(encoded))
                            }
                        }
                    }
                };

                while metadata.covered_through < status.oplog_idx {
                    let remaining =
                        status.oplog_idx.as_u64() - metadata.covered_through.as_u64();
                    let count = remaining
                        .min(
                            self.config
                                .invocation_results
                                .physical_index_catch_up_chunk_size,
                        )
                        .max(1);
                    let entries = self
                        .oplog_service
                        .read_exact(
                            owned_agent_id,
                            agent_mode,
                            metadata.covered_through.next(),
                            count,
                        )
                        .await;
                    if entries.is_empty() {
                        return Err(format!(
                            "failed to advance invocation result index for {owned_agent_id}: oplog range starting at {} was empty",
                            metadata.covered_through.next()
                        ));
                    }

                    let mut mappings = HashMap::new();
                    let mut advanced = metadata.clone();
                    fold_invocation_result_entries(
                        &mut advanced.current_idempotency_key,
                        &mut advanced.cancelled_idempotency_key,
                        &status.deleted_regions,
                        &entries,
                        |key, index| {
                            mappings.insert(key.clone(), index);
                        },
                    );
                    advanced.covered_through = *entries.keys().max().unwrap();

                    let mut fields = Vec::with_capacity(mappings.len() + 1);
                    for (key, oplog_index) in mappings {
                        fields.push((
                            invocation_result_index_field(&key),
                            serialize(&PersistedInvocationResult {
                                revert_generation: advanced.revert_generation,
                                oplog_index,
                            })?,
                        ));
                    }
                    fields.push((
                        INVOCATION_RESULT_INDEX_METADATA_FIELD.to_string(),
                        serialize(&advanced)?,
                    ));
                    let pairs: Vec<(&str, &[u8])> = fields
                        .iter()
                        .map(|(field, value)| (field.as_str(), value.as_slice()))
                        .collect();
                    let updated = self
                        .key_value_storage
                        .with_entity(
                            "worker",
                            "advance_invocation_result_index",
                            "invocation_result",
                        )
                        .compare_and_mutate_many_raw(
                            namespace.clone(),
                            INVOCATION_RESULT_INDEX_METADATA_FIELD,
                            expected_metadata.as_deref(),
                            &pairs,
                            &[],
                            DERIVED_CACHE_EXPIRY,
                        )
                        .await
                        .inspect_err(|_err| {
                            record_derived_cache_publication_failed("invocation_result_index");
                        })?;
                    if !updated {
                        continue 'reload;
                    }
                    expected_metadata = fields.last().map(|(_, value)| value.clone());
                    metadata = advanced;
                    crate::metrics::workers::record_invocation_result_index_catch_up(entries.len());
                }

                return Ok(());
            }
        }
        .await
    }

    async fn lookup_invocation_result_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationResultIndexLookup, String> {
        let values = self
            .key_value_storage
            .with_entity(
                "worker",
                "lookup_invocation_result_index",
                "invocation_result",
            )
            .get_many_raw(
                Self::invocation_result_index_namespace(&owned_agent_id.agent_id, fingerprint),
                vec![
                    INVOCATION_RESULT_INDEX_METADATA_FIELD.to_string(),
                    invocation_result_index_field(idempotency_key),
                ]
                .into(),
            )
            .await?;
        let Some(metadata) = values.first().and_then(Option::as_ref) else {
            return Ok(InvocationResultIndexLookup::Incomplete);
        };
        let metadata: InvocationResultIndexMetadata = deserialize(metadata)?;
        if metadata.revert_generation != status.invocation_results.revert_generation() {
            return Ok(InvocationResultIndexLookup::Incomplete);
        }

        let complete = metadata.covered_through >= status.oplog_idx
            || status
                .invocation_results
                .oldest_retained_index()
                .is_some_and(|oldest| oldest <= metadata.covered_through);
        if !complete {
            return Ok(InvocationResultIndexLookup::Incomplete);
        }

        let Some(value) = values.get(1).and_then(Option::as_ref) else {
            return Ok(InvocationResultIndexLookup::DefinitiveMiss);
        };
        let value: PersistedInvocationResult = deserialize(value)?;
        if value.revert_generation != metadata.revert_generation
            || status
                .deleted_regions
                .is_in_deleted_region(value.oplog_index)
        {
            return Ok(InvocationResultIndexLookup::DefinitiveMiss);
        }

        Ok(InvocationResultIndexLookup::Found(value.oplog_index))
    }

    async fn resolve_agent_identity(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        record_worker_call("resolve_agent_identity");
        let hint = self.read_cached_agent_mode(owned_agent_id).await?;
        record_agent_identity_resolution(if hint.is_some() { "hint" } else { "no_hint" });
        let first_mode = hint
            .as_ref()
            .map_or(AgentMode::Durable, |hint| hint.agent_mode);
        let second_mode = match first_mode {
            AgentMode::Durable => AgentMode::Ephemeral,
            AgentMode::Ephemeral => AgentMode::Durable,
        };
        let resolved = match self
            .read_agent_identity_in_mode(owned_agent_id, first_mode)
            .await?
        {
            Some(identity) => Some(identity),
            None => {
                record_agent_identity_resolution("alternate_mode_fallback");
                self.read_agent_identity_in_mode(owned_agent_id, second_mode)
                    .await?
            }
        };

        match &resolved {
            Some(identity) if identity.agent_mode == AgentMode::Durable => {
                let actual = CachedAgentMode {
                    agent_mode: identity.agent_mode,
                    fingerprint: identity.fingerprint,
                };
                if hint.as_ref() != Some(&actual) {
                    match self.write_cached_agent_mode(owned_agent_id, &actual).await {
                        Ok(()) if hint.is_some() => {
                            record_agent_identity_resolution("stale_hint_replacement");
                        }
                        Ok(()) => {}
                        Err(err) => {
                            error!("Failed to cache the agent mode for {owned_agent_id}: {err}");
                        }
                    }
                }
            }
            Some(_) if hint.is_some() => {
                match self.remove_cached_agent_mode(owned_agent_id).await {
                    Ok(()) => record_agent_identity_resolution("stale_hint_replacement"),
                    Err(err) => {
                        error!("Failed to remove stale agent mode for {owned_agent_id}: {err}");
                    }
                }
            }
            _ => {}
        }
        Ok(resolved)
    }

    async fn write_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, String> {
        record_worker_call("write_status");

        debug!("Writing cached agent status for {owned_agent_id} to {status_value:?}");

        if status_value.has_durable_stream_history {
            self.stream_session_index
                .catch_up(
                    owned_agent_id,
                    status_value.agent_mode,
                    fingerprint,
                    status_value.oplog_idx,
                )
                .await?;
        }

        self.catch_up_invocation_result_index(
            owned_agent_id,
            status_value.agent_mode,
            fingerprint,
            &status_value,
        )
        .await?;
        self.write_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id, fingerprint),
            previous_status,
            status_value,
        )
        .await
    }

    async fn read_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        agent_mode: AgentMode,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError> {
        record_worker_call("read_status_checkpoint");

        let status = self
            .read_split_status(
                owned_agent_id,
                Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
            )
            .await?;
        // `agent_mode` is transient (not part of `core`); restore the authoritative value.
        Ok(status.map(|mut status| {
            status.agent_mode = agent_mode;
            status
        }))
    }

    async fn write_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_checkpoint: Option<&AgentStatusRecord>,
        checkpoint: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, String> {
        record_worker_call("write_status_checkpoint");

        debug!(
            "Writing clean status checkpoint for {owned_agent_id} at oplog index {}",
            checkpoint.oplog_idx
        );

        if checkpoint.has_durable_stream_history {
            self.stream_session_index
                .catch_up(
                    owned_agent_id,
                    checkpoint.agent_mode,
                    fingerprint,
                    checkpoint.oplog_idx,
                )
                .await?;
        }
        self.write_split_status(
            owned_agent_id,
            Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
            previous_checkpoint,
            checkpoint,
        )
        .await
    }

    async fn set_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status_value: &AgentStatusRecord,
    ) -> Result<(), String> {
        record_worker_call("set_assignment_tracking");

        // Ephemeral workers are never tracked for recovery (mirrors `write_cached_status`).
        if status_value.agent_mode == AgentMode::Ephemeral {
            return Ok(());
        }

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assignment is not ready");

        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);
        let running_worker = RunningWorker {
            owned_agent_id: owned_agent_id.clone(),
            fingerprint,
        };

        if Self::should_track_for_assignment_recovery(status_value) {
            debug!("Adding worker to the set of running workers in shard {shard_id}");

            self.key_value_storage
                .with_entity("worker", "add", "agent_id")
                .add_to_set(
                    KeyValueStorageNamespace::RunningWorkers,
                    &Self::running_in_shard_key(&shard_id),
                    &running_worker,
                )
                .await
                .map_err(|err| {
                    format!(
                        "failed to add worker to the set of running workers per shard ids on KV storage: {err}"
                    )
                })
        } else {
            debug!("Removing worker from the set of running workers in shard {shard_id}");

            self.key_value_storage
                .with_entity("worker", "remove", "agent_id")
                .remove_from_set(
                    KeyValueStorageNamespace::RunningWorkers,
                    &Self::running_in_shard_key(&shard_id),
                    &running_worker,
                )
                .await
                .map_err(|err| {
                    format!(
                        "failed to remove worker from the set of running worker ids per shard on KV storage: {err}"
                    )
                })
        }
    }

    async fn remove_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), String> {
        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assignment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);
        self.remove_running_worker_member(
            &Self::running_in_shard_key(&shard_id),
            &RunningWorker {
                owned_agent_id: owned_agent_id.clone(),
                fingerprint,
            },
        )
        .await
        .map_err(|error| error.to_string())
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
mod tests {
    use super::*;
    use crate::model::ExecutionStatus;
    use crate::services::oplog::Oplog;
    use crate::services::shard::ShardServiceDefault;
    use crate::storage::keyvalue::KeyValueStorageError;
    use crate::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use async_trait::async_trait;
    use bytes::Bytes;
    use golem_common::model::Timestamp;
    use golem_common::model::account::AccountId;
    use golem_common::model::application::ApplicationId;
    use golem_common::model::card::{
        Card, CardId, InvocationWalletPin, StoredCard, WalletVersionToken,
    };
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::{OplogPayload, PayloadId, RawOplogPayload};
    use golem_common::model::regions::{DeletedRegions, OplogRegion};
    use golem_common::model::{
        AgentInvocationPayload, AgentInvocationResult, AgentMetadata, PendingInvocationRef,
        PendingUpdateKind, PendingUpdateRef, ScanCursor, ShardEpoch, ShardLeaseRevision,
    };
    use golem_common::read_only_lock;
    use golem_service_base::model::component::Component;
    use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;
    use tokio::sync::Notify;
    use uuid::Uuid;

    #[derive(Debug)]
    struct IndexTestOplogService {
        entries: BTreeMap<OplogIndex, OplogEntry>,
        stream_index: std::sync::OnceLock<Arc<StreamSessionIndexService>>,
        reads: StdMutex<Vec<(OplogIndex, u64)>>,
        pause_next_read: AtomicBool,
        read_started: Notify,
        resume_read: Notify,
    }

    impl IndexTestOplogService {
        fn new(entries: BTreeMap<OplogIndex, OplogEntry>) -> Self {
            Self {
                entries,
                stream_index: std::sync::OnceLock::new(),
                reads: StdMutex::new(Vec::new()),
                pause_next_read: AtomicBool::new(false),
                read_started: Notify::new(),
                resume_read: Notify::new(),
            }
        }

        fn pause_next_read(&self) {
            self.pause_next_read.store(true, Ordering::Release);
        }

        fn read_starts(&self) -> Vec<OplogIndex> {
            self.reads
                .lock()
                .unwrap()
                .iter()
                .map(|(start, _)| *start)
                .collect()
        }

        fn clear_reads(&self) {
            self.reads.lock().unwrap().clear();
        }
    }

    #[async_trait]
    impl OplogService for IndexTestOplogService {
        async fn staged_exists(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _stage_id: uuid::Uuid,
        ) -> Result<bool, String> {
            unimplemented!()
        }

        async fn lock_lifecycle(&self, _: &AgentId) -> OplogLifecycleGuard {
            unreachable!()
        }

        fn set_stream_session_index(&self, index: Arc<StreamSessionIndexService>) {
            self.stream_index.set(index).unwrap();
        }

        fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
            self.stream_index.get().cloned()
        }

        async fn create(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _initial_entry: OplogEntry,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
            _shard_epoch: Option<ShardEpoch>,
        ) -> Arc<dyn crate::services::oplog::Oplog> {
            unreachable!()
        }

        async fn create_fresh(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _initial_entry: OplogEntry,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
            _shard_epoch: Option<ShardEpoch>,
        ) -> Arc<dyn crate::services::oplog::Oplog> {
            unreachable!()
        }

        async fn open(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _last_oplog_index: Option<OplogIndex>,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
            _shard_epoch: Option<ShardEpoch>,
        ) -> Arc<dyn crate::services::oplog::Oplog> {
            unreachable!()
        }

        async fn get_last_index(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
        ) -> OplogIndex {
            self.entries
                .keys()
                .next_back()
                .copied()
                .unwrap_or(OplogIndex::NONE)
        }

        async fn delete(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _expected_epoch: Option<golem_common::model::ShardEpoch>,
        ) -> Result<(), crate::services::oplog::OplogError> {
            unreachable!()
        }

        async fn read_exact(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            idx: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            self.reads.lock().unwrap().push((idx, n));
            if self.pause_next_read.swap(false, Ordering::AcqRel) {
                self.read_started.notify_one();
                self.resume_read.notified().await;
            }
            let end = idx.as_u64().saturating_add(n.saturating_sub(1));
            self.entries
                .range(idx..=OplogIndex::from_u64(end))
                .map(|(index, entry)| (*index, entry.clone()))
                .collect()
        }

        async fn exists(&self, _owned_agent_id: &OwnedAgentId, _agent_mode: AgentMode) -> bool {
            true
        }

        async fn scan_for_component(
            &self,
            _environment_id: &EnvironmentId,
            _component_id: &ComponentId,
            _modes: Option<AgentMode>,
            _cursor: ScanCursor,
            _count: u64,
        ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
            unreachable!()
        }

        async fn upload_raw_payload(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _data: Vec<u8>,
        ) -> Result<RawOplogPayload, String> {
            unreachable!()
        }

        async fn download_raw_payload(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unreachable!()
        }
    }

    struct IndexTestComponentService;

    fn non_agent_component(component_id: ComponentId, environment_id: EnvironmentId) -> Component {
        Component {
            id: component_id,
            revision: ComponentRevision::INITIAL,
            environment_id,
            component_name: golem_common::model::component::ComponentName(
                "test-component".to_string(),
            ),
            hash: golem_common::model::diff::Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: AccountId::new(),
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
            application_name: golem_common::model::application::ApplicationName::try_from(
                "test-app".to_string(),
            )
            .unwrap(),
            environment_name: golem_common::model::environment::EnvironmentName::try_from(
                "test-env",
            )
            .unwrap(),
            component_size: 1,
            metadata: golem_common::model::component_metadata::ComponentMetadata::default(),
            created_at: chrono::Utc::now(),
            wasm_hash: golem_common::model::diff::Hash::empty(),
            object_store_key: "test-object".to_string(),
        }
    }

    #[async_trait]
    impl ComponentService for IndexTestComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            unreachable!()
        }

        async fn get_metadata(
            &self,
            component_id: ComponentId,
            _forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            Ok(non_agent_component(component_id, EnvironmentId::new()))
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            unreachable!()
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            Vec::new()
        }

        async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
    }

    fn invocation_pair(
        entries: &mut BTreeMap<OplogIndex, OplogEntry>,
        started_at: u64,
        key: &IdempotencyKey,
    ) {
        entries.insert(
            OplogIndex::from_u64(started_at),
            OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                idempotency_key: key.clone(),
                payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::ManualUpdate {
                    target_revision: ComponentRevision::INITIAL,
                })),
                trace_id: TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
                wallet_pin: Box::new(InvocationWalletPin {
                    wallet_token: WalletVersionToken {
                        wallet_id_hash: [0; 32],
                        generation: 0,
                    },
                    pinned_card_ids: Vec::new(),
                    scope_card_id: None,
                }),
            },
        );
        entries.insert(
            OplogIndex::from_u64(started_at + 1),
            OplogEntry::AgentInvocationFinished {
                timestamp: Timestamp::now_utc(),
                result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentInitialization)),
                method_name: None,
                consumed_fuel: 0,
                component_revision: ComponentRevision::INITIAL,
            },
        );
    }

    fn invocation_entries(keys: &[IdempotencyKey]) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut entries = BTreeMap::from([(OplogIndex::INITIAL, OplogEntry::no_op(None))]);
        for (offset, key) in keys.iter().enumerate() {
            invocation_pair(&mut entries, 2 + offset as u64 * 2, key);
        }
        entries
    }

    fn invocation_status(
        oplog_idx: u64,
        capacity: usize,
        results: &[(&IdempotencyKey, u64)],
        revert_generation: u64,
        deleted_regions: DeletedRegions,
    ) -> AgentStatusRecord {
        let mut invocation_results = InvocationResultMembership::new(capacity, 128, 3);
        for (key, index) in results {
            invocation_results.insert((*key).clone(), OplogIndex::from_u64(*index));
        }
        invocation_results.set_revert_generation(revert_generation);
        AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(oplog_idx),
            invocation_results,
            deleted_regions,
            ..AgentStatusRecord::default()
        }
    }

    fn index_test_service(
        entries: BTreeMap<OplogIndex, OplogEntry>,
    ) -> (
        Arc<DefaultWorkerService>,
        Arc<IndexTestOplogService>,
        OwnedAgentId,
    ) {
        let oplog = Arc::new(IndexTestOplogService::new(entries));
        let service =
            index_test_service_with(Arc::new(InMemoryKeyValueStorage::new()), oplog.clone());
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "invocation-index-test".to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
        (service, oplog, owned_agent_id)
    }

    fn index_test_service_with(
        storage: Arc<dyn KeyValueStorage + Send + Sync>,
        oplog: Arc<IndexTestOplogService>,
    ) -> Arc<DefaultWorkerService> {
        let mut config = GolemConfig::default();
        config.invocation_results.physical_index_catch_up_chunk_size = 2;
        Arc::new(DefaultWorkerService::new(
            storage,
            Arc::new(ShardServiceDefault::new()),
            oplog,
            Arc::new(IndexTestComponentService),
            Arc::new(config),
        ))
    }

    #[test]
    async fn get_recovers_uuid_named_non_agent_component_worker() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let agent_id = AgentId {
            component_id,
            agent_id: Uuid::new_v4().to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let create = OplogEntry::Create {
            timestamp: Timestamp::now_utc(),
            parameters: Box::new(golem_common::model::oplog::CreateParameters {
                agent_id: agent_id.clone(),
                owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
                agent_mode: AgentMode::Durable,
                component_revision: ComponentRevision::INITIAL,
                env: Vec::new(),
                environment_id,
                created_by: AccountId::new(),
                parent: None,
                component_size: 1,
                initial_total_linear_memory_size: 0,
                initial_active_plugins: HashSet::new(),
                local_agent_config: Vec::new(),
                original_phantom_id: None,
                instance_id: Uuid::new_v4(),
            }),
        };
        let shard_service = Arc::new(ShardServiceDefault::new());
        shard_service.register(4, &HashMap::new(), None, ShardLeaseRevision::default());
        let service = DefaultWorkerService::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            shard_service,
            Arc::new(IndexTestOplogService::new(BTreeMap::from([(
                OplogIndex::INITIAL,
                create,
            )]))),
            Arc::new(IndexTestComponentService),
            Arc::new(GolemConfig::default()),
        );

        let result = service.get(&owned_agent_id).await.unwrap().unwrap();

        assert_eq!(result.initial_worker_metadata.agent_id, agent_id);
        assert!(result.last_known_status.is_some());
    }

    #[test]
    async fn rejected_periodic_snapshot_watermark_is_monotonic_and_incarnation_scoped() {
        let (service, _, owned_agent_id) = index_test_service(BTreeMap::new());
        let first = AgentFingerprint::new();
        let second = AgentFingerprint::new();

        service
            .reject_periodic_snapshots_through(&owned_agent_id, first, OplogIndex::from_u64(12))
            .await
            .unwrap();
        service
            .reject_periodic_snapshots_through(&owned_agent_id, first, OplogIndex::from_u64(7))
            .await
            .unwrap();

        assert_eq!(
            service
                .get_rejected_periodic_snapshot_through(&owned_agent_id, first)
                .await
                .unwrap(),
            Some(OplogIndex::from_u64(12))
        );
        assert_eq!(
            service
                .get_rejected_periodic_snapshot_through(&owned_agent_id, second)
                .await
                .unwrap(),
            None
        );

        service
            .remove_cached_status(&owned_agent_id, invocation_index_fingerprint())
            .await
            .unwrap();
        assert_eq!(
            service
                .get_rejected_periodic_snapshot_through(&owned_agent_id, first)
                .await
                .unwrap(),
            Some(OplogIndex::from_u64(12))
        );

        service
            .remove_split_status(
                &owned_agent_id,
                DefaultWorkerService::rejected_periodic_snapshots_namespace(
                    &owned_agent_id.agent_id,
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .get_rejected_periodic_snapshot_through(&owned_agent_id, first)
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    async fn remembered_status_baseline_recovers_complete_hash_after_cache_loss() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let service = DefaultWorkerService::new(
            storage.clone(),
            Arc::new(ShardServiceDefault::new()),
            Arc::new(IndexTestOplogService::new(BTreeMap::new())),
            Arc::new(IndexTestComponentService),
            Arc::new(GolemConfig::default()),
        );
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "status-cache-loss".to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
        let fingerprint = AgentFingerprint::new();
        let namespace = DefaultWorkerService::status_namespace(&agent_id, fingerprint);
        let status = AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(10),
            skipped_regions: DeletedRegions::from_regions([OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(5),
            }]),
            ..AgentStatusRecord::default()
        };

        service
            .write_split_status(&owned_agent_id, namespace.clone(), None, status.clone())
            .await
            .unwrap();
        let fields = storage
            .keys("test", "test", namespace.clone())
            .await
            .unwrap();
        storage
            .del_many("test", "test", namespace.clone(), fields.into())
            .await
            .unwrap();

        service
            .write_split_status(
                &owned_agent_id,
                namespace.clone(),
                Some(&status),
                status.clone(),
            )
            .await
            .unwrap();

        assert_eq!(
            service
                .read_split_status(&owned_agent_id, namespace)
                .await
                .unwrap(),
            Some(status)
        );
    }

    #[test]
    async fn stale_status_baseline_reconciles_without_mixed_auxiliary_fields() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let service = DefaultWorkerService::new(
            storage,
            Arc::new(ShardServiceDefault::new()),
            Arc::new(IndexTestOplogService::new(BTreeMap::new())),
            Arc::new(IndexTestComponentService),
            Arc::new(GolemConfig::default()),
        );
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "status-stale-baseline".to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
        let namespace = DefaultWorkerService::status_namespace(&agent_id, AgentFingerprint::new());
        let persisted = AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(20),
            skipped_regions: DeletedRegions::from_regions([OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(5),
            }]),
            ..AgentStatusRecord::default()
        };
        let remembered = AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(10),
            ..AgentStatusRecord::default()
        };
        let replacement = AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(30),
            ..AgentStatusRecord::default()
        };

        service
            .write_split_status(&owned_agent_id, namespace.clone(), None, persisted)
            .await
            .unwrap();
        service
            .write_split_status(
                &owned_agent_id,
                namespace.clone(),
                Some(&remembered),
                replacement.clone(),
            )
            .await
            .unwrap();

        assert_eq!(
            service
                .read_split_status(&owned_agent_id, namespace)
                .await
                .unwrap(),
            Some(replacement)
        );
    }

    #[test]
    async fn cold_status_publication_removes_orphan_transfers_without_a_core() {
        let (service, storage, owned_agent_id, _) = assignment_tracking_test_service();
        let fingerprint = invocation_index_fingerprint();
        let mut source = sample_status();
        let (transfer_id, transfer) = source.received_card_transfers.iter().next().unwrap();
        let orphan_field = status_received_card_transfer_field(transfer_id);
        let orphan_value = serialize(transfer).unwrap();
        source.received_card_transfers = ReceivedCardTransferIndex::default();

        for namespace in [
            DefaultWorkerService::status_namespace(&owned_agent_id.agent_id, fingerprint),
            DefaultWorkerService::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
        ] {
            storage
                .with_entity("test", "seed_orphan_transfer", "transfer")
                .set_raw(namespace.clone(), &orphan_field, &orphan_value)
                .await
                .unwrap();

            service
                .write_split_status(&owned_agent_id, namespace.clone(), None, source.clone())
                .await
                .unwrap();

            assert_eq!(
                service
                    .read_split_status(&owned_agent_id, namespace)
                    .await
                    .unwrap(),
                Some(source.clone())
            );
        }
    }

    fn assignment_tracking_test_service() -> (
        DefaultWorkerService,
        Arc<InMemoryKeyValueStorage>,
        OwnedAgentId,
        usize,
    ) {
        let key_value_storage = Arc::new(InMemoryKeyValueStorage::new());
        let shard_service = Arc::new(ShardServiceDefault::new());
        let number_of_shards = 4;
        shard_service.register(
            number_of_shards,
            &HashMap::new(),
            None,
            ShardLeaseRevision::default(),
        );
        let service = DefaultWorkerService::new(
            key_value_storage.clone(),
            shard_service,
            Arc::new(IndexTestOplogService::new(BTreeMap::new())),
            Arc::new(IndexTestComponentService),
            Arc::new(GolemConfig::default()),
        );
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "assignment-tracking-test".to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
        (service, key_value_storage, owned_agent_id, number_of_shards)
    }

    async fn assignment_tracking_members(
        key_value_storage: &InMemoryKeyValueStorage,
        owned_agent_id: &OwnedAgentId,
        number_of_shards: usize,
    ) -> Vec<RunningWorker> {
        let shard_id = ShardId::from_agent_id(&owned_agent_id.agent_id, number_of_shards);
        key_value_storage
            .with_entity("test", "get_assignment_tracking", "agent_id")
            .members_of_set(
                KeyValueStorageNamespace::RunningWorkers,
                &DefaultWorkerService::running_in_shard_key(&shard_id),
            )
            .await
            .unwrap()
    }

    async fn invocation_index_metadata(
        service: &DefaultWorkerService,
        owned_agent_id: &OwnedAgentId,
    ) -> InvocationResultIndexMetadata {
        let value: Option<Result<InvocationResultIndexMetadata, String>> = service
            .key_value_storage
            .with_entity("test", "read_invocation_result_index", "metadata")
            .get_attempt_deserialize(
                DefaultWorkerService::invocation_result_index_namespace(
                    &owned_agent_id.agent_id,
                    invocation_index_fingerprint(),
                ),
                INVOCATION_RESULT_INDEX_METADATA_FIELD,
            )
            .await
            .unwrap();
        value.unwrap().unwrap()
    }

    fn idempotency_key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(value.to_string())
    }

    fn invocation_index_fingerprint() -> AgentFingerprint {
        AgentFingerprint(uuid::Uuid::from_u128(1))
    }

    fn transfer_id(value: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(value)
    }

    fn stored_card(card_id: CardId) -> StoredCard {
        StoredCard::Concrete(Card {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        })
    }

    fn sample_status() -> AgentStatusRecord {
        let mut status = AgentStatusRecord {
            status: AgentStatus::Running,
            oplog_idx: OplogIndex::from_u64(42),
            component_revision: ComponentRevision::new(3).unwrap(),
            ..AgentStatusRecord::default()
        };
        status
            .invocation_results
            .insert(idempotency_key("k1"), OplogIndex::from_u64(10));
        status
            .invocation_results
            .insert(idempotency_key("k2"), OplogIndex::from_u64(20));
        status.received_card_transfers.insert(
            transfer_id(1),
            ReceivedCardTransferState::Received {
                source_card_id: CardId::new(),
                card: stored_card(CardId::new()),
            },
        );
        status.durable_stream_sessions.insert(
            idempotency_key("stream-1"),
            DurableStreamSessionStatus {
                first_prepared: Some(OplogIndex::from_u64(30)),
                prepared: Some(OplogIndex::from_u64(35)),
                invocation_result: Some(OplogIndex::from_u64(40)),
                finished: None,
                ..Default::default()
            },
        );
        status.skipped_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
            OplogIndex::from_u64(2)..=OplogIndex::from_u64(5),
        )]);
        status.deleted_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
            OplogIndex::from_u64(7)..=OplogIndex::from_u64(8),
        )]);
        // Use millisecond-precise timestamps so they round-trip exactly through the codec (which
        // serializes `Timestamp` at millisecond resolution).
        status.failed_updates.push(FailedUpdateRecord {
            timestamp: Timestamp::from(1_700_000_000_000u64),
            target_revision: ComponentRevision::new(2).unwrap(),
            details: Some("boom".to_string()),
        });
        status.successful_updates.push(SuccessfulUpdateRecord {
            timestamp: Timestamp::from(1_700_000_001_000u64),
            target_revision: ComponentRevision::new(3).unwrap(),
        });
        status
    }

    /// Applies a computed `(sets, dels)` to a field map simulating the per-worker hash, then
    /// reassembles the cached status from it.
    fn apply_and_reassemble(
        store: &mut HashMap<String, Vec<u8>>,
        sets: Vec<(String, Vec<u8>)>,
        dels: Vec<String>,
    ) -> Option<AgentStatusRecord> {
        for (field, bytes) in sets {
            store.insert(field, bytes);
        }
        for field in dels {
            store.remove(&field);
        }
        reassemble_cached_status(
            store
                .iter()
                .map(|(name, bytes)| (name.clone(), Bytes::from(bytes.clone()))),
        )
    }

    #[test]
    fn split_status_round_trips_through_fields() {
        let full = sample_status();

        let mut core = full.clone();
        let parts = split_status(&mut core);
        let (sets, dels) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
        assert!(dels.is_empty());

        let mut store = HashMap::new();
        let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();

        // `agent_mode` is transient and defaults to `Durable`; `full` uses the default.
        assert_eq!(reassembled, full);
    }

    #[test]
    fn borrowed_status_core_matches_split_status_encoding() {
        let status = sample_status();
        let mut split = status.clone();
        split_status(&mut split);

        assert_eq!(serialize(&status_core(&status)), serialize(&split));
    }

    #[test]
    fn missing_core_field_is_a_cache_miss() {
        let full = sample_status();
        let mut core = full.clone();
        let parts = split_status(&mut core);
        let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();

        // Drop the core field; what remains must not reassemble.
        let without_core = sets
            .into_iter()
            .filter(|(name, _)| name != STATUS_CORE_FIELD)
            .map(|(name, bytes)| (name, Bytes::from(bytes)));
        assert!(reassemble_cached_status(without_core).is_none());
    }

    #[test]
    fn missing_membership_field_is_a_cache_miss() {
        let full = sample_status();
        let mut core = full.clone();
        let parts = split_status(&mut core);
        let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();

        let without_membership = sets
            .into_iter()
            .filter(|(name, _)| name != STATUS_MEMBERSHIP_FIELD)
            .map(|(name, bytes)| (name, Bytes::from(bytes)));
        assert!(reassemble_cached_status(without_membership).is_none());
    }

    #[test]
    fn hot_delta_only_writes_changed_fields() {
        let previous = sample_status();

        // Seed the store with the full previous state.
        let mut store = HashMap::new();
        {
            let mut core = previous.clone();
            let parts = split_status(&mut core);
            let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
            for (field, bytes) in sets {
                store.insert(field, bytes);
            }
        }

        // New status: a new bounded invocation result + advanced marker, but identical split
        // regions/updates.
        let mut new = previous.clone();
        new.oplog_idx = OplogIndex::from_u64(50);
        new.invocation_results
            .insert(idempotency_key("k3"), OplogIndex::from_u64(48));

        let mut core = new.clone();
        let parts = split_status(&mut core);
        let (sets, dels) =
            compute_status_field_writes(Some(&previous), &[], &core, &parts).unwrap();

        let written: HashSet<&str> = sets.iter().map(|(f, _)| f.as_str()).collect();
        assert!(written.contains(STATUS_CORE_FIELD));
        assert!(written.contains(STATUS_MEMBERSHIP_FIELD));
        // Unchanged parts are NOT re-sent.
        assert!(!written.contains(STATUS_REGIONS_FIELD));
        assert!(!written.contains(STATUS_UPDATES_FIELD));
        assert!(!written.contains(status_received_card_transfer_field(&transfer_id(1)).as_str()));
        assert_eq!(written.len(), 2);
        assert!(dels.is_empty());

        let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
        assert_eq!(reassembled, new);
    }

    #[test]
    fn hot_delta_only_writes_changed_transfer_fields() {
        let previous = sample_status();
        let mut new = previous.clone();
        new.oplog_idx = OplogIndex::from_u64(50);
        new.received_card_transfers.insert(
            transfer_id(2),
            ReceivedCardTransferState::Received {
                source_card_id: CardId::new(),
                card: stored_card(CardId::new()),
            },
        );

        let mut core = new;
        let parts = split_status(&mut core);
        let (sets, dels) =
            compute_status_field_writes(Some(&previous), &[], &core, &parts).unwrap();

        let written: HashSet<&str> = sets.iter().map(|(field, _)| field.as_str()).collect();
        assert_eq!(written.len(), 2);
        assert!(written.contains(STATUS_CORE_FIELD));
        assert!(written.contains(status_received_card_transfer_field(&transfer_id(2)).as_str()));
        assert!(dels.is_empty());
    }

    #[test]
    fn bounded_invocation_result_changes_are_stored_in_membership() {
        let previous = sample_status();

        let mut store = HashMap::new();
        {
            let mut core = previous.clone();
            let parts = split_status(&mut core);
            let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
            for (field, bytes) in sets {
                store.insert(field, bytes);
            }
        }

        // New status with k2 removed (as a revert would do).
        let mut new = previous.clone();
        new.invocation_results.remove(&idempotency_key("k2"));

        let mut core = new.clone();
        let parts = split_status(&mut core);
        let (sets, dels) =
            compute_status_field_writes(Some(&previous), &[], &core, &parts).unwrap();

        assert!(dels.is_empty());
        let written: HashSet<&str> = sets.iter().map(|(field, _)| field.as_str()).collect();
        assert_eq!(
            written,
            HashSet::from([STATUS_CORE_FIELD, STATUS_MEMBERSHIP_FIELD])
        );

        let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
        assert_eq!(reassembled, new);
    }

    #[test]
    fn cold_reconcile_deletes_stale_transfer_fields() {
        let stale_transfer_id = transfer_id(2);
        let existing_fields = vec![
            STATUS_CORE_FIELD.to_string(),
            status_received_card_transfer_field(&transfer_id(1)),
            status_received_card_transfer_field(&stale_transfer_id),
        ];
        let new = sample_status();

        let mut core = new;
        let parts = split_status(&mut core);
        let (_, dels) = compute_status_field_writes(None, &existing_fields, &core, &parts).unwrap();

        assert_eq!(
            dels,
            vec![status_received_card_transfer_field(&stale_transfer_id)]
        );
    }

    #[test]
    async fn invocation_result_index_resumes_across_calls_and_chunks() {
        let first = idempotency_key("first");
        let second = idempotency_key("second");
        let (service, oplog, owned_agent_id) =
            index_test_service(invocation_entries(&[first.clone(), second.clone()]));
        let partial = invocation_status(2, 2, &[], 0, DeletedRegions::new());
        let complete =
            invocation_status(5, 2, &[(&first, 3), (&second, 5)], 0, DeletedRegions::new());

        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &partial,
            )
            .await
            .unwrap();
        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &complete,
            )
            .await
            .unwrap();

        assert_eq!(
            oplog.read_starts(),
            vec![
                OplogIndex::INITIAL,
                OplogIndex::from_u64(3),
                OplogIndex::from_u64(5)
            ]
        );
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &complete,
                    &first,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(3))
        );
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &complete,
                    &second,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(5))
        );
    }

    #[test]
    async fn invocation_result_index_restarts_from_beginning_after_hash_loss() {
        let first = idempotency_key("first");
        let second = idempotency_key("second");
        let (service, oplog, owned_agent_id) =
            index_test_service(invocation_entries(&[first.clone(), second.clone()]));
        let partial = invocation_status(3, 2, &[(&first, 3)], 0, DeletedRegions::new());
        let complete =
            invocation_status(5, 2, &[(&first, 3), (&second, 5)], 0, DeletedRegions::new());

        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &partial,
            )
            .await
            .unwrap();
        oplog.pause_next_read();
        let catch_up = tokio::spawn({
            let service = service.clone();
            let owned_agent_id = owned_agent_id.clone();
            async move {
                service
                    .catch_up_invocation_result_index(
                        &owned_agent_id,
                        AgentMode::Durable,
                        invocation_index_fingerprint(),
                        &complete,
                    )
                    .await
            }
        });
        oplog.read_started.notified().await;

        let namespace = DefaultWorkerService::invocation_result_index_namespace(
            &owned_agent_id.agent_id,
            invocation_index_fingerprint(),
        );
        let fields = service
            .key_value_storage
            .with("test", "simulate_hash_loss")
            .keys(namespace.clone())
            .await
            .unwrap();
        service
            .key_value_storage
            .with("test", "simulate_hash_loss")
            .del_many(namespace, fields.into())
            .await
            .unwrap();
        oplog.resume_read.notify_one();
        catch_up.await.unwrap().unwrap();

        assert!(oplog.read_starts().contains(&OplogIndex::INITIAL));
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &invocation_status(
                        5,
                        2,
                        &[(&first, 3), (&second, 5)],
                        0,
                        DeletedRegions::new()
                    ),
                    &first,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(3))
        );
    }

    #[test]
    async fn invocation_result_index_ahead_of_status_is_not_reset() {
        let key = idempotency_key("completed");
        let (service, oplog, owned_agent_id) =
            index_test_service(invocation_entries(std::slice::from_ref(&key)));
        let complete = invocation_status(3, 2, &[(&key, 3)], 0, DeletedRegions::new());
        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &complete,
            )
            .await
            .unwrap();
        oplog.clear_reads();

        let stale = invocation_status(2, 2, &[], 0, DeletedRegions::new());
        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &stale,
            )
            .await
            .unwrap();

        assert!(oplog.read_starts().is_empty());
        assert_eq!(
            invocation_index_metadata(&service, &owned_agent_id)
                .await
                .covered_through,
            OplogIndex::from_u64(3)
        );
    }

    #[test]
    async fn invocation_result_index_and_exact_membership_are_jointly_complete() {
        let first = idempotency_key("first");
        let second = idempotency_key("second");
        let third = idempotency_key("third");
        let fourth = idempotency_key("fourth");
        let fifth = idempotency_key("fifth");
        let missing = idempotency_key("missing");
        let (service, _oplog, owned_agent_id) = index_test_service(invocation_entries(&[
            first.clone(),
            second.clone(),
            third.clone(),
            fourth.clone(),
            fifth.clone(),
        ]));
        let indexed = invocation_status(
            9,
            2,
            &[(&first, 3), (&second, 5), (&third, 7), (&fourth, 9)],
            0,
            DeletedRegions::new(),
        );
        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &indexed,
            )
            .await
            .unwrap();

        let current = invocation_status(
            11,
            2,
            &[
                (&first, 3),
                (&second, 5),
                (&third, 7),
                (&fourth, 9),
                (&fifth, 11),
            ],
            0,
            DeletedRegions::new(),
        );
        assert_eq!(
            current.invocation_results.oldest_retained_index(),
            Some(OplogIndex::from_u64(9))
        );
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &current,
                    &missing,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::DefinitiveMiss
        );
    }

    #[test]
    async fn incomplete_invocation_result_index_does_not_return_an_obsolete_result() {
        let repeated = idempotency_key("repeated");
        let (service, _oplog, owned_agent_id) =
            index_test_service(invocation_entries(&[repeated.clone(), repeated.clone()]));
        let partial = invocation_status(3, 0, &[(&repeated, 3)], 0, DeletedRegions::new());
        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &partial,
            )
            .await
            .unwrap();

        let current = invocation_status(
            5,
            0,
            &[(&repeated, 3), (&repeated, 5)],
            0,
            DeletedRegions::new(),
        );
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &current,
                    &repeated,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Incomplete
        );

        service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &current,
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &current,
                    &repeated,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(5))
        );
    }

    #[test]
    async fn independent_catch_up_cannot_publish_over_newer_generation() {
        let reverted = idempotency_key("independent-reverted");
        let current = idempotency_key("independent-current");
        let mut entries = invocation_entries(std::slice::from_ref(&reverted));
        entries.insert(
            OplogIndex::from_u64(4),
            OplogEntry::revert(OplogRegion::from_index_range(
                OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
            )),
        );
        invocation_pair(&mut entries, 5, &current);
        let oplog = Arc::new(IndexTestOplogService::new(entries));
        let faults = KeyValueStorageFaults::default();
        let storage: Arc<dyn KeyValueStorage + Send + Sync> =
            Arc::new(FaultInjectingKeyValueStorage::new(
                Arc::new(InMemoryKeyValueStorage::new()),
                faults.clone(),
            ));
        let old_service = index_test_service_with(storage.clone(), oplog.clone());
        let new_service = index_test_service_with(storage, oplog);
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "independent-invocation-index".to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
        let old_status = invocation_status(3, 2, &[(&reverted, 3)], 0, DeletedRegions::new());
        let deleted_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
            OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
        )]);
        let new_status = invocation_status(6, 2, &[(&current, 6)], 1, deleted_regions);

        let gate = faults.gate_next_pass("advance_invocation_result_index");
        let old_task = tokio::spawn({
            let service = old_service.clone();
            let owned_agent_id = owned_agent_id.clone();
            async move {
                service
                    .catch_up_invocation_result_index(
                        &owned_agent_id,
                        AgentMode::Durable,
                        invocation_index_fingerprint(),
                        &old_status,
                    )
                    .await
            }
        });
        gate.entered().await;
        new_service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &new_status,
            )
            .await
            .unwrap();
        gate.release();
        old_task.await.unwrap().unwrap();

        let metadata = invocation_index_metadata(&new_service, &owned_agent_id).await;
        assert_eq!(metadata.revert_generation, 1);
        assert_eq!(metadata.covered_through, OplogIndex::from_u64(6));
        assert_eq!(
            new_service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &new_status,
                    &current,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(6))
        );
        assert_eq!(
            new_service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &new_status,
                    &reverted,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::DefinitiveMiss
        );
    }

    #[test]
    async fn stale_reset_cannot_delete_independent_newer_progress() {
        let current = idempotency_key("reset-current");
        let oplog = Arc::new(IndexTestOplogService::new(invocation_entries(
            std::slice::from_ref(&current),
        )));
        let inner = Arc::new(InMemoryKeyValueStorage::new());
        let faults = KeyValueStorageFaults::default();
        let storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
            FaultInjectingKeyValueStorage::new(inner.clone(), faults.clone()),
        );
        let stale_service = index_test_service_with(storage.clone(), oplog.clone());
        let current_service = index_test_service_with(storage, oplog);
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "independent-reset-index".to_string(),
        };
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
        let namespace = DefaultWorkerService::invocation_result_index_namespace(
            &agent_id,
            invocation_index_fingerprint(),
        );
        let mut malformed_metadata = serialize(&InvocationResultIndexMetadata {
            covered_through: OplogIndex::from_u64(2),
            revert_generation: 0,
            current_idempotency_key: None,
            cancelled_idempotency_key: None,
        })
        .unwrap();
        malformed_metadata.pop();
        assert!(deserialize::<InvocationResultIndexMetadata>(&malformed_metadata).is_err());
        inner
            .set_many(
                "test",
                "seed_malformed_index",
                "invocation_result",
                namespace.clone(),
                &[
                    (
                        INVOCATION_RESULT_INDEX_METADATA_FIELD,
                        malformed_metadata.as_slice(),
                    ),
                    ("orphaned-mapping", b"stale"),
                ],
            )
            .await
            .unwrap();
        let stale_status = invocation_status(3, 1, &[(&current, 3)], 0, DeletedRegions::new());
        let current_status = invocation_status(3, 1, &[(&current, 3)], 1, DeletedRegions::new());

        let gate = faults.gate_next_pass("reset_invocation_result_index");
        let stale_task = tokio::spawn({
            let service = stale_service.clone();
            let owned_agent_id = owned_agent_id.clone();
            async move {
                service
                    .catch_up_invocation_result_index(
                        &owned_agent_id,
                        AgentMode::Durable,
                        invocation_index_fingerprint(),
                        &stale_status,
                    )
                    .await
            }
        });
        gate.entered().await;
        current_service
            .catch_up_invocation_result_index(
                &owned_agent_id,
                AgentMode::Durable,
                invocation_index_fingerprint(),
                &current_status,
            )
            .await
            .unwrap();
        gate.release();
        stale_task.await.unwrap().unwrap();

        let metadata = invocation_index_metadata(&current_service, &owned_agent_id).await;
        assert_eq!(metadata.revert_generation, 1);
        assert_eq!(metadata.covered_through, OplogIndex::from_u64(3));
        assert_eq!(
            inner
                .get(
                    "test",
                    "verify_reset",
                    "invocation_result",
                    namespace,
                    "orphaned-mapping",
                )
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            current_service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &current_status,
                    &current,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(3))
        );
    }

    #[test]
    async fn concurrent_generation_catch_up_clears_reverted_results() {
        let reverted = idempotency_key("reverted");
        let current = idempotency_key("current");
        let mut entries = invocation_entries(std::slice::from_ref(&reverted));
        entries.insert(
            OplogIndex::from_u64(4),
            OplogEntry::revert(OplogRegion::from_index_range(
                OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
            )),
        );
        invocation_pair(&mut entries, 5, &current);
        let (service, oplog, owned_agent_id) = index_test_service(entries);
        let old_status = invocation_status(3, 2, &[(&reverted, 3)], 0, DeletedRegions::new());
        let deleted_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
            OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
        )]);
        let new_status = invocation_status(6, 2, &[(&current, 6)], 1, deleted_regions);

        oplog.pause_next_read();
        let old_task = tokio::spawn({
            let service = service.clone();
            let owned_agent_id = owned_agent_id.clone();
            async move {
                service
                    .catch_up_invocation_result_index(
                        &owned_agent_id,
                        AgentMode::Durable,
                        invocation_index_fingerprint(),
                        &old_status,
                    )
                    .await
            }
        });
        oplog.read_started.notified().await;
        let new_task = tokio::spawn({
            let service = service.clone();
            let owned_agent_id = owned_agent_id.clone();
            let new_status = new_status.clone();
            async move {
                service
                    .catch_up_invocation_result_index(
                        &owned_agent_id,
                        AgentMode::Durable,
                        invocation_index_fingerprint(),
                        &new_status,
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!new_task.is_finished());
        oplog.resume_read.notify_one();
        old_task.await.unwrap().unwrap();
        new_task.await.unwrap().unwrap();

        let metadata = invocation_index_metadata(&service, &owned_agent_id).await;
        assert_eq!(metadata.revert_generation, 1);
        assert_eq!(metadata.covered_through, OplogIndex::from_u64(6));
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &new_status,
                    &reverted,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::DefinitiveMiss
        );
        assert_eq!(
            service
                .lookup_invocation_result_index(
                    &owned_agent_id,
                    invocation_index_fingerprint(),
                    &new_status,
                    &current,
                )
                .await
                .unwrap(),
            InvocationResultIndexLookup::Found(OplogIndex::from_u64(6))
        );
        let fields = service
            .key_value_storage
            .with("test", "list_invocation_result_index")
            .keys(DefaultWorkerService::invocation_result_index_namespace(
                &owned_agent_id.agent_id,
                invocation_index_fingerprint(),
            ))
            .await
            .unwrap();
        assert!(!fields.contains(&invocation_result_index_field(&reverted)));
    }

    #[test]
    async fn cancelled_catch_up_releases_invocation_result_index_lock_registration() {
        let key = idempotency_key("completed");
        let (service, oplog, owned_agent_id) =
            index_test_service(invocation_entries(std::slice::from_ref(&key)));
        let status = invocation_status(3, 1, &[(&key, 3)], 0, DeletedRegions::new());

        oplog.pause_next_read();
        let catch_up = tokio::spawn({
            let service = service.clone();
            let owned_agent_id = owned_agent_id.clone();
            async move {
                service
                    .catch_up_invocation_result_index(
                        &owned_agent_id,
                        AgentMode::Durable,
                        invocation_index_fingerprint(),
                        &status,
                    )
                    .await
            }
        });
        oplog.read_started.notified().await;

        catch_up.abort();
        assert!(catch_up.await.unwrap_err().is_cancelled());

        assert!(
            !service
                .invocation_result_index_locks
                .lock()
                .unwrap()
                .contains_key(&owned_agent_id),
            "a cancelled catch-up must not retain one dead lock registration per agent"
        );
    }

    #[test]
    async fn set_assignment_tracking_writes_durable_worker_to_recovery_index() {
        let (service, key_value_storage, owned_agent_id, number_of_shards) =
            assignment_tracking_test_service();
        let status = AgentStatusRecord {
            status: AgentStatus::Running,
            agent_mode: AgentMode::Durable,
            ..AgentStatusRecord::default()
        };

        service
            .set_assignment_tracking(&owned_agent_id, invocation_index_fingerprint(), &status)
            .await
            .unwrap();

        assert_eq!(
            assignment_tracking_members(&key_value_storage, &owned_agent_id, number_of_shards)
                .await,
            vec![RunningWorker {
                owned_agent_id,
                fingerprint: invocation_index_fingerprint(),
            }]
        );
    }

    #[test]
    async fn set_assignment_tracking_does_not_write_ephemeral_worker_to_recovery_index() {
        let (service, key_value_storage, owned_agent_id, number_of_shards) =
            assignment_tracking_test_service();
        let status = AgentStatusRecord {
            status: AgentStatus::Running,
            agent_mode: AgentMode::Ephemeral,
            ..AgentStatusRecord::default()
        };

        service
            .set_assignment_tracking(&owned_agent_id, invocation_index_fingerprint(), &status)
            .await
            .unwrap();

        assert!(
            assignment_tracking_members(&key_value_storage, &owned_agent_id, number_of_shards)
                .await
                .is_empty()
        );
    }

    #[test]
    async fn deletion_without_current_oplog_cleans_only_the_requested_incarnation() {
        let (service, storage, owned_agent_id, number_of_shards) =
            assignment_tracking_test_service();
        let first = AgentFingerprint(Uuid::from_u128(1));
        let second = AgentFingerprint(Uuid::from_u128(2));
        let first_status = AgentStatusRecord {
            status: AgentStatus::Running,
            agent_mode: AgentMode::Durable,
            oplog_idx: OplogIndex::INITIAL,
            ..AgentStatusRecord::default()
        };
        let second_status = AgentStatusRecord {
            component_size: 7,
            ..first_status.clone()
        };
        for (fingerprint, status) in [(first, &first_status), (second, &second_status)] {
            service
                .write_split_status(
                    &owned_agent_id,
                    DefaultWorkerService::status_namespace(&owned_agent_id.agent_id, fingerprint),
                    None,
                    status.clone(),
                )
                .await
                .unwrap();
            service
                .set_assignment_tracking(&owned_agent_id, fingerprint, status)
                .await
                .unwrap();
            service
                .reject_periodic_snapshots_through(&owned_agent_id, fingerprint, status.oplog_idx)
                .await
                .unwrap();
            storage
                .with_entity("test", "seed_invocation_index", "entry")
                .set_raw(
                    DefaultWorkerService::invocation_result_index_namespace(
                        &owned_agent_id.agent_id,
                        fingerprint,
                    ),
                    "entry",
                    b"value",
                )
                .await
                .unwrap();
        }
        service
            .write_cached_agent_mode(
                &owned_agent_id,
                &CachedAgentMode {
                    agent_mode: AgentMode::Durable,
                    fingerprint: second,
                },
            )
            .await
            .unwrap();

        service
            .remove(
                &mut crate::services::oplog::OpenOplogs::new("delete-test")
                    .lock_lifecycle(&owned_agent_id.agent_id)
                    .await,
                &owned_agent_id,
                AgentMode::Durable,
                first,
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            service
                .read_cached_status(&owned_agent_id, first)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            service
                .read_cached_status(&owned_agent_id, second)
                .await
                .unwrap(),
            Some(second_status.clone())
        );
        assert!(
            storage
                .with_entity("test", "read_invocation_index", "entry")
                .get_raw(
                    DefaultWorkerService::invocation_result_index_namespace(
                        &owned_agent_id.agent_id,
                        first,
                    ),
                    "entry",
                )
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            service
                .get_rejected_periodic_snapshot_through(&owned_agent_id, first)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            service
                .get_rejected_periodic_snapshot_through(&owned_agent_id, second)
                .await
                .unwrap(),
            Some(second_status.oplog_idx)
        );
        assert_eq!(
            service
                .read_cached_agent_mode(&owned_agent_id)
                .await
                .unwrap(),
            Some(CachedAgentMode {
                agent_mode: AgentMode::Durable,
                fingerprint: second,
            })
        );
        assert_eq!(
            assignment_tracking_members(&storage, &owned_agent_id, number_of_shards).await,
            vec![RunningWorker {
                owned_agent_id: owned_agent_id.clone(),
                fingerprint: second,
            }]
        );
        service
            .remove_cached_agent_mode_if_fingerprint(&owned_agent_id, second)
            .await
            .unwrap();
        assert_eq!(
            service
                .read_cached_agent_mode(&owned_agent_id)
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    fn tracks_workers_with_pending_invocations_for_assignment_recovery() {
        let mut status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };
        status.pending_invocations.push(PendingInvocationRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::INITIAL,
            idempotency_key: None,
            manual_update_target_revision: Some(
                golem_common::model::component::ComponentRevision::INITIAL,
            ),
        });

        assert!(DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }

    #[test]
    fn tracks_workers_with_pending_updates_for_assignment_recovery() {
        let status = AgentStatusRecord {
            status: AgentStatus::Idle,
            pending_updates: VecDeque::from([PendingUpdateRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::INITIAL,
                target_revision: golem_common::model::component::ComponentRevision::INITIAL,
                kind: PendingUpdateKind::Automatic,
            }]),
            ..AgentStatusRecord::default()
        };

        assert!(DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }

    #[test]
    fn tracks_idle_worker_with_pending_caller_side_stream_cancellation() {
        use golem_common::model::durable_stream::{
            LocalStreamId, StreamCancelReason, StreamCancelRole, StreamConsumerCancelIntentRecord,
            StreamInvocationId, StreamRecordReference,
        };

        let mut status = AgentStatusRecord::default();
        status
            .pending_durable_stream_cancellations
            .insert(StreamConsumerCancelIntentRecord {
                format_version: 1,
                session_key:
                    golem_common::model::durable_stream::StreamRegistrationInvocation::Remote(
                        StreamInvocationId {
                            callee_environment_id: EnvironmentId::new(),
                            callee: AgentId {
                                component_id: ComponentId::new(),
                                agent_id: "remote".into(),
                            },
                            callee_fingerprint: AgentFingerprint(uuid::Uuid::new_v4()),
                            idempotency_key: IdempotencyKey::new("caller-side".into()),
                        },
                    ),
                consumer_invocation: IdempotencyKey::new("consumer".into()),
                source: StreamRecordReference::Local(LocalStreamId(OplogIndex::from_u64(2))),
                epoch: 1,
                role: StreamCancelRole::OutputConsumer,
                reason: StreamCancelReason::Cancelled,
                details: None,
            });

        assert!(status.durable_stream_sessions.iter().next().is_none());
        assert!(DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
        status.pending_durable_stream_cancellations.clear();
        assert!(!DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }

    #[test]
    fn does_not_track_idle_workers_without_pending_work() {
        let status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };

        assert!(!DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }

    /// Minimal oplog service for recovery and identity tests.
    #[derive(Debug, Default)]
    struct FakeOplogService {
        existing: Vec<OwnedAgentId>,
        initial_entries: HashMap<(OwnedAgentId, AgentMode), OplogEntry>,
    }

    #[async_trait]
    impl OplogService for FakeOplogService {
        async fn staged_exists(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _stage_id: uuid::Uuid,
        ) -> Result<bool, String> {
            unimplemented!()
        }

        async fn lock_lifecycle(&self, _: &AgentId) -> OplogLifecycleGuard {
            unreachable!()
        }

        fn set_stream_session_index(&self, _index: Arc<StreamSessionIndexService>) {}

        fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
            None
        }

        async fn create_fresh(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _initial_entry: OplogEntry,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
            _shard_epoch: Option<ShardEpoch>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn create(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _initial_entry: OplogEntry,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
            _shard_epoch: Option<ShardEpoch>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn open(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _last_oplog_index: Option<OplogIndex>,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
            _shard_epoch: Option<ShardEpoch>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn get_last_index(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
        ) -> OplogIndex {
            unreachable!()
        }

        async fn delete(
            &self,
            _lifecycle: &mut OplogLifecycleGuard,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _expected_epoch: Option<golem_common::model::ShardEpoch>,
        ) -> Result<(), crate::services::oplog::OplogError> {
            unreachable!()
        }

        async fn read_exact(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _idx: OplogIndex,
            _n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            BTreeMap::new()
        }

        async fn read_source(
            &self,
            owned_agent_id: &OwnedAgentId,
            agent_mode: AgentMode,
            _idx: OplogIndex,
            _n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            self.initial_entries
                .get(&(owned_agent_id.clone(), agent_mode))
                .cloned()
                .map(|entry| BTreeMap::from([(OplogIndex::INITIAL, entry)]))
                .unwrap_or_default()
        }

        async fn exists(&self, owned_agent_id: &OwnedAgentId, _agent_mode: AgentMode) -> bool {
            self.existing.contains(owned_agent_id)
        }

        async fn scan_for_component(
            &self,
            _environment_id: &EnvironmentId,
            _component_id: &ComponentId,
            _modes: Option<AgentMode>,
            _cursor: ScanCursor,
            _count: u64,
        ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
            unreachable!()
        }

        async fn upload_raw_payload(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _data: Vec<u8>,
        ) -> Result<RawOplogPayload, String> {
            unreachable!()
        }

        async fn download_raw_payload(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unreachable!()
        }
    }

    #[derive(Debug)]
    struct UnusedComponentService;

    #[async_trait]
    impl ComponentService for UnusedComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            unreachable!()
        }

        async fn get_metadata(
            &self,
            _component_id: ComponentId,
            _forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            unreachable!()
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            unreachable!()
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            unreachable!()
        }

        async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {
            unreachable!()
        }
    }

    /// The error an unreachable cluster answers every operation with.
    fn unreachable() -> KeyValueStorageError {
        KeyValueStorageError::NotAttempted("pool acquire timed out".to_string())
    }

    /// Key-value storage whose every operation fails, standing in for an unreachable cluster.
    fn unreachable_storage() -> Arc<FaultInjectingKeyValueStorage> {
        let faults = KeyValueStorageFaults::default();
        faults.fail_all(usize::MAX, unreachable());
        Arc::new(FaultInjectingKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            faults,
        ))
    }

    fn test_owned_agent_id(name: &str) -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: name.to_string(),
            },
        )
    }

    fn test_create_entry(
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> OplogEntry {
        OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: owned_agent_id.agent_id.clone(),
            owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
            agent_mode,
            component_revision: ComponentRevision::INITIAL,
            env: Vec::new(),
            environment_id: owned_agent_id.environment_id,
            created_by: AccountId::new(),
            parent: None,
            component_size: 0,
            initial_total_linear_memory_size: 0,
            initial_active_plugins: HashSet::new(),
            local_agent_config: Vec::new(),
            original_phantom_id: None,
            instance_id: fingerprint.0,
        }))
    }

    fn test_worker_service(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        oplog_service: Arc<dyn OplogService>,
    ) -> DefaultWorkerService {
        DefaultWorkerService::new(
            key_value_storage,
            Arc::new(ShardServiceDefault::new()),
            oplog_service,
            Arc::new(UnusedComponentService),
            Arc::new(GolemConfig::default()),
        )
    }

    #[test]
    async fn recovery_scan_skips_workers_whose_oplog_is_gone() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
        let deleted = test_owned_agent_id("deleted-by-a-racing-delete");
        let deleted_member = RunningWorker {
            owned_agent_id: deleted,
            fingerprint: AgentFingerprint(Uuid::new_v4()),
        };
        storage
            .with_entity("worker", "add", "agent_id")
            .add_to_set(
                KeyValueStorageNamespace::RunningWorkers,
                &shard_key,
                &deleted_member,
            )
            .await
            .unwrap();

        let service = test_worker_service(storage.clone(), Arc::new(FakeOplogService::default()));

        // The index entry outlived the worker; recovery isolates that instead of aborting.
        let workers = service.enum_workers_at_key(&shard_key).await.unwrap();
        assert!(workers.is_empty());
        let remaining: Vec<RunningWorker> = storage
            .with_entity("worker", "enum", "agent_id")
            .members_of_set(KeyValueStorageNamespace::RunningWorkers, &shard_key)
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    async fn deletion_identity_classification_does_not_mutate_an_unrelated_hint() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let owned_agent_id = test_owned_agent_id("ephemeral-replacement");
        let current = AgentFingerprint(Uuid::from_u128(2));
        let unrelated_hint = AgentFingerprint(Uuid::from_u128(3));
        let oplog = FakeOplogService {
            initial_entries: HashMap::from([(
                (owned_agent_id.clone(), AgentMode::Ephemeral),
                test_create_entry(&owned_agent_id, AgentMode::Ephemeral, current),
            )]),
            ..FakeOplogService::default()
        };
        let service = test_worker_service(storage, Arc::new(oplog));
        let hint = CachedAgentMode {
            agent_mode: AgentMode::Durable,
            fingerprint: unrelated_hint,
        };
        service
            .write_cached_agent_mode(&owned_agent_id, &hint)
            .await
            .unwrap();

        let identity = service
            .read_agent_identity_for_deletion(&owned_agent_id, AgentMode::Durable)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(identity.agent_mode, AgentMode::Ephemeral);
        assert_eq!(identity.fingerprint, current);
        assert_eq!(
            service
                .read_cached_agent_mode(&owned_agent_id)
                .await
                .unwrap(),
            Some(hint)
        );
    }

    #[test]
    async fn stale_recovery_cleanup_removes_only_the_observed_incarnation() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
        let owned_agent_id = test_owned_agent_id("recreated");
        let stale = RunningWorker {
            owned_agent_id: owned_agent_id.clone(),
            fingerprint: AgentFingerprint(Uuid::from_u128(1)),
        };
        let current = RunningWorker {
            owned_agent_id,
            fingerprint: AgentFingerprint(Uuid::from_u128(2)),
        };
        for member in [&stale, &current] {
            storage
                .with_entity("worker", "add", "agent_id")
                .add_to_set(KeyValueStorageNamespace::RunningWorkers, &shard_key, member)
                .await
                .unwrap();
        }
        let service = test_worker_service(storage.clone(), Arc::new(FakeOplogService::default()));

        service
            .remove_running_worker_member(&shard_key, &stale)
            .await
            .unwrap();

        let remaining: Vec<RunningWorker> = storage
            .with_entity("worker", "enum", "agent_id")
            .members_of_set(KeyValueStorageNamespace::RunningWorkers, &shard_key)
            .await
            .unwrap();
        assert_eq!(remaining, vec![current]);
    }

    /// The distinction that matters more than the skip: an entry we could not *read* is not an
    /// entry that is gone. Skipping it would leave a running agent suspended for as long as this
    /// executor owns the shard, and would do it silently.
    #[test]
    async fn recovery_scan_fails_rather_than_stranding_a_worker_it_cannot_read() {
        let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
        let stranded = test_owned_agent_id("running-but-unreadable");
        let stranded_member = RunningWorker {
            owned_agent_id: stranded,
            fingerprint: AgentFingerprint(Uuid::new_v4()),
        };
        // The index itself still answers; every read of the worker behind it does not.
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        storage
            .with_entity("worker", "add", "agent_id")
            .add_to_set(
                KeyValueStorageNamespace::RunningWorkers,
                &shard_key,
                &stranded_member,
            )
            .await
            .unwrap();
        let faults = KeyValueStorageFaults::default();
        faults.fail_all_except(&["enum"], unreachable());

        let service = test_worker_service(
            Arc::new(FaultInjectingKeyValueStorage::new(storage, faults)),
            Arc::new(FakeOplogService::default()),
        );

        let result = service.enum_workers_at_key(&shard_key).await;

        assert!(
            result.is_err(),
            "an unreadable worker must fail the scan, not be skipped: {result:?}"
        );
    }

    #[test]
    async fn delete_reports_an_unreachable_storage() {
        let service =
            test_worker_service(unreachable_storage(), Arc::new(FakeOplogService::default()));
        let owned_agent_id = test_owned_agent_id("doomed");

        assert!(
            service
                .remove_cached_status(&owned_agent_id, AgentFingerprint(Uuid::new_v4()))
                .await
                .is_err(),
            "expected the cached status delete failure to surface"
        );
        assert!(
            service
                .remove(
                    &mut crate::services::oplog::OpenOplogs::new("delete-test")
                        .lock_lifecycle(&owned_agent_id.agent_id)
                        .await,
                    &owned_agent_id,
                    AgentMode::Durable,
                    AgentFingerprint(Uuid::new_v4()),
                    None,
                )
                .await
                .is_err(),
            "expected the delete failure to surface"
        );
    }

    #[test]
    async fn recovery_scan_reports_an_unreadable_index() {
        let service =
            test_worker_service(unreachable_storage(), Arc::new(FakeOplogService::default()));

        let result = service
            .enum_workers_at_key(&DefaultWorkerService::running_in_shard_key(&ShardId::new(
                0,
            )))
            .await;

        assert!(
            result.is_err(),
            "expected the index read failure to surface"
        );
    }
}
