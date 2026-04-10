use crate::services::{HasConfig, HasOplogService};
use async_recursion::async_recursion;
use golem_common::base_model::OplogIndex;
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::AgentInvocationPayload;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    AgentError, AgentResourceId, OplogEntry, OplogPayload, TimestampedUpdateDescription,
    UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    AgentInvocation, AgentResourceDescription, AgentStatus, AgentStatusRecord, FailedUpdateRecord,
    IdempotencyKey, OplogProcessorCheckpointState, OwnedAgentId, RetryConfig,
    SuccessfulUpdateRecord, Timestamp, TimestampedAgentInvocation,
};
use golem_common::serialization::deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Like calculate_last_known_status, but assumes that the oplog exists and has at least a Create entry in it.
pub async fn calculate_last_known_status_for_existing_worker<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    last_known: Option<AgentStatusRecord>,
) -> AgentStatusRecord
where
    T: HasOplogService + HasConfig + Sync,
{
    calculate_last_known_status(this, owned_agent_id, last_known)
        .await
        .expect("Failed to calculate oplog index for existing worker")
}

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker status.
#[async_recursion]
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    last_known: Option<AgentStatusRecord>,
) -> Option<AgentStatusRecord>
where
    T: HasOplogService + HasConfig + Sync,
{
    let last_known = last_known.unwrap_or_default();

    let last_oplog_index = this.oplog_service().get_last_index(owned_agent_id).await;
    assert!(last_oplog_index >= last_known.oplog_idx);

    if last_oplog_index == OplogIndex::NONE {
        // Worker status can only be recovered if we have at least the Create oplog entry, otherwise we cannot recover information like the component version
        None
    } else if last_known.oplog_idx == last_oplog_index {
        Some(last_known)
    } else {
        let new_entries: BTreeMap<OplogIndex, OplogEntry> = this
            .oplog_service()
            .read_range(
                owned_agent_id,
                last_known.oplog_idx.next(),
                last_oplog_index,
            )
            .await;

        let final_status = update_status_with_new_entries(
            this,
            owned_agent_id,
            last_known,
            new_entries,
            &this.config().retry,
        )
        .await;

        if let Some(final_status) = final_status {
            Some(final_status)
        } else {
            calculate_last_known_status(this, owned_agent_id, None).await
        }
    }
}

// update a worker status with new entries. Returns None if the status cannot be calculated from the new entries alone and needs to be recalculated from the beginning.
pub async fn update_status_with_new_entries<T: HasOplogService + Sync>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    // TODO: changing the retry policy will cause inconsistencies when reading existing oplogs.
    default_retry_policy: &RetryConfig,
) -> Option<AgentStatusRecord> {
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
    if skipped_regions.is_in_deleted_region(last_known.oplog_idx) {
        let last_known_skipped_regions_without_overrides =
            if last_known.skipped_regions.is_overridden() {
                let mut cloned = last_known.skipped_regions.clone();
                cloned.merge_override();
                cloned
            } else {
                last_known.skipped_regions.clone()
            };

        let new_skipped_regions_without_overrides = if skipped_regions.is_overridden() {
            let mut cloned = skipped_regions.clone();
            cloned.merge_override();
            cloned
        } else {
            skipped_regions.clone()
        };

        let effective_skipped_regions_changed =
            new_skipped_regions_without_overrides != last_known_skipped_regions_without_overrides;
        // We might have already calculated the status with these skipped regions as an override during a snapshot update.
        // No need to recompute in this case, we are already up to date.
        if effective_skipped_regions_changed {
            return None;
        }
    }

    let active_plugins = last_known.active_plugins.clone();

    let (status, current_retry_count, overridden_retry_config) = calculate_latest_worker_status(
        last_known.status,
        last_known.current_retry_count,
        last_known.overridden_retry_config,
        default_retry_policy,
        &skipped_regions,
        &deleted_regions,
        &new_entries,
    );

    let pending_invocations = calculate_pending_invocations(
        this,
        owned_agent_id,
        last_known.pending_invocations,
        &new_entries,
    )
    .await;
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
        &deleted_regions,
        &new_entries,
    );

    let (invocation_results, current_idempotency_key) = calculate_invocation_results(
        last_known.invocation_results,
        last_known.current_idempotency_key,
        &deleted_regions,
        &new_entries,
    );

    let total_linear_memory_size = calculate_total_linear_memory_size(
        last_known.total_linear_memory_size,
        &skipped_regions,
        &new_entries,
    );

    let current_filesystem_storage_usage = calculate_current_filesystem_storage_usage(
        last_known.current_filesystem_storage_usage,
        &skipped_regions,
        &new_entries,
    );

    let owned_resources =
        collect_resources(last_known.owned_resources, &skipped_regions, &new_entries);

    let active_plugins = calculate_active_plugins(active_plugins, &deleted_regions, &new_entries);

    let oplog_processor_checkpoints = calculate_oplog_processor_checkpoints(
        last_known.oplog_processor_checkpoints,
        &active_plugins,
        &deleted_regions,
        &new_entries,
    );

    let result = AgentStatusRecord {
        oplog_idx: new_entries
            .keys()
            .max()
            .cloned()
            .unwrap_or(last_known.oplog_idx),
        status,
        overridden_retry_config,
        pending_invocations,
        skipped_regions,
        pending_updates,
        failed_updates,
        successful_updates,
        invocation_results,
        current_idempotency_key,
        component_revision,
        component_size,
        owned_resources,
        total_linear_memory_size,
        current_filesystem_storage_usage,
        active_plugins,
        oplog_processor_checkpoints,
        deleted_regions,
        component_revision_for_replay,
        current_retry_count,
        last_manual_update_snapshot_index,
        last_automatic_snapshot_index,
        last_automatic_snapshot_timestamp,
    };

    Some(result)
}

fn calculate_latest_worker_status(
    mut current_status: AgentStatus,
    mut current_retry_count: HashMap<OplogIndex, u32>,
    mut current_retry_policy: Option<RetryConfig>,
    default_retry_policy: &RetryConfig,
    skipped_regions: &DeletedRegions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (AgentStatus, HashMap<OplogIndex, u32>, Option<RetryConfig>) {
    for (idx, entry) in entries {
        // Errors are counted in skipped regions too (but not in deleted ones),
        // otherwise we would not be able to know how many times we retried failures in atomic regions.
        // This must happen before the skipped-region continue below.
        if !deleted_regions.is_in_deleted_region(*idx)
            && let OplogEntry::Error { retry_from, .. } = entry
        {
            let new_count = current_retry_count
                .get(retry_from)
                .copied()
                .unwrap_or_default()
                + 1;
            current_retry_count.insert(*retry_from, new_count);
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
                ..
            } = entry
        {
            let count = current_retry_count
                .get(retry_from)
                .copied()
                .unwrap_or_default();
            if is_worker_error_retriable(
                current_retry_policy
                    .as_ref()
                    .unwrap_or(default_retry_policy),
                error,
                count,
                *inside_atomic_region,
            ) {
                current_status = AgentStatus::Retrying;
            } else {
                current_status = AgentStatus::Failed;
            }
        }

        match entry {
            OplogEntry::Create { .. } => {
                current_status = AgentStatus::Idle;
            }
            OplogEntry::HostCall { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::AgentInvocationStarted { .. } => {
                current_status = AgentStatus::Running;
                current_retry_count.clear();
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                current_status = AgentStatus::Idle;
                current_retry_count.clear();
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
            OplogEntry::ChangeRetryPolicy { new_policy, .. } => {
                current_retry_policy = Some(new_policy.clone());
                current_status = AgentStatus::Running;
            }
            OplogEntry::BeginAtomicRegion { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::EndAtomicRegion { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::BeginRemoteWrite { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::EndRemoteWrite { .. } => {
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
            OplogEntry::FilesystemStorageUsageUpdate { .. } => {}
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
            OplogEntry::ChangePersistenceLevel { .. } => {
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
            OplogEntry::Error { .. } => {
                // .. handled separately
            }
        }
    }
    (current_status, current_retry_count, current_retry_policy)
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

async fn calculate_pending_invocations<T: HasOplogService + Sync>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    initial: Vec<TimestampedAgentInvocation>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<TimestampedAgentInvocation> {
    let mut result = initial;
    for entry in entries.values() {
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

        match entry {
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => {
                let agent_payload: Option<AgentInvocationPayload> = match payload {
                    OplogPayload::Inline(p) => Some(*p.clone()),
                    OplogPayload::SerializedInline {
                        cached: Some(v), ..
                    } => Some((**v).clone()),
                    OplogPayload::SerializedInline { bytes, .. } => {
                        deserialize::<AgentInvocationPayload>(bytes)
                            .map_err(|e| {
                                tracing::warn!(
                                    "Failed to deserialize pending agent invocation payload: {e}"
                                );
                                e
                            })
                            .ok()
                    }
                    OplogPayload::External {
                        cached: Some(v), ..
                    } => Some((**v).clone()),
                    OplogPayload::External {
                        payload_id,
                        md5_hash,
                        ..
                    } => {
                        match this
                            .oplog_service()
                            .download_raw_payload(
                                owned_agent_id,
                                payload_id.clone(),
                                md5_hash.clone(),
                            )
                            .await
                        {
                            Ok(bytes) => deserialize::<AgentInvocationPayload>(&bytes)
                                .map_err(|e| {
                                    tracing::warn!(
                                        "Failed to deserialize external pending agent invocation payload: {e}"
                                    );
                                    e
                                })
                                .ok(),
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to download external pending agent invocation payload: {e}"
                                );
                                None
                            }
                        }
                    }
                };
                if let Some(agent_payload) = agent_payload {
                    let invocation_context_stack = InvocationContextStack::from_oplog_data(
                        trace_id.clone(),
                        trace_states.clone(),
                        invocation_context.clone(),
                    );
                    let invocation = AgentInvocation::from_parts(
                        idempotency_key.clone(),
                        agent_payload,
                        invocation_context_stack,
                    );
                    result.push(TimestampedAgentInvocation {
                        timestamp: *timestamp,
                        invocation,
                    });
                }
            }
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                result.retain(|invocation| {
                    !invocation.invocation.has_idempotency_key(idempotency_key)
                });
            }
            OplogEntry::PendingUpdate {
                description:
                    UpdateDescription::SnapshotBased {
                        target_revision, ..
                    },
                ..
            } => result.retain(|invocation| match invocation {
                TimestampedAgentInvocation {
                    invocation:
                        AgentInvocation::ManualUpdate {
                            target_revision: revision,
                            ..
                        },
                    ..
                } => revision != target_revision,
                _ => true,
            }),
            OplogEntry::FailedUpdate {
                target_revision, ..
            } => result.retain(|invocation| match invocation {
                TimestampedAgentInvocation {
                    invocation:
                        AgentInvocation::ManualUpdate {
                            target_revision: revision,
                            ..
                        },
                    ..
                } => revision != target_revision,
                _ => true,
            }),
            OplogEntry::CancelPendingInvocation {
                idempotency_key, ..
            } => {
                result.retain(|invocation| {
                    !invocation.invocation.has_idempotency_key(idempotency_key)
                });
            }
            _ => {}
        }
    }
    result
}

#[allow(clippy::type_complexity)]
fn calculate_update_fields(
    initial_pending_updates: VecDeque<TimestampedUpdateDescription>,
    initial_failed_updates: Vec<FailedUpdateRecord>,
    initial_successful_updates: Vec<SuccessfulUpdateRecord>,
    initial_revision: ComponentRevision,
    initial_component_size: u64,
    initial_component_revision_for_replay: ComponentRevision,
    initial_last_manual_update_snapshot_index: Option<OplogIndex>,
    initial_last_automatic_snapshot_index: Option<OplogIndex>,
    initial_last_automatic_snapshot_timestamp: Option<Timestamp>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    VecDeque<TimestampedUpdateDescription>,
    Vec<FailedUpdateRecord>,
    Vec<SuccessfulUpdateRecord>,
    ComponentRevision,
    u64,
    ComponentRevision,
    Option<OplogIndex>,
    Option<OplogIndex>,
    Option<Timestamp>,
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
                pending_updates.push_back(TimestampedUpdateDescription {
                    timestamp: *timestamp,
                    oplog_index: *oplog_idx,
                    description: description.clone(),
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

                if let Some(TimestampedUpdateDescription {
                    description: UpdateDescription::SnapshotBased { .. },
                    oplog_index: applied_update_oplog_index,
                    ..
                }) = pending_updates.pop_front()
                {
                    component_revision_for_replay = *target_revision;
                    last_manual_update_snapshot_index = Some(applied_update_oplog_index);
                    last_automatic_snapshot_index = None;
                    last_automatic_snapshot_timestamp = None;
                }
            }
            OplogEntry::Snapshot { timestamp, .. } => {
                last_automatic_snapshot_index = Some(*oplog_idx);
                last_automatic_snapshot_timestamp = Some(*timestamp);
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
    )
}

fn calculate_invocation_results(
    invocation_results: HashMap<IdempotencyKey, OplogIndex>,
    current_idempotency_key: Option<IdempotencyKey>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (HashMap<IdempotencyKey, OplogIndex>, Option<IdempotencyKey>) {
    let mut invocation_results = invocation_results;
    let mut current_idempotency_key = current_idempotency_key;

    for (oplog_idx, entry) in entries {
        // Skipping entries in deleted regions (by revert)
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            continue;
        }

        match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                current_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                if let Some(idempotency_key) = &current_idempotency_key {
                    invocation_results.insert(idempotency_key.clone(), *oplog_idx);
                }
                current_idempotency_key = None;
            }
            OplogEntry::Error { .. } => {
                if let Some(idempotency_key) = &current_idempotency_key {
                    invocation_results.insert(idempotency_key.clone(), *oplog_idx);
                }
            }
            OplogEntry::Exited { .. } => {
                if let Some(idempotency_key) = &current_idempotency_key {
                    invocation_results.insert(idempotency_key.clone(), *oplog_idx);
                }
            }
            _ => {}
        }
    }

    (invocation_results, current_idempotency_key)
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
                result += *delta;
            }
            _ => {}
        }
    }
    result
}

/// Accumulates `FilesystemStorageUsageUpdate` hint entries to reconstruct the current
/// storage usage at any point in the oplog. Used to populate
/// `AgentStatusRecord::current_filesystem_storage_usage` for pre-acquiring storage permits
/// when a worker restarts.
///
/// Mirrors `calculate_total_linear_memory_size`: entries in skipped regions
/// are excluded, and `Create` resets the counter to zero (a newly created worker
/// has no written files yet).
fn calculate_current_filesystem_storage_usage(
    current: u64,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> u64 {
    let mut result = current as i64;
    for (idx, entry) in entries {
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::Create { .. } => {
                result = 0;
            }
            OplogEntry::FilesystemStorageUsageUpdate { delta, .. } => {
                result = result.saturating_add(*delta);
            }
            _ => {}
        }
    }
    result.max(0) as u64
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

    result.retain(|grant_id, state| {
        active_plugins.contains(grant_id) || state.sending_up_to > state.confirmed_up_to
    });

    result
}

fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &AgentError,
    retry_count: u32,
    inside_atomic_region: bool,
) -> bool {
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
        AgentError::NodeOutOfFilesystemStorage => true,
        AgentError::AgentExceededFilesystemStorageLimit => false,
    }
}

#[cfg(test)]
mod test {
    use crate::model::ExecutionStatus;
    use crate::services::golem_config::GolemConfig;
    use crate::services::oplog::{Oplog, OplogService};
    use crate::services::{HasConfig, HasOplogService};
    use crate::worker::status::{
        calculate_last_known_status, calculate_last_known_status_for_existing_worker,
        calculate_oplog_processor_checkpoints,
    };
    use async_trait::async_trait;
    use golem_common::base_model::OplogIndex;
    use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{Principal, UntypedDataValue, UntypedElementValue};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::invocation_context::{InvocationContextStack, TraceId};
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{
        DurableFunctionType, HostRequest, HostRequestNoInput, HostResponse, OplogEntry,
        OplogPayload, PayloadId, RawOplogPayload, TimestampedUpdateDescription, UpdateDescription,
    };
    use golem_common::model::regions::{DeletedRegions, OplogRegion};
    use golem_common::model::{
        AgentId, AgentInvocation, AgentInvocationPayload, AgentInvocationResult, AgentMetadata,
        AgentStatus, AgentStatusRecord, FailedUpdateRecord, IdempotencyKey,
        OplogProcessorCheckpointState, OwnedAgentId, RetryConfig, ScanCursor,
        SuccessfulUpdateRecord, Timestamp, TimestampedAgentInvocation,
    };
    use golem_common::read_only_lock;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_wasm::{IntoValueAndType, Value};
    use pretty_assertions::assert_eq;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::Arc;
    use test_r::test;

    #[test]
    async fn empty() {
        let test_case = TestCase::builder(0).build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn storage_usage_accumulated_from_deltas() {
        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], IdempotencyKey::fresh())
            .filesystem_storage_usage_update(1024)
            .filesystem_storage_usage_update(2048)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn storage_usage_decremented_on_negative_delta() {
        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], IdempotencyKey::fresh())
            .filesystem_storage_usage_update(1024)
            .filesystem_storage_usage_update(-512)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn storage_usage_clamped_at_zero_on_underflow() {
        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], IdempotencyKey::fresh())
            .filesystem_storage_usage_update(100)
            .filesystem_storage_usage_update(-9999) // larger than total acquired
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .agent_invocation_started("b", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results_with_jump() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .jump(OplogIndex::from_u64(2))
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .agent_invocation_started("b", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results_with_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .agent_invocation_started("b", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .revert(OplogIndex::from_u64(5))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_auto_update_for_running() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(2).unwrap(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_update(&update1, |_| {})
            .successful_update(update1, 2000, &HashSet::new())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn auto_update_for_running_with_jump() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(2).unwrap(),
        };
        let update2 = UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(3).unwrap(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_update(&update1, |_| {})
            .pending_update(&update2, |_| {})
            .successful_update(update1, 2000, &HashSet::new())
            .jump(OplogIndex::from_u64(4))
            .successful_update(update2, 3000, &HashSet::new())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_update() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(2).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(AgentInvocation::ManualUpdate {
                target_revision: ComponentRevision::new(2).unwrap(),
            })
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .pending_update(&update1, |status| status.total_linear_memory_size = 200)
            .successful_update(update1, 2000, &HashSet::new())
            .agent_invocation_started("c", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_failed_update() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(2).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(AgentInvocation::ManualUpdate {
                target_revision: ComponentRevision::new(2).unwrap(),
            })
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .pending_update(&update1, |_| {})
            .failed_update(update1)
            .agent_invocation_started("c", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_failed_update_during_snapshot() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update2 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(2).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(AgentInvocation::ManualUpdate {
                target_revision: ComponentRevision::new(2).unwrap(),
            })
            .failed_update(update2)
            .agent_invocation_started("c", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn auto_update_for_running_with_jump_and_revert() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(2).unwrap(),
        };
        let update2 = UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(3).unwrap(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_update(&update1, |_| {})
            .pending_update(&update2, |_| {})
            .successful_update(update1, 2000, &HashSet::new())
            .jump(OplogIndex::from_u64(4))
            .successful_update(update2, 3000, &HashSet::new())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .revert(OplogIndex::from_u64(3))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_update_with_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(2).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(AgentInvocation::ManualUpdate {
                target_revision: ComponentRevision::new(2).unwrap(),
            })
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .pending_update(&update1, |_| {})
            .successful_update(update1, 2000, &HashSet::new())
            .agent_invocation_started("c", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .revert(OplogIndex::from_u64(4))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn multiple_manual_updates_with_jump_and_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(2).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        };
        let update2 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(2).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        };

        let test_case = TestCase::builder(1)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(AgentInvocation::ManualUpdate {
                target_revision: ComponentRevision::new(2).unwrap(),
            })
            .host_call(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .pending_update(&update1, |_| {})
            .failed_update(update1)
            .agent_invocation_started("c", vec![], k2.clone())
            .pending_invocation(AgentInvocation::ManualUpdate {
                target_revision: ComponentRevision::new(2).unwrap(),
            })
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .pending_update(&update2, |_| {})
            .successful_update(update2, 2000, &HashSet::new())
            .revert(OplogIndex::from_u64(5))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn multiple_reverts() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .pending_invocation(AgentInvocation::AgentMethod {
                idempotency_key: k2.clone(),
                method_name: "b".to_string(),
                input: UntypedDataValue::Tuple(vec![UntypedElementValue::ComponentModel(
                    Value::Bool(true),
                )]),
                invocation_context: InvocationContextStack::fresh(),
                principal: Principal::anonymous(),
            })
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1.clone(),
                ComponentRevision::INITIAL,
            )
            .agent_invocation_started("b", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2.clone(),
                ComponentRevision::INITIAL,
            )
            .revert(OplogIndex::from_u64(5))
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .agent_invocation_started("b", vec![], k2.clone())
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k2,
                ComponentRevision::INITIAL,
            )
            .revert(OplogIndex::from_u64(2))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn cancel_pending_invocation() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .pending_invocation(AgentInvocation::AgentMethod {
                idempotency_key: k1.clone(),
                method_name: "a".to_string(),
                input: UntypedDataValue::Tuple(vec![UntypedElementValue::ComponentModel(
                    Value::Bool(true),
                )]),
                invocation_context: InvocationContextStack::fresh(),
                principal: Principal::anonymous(),
            })
            .pending_invocation(AgentInvocation::AgentMethod {
                idempotency_key: k2.clone(),
                method_name: "b".to_string(),
                input: UntypedDataValue::Tuple(vec![]),
                invocation_context: InvocationContextStack::fresh(),
                principal: Principal::anonymous(),
            })
            .cancel_pending_invocation(k1)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn snapshot_tracking() {
        let k1 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .snapshot()
            .grow_memory(100)
            .snapshot()
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn snapshot_tracking_with_revert() {
        let k1 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .agent_invocation_started("a", vec![], k1.clone())
            .grow_memory(10)
            .snapshot()
            .grow_memory(100)
            .snapshot()
            .agent_invocation_finished(
                AgentInvocationResult::AgentInitialization,
                k1,
                ComponentRevision::INITIAL,
            )
            .revert(OplogIndex::from_u64(3))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn non_existing_oplog() {
        let environment_id = EnvironmentId::new();
        let owned_agent_id = OwnedAgentId::new(
            environment_id,
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "test-worker".to_string(),
            },
        );
        let test_case = TestCase {
            owned_agent_id: owned_agent_id.clone(),
            entries: vec![],
        };

        let result = calculate_last_known_status(&test_case, &owned_agent_id, None).await;
        assert2::assert!(let None = result);
    }

    struct TestCaseBuilder {
        entries: Vec<TestEntry>,
        previous_status_record: AgentStatusRecord,
        owned_agent_id: OwnedAgentId,
    }

    impl TestCaseBuilder {
        pub fn new(
            account_id: AccountId,
            owned_agent_id: OwnedAgentId,
            component_revision: ComponentRevision,
        ) -> Self {
            let status = AgentStatusRecord {
                component_revision,
                component_revision_for_replay: component_revision,
                component_size: 100,
                total_linear_memory_size: 200,
                oplog_idx: OplogIndex::INITIAL,
                ..Default::default()
            };
            TestCaseBuilder {
                entries: vec![TestEntry {
                    oplog_entry: OplogEntry::create(
                        owned_agent_id.agent_id(),
                        component_revision,
                        vec![],
                        owned_agent_id.environment_id(),
                        account_id,
                        None,
                        100,
                        200,
                        HashSet::new(),
                        BTreeMap::new(),
                        Vec::new(),
                        None,
                    ),
                    expected_status: status.clone(),
                }],
                previous_status_record: status,
                owned_agent_id,
            }
        }

        pub fn add(
            mut self,
            entry: OplogEntry,
            update: impl FnOnce(AgentStatusRecord) -> AgentStatusRecord,
        ) -> Self {
            self.previous_status_record.oplog_idx = self.previous_status_record.oplog_idx.next();
            self.previous_status_record = update(self.previous_status_record);
            self.entries.push(TestEntry {
                oplog_entry: entry,
                expected_status: self.previous_status_record.clone(),
            });
            self
        }

        pub fn agent_invocation_started(
            self,
            function_name: &str,
            request: Vec<Value>,
            idempotency_key: IdempotencyKey,
        ) -> Self {
            let payload = AgentInvocationPayload::AgentMethod {
                method_name: function_name.to_string(),
                input: UntypedDataValue::Tuple(
                    request
                        .into_iter()
                        .map(UntypedElementValue::ComponentModel)
                        .collect(),
                ),
                principal: Principal::anonymous(),
            };
            self.add(
                OplogEntry::AgentInvocationStarted {
                    timestamp: Timestamp::now_utc(),
                    idempotency_key: idempotency_key.clone(),
                    payload: OplogPayload::Inline(Box::new(payload)),
                    trace_id: TraceId::generate(),
                    trace_states: vec![],
                    invocation_context: vec![],
                },
                move |mut status| {
                    status.current_idempotency_key = Some(idempotency_key);
                    status.status = AgentStatus::Running;
                    if !status.pending_invocations.is_empty() {
                        status.pending_invocations.pop();
                    }
                    status
                },
            )
        }

        pub fn agent_invocation_finished(
            self,
            result: AgentInvocationResult,
            idempotency_key: IdempotencyKey,
            component_revision: ComponentRevision,
        ) -> Self {
            self.add(
                OplogEntry::AgentInvocationFinished {
                    timestamp: Timestamp::now_utc(),
                    result: OplogPayload::Inline(Box::new(result)),
                    consumed_fuel: 0,
                    component_revision,
                },
                move |mut status| {
                    status
                        .invocation_results
                        .insert(idempotency_key, status.oplog_idx);
                    status.current_idempotency_key = None;
                    status.status = AgentStatus::Idle;
                    status
                },
            )
        }

        pub fn host_call(
            self,
            name: &str,
            i: HostRequest,
            o: HostResponse,
            func_type: DurableFunctionType,
        ) -> Self {
            self.add(
                OplogEntry::HostCall {
                    timestamp: Timestamp::now_utc(),
                    function_name: HostFunctionName::Custom(name.to_string()),
                    request: OplogPayload::Inline(Box::new(i)),
                    response: OplogPayload::Inline(Box::new(o)),
                    durable_function_type: func_type,
                },
                |status| status,
            )
        }

        pub fn grow_memory(self, delta: u64) -> Self {
            self.add(
                OplogEntry::GrowMemory {
                    timestamp: Timestamp::now_utc(),
                    delta,
                },
                |mut status| {
                    status.total_linear_memory_size += delta;
                    status
                },
            )
        }

        pub fn filesystem_storage_usage_update(self, delta: i64) -> Self {
            self.add(
                OplogEntry::FilesystemStorageUsageUpdate {
                    timestamp: Timestamp::now_utc(),
                    delta,
                },
                |mut status| {
                    status.current_filesystem_storage_usage =
                        (status.current_filesystem_storage_usage as i64)
                            .saturating_add(delta)
                            .max(0) as u64;
                    status
                },
            )
        }

        pub fn snapshot(self) -> Self {
            let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
            let timestamp = Timestamp::now_utc().rounded();
            self.add(
                OplogEntry::Snapshot {
                    timestamp,
                    data: OplogPayload::Inline(Box::new(vec![])),
                    mime_type: "application/octet-stream".to_string(),
                },
                move |mut status| {
                    status.last_automatic_snapshot_index = Some(oplog_idx);
                    status.last_automatic_snapshot_timestamp = Some(timestamp);
                    status
                },
            )
        }

        pub fn jump(self, target: OplogIndex) -> Self {
            let current = OplogIndex::from_u64(self.entries.len() as u64 + 1);
            let region = OplogRegion {
                start: target,
                end: current,
            };
            let old_status = self.entries[u64::from(target) as usize - 1]
                .expected_status
                .clone();
            self.add(OplogEntry::jump(region.clone()), move |mut status| {
                status.status = old_status.status;
                status.component_revision = old_status.component_revision;
                status.current_idempotency_key = old_status.current_idempotency_key;
                status.total_linear_memory_size = old_status.total_linear_memory_size;
                status.component_size = old_status.component_size;
                status.owned_resources = old_status.owned_resources;
                status.skipped_regions.add(region);
                status
            })
        }

        pub fn revert(self, target: OplogIndex) -> Self {
            let current = OplogIndex::from_u64(self.entries.len() as u64 + 1);
            let region = OplogRegion {
                start: target.next(),
                end: current,
            };

            let old_status = self.entries[u64::from(target) as usize - 1]
                .expected_status
                .clone();
            self.add(OplogEntry::revert(region.clone()), move |mut status| {
                status.active_plugins = old_status.active_plugins;

                status.skipped_regions = old_status.skipped_regions;
                status.skipped_regions.add(region.clone());
                status.deleted_regions.add(region);

                status.status = old_status.status;
                status.component_revision = old_status.component_revision;
                status.current_idempotency_key = old_status.current_idempotency_key;
                status.total_linear_memory_size = old_status.total_linear_memory_size;
                status.component_size = old_status.component_size;
                status.owned_resources = old_status.owned_resources;
                status.successful_updates = old_status.successful_updates;
                status.failed_updates = old_status.failed_updates;
                status.invocation_results = old_status.invocation_results;
                status.component_revision_for_replay = old_status.component_revision_for_replay;
                status.last_manual_update_snapshot_index =
                    old_status.last_manual_update_snapshot_index;
                status.last_automatic_snapshot_index = old_status.last_automatic_snapshot_index;
                status.last_automatic_snapshot_timestamp =
                    old_status.last_automatic_snapshot_timestamp;

                status
            })
        }

        pub fn pending_invocation(self, invocation: AgentInvocation) -> Self {
            let (idempotency_key, invocation_payload, invocation_context) =
                invocation.clone().into_parts();
            let entry = OplogEntry::pending_agent_invocation(
                idempotency_key,
                OplogPayload::Inline(Box::new(invocation_payload)),
                invocation_context.trace_id.clone(),
                invocation_context.trace_states.clone(),
                invocation_context.to_oplog_data(),
            )
            .rounded();
            self.add(entry.clone(), move |mut status| {
                status.pending_invocations.push(TimestampedAgentInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                });
                status
            })
        }

        pub fn cancel_pending_invocation(self, idempotency_key: IdempotencyKey) -> Self {
            let entry = OplogEntry::cancel_pending_invocation(idempotency_key.clone()).rounded();
            self.add(entry.clone(), move |mut status| {
                status
                    .pending_invocations
                    .retain(|ti| ti.invocation.idempotency_key() != Some(&idempotency_key));
                status
            })
        }

        pub fn pending_update(
            self,
            update_description: &UpdateDescription,
            extra_status_updates: impl Fn(&mut AgentStatusRecord),
        ) -> Self {
            let entry = OplogEntry::pending_update(update_description.clone()).rounded();
            let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
            self.add(entry.clone(), move |mut status| {
                status
                    .pending_updates
                    .push_back(TimestampedUpdateDescription {
                        timestamp: entry.timestamp(),
                        oplog_index: oplog_idx,
                        description: update_description.clone(),
                    });

                if !status.pending_invocations.is_empty() {
                    status.pending_invocations.pop();
                }

                if let UpdateDescription::SnapshotBased { .. } = update_description {
                    status
                        .skipped_regions
                        .set_override(DeletedRegions::from_regions(vec![
                            OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=oplog_idx),
                        ]));
                }

                extra_status_updates(&mut status);

                status
            })
        }

        pub fn successful_update(
            self,
            update_description: UpdateDescription,
            new_component_size: u64,
            new_active_plugins: &HashSet<EnvironmentPluginGrantId>,
        ) -> Self {
            let old_status = self.entries.first().unwrap().expected_status.clone();
            let entry = OplogEntry::successful_update(
                *update_description.target_revision(),
                new_component_size,
                new_active_plugins.clone(),
            )
            .rounded();
            self.add(entry.clone(), move |mut status| {
                let applied_update = status.pending_updates.pop_front();
                status.successful_updates.push(SuccessfulUpdateRecord {
                    timestamp: entry.timestamp(),
                    target_revision: *update_description.target_revision(),
                });
                status.component_size = new_component_size;
                status.component_revision = *update_description.target_revision();
                status.active_plugins = new_active_plugins.clone();

                if status.skipped_regions.is_overridden() {
                    status.skipped_regions.merge_override();
                    status.total_linear_memory_size = old_status.total_linear_memory_size;
                    status.owned_resources = HashMap::new();
                }

                if let UpdateDescription::SnapshotBased {
                    target_revision, ..
                } = update_description
                {
                    status.component_revision_for_replay = target_revision;
                    status.last_manual_update_snapshot_index =
                        applied_update.map(|au| au.oplog_index);
                    status.last_automatic_snapshot_index = None;
                    status.last_automatic_snapshot_timestamp = None;
                };

                status
            })
        }

        pub fn failed_update(self, update_description: UpdateDescription) -> Self {
            let entry = OplogEntry::failed_update(
                *update_description.target_revision(),
                Some("details".to_string()),
            )
            .rounded();
            self.add(entry.clone(), move |mut status| {
                status.failed_updates.push(FailedUpdateRecord {
                    timestamp: entry.timestamp(),
                    target_revision: *update_description.target_revision(),
                    details: Some("details".to_string()),
                });
                status.pending_updates.pop_front();

                if status.skipped_regions.is_overridden() {
                    status.skipped_regions.drop_override();
                }

                if let UpdateDescription::SnapshotBased {
                    target_revision, ..
                } = update_description
                {
                    status
                        .pending_invocations
                        .retain(|invocation| match invocation {
                            TimestampedAgentInvocation {
                                invocation:
                                    AgentInvocation::ManualUpdate {
                                        target_revision: revision,
                                        ..
                                    },
                                ..
                            } => *revision != target_revision,
                            _ => true,
                        });
                };

                status
            })
        }

        pub fn build(self) -> TestCase {
            TestCase {
                owned_agent_id: self.owned_agent_id,
                entries: self
                    .entries
                    .into_iter()
                    .map(|entry| entry.rounded())
                    .collect(),
            }
        }
    }

    #[derive(Debug, Clone)]
    struct TestEntry {
        oplog_entry: OplogEntry,
        expected_status: AgentStatusRecord,
    }

    impl TestEntry {
        pub fn rounded(self) -> Self {
            TestEntry {
                oplog_entry: self.oplog_entry.rounded(),
                expected_status: self.expected_status,
            }
        }
    }

    #[derive(Debug, Clone)]
    struct TestCase {
        owned_agent_id: OwnedAgentId,
        entries: Vec<TestEntry>,
    }

    impl TestCase {
        pub fn builder(initial_component_version: u64) -> TestCaseBuilder {
            let environment_id = EnvironmentId::new();
            let account_id = AccountId::new();
            let owned_agent_id = OwnedAgentId::new(
                environment_id,
                &AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "test-worker".to_string(),
                },
            );
            TestCaseBuilder::new(
                account_id,
                owned_agent_id,
                initial_component_version.try_into().unwrap(),
            )
        }
    }

    impl HasOplogService for TestCase {
        fn oplog_service(&self) -> Arc<dyn OplogService> {
            Arc::new(self.clone())
        }
    }

    #[async_trait]
    impl OplogService for TestCase {
        async fn create(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _initial_entry: OplogEntry,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn open(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _last_oplog_index: Option<OplogIndex>,
            _initial_worker_metadata: AgentMetadata,
            _last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn get_last_index(&self, _owned_agent_id: &OwnedAgentId) -> OplogIndex {
            OplogIndex::from_u64(self.entries.len() as u64)
        }

        async fn delete(&self, _owned_agent_id: &OwnedAgentId) {
            unreachable!()
        }

        async fn read(
            &self,
            _owned_agent_id: &OwnedAgentId,
            idx: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let mut result = BTreeMap::new();
            let idx_u64: u64 = idx.into();
            for i in idx_u64..(idx_u64 + n) {
                if let Some(entry) = self.entries.get((i - 1) as usize) {
                    result.insert(OplogIndex::from_u64(i), entry.oplog_entry.clone());
                }
            }
            result
        }

        async fn exists(&self, _owned_agent_id: &OwnedAgentId) -> bool {
            unreachable!()
        }

        async fn scan_for_component(
            &self,
            _environment_id: &EnvironmentId,
            _component_id: &ComponentId,
            _cursor: ScanCursor,
            _count: u64,
        ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
            unreachable!()
        }

        async fn upload_raw_payload(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _data: Vec<u8>,
        ) -> Result<RawOplogPayload, String> {
            unreachable!()
        }

        async fn download_raw_payload(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unreachable!()
        }
    }

    impl HasConfig for TestCase {
        fn config(&self) -> Arc<GolemConfig> {
            Arc::new(GolemConfig {
                retry: RetryConfig::default(),
                ..Default::default()
            })
        }
    }

    async fn run_test_case(test_case: TestCase) {
        let final_expected_status = test_case.entries.last().unwrap().expected_status.clone();

        for idx in 0..=test_case.entries.len() {
            let last_known_status = if idx == 0 {
                None
            } else {
                Some(test_case.entries[idx - 1].expected_status.clone())
            };
            let final_status = calculate_last_known_status_for_existing_worker(
                &test_case,
                &test_case.owned_agent_id,
                last_known_status,
            )
            .await;

            assert_eq!(
                final_status, final_expected_status,
                "Calculating the last known status from oplog index {idx}"
            )
        }
    }

    // --------------------------------------------------------------------------
    // U2: Checkpoint state tracking — calculate_oplog_processor_checkpoints
    // --------------------------------------------------------------------------

    #[test]
    fn checkpoint_latest_per_grant_id_wins() {
        let grant_id = EnvironmentPluginGrantId::new();
        let target = AgentId {
            component_id: ComponentId::new(),
            agent_id: "target-worker".to_string(),
        };
        let active_plugins = HashSet::from([grant_id]);
        let deleted_regions = DeletedRegions::default();

        let entries = BTreeMap::from([
            (
                OplogIndex::from_u64(1),
                OplogEntry::OplogProcessorCheckpoint {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id,
                    target_agent_id: target.clone(),
                    confirmed_up_to: OplogIndex::from_u64(5),
                    sending_up_to: OplogIndex::from_u64(10),
                    last_batch_start: OplogIndex::NONE,
                },
            ),
            (
                OplogIndex::from_u64(2),
                OplogEntry::OplogProcessorCheckpoint {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id,
                    target_agent_id: target.clone(),
                    confirmed_up_to: OplogIndex::from_u64(10),
                    sending_up_to: OplogIndex::from_u64(20),
                    last_batch_start: OplogIndex::NONE,
                },
            ),
        ]);

        let result = calculate_oplog_processor_checkpoints(
            HashMap::new(),
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        assert_eq!(result.len(), 1);
        let state = result.get(&grant_id).unwrap();
        assert_eq!(state.confirmed_up_to, OplogIndex::from_u64(10));
        assert_eq!(state.sending_up_to, OplogIndex::from_u64(20));
        assert_eq!(state.target_agent_id, Some(target));
    }

    #[test]
    fn checkpoint_different_plugins_tracked_independently() {
        let grant_id_a = EnvironmentPluginGrantId::new();
        let grant_id_b = EnvironmentPluginGrantId::new();
        let target_a = AgentId {
            component_id: ComponentId::new(),
            agent_id: "target-a".to_string(),
        };
        let target_b = AgentId {
            component_id: ComponentId::new(),
            agent_id: "target-b".to_string(),
        };
        let active_plugins = HashSet::from([grant_id_a, grant_id_b]);
        let deleted_regions = DeletedRegions::default();

        let entries = BTreeMap::from([
            (
                OplogIndex::from_u64(1),
                OplogEntry::OplogProcessorCheckpoint {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id_a,
                    target_agent_id: target_a.clone(),
                    confirmed_up_to: OplogIndex::from_u64(5),
                    sending_up_to: OplogIndex::from_u64(10),
                    last_batch_start: OplogIndex::NONE,
                },
            ),
            (
                OplogIndex::from_u64(2),
                OplogEntry::OplogProcessorCheckpoint {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id_b,
                    target_agent_id: target_b.clone(),
                    confirmed_up_to: OplogIndex::from_u64(3),
                    sending_up_to: OplogIndex::from_u64(7),
                    last_batch_start: OplogIndex::NONE,
                },
            ),
        ]);

        let result = calculate_oplog_processor_checkpoints(
            HashMap::new(),
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        assert_eq!(result.len(), 2);

        let state_a = result.get(&grant_id_a).unwrap();
        assert_eq!(state_a.confirmed_up_to, OplogIndex::from_u64(5));
        assert_eq!(state_a.target_agent_id, Some(target_a));

        let state_b = result.get(&grant_id_b).unwrap();
        assert_eq!(state_b.confirmed_up_to, OplogIndex::from_u64(3));
        assert_eq!(state_b.target_agent_id, Some(target_b));
    }

    #[test]
    fn checkpoint_target_agent_id_preserved() {
        let grant_id = EnvironmentPluginGrantId::new();
        let target = AgentId {
            component_id: ComponentId::new(),
            agent_id: "specific-target".to_string(),
        };
        let active_plugins = HashSet::from([grant_id]);
        let deleted_regions = DeletedRegions::default();

        let entries = BTreeMap::from([(
            OplogIndex::from_u64(1),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
                target_agent_id: target.clone(),
                confirmed_up_to: OplogIndex::from_u64(5),
                sending_up_to: OplogIndex::from_u64(5),
                last_batch_start: OplogIndex::NONE,
            },
        )]);

        let result = calculate_oplog_processor_checkpoints(
            HashMap::new(),
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        let state = result.get(&grant_id).unwrap();
        assert_eq!(state.target_agent_id, Some(target));
    }

    // --------------------------------------------------------------------------
    // U3: Checkpoint cleanup — deactivated plugins evicted, in-flight retained
    // --------------------------------------------------------------------------

    #[test]
    fn deactivated_plugin_evicted_from_checkpoints() {
        let grant_id = EnvironmentPluginGrantId::new();
        // Plugin is no longer active
        let active_plugins = HashSet::new();
        let deleted_regions = DeletedRegions::default();

        // Pre-seed with a checkpoint that is fully confirmed (not in-flight)
        let initial = HashMap::from([(
            grant_id,
            OplogProcessorCheckpointState {
                target_agent_id: Some(AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "old-target".to_string(),
                }),
                confirmed_up_to: OplogIndex::from_u64(10),
                sending_up_to: OplogIndex::from_u64(10),
                last_batch_start: OplogIndex::NONE,
            },
        )]);

        let entries = BTreeMap::new();
        let result = calculate_oplog_processor_checkpoints(
            initial,
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        assert!(
            !result.contains_key(&grant_id),
            "Deactivated plugin with no in-flight batch should be evicted"
        );
    }

    #[test]
    fn deactivated_plugin_retained_when_in_flight() {
        let grant_id = EnvironmentPluginGrantId::new();
        // Plugin is no longer active
        let active_plugins = HashSet::new();
        let deleted_regions = DeletedRegions::default();

        // Pre-seed with a checkpoint that has in-flight batch (sending_up_to > confirmed_up_to)
        let initial = HashMap::from([(
            grant_id,
            OplogProcessorCheckpointState {
                target_agent_id: Some(AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "target".to_string(),
                }),
                confirmed_up_to: OplogIndex::from_u64(5),
                sending_up_to: OplogIndex::from_u64(15),
                last_batch_start: OplogIndex::NONE,
            },
        )]);

        let entries = BTreeMap::new();
        let result = calculate_oplog_processor_checkpoints(
            initial,
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        assert!(
            result.contains_key(&grant_id),
            "Deactivated plugin with in-flight batch should be retained"
        );
        let state = result.get(&grant_id).unwrap();
        assert_eq!(state.confirmed_up_to, OplogIndex::from_u64(5));
        assert_eq!(state.sending_up_to, OplogIndex::from_u64(15));
    }

    #[test]
    fn successful_update_drops_old_grant_retains_new() {
        let old_grant = EnvironmentPluginGrantId::new();
        let new_grant = EnvironmentPluginGrantId::new();
        let deleted_regions = DeletedRegions::default();
        // After SuccessfulUpdate, only new_grant is active
        let active_plugins = HashSet::from([new_grant]);

        // Pre-seed old checkpoint
        let initial = HashMap::from([(
            old_grant,
            OplogProcessorCheckpointState {
                target_agent_id: Some(AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "old".to_string(),
                }),
                confirmed_up_to: OplogIndex::from_u64(10),
                sending_up_to: OplogIndex::from_u64(10),
                last_batch_start: OplogIndex::NONE,
            },
        )]);

        let entries = BTreeMap::from([
            (
                OplogIndex::from_u64(11),
                OplogEntry::SuccessfulUpdate {
                    timestamp: Timestamp::now_utc(),
                    target_revision: ComponentRevision::new(2).unwrap(),
                    new_component_size: 200,
                    new_active_plugins: HashSet::from([new_grant]),
                },
            ),
            (
                OplogIndex::from_u64(12),
                OplogEntry::OplogProcessorCheckpoint {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: new_grant,
                    target_agent_id: AgentId {
                        component_id: ComponentId::new(),
                        agent_id: "new-target".to_string(),
                    },
                    confirmed_up_to: OplogIndex::from_u64(12),
                    sending_up_to: OplogIndex::from_u64(12),
                    last_batch_start: OplogIndex::NONE,
                },
            ),
        ]);

        let result = calculate_oplog_processor_checkpoints(
            initial,
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        assert!(
            !result.contains_key(&old_grant),
            "Old grant should be dropped after SuccessfulUpdate with different active set"
        );
        assert!(
            result.contains_key(&new_grant),
            "New grant should be present"
        );
    }

    // --------------------------------------------------------------------------
    // U4: Activation initialization — ActivatePlugin seeds checkpoint
    // --------------------------------------------------------------------------

    #[test]
    fn activate_plugin_initializes_checkpoint() {
        let grant_id = EnvironmentPluginGrantId::new();
        let active_plugins = HashSet::from([grant_id]);
        let deleted_regions = DeletedRegions::default();

        let activation_index = OplogIndex::from_u64(5);
        let entries = BTreeMap::from([(
            activation_index,
            OplogEntry::ActivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        )]);

        let result = calculate_oplog_processor_checkpoints(
            HashMap::new(),
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        assert_eq!(result.len(), 1);
        let state = result.get(&grant_id).unwrap();
        assert_eq!(
            state.confirmed_up_to, activation_index,
            "confirmed_up_to should be set to the activation index"
        );
        assert_eq!(
            state.sending_up_to, activation_index,
            "sending_up_to should be set to the activation index"
        );
        assert_eq!(
            state.target_agent_id, None,
            "target_agent_id should be None for freshly activated plugin"
        );
    }

    #[test]
    fn activate_plugin_does_not_overwrite_existing_checkpoint() {
        let grant_id = EnvironmentPluginGrantId::new();
        let target = AgentId {
            component_id: ComponentId::new(),
            agent_id: "existing-target".to_string(),
        };
        let active_plugins = HashSet::from([grant_id]);
        let deleted_regions = DeletedRegions::default();

        // Pre-seed with an existing checkpoint
        let initial = HashMap::from([(
            grant_id,
            OplogProcessorCheckpointState {
                target_agent_id: Some(target.clone()),
                confirmed_up_to: OplogIndex::from_u64(3),
                sending_up_to: OplogIndex::from_u64(8),
                last_batch_start: OplogIndex::NONE,
            },
        )]);

        let entries = BTreeMap::from([(
            OplogIndex::from_u64(10),
            OplogEntry::ActivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        )]);

        let result = calculate_oplog_processor_checkpoints(
            initial,
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        let state = result.get(&grant_id).unwrap();
        assert_eq!(
            state.target_agent_id,
            Some(target),
            "ActivatePlugin should not overwrite existing checkpoint (or_insert semantics)"
        );
        assert_eq!(state.confirmed_up_to, OplogIndex::from_u64(3));
        assert_eq!(state.sending_up_to, OplogIndex::from_u64(8));
    }

    #[test]
    fn deactivate_then_reactivate_seeds_new_checkpoint() {
        let grant_id = EnvironmentPluginGrantId::new();
        let active_plugins = HashSet::from([grant_id]);
        let deleted_regions = DeletedRegions::default();

        let entries = BTreeMap::from([
            (
                OplogIndex::from_u64(3),
                OplogEntry::ActivatePlugin {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id,
                },
            ),
            (
                OplogIndex::from_u64(7),
                OplogEntry::DeactivatePlugin {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id,
                },
            ),
            (
                OplogIndex::from_u64(10),
                OplogEntry::ActivatePlugin {
                    timestamp: Timestamp::now_utc(),
                    plugin_grant_id: grant_id,
                },
            ),
        ]);

        let result = calculate_oplog_processor_checkpoints(
            HashMap::new(),
            &active_plugins,
            &deleted_regions,
            &entries,
        );

        let state = result.get(&grant_id).unwrap();
        assert_eq!(
            state.confirmed_up_to,
            OplogIndex::from_u64(10),
            "After deactivate+reactivate, checkpoint should be seeded at new activation index"
        );
        assert_eq!(state.sending_up_to, OplogIndex::from_u64(10));
        assert_eq!(state.target_agent_id, None);
    }

    fn make_fs_entry(idx: u64, delta: i64) -> (OplogIndex, OplogEntry) {
        (
            OplogIndex::from_u64(idx),
            OplogEntry::FilesystemStorageUsageUpdate {
                timestamp: Timestamp::now_utc(),
                delta,
            },
        )
    }

    fn make_create_entry(idx: u64) -> (OplogIndex, OplogEntry) {
        use golem_common::base_model::account::AccountId;
        use golem_common::base_model::component::{ComponentId, ComponentRevision};
        use golem_common::base_model::environment::EnvironmentId;
        use golem_common::model::AgentId;
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "w".to_string(),
        };
        (
            OplogIndex::from_u64(idx),
            OplogEntry::create(
                agent_id,
                ComponentRevision::INITIAL,
                vec![],
                EnvironmentId::new(),
                AccountId::new(),
                None,
                0,
                0,
                Default::default(),
                Default::default(),
                vec![],
                None,
            ),
        )
    }

    /// `FilesystemStorageUsageUpdate` entries inside a deleted (skipped) region
    /// are excluded from `current_filesystem_storage_usage`. Only live entries count.
    #[test]
    fn filesystem_storage_usage_in_deleted_region_is_skipped() {
        use golem_common::model::regions::{DeletedRegionsBuilder, OplogRegion};
        let mut builder = DeletedRegionsBuilder::default();
        // Mark indices 2..=4 as deleted.
        builder.add(OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(4),
        });
        let deleted = builder.build();

        let entries: BTreeMap<OplogIndex, OplogEntry> = BTreeMap::from([
            make_fs_entry(2, 1024), // deleted — must be skipped
            make_fs_entry(3, 2048), // deleted — must be skipped
            make_fs_entry(5, 512),  // live
        ]);

        let result = super::calculate_current_filesystem_storage_usage(0, &deleted, &entries);
        assert_eq!(
            result, 512,
            "only the live entry outside the deleted region counts"
        );
    }

    /// A `Create` entry mid-oplog resets `current_filesystem_storage_usage` to zero,
    /// discarding usage accumulated before it (including the seed).
    #[test]
    fn filesystem_storage_usage_reset_to_zero_on_create() {
        let deleted = DeletedRegions::default();

        let entries: BTreeMap<OplogIndex, OplogEntry> = BTreeMap::from([
            make_fs_entry(1, 1024), // before Create → should be wiped
            make_create_entry(2),   // resets counter to 0
            make_fs_entry(3, 512),  // after Create → counts
        ]);

        // Seed with prior usage to confirm Create overrides the seed too.
        let result = super::calculate_current_filesystem_storage_usage(999, &deleted, &entries);
        assert_eq!(
            result, 512,
            "Create must reset usage to 0 before accumulating post-Create deltas"
        );
    }

    /// `current` seed is used as the starting value when there are no `Create`
    /// entries and no deleted regions.
    #[test]
    fn filesystem_storage_usage_uses_seed_when_no_create() {
        let deleted = DeletedRegions::default();

        let entries: BTreeMap<OplogIndex, OplogEntry> = BTreeMap::from([make_fs_entry(1, 512)]);

        let result = super::calculate_current_filesystem_storage_usage(1024, &deleted, &entries);
        assert_eq!(result, 1536, "seed + delta");
    }
}
