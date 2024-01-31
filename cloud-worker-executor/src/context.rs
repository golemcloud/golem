use crate::services::resource_limits::ResourceLimits;
use crate::services::{AdditionalDeps, HasResourceLimits};
use anyhow::Error;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::Val;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, VersionedWorkerId, WorkerId, WorkerMetadata,
    WorkerStatus,
};
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::metrics::wasm::record_allocated_memory;
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, WorkerConfig,
};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::OplogService;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::recovery::RecoveryManagement;
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::HasAll;
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
    StatusManagement, WorkerCtx,
};
use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock};
use tracing::debug;
use wasmtime::component::Instance;
use wasmtime::{AsContextMut, ResourceLimiterAsync};

pub struct Context {
    pub durable_ctx: DurableWorkerCtx<Context>,
    config: Arc<GolemConfig>,
    account_id: AccountId,
    resource_limits: Arc<dyn ResourceLimits + Send + Sync>,
    last_fuel_level: i64,
    min_fuel_level: i64,
}

impl Context {
    pub fn new(
        golem_ctx: DurableWorkerCtx<Context>,
        config: Arc<GolemConfig>,
        account_id: AccountId,
        resource_limits: Arc<dyn ResourceLimits + Send + Sync>,
    ) -> Self {
        Self {
            durable_ctx: golem_ctx,
            config,
            account_id,
            resource_limits,
            last_fuel_level: i64::MAX,
            min_fuel_level: i64::MAX,
        }
    }

    pub async fn get_max_memory(&self) -> Result<usize, GolemError> {
        self.resource_limits.get_max_memory(&self.account_id).await
    }
}

impl DurableWorkerCtxView<Context> for Context {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Context> {
        &self.durable_ctx
    }

    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Context> {
        &mut self.durable_ctx
    }
}

#[async_trait]
impl FuelManagement for Context {
    fn is_out_of_fuel(&self, current_level: i64) -> bool {
        current_level < self.min_fuel_level
    }

    async fn borrow_fuel(&mut self) -> Result<(), GolemError> {
        let amount = self
            .resource_limits
            .borrow_fuel(&self.account_id, self.config.limits.fuel_to_borrow)
            .await?;
        self.min_fuel_level -= amount;
        debug!("borrowed fuel for {}: {}", self.account_id, amount);
        Ok(())
    }

    fn borrow_fuel_sync(&mut self) {
        let amount = self
            .resource_limits
            .borrow_fuel_sync(&self.account_id, self.config.limits.fuel_to_borrow);
        match amount {
            Some(amount) => {
                debug!("borrowed fuel for {}: {}", self.account_id, amount);
                self.min_fuel_level -= amount;
            }
            None => panic!("Illegal state: account's resource limits are not available when borrow_fuel_sync is called")
        }
    }

    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, GolemError> {
        let unused = current_level - self.min_fuel_level;
        if unused > 0 {
            debug!("current_level: {current_level}");
            debug!("min_fuel_level: {}", self.min_fuel_level);
            debug!("last_fuel_level: {}", self.last_fuel_level);
            debug!("returning unused fuel for {}: {}", self.account_id, unused);
            self.resource_limits
                .return_fuel(&self.account_id, unused)
                .await?
        }
        let consumed = self.last_fuel_level - current_level;
        self.last_fuel_level = current_level;
        debug!("reset fuel mark for {}: {}", self.account_id, current_level);
        Ok(consumed)
    }
}

#[async_trait]
impl InvocationManagement for Context {
    async fn set_current_invocation_key(&mut self, invocation_key: Option<InvocationKey>) {
        self.durable_ctx
            .set_current_invocation_key(invocation_key)
            .await
    }

    async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.durable_ctx.get_current_invocation_key().await
    }

    async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.durable_ctx.interrupt_invocation_key(key).await
    }

    async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.durable_ctx.resume_invocation_key(key).await
    }

    async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Val>, GolemError>,
    ) {
        self.durable_ctx.confirm_invocation_key(key, vals).await
    }
}

#[async_trait]
impl IoCapturing for Context {
    async fn start_capturing_stdout(&mut self, provided_stdin: String) {
        self.durable_ctx
            .start_capturing_stdout(provided_stdin)
            .await
    }

    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error> {
        self.durable_ctx.finish_capturing_stdout().await
    }
}

#[async_trait]
impl StatusManagement for Context {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        self.durable_ctx.check_interrupt()
    }

    fn set_suspended(&self) {
        self.durable_ctx.set_suspended()
    }

    fn set_running(&self) {
        self.durable_ctx.set_running()
    }

    async fn get_worker_status(&self) -> WorkerStatus {
        self.durable_ctx.get_worker_status().await
    }

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.durable_ctx.store_worker_status(status).await
    }

    async fn deactivate(&self) {
        self.durable_ctx.deactivate().await
    }
}

#[async_trait]
impl InvocationHooks for Context {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Val>,
        calling_convention: Option<&CallingConvention>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input, calling_convention)
            .await
    }

    async fn on_invocation_failure(&mut self, error: &Error) -> Result<(), Error> {
        self.durable_ctx.on_invocation_failure(error).await
    }

    async fn on_invocation_failure_deactivated(
        &mut self,
        error: &Error,
    ) -> Result<WorkerStatus, Error> {
        self.durable_ctx
            .on_invocation_failure_deactivated(error)
            .await
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Val>,
        consumed_fuel: i64,
        output: Vec<Val>,
    ) -> Result<Option<Vec<Val>>, Error> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
    }
}

#[async_trait]
impl ResourceLimiterAsync for Context {
    async fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        let limit = self.get_max_memory().await?;
        debug!(
            "memory_growing: current={}, desired={}, maximum={:?}, account limit={}",
            current, desired, maximum, limit
        );
        let allow = if desired > limit {
            false
        } else {
            !matches!(maximum, Some(max) if desired > max)
        };

        record_allocated_memory(desired);
        Ok(allow)
    }

    async fn table_growing(
        &mut self,
        current: u32,
        desired: u32,
        maximum: Option<u32>,
    ) -> anyhow::Result<bool> {
        debug!(
            "table_growing: current={}, desired={}, maximum={:?}",
            current, desired, maximum
        );
        Ok(true)
    }
}

#[async_trait]
impl ExternalOperations<Context> for Context {
    type ExtraDeps = AdditionalDeps;

    async fn set_worker_status<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    ) {
        DurableWorkerCtx::<Context>::set_worker_status(this, worker_id, status).await
    }

    async fn get_worker_retry_count<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> u32 {
        DurableWorkerCtx::<Context>::get_worker_retry_count(this, worker_id).await
    }

    async fn get_assumed_worker_status<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> WorkerStatus {
        DurableWorkerCtx::<Context>::get_assumed_worker_status(this, worker_id, metadata).await
    }

    async fn prepare_instance(
        worker_id: &VersionedWorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<Context>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<T: HasAll<Context> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        this.extra_deps()
            .resource_limits()
            .update_last_known_limits(account_id, last_known_limits)
            .await
    }

    async fn on_worker_deleted<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<Context>::on_worker_deleted(this, worker_id).await
    }

    async fn on_shard_assignment_changed<T: HasAll<Context> + Send + Sync>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<Context>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl WorkerCtx for Context {
    type PublicState = PublicDurableWorkerState;

    async fn create(
        worker_id: VersionedWorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<Context>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let golem_ctx = DurableWorkerCtx::create(
            worker_id,
            account_id.clone(),
            promise_service,
            invocation_key_service,
            worker_service,
            key_value_service,
            blob_store_service,
            event_service,
            active_workers,
            oplog_service,
            scheduler_service,
            recovery_management,
            config.clone(),
            worker_config,
            execution_status,
        )
        .await?;
        Ok(Self::new(
            golem_ctx,
            config,
            account_id,
            extra_deps.resource_limits(),
        ))
    }

    fn get_public_state(&self) -> &Self::PublicState {
        self.durable_ctx.get_public_state()
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &VersionedWorkerId {
        self.durable_ctx.worker_id()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<Context>::is_exit(error)
    }
}
