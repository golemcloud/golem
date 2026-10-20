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
        component_revision_start_index: status.component_revision_start_index,
        current_retry_state: status.current_retry_state.clone(),
        authoritative_snapshot: status.authoritative_snapshot.clone(),
        last_automatic_snapshot: status.last_automatic_snapshot.clone(),
        previous_usable_automatic_snapshot: status.previous_usable_automatic_snapshot.clone(),
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

/// Requests the delete of the filesystem snapshots of a dead incarnation of an agent, whose
/// recovery-index member is stale.
pub type OnStale<'a> = &'a (dyn Fn(&OwnedAgentId, AgentFingerprint) + Send + Sync);

/// Why a recovery-index member is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaleReason {
    /// The agent has no oplog.
    Absent,
    /// The oplog of the agent belongs to another incarnation.
    FingerprintMismatch,
}

impl StaleReason {
    /// The label of the stale-member metric.
    fn label(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::FingerprintMismatch => "fingerprint_mismatch",
        }
    }
}

/// Logs that recovery leaves the agent as it is, because its oplog does not read. The member stays
/// in the recovery index, and the oplog stays as it is, so an operator can inspect it.
fn skip_unreadable_agent(owned_agent_id: &OwnedAgentId, error: &WorkerExecutorError) {
    error!(
        agent_id = %owned_agent_id,
        error = %error,
        "Skipping agent during recovery: its oplog is unreadable"
    );
}

/// Tells why the member of the incarnation `fingerprint` is stale, when `identity` is the stored
/// identity of its agent: no identity, or an identity with another fingerprint. `None` means that
/// the member is the stored incarnation.
fn stale_reason(
    identity: Option<&ResolvedAgentIdentity>,
    fingerprint: AgentFingerprint,
) -> Option<StaleReason> {
    match identity {
        None => Some(StaleReason::Absent),
        Some(identity) if identity.fingerprint != fingerprint => {
            Some(StaleReason::FingerprintMismatch)
        }
        Some(_) => None,
    }
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

    async fn enum_workers_at_key(
        &self,
        key: &str,
        on_stale: OnStale<'_>,
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
            //
            // An oplog that does not read is the exception: no retry reads it, so failing the scan
            // would strand every other agent of the shards behind it. That agent is left as it is.
            let identity = match self.resolve_agent_identity(owned_agent_id).await {
                Ok(identity) => identity,
                Err(error @ WorkerExecutorError::OplogFailed { .. }) => {
                    skip_unreadable_agent(owned_agent_id, &error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(reason) = stale_reason(identity.as_ref(), running_worker.fingerprint) {
                self.drop_stale_member(key, &running_worker, reason, on_stale)
                    .await?;
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
                // The metadata names another incarnation: the same comparison of fingerprints as
                // `stale_reason`, made on the metadata.
                Ok(Some(_)) => {
                    self.drop_stale_member(
                        key,
                        &running_worker,
                        StaleReason::FingerprintMismatch,
                        on_stale,
                    )
                    .await?;
                    debug!(
                        "Skipping {owned_agent_id} during recovery: stale recovery-index member"
                    );
                }
                Ok(None) => {
                    let current = self.resolve_agent_identity(owned_agent_id).await?;
                    if let Some(reason) = stale_reason(current.as_ref(), running_worker.fingerprint)
                    {
                        self.drop_stale_member(key, &running_worker, reason, on_stale)
                            .await?;
                    } else {
                        return Err(WorkerExecutorError::runtime(format!(
                            "failed to load metadata for existing {owned_agent_id} during recovery"
                        )));
                    }
                }
                Err(error @ WorkerExecutorError::OplogFailed { .. }) => {
                    skip_unreadable_agent(owned_agent_id, &error);
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

    /// Requests the delete of the snapshots of the stale member, then removes it from the index.
    /// A removal that fails leaves the member, so the next scan requests the delete again.
    async fn drop_stale_member(
        &self,
        key: &str,
        running_worker: &RunningWorker,
        reason: StaleReason,
        on_stale: OnStale<'_>,
    ) -> Result<(), WorkerExecutorError> {
        on_stale(&running_worker.owned_agent_id, running_worker.fingerprint);
        self.remove_running_worker_member(key, running_worker)
            .await?;
        record_stale_running_worker(reason.label());
        Ok(())
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

    /// Key holding the worker's immutable mode and fingerprint as a hint for identity resolution,
    /// stored separately from `AgentStatusRecord`. Populated lazily for durable workers and never
    /// written on the per-commit hot path. The `Create` entry remains authoritative, so stale hints
    /// are replaced or removed. This lives in the `Worker` namespace because it has an independent
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
            .await?
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
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), SessionError> {
        self.stream_session_index
            .lookup_producer_metadata(owned_agent_id, agent_mode, fingerprint, keys)
            .await
    }

    async fn lookup_durable_stream_recovery_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<DurableStreamRecoveryMetadata, SessionError> {
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
    ) -> Result<Option<OplogIndex>, SessionError> {
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
    ) -> Result<SessionControlMetadata, SessionError> {
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
    ) -> Result<Vec<OplogIndex>, SessionError> {
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
                        .await?;

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
        on_stale: OnStale<'_>,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError> {
        let shard_assignment = self.shard_service.try_get_current_assignment();
        let mut result = Vec::new();
        if let Some(shard_assignment) = shard_assignment {
            for shard_id in shard_assignment.shard_ids() {
                let key = Self::running_in_shard_key(&shard_id);
                let mut shard = self.enum_workers_at_key(&key, on_stale).await?;
                result.append(&mut shard);
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
        after_oplog_delete: &(dyn Fn(AgentFingerprint) + Send + Sync),
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
            // A previous deletion attempt may already have removed the Create entry while deeper
            // archive storage still needs cleanup. The lifecycle fence excludes a concurrent new
            // incarnation, so absence means this deletion must continue rather than declare the
            // partial cleanup complete.
            None => true,
        };

        // The oplog goes after everything it can rebuild, so a crash part-way leaves an oplog
        // behind: the next attempt (or load) re-derives whatever of the state below is missing and
        // finishes from there. Removed first, a crash would leave orphaned keys that a retry reads
        // as an agent already gone. The recovery-index entry comes after it: one a crash leaves
        // behind names no oplog, and the next recovery scan drops it.
        //
        // The epoch is confirmed before anything is removed, so a delete turned away by the fence
        // removes nothing. A new owner claiming the shard between this check and the oplog delete
        // is still refused by that delete, which keeps the oplog: the state removed in between is
        // what the new owner rebuilds from it.
        if delete_current_oplog && let Some(epoch) = expected_epoch {
            self.oplog_service
                .assert_owning_epoch(owned_agent_id, agent_mode, epoch)
                .await?;
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
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        self.key_value_storage
            .with("worker", "remove_rejected_periodic_snapshot")
            .del(
                Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
                &Self::rejected_periodic_snapshots_field(fingerprint),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;

        // Only this incarnation's oplog: a recreated agent's is not the deleting one's to remove.
        if delete_current_oplog {
            self.oplog_service
                .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
                .await?;
        }
        after_oplog_delete(fingerprint);

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

    async fn get_rejected_periodic_snapshots(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<HashSet<OplogIndex>, WorkerExecutorError> {
        let value: Option<Result<Vec<OplogIndex>, String>> = self
            .key_value_storage
            .with_entity("worker", "get_rejected_periodic_snapshots", "oplog_indexes")
            .get_attempt_deserialize(
                Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
                &Self::rejected_periodic_snapshots_field(fingerprint),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;
        value
            .transpose()
            .map(|indexes| indexes.into_iter().flatten().collect())
            .map_err(WorkerExecutorError::runtime)
    }

    async fn reject_periodic_snapshots(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        oplog_indexes: &HashSet<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        let namespace = Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id);
        let field = Self::rejected_periodic_snapshots_field(fingerprint);
        loop {
            let current = self
                .key_value_storage
                .with_entity(
                    "worker",
                    "read_rejected_periodic_snapshots",
                    "oplog_indexes",
                )
                .get_raw(namespace.clone(), &field)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            let current_indexes = match &current {
                Some(current) => {
                    deserialize::<Vec<OplogIndex>>(current).map_err(WorkerExecutorError::runtime)?
                }
                None => Vec::new(),
            };
            let Some(to_store) = rejections_to_store(current_indexes, status, oplog_indexes) else {
                return Ok(());
            };
            let encoded = serialize(&to_store).map_err(WorkerExecutorError::runtime)?;
            let updated = self
                .key_value_storage
                .with_entity("worker", "reject_periodic_snapshots", "oplog_indexes")
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
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
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
    ) -> Result<Option<DurableStreamPublicBinding>, SessionError> {
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
    ) -> Result<(), WorkerExecutorError> {
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
                    .await.map_err(WorkerExecutorError::runtime)?
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
                            .await.map_err(WorkerExecutorError::runtime)?;
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
                                let encoded = serialize(&metadata).map_err(WorkerExecutorError::runtime)?;
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
                                    }).map_err(WorkerExecutorError::runtime)?;
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
                        .await?;
                    if entries.is_empty() {
                        return Err(WorkerExecutorError::runtime(format!(
                            "failed to advance invocation result index for {owned_agent_id}: oplog range starting at {} was empty",
                            metadata.covered_through.next()
                        )));
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
                            }).map_err(WorkerExecutorError::runtime)?,
                        ));
                    }
                    fields.push((
                        INVOCATION_RESULT_INDEX_METADATA_FIELD.to_string(),
                        serialize(&advanced).map_err(WorkerExecutorError::runtime)?,
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
                        }).map_err(WorkerExecutorError::runtime)?;
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
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
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
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
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
        .map_err(WorkerExecutorError::runtime)
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
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
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
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }
        self.write_split_status(
            owned_agent_id,
            Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
            previous_checkpoint,
            checkpoint,
        )
        .await
        .map_err(WorkerExecutorError::runtime)
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

    async fn remove_if_stale(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        on_stale: OnStale<'_>,
    ) -> Result<bool, WorkerExecutorError> {
        let identity = self.resolve_agent_identity(owned_agent_id).await?;
        let Some(reason) = stale_reason(identity.as_ref(), fingerprint) else {
            return Ok(false);
        };
        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assignment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);
        self.drop_stale_member(
            &Self::running_in_shard_key(&shard_id),
            &RunningWorker {
                owned_agent_id: owned_agent_id.clone(),
                fingerprint,
            },
            reason,
            on_stale,
        )
        .await?;
        Ok(true)
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
