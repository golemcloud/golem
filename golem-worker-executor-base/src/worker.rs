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
use std::ops::DerefMut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::anyhow;
use golem_wasm_rpc::Value;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, span, warn, Instrument, Level};
use wasmtime::{Store, UpdateDeadline};

use golem_common::config::RetryConfig;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription, WorkerError,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    CallingConvention, ComponentVersion, FailedUpdateRecord, IdempotencyKey, OwnedWorkerId,
    SuccessfulUpdateRecord, Timestamp, TimestampedWorkerInvocation, WorkerId, WorkerInvocation,
    WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};

use crate::error::GolemError;
use crate::invocation::{invoke_worker, InvokeResult};
use crate::model::{ExecutionStatus, InterruptKind, LookupResult, TrapType, WorkerConfig};
use crate::services::events::Event;
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveWorkers, HasAll, HasBlobStoreService, HasComponentService, HasConfig, HasEvents,
    HasExtraDeps, HasKeyValueService, HasOplog, HasOplogService, HasPromiseService, HasRpc,
    HasSchedulerService, HasWasmtimeEngine, HasWorker, HasWorkerEnumerationService, HasWorkerProxy,
    HasWorkerService, UsesAllDeps,
};
use crate::workerctx::WorkerCtx;

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

    queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    pending_updates: Arc<RwLock<VecDeque<TimestampedUpdateDescription>>>,
    invocation_results: Arc<RwLock<HashMap<IdempotencyKey, InvocationResult>>>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
    stopping: AtomicBool,

    running: Arc<Mutex<Option<RunningWorker>>>,
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
        let this_clone = deps.clone();
        let owned_worker_id_clone = owned_worker_id.clone();

        let worker_details = deps
            .active_workers()
            .get_with(&owned_worker_id.worker_id, || {
                Box::pin(async move {
                    Ok(Arc::new(
                        Self::new(
                            &this_clone,
                            owned_worker_id_clone,
                            worker_args,
                            worker_env,
                            component_version,
                            parent,
                        )
                        .in_current_span()
                        .await?,
                    ))
                })
            })
            .await?;
        Ok(worker_details)
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

    async fn new<T: HasAll<Ctx>>(
        deps: &T,
        owned_worker_id: OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Self, GolemError> {
        let worker_metadata = Self::get_or_create_worker_metadata(
            deps,
            &owned_worker_id,
            component_version,
            worker_args,
            worker_env,
            parent,
        )
        .await?;
        let oplog = deps.oplog_service().open(&owned_worker_id).await;

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
            initial_pending_invocations.iter().cloned(),
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
        let running = Arc::new(Mutex::new(None));

        let execution_status = Arc::new(RwLock::new(ExecutionStatus::Suspended {
            last_known_status: worker_metadata.last_known_status.clone(),
        }));

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
            running,
            execution_status,
            stopping,
            initial_worker_metadata: worker_metadata,
        })
    }

    pub async fn start_if_needed(this: Arc<Worker<Ctx>>) -> Result<(), GolemError> {
        let mut running = this.running.lock().await;
        if running.is_none() {
            let component_id = this.owned_worker_id.component_id();
            let worker_metadata = this.get_metadata().await?;

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
            let (component, component_metadata) = this
                .component_service()
                .get(&this.engine(), &component_id, component_version)
                .await?;

            let context = Ctx::create(
                OwnedWorkerId::new(&worker_metadata.account_id, &worker_metadata.worker_id),
                component_metadata,
                this.promise_service(),
                this.worker_service(),
                this.worker_enumeration_service(),
                this.key_value_service(),
                this.blob_store_service(),
                this.event_service.clone(),
                this.active_workers(),
                this.oplog_service(),
                this.oplog.clone(),
                Arc::downgrade(&this),
                this.scheduler_service(),
                this.rpc(),
                this.worker_proxy(),
                this.extra_deps(),
                this.config(),
                WorkerConfig::new(
                    worker_metadata.worker_id.clone(),
                    worker_metadata.last_known_status.component_version,
                    worker_metadata.args.clone(),
                    worker_metadata.env.clone(),
                    worker_metadata.last_known_status.deleted_regions.clone(),
                ),
                this.execution_status.clone(),
            )
            .await?;

            let mut store = Store::new(&this.engine(), context);
            store.set_epoch_deadline(this.config().limits.epoch_ticks);
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

            let instance_pre = this.linker().instantiate_pre(&component).map_err(|e| {
                GolemError::worker_creation_failed(
                    this.owned_worker_id.worker_id(),
                    format!(
                        "Failed to pre-instantiate worker {}: {e}",
                        this.owned_worker_id
                    ),
                )
            })?;

            let instance = instance_pre
                .instantiate_async(&mut store)
                .await
                .map_err(|e| {
                    GolemError::worker_creation_failed(
                        this.owned_worker_id.worker_id(),
                        format!("Failed to instantiate worker {}: {e}", this.owned_worker_id),
                    )
                })?;
            let store = async_mutex::Mutex::new(store);

            *running = Some(RunningWorker::new(
                this.owned_worker_id.clone(),
                this.queue.clone(),
                this.clone(),
                this.oplog(),
                this.execution_status.clone(),
                instance,
                store,
            ));
        } else {
            debug!("Worker is already running");
        }

        Ok(())
    }

    pub async fn stop(&self) {
        self.stop_internal(false).await;
    }

    pub async fn restart(this: Arc<Worker<Ctx>>) -> Result<(), GolemError> {
        Self::restart_internal(this, false).await
    }

    pub fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.event_service.clone()
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
        if let Some(running) = self.running.lock().await.as_ref() {
            running.interrupt(interrupt_kind.clone());
        }

        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { last_known_status } => {
                let (sender, receiver) = tokio::sync::broadcast::channel(1);
                *execution_status = ExecutionStatus::Interrupting {
                    interrupt_kind,
                    await_interruption: Arc::new(sender),
                    last_known_status,
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
        }
    }

    pub async fn invoke(
        &self,
        idempotency_key: IdempotencyKey,
        calling_convention: CallingConvention,
        full_function_name: String,
        function_input: Vec<Value>,
    ) -> Result<Option<Result<Vec<Value>, GolemError>>, GolemError> {
        let output = self.lookup_invocation_result(&idempotency_key).await;

        match output {
            LookupResult::Complete(output) => Ok(Some(output)),
            LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
            LookupResult::Pending => Ok(None),
            LookupResult::New => {
                // Invoke the function in the background
                self.enqueue(
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

    pub async fn invoke_and_await(
        &self,
        idempotency_key: IdempotencyKey,
        calling_convention: CallingConvention,
        full_function_name: String,
        function_input: Vec<Value>,
    ) -> Result<Vec<Value>, GolemError> {
        match self
            .invoke(
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

                let result = self.wait_for_invocation_result(&idempotency_key).await;

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
        match self.running.lock().await.as_ref() {
            Some(running) => {
                running.enqueue_manual_update(target_version).await;
            }
            None => {
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
                    .push_back(timestamped_invocation);
                self.oplog.add_and_commit(entry).await;
                self.update_metadata()
                    .await
                    .expect("update_metadata failed"); // TODO
            }
        }
    }

    pub fn pending_invocations(&self) -> Vec<TimestampedWorkerInvocation> {
        self.queue.read().unwrap().iter().cloned().collect()
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
        result: Vec<Value>,
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
            debug!("store_invocation_failure for {key}");

            map.insert(
                key.clone(),
                InvocationResult::Cached {
                    result: Err(trap_type.clone()),
                    oplog_idx: oplog_index,
                },
            );
            let golem_error = trap_type.as_golem_error();
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
        self.worker_service()
            .update_status(&self.owned_worker_id, &status_value)
            .await;
        self.execution_status
            .write()
            .unwrap()
            .set_last_known_status(status_value);
    }

    /// Enqueue invocation of an exported function
    async fn enqueue(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        calling_convention: CallingConvention,
    ) {
        match self.running.lock().await.as_ref() {
            Some(running) => {
                running
                    .enqueue(
                        idempotency_key,
                        full_function_name,
                        function_input,
                        calling_convention,
                    )
                    .await;
            }
            None => {
                debug!("Worker is initializing, persisting pending invocation");
                let invocation = WorkerInvocation::ExportedFunction {
                    idempotency_key,
                    full_function_name,
                    function_input,
                    calling_convention,
                };
                let entry = OplogEntry::pending_worker_invocation(invocation.clone());
                let timestamped_invocation = TimestampedWorkerInvocation {
                    timestamp: entry.timestamp(),
                    invocation,
                };
                self.queue
                    .write()
                    .unwrap()
                    .push_back(timestamped_invocation);
                self.oplog.add_and_commit(entry).await;
                self.update_metadata()
                    .await
                    .expect("update_metadata failed"); // TODO
            }
        }
    }

    async fn wait_for_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
        match self.lookup_invocation_result(key).await {
            LookupResult::Interrupted => LookupResult::Interrupted,
            LookupResult::New | LookupResult::Pending => {
                self.events()
                    .wait_for(|event| match event {
                        Event::InvocationCompleted {
                            worker_id,
                            idempotency_key,
                            result,
                        } if *worker_id == self.owned_worker_id.worker_id
                            && idempotency_key == key =>
                        {
                            debug!("wait_for_invocation_result: accepting event {:?}", event);
                            Some(LookupResult::Complete(result.clone()))
                        }
                        _ => {
                            debug!("wait_for_invocation_result: skipping event {:?}", event);
                            None
                        }
                    })
                    .await
            }
            LookupResult::Complete(result) => LookupResult::Complete(result),
        }
    }

    async fn lookup_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
        let maybe_result = self.invocation_results.read().unwrap().get(key).cloned();
        if let Some(mut result) = maybe_result {
            result.cache(self.oplog.clone()).await;
            match result {
                InvocationResult::Cached {
                    result: Ok(values), ..
                } => LookupResult::Complete(Ok(values)),
                InvocationResult::Cached {
                    result: Err(TrapType::Interrupt(InterruptKind::Interrupt)),
                    ..
                } => LookupResult::Interrupted,
                InvocationResult::Cached {
                    result: Err(TrapType::Interrupt(_)),
                    ..
                } => LookupResult::Pending,
                InvocationResult::Cached {
                    result: Err(TrapType::Error(error)),
                    ..
                } => LookupResult::Complete(Err(GolemError::runtime(error.to_string()))),
                InvocationResult::Cached {
                    result: Err(TrapType::Exit),
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

    async fn stop_internal(&self, called_from_invocation_loop: bool) {
        // we don't want to re-enter stop from within the invocation loop
        if self
            .stopping
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            == Ok(false)
        {
            let mut running = self.running.lock().await;
            if let Some(running) = running.take() {
                debug!("Stopping running worker ({called_from_invocation_loop})");
                let queued_items = running
                    .queue
                    .write()
                    .unwrap()
                    .drain(..)
                    .collect::<VecDeque<_>>();

                *self.queue.write().unwrap() = queued_items;

                if !called_from_invocation_loop {
                    // If stop was called from outside, we wait until the invocation queue stops
                    // (it happens by `running` getting dropped)
                    let run_loop_handle = running.stop();
                    run_loop_handle.await.expect("Worker run loop failed");
                }
            } else {
                debug!("Worker was already stopped");
            }
            self.stopping.store(false, Ordering::Release);
        }
    }
    async fn restart_internal(
        this: Arc<Worker<Ctx>>,
        called_from_invocation_loop: bool,
    ) -> Result<(), GolemError> {
        this.stop_internal(called_from_invocation_loop).await;
        Self::start_if_needed(this).await
    }

    async fn get_or_create_worker_metadata<
        T: HasWorkerService + HasComponentService + HasConfig + HasOplogService,
    >(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        component_version: Option<u64>,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        parent: Option<WorkerId>,
    ) -> Result<WorkerMetadata, GolemError> {
        let component_id = owned_worker_id.component_id();

        let component_metadata = this
            .component_service()
            .get_metadata(&component_id, component_version)
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
                        ..initial_status
                    },
                };
                this.worker_service().add(&worker_metadata).await?;
                Ok(worker_metadata)
            }
            Some(previous_metadata) => Ok(WorkerMetadata {
                last_known_status: calculate_last_known_status(
                    this,
                    owned_worker_id,
                    &Some(previous_metadata.clone()),
                )
                .await?,
                ..previous_metadata
            }),
        }
    }
}

struct RunningWorker {
    handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<WorkerCommand>,
    queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    execution_status: Arc<RwLock<ExecutionStatus>>,

    oplog: Arc<dyn Oplog + Send + Sync>,
}

impl RunningWorker {
    pub fn new<Ctx: WorkerCtx>(
        owned_worker_id: OwnedWorkerId,
        queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
        parent: Arc<Worker<Ctx>>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        instance: wasmtime::component::Instance,
        store: async_mutex::Mutex<Store<Ctx>>,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        // Preload
        for _ in 0..queue.read().unwrap().len() {
            sender.send(WorkerCommand::Invocation).unwrap();
        }

        let active_clone = queue.clone();
        let owned_worker_id_clone = owned_worker_id.clone();
        let handle = tokio::task::spawn(async move {
            RunningWorker::invocation_loop(
                receiver,
                active_clone,
                owned_worker_id_clone,
                parent,
                instance,
                store,
            )
            .in_current_span()
            .await;
        });

        RunningWorker {
            handle: Some(handle),
            sender,
            queue,
            oplog,
            execution_status,
        }
    }

    pub fn stop(mut self) -> JoinHandle<()> {
        self.handle.take().unwrap()
    }

    pub async fn enqueue(
        &self,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        calling_convention: CallingConvention,
    ) {
        let invocation = WorkerInvocation::ExportedFunction {
            idempotency_key,
            full_function_name,
            function_input,
            calling_convention,
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
            .push_back(timestamped_invocation);
        self.sender.send(WorkerCommand::Invocation).unwrap()
    }

    fn interrupt(&self, kind: InterruptKind) {
        self.sender.send(WorkerCommand::Interrupt(kind)).unwrap();
    }

    async fn invocation_loop<Ctx: WorkerCtx>(
        mut receiver: UnboundedReceiver<WorkerCommand>,
        active: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
        owned_worker_id: OwnedWorkerId,
        parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
        instance: wasmtime::component::Instance,
        store: async_mutex::Mutex<Store<Ctx>>,
    ) {
        debug!("Invocation queue loop preparing the instance");

        let mut final_decision = {
            let mut store = store.lock().await;
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
                Ok(decision) => decision,
                Err(err) => {
                    warn!("Failed to start the worker: {err}");
                    parent.stop_internal(true).await;
                    store.data_mut().set_suspended();
                    return; // early return, we can't retry this
                }
            }
        };

        if final_decision == RecoveryDecision::None {
            debug!("Invocation queue loop started");

            // Exits when RunningWorker is dropped
            while let Some(cmd) = receiver.recv().await {
                match cmd {
                    WorkerCommand::Invocation => {
                        let message = active
                            .write()
                            .unwrap()
                            .pop_front()
                            .expect("Message should be present");
                        debug!("Invocation queue processing {message:?}");

                        let mut store_mutex = store.lock().await;
                        let store = store_mutex.deref_mut();

                        match message.invocation {
                            WorkerInvocation::ExportedFunction {
                                idempotency_key: invocation_key,
                                full_function_name,
                                function_input,
                                calling_convention,
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
                                        calling_convention,
                                        true, // We are always in live mode at this point
                                    )
                                    .await;

                                    match result {
                                        Ok(InvokeResult::Succeeded {
                                            output,
                                            consumed_fuel,
                                        }) => {
                                            store
                                                .data_mut()
                                                .on_invocation_success(
                                                    &full_function_name,
                                                    &function_input,
                                                    consumed_fuel,
                                                    output,
                                                )
                                                .await
                                                .unwrap(); // TODO: handle this error
                                            false // do not break
                                        }
                                        _ => {
                                            let trap_type = match result {
                                                Ok(invoke_result) => {
                                                    invoke_result.as_trap_type::<Ctx>()
                                                }
                                                Err(error) => Some(TrapType::from_error::<Ctx>(
                                                    &anyhow!(error),
                                                )),
                                            };
                                            let decision = match trap_type {
                                                Some(trap_type) => {
                                                    store
                                                        .data_mut()
                                                        .on_invocation_failure(&trap_type)
                                                        .await
                                                }
                                                None => RecoveryDecision::None,
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
                                    store.data_mut().begin_call_snapshotting_function();
                                    let result = invoke_worker(
                                        "golem:api/save-snapshot@0.2.0.{save}".to_string(),
                                        vec![],
                                        store,
                                        &instance,
                                        CallingConvention::Component,
                                        true,
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
                                                        final_decision = RecoveryDecision::Immediate;

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
                                }.instrument(span).await;
                                if do_break {
                                    break;
                                }
                            }
                        }
                    }
                    WorkerCommand::Interrupt(kind) => {
                        match kind {
                            InterruptKind::Restart | InterruptKind::Jump => {
                                final_decision = RecoveryDecision::Immediate;
                            }
                            _ => {
                                final_decision = RecoveryDecision::None;
                            }
                        }
                        break;
                    }
                }
            }
            debug!("Invocation queue loop for finished");
        }

        {
            store.lock().await.data_mut().set_suspended();
        }

        match final_decision {
            RecoveryDecision::Immediate => {
                debug!("Invocation queue loop triggering restart immediately");
                let _ = Worker::restart_internal(parent, true).await; // TODO: what to do with error here?
            }
            RecoveryDecision::Delayed(delay) => {
                debug!("Invocation queue loop sleeping for {delay:?} for delayed restart");
                tokio::time::sleep(delay).await;
                debug!("Invocation queue loop triggering restart after delay");
                let _ = Worker::restart_internal(parent, true).await; // TODO: what to do with error here?
            }
            RecoveryDecision::None => {
                debug!("Invocation queue loop notifying parent about being stopped");
                parent.stop_internal(true).await;
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
enum InvocationResult {
    Cached {
        result: Result<Vec<Value>, TrapType>,
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

    pub async fn cache(&mut self, oplog: Arc<dyn Oplog + Send + Sync>) {
        if let Self::Lazy { oplog_idx } = self {
            let oplog_idx = *oplog_idx;
            let entry = oplog.read(oplog_idx).await;

            let result = match entry {
                OplogEntry::ExportedFunctionCompleted { .. } => {
                    let values: Vec<golem_wasm_rpc::protobuf::Val> = oplog.get_payload_of_entry(&entry).await.expect("failed to deserialize function response payload").unwrap();
                    let values = values
                        .into_iter()
                        .map(|val| {
                            val.try_into()
                                .expect("failed to decode serialized protobuf value")
                        })
                        .collect();
                    Ok(values)
                }
                OplogEntry::Error { error, .. } => Err(TrapType::Error(error)),
                OplogEntry::Interrupted { .. } => Err(TrapType::Interrupt(InterruptKind::Interrupt)),
                OplogEntry::Exited { .. } => Err(TrapType::Exit),
                _ => panic!("Unexpected oplog entry pointed by invocation result at index {oplog_idx} for {oplog:?}")
            } ;

            *self = Self::Cached { result, oplog_idx }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum RecoveryDecision {
    Immediate,
    Delayed(Duration),
    None,
}

#[derive(Clone, Debug)]
enum WorkerCommand {
    Invocation,
    Interrupt(InterruptKind),
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

        let total_linear_memory_size = last_known.total_linear_memory_size; // TODO: recalculate this once we record memory grow events in oplog

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
            total_linear_memory_size,
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
            OplogEntry::SuccessfulUpdate {
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
