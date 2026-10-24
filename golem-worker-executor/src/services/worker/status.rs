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

use super::*;

/// Hash field holding the small part of the cached `AgentStatusRecord`. Always present for a cached
/// status; its absence is treated as a cache miss.
pub(super) const STATUS_CORE_FIELD: &str = "core";
/// Hash field holding the bounded invocation-result membership. Written only when it changes.
pub(super) const STATUS_MEMBERSHIP_FIELD: &str = "membership";
/// Hash field holding `(skipped_regions, deleted_regions)`. Written only when the regions change.
pub(super) const STATUS_REGIONS_FIELD: &str = "regions";
/// Hash field holding `(failed_updates, successful_updates)`. Written only when they change.
pub(super) const STATUS_UPDATES_FIELD: &str = "updates";
/// Prefix for per-transfer target receipt fields (`tr:{transfer_id}` -> receipt identity).
pub(super) const STATUS_RECEIVED_CARD_TRANSFER_PREFIX: &str = "tr:";
pub(super) fn status_received_card_transfer_field(transfer_id: &uuid::Uuid) -> String {
    format!("{STATUS_RECEIVED_CARD_TRANSFER_PREFIX}{transfer_id}")
}
/// Prefix for per-transfer source record fields (`ts:{transfer_id}` -> source records).
pub(super) const STATUS_SOURCE_CARD_TRANSFER_PREFIX: &str = "ts:";
pub(super) fn status_source_card_transfer_field(transfer_id: &uuid::Uuid) -> String {
    format!("{STATUS_SOURCE_CARD_TRANSFER_PREFIX}{transfer_id}")
}
/// Moves the separately persisted fields out of `status`, leaving the small `core` that is
/// serialized into the `core` field. Uses `mem::take`/`mem::replace`, so it does not clone the
/// potentially large updates, regions, or transfer index.
pub(super) fn split_status(status: &mut AgentStatusRecord) -> SplitStatusParts {
    SplitStatusParts {
        invocation_results: std::mem::replace(
            &mut status.invocation_results,
            InvocationResultMembership::new(0, 1, 1),
        ),
        received_card_transfers: std::mem::take(&mut status.received_card_transfers),
        source_card_transfers: std::mem::take(&mut status.source_card_transfers),
        failed_updates: std::mem::take(&mut status.failed_updates),
        successful_updates: std::mem::take(&mut status.successful_updates),
        skipped_regions: std::mem::replace(&mut status.skipped_regions, DeletedRegions::new()),
        deleted_regions: std::mem::replace(&mut status.deleted_regions, DeletedRegions::new()),
    }
}

pub(super) fn status_core(status: &AgentStatusRecord) -> AgentStatusRecord {
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
        source_card_transfers: SourceCardTransferIndex::default(),
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
/// `existing_split_fields` is used to delete stale `tr:` and `ts:` fields.
///
/// `core` is always part of `sets` (it carries the `oplog_idx` marker), so the marker and every
/// written part advance together in one atomic `set_many` by the caller.
pub(super) fn compute_status_field_writes(
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

    write_card_transfer_fields(
        previous.map(|previous| &previous.received_card_transfers),
        &parts.received_card_transfers,
        existing_split_fields,
        STATUS_RECEIVED_CARD_TRANSFER_PREFIX,
        status_received_card_transfer_field,
        &mut sets,
        &mut dels,
    )?;
    write_card_transfer_fields(
        previous.map(|previous| &previous.source_card_transfers),
        &parts.source_card_transfers,
        existing_split_fields,
        STATUS_SOURCE_CARD_TRANSFER_PREFIX,
        status_source_card_transfer_field,
        &mut sets,
        &mut dels,
    )?;

    Ok((sets, dels))
}

/// Adds the writes of the per-transfer fields of one transfer index: a delta against `previous`
/// when it is given, otherwise every transfer, and a delete of each stale `prefix` field.
fn write_card_transfer_fields<S: PartialEq + BinarySerializer>(
    previous: Option<&CardTransferIndex<S>>,
    transfers: &CardTransferIndex<S>,
    existing_split_fields: &[String],
    prefix: &str,
    field: fn(&uuid::Uuid) -> String,
    sets: &mut Vec<(String, Vec<u8>)>,
    dels: &mut Vec<String>,
) -> Result<(), String> {
    match previous {
        Some(previous) => {
            for (transfer_id, state) in transfers.changes_from(previous) {
                match state {
                    Some(state) => sets.push((field(&transfer_id), serialize(state)?)),
                    None => dels.push(field(&transfer_id)),
                }
            }
        }
        None => {
            let new_transfer_fields: HashSet<String> = transfers
                .iter()
                .map(|(transfer_id, _)| field(transfer_id))
                .collect();
            for (transfer_id, state) in transfers.iter() {
                sets.push((field(transfer_id), serialize(state)?));
            }

            for existing in existing_split_fields {
                if existing.starts_with(prefix) && !new_transfer_fields.contains(existing) {
                    dels.push(existing.clone());
                }
            }
        }
    }
    Ok(())
}

/// Reassembles a cached [`AgentStatusRecord`] from the split hash fields. Returns `None` if the
/// `core` field is missing (cache miss) or any field fails to deserialize in the current format
/// (treated as a cache miss). `agent_mode` is `#[transient]` and not part of `core`, so the
/// returned record carries the `Durable` deserialization default; callers restore it.
pub(super) fn reassemble_cached_status(
    fields: impl IntoIterator<Item = (String, bytes::Bytes)>,
) -> Option<AgentStatusRecord> {
    let mut core: Option<AgentStatusRecord> = None;
    let mut invocation_results: Option<InvocationResultMembership> = None;
    let mut regions: Option<(DeletedRegions, DeletedRegions)> = None;
    let mut updates: Option<(Vec<FailedUpdateRecord>, Vec<SuccessfulUpdateRecord>)> = None;
    let mut received_card_transfers = ReceivedCardTransferIndex::default();
    let mut source_card_transfers = SourceCardTransferIndex::default();

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
        } else if let Some(transfer_id) = name.strip_prefix(STATUS_SOURCE_CARD_TRANSFER_PREFIX) {
            let transfer_id = uuid::Uuid::parse_str(transfer_id).ok()?;
            let state = deserialize::<SourceCardTransferState>(&bytes).ok()?;
            source_card_transfers.insert(transfer_id, state);
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
    status.source_card_transfers = source_card_transfers;
    Some(status)
}

impl DefaultWorkerService {
    /// Namespace holding the agent's split cached status (one per-agent hash whose fields are
    /// `core`, `membership`, `regions`, `updates`, `tr:{transfer_id}`, and `ts:{transfer_id}`).
    pub(super) fn status_namespace(
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
    pub(super) fn checkpoint_namespace(
        agent_id: &AgentId,
        fingerprint: AgentFingerprint,
    ) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentStatusCheckpoint {
            agent_id: Arc::new(agent_id.clone()),
            fingerprint,
        }
    }

    /// Reads the cached `AgentStatusRecord` for `owned_agent_id`, if any, reassembling it from the
    /// split hash fields (`core`, `membership`, `regions`, `updates`, `tr:{transfer_id}`,
    /// `ts:{transfer_id}`). Returns `None` if the `core` field is missing (cache miss) or any
    /// field cannot be deserialized in the current format (treated as a cache miss).
    ///
    /// `agent_mode` is `#[transient]` and not part of `core`, so the returned record carries the
    /// `Durable` deserialization default; callers must restore it from the authoritative source.
    pub(super) async fn read_cached_status(
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
    /// from the `core` / `membership` / `regions` / `updates` / `tr:{transfer_id}` /
    /// `ts:{transfer_id}` fields. Returns `None` if `core` is missing (cache miss / torn write) or
    /// any field cannot be deserialized in the current format.
    ///
    /// `agent_mode` is `#[transient]` and not part of `core`, so the returned record carries the
    /// `Durable` deserialization default; callers must restore it from the authoritative source.
    pub(super) async fn read_split_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError> {
        // Single atomic read of every field of the per-agent status hash (`core`, `membership`,
        // `regions`, `updates`, `tr:{transfer_id}`, `ts:{transfer_id}`). This is one round-trip
        // (Redis `HGETALL`, a single `SELECT ... WHERE namespace`, or one locked scan in memory)
        // that observes a consistent snapshot, so it cannot reassemble a torn, mixed-generation
        // record. (A naive `keys` + `get_many` would be two round-trips, leaving a window where a
        // concurrent writer — the background status flusher for the live cache, or the
        // clean-checkpoint writer for the checkpoint namespace — could add a new split field that
        // the earlier `keys` did not list, yielding a record at the newer `core.oplog_idx` missing
        // derived status.)
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
    /// and received and source card transfers are written per key (only newly added/changed keys).
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
    pub(super) async fn write_split_status(
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
        reassembled.source_card_transfers = parts.source_card_transfers;
        Ok(reassembled)
    }

    /// Deletes every field of a split status hash, guarded by the observed core so a concurrent
    /// complete publication is either removed atomically or forces a retry.
    pub(super) async fn remove_split_status(
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
}
