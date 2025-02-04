use crate::error::GolemError;
use crate::services::{HasConfig, HasOplogService};
use crate::worker::is_worker_error_retriable;
use async_recursion::async_recursion;
use golem_common::base_model::{OplogIndex, PluginInstallationId};
use golem_common::model::oplog::{
    OplogEntry, TimestampedUpdateDescription, UpdateDescription, WorkerResourceId,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, RetryConfig, SuccessfulUpdateRecord,
    TimestampedWorkerInvocation, WorkerInvocation, WorkerMetadata, WorkerResourceDescription,
    WorkerStatus, WorkerStatusRecord, WorkerStatusRecordExtensions,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

// TODO: check all usages and replace with Worker::get_metadata where it is possible
/// Gets the last cached worker status record and the new oplog entries and calculates the new worker status.
#[async_recursion]
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    metadata: &Option<WorkerMetadata>,
) -> Result<WorkerStatusRecord, GolemError>
where
    T: HasOplogService + HasConfig + Sync,
{
    let last_known = metadata
        .as_ref()
        .map(|metadata| metadata.last_known_status.clone())
        .unwrap_or_default();

    let last_oplog_index = this.oplog_service().get_last_index(owned_worker_id).await;

    if last_known.oplog_idx == last_oplog_index {
        Ok(last_known)
    } else {
        let new_entries: BTreeMap<OplogIndex, OplogEntry> = this
            .oplog_service()
            .read_range(
                owned_worker_id,
                last_known.oplog_idx.next(),
                last_oplog_index,
            )
            .await;

        let mut initial_deleted_regions = last_known.deleted_regions.clone();
        if initial_deleted_regions.is_overridden() {
            initial_deleted_regions.drop_override(); // TODO: this seems to be incorrect
        }
        let deleted_regions = calculate_deleted_regions(initial_deleted_regions, &new_entries);

        // If the last known status is from a deleted region based on the latest deleted region status,
        // we cannot fold the new status from the new entries only, and need to recalculate the whole status
        // (Note that this is a rare case - for Jumps, this is not happening if the executor successfully writes out
        // the new status before performing the jump; for Reverts, the status is recalculated anyway, but only once, when
        // the revert is applied)
        if deleted_regions.is_in_deleted_region(last_known.oplog_idx) {
            calculate_last_known_status(this, owned_worker_id, &None).await
        } else {
            let active_plugins = last_known.active_plugins().clone();

            let overridden_retry_config = calculate_overridden_retry_policy(
                last_known.overridden_retry_config.clone(),
                &deleted_regions,
                &new_entries,
            );
            let status = calculate_latest_worker_status(
                &last_known.status,
                &this.config().retry,
                last_known.overridden_retry_config.clone(),
                &deleted_regions,
                &new_entries,
            );

            let pending_invocations =
                calculate_pending_invocations(last_known.pending_invocations, &new_entries);
            let (
                pending_updates,
                failed_updates,
                successful_updates,
                component_version,
                component_size,
            ) = calculate_update_fields(
                last_known.pending_updates,
                last_known.failed_updates,
                last_known.successful_updates,
                last_known.component_version,
                last_known.component_size,
                &new_entries,
            );

            // TODO: this seems to be incorrect
            // if let Some(TimestampedUpdateDescription {
            //                 oplog_index,
            //                 description: UpdateDescription::SnapshotBased { .. },
            //                 ..
            //             }) = pending_updates.front()
            // {
            //     deleted_regions.set_override(DeletedRegions::from_regions(vec![
            //         OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=*oplog_index),
            //     ]));
            // }

            let (invocation_results, current_idempotency_key) = calculate_invocation_results(
                last_known.invocation_results,
                last_known.current_idempotency_key,
                &deleted_regions,
                &new_entries,
            );

            let total_linear_memory_size = calculate_total_linear_memory_size(
                last_known.total_linear_memory_size,
                &deleted_regions,
                &new_entries,
            );

            let owned_resources = calculate_owned_resources(
                last_known.owned_resources,
                &deleted_regions,
                &new_entries,
            );

            let active_plugins =
                calculate_active_plugins(active_plugins, &deleted_regions, &new_entries);

            let result = WorkerStatusRecord {
                oplog_idx: last_oplog_index,
                status,
                overridden_retry_config,
                pending_invocations,
                deleted_regions,
                pending_updates,
                failed_updates,
                successful_updates,
                invocation_results,
                current_idempotency_key,
                component_version,
                component_size,
                owned_resources,
                total_linear_memory_size,
                extensions: WorkerStatusRecordExtensions::Extension1 { active_plugins },
            };
            Ok(result)
        }
    }
}

fn calculate_latest_worker_status(
    initial: &WorkerStatus,
    default_retry_policy: &RetryConfig,
    initial_retry_policy: Option<RetryConfig>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> WorkerStatus {
    let mut result = initial.clone();
    let mut last_error_count = 0;
    let mut current_retry_policy = initial_retry_policy;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions, as they are skipped during replay too
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        if !matches!(entry, OplogEntry::Error { .. }) {
            last_error_count = 0;
        }

        match entry {
            OplogEntry::Create { .. } => {
                result = WorkerStatus::Idle;
            }
            OplogEntry::ImportedFunctionInvokedV1 { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::ImportedFunctionInvoked { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::ExportedFunctionInvoked { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::ExportedFunctionCompleted { .. } => {
                result = WorkerStatus::Idle;
            }
            OplogEntry::Suspend { .. } => {
                result = WorkerStatus::Suspended;
            }
            OplogEntry::Error { error, .. } => {
                last_error_count += 1;

                if is_worker_error_retriable(
                    current_retry_policy
                        .as_ref()
                        .unwrap_or(default_retry_policy),
                    error,
                    last_error_count,
                ) {
                    result = WorkerStatus::Retrying;
                } else {
                    result = WorkerStatus::Failed;
                }
            }
            OplogEntry::NoOp { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::Jump { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::Interrupted { .. } => {
                result = WorkerStatus::Interrupted;
            }
            OplogEntry::Exited { .. } => {
                result = WorkerStatus::Exited;
            }
            OplogEntry::ChangeRetryPolicy { new_policy, .. } => {
                current_retry_policy = Some(new_policy.clone());
                result = WorkerStatus::Running;
            }
            OplogEntry::BeginAtomicRegion { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::EndAtomicRegion { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::BeginRemoteWrite { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::EndRemoteWrite { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::PendingWorkerInvocation { .. } => {}
            OplogEntry::PendingUpdate { .. } => {
                if result == WorkerStatus::Failed {
                    result = WorkerStatus::Retrying;
                }
            }
            OplogEntry::FailedUpdate { .. } => {}
            OplogEntry::SuccessfulUpdate { .. } => {}
            OplogEntry::GrowMemory { .. } => {}
            OplogEntry::CreateResource { .. } => {}
            OplogEntry::DropResource { .. } => {}
            OplogEntry::DescribeResource { .. } => {}
            OplogEntry::Log { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::Restart { .. } => {
                result = WorkerStatus::Idle;
            }
            OplogEntry::CreateV1 { .. } => {
                result = WorkerStatus::Idle;
            }
            OplogEntry::SuccessfulUpdateV1 { .. } => {}
            OplogEntry::ActivatePlugin { .. } => {}
            OplogEntry::DeactivatePlugin { .. } => {}
            OplogEntry::Revert { .. } => {}
        }
    }
    result
}

fn calculate_deleted_regions(
    initial: DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> DeletedRegions {
    let mut builder = DeletedRegionsBuilder::from_regions(initial.into_regions());
    for entry in entries.values() {
        match entry {
            OplogEntry::Jump { jump, .. } => {
                builder.add(jump.clone());
            }
            OplogEntry::Revert { dropped_region, .. } => {
                builder.add(dropped_region.clone());
            }
            _ => {}
        }
    }
    builder.build()
}

fn calculate_overridden_retry_policy(
    initial: Option<RetryConfig>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Option<RetryConfig> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping changes in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        if let OplogEntry::ChangeRetryPolicy { new_policy, .. } = entry {
            result = Some(new_policy.clone());
        }
    }
    result
}

fn calculate_pending_invocations(
    initial: Vec<TimestampedWorkerInvocation>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<TimestampedWorkerInvocation> {
    let mut result = initial;
    for entry in entries.values() {
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
                description: UpdateDescription::SnapshotBased { target_version, .. },
                ..
            } => result.retain(|invocation| match invocation {
                TimestampedWorkerInvocation {
                    invocation:
                        WorkerInvocation::ManualUpdate {
                            target_version: version,
                            ..
                        },
                    ..
                } => version != target_version,
                _ => true,
            }),
            _ => {}
        }
    }
    result
}

fn calculate_update_fields(
    initial_pending_updates: VecDeque<TimestampedUpdateDescription>,
    initial_failed_updates: Vec<FailedUpdateRecord>,
    initial_successful_updates: Vec<SuccessfulUpdateRecord>,
    initial_version: u64,
    initial_component_size: u64,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    VecDeque<TimestampedUpdateDescription>,
    Vec<FailedUpdateRecord>,
    Vec<SuccessfulUpdateRecord>,
    u64,
    u64,
) {
    let mut pending_updates = initial_pending_updates;
    let mut failed_updates = initial_failed_updates;
    let mut successful_updates = initial_successful_updates;
    let mut version = initial_version;
    let mut component_size = initial_component_size;
    for (oplog_idx, entry) in entries {
        match entry {
            OplogEntry::Create {
                component_version, ..
            } => {
                version = *component_version;
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
                target_version,
                details,
            } => {
                failed_updates.push(FailedUpdateRecord {
                    timestamp: *timestamp,
                    target_version: *target_version,
                    details: details.clone(),
                });
                pending_updates.pop_front();
            }
            OplogEntry::SuccessfulUpdateV1 {
                timestamp,
                target_version,
                new_component_size,
            } => {
                successful_updates.push(SuccessfulUpdateRecord {
                    timestamp: *timestamp,
                    target_version: *target_version,
                });
                version = *target_version;
                component_size = *new_component_size;
                pending_updates.pop_front();
            }
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
                new_component_size,
                ..
            } => {
                successful_updates.push(SuccessfulUpdateRecord {
                    timestamp: *timestamp,
                    target_version: *target_version,
                });
                version = *target_version;
                component_size = *new_component_size;
                pending_updates.pop_front();
            }
            _ => {}
        }
    }
    (
        pending_updates,
        failed_updates,
        successful_updates,
        version,
        component_size,
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
        // Skipping entries in deleted regions as they are not applied during replay
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
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> u64 {
    let mut result = total;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        if let OplogEntry::GrowMemory { delta, .. } = entry {
            result += *delta;
        }
    }
    result
}

fn calculate_owned_resources(
    initial: HashMap<WorkerResourceId, WorkerResourceDescription>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashMap<WorkerResourceId, WorkerResourceDescription> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::CreateResource { id, timestamp } => {
                result.insert(
                    *id,
                    WorkerResourceDescription {
                        created_at: *timestamp,
                        indexed_resource_key: None,
                    },
                );
            }
            OplogEntry::DropResource { id, .. } => {
                result.remove(id);
            }
            OplogEntry::DescribeResource {
                id,
                indexed_resource,
                ..
            } => {
                if let Some(description) = result.get_mut(id) {
                    description.indexed_resource_key = Some(indexed_resource.clone());
                }
            }
            _ => {}
        }
    }
    result
}

fn calculate_active_plugins(
    initial: HashSet<PluginInstallationId>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<PluginInstallationId> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::ActivatePlugin { plugin, .. } => {
                result.insert(plugin.clone());
            }
            OplogEntry::DeactivatePlugin { plugin, .. } => {
                result.remove(plugin);
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
