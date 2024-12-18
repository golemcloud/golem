use crate::additional_deps::AdditionalDeps;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::component::{ComponentOwner, DefaultComponentOwner};
use golem_common::model::oplog::WorkerResourceId;
use golem_common::model::plugin::DefaultPluginScope;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentVersion, IdempotencyKey, OwnedWorkerId,
    PluginInstallationId, WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::Uri;
use golem_wasm_rpc::Value;
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, ListDirectoryResult,
    ReadFileResult, TrapType, WorkerConfig,
};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::component::{ComponentMetadata, ComponentService};
use golem_worker_executor_base::services::file_loader::FileLoader;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::{Oplog, OplogService};
use golem_worker_executor_base::services::plugins::Plugins;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::rpc::Rpc;
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::{
    worker_enumeration, HasAll, HasConfig, HasOplogService,
};
use golem_worker_executor_base::worker::{RetryDecision, Worker};
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FileSystemReading, FuelManagement, IndexedResourceStore, InvocationHooks,
    InvocationManagement, StatusManagement, UpdateManagement, WorkerCtx,
};
use std::collections::HashSet;
use std::sync::{Arc, RwLock, Weak};
use wasmtime::component::{Instance, ResourceAny};
use wasmtime::{AsContextMut, ResourceLimiterAsync};

pub struct DebugContext {
    pub durable_ctx: DurableWorkerCtx<DebugContext>,
}

impl DurableWorkerCtxView<DebugContext> for DebugContext {
    fn durable_ctx(&self) -> &DurableWorkerCtx<DebugContext> {
        &self.durable_ctx
    }

    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<DebugContext> {
        &mut self.durable_ctx
    }
}

#[async_trait]
impl FuelManagement for DebugContext {
    fn is_out_of_fuel(&self, _current_level: i64) -> bool {
        false
    }

    async fn borrow_fuel(&mut self) -> Result<(), GolemError> {
        Ok(())
    }

    fn borrow_fuel_sync(&mut self) {}

    async fn return_fuel(&mut self, _current_level: i64) -> Result<i64, GolemError> {
        Ok(0)
    }
}

#[async_trait]
impl ExternalOperations<DebugContext> for DebugContext {
    type ExtraDeps = AdditionalDeps;

    async fn get_last_error_and_retry_count<T: HasAll<DebugContext> + Send + Sync>(
        this: &T,
        worker_id: &OwnedWorkerId,
    ) -> Option<LastError> {
        DurableWorkerCtx::<DebugContext>::get_last_error_and_retry_count(this, worker_id).await
    }

    async fn compute_latest_worker_status<T: HasOplogService + HasConfig + Send + Sync>(
        this: &T,
        worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        DurableWorkerCtx::<DebugContext>::compute_latest_worker_status(this, worker_id, metadata)
            .await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = DebugContext> + Send),
    ) -> Result<RetryDecision, GolemError> {
        DurableWorkerCtx::<DebugContext>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<T: HasAll<DebugContext> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<DebugContext>::record_last_known_limits(
            this,
            account_id,
            last_known_limits,
        )
        .await
    }

    async fn on_worker_deleted<T: HasAll<DebugContext> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<DebugContext>::on_worker_deleted(this, worker_id).await
    }

    async fn on_shard_assignment_changed<T: HasAll<DebugContext> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), anyhow::Error> {
        DurableWorkerCtx::<DebugContext>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl InvocationManagement for DebugContext {
    async fn set_current_idempotency_key(&mut self, idempotency_key: IdempotencyKey) {
        self.durable_ctx
            .set_current_idempotency_key(idempotency_key)
            .await
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.durable_ctx.get_current_idempotency_key().await
    }

    fn is_live(&self) -> bool {
        self.durable_ctx.is_live()
    }

    fn is_replay(&self) -> bool {
        self.durable_ctx.is_replay()
    }
}

#[async_trait]
impl StatusManagement for DebugContext {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        self.durable_ctx.check_interrupt()
    }

    async fn set_suspended(&self) -> Result<(), GolemError> {
        self.durable_ctx.set_suspended().await
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

    async fn update_pending_invocations(&self) {
        self.durable_ctx.update_pending_invocations().await
    }

    async fn update_pending_updates(&self) {
        self.durable_ctx.update_pending_updates().await
    }
}

#[async_trait]
impl InvocationHooks for DebugContext {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input)
            .await
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> RetryDecision {
        self.durable_ctx.on_invocation_failure(trap_type).await
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: TypeAnnotatedValue,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
    }
}

#[async_trait]
impl UpdateManagement for DebugContext {
    fn begin_call_snapshotting_function(&mut self) {
        self.durable_ctx.begin_call_snapshotting_function()
    }

    fn end_call_snapshotting_function(&mut self) {
        self.durable_ctx.end_call_snapshotting_function()
    }

    async fn on_worker_update_failed(
        &self,
        target_version: ComponentVersion,
        details: Option<String>,
    ) {
        self.durable_ctx
            .on_worker_update_failed(target_version, details)
            .await
    }

    async fn on_worker_update_succeeded(
        &self,
        target_version: ComponentVersion,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(target_version, new_component_size, new_active_plugins)
            .await
    }
}

#[async_trait]
impl IndexedResourceStore for DebugContext {
    fn get_indexed_resource(
        &self,
        resource_name: &str,
        resource_params: &[String],
    ) -> Option<WorkerResourceId> {
        self.durable_ctx
            .get_indexed_resource(resource_name, resource_params)
    }

    async fn store_indexed_resource(
        &mut self,
        resource_name: &str,
        resource_params: &[String],
        resource: WorkerResourceId,
    ) {
        self.durable_ctx
            .store_indexed_resource(resource_name, resource_params, resource)
            .await
    }

    fn drop_indexed_resource(&mut self, resource_name: &str, resource_params: &[String]) {
        self.durable_ctx
            .drop_indexed_resource(resource_name, resource_params)
    }
}

#[async_trait]
impl ResourceStore for DebugContext {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny) -> u64 {
        self.durable_ctx.add(resource).await
    }

    async fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.get(resource_id).await
    }

    async fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.borrow(resource_id).await
    }
}

#[async_trait]
impl FileSystemReading for DebugContext {
    async fn list_directory(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ListDirectoryResult, GolemError> {
        self.durable_ctx.list_directory(path).await
    }

    async fn read_file(&self, path: &ComponentFilePath) -> Result<ReadFileResult, GolemError> {
        self.durable_ctx.read_file(path).await
    }
}

#[async_trait]
impl ResourceLimiterAsync for DebugContext {
    async fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        let current_known = self.durable_ctx.total_linear_memory_size();
        let delta = (desired as u64).saturating_sub(current_known);
        if delta > 0 {
            Ok(self.durable_ctx.increase_memory(delta).await?)
        } else {
            Ok(true)
        }
    }

    async fn table_growing(
        &mut self,
        _current: u32,
        _desired: u32,
        _maximum: Option<u32>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[async_trait]
impl WorkerCtx for DebugContext {
    type PublicState = PublicDurableWorkerState<DebugContext>;
    type ComponentOwner = DefaultComponentOwner;
    type PluginScope = DefaultPluginScope;

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
        _active_workers: Arc<ActiveWorkers<DebugContext>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Weak<Worker<DebugContext>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        component_service: Arc<dyn ComponentService + Send + Sync>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<
            dyn Plugins<<Self::ComponentOwner as ComponentOwner>::PluginOwner, Self::PluginScope>
                + Send
                + Sync,
        >,
    ) -> Result<Self, GolemError> {
        let golem_ctx = DurableWorkerCtx::create(
            owned_worker_id,
            component_metadata,
            promise_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            event_service,
            oplog_service,
            oplog,
            invocation_queue,
            scheduler_service,
            rpc,
            worker_proxy,
            component_service,
            config,
            worker_config,
            execution_status,
            file_loader,
            plugins,
        )
        .await?;
        Ok(Self {
            durable_ctx: golem_ctx,
        })
    }

    fn get_public_state(&self) -> &Self::PublicState {
        &self.durable_ctx.public_state
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &WorkerId {
        self.durable_ctx.worker_id()
    }

    fn component_metadata(&self) -> &ComponentMetadata {
        self.durable_ctx.component_metadata()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<DebugContext>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.durable_ctx.worker_proxy()
    }
}
