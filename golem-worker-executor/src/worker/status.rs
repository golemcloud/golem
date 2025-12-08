use crate::services::{HasConfig, HasOplogService};
use async_recursion::async_recursion;
use golem_common::base_model::OplogIndex;
use golem_common::model::component::{ComponentRevision, PluginPriority};
use golem_common::model::oplog::{
    OplogEntry, TimestampedUpdateDescription, UpdateDescription, WorkerError, WorkerResourceId,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, RetryConfig, SuccessfulUpdateRecord,
    TimestampedWorkerInvocation, WorkerInvocation, WorkerResourceDescription, WorkerStatus,
    WorkerStatusRecord,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Like calculate_last_known_status, but assumes that the oplog exists and has at least a Create entry in it.
pub async fn calculate_last_known_status_for_existing_worker<T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    last_known: Option<WorkerStatusRecord>,
) -> WorkerStatusRecord
where
    T: HasOplogService + HasConfig + Sync,
{
    calculate_last_known_status(this, owned_worker_id, last_known)
        .await
        .expect("Failed to calculate oplog index for existing worker")
}

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker status.
#[async_recursion]
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    last_known: Option<WorkerStatusRecord>,
) -> Option<WorkerStatusRecord>
where
    T: HasOplogService + HasConfig + Sync,
{
    let last_known = last_known.unwrap_or_default();

    let last_oplog_index = this.oplog_service().get_last_index(owned_worker_id).await;
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
                owned_worker_id,
                last_known.oplog_idx.next(),
                last_oplog_index,
            )
            .await;

        let final_status =
            update_status_with_new_entries(last_known, new_entries, &this.config().retry);

        if let Some(final_status) = final_status {
            Some(final_status)
        } else {
            calculate_last_known_status(this, owned_worker_id, None).await
        }
    }
}

// update a worker status with new entries. Returns None if the status cannot be calculated from the new entries alone and needs to be recalculated from the beginning.
pub fn update_status_with_new_entries(
    last_known: WorkerStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    // TODO: changing the retry policy will cause inconsistencies when reading existing oplogs.
    default_retry_policy: &RetryConfig,
) -> Option<WorkerStatusRecord> {
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

    let pending_invocations =
        calculate_pending_invocations(last_known.pending_invocations, &new_entries);
    let (
        pending_updates,
        failed_updates,
        successful_updates,
        component_revision,
        component_size,
        component_revision_for_replay,
    ) = calculate_update_fields(
        last_known.pending_updates,
        last_known.failed_updates,
        last_known.successful_updates,
        last_known.component_revision,
        last_known.component_size,
        last_known.component_revision_for_replay,
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

    let owned_resources =
        collect_resources(last_known.owned_resources, &skipped_regions, &new_entries);

    let active_plugins = calculate_active_plugins(active_plugins, &deleted_regions, &new_entries);

    let result = WorkerStatusRecord {
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
        active_plugins,
        deleted_regions,
        component_revision_for_replay,
        current_retry_count,
    };

    Some(result)
}

fn calculate_latest_worker_status(
    mut current_status: WorkerStatus,
    mut current_retry_count: HashMap<OplogIndex, u32>,
    mut current_retry_policy: Option<RetryConfig>,
    default_retry_policy: &RetryConfig,
    skipped_regions: &DeletedRegions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (WorkerStatus, HashMap<OplogIndex, u32>, Option<RetryConfig>) {
    for (idx, entry) in entries {
        // Skipping entries in skipped regions, as they are skipped during replay too
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        // Errors are counted in skipped regions too (but not in deleted ones),
        // otherwise we would not be able to know how many times we retried failures in atomic regions
        if !deleted_regions.is_in_deleted_region(*idx) {
            if let OplogEntry::Error {
                error, retry_from, ..
            } = entry
            {
                let new_count = current_retry_count
                    .get(retry_from)
                    .copied()
                    .unwrap_or_default()
                    + 1;
                current_retry_count.insert(*retry_from, new_count);
                if is_worker_error_retriable(
                    current_retry_policy
                        .as_ref()
                        .unwrap_or(default_retry_policy),
                    error,
                    new_count,
                ) {
                    current_status = WorkerStatus::Retrying;
                } else {
                    current_status = WorkerStatus::Failed;
                }
            }
        }

        match entry {
            OplogEntry::Create { .. } => {
                current_status = WorkerStatus::Idle;
            }
            OplogEntry::ImportedFunctionInvoked { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::ExportedFunctionInvoked { .. } => {
                current_status = WorkerStatus::Running;
                current_retry_count.clear();
            }
            OplogEntry::ExportedFunctionCompleted { .. } => {
                current_status = WorkerStatus::Idle;
                current_retry_count.clear();
            }
            OplogEntry::Suspend { .. } => {
                current_status = WorkerStatus::Suspended;
            }
            OplogEntry::NoOp { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::Jump { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::Interrupted { .. } => {
                current_status = WorkerStatus::Interrupted;
            }
            OplogEntry::Exited { .. } => {
                current_status = WorkerStatus::Exited;
            }
            OplogEntry::ChangeRetryPolicy { new_policy, .. } => {
                current_retry_policy = Some(new_policy.clone());
                current_status = WorkerStatus::Running;
            }
            OplogEntry::BeginAtomicRegion { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::EndAtomicRegion { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::BeginRemoteWrite { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::EndRemoteWrite { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::PendingWorkerInvocation { .. } => {}
            OplogEntry::PendingUpdate { .. } => {
                if current_status == WorkerStatus::Failed {
                    current_status = WorkerStatus::Retrying;
                }
            }
            OplogEntry::FailedUpdate { .. } => {}
            OplogEntry::SuccessfulUpdate { .. } => {}
            OplogEntry::GrowMemory { .. } => {}
            OplogEntry::CreateResource { .. } => {}
            OplogEntry::DropResource { .. } => {}
            OplogEntry::Log { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::Restart { .. } => {
                current_status = WorkerStatus::Idle;
            }
            OplogEntry::ActivatePlugin { .. } => {}
            OplogEntry::DeactivatePlugin { .. } => {}
            OplogEntry::Revert { .. } => {}
            OplogEntry::CancelPendingInvocation { .. } => {}
            OplogEntry::StartSpan { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::FinishSpan { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::SetSpanAttribute { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::ChangePersistenceLevel { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::BeginRemoteTransaction { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::PreCommitRemoteTransaction { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::PreRollbackRemoteTransaction { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::CommittedRemoteTransaction { .. } => {
                current_status = WorkerStatus::Running;
            }
            OplogEntry::RolledBackRemoteTransaction { .. } => {
                current_status = WorkerStatus::Running;
            }
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

fn calculate_pending_invocations(
    initial: Vec<TimestampedWorkerInvocation>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<TimestampedWorkerInvocation> {
    let mut result = initial;
    for entry in entries.values() {
        // Here we are handling two categories of oplog entries:
        // - "input" entries adding items to pending queues (PendingWorkerInvocation, PendingUpdate)
        // - "output" entries removing items from pending queues when they got processed (ExportedFunctionInvoked, SuccessfulUpdate, FailedUpdate)
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
            OplogEntry::PendingWorkerInvocation {
                timestamp,
                invocation,
                ..
            } => {
                result.push(TimestampedWorkerInvocation {
                    timestamp: *timestamp,
                    invocation: invocation.clone(),
                });
            }
            OplogEntry::ExportedFunctionInvoked {
                idempotency_key, ..
            } => {
                result.retain(|invocation| match invocation {
                    TimestampedWorkerInvocation {
                        invocation:
                            WorkerInvocation::ExportedFunction {
                                idempotency_key: key,
                                ..
                            },
                        ..
                    } => key != idempotency_key,
                    _ => true,
                });
            }
            OplogEntry::PendingUpdate {
                description:
                    UpdateDescription::SnapshotBased {
                        target_revision, ..
                    },
                ..
            } => result.retain(|invocation| match invocation {
                TimestampedWorkerInvocation {
                    invocation:
                        WorkerInvocation::ManualUpdate {
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
                TimestampedWorkerInvocation {
                    invocation:
                        WorkerInvocation::ManualUpdate {
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
                result.retain(|invocation| match invocation {
                    TimestampedWorkerInvocation {
                        invocation:
                            WorkerInvocation::ExportedFunction {
                                idempotency_key: key,
                                ..
                            },
                        ..
                    } => key != idempotency_key,
                    _ => true,
                });
            }
            _ => {}
        }
    }
    result
}

fn calculate_update_fields(
    initial_pending_updates: VecDeque<TimestampedUpdateDescription>,
    initial_failed_updates: Vec<FailedUpdateRecord>,
    initial_successful_updates: Vec<SuccessfulUpdateRecord>,
    initial_version: ComponentRevision,
    initial_component_size: u64,
    initial_component_version_for_replay: ComponentRevision,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    VecDeque<TimestampedUpdateDescription>,
    Vec<FailedUpdateRecord>,
    Vec<SuccessfulUpdateRecord>,
    ComponentRevision,
    u64,
    ComponentRevision,
) {
    let mut pending_updates = initial_pending_updates;
    let mut failed_updates = initial_failed_updates;
    let mut successful_updates = initial_successful_updates;
    let mut revision = initial_version;
    let mut size = initial_component_size;
    let mut component_revision_for_replay = initial_component_version_for_replay;

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

                let applied_update = pending_updates.pop_front();
                if matches!(
                    applied_update,
                    Some(TimestampedUpdateDescription {
                        description: UpdateDescription::SnapshotBased { .. },
                        ..
                    })
                ) {
                    component_revision_for_replay = *target_revision
                }
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
            OplogEntry::ExportedFunctionInvoked {
                idempotency_key, ..
            } => {
                current_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::ExportedFunctionCompleted { .. } => {
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

fn collect_resources(
    initial: HashMap<WorkerResourceId, WorkerResourceDescription>,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashMap<WorkerResourceId, WorkerResourceDescription> {
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
                    WorkerResourceDescription {
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
    initial: HashSet<PluginPriority>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<PluginPriority> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::ActivatePlugin {
                plugin_priority, ..
            } => {
                result.insert(*plugin_priority);
            }
            OplogEntry::DeactivatePlugin {
                plugin_priority, ..
            } => {
                result.remove(plugin_priority);
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

fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &WorkerError,
    retry_count: u32,
) -> bool {
    match error {
        WorkerError::Unknown(_) => retry_count < retry_config.max_attempts,
        WorkerError::InvalidRequest(_) => false,
        WorkerError::StackOverflow => false,
        WorkerError::OutOfMemory => true,
        WorkerError::ExceededMemoryLimit => false,
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
    };
    use async_trait::async_trait;
    use golem_common::base_model::OplogIndex;
    use golem_common::model::account::AccountId;
    use golem_common::model::component::{ComponentId, ComponentRevision, PluginPriority};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::invocation_context::{InvocationContextStack, TraceId};
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{
        DurableFunctionType, HostRequest, HostRequestNoInput, HostResponse, OplogEntry,
        OplogPayload, PayloadId, RawOplogPayload, TimestampedUpdateDescription, UpdateDescription,
    };
    use golem_common::model::regions::{DeletedRegions, OplogRegion};
    use golem_common::model::{
        FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, RetryConfig, ScanCursor,
        SuccessfulUpdateRecord, Timestamp, TimestampedWorkerInvocation, WorkerId, WorkerInvocation,
        WorkerMetadata, WorkerStatus, WorkerStatusRecord,
    };
    use golem_common::read_only_lock;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_wasm::{IntoValueAndType, Value, ValueAndType};
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
    async fn invocation_results() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .exported_function_completed(None, k1)
            .exported_function_invoked("b", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results_with_jump() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .jump(OplogIndex::from_u64(2))
            .exported_function_completed(None, k1)
            .exported_function_invoked("b", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results_with_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .exported_function_completed(None, k1)
            .exported_function_invoked("b", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .revert(OplogIndex::from_u64(5))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_auto_update_for_running() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic {
            target_revision: ComponentRevision(2),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_update(&update1, |_| {})
            .successful_update(update1, 2000, &HashSet::new())
            .exported_function_completed(None, k1)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn auto_update_for_running_with_jump() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic {
            target_revision: ComponentRevision(2),
        };
        let update2 = UpdateDescription::Automatic {
            target_revision: ComponentRevision(3),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .imported_function_invoked(
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
            .exported_function_completed(None, k1)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_update() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision(2),
            payload: OplogPayload::Inline(Box::new(vec![])),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(WorkerInvocation::ManualUpdate {
                target_revision: ComponentRevision(2),
            })
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .exported_function_completed(None, k1)
            .pending_update(&update1, |status| status.total_linear_memory_size = 200)
            .successful_update(update1, 2000, &HashSet::new())
            .exported_function_invoked("c", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_failed_update() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision(2),
            payload: OplogPayload::Inline(Box::new(vec![])),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(WorkerInvocation::ManualUpdate {
                target_revision: ComponentRevision(2),
            })
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .exported_function_completed(None, k1)
            .pending_update(&update1, |_| {})
            .failed_update(update1)
            .exported_function_invoked("c", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_failed_update_during_snapshot() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update2 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision(2),
            payload: OplogPayload::Inline(Box::new(vec![])),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(WorkerInvocation::ManualUpdate {
                target_revision: ComponentRevision(2),
            })
            .failed_update(update2)
            .exported_function_invoked("c", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn auto_update_for_running_with_jump_and_revert() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic {
            target_revision: ComponentRevision(2),
        };
        let update2 = UpdateDescription::Automatic {
            target_revision: ComponentRevision(3),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .imported_function_invoked(
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
            .exported_function_completed(None, k1)
            .revert(OplogIndex::from_u64(3))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_update_with_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision(2),
            payload: OplogPayload::Inline(Box::new(vec![])),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(WorkerInvocation::ManualUpdate {
                target_revision: ComponentRevision(2),
            })
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .exported_function_completed(None, k1)
            .pending_update(&update1, |_| {})
            .successful_update(update1, 2000, &HashSet::new())
            .exported_function_invoked("c", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .revert(OplogIndex::from_u64(4))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn multiple_manual_updates_with_jump_and_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision(2),
            payload: OplogPayload::Inline(Box::new(vec![])),
        };
        let update2 = UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision(2),
            payload: OplogPayload::Inline(Box::new(vec![])),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .pending_invocation(WorkerInvocation::ManualUpdate {
                target_revision: ComponentRevision(2),
            })
            .imported_function_invoked(
                "b",
                HostRequest::NoInput(HostRequestNoInput {}),
                HostResponse::Custom(1.into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .exported_function_completed(None, k1)
            .pending_update(&update1, |_| {})
            .failed_update(update1)
            .exported_function_invoked("c", vec![], k2.clone())
            .pending_invocation(WorkerInvocation::ManualUpdate {
                target_revision: ComponentRevision(2),
            })
            .exported_function_completed(None, k2)
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
            .exported_function_invoked("a", vec![], k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .pending_invocation(WorkerInvocation::ExportedFunction {
                idempotency_key: k2.clone(),
                full_function_name: "b".to_string(),
                function_input: vec![Value::Bool(true)],
                invocation_context: InvocationContextStack::fresh(),
            })
            .exported_function_completed(None, k1.clone())
            .exported_function_invoked("b", vec![], k2.clone())
            .exported_function_completed(None, k2.clone())
            .revert(OplogIndex::from_u64(5))
            .exported_function_completed(None, k1)
            .exported_function_invoked("b", vec![], k2.clone())
            .exported_function_completed(None, k2)
            .revert(OplogIndex::from_u64(2))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn cancel_pending_invocation() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .pending_invocation(WorkerInvocation::ExportedFunction {
                idempotency_key: k1.clone(),
                full_function_name: "a".to_string(),
                function_input: vec![Value::Bool(true)],
                invocation_context: InvocationContextStack::fresh(),
            })
            .pending_invocation(WorkerInvocation::ExportedFunction {
                idempotency_key: k2.clone(),
                full_function_name: "b".to_string(),
                function_input: vec![],
                invocation_context: InvocationContextStack::fresh(),
            })
            .cancel_pending_invocation(k1)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn non_existing_oplog() {
        let environment_id = EnvironmentId::new();
        let owned_worker_id = OwnedWorkerId::new(
            &environment_id,
            &WorkerId {
                component_id: ComponentId::new(),
                worker_name: "test-worker".to_string(),
            },
        );
        let test_case = TestCase {
            owned_worker_id: owned_worker_id.clone(),
            entries: vec![],
        };

        let result = calculate_last_known_status(&test_case, &owned_worker_id, None).await;
        assert2::assert!(let None = result);
    }

    struct TestCaseBuilder {
        entries: Vec<TestEntry>,
        previous_status_record: WorkerStatusRecord,
        owned_worker_id: OwnedWorkerId,
    }

    impl TestCaseBuilder {
        pub fn new(
            account_id: AccountId,
            owned_worker_id: OwnedWorkerId,
            component_revision: ComponentRevision,
        ) -> Self {
            let status = WorkerStatusRecord {
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
                        owned_worker_id.worker_id(),
                        component_revision,
                        vec![],
                        owned_worker_id.environment_id(),
                        account_id,
                        None,
                        100,
                        200,
                        HashSet::new(),
                        BTreeMap::new(),
                        None,
                    ),
                    expected_status: status.clone(),
                }],
                previous_status_record: status,
                owned_worker_id,
            }
        }

        pub fn add(
            mut self,
            entry: OplogEntry,
            update: impl FnOnce(WorkerStatusRecord) -> WorkerStatusRecord,
        ) -> Self {
            self.previous_status_record.oplog_idx = self.previous_status_record.oplog_idx.next();
            self.previous_status_record = update(self.previous_status_record);
            self.entries.push(TestEntry {
                oplog_entry: entry,
                expected_status: self.previous_status_record.clone(),
            });
            self
        }

        pub fn exported_function_invoked(
            self,
            function_name: &str,
            request: Vec<Value>,
            idempotency_key: IdempotencyKey,
        ) -> Self {
            self.add(
                OplogEntry::ExportedFunctionInvoked {
                    timestamp: Timestamp::now_utc(),
                    function_name: function_name.to_string(),
                    request: OplogPayload::Inline(Box::new(request)),
                    idempotency_key: idempotency_key.clone(),
                    trace_id: TraceId::generate(),
                    trace_states: vec![],
                    invocation_context: vec![],
                },
                move |mut status| {
                    status.current_idempotency_key = Some(idempotency_key);
                    status.status = WorkerStatus::Running;
                    if !status.pending_invocations.is_empty() {
                        status.pending_invocations.pop();
                    }
                    status
                },
            )
        }

        pub fn exported_function_completed(
            self,
            response: Option<ValueAndType>,
            idempotency_key: IdempotencyKey,
        ) -> Self {
            self.add(
                OplogEntry::ExportedFunctionCompleted {
                    timestamp: Timestamp::now_utc(),
                    response: OplogPayload::Inline(Box::new(response)),
                    consumed_fuel: 0,
                },
                move |mut status| {
                    status
                        .invocation_results
                        .insert(idempotency_key, status.oplog_idx);
                    status.current_idempotency_key = None;
                    status.status = WorkerStatus::Idle;
                    status
                },
            )
        }

        pub fn imported_function_invoked(
            self,
            name: &str,
            i: HostRequest,
            o: HostResponse,
            func_type: DurableFunctionType,
        ) -> Self {
            self.add(
                OplogEntry::ImportedFunctionInvoked {
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

                status
            })
        }

        pub fn pending_invocation(self, invocation: WorkerInvocation) -> Self {
            let entry = OplogEntry::pending_worker_invocation(invocation.clone()).rounded();
            self.add(entry.clone(), move |mut status| {
                status
                    .pending_invocations
                    .push(TimestampedWorkerInvocation {
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
                    .retain(|invocation| match invocation {
                        TimestampedWorkerInvocation {
                            invocation:
                                WorkerInvocation::ExportedFunction {
                                    idempotency_key: key,
                                    ..
                                },
                            ..
                        } => key != &idempotency_key,
                        _ => true,
                    });
                status
            })
        }

        pub fn pending_update(
            self,
            update_description: &UpdateDescription,
            extra_status_updates: impl Fn(&mut WorkerStatusRecord),
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
            new_active_plugins: &HashSet<PluginPriority>,
        ) -> Self {
            let old_status = self.entries.first().unwrap().expected_status.clone();
            let entry = OplogEntry::successful_update(
                *update_description.target_revision(),
                new_component_size,
                new_active_plugins.clone(),
            )
            .rounded();
            self.add(entry.clone(), move |mut status| {
                let _ = status.pending_updates.pop_front();
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
                            TimestampedWorkerInvocation {
                                invocation:
                                    WorkerInvocation::ManualUpdate {
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
                owned_worker_id: self.owned_worker_id,
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
        expected_status: WorkerStatusRecord,
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
        owned_worker_id: OwnedWorkerId,
        entries: Vec<TestEntry>,
    }

    impl TestCase {
        pub fn builder(initial_component_version: u64) -> TestCaseBuilder {
            let environment_id = EnvironmentId::new();
            let account_id = AccountId::new();
            let owned_worker_id = OwnedWorkerId::new(
                &environment_id,
                &WorkerId {
                    component_id: ComponentId::new(),
                    worker_name: "test-worker".to_string(),
                },
            );
            TestCaseBuilder::new(
                account_id,
                owned_worker_id,
                ComponentRevision(initial_component_version),
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
            _owned_worker_id: &OwnedWorkerId,
            _initial_entry: OplogEntry,
            _initial_worker_metadata: WorkerMetadata,
            _last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn open(
            &self,
            _owned_worker_id: &OwnedWorkerId,
            _last_oplog_index: OplogIndex,
            _initial_worker_metadata: WorkerMetadata,
            _last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
            _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn get_last_index(&self, _owned_worker_id: &OwnedWorkerId) -> OplogIndex {
            OplogIndex::from_u64(self.entries.len() as u64)
        }

        async fn delete(&self, _owned_worker_id: &OwnedWorkerId) {
            unreachable!()
        }

        async fn read(
            &self,
            _owned_worker_id: &OwnedWorkerId,
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

        async fn exists(&self, _owned_worker_id: &OwnedWorkerId) -> bool {
            unreachable!()
        }

        async fn scan_for_component(
            &self,
            _environment_id: &EnvironmentId,
            _component_id: &ComponentId,
            _cursor: ScanCursor,
            _count: u64,
        ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError> {
            unreachable!()
        }

        async fn upload_raw_payload(
            &self,
            _owned_worker_id: &OwnedWorkerId,
            _data: Vec<u8>,
        ) -> Result<RawOplogPayload, String> {
            unreachable!()
        }

        async fn download_raw_payload(
            &self,
            _owned_worker_id: &OwnedWorkerId,
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
                &test_case.owned_worker_id,
                last_known_status,
            )
            .await;

            assert_eq!(
                final_status, final_expected_status,
                "Calculating the last known status from oplog index {idx}"
            )
        }
    }
}
