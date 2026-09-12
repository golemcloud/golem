use crate::services::oplog::OplogServiceOps;
use crate::services::{HasComponentService, HasConfig, HasOplogService, HasWorkerService};
use golem_common::base_model::OplogIndex;
use golem_common::base_model::durable_stream::StreamSessionRecordV1;
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::AgentInvocationPayload;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    AgentError, AgentResourceId, OplogEntry, OplogPayload, QueuedCardEvent, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    AgentResourceDescription, AgentStatus, AgentStatusRecord, DurableStreamSessionIndex,
    FailedUpdateRecord, IdempotencyKey, InvocationResultMembership, OplogProcessorCheckpointState,
    OwnedAgentId, PendingCardEventRef, PendingInvocationRef, PendingUpdateKind, PendingUpdateRef,
    ReceivedCardTransferIndex, ReceivedCardTransferState, RetryConfig, RetryPolicyState,
    SuccessfulUpdateRecord, Timestamp,
};
use golem_common::serialization::deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Like calculate_last_known_status, but assumes that the oplog exists and has at least a Create entry in it.
pub async fn calculate_last_known_status_for_existing_worker<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
) -> Result<AgentStatusRecord, String>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    calculate_last_known_status(this, owned_agent_id, agent_mode, last_known)
        .await?
        .ok_or_else(|| "failed to calculate status for existing worker".into())
}

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker
/// status, falling back to a full recompute from the start of the oplog when the cached baseline can
/// no longer be folded forward (e.g. a jump deleted its index).
///
/// This is the no-checkpoint variant; callers that have a [`WorkerService`] should prefer
/// [`calculate_last_known_status_with_checkpoint`] so a jump-induced full recompute can instead fold
/// forward from the clean status checkpoint.
///
/// [`WorkerService`]: crate::services::worker::WorkerService
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
) -> Result<Option<AgentStatusRecord>, String>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    calculate_last_known_status_with_checkpoint_reader(
        this,
        owned_agent_id,
        agent_mode,
        last_known,
        || async { None },
    )
    .await
}

/// Calculates the latest worker status, folding forward from the freshest usable baseline and, on a
/// fold failure, from the *clean* status checkpoint read from `this`'s [`WorkerService`].
///
/// This is the variant production callers should use: the checkpoint is read from
/// `this.worker_service()`, so callers no longer pass a checkpoint-read closure. The read still
/// happens lazily (only when the live cache baseline cannot be folded), so the common path pays no
/// extra read. See [`calculate_last_known_status_with_checkpoint_reader`] for the baseline order.
///
/// [`WorkerService`]: crate::services::worker::WorkerService
pub async fn calculate_last_known_status_with_checkpoint<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
) -> Result<Option<AgentStatusRecord>, String>
where
    T: HasOplogService + HasConfig + HasComponentService + HasWorkerService + Sync,
{
    let worker_service = this.worker_service();
    calculate_last_known_status_with_checkpoint_reader(
        this,
        owned_agent_id,
        agent_mode,
        last_known,
        || async move {
            // The checkpoint is only a fold baseline, and this function has no way to report a
            // read failure: falling back to a full recompute costs time, not correctness.
            worker_service
                .read_status_checkpoint(owned_agent_id, agent_mode)
                .await
                .unwrap_or_else(|err| {
                    tracing::error!(
                        "Failed to read the status checkpoint for {owned_agent_id}: {err}"
                    );
                    None
                })
        },
    )
    .await
}

/// Calculates the latest worker status, trying baselines in order of decreasing freshness:
///
/// 1. the live cached status (`last_known`), folded forward — the common cold-load path;
/// 2. on a fold failure (e.g. a jump deleted the cached index), the *clean* status checkpoint
///    (read lazily via `read_checkpoint`), folded forward — this predates any later jump region and
///    avoids re-reading the whole oplog;
/// 3. a full recompute from the start of the oplog.
///
/// The checkpoint read happens only when the live cache baseline is absent or its fold is
/// impossible, so the common path pays no extra read.
///
/// Most callers should use [`calculate_last_known_status_with_checkpoint`], which sources the
/// checkpoint from `this.worker_service()`. This lower-level variant takes an explicit
/// `read_checkpoint` closure for the few callers that cannot satisfy [`HasWorkerService`] for `this`
/// — notably the `DefaultWorkerService` cold path, which reaches its own `read_status_checkpoint`
/// through `&self` without an owning `Arc`.
pub async fn calculate_last_known_status_with_checkpoint_reader<T, Fut>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
    read_checkpoint: impl FnOnce() -> Fut,
) -> Result<Option<AgentStatusRecord>, String>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
    Fut: std::future::Future<Output = Option<AgentStatusRecord>>,
{
    // 1. Try folding forward from the live cached status.
    if let Some(last_known) = last_known
        && let Some(status) =
            try_fold_status_from(this, owned_agent_id, agent_mode, last_known).await?
    {
        crate::metrics::workers::record_agent_status_recompute("cache");
        return Ok(Some(status));
    }

    // 2. Live cache baseline missing or its fold was impossible (e.g. a jump deleted the cached
    //    index, or a revert moved the oplog behind it): try folding from the clean checkpoint.
    if let Some(checkpoint) = read_checkpoint().await
        && let Some(status) =
            try_fold_status_from(this, owned_agent_id, agent_mode, checkpoint).await?
    {
        crate::metrics::workers::record_agent_status_recompute("checkpoint");
        return Ok(Some(status));
    }

    // 3. Fall back to a full recompute from the start of the oplog.
    let status = try_fold_status_from(
        this,
        owned_agent_id,
        agent_mode,
        AgentStatusRecord::default(),
    )
    .await?;
    if status.is_some() {
        crate::metrics::workers::record_agent_status_recompute("full");
    }
    Ok(status)
}

/// Folds the oplog entries after `baseline.oplog_idx` onto `baseline`.
///
/// Returns `None` when the fold cannot produce a correct result from `baseline` alone:
/// - the oplog does not exist (no `Create` entry);
/// - `baseline` is ahead of the current last oplog index (e.g. a revert truncated the oplog below a
///   stale checkpoint), so there is nothing to fold forward;
/// - a newly added skipped/deleted region covers `baseline.oplog_idx` (see
///   [`update_status_with_new_entries`]), making the baseline unusable.
///
/// On `None` the caller should retry from an earlier baseline (a checkpoint, or the start of the
/// oplog). A `baseline` of [`AgentStatusRecord::default`] (oplog index 0) folds the whole oplog and
/// never hits the region case, so it always succeeds when the oplog exists.
pub async fn try_fold_status_from<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    mut baseline: AgentStatusRecord,
) -> Result<Option<AgentStatusRecord>, String>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    let full_rebuild = baseline.oplog_idx == OplogIndex::NONE;
    if full_rebuild && baseline.invocation_results.is_empty() {
        baseline.invocation_results = this.config().invocation_results.membership();
    }

    let last_oplog_index = this
        .oplog_service()
        .get_last_index(owned_agent_id, agent_mode)
        .await;

    if last_oplog_index == OplogIndex::NONE {
        // Worker status can only be recovered if we have at least the Create oplog entry, otherwise
        // we cannot recover information like the component version.
        return Ok(None);
    }

    if last_oplog_index < baseline.oplog_idx {
        // The baseline is ahead of the oplog (e.g. a revert truncated it below a stale checkpoint);
        // we cannot fold forward, so the caller must retry from an earlier baseline.
        return Ok(None);
    }

    if baseline.oplog_idx == last_oplog_index {
        return Ok(Some(baseline));
    }

    let chunk_size = this
        .config()
        .invocation_results
        .physical_index_catch_up_chunk_size
        .max(1);

    if full_rebuild {
        return fold_status_with_precomputed_regions(
            this,
            owned_agent_id,
            agent_mode,
            baseline,
            last_oplog_index,
            chunk_size,
        )
        .await;
    }

    let original_baseline = baseline.clone();
    let mut first = baseline.oplog_idx.next();
    while first <= last_oplog_index {
        let count = (last_oplog_index.as_u64() - first.as_u64() + 1).min(chunk_size);
        let mut entries = this
            .oplog_service()
            .read_exact(owned_agent_id, agent_mode, first, count)
            .await;
        if entries.is_empty() {
            return Ok(None);
        }
        hydrate_stream_session_payloads(this, owned_agent_id, agent_mode, &mut entries).await?;
        hydrate_initial_pending_evidence(this, owned_agent_id, agent_mode, &mut baseline, &entries)
            .await?;
        let finalize_oplog_processor_checkpoints =
            entries.keys().next_back() == Some(&last_oplog_index);
        baseline = match update_status_with_new_entries_internal(
            agent_mode,
            baseline,
            entries,
            &this.config().retry,
            true,
            finalize_oplog_processor_checkpoints,
        )? {
            Some(status) => status,
            None => {
                return fold_status_with_precomputed_regions(
                    this,
                    owned_agent_id,
                    agent_mode,
                    original_baseline,
                    last_oplog_index,
                    chunk_size,
                )
                .await;
            }
        };
        first = baseline.oplog_idx.next();
    }
    Ok(Some(baseline))
}

async fn fold_status_with_precomputed_regions<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    mut baseline: AgentStatusRecord,
    last_oplog_index: OplogIndex,
    chunk_size: u64,
) -> Result<Option<AgentStatusRecord>, String>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    let start = baseline.oplog_idx.next();
    let mut deleted_regions = baseline.deleted_regions.clone();
    let mut region_entries = BTreeMap::new();
    let mut first = start;
    while first <= last_oplog_index {
        let count = (last_oplog_index.as_u64() - first.as_u64() + 1).min(chunk_size);
        let entries = this
            .oplog_service()
            .read_exact(owned_agent_id, agent_mode, first, count)
            .await;
        if entries.is_empty() {
            return Ok(None);
        }
        deleted_regions = calculate_deleted_regions(deleted_regions, &entries);
        region_entries.extend(entries.iter().filter_map(|(index, entry)| {
            let relevant = matches!(
                entry,
                OplogEntry::Jump { .. }
                    | OplogEntry::Revert { .. }
                    | OplogEntry::PendingUpdate {
                        description: UpdateDescription::SnapshotBased { .. },
                        ..
                    }
                    | OplogEntry::SuccessfulUpdate { .. }
                    | OplogEntry::FailedUpdate { .. }
            );
            relevant.then(|| (*index, entry.clone()))
        }));
        first = entries.keys().max().unwrap().next();
    }

    let skipped_regions = calculate_skipped_regions(
        baseline.skipped_regions.clone(),
        &deleted_regions,
        &region_entries,
    );

    if baseline_is_invalidated(&baseline, &skipped_regions) {
        return Ok(None);
    }
    baseline.deleted_regions = deleted_regions;
    baseline.skipped_regions = skipped_regions;

    first = start;
    while first <= last_oplog_index {
        let count = (last_oplog_index.as_u64() - first.as_u64() + 1).min(chunk_size);
        let mut entries = this
            .oplog_service()
            .read_exact(owned_agent_id, agent_mode, first, count)
            .await;
        if entries.is_empty() {
            return Ok(None);
        }
        hydrate_stream_session_payloads(this, owned_agent_id, agent_mode, &mut entries).await?;
        hydrate_initial_pending_evidence(this, owned_agent_id, agent_mode, &mut baseline, &entries)
            .await?;
        let finalize_oplog_processor_checkpoints =
            entries.keys().next_back() == Some(&last_oplog_index);
        let deleted_regions = baseline.deleted_regions.clone();
        let skipped_regions = baseline.skipped_regions.clone();
        baseline = update_status_with_precomputed_regions(
            agent_mode,
            baseline,
            entries,
            &this.config().retry,
            deleted_regions,
            skipped_regions,
            finalize_oplog_processor_checkpoints,
        )?;
        first = baseline.oplog_idx.next();
    }
    Ok(Some(baseline))
}

async fn hydrate_stream_session_payloads<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    entries: &mut BTreeMap<OplogIndex, OplogEntry>,
) -> Result<(), String>
where
    T: HasOplogService + Sync,
{
    for entry in entries.values_mut() {
        if let OplogEntry::StreamSession { record, .. } = entry {
            let decoded = this
                .oplog_service()
                .download_payload(owned_agent_id, agent_mode, record.clone())
                .await
                .map_err(|error| {
                    format!("failed to load durable stream session payload: {error}")
                })?;
            *record = OplogPayload::Inline(Box::new(decoded));
        }
    }
    Ok(())
}

async fn hydrate_initial_pending_evidence<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    baseline: &mut AgentStatusRecord,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<(), String>
where
    T: HasOplogService + Sync,
{
    for (attached_idx, entry) in entries {
        let OplogEntry::StreamSession {
            record: OplogPayload::Inline(record),
            ..
        } = entry
        else {
            continue;
        };
        let StreamSessionRecordV1::Attached(attached) = record.as_ref() else {
            continue;
        };
        let Some(status) = baseline
            .durable_stream_sessions
            .get(&attached.session_key.idempotency_key)
            .cloned()
        else {
            continue;
        };
        if status.lifecycle_error.is_some() || status.validated_initial_pending_invocation.is_some()
        {
            continue;
        }
        let mut status = status;
        if !status.validate_initial_attachment_reference(*attached_idx, attached) {
            baseline
                .durable_stream_sessions
                .insert(attached.session_key.idempotency_key.clone(), status);
            continue;
        }
        let persisted_referent;
        let referent = if let Some(entry) = entries.get(&attached.pending_invocation_oplog_index) {
            Some(entry)
        } else {
            persisted_referent = this
                .oplog_service()
                .read_exact(
                    owned_agent_id,
                    agent_mode,
                    attached.pending_invocation_oplog_index,
                    1,
                )
                .await;
            persisted_referent.get(&attached.pending_invocation_oplog_index)
        };
        match referent {
            Some(OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            }) => {
                status.apply_pending_invocation(
                    attached.pending_invocation_oplog_index,
                    idempotency_key,
                );
                baseline
                    .durable_stream_sessions
                    .insert(attached.session_key.idempotency_key.clone(), status);
            }
            _ => {
                status.lifecycle_error = Some(
                    "durable Attached record references a missing or invalid pending invocation"
                        .into(),
                );
                baseline
                    .durable_stream_sessions
                    .insert(attached.session_key.idempotency_key.clone(), status);
            }
        }
    }
    Ok(())
}

// update a worker status with new entries. Returns None if the status cannot be calculated from the new entries alone and needs to be recalculated from the beginning.
pub fn update_status_with_new_entries(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    // TODO: changing the retry policy will cause inconsistencies when reading existing oplogs.
    default_retry_policy: &RetryConfig,
) -> Result<Option<AgentStatusRecord>, String> {
    update_status_with_new_entries_internal(
        agent_mode,
        last_known,
        new_entries,
        default_retry_policy,
        true,
        true,
    )
}

fn update_status_with_new_entries_internal(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    default_retry_policy: &RetryConfig,
    validate_baseline: bool,
    finalize_oplog_processor_checkpoints: bool,
) -> Result<Option<AgentStatusRecord>, String> {
    let deleted_regions =
        calculate_deleted_regions(last_known.deleted_regions.clone(), &new_entries);

    let skipped_regions = calculate_skipped_regions(
        last_known.skipped_regions.clone(),
        &deleted_regions,
        &new_entries,
    );

    // If the last known status is from a deleted region based on the latest deleted region status,
    // we cannot fold the new status from the new entries only, and need to recalculate the whole status
    // (Note that this is a rare case - for Jumps, this is not happening if the executor successfully writes out
    // the new status before performing the jump; for Reverts, the status is recalculated anyway, but only once, when
    // the revert is applied)
    if validate_baseline && baseline_is_invalidated(&last_known, &skipped_regions) {
        return Ok(None);
    }

    Ok(Some(update_status_with_precomputed_regions(
        agent_mode,
        last_known,
        new_entries,
        default_retry_policy,
        deleted_regions,
        skipped_regions,
        finalize_oplog_processor_checkpoints,
    )?))
}

fn baseline_is_invalidated(baseline: &AgentStatusRecord, skipped_regions: &DeletedRegions) -> bool {
    if !skipped_regions.is_in_deleted_region(baseline.oplog_idx) {
        return false;
    }

    let baseline_without_overrides = if baseline.skipped_regions.is_overridden() {
        let mut cloned = baseline.skipped_regions.clone();
        cloned.merge_override();
        cloned
    } else {
        baseline.skipped_regions.clone()
    };
    let new_without_overrides = if skipped_regions.is_overridden() {
        let mut cloned = skipped_regions.clone();
        cloned.merge_override();
        cloned
    } else {
        skipped_regions.clone()
    };
    new_without_overrides != baseline_without_overrides
}

fn update_status_with_precomputed_regions(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    default_retry_policy: &RetryConfig,
    deleted_regions: DeletedRegions,
    skipped_regions: DeletedRegions,
    finalize_oplog_processor_checkpoints: bool,
) -> Result<AgentStatusRecord, String> {
    let active_plugins = last_known.active_plugins.clone();

    let (status, current_retry_state, overridden_retry_config) = calculate_latest_worker_status(
        last_known.status,
        last_known.current_retry_state,
        last_known.overridden_retry_config,
        default_retry_policy,
        &skipped_regions,
        &deleted_regions,
        &new_entries,
    );

    let pending_invocations =
        calculate_pending_invocations(last_known.pending_invocations, &new_entries);
    let pending_card_events =
        calculate_pending_card_events(last_known.pending_card_events, &new_entries);
    let received_card_transfers =
        calculate_received_card_transfers(last_known.received_card_transfers, &new_entries);
    let durable_stream_sessions =
        calculate_durable_stream_sessions(last_known.durable_stream_sessions, &new_entries)?;
    let mut pending_durable_stream_cancellations = last_known.pending_durable_stream_cancellations;
    for entry in new_entries.values() {
        if let OplogEntry::StreamSession {
            record: OplogPayload::Inline(record),
            ..
        } = entry
        {
            match record.as_ref() {
                StreamSessionRecordV1::ConsumerCancelIntent(intent) => {
                    if !pending_durable_stream_cancellations.iter().any(|existing| {
                        existing.session_key == intent.session_key
                            && existing.stream_id == intent.stream_id
                    }) {
                        pending_durable_stream_cancellations.insert(intent.clone());
                    }
                }
                StreamSessionRecordV1::ConsumerCancelApplied(receipt) => {
                    pending_durable_stream_cancellations.remove(&receipt.intent);
                }
                _ => {}
            }
        }
    }
    let has_durable_stream_history = last_known.has_durable_stream_history
        || new_entries.values().any(|entry| {
            matches!(
                entry,
                OplogEntry::StreamRegistered { .. }
                    | OplogEntry::StreamItems { .. }
                    | OplogEntry::StreamEnd { .. }
                    | OplogEntry::StreamCancel { .. }
                    | OplogEntry::StreamSession { .. }
            )
        });
    let (
        pending_updates,
        failed_updates,
        successful_updates,
        component_revision,
        component_size,
        component_revision_for_replay,
        last_manual_update_snapshot_index,
        last_automatic_snapshot_index,
        last_automatic_snapshot_timestamp,
        last_automatic_snapshot_component_revision,
    ) = calculate_update_fields(
        last_known.pending_updates,
        last_known.failed_updates,
        last_known.successful_updates,
        last_known.component_revision,
        last_known.component_size,
        last_known.component_revision_for_replay,
        last_known.last_manual_update_snapshot_index,
        last_known.last_automatic_snapshot_index,
        last_known.last_automatic_snapshot_timestamp,
        last_known.last_automatic_snapshot_component_revision,
        &deleted_regions,
        &new_entries,
    );

    let (invocation_results, current_idempotency_key, cancelled_idempotency_key) =
        calculate_invocation_results(
            last_known.invocation_results,
            last_known.current_idempotency_key,
            last_known.cancelled_idempotency_key,
            &deleted_regions,
            &new_entries,
        );

    let total_linear_memory_size = calculate_total_linear_memory_size(
        last_known.total_linear_memory_size,
        &skipped_regions,
        &new_entries,
    );

    let owned_resources =
        collect_resources(last_known.owned_resources, &skipped_regions, &new_entries);

    let active_plugins = calculate_active_plugins(active_plugins, &deleted_regions, &new_entries);

    let revoked_cards = calculate_revoked_cards(last_known.revoked_cards, &new_entries);

    let oplog_processor_checkpoints = calculate_oplog_processor_checkpoints(
        last_known.oplog_processor_checkpoints,
        &active_plugins,
        &deleted_regions,
        &new_entries,
        finalize_oplog_processor_checkpoints,
    );

    Ok(AgentStatusRecord {
        oplog_idx: new_entries
            .keys()
            .max()
            .cloned()
            .unwrap_or(last_known.oplog_idx),
        status,
        overridden_retry_config,
        pending_invocations,
        pending_card_events,
        skipped_regions,
        pending_updates,
        failed_updates,
        successful_updates,
        invocation_results,
        received_card_transfers,
        durable_stream_sessions,
        has_durable_stream_history,
        pending_durable_stream_cancellations,
        current_idempotency_key,
        cancelled_idempotency_key,
        component_revision,
        component_size,
        owned_resources,
        total_linear_memory_size,
        active_plugins,
        oplog_processor_checkpoints,
        revoked_cards,
        deleted_regions,
        component_revision_for_replay,
        current_retry_state,
        last_manual_update_snapshot_index,
        last_automatic_snapshot_index,
        last_automatic_snapshot_timestamp,
        last_automatic_snapshot_component_revision,
        agent_mode,
    })
}

fn calculate_latest_worker_status(
    mut current_status: AgentStatus,
    mut current_retry_state: HashMap<OplogIndex, RetryPolicyState>,
    current_retry_policy: Option<RetryConfig>,
    default_retry_policy: &RetryConfig,
    skipped_regions: &DeletedRegions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    AgentStatus,
    HashMap<OplogIndex, RetryPolicyState>,
    Option<RetryConfig>,
) {
    for (idx, entry) in entries {
        // Errors are counted in skipped regions too (but not in deleted ones),
        // otherwise we would not be able to know how many times we retried failures in atomic regions.
        // This must happen before the skipped-region continue below.
        if !deleted_regions.is_in_deleted_region(*idx)
            && let OplogEntry::Error {
                retry_from,
                retry_policy_state: Some(state),
                ..
            } = entry
        {
            current_retry_state.insert(*retry_from, state.clone());
        }

        // Skipping entries in skipped regions, as they are skipped during replay too
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        // For non-skipped errors, update the worker status based on the accumulated retry count
        if !deleted_regions.is_in_deleted_region(*idx)
            && let OplogEntry::Error {
                error,
                retry_from,
                inside_atomic_region,
                retry_policy_state,
                ..
            } = entry
        {
            if matches!(error, AgentError::PermissionDenied(_)) {
                current_status = AgentStatus::Idle;
                current_retry_state.clear();
            } else {
                let count = current_retry_state
                    .get(retry_from)
                    .map(|s| s.retry_count())
                    .unwrap_or_default();
                if is_worker_error_retriable(
                    current_retry_policy
                        .as_ref()
                        .unwrap_or(default_retry_policy),
                    error,
                    count,
                    *inside_atomic_region,
                    retry_policy_state.as_ref(),
                ) {
                    current_status = AgentStatus::Retrying;
                } else {
                    current_status = AgentStatus::Failed;
                }
            }
        }

        match entry {
            OplogEntry::Create { .. } => {
                current_status = AgentStatus::Idle;
            }
            OplogEntry::Start { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::End { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Cancelled { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::CompletionDiscarded { .. } | OplogEntry::CompletionDelivered { .. } => {}
            OplogEntry::AgentInvocationStarted { .. } => {
                current_status = AgentStatus::Running;
                current_retry_state.clear();
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                current_status = AgentStatus::Idle;
                current_retry_state.clear();
            }
            OplogEntry::Suspend { .. } => {
                current_status = AgentStatus::Suspended;
            }
            OplogEntry::NoOp { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Jump { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Interrupted { .. } => {
                current_status = AgentStatus::Interrupted;
            }
            OplogEntry::Exited { .. } => {
                current_status = AgentStatus::Exited;
            }
            OplogEntry::SetRetryPolicy { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::RemoveRetryPolicy { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::BeginAtomicRegion { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::EndAtomicRegion { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::PendingAgentInvocation { .. } => {}
            OplogEntry::PendingUpdate { .. } => {
                if current_status == AgentStatus::Failed {
                    current_status = AgentStatus::Retrying;
                }
            }
            OplogEntry::FailedUpdate { .. } => {}
            OplogEntry::SuccessfulUpdate { .. } => {}
            OplogEntry::GrowMemory { .. } => {}
            OplogEntry::CreateResource { .. } => {}
            OplogEntry::DropResource { .. } => {}
            OplogEntry::Log { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Restart { .. } => {
                current_status = AgentStatus::Idle;
            }
            OplogEntry::ActivatePlugin { .. } => {}
            OplogEntry::DeactivatePlugin { .. } => {}
            OplogEntry::Revert { .. } => {}
            OplogEntry::CancelPendingInvocation { .. } => {}
            OplogEntry::StartSpan { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::FinishSpan { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::SetSpanAttribute { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::BeginRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::PreCommitRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::PreRollbackRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::CommittedRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::RolledBackRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Snapshot { .. } => {}
            OplogEntry::OplogProcessorCheckpoint { .. } => {}
            OplogEntry::CardEventQueued { .. } => {}
            OplogEntry::CardInstalled { .. } => {}
            OplogEntry::CardInstallFailed { .. } => {}
            OplogEntry::CardDerived { .. } => {}
            OplogEntry::CardTransferStarted { .. } => {}
            OplogEntry::CardTransferred { .. } => {}
            OplogEntry::CardRevokedCascade { .. } => {}
            OplogEntry::CardTransferConfirmed { .. } => {}
            OplogEntry::CardRevoked { .. } => {}
            OplogEntry::CardExpired { .. } => {}
            OplogEntry::HostStreamFrame { .. } => {}
            OplogEntry::StreamRegistered { .. }
            | OplogEntry::StreamItems { .. }
            | OplogEntry::StreamEnd { .. }
            | OplogEntry::StreamCancel { .. }
            | OplogEntry::StreamSession { .. } => {}
            OplogEntry::Error { .. } => {
                // .. handled separately
            }
        }
    }
    (current_status, current_retry_state, current_retry_policy)
}

fn calculate_revoked_cards(
    mut revoked_cards: HashSet<golem_common::model::card::CardId>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<golem_common::model::card::CardId> {
    for entry in entries.values() {
        match entry {
            OplogEntry::CardRevoked { card_id, .. } => {
                revoked_cards.insert(*card_id);
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids, ..
            } => revoked_cards.extend(revoked_card_ids.iter().copied()),
            OplogEntry::CardInstalled { card, .. } => {
                revoked_cards.remove(&card.card_id());
            }
            _ => {}
        }
    }

    revoked_cards
}

fn calculate_deleted_regions(
    initial_deleted: DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> DeletedRegions {
    let mut deleted_builder = DeletedRegionsBuilder::from_regions(initial_deleted.into_regions());
    for entry in entries.values() {
        if let OplogEntry::Revert { dropped_region, .. } = entry {
            deleted_builder.add(dropped_region.clone());
        }
    }
    deleted_builder.build()
}

fn calculate_skipped_regions(
    initial_skipped: DeletedRegions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> DeletedRegions {
    let mut skipped_without_override = initial_skipped.clone();
    if skipped_without_override.is_overridden() {
        skipped_without_override.drop_override();
    }

    let mut skipped_override = initial_skipped.get_override();

    let mut skipped_builder =
        DeletedRegionsBuilder::from_regions(skipped_without_override.into_regions());
    for (idx, entry) in entries {
        // Skipping deleted regions (by revert) from constructing the skipped regions
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::Jump { jump, .. } => {
                skipped_builder.add(jump.clone());
            }
            OplogEntry::Revert { dropped_region, .. } => {
                skipped_builder.add(dropped_region.clone());
            }
            OplogEntry::PendingUpdate {
                description: UpdateDescription::SnapshotBased { .. },
                ..
            } => {
                skipped_override = Some(
                    DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                        OplogIndex::INITIAL.next()..=*idx,
                    )])
                    .build(),
                )
            }
            OplogEntry::SuccessfulUpdate { .. } => {
                if let Some(ovrd) = skipped_override {
                    for region in ovrd.into_regions() {
                        skipped_builder.add(region);
                    }
                    skipped_override = None;
                }
            }
            OplogEntry::FailedUpdate { .. } => {
                skipped_override = None;
            }
            _ => {}
        }
    }

    for deleted_region in deleted_regions.regions() {
        skipped_builder.add(deleted_region.clone());
    }

    let mut new_skipped = skipped_builder.build();
    if let Some(ovrd) = skipped_override {
        new_skipped.set_override(ovrd);
    }

    new_skipped
}

/// Determines whether a pending agent invocation payload is a manual update and, if so, returns
/// its target revision.
///
/// Manual update payloads are tiny and always stored inline, so this never needs to download an
/// external payload: an `External` payload is by definition not a manual update.
fn manual_update_target_revision_of(
    payload: &OplogPayload<AgentInvocationPayload>,
) -> Option<ComponentRevision> {
    fn target_revision(payload: &AgentInvocationPayload) -> Option<ComponentRevision> {
        match payload {
            AgentInvocationPayload::ManualUpdate { target_revision } => Some(*target_revision),
            _ => None,
        }
    }

    match payload {
        OplogPayload::Inline(p) => target_revision(p),
        OplogPayload::SerializedInline {
            cached: Some(v), ..
        } => target_revision(v),
        OplogPayload::SerializedInline { bytes, .. } => {
            deserialize::<AgentInvocationPayload>(bytes)
                .map_err(|e| {
                    tracing::warn!("Failed to deserialize pending agent invocation payload: {e}");
                    e
                })
                .ok()
                .as_ref()
                .and_then(target_revision)
        }
        OplogPayload::External {
            cached: Some(v), ..
        } => target_revision(v),
        OplogPayload::External { .. } => None,
    }
}

fn calculate_pending_invocations(
    initial: Vec<PendingInvocationRef>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<PendingInvocationRef> {
    let mut result = initial;
    for (oplog_idx, entry) in entries {
        // Here we are handling two categories of oplog entries:
        // - "input" entries adding items to pending queues (PendingAgentInvocation, PendingUpdate)
        // - "output" entries removing items from pending queues when they got processed (AgentInvocationStarted, SuccessfulUpdate, FailedUpdate)
        //
        // Skipped regions does not matter for us - they are representing jumps and updates, and anything that happens in these regions
        // is part of the history and we take it into accout (for example a new pending invocation comes in the previous iteration of a retried
        // transaction, etc).
        //
        // Deleted regions are created by reverting some oplog entries; Even then, we still want to take both the input and output
        // entries into account in deleted regions in the following way:
        // - Incoming pending invocation or update that has not been processed yet is NOT affected by revert - they remain pending
        // - If a pending invocation or update was attempted (no matter if succeeded or not) in the reverted region, we remove it from
        //   the pending queue, so the revert will not make them retried.
        //
        // We only store a lightweight reference to the originating oplog entry; the full invocation
        // payload (input parameters, snapshot data, oplog entry batches, ...) stays in the oplog and
        // is hydrated on demand by the paths that actually execute the invocation.

        match entry {
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
                ..
            } => {
                // A manual update is the only invocation variant without a semantic idempotency
                // key, so we capture its target revision and drop the (freshly generated, unused)
                // idempotency key for it.
                let manual_update_target_revision = manual_update_target_revision_of(payload);
                let idempotency_key = if manual_update_target_revision.is_some() {
                    None
                } else {
                    Some(idempotency_key.clone())
                };
                result.push(PendingInvocationRef {
                    timestamp: *timestamp,
                    oplog_index: *oplog_idx,
                    idempotency_key,
                    manual_update_target_revision,
                });
            }
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                result.retain(|invocation| !invocation.has_idempotency_key(idempotency_key));
            }
            OplogEntry::PendingUpdate {
                description:
                    UpdateDescription::SnapshotBased {
                        target_revision, ..
                    },
                ..
            } => result.retain(|invocation| {
                invocation.manual_update_target_revision.as_ref() != Some(target_revision)
            }),
            OplogEntry::FailedUpdate {
                target_revision, ..
            } => result.retain(|invocation| {
                invocation.manual_update_target_revision.as_ref() != Some(target_revision)
            }),
            OplogEntry::CancelPendingInvocation {
                idempotency_key, ..
            } => {
                result.retain(|invocation| !invocation.has_idempotency_key(idempotency_key));
            }
            _ => {}
        }
    }
    result
}

pub(crate) fn calculate_pending_card_events(
    initial: Vec<PendingCardEventRef>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<PendingCardEventRef> {
    let mut result = initial;

    for (oplog_idx, entry) in entries {
        match entry {
            OplogEntry::CardEventQueued {
                timestamp,
                entity_parent_start_index,
                event,
            } => {
                result.push(PendingCardEventRef {
                    timestamp: *timestamp,
                    oplog_index: *oplog_idx,
                    entity_parent_start_index: *entity_parent_start_index,
                    event: event.clone(),
                });
            }
            OplogEntry::CardInstalled {
                queued_event_index: Some(queued_event_index),
                ..
            } => {
                result.retain(|event| {
                    event.oplog_index != *queued_event_index
                        || !matches!(event.event, QueuedCardEvent::Install(_))
                });
            }
            OplogEntry::CardInstallFailed {
                queued_event_index, ..
            } => {
                result.retain(|event| {
                    event.oplog_index != *queued_event_index
                        || !matches!(
                            event.event,
                            QueuedCardEvent::Install(_) | QueuedCardEvent::TransferReceived(_)
                        )
                });
            }
            OplogEntry::CardRevoked {
                queued_event_index, ..
            } => {
                result.retain(|event| {
                    event.oplog_index != *queued_event_index
                        || !matches!(event.event, QueuedCardEvent::Revoke(_))
                });
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids, ..
            } => {
                result.retain(|event| {
                    !matches!(
                        &event.event,
                        QueuedCardEvent::Revoke(revoke)
                            if revoked_card_ids.contains(&revoke.card_id)
                    )
                });
            }
            OplogEntry::CardTransferConfirmed {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => {
                result.retain(|event| {
                    !matches!(
                        &event.event,
                        QueuedCardEvent::TransferStarted(transfer)
                            if transfer.transfer_id == *transfer_id
                                && transfer.card_id == *source_card_id
                                && transfer.card.as_ref().is_some_and(|card| card.card_id() == *installed_card_id)
                                && transfer.target_holder == *target_holder
                    )
                });
            }
            OplogEntry::CardTransferred {
                transfer_id,
                source_card_id,
                installed_card_id,
                card,
                ..
            } => {
                result.retain(|event| {
                    !matches!(
                        &event.event,
                        QueuedCardEvent::TransferReceived(receipt)
                            if receipt.transfer_id == *transfer_id
                                && receipt.source_card_id.is_none_or(|receipt_source_card_id| {
                                    source_card_id.is_none_or(|source_card_id| {
                                        receipt_source_card_id == source_card_id
                                    })
                                })
                                && receipt.card_id == *installed_card_id
                                && receipt.card.as_ref() == Some(card)
                    )
                });
            }
            _ => {}
        }
    }

    result
}

fn calculate_received_card_transfers(
    mut transfers: ReceivedCardTransferIndex,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> ReceivedCardTransferIndex {
    for entry in entries.values() {
        let OplogEntry::CardEventQueued {
            event: QueuedCardEvent::TransferReceived(receipt),
            ..
        } = entry
        else {
            continue;
        };

        let Some(card) = &receipt.card else {
            transfers.insert(receipt.transfer_id, ReceivedCardTransferState::Conflict);
            continue;
        };

        match transfers.get(&receipt.transfer_id) {
            None => {
                transfers.insert(
                    receipt.transfer_id,
                    ReceivedCardTransferState::Received {
                        source_card_id: receipt.source_card_id,
                        card: card.clone(),
                    },
                );
            }
            Some(ReceivedCardTransferState::Conflict) => {}
            Some(ReceivedCardTransferState::Received {
                source_card_id,
                card: recorded_card,
            }) => {
                if recorded_card != card
                    || matches!(
                        (source_card_id, receipt.source_card_id),
                        (Some(recorded), Some(received)) if *recorded != received
                    )
                {
                    transfers.insert(receipt.transfer_id, ReceivedCardTransferState::Conflict);
                } else if source_card_id.is_none() && receipt.source_card_id.is_some() {
                    transfers.insert(
                        receipt.transfer_id,
                        ReceivedCardTransferState::Received {
                            source_card_id: receipt.source_card_id,
                            card: recorded_card.clone(),
                        },
                    );
                }
            }
        }
    }

    transfers
}

// Session lifecycle follows raw durable stream history, including entries inside guest jumps.
// Unlike invocation results, external stream effects cannot be undone by a logical oplog deletion;
// worker revert rejects any worker with durable stream history.
fn calculate_durable_stream_sessions(
    mut sessions: DurableStreamSessionIndex,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<DurableStreamSessionIndex, String> {
    for (oplog_idx, entry) in entries {
        sessions.apply_oplog_entry(*oplog_idx, entry)?;
    }
    Ok(sessions)
}

#[allow(clippy::type_complexity)]
fn calculate_update_fields(
    initial_pending_updates: VecDeque<PendingUpdateRef>,
    initial_failed_updates: Vec<FailedUpdateRecord>,
    initial_successful_updates: Vec<SuccessfulUpdateRecord>,
    initial_revision: ComponentRevision,
    initial_component_size: u64,
    initial_component_revision_for_replay: ComponentRevision,
    initial_last_manual_update_snapshot_index: Option<OplogIndex>,
    initial_last_automatic_snapshot_index: Option<OplogIndex>,
    initial_last_automatic_snapshot_timestamp: Option<Timestamp>,
    initial_last_automatic_snapshot_component_revision: Option<ComponentRevision>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    VecDeque<PendingUpdateRef>,
    Vec<FailedUpdateRecord>,
    Vec<SuccessfulUpdateRecord>,
    ComponentRevision,
    u64,
    ComponentRevision,
    Option<OplogIndex>,
    Option<OplogIndex>,
    Option<Timestamp>,
    Option<ComponentRevision>,
) {
    let mut pending_updates = initial_pending_updates;
    let mut failed_updates = initial_failed_updates;
    let mut successful_updates = initial_successful_updates;
    let mut revision = initial_revision;
    let mut size = initial_component_size;
    let mut component_revision_for_replay = initial_component_revision_for_replay;
    let mut last_manual_update_snapshot_index = initial_last_manual_update_snapshot_index;
    let mut last_automatic_snapshot_index = initial_last_automatic_snapshot_index;
    let mut last_automatic_snapshot_timestamp = initial_last_automatic_snapshot_timestamp;
    let mut last_automatic_snapshot_component_revision =
        initial_last_automatic_snapshot_component_revision;

    for (oplog_idx, entry) in entries {
        // Skipping entries in deleted regions (by revert)
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            continue;
        }

        match entry {
            OplogEntry::Create {
                component_revision,
                component_size,
                ..
            } => {
                revision = *component_revision;
                component_revision_for_replay = *component_revision;
                size = *component_size;
            }
            OplogEntry::PendingUpdate {
                timestamp,
                description,
                ..
            } => {
                let kind = match description {
                    UpdateDescription::Automatic { .. } => PendingUpdateKind::Automatic,
                    UpdateDescription::SnapshotBased { .. } => PendingUpdateKind::SnapshotBased,
                };
                pending_updates.push_back(PendingUpdateRef {
                    timestamp: *timestamp,
                    oplog_index: *oplog_idx,
                    target_revision: *description.target_revision(),
                    kind,
                });
            }
            OplogEntry::FailedUpdate {
                timestamp,
                target_revision,
                details,
            } => {
                failed_updates.push(FailedUpdateRecord {
                    timestamp: *timestamp,
                    target_revision: *target_revision,
                    details: details.clone(),
                });
                pending_updates.pop_front();
            }
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_revision,
                new_component_size,
                ..
            } => {
                successful_updates.push(SuccessfulUpdateRecord {
                    timestamp: *timestamp,
                    target_revision: *target_revision,
                });
                revision = *target_revision;
                size = *new_component_size;

                let applied_update = pending_updates.pop_front();
                last_automatic_snapshot_index = None;
                last_automatic_snapshot_timestamp = None;
                last_automatic_snapshot_component_revision = None;

                if let Some(PendingUpdateRef {
                    kind: PendingUpdateKind::SnapshotBased,
                    oplog_index: applied_update_oplog_index,
                    ..
                }) = applied_update
                {
                    component_revision_for_replay = *target_revision;
                    last_manual_update_snapshot_index = Some(applied_update_oplog_index);
                }
            }
            OplogEntry::Snapshot { timestamp, .. } => {
                last_automatic_snapshot_index = Some(*oplog_idx);
                last_automatic_snapshot_timestamp = Some(*timestamp);
                last_automatic_snapshot_component_revision = Some(revision);
            }
            _ => {}
        }
    }
    (
        pending_updates,
        failed_updates,
        successful_updates,
        revision,
        size,
        component_revision_for_replay,
        last_manual_update_snapshot_index,
        last_automatic_snapshot_index,
        last_automatic_snapshot_timestamp,
        last_automatic_snapshot_component_revision,
    )
}

fn calculate_invocation_results(
    invocation_results: InvocationResultMembership,
    current_idempotency_key: Option<IdempotencyKey>,
    cancelled_idempotency_key: Option<IdempotencyKey>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    InvocationResultMembership,
    Option<IdempotencyKey>,
    Option<IdempotencyKey>,
) {
    let mut invocation_results = invocation_results;
    let revert_count = entries
        .values()
        .filter(|entry| matches!(entry, OplogEntry::Revert { .. }))
        .count() as u64;
    invocation_results.set_revert_generation(
        invocation_results
            .revert_generation()
            .wrapping_add(revert_count),
    );
    let mut current_idempotency_key = current_idempotency_key;
    let mut cancelled_idempotency_key = cancelled_idempotency_key;

    fold_invocation_result_entries(
        &mut current_idempotency_key,
        &mut cancelled_idempotency_key,
        deleted_regions,
        entries,
        |key, index| invocation_results.insert(key.clone(), index),
    );

    (
        invocation_results,
        current_idempotency_key,
        cancelled_idempotency_key,
    )
}

pub(crate) fn fold_invocation_result_entries(
    current_idempotency_key: &mut Option<IdempotencyKey>,
    cancelled_idempotency_key: &mut Option<IdempotencyKey>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    mut observe_result: impl FnMut(&IdempotencyKey, OplogIndex),
) {
    for (oplog_idx, entry) in entries {
        // Skipping entries in deleted regions (by revert)
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            *cancelled_idempotency_key = None;
            continue;
        }

        match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                *cancelled_idempotency_key = None;
                *current_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                *cancelled_idempotency_key = None;
                if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
                *current_idempotency_key = None;
            }
            OplogEntry::CancelPendingInvocation {
                idempotency_key, ..
            } => {
                *cancelled_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::Error {
                error: AgentError::PermissionDenied(_),
                ..
            } => {
                if let Some(idempotency_key) = cancelled_idempotency_key.take() {
                    observe_result(&idempotency_key, *oplog_idx);
                } else if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
            }
            OplogEntry::Error { .. } => {
                *cancelled_idempotency_key = None;
                if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
            }
            OplogEntry::Exited { .. } => {
                *cancelled_idempotency_key = None;
                if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
            }
            _ => {
                *cancelled_idempotency_key = None;
            }
        }
    }
}

fn calculate_total_linear_memory_size(
    total: u64,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> u64 {
    let mut result = total;
    for (idx, entry) in entries {
        // Skipping entries in skipped regions as they are not applied during replay
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::Create {
                initial_total_linear_memory_size,
                ..
            } => {
                result = *initial_total_linear_memory_size;
            }
            OplogEntry::GrowMemory { delta, .. } => {
                result = result.saturating_add(*delta);
            }
            OplogEntry::SuccessfulUpdate {
                new_total_linear_memory_size: Some(new_total),
                ..
            } => {
                result = *new_total;
            }
            _ => {}
        }
    }
    result
}

fn collect_resources(
    initial: HashMap<AgentResourceId, AgentResourceDescription>,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashMap<AgentResourceId, AgentResourceDescription> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::CreateResource {
                id,
                timestamp,
                resource_type_id,
                ..
            } => {
                result.insert(
                    *id,
                    AgentResourceDescription {
                        created_at: *timestamp,
                        resource_owner: resource_type_id.owner.clone(),
                        resource_name: resource_type_id.name.clone(),
                    },
                );
            }
            OplogEntry::DropResource { id, .. } => {
                result.remove(id);
            }

            _ => {}
        }
    }
    result
}

fn calculate_active_plugins(
    initial: HashSet<EnvironmentPluginGrantId>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<EnvironmentPluginGrantId> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::Create {
                initial_active_plugins,
                ..
            } => {
                result = initial_active_plugins.clone();
            }
            OplogEntry::ActivatePlugin {
                plugin_grant_id, ..
            } => {
                result.insert(*plugin_grant_id);
            }
            OplogEntry::DeactivatePlugin {
                plugin_grant_id, ..
            } => {
                result.remove(plugin_grant_id);
            }
            OplogEntry::SuccessfulUpdate {
                new_active_plugins, ..
            } => {
                result = new_active_plugins.clone();
            }
            _ => {}
        }
    }
    result
}

fn calculate_oplog_processor_checkpoints(
    mut result: HashMap<EnvironmentPluginGrantId, OplogProcessorCheckpointState>,
    active_plugins: &HashSet<EnvironmentPluginGrantId>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    finalize: bool,
) -> HashMap<EnvironmentPluginGrantId, OplogProcessorCheckpointState> {
    for (idx, entry) in entries {
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::OplogProcessorCheckpoint {
                plugin_grant_id,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
                ..
            } => {
                result.insert(
                    *plugin_grant_id,
                    OplogProcessorCheckpointState {
                        target_agent_id: Some(target_agent_id.clone()),
                        confirmed_up_to: *confirmed_up_to,
                        sending_up_to: *sending_up_to,
                        last_batch_start: *last_batch_start,
                    },
                );
            }
            OplogEntry::ActivatePlugin {
                plugin_grant_id, ..
            } => {
                result
                    .entry(*plugin_grant_id)
                    .or_insert(OplogProcessorCheckpointState {
                        target_agent_id: None,
                        confirmed_up_to: *idx,
                        sending_up_to: *idx,
                        last_batch_start: *idx,
                    });
            }
            OplogEntry::DeactivatePlugin {
                plugin_grant_id, ..
            } => {
                // Remove non-in-flight checkpoint so a later same-fold ActivatePlugin
                // can seed a fresh checkpoint at the new activation index
                let keep_in_flight = result
                    .get(plugin_grant_id)
                    .is_some_and(|state| state.sending_up_to > state.confirmed_up_to);
                if !keep_in_flight {
                    result.remove(plugin_grant_id);
                }
            }
            OplogEntry::SuccessfulUpdate {
                new_active_plugins, ..
            } => {
                result.retain(|grant_id, state| {
                    new_active_plugins.contains(grant_id)
                        || state.sending_up_to > state.confirmed_up_to
                });
                for grant_id in new_active_plugins {
                    result
                        .entry(*grant_id)
                        .or_insert(OplogProcessorCheckpointState {
                            target_agent_id: None,
                            confirmed_up_to: *idx,
                            sending_up_to: *idx,
                            last_batch_start: *idx,
                        });
                }
            }
            _ => {}
        }
    }

    if finalize {
        result.retain(|grant_id, state| {
            active_plugins.contains(grant_id) || state.sending_up_to > state.confirmed_up_to
        });
    }

    result
}

fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &AgentError,
    retry_count: u32,
    inside_atomic_region: bool,
    semantic_retry_state: Option<&RetryPolicyState>,
) -> bool {
    if let Some(state) = semantic_retry_state {
        return !state.is_exhausted();
    }

    match error {
        AgentError::Unknown(_) | AgentError::TransientError(_) => {
            retry_count < retry_config.max_attempts
        }
        AgentError::DeterministicTrap(_) if inside_atomic_region => {
            retry_count < retry_config.max_attempts
        }
        AgentError::InvalidRequest(_) => false,
        AgentError::StackOverflow => false,
        AgentError::OutOfMemory => true,
        AgentError::ExceededMemoryLimit => false,
        AgentError::ExceededTableLimit => false,
        AgentError::InternalError(_) => false,
        AgentError::DeterministicTrap(_) => false,
        AgentError::PermanentError(_) => false,
        AgentError::ExceededHttpCallLimit => false,
        AgentError::ExceededRpcCallLimit => false,
        AgentError::AgentTerminatedByQuota(_) => false,
        AgentError::EphemeralSleepTooLong(_) => false,
        AgentError::EphemeralFuelExhausted(_) => false,
        AgentError::EphemeralCannotSuspend(_) => false,
        AgentError::ReadOnlyViolation(_) => false,
        AgentError::PermissionDenied(_) => false,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod test;
