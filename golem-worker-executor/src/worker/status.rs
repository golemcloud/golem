use crate::services::{HasConfig, HasOplogService};
use crate::worker::is_worker_error_retriable;
use async_recursion::async_recursion;
use golem_common::base_model::{OplogIndex, PluginInstallationId};
use golem_common::model::oplog::{OplogEntry, TimestampedUpdateDescription, UpdateDescription};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    AgentInstanceDescription, ExportedResourceInstanceDescription, ExportedResourceInstanceKey,
    FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, RetryConfig, SuccessfulUpdateRecord,
    TimestampedWorkerInvocation, WorkerInvocation, WorkerMetadata, WorkerResourceDescription,
    WorkerResourceKey, WorkerStatus, WorkerStatusRecord,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker status.
#[async_recursion]
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    metadata: &Option<WorkerMetadata>,
) -> Result<WorkerStatusRecord, WorkerExecutorError>
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
            calculate_last_known_status(this, owned_worker_id, &None).await
        } else {
            let active_plugins = last_known.active_plugins.clone();

            let overridden_retry_config = calculate_overridden_retry_policy(
                last_known.overridden_retry_config.clone(),
                &skipped_regions,
                &new_entries,
            );
            let status = calculate_latest_worker_status(
                &last_known.status,
                &this.config().retry,
                last_known.overridden_retry_config.clone(),
                &skipped_regions,
                &new_entries,
            );

            let pending_invocations = calculate_pending_invocations(
                last_known.pending_invocations,
                &deleted_regions,
                &new_entries,
            );
            let (
                pending_updates,
                failed_updates,
                successful_updates,
                component_version,
                component_size,
                component_version_for_replay,
            ) = calculate_update_fields(
                last_known.pending_updates,
                last_known.failed_updates,
                last_known.successful_updates,
                last_known.component_version,
                last_known.component_size,
                last_known.component_version_for_replay,
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

            let active_plugins =
                calculate_active_plugins(active_plugins, &deleted_regions, &new_entries);

            let result = WorkerStatusRecord {
                oplog_idx: last_oplog_index,
                status,
                overridden_retry_config,
                pending_invocations,
                skipped_regions,
                pending_updates,
                failed_updates,
                successful_updates,
                invocation_results,
                current_idempotency_key,
                component_version,
                component_size,
                owned_resources,
                total_linear_memory_size,
                active_plugins,
                deleted_regions,
                component_version_for_replay,
            };
            Ok(result)
        }
    }
}

fn calculate_latest_worker_status(
    initial: &WorkerStatus,
    default_retry_policy: &RetryConfig,
    initial_retry_policy: Option<RetryConfig>,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> WorkerStatus {
    let mut result = initial.clone();
    let mut last_error_count = 0;
    let mut current_retry_policy = initial_retry_policy;
    for (idx, entry) in entries {
        // Skipping entries in skipped regions, as they are skipped during replay too
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        if !matches!(entry, OplogEntry::Error { .. }) {
            last_error_count = 0;
        }

        match entry {
            OplogEntry::Create { .. } => {
                result = WorkerStatus::Idle;
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
            OplogEntry::ActivatePlugin { .. } => {}
            OplogEntry::DeactivatePlugin { .. } => {}
            OplogEntry::Revert { .. } => {}
            OplogEntry::CancelPendingInvocation { .. } => {}
            OplogEntry::StartSpan { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::FinishSpan { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::SetSpanAttribute { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::ChangePersistenceLevel { .. } => {
                result = WorkerStatus::Running;
            }
            OplogEntry::CreateAgentInstance { .. } => {}
            OplogEntry::DropAgentInstance { .. } => {}
        }
    }
    result
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

fn calculate_overridden_retry_policy(
    initial: Option<RetryConfig>,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Option<RetryConfig> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping changes in skipped regions as they are not applied during replay
        if skipped_regions.is_in_deleted_region(*idx) {
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
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<TimestampedWorkerInvocation> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions (by revert) but not by skipped regions (by jumps and updates)
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

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
    initial_version: u64,
    initial_component_size: u64,
    initial_component_version_for_replay: u64,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    VecDeque<TimestampedUpdateDescription>,
    Vec<FailedUpdateRecord>,
    Vec<SuccessfulUpdateRecord>,
    u64,
    u64,
    u64,
) {
    let mut pending_updates = initial_pending_updates;
    let mut failed_updates = initial_failed_updates;
    let mut successful_updates = initial_successful_updates;
    let mut version = initial_version;
    let mut size = initial_component_size;
    let mut component_version_for_replay = initial_component_version_for_replay;

    for (oplog_idx, entry) in entries {
        // Skipping entries in deleted regions (by revert)
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            continue;
        }

        match entry {
            OplogEntry::Create {
                component_version,
                component_size,
                ..
            } => {
                version = *component_version;
                component_version_for_replay = *component_version;
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
                size = *new_component_size;

                let applied_update = pending_updates.pop_front();
                if matches!(
                    applied_update,
                    Some(TimestampedUpdateDescription {
                        description: UpdateDescription::SnapshotBased { .. },
                        ..
                    })
                ) {
                    component_version_for_replay = *target_version
                }
            }
            _ => {}
        }
    }
    (
        pending_updates,
        failed_updates,
        successful_updates,
        version,
        size,
        component_version_for_replay,
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
    initial: HashMap<WorkerResourceKey, WorkerResourceDescription>,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashMap<WorkerResourceKey, WorkerResourceDescription> {
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
                    WorkerResourceKey::ExportedResourceInstanceKey(ExportedResourceInstanceKey {
                        resource_id: *id,
                    }),
                    WorkerResourceDescription::ExportedResourceInstance(
                        ExportedResourceInstanceDescription {
                            created_at: *timestamp,
                            resource_owner: resource_type_id.owner.clone(),
                            resource_name: resource_type_id.name.clone(),
                            resource_params: None,
                        },
                    ),
                );
            }
            OplogEntry::DropResource { id, .. } => {
                result.remove(&WorkerResourceKey::ExportedResourceInstanceKey(
                    ExportedResourceInstanceKey { resource_id: *id },
                ));
            }
            OplogEntry::DescribeResource {
                id,
                indexed_resource_parameters,
                ..
            } => {
                if let Some(WorkerResourceDescription::ExportedResourceInstance(
                    ExportedResourceInstanceDescription {
                        resource_params, ..
                    },
                )) = result.get_mut(&WorkerResourceKey::ExportedResourceInstanceKey(
                    ExportedResourceInstanceKey { resource_id: *id },
                )) {
                    *resource_params = Some(indexed_resource_parameters.clone());
                }
            }
            OplogEntry::CreateAgentInstance {
                timestamp,
                key,
                parameters,
            } => {
                result.insert(
                    WorkerResourceKey::AgentInstanceKey(key.clone()),
                    WorkerResourceDescription::AgentInstance(AgentInstanceDescription {
                        created_at: *timestamp,
                        agent_parameters: parameters.clone(),
                    }),
                );
            }
            OplogEntry::DropAgentInstance { key, .. } => {
                result.remove(&WorkerResourceKey::AgentInstanceKey(key.clone()));
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

#[cfg(test)]
mod test {
    use crate::model::ExecutionStatus;
    use crate::services::golem_config::GolemConfig;
    use crate::services::oplog::tests::rounded;
    use crate::services::oplog::{Oplog, OplogService};
    use crate::services::{HasConfig, HasOplogService};
    use crate::worker::status::calculate_last_known_status;
    use async_trait::async_trait;
    use bincode::Encode;
    use bytes::Bytes;
    use golem_common::base_model::OplogIndex;
    use golem_common::model::invocation_context::{InvocationContextStack, TraceId};
    use golem_common::model::oplog::{
        DurableFunctionType, OplogEntry, OplogPayload, TimestampedUpdateDescription,
        UpdateDescription,
    };
    use golem_common::model::regions::{DeletedRegions, OplogRegion};
    use golem_common::model::{
        AccountId, ComponentId, ComponentVersion, FailedUpdateRecord, IdempotencyKey,
        OwnedWorkerId, PluginInstallationId, ProjectId, RetryConfig, ScanCursor,
        SuccessfulUpdateRecord, Timestamp, TimestampedWorkerInvocation, WorkerId, WorkerInvocation,
        WorkerMetadata, WorkerStatus, WorkerStatusRecord,
    };
    use golem_common::serialization::serialize;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_wasm_rpc::Value;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::{Arc, RwLock};
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
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .exported_function_completed(&'x', k1)
            .exported_function_invoked("b", &1, k2.clone())
            .exported_function_completed(&'y', k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results_with_jump() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .jump(OplogIndex::from_u64(2))
            .exported_function_completed(&'x', k1)
            .exported_function_invoked("b", &1, k2.clone())
            .exported_function_completed(&'y', k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn invocation_results_with_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();

        let test_case = TestCase::builder(0)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .exported_function_completed(&'x', k1)
            .exported_function_invoked("b", &1, k2.clone())
            .exported_function_completed(&'y', k2)
            .revert(OplogIndex::from_u64(5))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_auto_update_for_running() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic { target_version: 2 };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_update(&update1)
            .successful_update(update1, 2000, &HashSet::new())
            .exported_function_completed(&'x', k1)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn auto_update_for_running_with_jump() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic { target_version: 2 };
        let update2 = UpdateDescription::Automatic { target_version: 3 };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_update(&update1)
            .pending_update(&update2)
            .successful_update(update1, 2000, &HashSet::new())
            .jump(OplogIndex::from_u64(4))
            .successful_update(update2, 3000, &HashSet::new())
            .exported_function_completed(&'x', k1)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_update() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_version: 2,
            payload: OplogPayload::Inline(vec![]),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_invocation(WorkerInvocation::ManualUpdate { target_version: 2 })
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .exported_function_completed(&'x', k1)
            .pending_update(&update1)
            .successful_update(update1, 2000, &HashSet::new())
            .exported_function_invoked("c", &0, k2.clone())
            .exported_function_completed(&'y', k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_failed_update() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_version: 2,
            payload: OplogPayload::Inline(vec![]),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_invocation(WorkerInvocation::ManualUpdate { target_version: 2 })
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .exported_function_completed(&'x', k1)
            .pending_update(&update1)
            .failed_update(update1)
            .exported_function_invoked("c", &0, k2.clone())
            .exported_function_completed(&'y', k2)
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn auto_update_for_running_with_jump_and_revert() {
        let k1 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::Automatic { target_version: 2 };
        let update2 = UpdateDescription::Automatic { target_version: 3 };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_update(&update1)
            .pending_update(&update2)
            .successful_update(update1, 2000, &HashSet::new())
            .jump(OplogIndex::from_u64(4))
            .successful_update(update2, 3000, &HashSet::new())
            .exported_function_completed(&'x', k1)
            .revert(OplogIndex::from_u64(3))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn single_manual_update_with_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_version: 2,
            payload: OplogPayload::Inline(vec![]),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_invocation(WorkerInvocation::ManualUpdate { target_version: 2 })
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .exported_function_completed(&'x', k1)
            .pending_update(&update1)
            .successful_update(update1, 2000, &HashSet::new())
            .exported_function_invoked("c", &0, k2.clone())
            .exported_function_completed(&'y', k2)
            .revert(OplogIndex::from_u64(4))
            .build();

        run_test_case(test_case).await;
    }

    #[test]
    async fn multiple_manual_updates_with_jump_and_revert() {
        let k1 = IdempotencyKey::fresh();
        let k2 = IdempotencyKey::fresh();
        let update1 = UpdateDescription::SnapshotBased {
            target_version: 2,
            payload: OplogPayload::Inline(vec![]),
        };
        let update2 = UpdateDescription::SnapshotBased {
            target_version: 2,
            payload: OplogPayload::Inline(vec![]),
        };

        let test_case = TestCase::builder(1)
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .pending_invocation(WorkerInvocation::ManualUpdate { target_version: 2 })
            .imported_function_invoked("b", &0, &1, DurableFunctionType::ReadLocal)
            .exported_function_completed(&'x', k1)
            .pending_update(&update1)
            .failed_update(update1)
            .exported_function_invoked("c", &0, k2.clone())
            .pending_invocation(WorkerInvocation::ManualUpdate { target_version: 2 })
            .exported_function_completed(&'y', k2)
            .pending_update(&update2)
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
            .exported_function_invoked("a", &0, k1.clone())
            .grow_memory(10)
            .grow_memory(100)
            .pending_invocation(WorkerInvocation::ExportedFunction {
                idempotency_key: k2.clone(),
                full_function_name: "b".to_string(),
                function_input: vec![Value::Bool(true)],
                invocation_context: InvocationContextStack::fresh(),
            })
            .exported_function_completed(&'x', k1.clone())
            .exported_function_invoked("b", &1, k2.clone())
            .exported_function_completed(&'y', k2.clone())
            .revert(OplogIndex::from_u64(5))
            .exported_function_completed(&'x', k1)
            .exported_function_invoked("b", &1, k2.clone())
            .exported_function_completed(&'y', k2)
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

    struct TestCaseBuilder {
        entries: Vec<TestEntry>,
        previous_status_record: WorkerStatusRecord,
        owned_worker_id: OwnedWorkerId,
        account_id: AccountId,
    }

    impl TestCaseBuilder {
        pub fn new(
            account_id: AccountId,
            owned_worker_id: OwnedWorkerId,
            component_version: ComponentVersion,
        ) -> Self {
            let status = WorkerStatusRecord {
                component_version,
                component_version_for_replay: component_version,
                component_size: 100,
                total_linear_memory_size: 200,
                oplog_idx: OplogIndex::INITIAL,
                ..Default::default()
            };
            TestCaseBuilder {
                entries: vec![TestEntry {
                    oplog_entry: OplogEntry::create(
                        owned_worker_id.worker_id(),
                        component_version,
                        vec![],
                        vec![],
                        BTreeMap::new(),
                        owned_worker_id.project_id(),
                        account_id.clone(),
                        None,
                        100,
                        200,
                        HashSet::new(),
                    ),
                    expected_status: status.clone(),
                }],
                previous_status_record: status,
                owned_worker_id,
                account_id,
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

        pub fn exported_function_invoked<R: Encode>(
            self,
            function_name: &str,
            request: &R,
            idempotency_key: IdempotencyKey,
        ) -> Self {
            self.add(
                OplogEntry::ExportedFunctionInvoked {
                    timestamp: Timestamp::now_utc(),
                    function_name: function_name.to_string(),
                    request: OplogPayload::Inline(serialize(request).unwrap().to_vec()),
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

        pub fn exported_function_completed<R: Encode>(
            self,
            response: &R,
            idempotency_key: IdempotencyKey,
        ) -> Self {
            self.add(
                OplogEntry::ExportedFunctionCompleted {
                    timestamp: Timestamp::now_utc(),
                    response: OplogPayload::Inline(serialize(response).unwrap().to_vec()),
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

        pub fn imported_function_invoked<I: Encode, O: Encode>(
            self,
            name: &str,
            i: &I,
            o: &O,
            func_type: DurableFunctionType,
        ) -> Self {
            self.add(
                OplogEntry::ImportedFunctionInvoked {
                    timestamp: Timestamp::now_utc(),
                    function_name: name.to_string(),
                    request: OplogPayload::Inline(serialize(i).unwrap().to_vec()),
                    response: OplogPayload::Inline(serialize(o).unwrap().to_vec()),
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
                status.component_version = old_status.component_version;
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
                status.component_version = old_status.component_version;
                status.current_idempotency_key = old_status.current_idempotency_key;
                status.total_linear_memory_size = old_status.total_linear_memory_size;
                status.component_size = old_status.component_size;
                status.owned_resources = old_status.owned_resources;
                status.pending_invocations = old_status.pending_invocations;
                status.pending_updates = old_status.pending_updates;
                status.successful_updates = old_status.successful_updates;
                status.failed_updates = old_status.failed_updates;
                status.invocation_results = old_status.invocation_results;
                status.component_version_for_replay = old_status.component_version_for_replay;

                status
            })
        }

        pub fn pending_invocation(self, invocation: WorkerInvocation) -> Self {
            let entry = rounded(OplogEntry::pending_worker_invocation(invocation.clone()));
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
            let entry = rounded(OplogEntry::cancel_pending_invocation(
                idempotency_key.clone(),
            ));
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

        pub fn pending_update(self, update_description: &UpdateDescription) -> Self {
            let entry = rounded(OplogEntry::pending_update(update_description.clone()));
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

                status
            })
        }

        pub fn successful_update(
            self,
            update_description: UpdateDescription,
            new_component_size: u64,
            new_active_plugins: &HashSet<PluginInstallationId>,
        ) -> Self {
            let old_status = self.entries.first().unwrap().expected_status.clone();
            self.add(
                rounded(OplogEntry::successful_update(
                    *update_description.target_version(),
                    new_component_size,
                    new_active_plugins.clone(),
                )),
                move |mut status| {
                    let pending = status.pending_updates.pop_front();
                    status.successful_updates.push(SuccessfulUpdateRecord {
                        timestamp: pending.unwrap().timestamp,
                        target_version: *update_description.target_version(),
                    });
                    status.component_size = new_component_size;
                    status.component_version = *update_description.target_version();
                    status.active_plugins = new_active_plugins.clone();

                    if status.skipped_regions.is_overridden() {
                        status.skipped_regions.merge_override();
                        status.total_linear_memory_size = old_status.total_linear_memory_size;
                        status.owned_resources = HashMap::new();
                    }

                    if let UpdateDescription::SnapshotBased { target_version, .. } =
                        update_description
                    {
                        status.component_version_for_replay = target_version;
                    };

                    status
                },
            )
        }

        pub fn failed_update(self, update_description: UpdateDescription) -> Self {
            let entry = rounded(OplogEntry::failed_update(
                *update_description.target_version(),
                Some("details".to_string()),
            ));
            self.add(entry.clone(), move |mut status| {
                status.failed_updates.push(FailedUpdateRecord {
                    timestamp: entry.timestamp(),
                    target_version: *update_description.target_version(),
                    details: Some("details".to_string()),
                });
                status.pending_updates.pop_front();

                if status.skipped_regions.is_overridden() {
                    status.skipped_regions.drop_override();
                }

                status
            })
        }

        pub fn build(self) -> TestCase {
            TestCase {
                account_id: self.account_id,
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
                oplog_entry: rounded(self.oplog_entry),
                expected_status: self.expected_status,
            }
        }
    }

    #[derive(Debug, Clone)]
    struct TestCase {
        account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        entries: Vec<TestEntry>,
    }

    impl TestCase {
        pub fn builder(initial_component_version: ComponentVersion) -> TestCaseBuilder {
            let project_id = ProjectId::new_v4();
            let account_id = AccountId {
                value: "test-account".to_string(),
            };
            let owned_worker_id = OwnedWorkerId::new(
                &project_id,
                &WorkerId {
                    component_id: ComponentId::new_v4(),
                    worker_name: "test-worker".to_string(),
                },
            );
            TestCaseBuilder::new(account_id, owned_worker_id, initial_component_version)
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
            _execution_status: Arc<RwLock<ExecutionStatus>>,
        ) -> Arc<dyn Oplog + 'static> {
            unreachable!()
        }

        async fn open(
            &self,
            _owned_worker_id: &OwnedWorkerId,
            _last_oplog_index: OplogIndex,
            _initial_worker_metadata: WorkerMetadata,
            _execution_status: Arc<RwLock<ExecutionStatus>>,
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
            _project_id: &ProjectId,
            _component_id: &ComponentId,
            _cursor: ScanCursor,
            _count: u64,
        ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError> {
            unreachable!()
        }

        async fn upload_payload(
            &self,
            _owned_worker_id: &OwnedWorkerId,
            _data: &[u8],
        ) -> Result<OplogPayload, String> {
            unreachable!()
        }

        async fn download_payload(
            &self,
            _owned_worker_id: &OwnedWorkerId,
            _payload: &OplogPayload,
        ) -> Result<Bytes, String> {
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
            let known_metadata = if idx == 0 {
                None
            } else {
                Some(WorkerMetadata {
                    last_known_status: test_case.entries[idx - 1].expected_status.clone(),
                    ..WorkerMetadata::default(
                        test_case.owned_worker_id.worker_id(),
                        test_case.account_id.clone(),
                        test_case.owned_worker_id.project_id(),
                    )
                })
            };
            let final_status = calculate_last_known_status(
                &test_case,
                &test_case.owned_worker_id,
                &known_metadata,
            )
            .await
            .unwrap();

            assert_eq!(
                final_status, final_expected_status,
                "Calculating the last known status from oplog index {idx}"
            )
        }
    }
}
