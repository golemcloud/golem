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

use anyhow::anyhow;
use golem_wasm_rpc::Value;
use std::collections::{HashMap, VecDeque};
use std::ops::DerefMut;
use std::sync::Weak;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, span, warn, Instrument, Level};
use wasmtime::{Store, UpdateDeadline};

use crate::error::GolemError;
use crate::invocation::{invoke_worker, InvokeResult};
use crate::model::{ExecutionStatus, InterruptKind, LookupResult, TrapType, WorkerConfig};
use crate::services::events::Event;
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveWorkers, HasAll, HasBlobStoreService, HasComponentService, HasConfig, HasEvents,
    HasExtraDeps, HasInvocationQueue, HasKeyValueService, HasOplog, HasOplogService,
    HasPromiseService, HasRpc, HasSchedulerService, HasWasmtimeEngine, HasWorkerEnumerationService,
    HasWorkerProxy, HasWorkerService, UsesAllDeps,
};
use crate::worker::calculate_last_known_status;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    CallingConvention, ComponentVersion, IdempotencyKey, OwnedWorkerId, Timestamp,
    TimestampedWorkerInvocation, WorkerInvocation, WorkerMetadata, WorkerStatusRecord,
};

/// Per-worker invocation queue service
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
pub struct InvocationQueue<Ctx: WorkerCtx> {
    owned_worker_id: OwnedWorkerId,

    oplog: Arc<dyn Oplog + Send + Sync>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>, // TODO: rename

    deps: All<Ctx>,

    queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    pending_updates: Arc<RwLock<VecDeque<TimestampedUpdateDescription>>>,
    invocation_results: Arc<RwLock<HashMap<IdempotencyKey, InvocationResult>>>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,

    running: Arc<Mutex<Option<RunningInvocationQueue>>>,
}

impl<Ctx: WorkerCtx> HasOplog for InvocationQueue<Ctx> {
    fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
        self.oplog.clone()
    }
}

impl<Ctx: WorkerCtx> UsesAllDeps for InvocationQueue<Ctx> {
    type Ctx = Ctx;

    fn all(&self) -> &All<Self::Ctx> {
        &self.deps
    }
}

impl<Ctx: WorkerCtx> Drop for InvocationQueue<Ctx> {
    fn drop(&mut self) {
        debug!(
            "Dropping InvocationQueue {}",
            self.owned_worker_id.worker_id.to_string()
        );
    }
}

impl<Ctx: WorkerCtx> InvocationQueue<Ctx> {
    pub async fn new<T: HasAll<Ctx>>(
        deps: &T,
        owned_worker_id: OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
    ) -> Result<Self, GolemError> {
        let worker_metadata = Self::get_or_create_worker_metadata(
            deps,
            &owned_worker_id,
            component_version,
            worker_args,
            worker_env,
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

        Ok(InvocationQueue {
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
            initial_worker_metadata: worker_metadata,
        })
    }

    pub async fn start_if_needed(this: Arc<InvocationQueue<Ctx>>) -> Result<(), GolemError> {
        let mut running = this.running.lock().await;
        if running.is_none() {
            debug!("Starting worker");
            // TODO: split/refactor

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
            let component = this
                .component_service()
                .get(&this.engine(), &component_id, component_version)
                .await?;

            let context = Ctx::create(
                OwnedWorkerId::new(&worker_metadata.account_id, &worker_metadata.worker_id),
                this.promise_service(),
                this.events(),
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

            *running = Some(RunningInvocationQueue::new(
                this.owned_worker_id.clone(),
                this.queue.clone(),
                Arc::downgrade(&this),
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
        let mut running = self.running.lock().await;
        if let Some(running) = running.take() {
            debug!("Stopping running worker");
            let queued_items = running
                .queue
                .write()
                .unwrap()
                .drain(..)
                .collect::<VecDeque<_>>();
            *self.queue.write().unwrap() = queued_items;

            // TODO: save last known status?
            // TODO: make sure the loop is stopped (probably it's already implemented?)
        } else {
            debug!("Worker was already stopped");
        }
    }

    pub async fn restart(this: Arc<InvocationQueue<Ctx>>) -> Result<(), GolemError> {
        // TODO
        this.stop().await;
        Self::start_if_needed(this).await
    }

    pub fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.event_service.clone()
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

    /// Enqueue invocation of an exported function
    pub async fn enqueue(
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
                debug!(
                    "Worker {} is initializing, persisting pending invocation",
                    self.owned_worker_id
                );
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
        let mut map = self.invocation_results.write().unwrap();
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

    pub async fn store_invocation_resuming(&self, key: &IdempotencyKey) {
        let mut map = self.invocation_results.write().unwrap();
        map.remove(key);
    }

    pub async fn wait_for_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
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
                            Some(LookupResult::Complete(result.clone()))
                        }
                        _ => None,
                    })
                    .await
            }
            LookupResult::Complete(result) => LookupResult::Complete(result),
        }
    }

    pub async fn lookup_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
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

    async fn get_or_create_worker_metadata<
        T: HasWorkerService + HasComponentService + HasConfig + HasOplogService,
    >(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        component_version: Option<u64>,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
    ) -> Result<WorkerMetadata, GolemError> {
        let component_id = owned_worker_id.component_id();

        let component_version = match component_version {
            Some(component_version) => component_version,
            None => {
                this.component_service()
                    .get_latest_version(&component_id)
                    .await?
            }
        };

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
                    last_known_status: WorkerStatusRecord {
                        component_version,
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

struct RunningInvocationQueue {
    owned_worker_id: OwnedWorkerId,

    _handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<()>,
    queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
    execution_status: Arc<RwLock<ExecutionStatus>>,

    oplog: Arc<dyn Oplog + Send + Sync>,
}

impl Drop for RunningInvocationQueue {
    fn drop(&mut self) {
        debug!(
            "Dropping RunningInvocationQueue {}",
            self.owned_worker_id.worker_id.to_string()
        );
    }
}

impl RunningInvocationQueue {
    pub fn new<Ctx: WorkerCtx>(
        owned_worker_id: OwnedWorkerId,
        queue: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
        parent: Weak<InvocationQueue<Ctx>>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        instance: wasmtime::component::Instance,
        store: async_mutex::Mutex<Store<Ctx>>,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        // Preload
        for _ in 0..queue.read().unwrap().len() {
            sender.send(()).unwrap();
        }

        let active_clone = queue.clone();
        let owned_worker_id_clone = owned_worker_id.clone();
        let handle = tokio::task::spawn(async move {
            RunningInvocationQueue::invocation_loop(
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

        RunningInvocationQueue {
            owned_worker_id,
            _handle: Some(handle),
            sender,
            queue,
            oplog,
            execution_status,
        }
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
        self.sender.send(()).unwrap()
    }

    async fn invocation_loop<Ctx: WorkerCtx>(
        mut receiver: UnboundedReceiver<()>,
        active: Arc<RwLock<VecDeque<TimestampedWorkerInvocation>>>,
        owned_worker_id: OwnedWorkerId,
        parent: Weak<InvocationQueue<Ctx>>,
        instance: wasmtime::component::Instance,
        store: async_mutex::Mutex<Store<Ctx>>,
    ) {
        debug!("Invocation queue loop preparing the instance");

        let mut final_decision = {
            let mut store = store.lock().await;
            let prepare_result =
                Ctx::prepare_instance(&owned_worker_id.worker_id, &instance, &mut *store).await;
            debug!("prepare_instance resulted in {prepare_result:?}");
            match prepare_result {
                Ok(decision) => decision,
                Err(err) => {
                    warn!("Failed to start the worker: {err}");
                    if let Some(parent) = parent.upgrade() {
                        parent.stop().await;
                    }
                    store.data_mut().set_suspended();
                    return; // early return, we can't retry this
                }
            }
        };

        if final_decision == RecoveryDecision::None {
            debug!("Invocation queue loop started");

            // Exits when RunningInvocationQueue is dropped
            while receiver.recv().await.is_some() {
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
                                    .invocation_queue()
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
                                        Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
                                        Err(error) => {
                                            Some(TrapType::from_error::<Ctx>(&anyhow!(error)))
                                        }
                                    };
                                    let decision = match trap_type {
                                        Some(trap_type) => {
                                            store.data_mut().on_invocation_failure(&trap_type).await
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
                                "golem:api/save-snapshot@0.2.0/save".to_string(),
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
                                    if let Some(parent) = parent.upgrade() {
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
                                        }
                                    } else {
                                        panic!("Parent invocation queue was unexpectedly dropped")
                                    }
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
            debug!("Invocation queue loop for finished");
        }

        {
            store.lock().await.data_mut().set_suspended();
        }

        match final_decision {
            RecoveryDecision::Immediate => {
                if let Some(parent) = parent.upgrade() {
                    debug!("Invocation queue loop triggering restart immediately");
                    let _ = InvocationQueue::restart(parent).await; // TODO: what to do with error here?
                }
            }
            RecoveryDecision::Delayed(delay) => {
                debug!("Invocation queue loop sleeping for {delay:?} for delayed restart");
                tokio::time::sleep(delay).await;
                if let Some(parent) = parent.upgrade() {
                    debug!("Invocation queue loop triggering restart after delay");
                    let _ = InvocationQueue::restart(parent).await; // TODO: what to do with error here?
                }
            }
            RecoveryDecision::None => {
                if let Some(parent) = parent.upgrade() {
                    debug!("Invocation queue loop notifying parent about being stopped");
                    parent.stop().await;
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
