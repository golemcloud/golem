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

use std::collections::{HashMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_mutex::Mutex;
use golem_common::cache::PendingOrFinal;
use golem_common::config::RetryConfig;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    AccountId, CallingConvention, FailedUpdateRecord, IdempotencyKey, SuccessfulUpdateRecord,
    Timestamp, TimestampedWorkerInvocation, WorkerId, WorkerInvocation, WorkerMetadata,
    WorkerStatus, WorkerStatusRecord,
};
use golem_wasm_rpc::Value;
use tokio::sync::broadcast::Receiver;
use tracing::{debug, error, info};
use wasmtime::{Store, UpdateDeadline};

use crate::error::GolemError;
use crate::metrics::wasm::{record_create_worker, record_create_worker_failure};
use crate::model::{ExecutionStatus, InterruptKind, LookupResult, TrapType, WorkerConfig};
use crate::services::events::Events;
use crate::services::golem_config::GolemConfig;
use crate::services::invocation_queue::InvocationQueue;
use crate::services::oplog::Oplog;
use crate::services::recovery::is_worker_error_retriable;
use crate::services::worker_activator::WorkerActivator;
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    HasAll, HasComponentService, HasConfig, HasInvocationQueue, HasOplogService, HasWorkerService,
};
use crate::workerctx::WorkerCtx;

/// Worker is one active wasmtime instance representing a Golem worker with its corresponding
/// worker context. The worker struct itself is responsible for creating/reactivating/interrupting
/// the worker, but the actual worker invocation is implemented in separate functions in the
/// 'invocation' module.
pub struct Worker<Ctx: WorkerCtx> {
    /// Metadata associated with the worker
    pub metadata: WorkerMetadata,

    /// The active wasmtime instance
    pub instance: wasmtime::component::Instance,

    /// The active wasmtime store holding the worker context
    pub store: Mutex<Store<Ctx>>,

    /// The public part of the worker context
    pub public_state: Ctx::PublicState,

    /// The current execution status of the worker
    pub execution_status: Arc<RwLock<ExecutionStatus>>,
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Creates a new worker.
    ///
    /// This involves downloading the component (WASM), creating the worker context and the wasmtime instance.
    ///
    /// Arguments:
    /// - `this` - the caller object having reference to all services
    /// - `worker_id` - the worker id (consisting of a component id and a worker name)
    /// - `worker_args` - the command line arguments to be associated with the worker
    /// - `worker_env` - the environment variables to be associated with the worker
    /// - `component_version` - the version of the component to be used (if None, the latest version is used)
    /// - `account_id` - the account id of the user who initiated the creation of the worker
    /// - `pending_worker` - the pending worker object which is already published during the worker initializes. This allows clients
    ///                      to connect to the worker's event stream during it initializes.
    pub async fn new<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        mut worker_metadata: WorkerMetadata,
        pending_worker: &PendingWorker<Ctx>,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx>,
    {
        let start = Instant::now();
        let result = {
            let component_id = worker_id.component_id.clone();

            loop {
                let component_version = worker_metadata
                    .last_known_status
                    .pending_updates
                    .front()
                    .map_or(
                        worker_metadata.last_known_status.component_version,
                        |update| {
                            let target_version = *update.description.target_version();
                            info!("Attempting {} update for {worker_id} from {} to version {target_version}",
                                match update.description {
                                    UpdateDescription::Automatic { .. } => "automatic",
                                    UpdateDescription::SnapshotBased { .. } => "snapshot based"
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

                let execution_status = Arc::new(RwLock::new(ExecutionStatus::Suspended {
                    last_known_status: worker_metadata.last_known_status.clone(),
                }));

                let context = Ctx::create(
                    worker_metadata.worker_id.clone(),
                    worker_metadata.account_id.clone(),
                    this.promise_service(),
                    this.events(),
                    this.worker_service(),
                    this.worker_enumeration_service(),
                    this.key_value_service(),
                    this.blob_store_service(),
                    pending_worker.event_service.clone(),
                    this.active_workers(),
                    this.oplog_service(),
                    pending_worker.oplog.clone(),
                    pending_worker.invocation_queue.clone(),
                    this.scheduler_service(),
                    this.recovery_management(),
                    this.rpc(),
                    this.worker_proxy(),
                    this.extra_deps(),
                    this.config(),
                    WorkerConfig::new(
                        worker_metadata.worker_id.clone(),
                        worker_metadata.last_known_status.component_version,
                        worker_args.clone(),
                        worker_env.clone(),
                        worker_metadata.last_known_status.deleted_regions.clone(),
                    ),
                    execution_status.clone(),
                )
                .await?;

                let public_state = context.get_public_state().clone();
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
                        worker_id.clone(),
                        format!("Failed to pre-instantiate worker {worker_id}: {e}"),
                    )
                })?;

                let instance = instance_pre
                    .instantiate_async(&mut store)
                    .await
                    .map_err(|e| {
                        GolemError::worker_creation_failed(
                            worker_id.clone(),
                            format!("Failed to instantiate worker {worker_id}: {e}"),
                        )
                    })?;

                let result = Arc::new(Worker {
                    metadata: worker_metadata.clone(),
                    instance,
                    store: Mutex::new(store),
                    public_state,
                    execution_status,
                });

                InvocationQueue::attach(result.public_state.invocation_queue(), result.clone())
                    .await;

                let need_restart = {
                    let mut store = result.store.lock().await;
                    Ctx::prepare_instance(&worker_metadata.worker_id, &result.instance, &mut *store)
                        .await?
                };

                if need_restart {
                    // Need to detach the invocation queue, because we have to be able
                    // to attach it to the next try's instance
                    result.public_state.invocation_queue().detach().await;

                    // Need to use the latest worker status
                    let updated_status = calculate_last_known_status(
                        this,
                        &worker_metadata.worker_id,
                        &Some(worker_metadata.clone()),
                    )
                    .await?;
                    worker_metadata.last_known_status = updated_status;

                    // Restart the whole loop
                    continue;
                }

                info!(
                    "Worker {}/{} activated",
                    worker_id.slug(),
                    worker_metadata.last_known_status.component_version
                );

                break Ok(result);
            }
        };

        match &result {
            Ok(_) => record_create_worker(start.elapsed()),
            Err(err) => record_create_worker_failure(err),
        }

        result
    }

    /// Makes sure that the worker is active, but without waiting for it to be idle.
    ///
    /// If the worker is already in memory this does nothing. Otherwise, the worker will be
    /// created (same as get_or_create_worker) but in a background task.
    ///
    /// If the active worker cache is not full, this newly created worker will be added to it.
    /// If it was full, the worker will be dropped but only after it finishes recovering which means
    /// a previously interrupted / suspended invocation might be resumed.
    pub async fn activate<T>(
        this: &T,
        worker_id: &WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        component_version: Option<u64>,
        account_id: AccountId,
    ) where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker_id_clone = worker_id.clone();
        let this_clone = this.clone();
        tokio::task::spawn(async move {
            let result = Worker::get_or_create_with_config(
                &this_clone,
                &worker_id_clone,
                worker_args,
                worker_env,
                component_version,
                account_id,
            )
            .await;
            if let Err(err) = result {
                error!("Failed to activate worker {worker_id_clone}: {err}");
            }
        });
    }

    pub async fn get_or_create<T>(
        this: &T,
        worker_id: &WorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        account_id: AccountId,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let (worker_args, worker_env) = match this.worker_service().get(worker_id).await {
            Some(metadata) => (metadata.args, metadata.env),
            None => (
                worker_args.unwrap_or_default(),
                worker_env.unwrap_or_default(),
            ),
        };

        Worker::get_or_create_with_config(
            this,
            worker_id,
            worker_args,
            worker_env,
            component_version,
            account_id,
        )
        .await
    }

    pub async fn get_or_create_with_config<T>(
        this: &T,
        worker_id: &WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        component_version: Option<u64>,
        account_id: AccountId,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone_1 = worker_id.clone();
        let worker_id_clone_2 = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();

        let worker_metadata = Self::get_or_create_worker_metadata(
            this,
            worker_id,
            component_version,
            worker_args.clone(),
            worker_env.clone(),
            account_id.clone(),
        )
        .await?;

        let oplog = this.oplog_service().open(&account_id, worker_id).await;
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

        let worker_details = this
            .active_workers()
            .get_with(
                worker_id.clone(),
                || {
                    PendingWorker::new(
                        worker_id_clone_1,
                        config_clone,
                        oplog,
                        this.worker_activator().clone(),
                        this.events().clone(),
                        &initial_pending_invocations,
                        &initial_pending_updates,
                        &initial_invocation_results,
                    )
                },
                |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        Worker::new(
                            &this_clone,
                            worker_id_clone_2,
                            worker_args_clone,
                            worker_env_clone,
                            worker_metadata,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await?;
        validate_worker(worker_details.metadata.clone(), worker_args, worker_env)?;
        Ok(worker_details)
    }

    /// Gets an already active worker or creates a new one and returns the pending worker object
    ///
    /// The pending worker object holds a reference to the event service, invocation queue and oplog
    /// of the worker that is getting created, allowing the caller to connect to the worker's event stream even before it is fully
    /// initialized.
    pub async fn get_or_create_pending<T>(
        this: &T,
        worker_id: &WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        component_version: Option<u64>,
        account_id: AccountId,
    ) -> Result<PendingOrFinal<PendingWorker<Ctx>, Arc<Self>>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone_1 = worker_id.clone();
        let worker_id_clone_2 = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();

        let worker_metadata = Self::get_or_create_worker_metadata(
            this,
            worker_id,
            component_version,
            worker_args.clone(),
            worker_env.clone(),
            account_id.clone(),
        )
        .await?;

        let oplog = this.oplog_service().open(&account_id, worker_id).await;
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

        this.active_workers()
            .get_pending_with(
                worker_id.clone(),
                || {
                    PendingWorker::new(
                        worker_id_clone_1,
                        config_clone,
                        oplog,
                        this.worker_activator().clone(),
                        this.events().clone(),
                        &initial_pending_invocations,
                        &initial_pending_updates,
                        &initial_invocation_results,
                    )
                },
                move |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        Worker::new(
                            &this_clone,
                            worker_id_clone_2,
                            worker_args_clone,
                            worker_env_clone,
                            worker_metadata,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await
    }

    /// Creates a new worker and returns the pending worker object, and pauses loading
    /// the worker until an explicit call to a oneshot resume channel.
    ///
    /// If the worker is already active, the function fails.
    ///
    /// The pending worker object holds a reference to the event service, invocation queue and oplog
    /// of the worker that is getting created, allowing the caller to connect to the worker's event stream even before it is fully
    /// initialized.
    pub async fn get_or_create_paused_pending<T>(
        this: &T,
        worker_id: &WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        component_version: Option<u64>,
        account_id: AccountId,
    ) -> Result<(PendingWorker<Ctx>, tokio::sync::oneshot::Sender<()>), GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone_1 = worker_id.clone();
        let worker_id_clone_2 = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();

        let mut worker_metadata = Self::get_or_create_worker_metadata(
            this,
            worker_id,
            component_version,
            worker_args.clone(),
            worker_env.clone(),
            account_id.clone(),
        )
        .await?;

        let oplog = this.oplog_service().open(&account_id, worker_id).await;
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

        let (resume_sender, resume_receiver) = tokio::sync::oneshot::channel();

        let pending_or_final = this
            .active_workers()
            .get_pending_with(
                worker_id.clone(),
                || {
                    PendingWorker::new(
                        worker_id_clone_1,
                        config_clone,
                        oplog,
                        this.worker_activator().clone(),
                        this.events().clone(),
                        &initial_pending_invocations,
                        &initial_pending_updates,
                        &initial_invocation_results,
                    )
                },
                move |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        resume_receiver.await.unwrap();

                        // Getting an up-to-date worker metadata before continuing with the worker creation
                        let worker_status = calculate_last_known_status(
                            &this_clone,
                            &worker_id_clone_2,
                            &Some(worker_metadata.clone()),
                        )
                        .await?;
                        worker_metadata.last_known_status = worker_status;

                        Worker::new(
                            &this_clone,
                            worker_id_clone_2,
                            worker_args_clone,
                            worker_env_clone,
                            worker_metadata,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await?;

        match pending_or_final {
            PendingOrFinal::Pending(pending) => Ok((pending, resume_sender)),
            PendingOrFinal::Final(_) => Err(GolemError::unknown(
                "Worker was unexpectedly already active",
            )),
        }
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
    pub fn set_interrupting(&self, interrupt_kind: InterruptKind) -> Option<Receiver<()>> {
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
            ExecutionStatus::Suspended { last_known_status } => {
                *execution_status = ExecutionStatus::Interrupted {
                    interrupt_kind,
                    last_known_status,
                };
                None
            }
            ExecutionStatus::Interrupting {
                await_interruption, ..
            } => {
                let receiver = await_interruption.subscribe();
                Some(receiver)
            }
            ExecutionStatus::Interrupted { .. } => None,
        }
    }

    pub fn get_metadata(&self) -> WorkerMetadata {
        let mut result = self.metadata.clone();
        result.last_known_status = self
            .execution_status
            .read()
            .unwrap()
            .last_known_status()
            .clone();
        result
    }

    async fn get_or_create_worker_metadata<
        T: HasWorkerService + HasComponentService + HasConfig + HasOplogService,
    >(
        this: &T,
        worker_id: &WorkerId,
        component_version: Option<u64>,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        account_id: AccountId,
    ) -> Result<WorkerMetadata, GolemError> {
        let component_id = worker_id.component_id.clone();

        let component_version = match component_version {
            Some(component_version) => component_version,
            None => {
                this.component_service()
                    .get_latest_version(&component_id)
                    .await?
            }
        };

        match this.worker_service().get(worker_id).await {
            None => {
                let initial_status = calculate_last_known_status(this, worker_id, &None).await?;
                let worker_metadata = WorkerMetadata {
                    worker_id: worker_id.clone(),
                    args: worker_args,
                    env: worker_env,
                    account_id,
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
                    worker_id,
                    &Some(previous_metadata.clone()),
                )
                .await?,
                ..previous_metadata
            }),
        }
    }
}

impl<Ctx: WorkerCtx> Drop for Worker<Ctx> {
    fn drop(&mut self) {
        info!("Deactivated worker {}", self.metadata.worker_id);
    }
}

impl<Ctx: WorkerCtx> Debug for Worker<Ctx> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerDetails({})", self.metadata.worker_id)
    }
}

/// Handle to a worker's invocation queue, oplog and event service during it is getting initialized
pub struct PendingWorker<Ctx: WorkerCtx> {
    pub event_service: Arc<dyn WorkerEventService + Send + Sync>,
    pub oplog: Arc<dyn Oplog + Send + Sync>,
    pub invocation_queue: Arc<InvocationQueue<Ctx>>,
    pub worker_id: WorkerId,
}

impl<Ctx: WorkerCtx> Clone for PendingWorker<Ctx> {
    fn clone(&self) -> Self {
        PendingWorker {
            event_service: self.event_service.clone(),
            oplog: self.oplog.clone(),
            invocation_queue: self.invocation_queue.clone(),
            worker_id: self.worker_id.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> PendingWorker<Ctx> {
    pub fn new(
        worker_id: WorkerId,
        config: Arc<GolemConfig>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        events: Arc<Events>,
        initial_pending_invocations: &[TimestampedWorkerInvocation],
        initial_pending_updates: &[TimestampedUpdateDescription],
        initial_invocation_results: &HashMap<IdempotencyKey, OplogIndex>,
    ) -> Result<PendingWorker<Ctx>, GolemError> {
        let invocation_queue = Arc::new(InvocationQueue::new(
            worker_id.clone(),
            oplog.clone(),
            worker_activator,
            events,
            initial_pending_invocations,
            initial_pending_updates,
            initial_invocation_results,
        ));

        Ok(PendingWorker {
            event_service: Arc::new(WorkerEventServiceDefault::new(
                config.limits.event_broadcast_capacity,
                config.limits.event_history_size,
            )),
            oplog,
            invocation_queue,
            worker_id,
        })
    }
}

fn validate_worker(
    worker_metadata: WorkerMetadata,
    worker_args: Vec<String>,
    worker_env: Vec<(String, String)>,
) -> Result<(), GolemError> {
    let mut errors: Vec<String> = Vec::new();
    if worker_metadata.args != worker_args {
        let error = format!(
            "Worker is already running with different args: {:?} != {:?}",
            worker_metadata.args, worker_args
        );
        errors.push(error)
    }
    if worker_metadata.env != worker_env {
        let error = format!(
            "Worker is already running with different env: {:?} != {:?}",
            worker_metadata.env, worker_env
        );
        errors.push(error)
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(GolemError::worker_creation_failed(
            worker_metadata.worker_id,
            errors.join("\n"),
        ))
    }
}

pub async fn invoke<Ctx: WorkerCtx>(
    worker: Arc<Worker<Ctx>>,
    idempotency_key: IdempotencyKey,
    calling_convention: CallingConvention,
    full_function_name: String,
    function_input: Vec<Value>,
) -> Result<Option<Result<Vec<Value>, GolemError>>, GolemError> {
    let output = worker
        .public_state
        .invocation_queue()
        .lookup_invocation_result(&idempotency_key)
        .await;

    match output {
        LookupResult::Complete(output) => Ok(Some(output)),
        LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
        LookupResult::Pending => Ok(None),
        LookupResult::New => {
            // Invoke the function in the background
            worker
                .public_state
                .invocation_queue()
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

pub async fn invoke_and_await<Ctx: WorkerCtx>(
    worker: Arc<Worker<Ctx>>,
    idempotency_key: IdempotencyKey,
    calling_convention: CallingConvention,
    full_function_name: String,
    function_input: Vec<Value>,
) -> Result<Vec<Value>, GolemError> {
    let worker_id = worker.metadata.worker_id.clone();
    match invoke(
        worker.clone(),
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
            debug!("Waiting for idempotency key {idempotency_key} to complete for {worker_id}",);

            let invocation_queue = worker.public_state.invocation_queue().clone();
            drop(worker); // we must not hold reference to the worker while waiting

            let result = invocation_queue
                .wait_for_invocation_result(&idempotency_key)
                .await;

            debug!(
                "Invocation key {} lookup result for {worker_id}: {:?}",
                idempotency_key, result
            );
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
    worker_id: &WorkerId,
    metadata: &Option<WorkerMetadata>,
) -> Result<WorkerStatusRecord, GolemError>
where
    T: HasOplogService + HasWorkerService + HasConfig,
{
    let last_known = metadata
        .as_ref()
        .map(|metadata| metadata.last_known_status.clone())
        .unwrap_or_default();

    let last_oplog_index = this.oplog_service().get_last_index(worker_id).await;
    if last_known.oplog_idx == last_oplog_index {
        Ok(last_known)
    } else {
        debug!("Calculating new worker status for {worker_id} from {} to {}", last_known.oplog_idx, last_oplog_index);
        let new_entries: Vec<OplogEntry> = this
            .oplog_service()
            .read(
                worker_id,
                last_known.oplog_idx + 1,
                last_oplog_index - last_known.oplog_idx,
            )
            .await
            .into_values()
            .collect();

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
                last_known.oplog_idx,
                &new_entries,
            );

        if let Some(TimestampedUpdateDescription {
            oplog_index,
            description: UpdateDescription::SnapshotBased { .. },
            ..
        }) = pending_updates.front()
        {
            deleted_regions.set_override(DeletedRegions::from_regions(vec![
                OplogRegion::from_range(1..=*oplog_index),
            ]));
        }

        let (invocation_results, current_idempotency_key) = calculate_invocation_results(
            last_known.invocation_results,
            last_known.current_idempotency_key,
            last_known.oplog_idx,
            &new_entries,
        );

        debug!("calculate_last_known_status using last oplog index {last_oplog_index} as reference");
        Ok(WorkerStatusRecord {
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
        })
    }
}

fn calculate_latest_worker_status(
    initial: &WorkerStatus,
    default_retry_policy: &RetryConfig,
    initial_retry_policy: Option<RetryConfig>,
    entries: &[OplogEntry],
) -> WorkerStatus {
    let mut result = initial.clone();
    let mut last_error_count = 0;
    let mut current_retry_policy = initial_retry_policy;
    for entry in entries {
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

fn calculate_deleted_regions(initial: DeletedRegions, entries: &[OplogEntry]) -> DeletedRegions {
    let mut builder = DeletedRegionsBuilder::from_regions(initial.into_regions());
    for entry in entries {
        if let OplogEntry::Jump { jump, .. } = entry {
            builder.add(jump.clone());
        }
    }
    builder.build()
}

pub fn calculate_worker_status(
    retry_config: &RetryConfig,
    trap_type: &TrapType,
    previous_tries: u64,
) -> WorkerStatus {
    match trap_type {
        TrapType::Interrupt(InterruptKind::Interrupt) => WorkerStatus::Interrupted,
        TrapType::Interrupt(InterruptKind::Suspend) => WorkerStatus::Suspended,
        TrapType::Interrupt(InterruptKind::Jump) => WorkerStatus::Running,
        TrapType::Interrupt(InterruptKind::Restart) => WorkerStatus::Running,
        TrapType::Exit => WorkerStatus::Exited,
        TrapType::Error(error) => {
            if is_worker_error_retriable(retry_config, error, previous_tries) {
                WorkerStatus::Retrying
            } else {
                WorkerStatus::Failed
            }
        }
    }
}

fn calculate_overridden_retry_policy(
    initial: Option<RetryConfig>,
    entries: &[OplogEntry],
) -> Option<RetryConfig> {
    let mut result = initial;
    for entry in entries {
        if let OplogEntry::ChangeRetryPolicy { new_policy, .. } = entry {
            result = Some(new_policy.clone());
        }
    }
    result
}

fn calculate_pending_invocations(
    initial: Vec<TimestampedWorkerInvocation>,
    entries: &[OplogEntry],
) -> Vec<TimestampedWorkerInvocation> {
    let mut result = initial;
    for entry in entries {
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
    start_index: OplogIndex,
    entries: &[OplogEntry],
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
    for (n, entry) in entries.iter().enumerate() {
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
                    oplog_index: start_index + (n as OplogIndex),
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
    start_index: OplogIndex,
    entries: &[OplogEntry],
) -> (HashMap<IdempotencyKey, OplogIndex>, Option<IdempotencyKey>) {
    let mut invocation_results = invocation_results;
    let mut current_idempotency_key = current_idempotency_key;

    for (n, entry) in entries.iter().enumerate() {
        let oplog_idx = start_index + (n as OplogIndex);
        match entry {
            OplogEntry::ExportedFunctionInvoked {
                idempotency_key, ..
            } => {
                current_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::ExportedFunctionCompleted { .. } => {
                current_idempotency_key = None;
            }
            OplogEntry::Error { .. } => {
                if let Some(idempotency_key) = &current_idempotency_key {
                    invocation_results.insert(idempotency_key.clone(), oplog_idx);
                }
            }
            OplogEntry::Exited { .. } => {
                if let Some(idempotency_key) = &current_idempotency_key {
                    invocation_results.insert(idempotency_key.clone(), oplog_idx);
                }
            }
            _ => {}
        }
    }

    (invocation_results, current_idempotency_key)
}
