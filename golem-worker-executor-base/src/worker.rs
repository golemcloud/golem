// Copyright 2024-2025 Golem Cloud
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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::mem;
use std::ops::DerefMut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::durable_host::recover_stderr_logs;
use crate::error::{GolemError, WorkerOutOfMemory};
use crate::function_result_interpreter::interpret_function_results;
use crate::invocation::{find_first_available_function, invoke_worker, InvokeResult};
use crate::model::{
    ExecutionStatus, InterruptKind, ListDirectoryResult, LookupResult, ReadFileResult, TrapType,
    WorkerConfig,
};
use crate::services::component::ComponentMetadata;
use crate::services::events::Event;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps};
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveWorkers, HasAll, HasBlobStoreService, HasComponentService, HasConfig, HasEvents,
    HasExtraDeps, HasFileLoader, HasKeyValueService, HasOplog, HasOplogService, HasPlugins,
    HasPromiseService, HasRpc, HasSchedulerService, HasWasmtimeEngine, HasWorker,
    HasWorkerEnumerationService, HasWorkerProxy, HasWorkerService, UsesAllDeps,
};
use crate::workerctx::{PublicWorkerIo, WorkerCtx};
use anyhow::anyhow;
use drop_stream::DropStream;
use futures::channel::oneshot;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription, WorkerError,
    WorkerResourceId,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::RetryConfig;
use golem_common::model::{
    exports, ComponentFilePath, ComponentType, PluginInstallationId, WorkerStatusRecordExtensions,
};
use golem_common::model::{
    ComponentVersion, FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, SuccessfulUpdateRecord,
    Timestamp, TimestampedWorkerInvocation, WorkerId, WorkerInvocation, WorkerMetadata,
    WorkerResourceDescription, WorkerStatus, WorkerStatusRecord,
};
use golem_common::retries::get_delay;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, MutexGuard, OwnedSemaphorePermit};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, span, warn, Instrument, Level};
use wasmtime::component::Instance;
use wasmtime::{AsContext, Store, UpdateDeadline};

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

    oplog: Arc<dyn Oplog + Send + Sync>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>, // TODO: rename

    deps: All<Ctx>,

    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    pending_updates: Arc<RwLock<VecDeque<TimestampedUpdateDescription>>>,

    invocation_results: Arc<RwLock<HashMap<IdempotencyKey, InvocationResult>>>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
    stopping: AtomicBool,
    worker_estimate_coefficient: f64,

    instance: Arc<Mutex<WorkerInstance>>,
    oom_retry_config: RetryConfig,
}

impl<Ctx: WorkerCtx> HasOplog for Worker<Ctx> {
    fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
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
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        deps.active_workers()
            .get_or_add(
                deps,
                owned_worker_id,
                worker_args,
                worker_env,
                component_version,
                parent,
            )
            .await
    }

    /// Gets or creates a worker and makes sure it is running
    pub async fn get_or_create_running<T>(
        deps: &T,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker = Self::get_or_create_suspended(
            deps,
            owned_worker_id,
            worker_args,
            worker_env,
            component_version,
            parent,
        )
        .await?;
        Self::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    pub async fn new<T: HasAll<Ctx>>(
        deps: &T,
        owned_worker_id: OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Self, GolemError> {
        let (worker_metadata, execution_status) = Self::get_or_create_worker_metadata(
            deps,
            &owned_worker_id,
            component_version,
            worker_args,
            worker_env,
            parent,
        )
        .await?;
        let initial_component_metadata = deps
            .component_service()
            .get_metadata(
                &owned_worker_id.account_id,
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
                .map(|inv| QueuedWorkerInvocation::External(inv.clone())),
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
            event_service: Arc::new(WorkerEventServiceDefault::new(
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

    pub fn oom_retry_config(&self) -> &RetryConfig {
        &self.oom_retry_config
    }

    pub async fn start_if_needed(this: Arc<Worker<Ctx>>) -> Result<bool, GolemError> {
        Self::start_if_needed_internal(this, 0).await
    }

    async fn start_if_needed_internal(
        this: Arc<Worker<Ctx>>,
        oom_retry_count: u64,
    ) -> Result<bool, GolemError> {
        let mut instance = this.instance.lock().await;
        if instance.is_unloaded() {
            this.mark_as_loading();
            *instance = WorkerInstance::WaitingForPermit(WaitingWorker::new(
                this.clone(),
                this.memory_requirement().await?,
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
        *instance = WorkerInstance::Running(RunningWorker::new(
            this.owned_worker_id.clone(),
            this.queue.clone(),
            this.clone(),
            this.oplog(),
            this.execution_status.clone(),
            permit,
            oom_retry_count,
        ));
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
                if is_running_worker_idle(running) {
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
        self.event_service.clone()
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
    async fn update_metadata(&self) -> Result<(), GolemError> {
        let previous_metadata = self.get_metadata().await?;
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

    pub async fn get_metadata(&self) -> Result<WorkerMetadata, GolemError> {
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
    ///    remain interrupted.
    /// - `Restart` is a simulated crash, the worker gets automatically restarted after it got interrupted,
    ///    but only if the worker context supports recovering workers.
    /// - `Suspend` means that the worker should be moved out of memory and stay in suspended state,
    ///    automatically resumed when the worker is needed again. This only works if the worker context
    ///    supports recovering workers.
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

    pub async fn invoke(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
    ) -> Result<Option<Result<TypeAnnotatedValue, GolemError>>, GolemError> {
        let output = self.lookup_invocation_result(&idempotency_key).await;

        match output {
            LookupResult::Complete(output) => Ok(Some(output)),
            LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
            LookupResult::Pending => Ok(None),
            LookupResult::New => {
                // Invoke the function in the background
                self.enqueue(idempotency_key, full_function_name, function_input)
                    .await;
                Ok(None)
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
    ) -> Result<TypeAnnotatedValue, GolemError> {
        match self
            .invoke(idempotency_key.clone(), full_function_name, function_input)
            .await?
        {
            Some(Ok(output)) => Ok(output),
            Some(Err(err)) => Err(err),
            None => {
                debug!("Waiting for idempotency key to complete",);

                let result = self.wait_for_invocation_result(&idempotency_key).await;

                debug!("Idempotency key lookup result: {:?}", result);
                match result {
                    Ok(LookupResult::Complete(Ok(output))) => Ok(output),
                    Ok(LookupResult::Complete(Err(err))) => Err(err),
                    Ok(LookupResult::Interrupted) => Err(InterruptKind::Interrupt.into()),
                    Ok(LookupResult::Pending) => Err(GolemError::unknown(
                        "Unexpected pending result after invoke",
                    )),
                    Ok(LookupResult::New) => Err(GolemError::unknown(
                        "Unexpected missing result after invoke",
                    )),
                    Err(recv_error) => Err(GolemError::unknown(format!(
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
            .unwrap()
            .push_back(timestamped_update);
        self.oplog.add_and_commit(entry).await;
        self.update_metadata()
            .await
            .expect("update_metadata failed"); // TODO
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
                    .unwrap()
                    .push_back(QueuedWorkerInvocation::External(timestamped_invocation));
                self.oplog.add_and_commit(entry).await;
                self.update_metadata()
                    .await
                    .expect("update_metadata failed"); // TODO
            }
        }
    }

    pub fn pending_invocations(&self) -> Vec<TimestampedWorkerInvocation> {
        self.queue
            .read()
            .unwrap()
            .iter()
            .filter_map(|inv| inv.as_external().cloned())
            .collect()
    }

    pub fn pending_updates(&self) -> (VecDeque<TimestampedUpdateDescription>, DeletedRegions) {
        let pending_updates = self.pending_updates.read().unwrap().clone();
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

    pub fn pop_pending_update(&self) -> Option<TimestampedUpdateDescription> {
        self.pending_updates.write().unwrap().pop_front()
    }

    pub fn invocation_results(&self) -> HashMap<IdempotencyKey, OplogIndex> {
        HashMap::from_iter(
            self.invocation_results
                .read()
                .unwrap()
                .iter()
                .map(|(key, result)| (key.clone(), result.oplog_idx())),
        )
    }

    pub async fn store_invocation_success(
        &self,
        key: &IdempotencyKey,
        result: TypeAnnotatedValue,
        oplog_index: OplogIndex,
    ) {
        let mut map = self.invocation_results.write().unwrap();
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
        let pending = self.pending_invocations();
        let keys_to_fail = [
            vec![key],
            pending
                .iter()
                .filter_map(|entry| entry.invocation.idempotency_key())
                .collect(),
        ]
        .concat();
        let mut map = self.invocation_results.write().unwrap();
        for key in keys_to_fail {
            let stderr = self.event_service.get_last_invocation_errors();
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
        let mut map = self.invocation_results.write().unwrap();
        map.remove(key);
    }

    pub async fn update_status(&self, status_value: WorkerStatusRecord) {
        // Need to make sure the oplog is committed, because the updated status stores the current
        // last oplog index as reference.
        self.oplog().commit(CommitLevel::DurableOnly).await;
        // Storing the status in the key-value storage
        let component_type = self.execution_status.read().unwrap().component_type();
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
    pub async fn memory_requirement(&self) -> Result<u64, GolemError> {
        let metadata = self.get_metadata().await?;

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
    pub fn is_currently_idle_but_running(&self) -> bool {
        match self.instance.try_lock() {
            Ok(guard) => match &*guard {
                WorkerInstance::Running(running) => {
                    let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                    let has_invocations = !self.pending_invocations().is_empty();
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
    pub async fn last_execution_state_change(&self) -> Timestamp {
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
    ) {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                running
                    .enqueue(idempotency_key, full_function_name, function_input)
                    .await;
            }
            WorkerInstance::Unloaded | WorkerInstance::WaitingForPermit(_) => {
                debug!("Worker is initializing, persisting pending invocation");
                let invocation = WorkerInvocation::ExportedFunction {
                    idempotency_key,
                    full_function_name,
                    function_input,
                };
                let entry = OplogEntry::pending_worker_invocation(invocation.clone());
                let timestamped_invocation = TimestampedWorkerInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                };
                self.queue
                    .write()
                    .unwrap()
                    .push_back(QueuedWorkerInvocation::External(timestamped_invocation));
                self.oplog.add_and_commit(entry).await;
                self.update_metadata()
                    .await
                    .expect("update_metadata failed"); // TODO
            }
        }
    }

    pub async fn list_directory(
        &self,
        path: ComponentFilePath,
    ) -> Result<ListDirectoryResult, GolemError> {
        let (sender, receiver) = oneshot::channel();

        let mutex = self.instance.lock().await;

        self.queue
            .write()
            .unwrap()
            .push_back(QueuedWorkerInvocation::ListDirectory { path, sender });

        // Two cases here:
        // - Worker is running, we can send the invocation command and the worker will look at the queue immediately
        // - Worker is starting, it will process the request when it is started

        if let WorkerInstance::Running(running) = &*mutex {
            running.sender.send(WorkerCommand::Invocation).unwrap();
        };

        drop(mutex);

        receiver.await.unwrap()
    }

    pub async fn read_file(&self, path: ComponentFilePath) -> Result<ReadFileResult, GolemError> {
        let (sender, receiver) = oneshot::channel();

        let mutex = self.instance.lock().await;

        self.queue
            .write()
            .unwrap()
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
    ) -> Result<(), GolemError> {
        self.oplog
            .add_and_commit(OplogEntry::activate_plugin(plugin_installation_id))
            .await;
        self.update_metadata().await?;
        Ok(())
    }

    pub async fn deactivate_plugin(
        &self,
        plugin_installation_id: PluginInstallationId,
    ) -> Result<(), GolemError> {
        self.oplog
            .add_and_commit(OplogEntry::deactivate_plugin(plugin_installation_id))
            .await;
        self.update_metadata().await?;
        Ok(())
    }

    async fn wait_for_invocation_result(
        &self,
        key: &IdempotencyKey,
    ) -> Result<LookupResult, RecvError> {
        let mut subscription = self.events().subscribe();
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
        let maybe_result = self.invocation_results.read().unwrap().get(key).cloned();
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
                } => LookupResult::Complete(Err(GolemError::runtime(error.to_string(&stderr)))),
                InvocationResult::Cached {
                    result:
                        Err(FailedInvocationResult {
                            trap_type: TrapType::Exit,
                            ..
                        }),
                    ..
                } => LookupResult::Complete(Err(GolemError::runtime("Process exited"))),
                InvocationResult::Lazy { .. } => {
                    panic!("Unexpected lazy result after InvocationResult.cache")
                }
            }
        } else {
            let is_pending = self
                .pending_invocations()
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
        fail_pending_invocations: Option<GolemError>,
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
        fail_pending_invocations: Option<GolemError>,
    ) {
        if let WorkerInstance::Running(running) = instance.unload() {
            debug!("Stopping running worker ({called_from_invocation_loop})");
            let queued_items = running
                .queue
                .write()
                .unwrap()
                .drain(..)
                .collect::<VecDeque<_>>();

            if let Some(fail_pending_invocations) = fail_pending_invocations {
                // Publishing the provided initialization error to all pending invocations.
                // We cannot persist these failures, so they remain pending in the oplog, and
                // on next recovery they will be retried, but we still want waiting callers
                // to get the error.
                for item in queued_items {
                    match item {
                        QueuedWorkerInvocation::External(inner) => {
                            if let Some(idempotency_key) = inner.invocation.idempotency_key() {
                                self.events().publish(Event::InvocationCompleted {
                                    worker_id: self.owned_worker_id.worker_id(),
                                    idempotency_key: idempotency_key.clone(),
                                    result: Err(fail_pending_invocations.clone()),
                                })
                            }
                        }
                        QueuedWorkerInvocation::ListDirectory { sender, .. } => {
                            let _ = sender.send(Err(fail_pending_invocations.clone()));
                        }
                        QueuedWorkerInvocation::ReadFile { sender, .. } => {
                            let _ = sender.send(Err(fail_pending_invocations.clone()));
                        }
                    }
                }
            } else {
                *self.queue.write().unwrap() = queued_items;
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
    ) -> Result<bool, GolemError> {
        this.stop_internal(called_from_invocation_loop, None).await;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Self::start_if_needed_internal(this, oom_retry_count).await
    }

    async fn get_or_create_worker_metadata<
        T: HasWorkerService + HasComponentService + HasConfig + HasOplogService,
    >(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        component_version: Option<ComponentVersion>,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        parent: Option<WorkerId>,
    ) -> Result<(WorkerMetadata, Arc<RwLock<ExecutionStatus>>), GolemError> {
        let component_id = owned_worker_id.component_id();
        let component_metadata = this
            .component_service()
            .get_metadata(
                &owned_worker_id.account_id,
                &component_id,
                component_version,
            )
            .await?;
        match this.worker_service().get(owned_worker_id).await {
            None => {
                let initial_status =
                    calculate_last_known_status(this, owned_worker_id, &None).await?;
                let worker_metadata = WorkerMetadata {
                    worker_id: owned_worker_id.worker_id(),
                    args: worker_args.unwrap_or_default(),
                    env: worker_env.unwrap_or_default(),
                    account_id: owned_worker_id.account_id(),
                    created_at: Timestamp::now_utc(),
                    parent,
                    last_known_status: WorkerStatusRecord {
                        component_version: component_metadata.version,
                        component_size: component_metadata.size,
                        total_linear_memory_size: component_metadata
                            .memories
                            .iter()
                            .map(|m| m.initial)
                            .sum(),
                        extensions: WorkerStatusRecordExtensions::Extension1 {
                            active_plugins: component_metadata
                                .plugin_installations
                                .iter()
                                .map(|i| i.id.clone())
                                .collect(),
                        },
                        ..initial_status
                    },
                };
                let execution_status = this
                    .worker_service()
                    .add(&worker_metadata, component_metadata.component_type)
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
                let execution_status = Arc::new(RwLock::new(ExecutionStatus::Suspended {
                    last_known_status: worker_metadata.last_known_status.clone(),
                    component_type: component_metadata.component_type,
                    timestamp: Timestamp::now_utc(),
                }));
                Ok((worker_metadata, execution_status))
            }
        }
    }
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
    execution_status: Arc<RwLock<ExecutionStatus>>,

    oplog: Arc<dyn Oplog + Send + Sync>,

    permit: OwnedSemaphorePermit,
    waiting_for_command: Arc<AtomicBool>,
}

impl RunningWorker {
    pub fn new<Ctx: WorkerCtx>(
        owned_worker_id: OwnedWorkerId,
        queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        parent: Arc<Worker<Ctx>>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        permit: OwnedSemaphorePermit,
        oom_retry_count: u64,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        // Preload
        for _ in 0..queue.read().unwrap().len() {
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
        let handle = tokio::task::spawn(async move {
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
        });

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
    ) {
        let invocation = WorkerInvocation::ExportedFunction {
            idempotency_key,
            full_function_name,
            function_input,
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
            .unwrap()
            .push_back(QueuedWorkerInvocation::External(timestamped_invocation));
        self.sender.send(WorkerCommand::Invocation).unwrap()
    }

    fn interrupt(&self, kind: InterruptKind) {
        self.sender.send(WorkerCommand::Interrupt(kind)).unwrap();
    }

    async fn create_instance<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
    ) -> Result<(Instance, async_mutex::Mutex<Store<Ctx>>), GolemError> {
        let account_id = parent.owned_worker_id.account_id();
        let component_id = parent.owned_worker_id.component_id();
        let worker_metadata = parent.get_metadata().await?;

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
        let (component, component_metadata) = parent
            .component_service()
            .get(
                &parent.engine(),
                &account_id,
                &component_id,
                component_version,
            )
            .await?;

        let context = Ctx::create(
            OwnedWorkerId::new(&worker_metadata.account_id, &worker_metadata.worker_id),
            component_metadata.clone(),
            parent.promise_service(),
            parent.worker_service(),
            parent.worker_enumeration_service(),
            parent.key_value_service(),
            parent.blob_store_service(),
            parent.event_service.clone(),
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
                worker_metadata.last_known_status.component_version,
                worker_metadata.args.clone(),
                worker_metadata.env.clone(),
                worker_metadata.last_known_status.deleted_regions.clone(),
                worker_metadata.last_known_status.total_linear_memory_size,
            ),
            parent.execution_status.clone(),
            parent.file_loader(),
            parent.plugins(),
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
            GolemError::worker_creation_failed(
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
                GolemError::worker_creation_failed(
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
        mut receiver: UnboundedReceiver<WorkerCommand>,
        active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        owned_worker_id: OwnedWorkerId,
        parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
        waiting_for_command: Arc<AtomicBool>,
        oom_retry_count: u64,
    ) {
        loop {
            debug!("Invocation queue loop creating the instance");

            let (instance, store) = match Self::create_instance(parent.clone()).await {
                Ok((instance, store)) => {
                    parent.events().publish(Event::WorkerLoaded {
                        worker_id: owned_worker_id.worker_id(),
                        result: Ok(()),
                    });
                    (instance, store)
                }
                Err(err) => {
                    warn!("Failed to start the worker: {err}");
                    parent.events().publish(Event::WorkerLoaded {
                        worker_id: owned_worker_id.worker_id(),
                        result: Err(err.clone()),
                    });
                    parent.stop_internal(true, Some(err)).await;
                    break; // early return, we can't retry this
                }
            };

            debug!("Invocation queue loop preparing the instance");

            let mut final_decision = {
                let mut store = store.lock().await;

                store
                    .data_mut()
                    .set_suspended()
                    .await
                    .expect("Initial set_suspended should never fail");
                let span = span!(
                    Level::INFO,
                    "invocation",
                    worker_id = owned_worker_id.worker_id.to_string(),
                );
                let prepare_result =
                    Ctx::prepare_instance(&owned_worker_id.worker_id, &instance, &mut *store)
                        .instrument(span)
                        .await;

                match prepare_result {
                    Ok(decision) => {
                        debug!("Recovery decision from prepare_instance: {decision:?}");
                        decision
                    }
                    Err(err) => {
                        warn!("Failed to start the worker: {err}");
                        if let Err(err2) = store.data_mut().set_suspended().await {
                            warn!("Additional error during startup of the worker: {err2}");
                        }

                        parent.stop_internal(true, Some(err)).await;
                        break; // early return, we can't retry this
                    }
                }
            };

            if final_decision == RetryDecision::None {
                debug!("Invocation queue loop started");

                // Exits when RunningWorker is dropped
                waiting_for_command.store(true, Ordering::Release);
                while let Some(cmd) = receiver.recv().await {
                    waiting_for_command.store(false, Ordering::Release);
                    match cmd {
                        WorkerCommand::Invocation => {
                            let message = active
                                .write()
                                .unwrap()
                                .pop_front()
                                .expect("Message should be present");

                            let mut store_mutex = store.lock().await;
                            let store = store_mutex.deref_mut();

                            match message {
                                QueuedWorkerInvocation::ListDirectory { path, sender } => {
                                    let result = store.data_mut().list_directory(&path).await;
                                    let _ = sender.send(result);
                                }
                                QueuedWorkerInvocation::ReadFile { path, sender } => {
                                    let result = store.data_mut().read_file(&path).await;
                                    match result {
                                        Ok(ReadFileResult::Ok(stream)) => {
                                            // special case. We need to wait until the stream is consumed to avoid corruption
                                            //
                                            // This will delay processing of the next invocation and is quite unfortunate.
                                            // A possible improvement would be to check whether we are on a copy-on-write filesystem
                                            // if yes, we can make a cheap copy of the file here and serve the read from that copy.

                                            let (latch, latch_receiver) = oneshot::channel();
                                            let drop_stream =
                                                DropStream::new(stream, || latch.send(()).unwrap());
                                            let _ = sender.send(Ok(ReadFileResult::Ok(Box::pin(
                                                drop_stream,
                                            ))));
                                            latch_receiver.await.unwrap();
                                        }
                                        other => {
                                            let _ = sender.send(other);
                                        }
                                    };
                                }
                                QueuedWorkerInvocation::External(inner) => {
                                    match inner.invocation {
                                        WorkerInvocation::ExportedFunction {
                                            idempotency_key: invocation_key,
                                            full_function_name,
                                            function_input,
                                        } => {
                                            let span = span!(
                                                Level::INFO,
                                                "invocation",
                                                worker_id = owned_worker_id.worker_id.to_string(),
                                                idempotency_key = invocation_key.to_string(),
                                                function = full_function_name
                                            );
                                            let do_break = async {
                                                store
                                                    .data_mut()
                                                    .set_current_idempotency_key(invocation_key)
                                                    .await;

                                                if let Some(idempotency_key) =
                                                    &store.data().get_current_idempotency_key().await
                                                {
                                                    store
                                                        .data_mut()
                                                        .get_public_state()
                                                        .worker()
                                                        .store_invocation_resuming(idempotency_key)
                                                        .await;
                                                }

                                                // Make sure to update the pending invocation queue in the status record before
                                                // the invocation writes the invocation start oplog entry
                                                store.data_mut().update_pending_invocations().await;

                                                let result = invoke_worker(
                                                    full_function_name.clone(),
                                                    function_input.clone(),
                                                    store,
                                                    &instance,
                                                )
                                                    .await;

                                                match result {
                                                    Ok(InvokeResult::Succeeded {
                                                           output,
                                                           consumed_fuel,
                                                       }) => {
                                                        let component_metadata =
                                                            store.as_context().data().component_metadata();

                                                        let function_results = exports::function_by_name(
                                                            &component_metadata.exports,
                                                            &full_function_name,
                                                        );

                                                        match function_results {
                                                            Ok(Some(export_function)) => {
                                                                let function_results = export_function
                                                                    .results
                                                                    .into_iter()
                                                                    .collect();

                                                                let result = interpret_function_results(
                                                                    output,
                                                                    function_results,
                                                                )
                                                                    .map_err(|e| GolemError::ValueMismatch {
                                                                        details: e.join(", "),
                                                                    });

                                                                match result {
                                                                    Ok(result) => {
                                                                        store
                                                                            .data_mut()
                                                                            .on_invocation_success(
                                                                                &full_function_name,
                                                                                &function_input,
                                                                                consumed_fuel,
                                                                                result,
                                                                            )
                                                                            .await
                                                                            .unwrap(); // TODO: handle this error

                                                                        if store
                                                                            .data_mut()
                                                                            .component_metadata()
                                                                            .component_type
                                                                            == ComponentType::Ephemeral
                                                                        {
                                                                            final_decision =
                                                                                RetryDecision::None;
                                                                            true // stop after the invocation
                                                                        } else {
                                                                            false // continue processing the queue
                                                                        }
                                                                    }
                                                                    Err(error) => {
                                                                        let trap_type =
                                                                            TrapType::from_error::<Ctx>(
                                                                                &anyhow!(error),
                                                                            );

                                                                        store
                                                                            .data_mut()
                                                                            .on_invocation_failure(
                                                                                &trap_type,
                                                                            )
                                                                            .await;

                                                                        final_decision =
                                                                            RetryDecision::None;
                                                                        true // break
                                                                    }
                                                                }
                                                            }

                                                            Ok(None) => {
                                                                store
                                                                    .data_mut()
                                                                    .on_invocation_failure(
                                                                        &TrapType::Error(
                                                                            WorkerError::InvalidRequest(
                                                                                "Function not found"
                                                                                    .to_string(),
                                                                            ),
                                                                        ),
                                                                    )
                                                                    .await;

                                                                final_decision = RetryDecision::None;
                                                                true // break
                                                            }

                                                            Err(result) => {
                                                                store
                                                                    .data_mut()
                                                                    .on_invocation_failure(
                                                                        &TrapType::Error(
                                                                            WorkerError::Unknown(result),
                                                                        ),
                                                                    )
                                                                    .await;

                                                                final_decision = RetryDecision::None;
                                                                true // break
                                                            }
                                                        }
                                                    }
                                                    _ => {
                                                        let trap_type = match result {
                                                            Ok(invoke_result) => {
                                                                invoke_result.as_trap_type::<Ctx>()
                                                            }
                                                            Err(error) => {
                                                                Some(TrapType::from_error::<Ctx>(&anyhow!(
                                                                    error
                                                                )))
                                                            }
                                                        };
                                                        let decision = match trap_type {
                                                            Some(trap_type) => {
                                                                store
                                                                    .data_mut()
                                                                    .on_invocation_failure(&trap_type)
                                                                    .await
                                                            }
                                                            None => RetryDecision::None,
                                                        };

                                                        final_decision = decision;
                                                        true // break
                                                    }
                                                }
                                            }
                                                .instrument(span)
                                                .await;
                                            if do_break {
                                                break;
                                            }
                                        }
                                        WorkerInvocation::ManualUpdate { target_version } => {
                                            let span = span!(
                                                Level::INFO,
                                                "manual_update",
                                                worker_id = owned_worker_id.worker_id.to_string(),
                                                target_version = target_version.to_string()
                                            );
                                            let do_break = async {
                                                let _idempotency_key = {
                                                    let ctx = store.data_mut();
                                                    let idempotency_key = IdempotencyKey::fresh();
                                                    ctx.set_current_idempotency_key(idempotency_key.clone())
                                                        .await;
                                                    idempotency_key
                                                };

                                                if let Some(save_snapshot) = find_first_available_function(
                                                    store,
                                                    &instance,
                                                    vec![
                                                        "golem:api/save-snapshot@1.1.0.{save}".to_string(),
                                                        "golem:api/save-snapshot@0.2.0.{save}".to_string(),
                                                    ],
                                                ) {
                                                    store.data_mut().begin_call_snapshotting_function();

                                                    let result = invoke_worker(
                                                        save_snapshot,
                                                        vec![],
                                                        store,
                                                        &instance,
                                                    )
                                                        .await;
                                                    store.data_mut().end_call_snapshotting_function();

                                                    match result {
                                                        Ok(InvokeResult::Succeeded { output, .. }) =>
                                                            if let Some(bytes) = Self::decode_snapshot_result(output) {
                                                                match store
                                                                    .data_mut()
                                                                    .get_public_state()
                                                                    .oplog()
                                                                    .create_snapshot_based_update_description(
                                                                        target_version,
                                                                        &bytes,
                                                                    )
                                                                    .await
                                                                {
                                                                    Ok(update_description) => {
                                                                        // Enqueue the update
                                                                        parent.enqueue_update(update_description).await;

                                                                        // Make sure to update the pending updates queue
                                                                        store.data_mut().update_pending_updates().await;

                                                                        // Reactivate the worker
                                                                        final_decision = RetryDecision::Immediate;

                                                                        // Stop processing the queue to avoid race conditions
                                                                        true
                                                                    }
                                                                    Err(error) => {
                                                                        Self::fail_update(target_version, format!("failed to store the snapshot for manual update: {error}"), store).await;
                                                                        false
                                                                    }
                                                                }
                                                            } else {
                                                                Self::fail_update(target_version, "failed to get a snapshot for manual update: invalid snapshot result".to_string(), store).await;
                                                                false
                                                            },
                                                        Ok(InvokeResult::Failed { error, .. }) => {
                                                            let stderr = store.data().get_public_state().event_service().get_last_invocation_errors();
                                                            let error = error.to_string(&stderr);
                                                            Self::fail_update(
                                                                target_version,
                                                                format!("failed to get a snapshot for manual update: {error}"),
                                                                store,
                                                            ).await;
                                                            false
                                                        }
                                                        Ok(InvokeResult::Exited { .. }) => {
                                                            Self::fail_update(
                                                                target_version,
                                                                "failed to get a snapshot for manual update: it called exit".to_string(),
                                                                store,
                                                            ).await;
                                                            false
                                                        }
                                                        Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => {
                                                            Self::fail_update(
                                                                target_version,
                                                                format!("failed to get a snapshot for manual update: {interrupt_kind:?}"),
                                                                store,
                                                            ).await;
                                                            false
                                                        }
                                                        Err(error) => {
                                                            Self::fail_update(
                                                                target_version,
                                                                format!("failed to get a snapshot for manual update: {error:?}"),
                                                                store,
                                                            ).await;
                                                            false
                                                        }
                                                    }
                                                } else {
                                                    Self::fail_update(
                                                        target_version,
                                                        "failed to get a snapshot for manual update: save-snapshot is not exported".to_string(),
                                                        store,
                                                    ).await;
                                                    false
                                                }
                                            }.instrument(span).await;
                                            if do_break {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        WorkerCommand::Interrupt(kind) => {
                            match kind {
                                InterruptKind::Restart | InterruptKind::Jump => {
                                    final_decision = RetryDecision::Immediate;
                                }
                                _ => {
                                    final_decision = RetryDecision::None;
                                }
                            }
                            break;
                        }
                    }
                    waiting_for_command.store(true, Ordering::Release);
                }
                waiting_for_command.store(false, Ordering::Release);

                debug!("Invocation queue loop finished");
            }

            {
                if let Err(err) = store.lock().await.data_mut().set_suspended().await {
                    error!("Failed to set the worker to suspended state at the end of the invocation loop: {err}");
                }
            }

            // Make sure all pending commits are done
            store
                .lock()
                .await
                .data_mut()
                .get_public_state()
                .oplog()
                .commit(CommitLevel::Immediate)
                .await;

            match final_decision {
                RetryDecision::Immediate => {
                    debug!("Invocation queue loop triggering restart immediately");
                    continue;
                }
                RetryDecision::Delayed(delay) => {
                    debug!("Invocation queue loop sleeping for {delay:?} for delayed restart");
                    tokio::time::sleep(delay).await;
                    debug!("Invocation queue loop restarting after delay");
                    continue;
                }
                RetryDecision::None => {
                    debug!("Invocation queue loop notifying parent about being stopped");
                    parent.stop_internal(true, None).await;
                    break;
                }
                RetryDecision::ReacquirePermits => {
                    let delay = get_delay(parent.oom_retry_config(), oom_retry_count);
                    debug!("Invocation queue loop dropping memory permits and triggering restart with a delay of {delay:?}");
                    let _ = Worker::restart_on_oom(parent, true, delay, oom_retry_count + 1).await;
                    break;
                }
            }
        }
    }

    async fn fail_update<Ctx: WorkerCtx>(
        target_version: ComponentVersion,
        error: String,
        store: &mut Store<Ctx>,
    ) {
        store
            .data_mut()
            .on_worker_update_failed(target_version, Some(error))
            .await;
    }

    /// Attempts to interpret the save snapshot result as a byte vector
    fn decode_snapshot_result(values: Vec<Value>) -> Option<Vec<u8>> {
        if values.len() == 1 {
            if let Value::List(bytes) = &values[0] {
                let mut result = Vec::new();
                for value in bytes {
                    if let Value::U8(byte) = value {
                        result.push(*byte);
                    } else {
                        return None;
                    }
                }
                Some(result)
            } else {
                None
            }
        } else {
            None
        }
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
        result: Result<TypeAnnotatedValue, FailedInvocationResult>,
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
                    let values: TypeAnnotatedValue =
                        services.oplog().get_payload_of_entry(&entry).await.expect("failed to deserialize function response payload").unwrap();

                    Ok(values)
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
    Interrupt(InterruptKind),
}

pub async fn get_component_metadata<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
) -> Result<ComponentMetadata, GolemError> {
    let account_id = worker.owned_worker_id.account_id();
    let component_id = worker.owned_worker_id.component_id();
    let worker_metadata = worker.get_metadata().await?;

    let component_version = worker_metadata.last_known_status.component_version;

    let component_metadata = worker
        .component_service()
        .get_metadata(&account_id, &component_id, Some(component_version))
        .await?;

    Ok(component_metadata)
}

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker status.
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    metadata: &Option<WorkerMetadata>,
) -> Result<WorkerStatusRecord, GolemError>
where
    T: HasOplogService + HasConfig,
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

        let active_plugins = last_known.active_plugins().clone();

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

        let total_linear_memory_size =
            calculate_total_linear_memory_size(last_known.total_linear_memory_size, &new_entries);

        let owned_resources = calculate_owned_resources(last_known.owned_resources, &new_entries);

        let active_plugins = calculate_active_plugins(active_plugins, &new_entries);

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

fn calculate_total_linear_memory_size(
    total: u64,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> u64 {
    let mut result = total;
    for entry in entries.values() {
        if let OplogEntry::GrowMemory { delta, .. } = entry {
            result += *delta;
        }
    }
    result
}

fn calculate_owned_resources(
    initial: HashMap<WorkerResourceId, WorkerResourceDescription>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashMap<WorkerResourceId, WorkerResourceDescription> {
    let mut result = initial;
    for entry in entries.values() {
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
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<PluginInstallationId> {
    let mut result = initial;
    for entry in entries.values() {
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

fn is_running_worker_idle(running: &RunningWorker) -> bool {
    running.waiting_for_command.load(Ordering::Acquire) && running.queue.read().unwrap().is_empty()
}

#[derive(Debug)]
pub enum QueuedWorkerInvocation {
    /// 'Real' invocations that make sense from a domain model point of view and should be exposed to the user.
    /// All other cases here are used for concurrency control and should not be exposed to the user.
    External(TimestampedWorkerInvocation),
    ListDirectory {
        path: ComponentFilePath,
        sender: oneshot::Sender<Result<ListDirectoryResult, GolemError>>,
    },
    // The worker will suspend execution until the stream is dropped, so consume in a timely manner.
    ReadFile {
        path: ComponentFilePath,
        sender: oneshot::Sender<Result<ReadFileResult, GolemError>>,
    },
}

impl QueuedWorkerInvocation {
    fn as_external(&self) -> Option<&TimestampedWorkerInvocation> {
        match self {
            Self::External(invocation) => Some(invocation),
            _ => None,
        }
    }
}
