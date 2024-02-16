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

use std::fmt::{Debug, Formatter};
use std::ops::DerefMut;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_mutex::Mutex;
use bytes::Bytes;
use golem_common::cache::PendingOrFinal;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, VersionedWorkerId, WorkerId, WorkerMetadata,
    WorkerStatusRecord,
};
use golem_wasm_rpc::Value;
use tokio::sync::broadcast::Receiver;
use tracing::{debug, error, info};
use wasmtime::{Store, UpdateDeadline};

use crate::error::GolemError;
use crate::invocation::invoke_worker;
use crate::metrics::wasm::{record_create_worker, record_create_worker_failure};
use crate::model::{ExecutionStatus, InterruptKind, WorkerConfig};
use crate::services::golem_config::GolemConfig;
use crate::services::invocation_key::LookupResult;
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{HasAll, HasInvocationKeyService};
use crate::workerctx::{PublicWorkerIo, WorkerCtx};

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
    /// This involves downloading the template (WASM), creating the worker context and the wasmtime instance.
    ///
    /// Arguments:
    /// - `this` - the caller object having reference to all services
    /// - `worker_id` - the worker id (consisting of a template id and a worker name)
    /// - `worker_args` - the command line arguments to be associated with the worker
    /// - `worker_env` - the environment variables to be associated with the worker
    /// - `template_version` - the version of the template to be used (if None, the latest version is used)
    /// - `account_id` - the account id of the user who initiated the creation of the worker
    /// - `pending_worker` - the pending worker object which is already published during the worker initializes. This allows clients
    ///                      to connect to the worker's event stream during it initializes.
    pub async fn new<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
        pending_worker: &PendingWorker,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx>,
    {
        let start = Instant::now();
        let result = {
            let template_id = worker_id.template_id.clone();

            let (template_version, component) = match template_version {
                Some(component_version) => (
                    component_version,
                    this.template_service()
                        .get(&this.engine(), &template_id, component_version)
                        .await?,
                ),
                None => {
                    this.template_service()
                        .get_latest(&this.engine(), &template_id)
                        .await?
                }
            };

            let versioned_worker_id = VersionedWorkerId {
                worker_id: worker_id.clone(),
                template_version,
            };

            let worker_metadata = WorkerMetadata {
                worker_id: versioned_worker_id.clone(),
                args: worker_args.clone(),
                env: worker_env.clone(),
                account_id,
                last_known_status: WorkerStatusRecord::default(),
            };

            this.worker_service().add(&worker_metadata).await?;

            let execution_status = Arc::new(RwLock::new(ExecutionStatus::Suspended));

            let context = Ctx::create(
                worker_metadata.worker_id.clone(),
                worker_metadata.account_id.clone(),
                this.promise_service(),
                this.invocation_key_service(),
                this.worker_service(),
                this.key_value_service(),
                this.blob_store_service(),
                pending_worker.event_service.clone(),
                this.active_workers(),
                this.oplog_service(),
                this.scheduler_service(),
                this.recovery_management(),
                this.rpc(),
                this.extra_deps(),
                this.config(),
                WorkerConfig::new(worker_metadata.worker_id.clone(), worker_args, worker_env),
                execution_status.clone(),
            )
            .await?;

            let public_state = context.get_public_state().clone();

            let mut store = Store::new(&this.engine(), context);
            store.set_epoch_deadline(this.config().limits.epoch_ticks);
            store.epoch_deadline_callback(|mut store| {
                let current_level = store.get_fuel().unwrap_or(0);
                if store.data().is_out_of_fuel(current_level as i64) {
                    debug!("ran out of fuel, borrowing more");
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
                    format!("Failed to pre-instantiate component: {e}"),
                )
            })?;

            let instance = instance_pre
                .instantiate_async(&mut store)
                .await
                .map_err(|e| {
                    GolemError::worker_creation_failed(
                        worker_id.clone(),
                        format!("Failed to instantiate component: {e}"),
                    )
                })?;

            Ctx::prepare_instance(&versioned_worker_id, &instance, &mut store).await?;

            let result = Arc::new(Worker {
                metadata: worker_metadata.clone(),
                instance,
                store: Mutex::new(store),
                public_state,
                execution_status,
            });

            info!("Worker {}/{} activated", worker_id.slug(), template_version);

            Ok(result)
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
        template_version: Option<i32>,
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
                template_version,
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
        template_version: Option<i32>,
        account_id: AccountId,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let (worker_args, worker_env) = match this.worker_service().get(&worker_id).await {
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
            template_version,
            account_id,
        )
        .await
    }

    pub async fn get_or_create_with_config<T>(
        this: &T,
        worker_id: &WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();
        let worker_details = this
            .active_workers()
            .get_with(
                worker_id.clone(),
                || PendingWorker::new(config_clone),
                |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        Worker::new(
                            &this_clone,
                            worker_id_clone,
                            worker_args_clone,
                            worker_env_clone,
                            template_version,
                            account_id,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await?;
        validate_worker(
            worker_details.metadata.clone(),
            worker_args,
            worker_env,
            template_version,
        )?;
        Ok(worker_details)
    }

    /// Gets an already active worker or creates a new one and returns the pending worker object
    ///
    /// The pending worker object holds a reference to the event service of the worker that is getting
    /// created, allowing the caller to connect to the worker's event stream even before it is fully
    /// initialized.
    pub async fn get_or_create_pending<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
    ) -> Result<PendingOrFinal<PendingWorker, Arc<Self>>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();
        this.active_workers()
            .get_pending_with(
                worker_id.clone(),
                || PendingWorker::new(config_clone),
                move |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        Worker::new(
                            &this_clone,
                            worker_id_clone,
                            worker_args_clone,
                            worker_env_clone,
                            template_version,
                            account_id,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await
    }

    /// Looks up a given invocation key's current status.
    /// As the invocation key status is only stored in memory, we need to have an active
    /// instance (instance_details) to call this function.
    pub fn lookup_result<T>(&self, this: &T, invocation_key: &InvocationKey) -> LookupResult
    where
        T: HasInvocationKeyService,
    {
        this.invocation_key_service()
            .lookup_key(&self.metadata.worker_id.worker_id, invocation_key)
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
            ExecutionStatus::Running => {
                let (sender, receiver) = tokio::sync::broadcast::channel(1);
                *execution_status = ExecutionStatus::Interrupting {
                    interrupt_kind,
                    await_interruption: Arc::new(sender),
                };
                Some(receiver)
            }
            ExecutionStatus::Suspended => {
                *execution_status = ExecutionStatus::Interrupted { interrupt_kind };
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

/// Handle to a worker's event service during it is getting initialized
#[derive(Clone)]
pub struct PendingWorker {
    pub event_service: Arc<dyn WorkerEventService + Send + Sync>,
}

impl PendingWorker {
    pub fn new(config: Arc<GolemConfig>) -> Result<PendingWorker, GolemError> {
        Ok(PendingWorker {
            event_service: Arc::new(WorkerEventServiceDefault::new(
                config.limits.event_broadcast_capacity,
                config.limits.event_history_size,
            )),
        })
    }
}

fn validate_worker(
    worker_metadata: WorkerMetadata,
    worker_args: Vec<String>,
    worker_env: Vec<(String, String)>,
    template_version: Option<i32>,
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
    if let Some(version) = template_version {
        if worker_metadata.worker_id.template_version != version {
            let error = format!(
                "Worker is already running with different template version: {:?} != {:?}",
                worker_metadata.worker_id.template_version, version
            );
            errors.push(error)
        }
    };
    if errors.is_empty() {
        Ok(())
    } else {
        Err(GolemError::worker_creation_failed(
            worker_metadata.worker_id.worker_id,
            errors.join("\n"),
        ))
    }
}

pub async fn invoke<Ctx: WorkerCtx, T>(
    worker: Arc<Worker<Ctx>>,
    this: &T,
    invocation_key: Option<InvocationKey>,
    calling_convention: CallingConvention,
    full_function_name: String,
    function_input: Vec<Value>,
) -> Result<Option<Result<Vec<Value>, GolemError>>, GolemError>
where
    T: HasInvocationKeyService,
{
    let output = match &invocation_key {
        Some(invocation_key) => worker.lookup_result(this, invocation_key),
        None => LookupResult::Pending,
    };

    match output {
        LookupResult::Complete(output) => Ok(Some(output)),
        LookupResult::Invalid => Err(GolemError::invalid_request(format!(
            "Invalid invocation key {} for {}",
            invocation_key.unwrap(),
            worker.metadata.worker_id.worker_id
        ))),
        LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
        LookupResult::Pending => {
            if calling_convention == CallingConvention::StdioEventloop {
                // We only have to invoke the function if it is not running yet
                let requires_invoke = {
                    let public_state = &worker.public_state;

                    let bytes = match function_input.first() {
                        Some(Value::String(value)) => {
                            Ok(Bytes::from(format!("{}\n", value).to_string()))
                        }
                        _ => Err(GolemError::invalid_request(
                            "unexpected function input for stdio-eventloop calling convention",
                        )),
                    }?;

                    public_state
                        .enqueue(
                            bytes,
                            invocation_key
                                .clone()
                                .expect("stdio-eventloop mode requires an invocation key"),
                        )
                        .await;
                    let execution_status = worker.execution_status.read().unwrap().clone();
                    !execution_status.is_running()
                };

                if requires_invoke {
                    // Invoke the function in the background
                    let worker_clone = worker.clone();
                    tokio::spawn(async move {
                        let instance = &worker_clone.instance;
                        let store = &worker_clone.store;
                        let mut store_mutex = store.lock().await;
                        let store = store_mutex.deref_mut();

                        store
                            .data_mut()
                            .set_current_invocation_key(invocation_key)
                            .await;
                        let _ = invoke_worker(
                            full_function_name,
                            vec![],
                            store,
                            instance,
                            &CallingConvention::Component,
                            true,
                        )
                        .await;
                    });
                }
                Ok(None)
            } else {
                // Invoke the function in the background
                let worker_clone = worker.clone();
                tokio::spawn(async move {
                    let instance = &worker_clone.instance;
                    let store = &worker_clone.store;
                    let mut store_mutex = store.lock().await;
                    let store = store_mutex.deref_mut();

                    store
                        .data_mut()
                        .set_current_invocation_key(invocation_key)
                        .await;
                    let _ = invoke_worker(
                        full_function_name,
                        function_input,
                        store,
                        instance,
                        &calling_convention,
                        true,
                    )
                    .await;
                });
                Ok(None)
            }
        }
    }
}

pub async fn invoke_and_await<Ctx: WorkerCtx, T>(
    worker: Arc<Worker<Ctx>>,
    this: &T,
    invocation_key: Option<InvocationKey>,
    calling_convention: CallingConvention,
    full_function_name: String,
    function_input: Vec<Value>,
) -> Result<Vec<Value>, GolemError>
where
    T: HasInvocationKeyService,
{
    match invoke(
        worker.clone(),
        this,
        invocation_key.clone(),
        calling_convention,
        full_function_name,
        function_input,
    )
    .await?
    {
        Some(Ok(output)) => Ok(output),
        Some(Err(err)) => Err(err),
        None => {
            let worker_id = &worker.metadata.worker_id.worker_id;
            let invocation_key =
                invocation_key.expect("missing invocation key for invoke-and-await");

            debug!("Waiting for invocation key {} to complete", invocation_key);
            let result = this
                .invocation_key_service()
                .wait_for_confirmation(&worker_id, &invocation_key)
                .await;

            debug!(
                "Invocation key {} lookup result: {:?}",
                invocation_key, result
            );
            match result {
                LookupResult::Invalid => Err(GolemError::invalid_request(format!(
                    "Invalid invocation key {invocation_key} for {worker_id}"
                ))),
                LookupResult::Complete(Ok(output)) => Ok(output),
                LookupResult::Complete(Err(err)) => Err(err),
                LookupResult::Interrupted => Err(InterruptKind::Interrupt.into()),
                LookupResult::Pending => {
                    Err(GolemError::unknown("Unexpected pending invocation key"))
                }
            }
        }
    }
}
