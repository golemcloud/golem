// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

pub mod invocation;
mod invocation_loop;
pub mod status;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::durable_host::recover_stderr_logs;
use crate::model::{ExecutionStatus, LookupResult, ReadFileResult, TrapType, WorkerConfig};
use crate::services::events::{Event, EventsSubscription};
use crate::services::oplog::{CommitLevel, Oplog, OplogOps};
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveWorkers, HasAll, HasBlobStoreService, HasComponentService, HasConfig, HasEvents,
    HasExtraDeps, HasFileLoader, HasKeyValueService, HasOplog, HasOplogService, HasPlugins,
    HasProjectService, HasPromiseService, HasRdbmsService, HasResourceLimits, HasRpc,
    HasSchedulerService, HasWasmtimeEngine, HasWorkerEnumerationService, HasWorkerForkService,
    HasWorkerProxy, HasWorkerService, UsesAllDeps,
};
use crate::worker::invocation_loop::InvocationLoop;
use crate::worker::status::calculate_last_known_status;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use futures::channel::oneshot;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription, WorkerError,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{AccountId, RetryConfig};
use golem_common::model::{ComponentFilePath, ComponentType, PluginInstallationId};
use golem_common::model::{
    ComponentVersion, GetFileSystemNodeResult, IdempotencyKey, OwnedWorkerId, Timestamp,
    TimestampedWorkerInvocation, WorkerId, WorkerInvocation, WorkerMetadata, WorkerStatusRecord,
};
use golem_service_base::error::worker_executor::{
    InterruptKind, WorkerExecutorError, WorkerOutOfMemory,
};
use golem_service_base::model::RevertWorkerTarget;
use golem_wasm_ast::analysis::AnalysedFunctionResult;
use golem_wasm_rpc::{Value, ValueAndType};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, MutexGuard, OwnedSemaphorePermit, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, span, warn, Instrument, Level};
use wasmtime::component::Instance;
use wasmtime::{Store, UpdateDeadline};

/// Represents worker that may be running or suspended.
///
/// It is responsible for receiving incoming worker invocations in a non-blocking way,
/// persisting them and also making sure that all the enqueued invocations eventually get
/// processed, in the same order as they came in.
///
/// Invocations have an associated idempotency key that is used to ensure that the same invocation
/// is not processed multiple times.
///
/// If the queue is empty, the service can trigger invocations directly as an optimization.
///
/// Every worker invocation should be done through this service.
pub struct Worker<Ctx: WorkerCtx> {
    owned_worker_id: OwnedWorkerId,

    oplog: Arc<dyn Oplog>,
    worker_event_service: Arc<dyn WorkerEventService + Send + Sync>,

    deps: All<Ctx>,

    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    pending_updates: Arc<RwLock<VecDeque<TimestampedUpdateDescription>>>,

    invocation_results: Arc<RwLock<HashMap<IdempotencyKey, InvocationResult>>>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
    stopping: AtomicBool,
    worker_estimate_coefficient: f64,

    instance: Arc<Mutex<WorkerInstance>>,
    oom_retry_config: RetryConfig,
}

impl<Ctx: WorkerCtx> HasOplog for Worker<Ctx> {
    fn oplog(&self) -> Arc<dyn Oplog> {
        self.oplog.clone()
    }
}

impl<Ctx: WorkerCtx> UsesAllDeps for Worker<Ctx> {
    type Ctx = Ctx;

    fn all(&self) -> &All<Self::Ctx> {
        &self.deps
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Gets or creates a worker, but does not start it
    pub async fn get_or_create_suspended<T>(
        deps: &T,
        account_id: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        worker_wasi_config_vars: Option<BTreeMap<String, String>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        deps.active_workers()
            .get_or_add(
                deps,
                owned_worker_id,
                account_id,
                worker_args,
                worker_env,
                worker_wasi_config_vars,
                component_version,
                parent,
            )
            .await
    }

    /// Gets or creates a worker and makes sure it is running
    pub async fn get_or_create_running<T>(
        deps: &T,
        account_id: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        worker_wasi_config_vars: Option<BTreeMap<String, String>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker = Self::get_or_create_suspended(
            deps,
            account_id,
            owned_worker_id,
            worker_args,
            worker_env,
            worker_wasi_config_vars,
            component_version,
            parent,
        )
        .await?;
        Self::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    pub async fn get_latest_metadata<
        T: HasActiveWorkers<Ctx> + HasWorkerService + HasOplogService + HasConfig + Sync,
    >(
        deps: &T,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Option<WorkerMetadata>, WorkerExecutorError> {
        if let Some(worker) = deps.active_workers().try_get(owned_worker_id).await {
            Ok(Some(worker.get_metadata()?))
        } else if let Some(previous_metadata) = deps.worker_service().get(owned_worker_id).await {
            Ok(Some(WorkerMetadata {
                last_known_status: calculate_last_known_status(
                    deps,
                    owned_worker_id,
                    &Some(previous_metadata.clone()),
                )
                .await?,
                ..previous_metadata
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn new<T: HasAll<Ctx>>(
        deps: &T,
        account_id: &AccountId,
        owned_worker_id: OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        worker_config: Option<BTreeMap<String, String>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Self, WorkerExecutorError> {
        let (worker_metadata, execution_status) = Self::get_or_create_worker_metadata(
            deps,
            account_id,
            &owned_worker_id,
            component_version,
            worker_args,
            worker_env,
            worker_config,
            parent,
        )
        .await?;

        let initial_component_metadata = deps
            .component_service()
            .get_metadata(
                &owned_worker_id.project_id,
                &owned_worker_id.worker_id.component_id,
                Some(worker_metadata.last_known_status.component_version),
            )
            .await?;
        execution_status
            .write()
            .unwrap()
            .set_component_type(initial_component_metadata.component_type);

        let last_oplog_index = deps.oplog_service().get_last_index(&owned_worker_id).await;

        let oplog = deps
            .oplog_service()
            .open(
                &owned_worker_id,
                last_oplog_index,
                worker_metadata.clone(),
                execution_status.clone(),
            )
            .await;

        let initial_pending_invocations = worker_metadata
            .last_known_status
            .pending_invocations
            .clone();
        let initial_pending_updates = worker_metadata
            .last_known_status
            .pending_updates
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let initial_invocation_results =
            worker_metadata.last_known_status.invocation_results.clone();

        let queue = Arc::new(RwLock::new(VecDeque::from_iter(
            initial_pending_invocations
                .iter()
                .map(|inv| QueuedWorkerInvocation::External {
                    invocation: inv.clone(),
                    canceled: false,
                }),
        )));
        let pending_updates = Arc::new(RwLock::new(VecDeque::from_iter(
            initial_pending_updates.iter().cloned(),
        )));
        let invocation_results = Arc::new(RwLock::new(HashMap::from_iter(
            initial_invocation_results.iter().map(|(key, oplog_idx)| {
                (
                    key.clone(),
                    InvocationResult::Lazy {
                        oplog_idx: *oplog_idx,
                    },
                )
            }),
        )));
        let instance = Arc::new(Mutex::new(WorkerInstance::Unloaded));

        let stopping = AtomicBool::new(false);

        Ok(Worker {
            owned_worker_id,
            oplog,
            worker_event_service: Arc::new(WorkerEventServiceDefault::new(
                deps.config().limits.event_broadcast_capacity,
                deps.config().limits.event_history_size,
            )),
            deps: All::from_other(deps),
            queue,
            pending_updates,
            invocation_results,
            instance,
            execution_status,
            stopping,
            initial_worker_metadata: worker_metadata,
            worker_estimate_coefficient: deps.config().memory.worker_estimate_coefficient,
            oom_retry_config: deps.config().memory.oom_retry_config.clone(),
        })
    }

    pub fn worker_id(&self) -> WorkerId {
        self.owned_worker_id.worker_id()
    }

    pub fn oom_retry_config(&self) -> &RetryConfig {
        &self.oom_retry_config
    }

    pub async fn start_if_needed(this: Arc<Worker<Ctx>>) -> Result<bool, WorkerExecutorError> {
        Self::start_if_needed_internal(this, 0).await
    }

    async fn start_if_needed_internal(
        this: Arc<Worker<Ctx>>,
        oom_retry_count: u64,
    ) -> Result<bool, WorkerExecutorError> {
        let mut instance = this.instance.lock().await;
        if instance.is_unloaded() {
            this.mark_as_loading();
            *instance = WorkerInstance::WaitingForPermit(WaitingWorker::new(
                this.clone(),
                this.memory_requirement()?,
                oom_retry_count,
            ));
            Ok(true)
        } else {
            debug!("Worker is already running or waiting for permit");
            Ok(false)
        }
    }

    pub(crate) async fn start_with_permit(
        this: Arc<Worker<Ctx>>,
        permit: OwnedSemaphorePermit,
        oom_retry_count: u64,
    ) {
        let mut instance = this.instance.lock().await;
        *instance = WorkerInstance::Running(
            RunningWorker::new(
                this.owned_worker_id.clone(),
                this.queue.clone(),
                this.clone(),
                this.oplog(),
                this.execution_status.clone(),
                permit,
                oom_retry_count,
            )
            .await,
        );
    }

    pub async fn stop(&self) {
        self.stop_internal(false, None).await;
    }

    /// This method is supposed to be called on a worker for what `is_currently_idle_but_running`
    /// previously returned true.
    ///
    /// It is not guaranteed that the worker is still "running (loaded in memory) but idle" when
    /// this method is called, so it rechecks this condition and only stops the worker if it
    /// is still true. If it was not true, it returns false.
    ///
    /// There are two conditions to this:
    /// - the ExecutionStatus must be suspended; this means the worker is currently not running any invocations
    /// - there must be no more pending invocations in the invocation queue
    ///
    /// Here we first acquire the `instance` lock. This means the worker cannot be started/stopped while we
    /// are processing this method.
    /// If it was not running, then we don't have to stop it.
    /// If it was running then we recheck the conditions and then stop the worker.
    ///
    /// We know that the conditions remain true because:
    /// - the invocation queue is empty, so it cannot get into `ExecutionStatus::Running`, as there is nothing to run
    /// - nothing can be added to the invocation queue because we are holding the `instance` lock
    ///
    /// By passing the running lock to `stop_internal_running` it is never released and the stop eventually
    /// drops the `RunningWorker` instance.
    ///
    /// The `stopping` flag is only used to prevent re-entrance of the stopping sequence in case the invocation loop
    /// triggers a stop (in case of a failure - by the way it should not happen here because the worker is idle).
    pub async fn stop_if_idle(&self) -> bool {
        let instance_guard = self.instance.lock().await;
        match &*instance_guard {
            WorkerInstance::Running(running) => {
                if is_running_worker_idle(running).await {
                    if self.stopping.compare_exchange(
                        false,
                        true,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    ) == Ok(false)
                    {
                        self.stop_internal_running(instance_guard, false, None)
                            .await;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            WorkerInstance::WaitingForPermit(_) => false,
            WorkerInstance::Unloaded => false,
        }
    }

    pub fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.worker_event_service.clone()
    }

    pub fn is_loading(&self) -> bool {
        matches!(
            &*self.execution_status.read().unwrap(),
            ExecutionStatus::Loading { .. }
        )
    }

    fn mark_as_loading(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        *execution_status = ExecutionStatus::Loading {
            last_known_status: execution_status.last_known_status().clone(),
            component_type: execution_status.component_type(),
            timestamp: Timestamp::now_utc(),
        };
    }

    /// Updates the cached metadata in execution_status
    async fn update_metadata(&self) -> Result<(), WorkerExecutorError> {
        let previous_metadata = self.get_metadata()?;
        let last_known_status = calculate_last_known_status(
            self,
            &self.owned_worker_id,
            &Some(previous_metadata.clone()),
        )
        .await?;
        let mut execution_status = self.execution_status.write().unwrap();
        execution_status.set_last_known_status(last_known_status);
        Ok(())
    }

    pub fn get_metadata(&self) -> Result<WorkerMetadata, WorkerExecutorError> {
        let updated_status = self
            .execution_status
            .read()
            .unwrap()
            .last_known_status()
            .clone();
        let result = self.initial_worker_metadata.clone();
        Ok(WorkerMetadata {
            last_known_status: updated_status,
            ..result
        })
    }

    /// Marks the worker as interrupting - this should eventually make the worker interrupted.
    /// There are several interruption modes but not all of them are supported by all worker
    /// executor implementations.
    ///
    /// - `Interrupt` means that the worker should be interrupted as soon as possible, and it should
    ///   remain interrupted.
    /// - `Restart` is a simulated crash, the worker gets automatically restarted after it got interrupted,
    ///   but only if the worker context supports recovering workers.
    /// - `Suspend` means that the worker should be moved out of memory and stay in suspended state,
    ///   automatically resumed when the worker is needed again. This only works if the worker context
    ///   supports recovering workers.
    pub async fn set_interrupting(&self, interrupt_kind: InterruptKind) -> Option<Receiver<()>> {
        if let WorkerInstance::Running(running) = &*self.instance.lock().await {
            running.interrupt(interrupt_kind.clone());
        }

        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running {
                last_known_status, ..
            } => {
                let (sender, receiver) = tokio::sync::broadcast::channel(1);
                *execution_status = ExecutionStatus::Interrupting {
                    interrupt_kind,
                    await_interruption: Arc::new(sender),
                    last_known_status,
                    component_type: execution_status.component_type(),
                    timestamp: Timestamp::now_utc(),
                };
                Some(receiver)
            }
            ExecutionStatus::Suspended { .. } => None,
            ExecutionStatus::Interrupting {
                await_interruption, ..
            } => {
                let receiver = await_interruption.subscribe();
                Some(receiver)
            }
            ExecutionStatus::Loading { .. } => None,
        }
    }

    pub async fn resume_replay(&self) -> Result<(), WorkerExecutorError> {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                running
                    .sender
                    .send(WorkerCommand::ResumeReplay)
                    .expect("Failed to send resume command");

                Ok(())
            }
            WorkerInstance::Unloaded | WorkerInstance::WaitingForPermit(_) => {
                Err(WorkerExecutorError::invalid_request(
                    "Explicit resume is not supported for uninitialized workers",
                ))
            }
        }
    }

    pub async fn invoke(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    ) -> Result<ResultOrSubscription, WorkerExecutorError> {
        let output = self.lookup_invocation_result(&idempotency_key).await;

        match output {
            LookupResult::Complete(output) => Ok(ResultOrSubscription::Finished(output)),
            LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
            LookupResult::Pending => {
                let subscription = self.events().subscribe();
                Ok(ResultOrSubscription::Pending(subscription))
            }
            LookupResult::New => {
                // Invoke the function in the background
                let subscription = self.events().subscribe();

                self.enqueue(
                    idempotency_key,
                    full_function_name,
                    function_input,
                    invocation_context,
                )
                .await;
                Ok(ResultOrSubscription::Pending(subscription))
            }
        }
    }

    /// Invokes the worker and awaits for a result.
    ///
    /// Successful result is a `TypeAnnotatedValue` encoding either a tuple or a record.
    pub async fn invoke_and_await(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    ) -> Result<Option<ValueAndType>, WorkerExecutorError> {
        match self
            .invoke(
                idempotency_key.clone(),
                full_function_name,
                function_input,
                invocation_context,
            )
            .await?
        {
            ResultOrSubscription::Finished(Ok(output)) => Ok(output),
            ResultOrSubscription::Finished(Err(err)) => Err(err),
            ResultOrSubscription::Pending(subscription) => {
                debug!("Waiting for idempotency key to complete",);

                let result = self
                    .wait_for_invocation_result(&idempotency_key, subscription)
                    .await;

                debug!("Idempotency key lookup result: {:?}", result);
                match result {
                    Ok(LookupResult::Complete(Ok(output))) => Ok(output),
                    Ok(LookupResult::Complete(Err(err))) => Err(err),
                    Ok(LookupResult::Interrupted) => Err(InterruptKind::Interrupt.into()),
                    Ok(LookupResult::Pending) => Err(WorkerExecutorError::unknown(
                        "Unexpected pending result after invoke",
                    )),
                    Ok(LookupResult::New) => Err(WorkerExecutorError::unknown(
                        "Unexpected missing result after invoke",
                    )),
                    Err(recv_error) => Err(WorkerExecutorError::unknown(format!(
                        "Failed waiting for invocation result: {recv_error}"
                    ))),
                }
            }
        }
    }

    /// Enqueue attempting an update.
    ///
    /// The update itself is not performed by the invocation queue's processing loop,
    /// it is going to affect how the worker is recovered next time.
    pub async fn enqueue_update(&self, update_description: UpdateDescription) {
        let entry = OplogEntry::pending_update(update_description.clone());
        let timestamped_update = TimestampedUpdateDescription {
            timestamp: entry.timestamp(),
            oplog_index: self.oplog.current_oplog_index().await.next(),
            description: update_description,
        };
        self.pending_updates
            .write()
            .await
            .push_back(timestamped_update);
        self.oplog.add_and_commit(entry).await;
        self.update_metadata()
            .await
            .expect("update_metadata failed");
    }

    /// Enqueues a manual update.
    ///
    /// This enqueues a special function invocation that saves the component's state and
    /// triggers a restart immediately.
    pub async fn enqueue_manual_update(&self, target_version: ComponentVersion) {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                running.enqueue_manual_update(target_version).await;
            }
            WorkerInstance::Unloaded | WorkerInstance::WaitingForPermit(_) => {
                debug!("Worker is initializing, persisting manual update request");
                let invocation = WorkerInvocation::ManualUpdate { target_version };
                let entry = OplogEntry::pending_worker_invocation(invocation.clone());
                let timestamped_invocation = TimestampedWorkerInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                };
                self.queue
                    .write()
                    .await
                    .push_back(QueuedWorkerInvocation::External {
                        invocation: timestamped_invocation,
                        canceled: false,
                    });
                self.oplog.add_and_commit(entry).await;
            }
        }
        self.update_metadata()
            .await
            .expect("update_metadata failed");
    }

    pub async fn pending_invocations(&self) -> Vec<TimestampedWorkerInvocation> {
        self.queue
            .read()
            .await
            .iter()
            .filter_map(|inv| inv.as_external_active().cloned())
            .collect()
    }

    pub async fn pending_updates(
        &self,
    ) -> (VecDeque<TimestampedUpdateDescription>, DeletedRegions) {
        let pending_updates = self.pending_updates.read().await.clone();
        let mut deleted_regions = DeletedRegionsBuilder::new();
        if let Some(TimestampedUpdateDescription {
            oplog_index,
            description: UpdateDescription::SnapshotBased { .. },
            ..
        }) = pending_updates.front()
        {
            deleted_regions.add(OplogRegion::from_index_range(
                OplogIndex::INITIAL.next()..=*oplog_index,
            ));
        }

        (pending_updates, deleted_regions.build())
    }

    pub async fn pop_pending_update(&self) -> Option<TimestampedUpdateDescription> {
        self.pending_updates.write().await.pop_front()
    }

    pub async fn peek_pending_update(&self) -> Option<TimestampedUpdateDescription> {
        self.pending_updates.read().await.front().cloned()
    }

    pub async fn invocation_results(&self) -> HashMap<IdempotencyKey, OplogIndex> {
        HashMap::from_iter(
            self.invocation_results
                .read()
                .await
                .iter()
                .map(|(key, result)| (key.clone(), result.oplog_idx())),
        )
    }

    pub async fn store_invocation_success(
        &self,
        key: &IdempotencyKey,
        result: Option<ValueAndType>,
        oplog_index: OplogIndex,
    ) {
        let mut map = self.invocation_results.write().await;
        map.insert(
            key.clone(),
            InvocationResult::Cached {
                result: Ok(result.clone()),
                oplog_idx: oplog_index,
            },
        );
        debug!("Stored invocation success for {key}");
        self.events().publish(Event::InvocationCompleted {
            worker_id: self.owned_worker_id.worker_id(),
            idempotency_key: key.clone(),
            result: Ok(result),
        });
    }

    pub async fn store_invocation_failure(
        &self,
        key: &IdempotencyKey,
        trap_type: &TrapType,
        oplog_index: OplogIndex,
    ) {
        let pending = self.pending_invocations().await;
        let keys_to_fail = [
            vec![key],
            pending
                .iter()
                .filter_map(|entry| entry.invocation.idempotency_key())
                .collect(),
        ]
        .concat();
        let mut map = self.invocation_results.write().await;
        for key in keys_to_fail {
            let stderr = self.worker_event_service.get_last_invocation_errors();
            map.insert(
                key.clone(),
                InvocationResult::Cached {
                    result: Err(FailedInvocationResult {
                        trap_type: trap_type.clone(),
                        stderr: stderr.clone(),
                    }),
                    oplog_idx: oplog_index,
                },
            );
            let golem_error = trap_type.as_golem_error(&stderr);
            if let Some(golem_error) = golem_error {
                self.events().publish(Event::InvocationCompleted {
                    worker_id: self.owned_worker_id.worker_id(),
                    idempotency_key: key.clone(),
                    result: Err(golem_error),
                });
            }
        }
    }

    pub async fn store_invocation_resuming(&self, key: &IdempotencyKey) {
        let mut map = self.invocation_results.write().await;
        map.remove(key);
    }

    pub fn component_type(&self) -> ComponentType {
        self.execution_status.read().unwrap().component_type()
    }

    pub async fn update_status(&self, status_value: WorkerStatusRecord) {
        // Need to make sure the oplog is committed, because the updated status stores the current
        // last oplog index as reference.
        self.oplog().commit(CommitLevel::DurableOnly).await;
        // Storing the status in the key-value storage
        let component_type = self.component_type();
        self.worker_service()
            .update_status(&self.owned_worker_id, &status_value, component_type)
            .await;
        // Updating the status in memory
        self.execution_status
            .write()
            .unwrap()
            .set_last_known_status(status_value);
    }

    /// Gets the estimated memory requirement of the worker
    pub fn memory_requirement(&self) -> Result<u64, WorkerExecutorError> {
        let metadata = self.get_metadata()?;

        let ml = metadata.last_known_status.total_linear_memory_size as f64;
        let sw = metadata.last_known_status.component_size as f64;
        let c = 2.0;
        let x = self.worker_estimate_coefficient;
        Ok((x * (ml + c * sw)) as u64)
    }

    /// Returns true if the worker is running, but it is not performing any invocations at the moment
    /// (ExecutionStatus::Suspended) and has no pending invocation in its invocation queue.
    ///
    /// These workers can be stopped to free up available worker memory.
    pub async fn is_currently_idle_but_running(&self) -> bool {
        match self.instance.try_lock() {
            Ok(guard) => match &*guard {
                WorkerInstance::Running(running) => {
                    let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                    let has_invocations = !self.pending_invocations().await.is_empty();
                    debug!("Worker {} is running, waiting_for_command: {waiting_for_command} has_invocations: {has_invocations}", self.owned_worker_id);
                    waiting_for_command && !has_invocations
                }
                WorkerInstance::WaitingForPermit(_) => {
                    debug!(
                        "Worker {} is waiting for permit, cannot be used to free up memory",
                        self.owned_worker_id
                    );
                    false
                }
                WorkerInstance::Unloaded => {
                    debug!(
                        "Worker {} is unloaded, cannot be used to free up memory",
                        self.owned_worker_id
                    );
                    false
                }
            },
            Err(_) => false,
        }
    }

    /// Gets the timestamp of the last time the execution status changed
    pub fn last_execution_state_change(&self) -> Timestamp {
        self.execution_status.read().unwrap().timestamp()
    }

    pub async fn increase_memory(&self, delta: u64) -> anyhow::Result<()> {
        match &mut *self.instance.lock().await {
            WorkerInstance::Running(ref mut running) => {
                if let Some(new_permits) = self.active_workers().try_acquire(delta).await {
                    running.merge_extra_permits(new_permits);
                    Ok(())
                } else {
                    Err(anyhow!(WorkerOutOfMemory))
                }
            }
            WorkerInstance::WaitingForPermit(_) => Ok(()),
            WorkerInstance::Unloaded => Ok(()),
        }
    }

    /// Enqueue invocation of an exported function
    async fn enqueue(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    ) {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                running
                    .enqueue(
                        idempotency_key,
                        full_function_name,
                        function_input,
                        invocation_context,
                    )
                    .await;
            }
            WorkerInstance::Unloaded | WorkerInstance::WaitingForPermit(_) => {
                debug!("Worker is initializing, persisting pending invocation");
                let invocation = WorkerInvocation::ExportedFunction {
                    idempotency_key,
                    full_function_name,
                    function_input,
                    invocation_context,
                };
                let entry = OplogEntry::pending_worker_invocation(invocation.clone());
                let timestamped_invocation = TimestampedWorkerInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                };
                self.queue
                    .write()
                    .await
                    .push_back(QueuedWorkerInvocation::External {
                        invocation: timestamped_invocation,
                        canceled: false,
                    });
                self.oplog.add_and_commit(entry).await;
            }
        }

        self.update_metadata()
            .await
            .expect("update_metadata failed");
    }

    pub async fn get_file_system_node(
        &self,
        path: ComponentFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        let (sender, receiver) = oneshot::channel();

        let mutex = self.instance.lock().await;

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::GetFileSystemNode { path, sender });

        // Two cases here:
        // - Worker is running, we can send the invocation command and the worker will look at the queue immediately
        // - Worker is starting, it will process the request when it is started

        if let WorkerInstance::Running(running) = &*mutex {
            running.sender.send(WorkerCommand::Invocation).unwrap();
        };

        drop(mutex);

        receiver.await.unwrap()
    }

    pub async fn read_file(
        &self,
        path: ComponentFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        let (sender, receiver) = oneshot::channel();

        let mutex = self.instance.lock().await;

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::ReadFile { path, sender });

        if let WorkerInstance::Running(running) = &*mutex {
            running.sender.send(WorkerCommand::Invocation).unwrap();
        };

        drop(mutex);

        receiver.await.unwrap()
    }

    pub async fn activate_plugin(
        &self,
        plugin_installation_id: PluginInstallationId,
    ) -> Result<(), WorkerExecutorError> {
        self.oplog
            .add_and_commit(OplogEntry::activate_plugin(plugin_installation_id))
            .await;
        self.update_metadata().await?;
        Ok(())
    }

    pub async fn deactivate_plugin(
        &self,
        plugin_installation_id: PluginInstallationId,
    ) -> Result<(), WorkerExecutorError> {
        self.oplog
            .add_and_commit(OplogEntry::deactivate_plugin(plugin_installation_id))
            .await;
        self.update_metadata().await?;
        Ok(())
    }

    /// Reverts the worker to a previous state, selected by either the last oplog index to keep
    /// or the number of invocations to drop.
    ///
    /// The revert operations is implemented by inserting a special oplog entry that
    /// extends the worker's deleted oplog regions, skipping entries from the end of the oplog.
    pub async fn revert(&self, target: RevertWorkerTarget) -> Result<(), WorkerExecutorError> {
        match target {
            RevertWorkerTarget::RevertToOplogIndex(target) => {
                self.revert_to_last_oplog_index(target.last_oplog_index)
                    .await
            }
            RevertWorkerTarget::RevertLastInvocations(target) => {
                if let Some(last_oplog_index) = self
                    .find_nth_invocation_from_end(target.number_of_invocations as usize)
                    .await
                {
                    self.revert_to_last_oplog_index(last_oplog_index.previous())
                        .await
                } else {
                    Err(WorkerExecutorError::invalid_request(format!(
                        "Could not find {} invocations to revert",
                        target.number_of_invocations
                    )))
                }
            }
        }
    }

    pub async fn cancel_invocation(
        &self,
        idempotency_key: IdempotencyKey,
    ) -> Result<(), WorkerExecutorError> {
        let mut queue = self.queue.write().await;
        for item in queue.iter_mut() {
            if item.matches_idempotency_key(&idempotency_key) {
                if let QueuedWorkerInvocation::External { canceled, .. } = item {
                    *canceled = true;
                }
            }
        }

        self.oplog
            .add_and_commit(OplogEntry::cancel_pending_invocation(idempotency_key))
            .await;
        self.update_metadata().await?;

        Ok(())
    }

    /// Starting from the end of the oplog, find the Nth ExportedFunctionInvoked entry's index.
    async fn find_nth_invocation_from_end(&self, n: usize) -> Option<OplogIndex> {
        let mut current = self.oplog.current_oplog_index().await;
        let mut found = 0;
        loop {
            let entry = self.oplog.read(current).await;

            if matches!(entry, OplogEntry::ExportedFunctionInvoked { .. }) {
                found += 1;
                if found == n {
                    return Some(current);
                }
            }

            if current == OplogIndex::INITIAL {
                return None;
            } else {
                current = current.previous();
            }
        }
    }

    async fn revert_to_last_oplog_index(
        &self,
        last_oplog_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.stop().await;

        let region_end = self.oplog.current_oplog_index().await;
        let region_start = last_oplog_index.next();
        let metadata = self.get_metadata()?;

        if metadata
            .last_known_status
            .skipped_regions
            .is_in_deleted_region(region_start)
        {
            Err(WorkerExecutorError::invalid_request(format!(
                "Attempted to revert to a deleted region in oplog to index {last_oplog_index}"
            )))
        } else {
            let region = OplogRegion {
                start: region_start,
                end: region_end,
            };

            // Resetting the worker status so it is recalculated even if the server crashes
            self.worker_service()
                .update_status(
                    &self.owned_worker_id,
                    &WorkerStatusRecord::default(),
                    self.component_type(),
                )
                .await;
            self.oplog.add_and_commit(OplogEntry::revert(region)).await;

            // Recalculating the status from the whole oplog, because the newly deleted region may contain things like worker updates.
            let recalculated_status =
                calculate_last_known_status(self, &self.owned_worker_id, &None).await?;
            self.worker_service()
                .update_status(
                    &self.owned_worker_id,
                    &recalculated_status,
                    self.component_type(),
                )
                .await;
            let mut execution_status = self.execution_status.write().unwrap();
            execution_status.set_last_known_status(recalculated_status.clone());

            Ok(())
        }
    }

    async fn wait_for_invocation_result(
        &self,
        key: &IdempotencyKey,
        mut subscription: EventsSubscription,
    ) -> Result<LookupResult, RecvError> {
        loop {
            match self.lookup_invocation_result(key).await {
                LookupResult::Interrupted => break Ok(LookupResult::Interrupted),
                LookupResult::New | LookupResult::Pending => {
                    let wait_result = subscription
                        .wait_for(|event| match event {
                            Event::InvocationCompleted {
                                worker_id,
                                idempotency_key,
                                result,
                            } if *worker_id == self.owned_worker_id.worker_id
                                && idempotency_key == key =>
                            {
                                Some(LookupResult::Complete(result.clone()))
                            }
                            _ => None,
                        })
                        .await;
                    match wait_result {
                        Ok(result) => break Ok(result),
                        Err(RecvError::Lagged(_)) => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                        Err(RecvError::Closed) => break Err(RecvError::Closed),
                    }
                }
                LookupResult::Complete(result) => break Ok(LookupResult::Complete(result)),
            }
        }
    }

    async fn lookup_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
        let maybe_result = self.invocation_results.read().await.get(key).cloned();
        if let Some(mut result) = maybe_result {
            result.cache(&self.owned_worker_id, self).await;
            match result {
                InvocationResult::Cached {
                    result: Ok(values), ..
                } => LookupResult::Complete(Ok(values)),
                InvocationResult::Cached {
                    result:
                        Err(FailedInvocationResult {
                            trap_type: TrapType::Interrupt(InterruptKind::Interrupt),
                            ..
                        }),
                    ..
                } => LookupResult::Interrupted,
                InvocationResult::Cached {
                    result:
                        Err(FailedInvocationResult {
                            trap_type: TrapType::Interrupt(_),
                            ..
                        }),
                    ..
                } => LookupResult::Pending,
                InvocationResult::Cached {
                    result:
                        Err(FailedInvocationResult {
                            trap_type: TrapType::Error(error),
                            stderr,
                        }),
                    ..
                } => LookupResult::Complete(Err(WorkerExecutorError::InvocationFailed {
                    error,
                    stderr,
                })),
                InvocationResult::Cached {
                    result:
                        Err(FailedInvocationResult {
                            trap_type: TrapType::Exit,
                            ..
                        }),
                    ..
                } => LookupResult::Complete(Err(WorkerExecutorError::runtime("Process exited"))),
                InvocationResult::Lazy { .. } => {
                    panic!("Unexpected lazy result after InvocationResult.cache")
                }
            }
        } else {
            let is_pending = self
                .pending_invocations()
                .await
                .iter()
                .any(|entry| entry.invocation.is_idempotency_key(key));
            if is_pending {
                LookupResult::Pending
            } else {
                LookupResult::New
            }
        }
    }

    async fn stop_internal(
        &self,
        called_from_invocation_loop: bool,
        fail_pending_invocations: Option<WorkerExecutorError>,
    ) {
        // we don't want to re-enter stop from within the invocation loop
        if self
            .stopping
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            == Ok(false)
        {
            let instance = self.instance.lock().await;
            self.stop_internal_running(
                instance,
                called_from_invocation_loop,
                fail_pending_invocations,
            )
            .await;
        }
    }

    async fn stop_internal_running(
        &self,
        mut instance: MutexGuard<'_, WorkerInstance>,
        called_from_invocation_loop: bool,
        fail_pending_invocations: Option<WorkerExecutorError>,
    ) {
        if let WorkerInstance::Running(running) = instance.unload() {
            debug!("Stopping running worker ({called_from_invocation_loop})");
            let queued_items = running
                .queue
                .write()
                .await
                .drain(..)
                .collect::<VecDeque<_>>();

            if let Some(fail_pending_invocations) = fail_pending_invocations {
                // Publishing the provided initialization error to all pending invocations.
                // We cannot persist these failures, so they remain pending in the oplog, and
                // on next recovery they will be retried, but we still want waiting callers
                // to get the error.
                for item in queued_items {
                    match item {
                        QueuedWorkerInvocation::External {
                            invocation: inner,
                            canceled,
                        } => {
                            if !canceled {
                                if let Some(idempotency_key) = inner.invocation.idempotency_key() {
                                    self.events().publish(Event::InvocationCompleted {
                                        worker_id: self.owned_worker_id.worker_id(),
                                        idempotency_key: idempotency_key.clone(),
                                        result: Err(fail_pending_invocations.clone()),
                                    })
                                }
                            }
                        }
                        QueuedWorkerInvocation::GetFileSystemNode { sender, .. } => {
                            let _ = sender.send(Err(fail_pending_invocations.clone()));
                        }
                        QueuedWorkerInvocation::ReadFile { sender, .. } => {
                            let _ = sender.send(Err(fail_pending_invocations.clone()));
                        }
                        QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                            let _ = sender.send(Err(fail_pending_invocations.clone()));
                        }
                    }
                }
            } else {
                *self.queue.write().await = queued_items;
            }

            if !called_from_invocation_loop {
                // If stop was called from outside, we wait until the invocation queue stops
                // (it happens by `running` getting dropped)
                let run_loop_handle = running.stop(); // this drops `running`
                run_loop_handle.await.expect("Worker run loop failed");
            }
        } else {
            debug!("Worker was already stopped");
        }
        self.stopping.store(false, Ordering::Release);
    }

    async fn restart_on_oom(
        this: Arc<Worker<Ctx>>,
        called_from_invocation_loop: bool,
        delay: Option<Duration>,
        oom_retry_count: u64,
    ) -> Result<bool, WorkerExecutorError> {
        this.stop_internal(called_from_invocation_loop, None).await;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Self::start_if_needed_internal(this, oom_retry_count).await
    }

    async fn get_or_create_worker_metadata<
        T: HasWorkerService + HasComponentService + HasConfig + HasOplogService + Sync,
    >(
        this: &T,
        account_id: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        component_version: Option<ComponentVersion>,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        worker_wasi_config_vars: Option<BTreeMap<String, String>>,
        parent: Option<WorkerId>,
    ) -> Result<(WorkerMetadata, Arc<std::sync::RwLock<ExecutionStatus>>), WorkerExecutorError>
    {
        let component_id = owned_worker_id.component_id();
        let component = this
            .component_service()
            .get_metadata(
                &owned_worker_id.project_id,
                &component_id,
                component_version,
            )
            .await?;

        let worker_env = merge_worker_env_with_component_env(worker_env, component.env);

        match this.worker_service().get(owned_worker_id).await {
            None => {
                let initial_status =
                    calculate_last_known_status(this, owned_worker_id, &None).await?;
                let worker_metadata = WorkerMetadata {
                    worker_id: owned_worker_id.worker_id(),
                    args: worker_args.unwrap_or_default(),
                    env: worker_env,
                    wasi_config_vars: worker_wasi_config_vars.unwrap_or_default(),
                    project_id: owned_worker_id.project_id(),
                    created_by: account_id.clone(),
                    created_at: Timestamp::now_utc(),
                    parent,
                    last_known_status: WorkerStatusRecord {
                        component_version: component.versioned_component_id.version,
                        component_version_for_replay: component.versioned_component_id.version,
                        component_size: component.component_size,
                        total_linear_memory_size: component
                            .metadata
                            .memories()
                            .iter()
                            .map(|m| m.initial)
                            .sum(),
                        active_plugins: component
                            .installed_plugins
                            .iter()
                            .map(|i| i.id.clone())
                            .collect(),
                        ..initial_status
                    },
                };
                let execution_status = this
                    .worker_service()
                    .add(&worker_metadata, component.component_type)
                    .await?;
                Ok((worker_metadata, execution_status))
            }
            Some(previous_metadata) => {
                let worker_metadata = WorkerMetadata {
                    last_known_status: calculate_last_known_status(
                        this,
                        owned_worker_id,
                        &Some(previous_metadata.clone()),
                    )
                    .await?,
                    ..previous_metadata
                };
                let execution_status =
                    Arc::new(std::sync::RwLock::new(ExecutionStatus::Suspended {
                        last_known_status: worker_metadata.last_known_status.clone(),
                        component_type: component.component_type,
                        timestamp: Timestamp::now_utc(),
                    }));
                Ok((worker_metadata, execution_status))
            }
        }
    }

    pub async fn await_ready_to_process_commands(&self) -> Result<(), WorkerExecutorError> {
        let (sender, receiver) = oneshot::channel();

        let mutex = self.instance.lock().await;

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender });

        if let WorkerInstance::Running(running) = &*mutex {
            running.sender.send(WorkerCommand::Invocation).unwrap();
        };

        drop(mutex);

        receiver.await.unwrap()
    }
}

pub fn merge_worker_env_with_component_env(
    worker_env: Option<Vec<(String, String)>>,
    component_env: HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut seen_keys = HashSet::new();
    let mut result = Vec::new();

    if let Some(worker_env) = worker_env {
        for (key, value) in worker_env {
            seen_keys.insert(key.clone());
            result.push((key, value));
        }
    }

    for (key, value) in component_env {
        // Prioritise per worker environment variables all the time
        if !seen_keys.contains(&key) {
            result.push((key, value));
        }
    }

    result
}

enum WorkerInstance {
    Unloaded,
    #[allow(dead_code)]
    WaitingForPermit(WaitingWorker),
    Running(RunningWorker),
}

impl WorkerInstance {
    pub fn is_unloaded(&self) -> bool {
        matches!(self, WorkerInstance::Unloaded)
    }

    #[allow(unused)]
    pub fn is_running(&self) -> bool {
        matches!(self, WorkerInstance::Running(_))
    }

    #[allow(unused)]
    pub fn is_waiting_for_permit(&self) -> bool {
        matches!(self, WorkerInstance::WaitingForPermit(_))
    }

    pub fn unload(&mut self) -> WorkerInstance {
        mem::replace(self, WorkerInstance::Unloaded)
    }
}

struct WaitingWorker {
    handle: Option<JoinHandle<()>>,
}

impl WaitingWorker {
    pub fn new<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
        memory_requirement: u64,
        oom_retry_count: u64,
    ) -> Self {
        let span = span!(
            Level::INFO,
            "waiting-for-permits",
            worker_id = parent.owned_worker_id.worker_id.to_string(),
        );
        let handle = tokio::task::spawn(
            async move {
                let permit = parent.active_workers().acquire(memory_requirement).await;
                Worker::start_with_permit(parent, permit, oom_retry_count).await;
            }
            .instrument(span),
        );
        WaitingWorker {
            handle: Some(handle),
        }
    }
}

impl Drop for WaitingWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

struct RunningWorker {
    handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<WorkerCommand>,
    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,

    oplog: Arc<dyn Oplog>,

    permit: OwnedSemaphorePermit,
    waiting_for_command: Arc<AtomicBool>,
}

impl RunningWorker {
    pub async fn new<Ctx: WorkerCtx>(
        owned_worker_id: OwnedWorkerId,
        queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        parent: Arc<Worker<Ctx>>,
        oplog: Arc<dyn Oplog>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        permit: OwnedSemaphorePermit,
        oom_retry_count: u64,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        // Preload
        for _ in 0..queue.read().await.len() {
            sender.send(WorkerCommand::Invocation).unwrap();
        }

        let active_clone = queue.clone();
        let owned_worker_id_clone = owned_worker_id.clone();
        let waiting_for_command = Arc::new(AtomicBool::new(false));
        let waiting_for_command_clone = waiting_for_command.clone();

        let span = span!(
            Level::INFO,
            "invocation-loop",
            worker_id = parent.owned_worker_id.worker_id.to_string(),
        );
        let handle = tokio::task::spawn(
            async move {
                RunningWorker::invocation_loop(
                    receiver,
                    active_clone,
                    owned_worker_id_clone,
                    parent,
                    waiting_for_command_clone,
                    oom_retry_count,
                )
                .instrument(span)
                .await;
            }
            .in_current_span(),
        );

        RunningWorker {
            handle: Some(handle),
            sender,
            queue,
            oplog,
            execution_status,
            permit,
            waiting_for_command,
        }
    }

    pub fn merge_extra_permits(&mut self, extra_permit: OwnedSemaphorePermit) {
        self.permit.merge(extra_permit);
    }

    pub fn stop(mut self) -> JoinHandle<()> {
        self.handle.take().unwrap()
    }

    pub async fn enqueue(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    ) {
        let invocation = WorkerInvocation::ExportedFunction {
            idempotency_key,
            full_function_name,
            function_input,
            invocation_context,
        };
        self.enqueue_worker_invocation(invocation).await;
    }

    pub async fn enqueue_manual_update(&self, target_version: ComponentVersion) {
        let invocation = WorkerInvocation::ManualUpdate { target_version };
        self.enqueue_worker_invocation(invocation).await;
    }

    async fn enqueue_worker_invocation(&self, invocation: WorkerInvocation) {
        let entry = OplogEntry::pending_worker_invocation(invocation.clone());
        let timestamped_invocation = TimestampedWorkerInvocation {
            timestamp: entry.timestamp(),
            invocation,
        };
        if self.execution_status.read().unwrap().is_running() {
            debug!("Worker is busy, persisting pending invocation",);
            // The worker is currently busy, so we write the pending worker invocation to the oplog
            self.oplog.add_and_commit(entry).await;
        }
        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::External {
                invocation: timestamped_invocation,
                canceled: false,
            });
        self.sender.send(WorkerCommand::Invocation).unwrap()
    }

    fn interrupt(&self, kind: InterruptKind) {
        // In some cases it is possible that the invocation loop is already quitting and the receiver gets
        // dropped when we get here. In this case the send fails, but we ignore it as the running worker got
        // interrupted anyway.
        let _ = self.sender.send(WorkerCommand::Interrupt(kind));
    }

    async fn create_instance<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
    ) -> Result<(Instance, async_mutex::Mutex<Store<Ctx>>), WorkerExecutorError> {
        let project_id = parent.owned_worker_id.project_id();
        let component_id = parent.owned_worker_id.component_id();
        let worker_metadata = parent.get_metadata()?;

        let (component, component_metadata) = {
            let component_version = worker_metadata
                .last_known_status
                .pending_updates
                .front()
                .map_or(
                    worker_metadata.last_known_status.component_version,
                    |update| {
                        let target_version = *update.description.target_version();
                        info!(
                            "Attempting {} update from {} to version {target_version}",
                            match update.description {
                                UpdateDescription::Automatic { .. } => "automatic",
                                UpdateDescription::SnapshotBased { .. } => "snapshot based",
                            },
                            worker_metadata.last_known_status.component_version
                        );
                        target_version
                    },
                );

            match parent
                .component_service()
                .get(
                    &parent.engine(),
                    &project_id,
                    &component_id,
                    component_version,
                )
                .await
            {
                Ok((component, component_metadata)) => Ok((component, component_metadata)),
                Err(error) => {
                    if component_version != worker_metadata.last_known_status.component_version {
                        // An update was attempted but the targeted version does not exist
                        warn!(
                            "Attempting update to version {component_version} failed with {error}"
                        );

                        parent.pop_pending_update().await;
                        Ctx::on_worker_update_failed_to_start(
                            &parent.deps,
                            &parent.initial_worker_metadata.created_by,
                            &parent.owned_worker_id,
                            component_version,
                            Some(error.to_string()),
                        )
                        .await?;

                        parent
                            .component_service()
                            .get(
                                &parent.engine(),
                                &project_id,
                                &component_id,
                                worker_metadata.last_known_status.component_version,
                            )
                            .await
                    } else {
                        Err(error)
                    }
                }
            }?
        };

        let component_env = component_metadata.env.clone();

        let worker_env =
            merge_worker_env_with_component_env(Some(worker_metadata.env), component_env);

        let component_version_for_replay = worker_metadata
            .last_known_status
            .pending_updates
            .front()
            .and_then(|update| match update.description {
                UpdateDescription::SnapshotBased { target_version, .. } => Some(target_version),
                _ => None,
            })
            .unwrap_or(
                worker_metadata
                    .last_known_status
                    .component_version_for_replay,
            );

        let context = Ctx::create(
            worker_metadata.created_by,
            OwnedWorkerId::new(&worker_metadata.project_id, &worker_metadata.worker_id),
            parent.promise_service(),
            parent.worker_service(),
            parent.worker_enumeration_service(),
            parent.key_value_service(),
            parent.blob_store_service(),
            parent.rdbms_service(),
            parent.worker_event_service.clone(),
            parent.active_workers(),
            parent.oplog_service(),
            parent.oplog.clone(),
            Arc::downgrade(&parent),
            parent.scheduler_service(),
            parent.rpc(),
            parent.worker_proxy(),
            parent.component_service(),
            parent.extra_deps(),
            parent.config(),
            WorkerConfig::new(
                worker_metadata.worker_id.clone(),
                component_metadata.versioned_component_id.version,
                worker_metadata.args.clone(),
                worker_env,
                worker_metadata.last_known_status.skipped_regions.clone(),
                worker_metadata.last_known_status.total_linear_memory_size,
                component_version_for_replay,
                parent.initial_worker_metadata.created_by.clone(),
                worker_metadata.wasi_config_vars,
            ),
            parent.execution_status.clone(),
            parent.file_loader(),
            parent.plugins(),
            parent.worker_fork_service(),
            parent.resource_limits(),
            parent.project_service(),
        )
        .await?;

        let engine = parent.engine();
        let mut store = Store::new(&engine, context);
        store.set_epoch_deadline(parent.config().limits.epoch_ticks);
        let worker_id_clone = worker_metadata.worker_id.clone();
        store.epoch_deadline_callback(move |mut store| {
            let current_level = store.get_fuel().unwrap_or(0);
            if store.data().is_out_of_fuel(current_level as i64) {
                debug!("{worker_id_clone} ran out of fuel, borrowing more");
                store.data_mut().borrow_fuel_sync();
            }

            match store.data_mut().check_interrupt() {
                Some(kind) => Err(kind.into()),
                None => Ok(UpdateDeadline::Yield(1)),
            }
        });

        store.set_fuel(i64::MAX as u64)?;
        store.data_mut().borrow_fuel().await?; // Borrowing fuel for initialization and also to make sure account is in cache

        store.limiter_async(|ctx| ctx.resource_limiter());

        let mut linker = (*parent.linker()).clone(); // fresh linker
        store
            .data_mut()
            .link(&engine, &mut linker, &component, &component_metadata)?;

        let instance_pre = linker.instantiate_pre(&component).map_err(|e| {
            WorkerExecutorError::worker_creation_failed(
                parent.owned_worker_id.worker_id(),
                format!(
                    "Failed to pre-instantiate worker {}: {e}",
                    parent.owned_worker_id
                ),
            )
        })?;

        let instance = instance_pre
            .instantiate_async(&mut store)
            .await
            .map_err(|e| {
                WorkerExecutorError::worker_creation_failed(
                    parent.owned_worker_id.worker_id(),
                    format!(
                        "Failed to instantiate worker {}: {e}",
                        parent.owned_worker_id
                    ),
                )
            })?;
        let store = async_mutex::Mutex::new(store);
        Ok((instance, store))
    }

    async fn invocation_loop<Ctx: WorkerCtx>(
        receiver: UnboundedReceiver<WorkerCommand>,
        active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        owned_worker_id: OwnedWorkerId,
        parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
        waiting_for_command: Arc<AtomicBool>,
        oom_retry_count: u64,
    ) {
        let mut invocation_loop = InvocationLoop {
            receiver,
            active,
            owned_worker_id,
            parent,
            waiting_for_command,
            oom_retry_count,
        };
        invocation_loop.run().await;
    }
}

#[derive(Debug, Clone)]
struct FailedInvocationResult {
    pub trap_type: TrapType,
    pub stderr: String,
}

#[derive(Debug, Clone)]
enum InvocationResult {
    Cached {
        result: Result<Option<ValueAndType>, FailedInvocationResult>,
        oplog_idx: OplogIndex,
    },
    Lazy {
        oplog_idx: OplogIndex,
    },
}

impl InvocationResult {
    pub fn oplog_idx(&self) -> OplogIndex {
        match self {
            Self::Cached { oplog_idx, .. } | Self::Lazy { oplog_idx } => *oplog_idx,
        }
    }

    pub async fn cache<T: HasOplog + HasOplogService + HasConfig>(
        &mut self,
        owned_worker_id: &OwnedWorkerId,
        services: &T,
    ) {
        if let Self::Lazy { oplog_idx } = self {
            let oplog_idx = *oplog_idx;
            let entry = services.oplog().read(oplog_idx).await;

            let result = match entry {
                OplogEntry::ExportedFunctionCompleted { .. } => {
                    let value: Option<ValueAndType> =
                        services.oplog().get_payload_of_entry(&entry).await.expect("failed to deserialize function response payload").unwrap();

                    Ok(value)
                }
                OplogEntry::Error { error, .. } => {
                    let stderr = recover_stderr_logs(services, owned_worker_id, oplog_idx).await;
                    Err(FailedInvocationResult { trap_type: TrapType::Error(error), stderr })
                }
                OplogEntry::Interrupted { .. } => Err(FailedInvocationResult { trap_type: TrapType::Interrupt(InterruptKind::Interrupt), stderr: "".to_string() }),
                OplogEntry::Exited { .. } => Err(FailedInvocationResult { trap_type: TrapType::Exit, stderr: "".to_string() }),
                _ => panic!("Unexpected oplog entry pointed by invocation result at index {oplog_idx} for {owned_worker_id:?}")
            };

            *self = Self::Cached { result, oplog_idx }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum RetryDecision {
    /// Immediately retry by recreating the instance using the existing permits
    Immediate,
    /// Retry after a given delay by recreating the instance using the existing permits
    Delayed(Duration),
    /// No retry possible
    None,
    /// Retry immediately but drop and reacquire permits
    ReacquirePermits,
}

#[derive(Debug)]
enum WorkerCommand {
    Invocation,
    ResumeReplay,
    Interrupt(InterruptKind),
}

pub fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &WorkerError,
    retry_count: u64,
) -> bool {
    match error {
        WorkerError::Unknown(_) => retry_count < (retry_config.max_attempts as u64),
        WorkerError::InvalidRequest(_) => false,
        WorkerError::StackOverflow => false,
        WorkerError::OutOfMemory => true,
    }
}

async fn is_running_worker_idle(running: &RunningWorker) -> bool {
    running.waiting_for_command.load(Ordering::Acquire) && running.queue.read().await.is_empty()
}

#[derive(Debug)]
pub enum QueuedWorkerInvocation {
    /// 'Real' invocations that make sense from a domain model point of view and should be exposed to the user.
    /// All other cases here are used for concurrency control and should not be exposed to the user.
    External {
        invocation: TimestampedWorkerInvocation,
        canceled: bool,
    },
    GetFileSystemNode {
        path: ComponentFilePath,
        sender: oneshot::Sender<Result<GetFileSystemNodeResult, WorkerExecutorError>>,
    },
    // The worker will suspend execution until the stream is dropped, so consume in a timely manner.
    ReadFile {
        path: ComponentFilePath,
        sender: oneshot::Sender<Result<ReadFileResult, WorkerExecutorError>>,
    },
    // Waits for the invocation loop to pick up this message, ensuring that the worker is ready to process followup commands.
    // The sender will be called with Ok if the worker is in a running state.
    // If the worker initializaiton fails and will not recover without manual intervention it will be called with Err.
    AwaitReadyToProcessCommands {
        sender: oneshot::Sender<Result<(), WorkerExecutorError>>,
    },
}

impl QueuedWorkerInvocation {
    fn as_external_active(&self) -> Option<&TimestampedWorkerInvocation> {
        match self {
            Self::External {
                invocation,
                canceled: false,
            } => Some(invocation),
            _ => None,
        }
    }

    fn matches_idempotency_key(&self, idempotency_key: &IdempotencyKey) -> bool {
        match self {
            Self::External { invocation, .. } => {
                invocation.invocation.idempotency_key() == Some(idempotency_key)
            }
            _ => false,
        }
    }
}

pub enum ResultOrSubscription {
    Finished(Result<Option<ValueAndType>, WorkerExecutorError>),
    Pending(EventsSubscription),
}

pub fn interpret_function_result(
    function_results: Option<Value>,
    expected_types: Option<AnalysedFunctionResult>,
) -> Result<Option<ValueAndType>, Vec<String>> {
    match (function_results, expected_types) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(vec![
            "Unexpected result value (got some, expected: none)".to_string()
        ]),
        (None, Some(_)) => Err(vec![
            "Unexpected result value (got none, expected: some)".to_string()
        ]),
        (Some(value), Some(expected)) => Ok(Some(ValueAndType::new(value, expected.typ))),
    }
}
