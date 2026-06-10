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
use crate::metrics::workers::record_worker_call;
use crate::services::oplog::OplogService;
use crate::services::shard::ShardService;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::status::calculate_last_known_status_with_checkpoint_reader;
use async_trait::async_trait;
use golem_common::model::agent::{AgentMode, LegacyParsedAgentId};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatus, AgentStatusRecord, FailedUpdateRecord,
    IdempotencyKey, OwnedAgentId, ShardId, SuccessfulUpdateRecord,
};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::debug;

/// Hash field holding the bounded part of the cached `AgentStatusRecord` (everything except the
/// unbounded fields that are stored separately). Always present for a cached status; its absence is
/// treated as a cache miss.
const STATUS_CORE_FIELD: &str = "core";
/// Hash field holding `(skipped_regions, deleted_regions)`. Written only when the regions change.
const STATUS_REGIONS_FIELD: &str = "regions";
/// Hash field holding `(failed_updates, successful_updates)`. Written only when they change.
const STATUS_UPDATES_FIELD: &str = "updates";
/// Prefix for per-idempotency-key invocation result fields (`ir:{idempotency_key}` -> `OplogIndex`).
const STATUS_INVOCATION_RESULT_PREFIX: &str = "ir:";

fn status_invocation_result_field(key: &IdempotencyKey) -> String {
    format!("{STATUS_INVOCATION_RESULT_PREFIX}{}", key.value)
}

/// The result of computing a status cache write: `(fields_to_set, field_names_to_delete)`.
type StatusFieldWrites = (Vec<(String, Vec<u8>)>, Vec<String>);

/// The unbounded parts of an [`AgentStatusRecord`] that are stored separately from `core`. They are
/// taken out of the record (`mem::take`) before serializing `core`, so this never clones the large
/// fields.
struct SplitStatusParts {
    invocation_results: HashMap<IdempotencyKey, OplogIndex>,
    failed_updates: Vec<FailedUpdateRecord>,
    successful_updates: Vec<SuccessfulUpdateRecord>,
    skipped_regions: DeletedRegions,
    deleted_regions: DeletedRegions,
}

/// Moves the unbounded fields out of `status`, leaving it as the small fixed-size `core` that is
/// serialized into the `core` field. Uses `mem::take`/`mem::replace`, so it does not clone the
/// (potentially large) invocation results / updates / regions.
fn split_status(status: &mut AgentStatusRecord) -> SplitStatusParts {
    SplitStatusParts {
        invocation_results: std::mem::take(&mut status.invocation_results),
        failed_updates: std::mem::take(&mut status.failed_updates),
        successful_updates: std::mem::take(&mut status.successful_updates),
        skipped_regions: std::mem::replace(&mut status.skipped_regions, DeletedRegions::new()),
        deleted_regions: std::mem::replace(&mut status.deleted_regions, DeletedRegions::new()),
    }
}

/// Computes the minimal set of hash-field writes/deletes needed to bring the cached status up to
/// date.
///
/// `core` must be the already-split (emptied) record. When `previous` is `Some`, the result is a
/// delta against it (this is the hot path; invocation results only ever grow there, so `dels` is
/// usually empty). When `previous` is `None` (cold path: create / cache-miss recompute / detach
/// reload), every part is written and `existing_invocation_result_fields` is used to delete stale
/// `ir:` fields that are no longer present (e.g. after a revert removed results).
///
/// `core` is always part of `sets` (it carries the `oplog_idx` marker), so the marker and every
/// written part advance together in one atomic `set_many` by the caller.
fn compute_status_field_writes(
    previous: Option<&AgentStatusRecord>,
    existing_invocation_result_fields: &[String],
    core: &AgentStatusRecord,
    parts: &SplitStatusParts,
) -> Result<StatusFieldWrites, String> {
    let mut sets: Vec<(String, Vec<u8>)> = Vec::new();
    let mut dels: Vec<String> = Vec::new();

    sets.push((STATUS_CORE_FIELD.to_string(), serialize(core)?));

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
            for (key, oplog_idx) in &parts.invocation_results {
                if previous.invocation_results.get(key) != Some(oplog_idx) {
                    sets.push((status_invocation_result_field(key), serialize(oplog_idx)?));
                }
            }
            for key in previous.invocation_results.keys() {
                if !parts.invocation_results.contains_key(key) {
                    dels.push(status_invocation_result_field(key));
                }
            }
        }
        None => {
            let new_fields: HashSet<String> = parts
                .invocation_results
                .keys()
                .map(status_invocation_result_field)
                .collect();
            for (key, oplog_idx) in &parts.invocation_results {
                sets.push((status_invocation_result_field(key), serialize(oplog_idx)?));
            }
            for field in existing_invocation_result_fields {
                if field.starts_with(STATUS_INVOCATION_RESULT_PREFIX) && !new_fields.contains(field)
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
    let mut regions: Option<(DeletedRegions, DeletedRegions)> = None;
    let mut updates: Option<(Vec<FailedUpdateRecord>, Vec<SuccessfulUpdateRecord>)> = None;
    let mut invocation_results: HashMap<IdempotencyKey, OplogIndex> = HashMap::new();

    for (name, bytes) in fields {
        if name == STATUS_CORE_FIELD {
            core = Some(deserialize::<AgentStatusRecord>(&bytes).ok()?);
        } else if name == STATUS_REGIONS_FIELD {
            regions = Some(deserialize::<(DeletedRegions, DeletedRegions)>(&bytes).ok()?);
        } else if name == STATUS_UPDATES_FIELD {
            updates = Some(
                deserialize::<(Vec<FailedUpdateRecord>, Vec<SuccessfulUpdateRecord>)>(&bytes)
                    .ok()?,
            );
        } else if let Some(key) = name.strip_prefix(STATUS_INVOCATION_RESULT_PREFIX) {
            let oplog_idx = deserialize::<OplogIndex>(&bytes).ok()?;
            invocation_results.insert(IdempotencyKey::new(key.to_string()), oplog_idx);
        }
        // Unknown fields are ignored.
    }

    let mut status = core?;
    if let Some((skipped_regions, deleted_regions)) = regions {
        status.skipped_regions = skipped_regions;
        status.deleted_regions = deleted_regions;
    }
    if let Some((failed_updates, successful_updates)) = updates {
        status.failed_updates = failed_updates;
        status.successful_updates = successful_updates;
    }
    status.invocation_results = invocation_results;
    Some(status)
}

#[derive(Debug, Clone)]
pub struct GetWorkerMetadataResult {
    // Status of the worker at the time of the create oplog entry
    pub initial_worker_metadata: AgentMetadata,
    // Last known cached status of the worker. Might be outdated
    pub last_known_status: Option<AgentStatusRecord>,
}

/// Service for persisting the current set of Golem workers represented by their metadata
#[async_trait]
pub trait WorkerService: Send + Sync {
    async fn get(&self, owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult>;

    async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult>;

    async fn remove(&self, owned_agent_id: &OwnedAgentId);

    async fn remove_cached_status(&self, owned_agent_id: &OwnedAgentId);

    /// Returns the persisted [`AgentMode`] for the worker, if it exists.
    ///
    /// The mode is decided at worker create time and persisted in the `Create` oplog entry,
    /// and is also kept on the cached [`AgentStatusRecord`]. This method first consults the
    /// cached status record (which exists for active durable workers) and, on cache miss,
    /// probes both oplog namespaces (durable and ephemeral). Returns `None` if no oplog
    /// exists in either namespace.
    async fn get_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentMode>;

    /// Writes the cached status *blob* for the worker (no `RunningWorkers` index maintenance).
    ///
    /// The cached `AgentStatusRecord` is stored split across several fields of a per-agent hash
    /// (see [`KeyValueStorageNamespace::AgentStatus`]): a small `core`, the `regions`, the
    /// `updates`, and one field per idempotency key. Only the fields that actually changed are
    /// written, so the unbounded parts (most notably the invocation results) are not re-sent on
    /// every flush.
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
    /// Returns `None` on a cache miss or stale format. The transient `agent_mode` field (not part
    /// of the persisted `core`) is restored from `agent_mode`.
    async fn read_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Option<AgentStatusRecord>;

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
        previous_checkpoint: Option<&AgentStatusRecord>,
        checkpoint: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, String>;

    /// Updates the `RunningWorkers` recovery index for the worker according to `status_value`.
    ///
    /// This is the authoritative index consulted on crash/reshard recovery to decide which workers
    /// to resume, so it is always maintained synchronously (never deferred to the background
    /// flusher). The worker is added when [`should_track_for_assignment_recovery`] holds and
    /// removed otherwise. No-op for ephemeral workers.
    async fn set_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        status_value: &AgentStatusRecord,
    );

    /// Convenience cold-path helper that writes the blob *and* updates the recovery index in one
    /// call. Panics on a blob write failure (cold paths cannot meaningfully recover). Hot paths use
    /// the background flusher (blob) together with [`set_assignment_tracking`] (index) instead.
    async fn update_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) {
        self.set_assignment_tracking(owned_agent_id, &status_value)
            .await;
        self.write_cached_status(owned_agent_id, previous_status, status_value)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to write cached status for {owned_agent_id}: {err}")
            });
    }
}

#[derive(Clone)]
pub struct DefaultWorkerService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    shard_service: Arc<dyn ShardService>,
    oplog_service: Arc<dyn OplogService>,
    component_service: Arc<dyn ComponentService>,
    config: Arc<GolemConfig>,
}

impl DefaultWorkerService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        oplog_service: Arc<dyn OplogService>,
        component_service: Arc<dyn ComponentService>,
        config: Arc<GolemConfig>,
    ) -> Self {
        Self {
            key_value_storage,
            shard_service,
            oplog_service,
            component_service,
            config,
        }
    }

    async fn enum_workers_at_key(&self, key: &str) -> Vec<GetWorkerMetadataResult> {
        record_worker_call("enum");

        let value: Vec<OwnedAgentId> = self
            .key_value_storage
            .with_entity("worker", "enum", "agent_id")
            .members_of_set(KeyValueStorageNamespace::RunningWorkers, key)
            .await
            .unwrap_or_else(|err| panic!("failed to get worker ids from KV storage: {err}"));

        let mut workers = Vec::new();

        for owned_agent_id in value {
            let metadata = self
                .get(&owned_agent_id)
                .await
                .unwrap_or_else(|| panic!("failed to get worker metadata from KV storage"));
            workers.push(metadata);
        }

        workers
    }

    /// Namespace holding the agent's split cached status (one per-agent hash whose fields are
    /// `core`, `regions`, `updates`, and `ir:{idempotency_key}`).
    fn status_namespace(agent_id: &AgentId) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentStatus {
            agent_id: agent_id.clone(),
        }
    }

    /// Namespace holding the agent's *clean* status checkpoint. Same physical split layout as
    /// [`Self::status_namespace`], but written only at structurally clean boundaries (snapshot
    /// save / throttled idle) and never advanced by the background flusher, so it can serve as a
    /// fold baseline that always predates any later jump region.
    fn checkpoint_namespace(agent_id: &AgentId) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentStatusCheckpoint {
            agent_id: agent_id.clone(),
        }
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
    /// split hash fields (`core`, `regions`, `updates`, `ir:{key}`). Returns `None` if the `core`
    /// field is missing (cache miss) or any field cannot be deserialized in the current format
    /// (treated as a cache miss).
    ///
    /// `agent_mode` is `#[transient]` and not part of `core`, so the returned record carries the
    /// `Durable` deserialization default; callers must restore it from the authoritative source.
    async fn read_cached_status(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentStatusRecord> {
        self.read_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id),
        )
        .await
    }

    /// Reads a split status record (live cache or checkpoint) from `namespace`, reassembling it
    /// from the `core` / `regions` / `updates` / `ir:{key}` fields. Returns `None` if `core` is
    /// missing (cache miss / torn write) or any field cannot be deserialized in the current format.
    ///
    /// `agent_mode` is `#[transient]` and not part of `core`, so the returned record carries the
    /// `Durable` deserialization default; callers must restore it from the authoritative source.
    async fn read_split_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
    ) -> Option<AgentStatusRecord> {
        // Single atomic read of every field of the per-agent status hash (`core`, `regions`,
        // `updates`, `ir:{key}`). This is one round-trip (Redis `HGETALL`, a single
        // `SELECT ... WHERE namespace`, or one locked scan in memory) that observes a consistent
        // snapshot, so it cannot reassemble a torn, mixed-generation record. (A naive
        // `keys` + `get_many` would be two round-trips, leaving a window where a concurrent writer
        // — the background status flusher for the live cache, or the clean-checkpoint writer for
        // the checkpoint namespace — could add a new `ir:{key}` field the earlier `keys` did not
        // list, yielding a record at the newer `core.oplog_idx` missing an invocation result.)
        let fields = self
            .key_value_storage
            .with_entity("worker", "read_cached_status", "agent_status")
            .get_all_raw(namespace)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get agent status for {owned_agent_id} from KV storage: {err}")
            });

        // No `core` field -> nothing cached (or a torn/partial write); treat as a cache miss.
        if !fields.iter().any(|(name, _)| name == STATUS_CORE_FIELD) {
            return None;
        }

        reassemble_cached_status(fields)
    }

    /// Writes the split status fields for an agent, sending only the parts that changed.
    ///
    /// `core` is always written (it carries the `oplog_idx` marker that versions the whole record).
    /// `regions`/`updates` are written only when they differ from `previous_status`, and invocation
    /// results are written per idempotency key (only newly added/changed keys).
    ///
    /// Atomicity: the marker (in `core`) and every field written in the same call advance together
    /// in a single atomic `set_many` (one `HMSET` on Redis, one transaction on SQL). This preserves
    /// the invariant that each persisted field's content matches `core.oplog_idx`, which the oplog
    /// fold relies on. When stale fields must be removed (e.g. invocation results dropped by a
    /// revert), they are deleted *together with* `core` before the `set_many`. Dropping `core`
    /// first is what makes the two-step delete-then-write crash-safe: with `core` absent, any crash
    /// or read in the gap before the final write is treated as a cache miss and recomputed from the
    /// oplog, rather than reassembling a torn record whose remaining fields no longer match the
    /// (stale) marker. The final `set_many` always re-establishes `core` atomically with the
    /// changed parts.
    async fn write_status_fields(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
        previous_status: Option<&AgentStatusRecord>,
        core: &AgentStatusRecord,
        parts: &SplitStatusParts,
    ) -> Result<(), String> {
        // On the cold path (no in-memory previous) we reconcile invocation-result fields against
        // what is currently stored, so that results removed by a revert are deleted.
        let existing_fields = if previous_status.is_none() {
            self.key_value_storage
                .with("worker", "update_status")
                .keys(namespace.clone())
                .await
                .map_err(|err| {
                    format!("failed to list agent status fields for {owned_agent_id}: {err}")
                })?
        } else {
            Vec::new()
        };

        let (sets, dels) =
            compute_status_field_writes(previous_status, &existing_fields, core, parts).map_err(
                |err| format!("failed to serialize agent status for {owned_agent_id}: {err}"),
            )?;

        // Delete stale fields first, dropping `core` along with them so the cache reads as a miss
        // until the final `set_many` re-establishes it (see atomicity note above).
        if !dels.is_empty() {
            let mut to_delete = dels;
            to_delete.push(STATUS_CORE_FIELD.to_string());
            self.key_value_storage
                .with("worker", "update_status")
                .del_many(namespace.clone(), to_delete)
                .await
                .map_err(|err| {
                    format!(
                        "failed to remove stale agent status fields for {owned_agent_id}: {err}"
                    )
                })?;
        }

        // Single atomic write: core + changed parts + new/updated invocation results.
        let pairs: Vec<(&str, &[u8])> = sets
            .iter()
            .map(|(field, bytes)| (field.as_str(), bytes.as_slice()))
            .collect();

        self.key_value_storage
            .with_entity("worker", "update_status", "agent_status")
            .set_many_raw(namespace, &pairs)
            .await
            .map_err(|err| format!("failed to set agent status in KV storage: {err}"))?;

        Ok(())
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

        // Split the record: take the unbounded fields out so `core` stays small and fixed-size.
        // `split_status` moves the large fields out of `core` into `parts` (no clone).
        let mut core = status_value;
        let parts = split_status(&mut core);

        self.write_status_fields(owned_agent_id, namespace, previous_status, &core, &parts)
            .await?;

        // Reassemble the record (moving the parts back into `core`, no clone) so the caller gets
        // back a complete baseline for computing the next delta.
        let mut reassembled = core;
        reassembled.skipped_regions = parts.skipped_regions;
        reassembled.deleted_regions = parts.deleted_regions;
        reassembled.failed_updates = parts.failed_updates;
        reassembled.successful_updates = parts.successful_updates;
        reassembled.invocation_results = parts.invocation_results;
        Ok(reassembled)
    }

    /// Deletes every field of a split status hash (`namespace`). Enumerating + deleting is fine on
    /// the cold `remove` path: the agent is owned by this executor (no concurrent writer).
    async fn remove_split_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
    ) {
        let status_fields = self
            .key_value_storage
            .with("worker", "remove")
            .keys(namespace.clone())
            .await
            .unwrap_or_else(|err| {
                panic!("failed to list agent status fields for {owned_agent_id}: {err}")
            });

        if !status_fields.is_empty() {
            self.key_value_storage
                .with("worker", "remove")
                .del_many(namespace, status_fields)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to remove agent status in the KV storage: {err}")
                });
        }
    }

    /// Reads the dedicated `agent_mode` key, if present. Returns `None` on a cache miss or if the
    /// stored value cannot be deserialized in the current format (treated as a miss).
    async fn read_cached_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
        let value: Option<Result<AgentMode, String>> = self
            .key_value_storage
            .with_entity("worker", "read_cached_agent_mode", "agent_mode")
            .get_attempt_deserialize(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get agent mode for {owned_agent_id} from KV storage: {err}")
            });

        match value {
            Some(Ok(agent_mode)) => Some(agent_mode),
            Some(Err(_)) | None => None,
        }
    }

    /// Populates the dedicated `agent_mode` key. Only called on a `get_agent_mode` cache miss for
    /// durable workers, never on the per-commit hot path. The value is immutable for the life of
    /// the worker, so concurrent writers would write the same value.
    async fn write_cached_agent_mode(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
        self.key_value_storage
            .with_entity("worker", "write_cached_agent_mode", "agent_mode")
            .set(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
                &agent_mode,
            )
            .await
            .unwrap_or_else(|err| panic!("failed to set agent mode in KV storage: {err}"));
    }

    pub(crate) fn should_track_for_assignment_recovery(status: &AgentStatusRecord) -> bool {
        matches!(
            status.status,
            AgentStatus::Running | AgentStatus::Retrying | AgentStatus::Interrupted
        ) || status.has_pending_work()
    }
}

#[async_trait]
impl WorkerService for DefaultWorkerService {
    async fn get(&self, owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult> {
        record_worker_call("get");

        let agent_mode = self.get_agent_mode(owned_agent_id).await?;

        let initial_oplog_entry = self
            .oplog_service
            .read(owned_agent_id, agent_mode, OplogIndex::INITIAL, 1)
            .await
            .into_iter()
            .next();

        debug!("Found initial oplog entry for worker: {initial_oplog_entry:?}");

        match initial_oplog_entry {
            None => None,
            Some((
                _,
                OplogEntry::Create {
                    agent_id,
                    agent_mode: persisted_agent_mode,
                    component_revision,
                    env,
                    environment_id,
                    created_by,
                    timestamp,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins,
                    local_agent_config,
                    original_phantom_id,
                    agent_initial_card,
                    instance_id,
                },
            )) => {
                debug_assert_eq!(persisted_agent_mode, agent_mode);
                let agent_mode = persisted_agent_mode;
                let agent_type_name =
                    LegacyParsedAgentId::parse_agent_type_name(&agent_id.agent_id).ok();
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
                    })?;

                let config = local_agent_config
                    .into_iter()
                    .map(|lac| {
                        lac.enrich_with_type(&component_metadata.metadata, agent_type_name.as_ref())
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap_or_else(|err| {
                        panic!("failed enriching local agent config for {owned_agent_id}: {err}")
                    });

                let agent_initial_card = if agent_initial_card.is_empty() {
                    golem_common::model::card::Card {
                        card_id: golem_common::model::card::CardId::new(),
                        parent_ids: Vec::new(),
                        lower_positive: Vec::new(),
                        lower_negative: Vec::new(),
                        upper_positive: Vec::new(),
                        upper_negative: Vec::new(),
                        created_at: chrono::Utc::now(),
                        expires_at: None,
                        system_card: false,
                        managed_by: None,
                    }
                } else {
                    golem_common::serialization::deserialize(&agent_initial_card)
                        .unwrap_or_else(|err| {
                            panic!(
                                "failed to deserialize agent initial card for {owned_agent_id}: {err}"
                            )
                        })
                };

                let initial_worker_metadata = AgentMetadata {
                    agent_id,
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
                        agent_mode,
                        ..AgentStatusRecord::default()
                    },
                    original_phantom_id,
                    fingerprint: AgentFingerprint(instance_id),
                    agent_mode,
                    agent_initial_card,
                };

                let last_known_status = match self.read_cached_status(owned_agent_id).await {
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
                            || self.read_status_checkpoint(owned_agent_id, agent_mode),
                        )
                        .await
                        .expect("Failed to recompute worker status for existing worker");

                        // Cold path: no in-memory previous, reconcile against stored fields.
                        self.update_cached_status(owned_agent_id, None, last_known_status.clone())
                            .await;

                        Some(last_known_status)
                    }
                };

                Some(GetWorkerMetadataResult {
                    initial_worker_metadata,
                    last_known_status,
                })
            }
            Some(_) => panic!("Encountered malformed oplog without create oplog entry"),
        }
    }

    async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult> {
        let shard_assignment = self.shard_service.try_get_current_assignment();
        let mut result: Vec<GetWorkerMetadataResult> = vec![];
        if let Some(shard_assignment) = shard_assignment {
            for shard_id in shard_assignment.shard_ids {
                let key = Self::running_in_shard_key(&shard_id);
                let mut shard_worker = self.enum_workers_at_key(&key).await;
                result.append(&mut shard_worker);
            }
        }
        result
    }

    async fn remove(&self, owned_agent_id: &OwnedAgentId) {
        record_worker_call("remove");

        if let Some(agent_mode) = self.get_agent_mode(owned_agent_id).await {
            self.oplog_service.delete(owned_agent_id, agent_mode).await;
        }
        self.remove_cached_status(owned_agent_id).await;

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assigment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);

        self
            .key_value_storage
            .with_entity("worker", "remove", "agent_id")
            .remove_from_set(KeyValueStorageNamespace::RunningWorkers, &Self::running_in_shard_key(&shard_id), owned_agent_id)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove worker from the set of running worker ids per shard in KV storage: {err}"
                )
            });
    }

    async fn remove_cached_status(&self, owned_agent_id: &OwnedAgentId) {
        record_worker_call("remove_cached_status");

        let agent_id = &owned_agent_id.agent_id;

        // Delete the live cached status hash and the clean checkpoint hash.
        self.remove_split_status(owned_agent_id, Self::status_namespace(agent_id))
            .await;
        self.remove_split_status(owned_agent_id, Self::checkpoint_namespace(agent_id))
            .await;

        // The `agent_mode` key has its own lifecycle and lives in the `Worker` namespace.
        self.key_value_storage
            .with("worker", "remove")
            .del(
                KeyValueStorageNamespace::Worker {
                    agent_id: agent_id.clone(),
                },
                &Self::agent_mode_key(agent_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove worker agent mode in the KV storage: {err}")
            });
    }

    async fn get_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
        record_worker_call("get_agent_mode");

        // Fast path: dedicated `agent_mode` key (only populated for durable workers).
        if let Some(agent_mode) = self.read_cached_agent_mode(owned_agent_id).await {
            return Some(agent_mode);
        }

        // Cache miss (e.g. ephemeral worker, or durable worker whose dedicated key has not been
        // populated yet): probe both oplog namespaces. Constant-time existence checks.
        if self
            .oplog_service
            .exists(owned_agent_id, AgentMode::Durable)
            .await
        {
            // Populate the dedicated key so subsequent lookups skip the oplog probe. Only durable
            // workers are cached (mirrors `update_cached_status`): an ephemeral worker's oplog is
            // transient, so caching its mode could outlive the worker and return a stale `Some`.
            self.write_cached_agent_mode(owned_agent_id, AgentMode::Durable)
                .await;
            Some(AgentMode::Durable)
        } else if self
            .oplog_service
            .exists(owned_agent_id, AgentMode::Ephemeral)
            .await
        {
            Some(AgentMode::Ephemeral)
        } else {
            None
        }
    }

    async fn write_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, String> {
        record_worker_call("write_status");

        debug!("Writing cached agent status for {owned_agent_id} to {status_value:?}");

        self.write_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id),
            previous_status,
            status_value,
        )
        .await
    }

    async fn read_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Option<AgentStatusRecord> {
        record_worker_call("read_status_checkpoint");

        let mut status = self
            .read_split_status(
                owned_agent_id,
                Self::checkpoint_namespace(&owned_agent_id.agent_id),
            )
            .await?;
        // `agent_mode` is transient (not part of `core`); restore the authoritative value.
        status.agent_mode = agent_mode;
        Some(status)
    }

    async fn write_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        previous_checkpoint: Option<&AgentStatusRecord>,
        checkpoint: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, String> {
        record_worker_call("write_status_checkpoint");

        debug!(
            "Writing clean status checkpoint for {owned_agent_id} at oplog index {}",
            checkpoint.oplog_idx
        );

        self.write_split_status(
            owned_agent_id,
            Self::checkpoint_namespace(&owned_agent_id.agent_id),
            previous_checkpoint,
            checkpoint,
        )
        .await
    }

    async fn set_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        status_value: &AgentStatusRecord,
    ) {
        record_worker_call("set_assignment_tracking");

        // Ephemeral workers are never tracked for recovery (mirrors `write_cached_status`).
        if status_value.agent_mode == AgentMode::Ephemeral {
            return;
        }

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assignment is not ready");

        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);

        if Self::should_track_for_assignment_recovery(status_value) {
            debug!("Adding worker to the set of running workers in shard {shard_id}");

            self
                .key_value_storage
                .with_entity("worker", "add", "agent_id")
                .add_to_set(KeyValueStorageNamespace::RunningWorkers, &Self::running_in_shard_key(&shard_id), owned_agent_id)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to add worker to the set of running workers per shard ids on KV storage: {err}"
                    )
                });
        } else {
            debug!("Removing worker from the set of running workers in shard {shard_id}");

            self
                .key_value_storage
                .with_entity("worker", "remove", "agent_id")
                .remove_from_set(KeyValueStorageNamespace::RunningWorkers, &Self::running_in_shard_key(&shard_id), owned_agent_id)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to remove worker from the set of running worker ids per shard on KV storage: {err}"
                    )
                });
        }
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
mod tests {
    use super::*;
    use bytes::Bytes;
    use golem_common::model::Timestamp;
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::regions::{DeletedRegions, OplogRegion};
    use golem_common::model::{PendingInvocationRef, PendingUpdateKind, PendingUpdateRef};
    use std::collections::VecDeque;
    use test_r::test;

    fn idempotency_key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(value.to_string())
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

        // New status: a new invocation result + advanced marker, but identical regions/updates and
        // unchanged existing invocation results.
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
        assert!(written.contains(status_invocation_result_field(&idempotency_key("k3")).as_str()));
        // Unchanged parts are NOT re-sent.
        assert!(!written.contains(STATUS_REGIONS_FIELD));
        assert!(!written.contains(STATUS_UPDATES_FIELD));
        assert!(!written.contains(status_invocation_result_field(&idempotency_key("k1")).as_str()));
        assert!(dels.is_empty());

        let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
        assert_eq!(reassembled, new);
    }

    #[test]
    fn delta_deletes_removed_invocation_results() {
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

        assert_eq!(
            dels,
            vec![status_invocation_result_field(&idempotency_key("k2"))]
        );

        let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
        assert_eq!(reassembled, new);
    }

    #[test]
    fn cold_reconcile_deletes_stale_invocation_results() {
        // Store already holds ir:k1 and ir:k2 from a previous state.
        let existing_fields = vec![
            STATUS_CORE_FIELD.to_string(),
            status_invocation_result_field(&idempotency_key("k1")),
            status_invocation_result_field(&idempotency_key("k2")),
        ];

        // New status only has k1.
        let mut new = sample_status();
        new.invocation_results.remove(&idempotency_key("k2"));

        let mut core = new.clone();
        let parts = split_status(&mut core);
        let (_, dels) = compute_status_field_writes(None, &existing_fields, &core, &parts).unwrap();

        assert_eq!(
            dels,
            vec![status_invocation_result_field(&idempotency_key("k2"))]
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
    fn does_not_track_idle_workers_without_pending_work() {
        let status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };

        assert!(!DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }
}
