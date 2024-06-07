// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use golem_wasm_rpc::Value;
use tracing::{debug, error, Instrument};

use golem_common::config::RetryConfig;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription, WorkerError,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    CallingConvention, FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, SuccessfulUpdateRecord,
    TimestampedWorkerInvocation, WorkerInvocation, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord,
};

use crate::error::GolemError;
use crate::model::{InterruptKind, LookupResult};
use crate::services::invocation_queue::InvocationQueue;
use crate::services::{HasAll, HasConfig, HasOplogService, HasWorkerService};
use crate::workerctx::WorkerCtx;

/// Makes sure that the worker is active, but without waiting for it to be idle.
///
/// If the worker is already in memory this does nothing. Otherwise, the worker will be
/// created (same as get_or_create_worker) but in a background task.
///
/// If the active worker cache is not full, this newly created worker will be added to it.
/// If it was full, the worker will be dropped but only after it finishes recovering which means
/// a previously interrupted / suspended invocation might be resumed.
pub async fn activate<Ctx: WorkerCtx, T>(this: &T, owned_worker_id: &OwnedWorkerId)
where
    T: HasAll<Ctx> + Send + Sync + Clone + 'static,
{
    match get_or_create(this, owned_worker_id, None, None, None).await {
        Ok(worker) => {
            if let Err(err) = InvocationQueue::start_if_needed(worker).await {
                error!("Failed to activate worker: {err}");
            }
        }
        Err(err) => {
            error!("Failed to activate worker: {err}");
        }
    }
}

pub async fn get_or_create<Ctx: WorkerCtx, T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    worker_args: Option<Vec<String>>,
    worker_env: Option<Vec<(String, String)>>,
    component_version: Option<u64>,
) -> Result<Arc<InvocationQueue<Ctx>>, GolemError>
where
    T: HasAll<Ctx> + Clone + Send + Sync + 'static,
{
    let this_clone = this.clone();
    let owned_worker_id_clone = owned_worker_id.clone();

    let worker_details = this
        .active_workers()
        .get_with(&owned_worker_id.worker_id, || {
            Box::pin(async move {
                Ok(Arc::new(
                    InvocationQueue::new(
                        &this_clone,
                        owned_worker_id_clone,
                        worker_args,
                        worker_env,
                        component_version,
                    )
                    .in_current_span()
                    .await?,
                ))
            })
        })
        .await?;
    Ok(worker_details)
}

// TODO: move to InvocationQueue?
pub async fn invoke<Ctx: WorkerCtx>(
    invocation_queue: Arc<InvocationQueue<Ctx>>,
    idempotency_key: IdempotencyKey,
    calling_convention: CallingConvention,
    full_function_name: String,
    function_input: Vec<Value>,
) -> Result<Option<Result<Vec<Value>, GolemError>>, GolemError> {
    let output = invocation_queue
        .lookup_invocation_result(&idempotency_key)
        .await;

    match output {
        LookupResult::Complete(output) => Ok(Some(output)),
        LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
        LookupResult::Pending => Ok(None),
        LookupResult::New => {
            // Invoke the function in the background
            invocation_queue
                .enqueue(
                    idempotency_key,
                    full_function_name,
                    function_input,
                    calling_convention,
                )
                .await;
            Ok(None)
        }
    }
}

// TODO: move to InvocationQueue?
pub async fn invoke_and_await<Ctx: WorkerCtx>(
    invocation_queue: Arc<InvocationQueue<Ctx>>,
    idempotency_key: IdempotencyKey,
    calling_convention: CallingConvention,
    full_function_name: String,
    function_input: Vec<Value>,
) -> Result<Vec<Value>, GolemError> {
    match invoke(
        invocation_queue.clone(),
        idempotency_key.clone(),
        calling_convention,
        full_function_name,
        function_input,
    )
    .await?
    {
        Some(Ok(output)) => Ok(output),
        Some(Err(err)) => Err(err),
        None => {
            debug!("Waiting for idempotency key to complete",);

            let result = invocation_queue
                .wait_for_invocation_result(&idempotency_key)
                .await;

            debug!("Idempotency key lookup result: {:?}", result);
            match result {
                LookupResult::Complete(Ok(output)) => Ok(output),
                LookupResult::Complete(Err(err)) => Err(err),
                LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
                LookupResult::Pending => Err(GolemError::unknown(
                    "Unexpected pending result after invoke",
                )),
                LookupResult::New => Err(GolemError::unknown(
                    "Unexpected missing result after invoke",
                )),
            }
        }
    }
}

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker status.
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    metadata: &Option<WorkerMetadata>,
) -> Result<WorkerStatusRecord, GolemError>
where
    T: HasOplogService + HasWorkerService + HasConfig,
{
    let last_known = metadata
        .as_ref()
        .map(|metadata| metadata.last_known_status.clone())
        .unwrap_or_default();

    let last_oplog_index = this.oplog_service().get_last_index(owned_worker_id).await;
    if last_known.oplog_idx == last_oplog_index {
        Ok(last_known)
    } else {
        debug!(
            "Calculating new worker status from {} to {}",
            last_known.oplog_idx, last_oplog_index
        );
        let new_entries: BTreeMap<OplogIndex, OplogEntry> = this
            .oplog_service()
            .read_range(
                owned_worker_id,
                last_known.oplog_idx.next(),
                last_oplog_index,
            )
            .await;

        let overridden_retry_config = calculate_overridden_retry_policy(
            last_known.overridden_retry_config.clone(),
            &new_entries,
        );
        let status = calculate_latest_worker_status(
            &last_known.status,
            &this.config().retry,
            last_known.overridden_retry_config.clone(),
            &new_entries,
        );

        let mut initial_deleted_regions = last_known.deleted_regions;
        if initial_deleted_regions.is_overridden() {
            initial_deleted_regions.drop_override();
        }

        let mut deleted_regions = calculate_deleted_regions(initial_deleted_regions, &new_entries);
        let pending_invocations =
            calculate_pending_invocations(last_known.pending_invocations, &new_entries);
        let (pending_updates, failed_updates, successful_updates, component_version) =
            calculate_update_fields(
                last_known.pending_updates,
                last_known.failed_updates,
                last_known.successful_updates,
                last_known.component_version,
                &new_entries,
            );

        if let Some(TimestampedUpdateDescription {
            oplog_index,
            description: UpdateDescription::SnapshotBased { .. },
            ..
        }) = pending_updates.front()
        {
            deleted_regions.set_override(DeletedRegions::from_regions(vec![
                OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=*oplog_index),
            ]));
        }

        let (invocation_results, current_idempotency_key) = calculate_invocation_results(
            last_known.invocation_results,
            last_known.current_idempotency_key,
            &new_entries,
        );

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
        };
        debug!(
            "calculate_last_known_status using last oplog index {last_oplog_index} as reference resulted in {result:?}"
        );
        Ok(result)
    }
}

fn calculate_latest_worker_status(
    initial: &WorkerStatus,
    default_retry_policy: &RetryConfig,
    initial_retry_policy: Option<RetryConfig>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> WorkerStatus {
    let mut result = initial.clone();
    let mut last_error_count = 0;
    let mut current_retry_policy = initial_retry_policy;
    for entry in entries.values() {
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
        if let OplogEntry::Jump { jump, .. } = entry {
            builder.add(jump.clone());
        }
    }
    builder.build()
}

fn calculate_overridden_retry_policy(
    initial: Option<RetryConfig>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Option<RetryConfig> {
    let mut result = initial;
    for entry in entries.values() {
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
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    VecDeque<TimestampedUpdateDescription>,
    Vec<FailedUpdateRecord>,
    Vec<SuccessfulUpdateRecord>,
    u64,
) {
    let mut pending_updates = initial_pending_updates;
    let mut failed_updates = initial_failed_updates;
    let mut successful_updates = initial_successful_updates;
    let mut version = initial_version;
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
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
            } => {
                successful_updates.push(SuccessfulUpdateRecord {
                    timestamp: *timestamp,
                    target_version: *target_version,
                });
                version = *target_version;
                pending_updates.pop_front();
            }
            _ => {}
        }
    }
    (pending_updates, failed_updates, successful_updates, version)
}

fn calculate_invocation_results(
    invocation_results: HashMap<IdempotencyKey, OplogIndex>,
    current_idempotency_key: Option<IdempotencyKey>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (HashMap<IdempotencyKey, OplogIndex>, Option<IdempotencyKey>) {
    let mut invocation_results = invocation_results;
    let mut current_idempotency_key = current_idempotency_key;

    for (oplog_idx, entry) in entries {
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

pub fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &WorkerError,
    retry_count: u64,
) -> bool {
    match error {
        WorkerError::Unknown(_) => retry_count < (retry_config.max_attempts as u64),
        WorkerError::StackOverflow => false,
    }
}
