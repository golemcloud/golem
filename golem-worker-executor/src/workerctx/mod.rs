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

pub mod default;

use crate::model::{ExecutionStatus, LastError, ReadFileResult, TrapType, WorkerConfig};
use crate::services::active_workers::ActiveWorkers;
use crate::services::agent_types::AgentTypesService;
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentService;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{Oplog, OplogService};
use crate::services::promise::PromiseService;
use crate::services::rdbms::RdbmsService;
use crate::services::resource_limits::ResourceLimits;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::shard::ShardService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_fork::WorkerForkService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{worker_enumeration, HasAll, HasOplog, HasWorker};
use crate::worker::{RetryDecision, Worker};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentId, AgentMode};
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentRevision, PluginPriority,
};
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::TimestampedUpdateDescription;
use golem_common::model::{
    IdempotencyKey, OplogIndex, OwnedWorkerId, WorkerId, WorkerStatusRecord,
};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::GetFileSystemNodeResult;
use golem_wasm::wasmtime::ResourceStore;
use golem_wasm::{Value, ValueAndType};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Weak};
use uuid::Uuid;
use wasmtime::component::{Component, Instance, Linker};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::p2::WasiView;
use wasmtime_wasi_http::WasiHttpView;

/// WorkerCtx is the primary customization and extension point of worker executor. It is the context
/// associated with each running worker, and it is responsible for initializing the WASM linker as
/// well as providing hooks for the general worker executor logic.
#[async_trait]
pub trait WorkerCtx:
    FuelManagement
    + InvocationManagement
    + StatusManagement
    + InvocationHooks
    + ExternalOperations<Self>
    + ResourceStore
    + UpdateManagement
    + FileSystemReading
    + DynamicLinking<Self>
    + InvocationContextManagement
    + HasWasiConfigVars
    + Send
    + Sync
    + Sized
    + 'static
{
    /// PublicState is a subset of the worker context that is accessible outside the worker
    /// execution. This is useful to publish queues and similar objects to communicate with the
    /// executing worker from things like a request handler.
    type PublicState: PublicWorkerIo + HasWorker<Self> + HasOplog + Clone + Send + Sync;

    /// Static log event behaviour configuration for workers
    const LOG_EVENT_EMIT_BEHAVIOUR: LogEventEmitBehaviour;

    /// Creates a new worker context
    ///
    /// Arguments:
    /// - `owned_worker_id`: The worker ID (consists of the component id and worker name as well as the worker's owner account)
    /// - `component_metadata`: Metadata associated with the worker's component
    /// - `initial_component_metadata`: Metadata associated with the worker's component at the start of replay. Might be same or earlier than component_metadata
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
    /// - `worker_proxy`: Access to the worker proxy above the worker executor cluster
    /// - `extra_deps`: Extra dependencies that are required by this specific worker context
    /// - `config`: The shared worker configuration
    /// - `worker_config`: Configuration for this specific worker
    /// - `execution_status`: Lock created to store the execution status
    /// - `file_loader`: The service for loading files and making them available to workers
    #[allow(clippy::too_many_arguments)]
    async fn create(
        account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        agent_id: Option<AgentId>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        event_service: Arc<dyn WorkerEventService>,
        active_workers: Arc<ActiveWorkers<Self>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Self>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        worker_fork: Arc<dyn WorkerForkService>,
        resource_limits: Arc<dyn ResourceLimits>,
        agent_types_service: Arc<dyn AgentTypesService>,
        shard_service: Arc<dyn ShardService>,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<Uuid>,
    ) -> Result<Self, WorkerExecutorError>;

    fn as_wasi_view(&mut self) -> impl WasiView;
    fn as_wasi_http_view(&mut self) -> impl WasiHttpView;

    /// Get the public part of the worker context
    fn get_public_state(&self) -> &Self::PublicState;

    /// Get a wasmtime resource limiter implementation for this worker context.
    ///
    /// The `ResourceLimiterAsync` trait can be used to limit the amount of WASM memory
    /// and table reservations.
    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync;

    /// Get the worker ID associated with this worker context
    fn worker_id(&self) -> &WorkerId;

    /// Get the owned worker ID associated with this worker context
    fn owned_worker_id(&self) -> &OwnedWorkerId;

    /// Get the agent-id resolved from the worker name
    fn agent_id(&self) -> Option<AgentId>;

    fn agent_mode(&self) -> AgentMode;

    /// Gets the account created this worker
    fn created_by(&self) -> &AccountId;

    fn component_metadata(&self) -> &ComponentDto;

    /// The WASI exit API can use a special error to exit from the WASM execution. As this depends
    /// on the actual WASI implementation installed by the worker context, this function is used to
    ///determine if an error is an exit error and if so, what the exit code is.
    fn is_exit(error: &anyhow::Error) -> Option<i32>;

    /// Gets the worker-executor's WASM RPC implementation
    fn rpc(&self) -> Arc<dyn Rpc>;

    /// Gets an interface to the worker-proxy which can direct calls to other worker executors
    /// in the cluster
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy>;

    fn component_service(&self) -> Arc<dyn ComponentService>;

    fn worker_fork(&self) -> Arc<dyn WorkerForkService>;
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
    async fn borrow_fuel(&mut self, current_level: i64) -> Result<(), WorkerExecutorError>;

    /// Same as `borrow_fuel` but synchronous as it is called from the epoch_deadline_callback.
    /// This assumes that there is a cached available resource limits that can be used to calculate
    /// borrow fuel without reaching out to external services.
    fn borrow_fuel_sync(&mut self, current_level: i64);

    /// Returns the remaining fuel that was previously borrowed. The remaining amount can be calculated
    /// by the current fuel level and some internal state of the worker context.
    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, WorkerExecutorError>;
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

    /// Sets (overwrites) the current invocation context stack
    async fn set_current_invocation_context(
        &mut self,
        invocation_context: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError>;

    /// Gets the current invocation context stack
    async fn get_current_invocation_context(&self) -> InvocationContextStack;

    /// Returns whether we are in live mode where we are executing new calls.
    fn is_live(&self) -> bool;

    /// Returns whether we are in replay mode where we are replaying old calls.
    fn is_replay(&self) -> bool;
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
    #[allow(clippy::ptr_arg)]
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
    ) -> Result<(), WorkerExecutorError>;

    /// Called when a worker invocation fails
    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> RetryDecision;

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
        output: Option<ValueAndType>,
    ) -> Result<(), WorkerExecutorError>;

    /// Gets the retry point that should be associated with a current error. Errors are grouped
    /// by this information. The current oplog index is a good default.
    async fn get_current_retry_point(&self) -> OplogIndex;
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
        target_revision: ComponentRevision,
        details: Option<String>,
    );

    /// Called when an update attempt succeeded
    async fn on_worker_update_succeeded(
        &self,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginPriority>,
    );
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
        latest_worker_status: &WorkerStatusRecord,
    ) -> Option<LastError>;

    /// Resume the replay of a worker instance. Note that if the previous replay
    /// hasn't reached the end of the replay (which is usually last index in oplog)
    /// resume_replay will ensure to start replay from the last replayed index.
    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError>;

    /// Prepares a wasmtime instance after it has been created, but before it can be invoked.
    /// This can be used to restore the previous state of the worker, but by general it can be no-op.
    ///
    /// If the result is:
    /// - Err() - a fatal error happened during preparation that cannot be retried
    /// - Ok(None) - the preparation succeeded and the instance is ready to be used
    /// - Ok(Some(RetryDecision::Immediate)) - the preparation has been interrupted by an error but should be retried immediately
    /// - Ok(Some(RetryDecision::Delayed())) - the preparation has been interrupted by an error and should be retried after a delay
    /// - Ok(Some(RetryDecision::ReacquirePermits)) - the preparation has been interrupted by an error, but should be retried immediately after dropping and reacquiring te permits
    /// - Ok(Some(RetryDecision::None)) - the preparation has been interrupted and should not be retried, but it is not an error (example: suspend after resuming a previously interrupted invocation)
    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError>;

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

/// Trait used for reading worker filesystem. The worker will not be running any invocations when methods of this trait are called,
/// so not locking is needed in the implementation.
#[async_trait]
pub trait FileSystemReading {
    async fn get_file_system_node(
        &self,
        path: &ComponentFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError>;
    async fn read_file(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError>;
}

/// Functions to manipulate and query the current invocation context
#[async_trait]
pub trait InvocationContextManagement {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
        activate: bool,
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError>;

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError>;

    /// Removes an inherited span without finishing it
    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError>;

    /// Removes and finishes a local span
    async fn finish_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError>;

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), WorkerExecutorError>;

    /// Clones every element of the stack belonging to the given current span id, and sets
    /// the inherited flag to true on them, without changing the spans in this invocation context.
    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack;
}

#[async_trait]
pub trait DynamicLinking<Ctx: WorkerCtx> {
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Ctx>,
        component: &Component,
        component_metadata: &ComponentDto,
    ) -> anyhow::Result<()>;
}

pub trait HasWasiConfigVars {
    fn wasi_config_vars(&self) -> BTreeMap<String, String>;
}

pub enum LogEventEmitBehaviour {
    /// Always emit all log event
    Always,
    /// Emit log events only during live mode
    LiveOnly,
}
