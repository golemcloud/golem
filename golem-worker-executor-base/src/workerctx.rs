use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, VersionedWorkerId, WorkerId, WorkerMetadata,
    WorkerStatus,
};
use golem_common::proto::golem::Val;
use tokio::runtime::Handle;
use wasmtime::{AsContextMut, ResourceLimiterAsync};

use crate::error::GolemError;
use crate::model::{CurrentResourceLimits, ExecutionStatus, InterruptKind, WorkerConfig};
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::golem_config::GolemConfig;
use crate::services::invocation_key::InvocationKeyService;
use crate::services::key_value::KeyValueService;
use crate::services::promise::PromiseService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::{HasAll, HasExtraDeps};

#[async_trait]
pub trait WorkerCtx:
    FuelManagement
    + InvocationManagement
    + IoCapturing
    + StatusManagement
    + InvocationHooks
    + ExternalOperations<Self>
    + Send
    + Sync
    + Sized
    + 'static
{
    type PublicState: PublicWorkerIo + Clone + Send + Sync;

    async fn create(
        worker_id: VersionedWorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<Self>>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        runtime: Handle,
    ) -> Result<Self, GolemError>;

    fn get_public_state(&self) -> &Self::PublicState;

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync;

    fn worker_id(&self) -> &VersionedWorkerId;

    fn is_exit(error: &anyhow::Error) -> Option<i32>;
}

#[async_trait]
pub trait FuelManagement {
    fn is_out_of_fuel(&self, current_level: i64) -> bool;
    async fn borrow_fuel(&mut self) -> Result<(), GolemError>;
    fn borrow_fuel_sync(&mut self);
    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, GolemError>;
}

#[async_trait]
pub trait InvocationManagement {
    async fn set_current_invocation_key(&mut self, invocation_key: Option<InvocationKey>);
    async fn get_current_invocation_key(&self) -> Option<InvocationKey>;
    async fn interrupt_invocation_key(&mut self, key: &InvocationKey);
    async fn resume_invocation_key(&mut self, key: &InvocationKey);
    async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Val>, GolemError>,
    );
}

#[async_trait]
pub trait IoCapturing {
    async fn start_capturing_stdout(&mut self, provided_stdin: String);
    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error>;
}

#[async_trait]
pub trait StatusManagement {
    fn check_interrupt(&self) -> Option<InterruptKind>;
    fn set_suspended(&self);
    fn set_running(&self);

    async fn get_worker_status(&self) -> WorkerStatus;
    async fn store_worker_status(&self, status: WorkerStatus);

    async fn deactivate(&self);
}

#[async_trait]
pub trait InvocationHooks {
    #[allow(clippy::ptr_arg)]
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Val>,
        calling_convention: Option<&CallingConvention>,
    ) -> anyhow::Result<()>;
    async fn on_invocation_failure(&mut self, error: &anyhow::Error) -> Result<(), anyhow::Error>;
    async fn on_invocation_failure_deactivated(
        &mut self,
        error: &anyhow::Error,
    ) -> Result<WorkerStatus, anyhow::Error>;
    #[allow(clippy::ptr_arg)]
    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Val>,
        consumed_fuel: i64,
        output: Vec<Val>,
    ) -> Result<Option<Vec<Val>>, anyhow::Error>;
}

/// Operations not requiring an active worker context, but still depending on the
/// worker context implementation.
#[async_trait]
pub trait ExternalOperations<Ctx: WorkerCtx> {
    type ExtraDeps: Clone + Send + Sync + 'static;

    async fn set_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    );

    async fn get_worker_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> u32;

    async fn get_assumed_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> WorkerStatus;

    async fn prepare_instance(
        worker_id: &VersionedWorkerId,
        instance: &wasmtime::component::Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<(), GolemError>;

    async fn record_last_known_limits<T: HasExtraDeps<Ctx> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError>;

    async fn on_worker_deleted<T: HasExtraDeps<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError>;

    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
    ) -> Result<(), anyhow::Error>;
}

#[async_trait]
pub trait PublicWorkerIo {
    fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync>;
    async fn enqueue(&self, message: Bytes, invocation_key: InvocationKey);
}
