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

use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock, Weak};

use async_trait::async_trait;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::Value;
use wasmtime::{AsContextMut, ResourceLimiterAsync};

use golem_common::model::{
    AccountId, CallingConvention, ComponentVersion, IdempotencyKey, OwnedWorkerId, WorkerId,
    WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};

use crate::error::GolemError;
use crate::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, TrapType, WorkerConfig,
};
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentMetadata;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{Oplog, OplogService};
use crate::services::promise::PromiseService;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{worker_enumeration, HasAll, HasOplog, HasWorker};
use crate::worker::{RecoveryDecision, Worker};

/// WorkerCtx is the primary customization and extension point of worker executor. It is the context
/// associated with each running worker, and it is responsible for initializing the WASM linker as
/// well as providing hooks for the general worker executor logic.
#[async_trait]
pub trait WorkerCtx:
    FuelManagement
    + InvocationManagement
    + IoCapturing
    + StatusManagement
    + InvocationHooks
    + ExternalOperations<Self>
    + ResourceStore
    + IndexedResourceStore
    + UpdateManagement
    + Send
    + Sync
    + Sized
    + 'static
{
    /// PublicState is a subset of the worker context which is accessible outside the worker
    /// execution. This is useful to publish queues and similar objects to communicate with the
    /// executing worker from things like a request handler.
    type PublicState: PublicWorkerIo + HasWorker<Self> + HasOplog + Clone + Send + Sync;

    /// Creates a new worker context
    ///
    /// Arguments:
    /// - `owned_worker_id`: The worker ID (consists of the component id and worker name as well as the worker's owner account)
    /// - `component_metadata`: Metadata associated with the worker's component
    /// - `promise_service`: The service for managing promises
    /// - `worker_service`: The service for managing workers
    /// - `key_value_service`: The service for storing key-value pairs
    /// - `blob_store_service`: The service for storing arbitrary blobs
    /// - `event_service`: The service for publishing worker events
    /// - `active_workers`: The service for managing active workers
    /// - `oplog_service`: The service for reading and writing the oplog
    /// - `scheduler_service`: The scheduler implementation responsible for waking up suspended workers
    /// - `recovery_management`: The service for deciding if a worker should be recovered
    /// - `rpc`: The RPC implementation used for worker to worker communication
    /// - `worker_proyx`: Access to the worker proxy above the worker executor cluster
    /// - `extra_deps`: Extra dependencies that are required by this specific worker context
    /// - `config`: The shared worker configuration
    /// - `worker_config`: Configuration for this specific worker
    /// - `execution_status`: Lock created to store the execution status
    #[allow(clippy::too_many_arguments)]
    async fn create(
        owned_worker_id: OwnedWorkerId,
        component_metadata: ComponentMetadata,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<Self>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Weak<Worker<Self>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError>;

    /// Get the public part of the worker context
    fn get_public_state(&self) -> &Self::PublicState;

    /// Get a wasmtime resource limiter implementation for this worker context.
    ///
    /// The `ResourceLimiterAsync` trait can be used to limit the amount of WASM memory
    /// and table reservations.
    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync;

    /// Get the worker ID associated with this worker context
    fn worker_id(&self) -> &WorkerId;

    fn component_metadata(&self) -> &ComponentMetadata;

    /// The WASI exit API can use a special error to exit from the WASM execution. As this depends
    /// on the actual WASI implementation installed by the worker context, this function is used to
    ///determine if an error is an exit error and if so, what the exit code is.
    fn is_exit(error: &anyhow::Error) -> Option<i32>;

    /// Gets the worker-executor's WASM RPC implementation
    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync>;

    /// Gets an interface to the worker-proxy which can direct calls to other worker executors
    /// in the cluster
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync>;
}

/// The fuel management interface of a worker context is responsible for borrowing and returning
/// fuel required for executing a worker. The implementation can decide to ignore fuel management
/// and allow unconstrained execution of the worker, or it can communicate with some external store
/// to synchronize the available fuel with other workers.
///
/// Golem worker executors are not using wasmtime's fuel support directly to suspend workers when
/// reaching a zero amount - they initialize the fuel level to a large value and then periodically
/// call functions of this trait to check if the worker is out of fuel. If it is, it tries to borrow
/// more, and once the invocation is finished, returns the remaining amount. The implementation is
/// supposed to track the amount of borrowed fuel and compare that with the actual fuel levels
///passed to these functions.
#[async_trait]
pub trait FuelManagement {
    /// Check if the worker is out of fuel
    /// Arguments:
    /// - `current_level`: The current fuel level, it can be compared with a pre-calculated minimum level
    fn is_out_of_fuel(&self, current_level: i64) -> bool;

    /// Borrows some fuel for the execution. The amount borrowed is not used by the execution engine,
    /// but the worker context can store it and use it in `is_out_of_fuel` to check if the worker is
    /// within the limits.
    async fn borrow_fuel(&mut self) -> Result<(), GolemError>;

    /// Same as `borrow_fuel` but synchronous as it is called from the epoch_deadline_callback.
    /// This assumes that there is a cached available resource limits that can be used to calculate
    /// borrow fuel without reaching out to external services.
    fn borrow_fuel_sync(&mut self);

    /// Returns the remaining fuel that was previously borrowed. The remaining amount can be calculated
    /// by the current fuel level and some internal state of the worker context.
    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, GolemError>;
}

/// The invocation management interface of a worker context is responsible for connecting
/// an invocation key with a worker, and storing its result.
///
/// The invocation key is a unique identifier representing one invocation, generated separately
/// from the invocation itself. It guarantees that the invocation is executed only once, even if
/// the actual request is retried and reaches the worker executor twice.
///
/// A worker can be invoked multiple times during its lifetime, and each invocation has its own
/// invocation key, but only one invocation can be active at a time.
#[async_trait]
pub trait InvocationManagement {
    /// Sets the invocation key associated with the current invocation of the worker.
    async fn set_current_idempotency_key(&mut self, key: IdempotencyKey);

    /// Gets the invocation key associated with the current invocation of the worker.
    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey>;
}

/// The IoCapturing interface of a worker context is used by the Stdio calling convention to
/// associate a provided standard input string with the worker, and start capturing its emitted
/// standard output.
///
/// This feature enables passing data to and from workers even if they don't support WIT bindings.
#[async_trait]
pub trait IoCapturing {
    /// Starts capturing the standard output of the worker, and at the same time, provides some
    /// predefined standard input to it.
    async fn start_capturing_stdout(&mut self, provided_stdin: String);

    /// Finishes capturing the standard output of the worker and returns the captured string.
    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error>;
}

/// The status management interface of a worker context is responsible for querying and storing
/// the worker's status.
///
/// See `WorkerStatus` for the possible states of a worker.
#[async_trait]
pub trait StatusManagement {
    /// Checks if the worker is being interrupted, or has been interrupted. If not, the result
    /// is None. Otherwise, it is the kind of interrupt that happened.
    fn check_interrupt(&self) -> Option<InterruptKind>;

    /// Sets the worker status to suspended
    fn set_suspended(&self);

    /// Sets the worker status to running
    fn set_running(&self);

    /// Gets the current worker status
    async fn get_worker_status(&self) -> WorkerStatus;

    /// Stores the current worker status
    async fn store_worker_status(&self, status: WorkerStatus);

    /// Update the pending invocations of the worker
    async fn update_pending_invocations(&self);

    /// Update the pending updates of the worker
    async fn update_pending_updates(&self);
}

/// The invocation hooks interface of a worker context has some functions called around
/// worker invocation. These hooks can be used observe the beginning and the end (either
/// successful or failed) of invocations.
#[async_trait]
pub trait InvocationHooks {
    /// Called when a worker is about to be invoked
    /// Arguments:
    /// - `full_function_name`: The full name of the function being invoked (including the exported interface name if any)
    /// - `function_input`: The input of the function being invoked
    /// - `calling_convention`: The calling convention used to invoke the function
    #[allow(clippy::ptr_arg)]
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        calling_convention: Option<CallingConvention>,
    ) -> Result<(), GolemError>;

    /// Called when a worker invocation fails
    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> RecoveryDecision;

    /// Called when a worker invocation succeeds
    /// Arguments:
    /// - `full_function_name`: The full name of the function being invoked (including the exported interface name if any)
    /// - `function_input`: The input of the function being invoked
    /// - `consumed_fuel`: The amount of fuel consumed by the invocation
    /// - `output`: The output of the function being invoked
    #[allow(clippy::ptr_arg)]
    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<(), GolemError>;
}

#[async_trait]
pub trait UpdateManagement {
    /// Marks the beginning of a snapshot function call. This can be used to disabled persistence
    fn begin_call_snapshotting_function(&mut self);

    /// Marks the end of a snapshot function call. This can be used to re-enable persistence
    fn end_call_snapshotting_function(&mut self);

    /// Called when an update attempt has failed
    async fn on_worker_update_failed(
        &self,
        target_version: ComponentVersion,
        details: Option<String>,
    );

    /// Called when an update attempt succeeded
    async fn on_worker_update_succeeded(
        &self,
        target_version: ComponentVersion,
        new_component_size: u64,
    );
}

/// Stores resources created within the worker indexed by their constructor parameters
///
/// This is a secondary mapping on top of `ResourceStore`, which handles the mapping between
/// resource identifiers to actual wasmtime `ResourceAny` instances.
///
/// Note that the parameters are passed as unparsed WAVE strings instead of their parsed `Value`
/// representation - the string representation is easier to hash and allows us to reduce the number
/// of times we need to parse the parameters.
pub trait IndexedResourceStore {
    fn get_indexed_resource(&self, resource_name: &str, resource_params: &[String]) -> Option<u64>;
    fn store_indexed_resource(
        &mut self,
        resource_name: &str,
        resource_params: &[String],
        resource: u64,
    );
    fn drop_indexed_resource(&mut self, resource_name: &str, resource_params: &[String]);
}

/// Operations not requiring an active worker context, but still depending on the
/// worker context implementation.
#[async_trait]
pub trait ExternalOperations<Ctx: WorkerCtx> {
    /// Extra dependencies required by this specific worker context. A value of this type is
    /// passed to the created worker context in the 'extra_deps' parameter of 'WorkerCtx::create'.
    type ExtraDeps: Clone + Send + Sync + 'static;

    /// Gets how many times the worker has been retried to recover from an error, and what
    /// error was stored in the last entry.
    async fn get_last_error_and_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
    ) -> Option<LastError>;

    /// Gets a best-effort current worker status without activating the worker
    async fn compute_latest_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError>;

    /// Prepares a wasmtime instance after it has been created, but before it can be invoked.
    /// This can be used to restore the previous state of the worker but by general it can be no-op.
    ///
    /// If the result is true, the instance
    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &wasmtime::component::Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<RecoveryDecision, GolemError>;

    /// Records the last known resource limits of a worker without activating it
    async fn record_last_known_limits<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError>;

    /// Callback called when a worker is deleted
    async fn on_worker_deleted<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError>;

    /// Callback called when the executor's shard assignment has been changed
    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), anyhow::Error>;
}

/// A required interface to be implemented by the worker context's public state.
///
/// It is used to "connect" to a worker's event stream
#[async_trait]
pub trait PublicWorkerIo {
    /// Gets the event service created for the worker, which can be used to
    /// subscribe to worker events.
    fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync>;
}
