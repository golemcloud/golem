use crate::services::resource_limits::ResourceLimits;
use crate::services::{AdditionalDeps, HasResourceLimits};
use crate::CloudGolemTypes;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::invocation_context::{self, InvocationContextStack};
use golem_common::model::oplog::WorkerResourceId;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentVersion, IdempotencyKey, OwnedWorkerId,
    PluginInstallationId, TargetWorkerId, WorkerId, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord,
};
use golem_wasm_rpc::golem_rpc_0_1_x::types::{
    Datetime, FutureInvokeResult, HostFutureInvokeResult, Pollable, WasmRpc,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{CancellationTokenEntry, HostWasmRpc, RpcError, Uri, Value, WitValue};
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::metrics::wasm::record_allocated_memory;
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
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, IndexedResourceStore,
    InvocationContextManagement, InvocationHooks, InvocationManagement, StatusManagement,
    UpdateManagement, WorkerCtx,
};
use std::collections::HashSet;
use std::sync::{Arc, RwLock, Weak};
use tracing::debug;
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::WasiView;
use wasmtime_wasi_http::WasiHttpView;

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
    async fn set_current_idempotency_key(&mut self, key: IdempotencyKey) {
        self.durable_ctx.set_current_idempotency_key(key).await
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.durable_ctx.get_current_idempotency_key().await
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.durable_ctx.get_current_invocation_context().await
    }

    async fn set_current_invocation_context(
        &mut self,
        stack: InvocationContextStack,
    ) -> Result<(), GolemError> {
        self.durable_ctx.set_current_invocation_context(stack).await
    }

    fn is_live(&self) -> bool {
        self.durable_ctx.is_live()
    }

    fn is_replay(&self) -> bool {
        self.durable_ctx.is_replay()
    }
}

#[async_trait]
impl StatusManagement for Context {
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
impl InvocationHooks for Context {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input)
            .await
    }

    async fn on_invocation_failure(&mut self, error: &TrapType) -> RetryDecision {
        self.durable_ctx.on_invocation_failure(error).await
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
        current: usize,
        desired: usize,
        maximum: Option<usize>,
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

    async fn get_last_error_and_retry_count<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &OwnedWorkerId,
        worker_status_record: &WorkerStatusRecord,
    ) -> Option<LastError> {
        DurableWorkerCtx::<Context>::get_last_error_and_retry_count(
            this,
            worker_id,
            worker_status_record,
        )
        .await
    }

    async fn compute_latest_worker_status<T: HasOplogService + HasConfig + Send + Sync>(
        this: &T,
        worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        DurableWorkerCtx::<Context>::compute_latest_worker_status(this, worker_id, metadata).await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Context> + Send),
        instance: &Instance,
    ) -> Result<RetryDecision, GolemError> {
        DurableWorkerCtx::<Context>::resume_replay(store, instance).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<RetryDecision, GolemError> {
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

    async fn on_shard_assignment_changed<T: HasAll<Context> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<Context>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl ResourceStore for Context {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny) -> u64 {
        self.durable_ctx.add(resource).await
    }

    async fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        ResourceStore::get(&mut self.durable_ctx, resource_id).await
    }

    async fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.borrow(resource_id).await
    }
}

#[async_trait]
impl UpdateManagement for Context {
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
impl IndexedResourceStore for Context {
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
impl FileSystemReading for Context {
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
impl HostWasmRpc for Context {
    async fn new(&mut self, location: Uri) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.new(location).await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<WitValue, RpcError>> {
        self.durable_ctx
            .invoke_and_await(self_, function_name, function_params)
            .await
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<(), RpcError>> {
        self.durable_ctx
            .invoke(self_, function_name, function_params)
            .await
    }

    async fn async_invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        self.durable_ctx
            .async_invoke_and_await(self_, function_name, function_params)
            .await
    }

    async fn schedule_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        scheduled_time: Datetime,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .schedule_invocation(self_, scheduled_time, function_name, function_params)
            .await
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        scheduled_time: Datetime,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<CancellationTokenEntry>> {
        self.durable_ctx
            .schedule_cancelable_invocation(self_, scheduled_time, function_name, function_params)
            .await
    }

    async fn drop(&mut self, rep: Resource<WasmRpc>) -> anyhow::Result<()> {
        HostWasmRpc::drop(&mut self.durable_ctx, rep).await
    }
}

#[async_trait]
impl HostFutureInvokeResult for Context {
    async fn subscribe(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        HostFutureInvokeResult::subscribe(&mut self.durable_ctx, self_).await
    }

    async fn get(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Option<Result<WitValue, RpcError>>> {
        HostFutureInvokeResult::get(&mut self.durable_ctx, self_).await
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        HostFutureInvokeResult::drop(&mut self.durable_ctx, rep).await
    }
}

#[async_trait]
impl DynamicLinking<Context> for Context {
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Context>,
        component: &Component,
        component_metadata: &ComponentMetadata<CloudGolemTypes>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .link(engine, linker, component, component_metadata)
    }
}

impl InvocationContextManagement for Context {
    fn start_span(
        &mut self,
        initial_attributes: &[(String, invocation_context::AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, GolemError> {
        self.durable_ctx.start_span(initial_attributes)
    }

    fn start_child_span(
        &mut self,
        parent: &invocation_context::SpanId,
        initial_attributes: &[(String, invocation_context::AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, GolemError> {
        self.durable_ctx
            .start_child_span(parent, initial_attributes)
    }

    fn finish_span(&mut self, span_id: &invocation_context::SpanId) -> Result<(), GolemError> {
        self.durable_ctx.finish_span(span_id)
    }

    fn remove_span(&mut self, span_id: &invocation_context::SpanId) -> Result<(), GolemError> {
        self.durable_ctx.remove_span(span_id)
    }
}

#[async_trait]
impl WorkerCtx for Context {
    type PublicState = PublicDurableWorkerState<Context>;

    type Types = CloudGolemTypes;

    async fn create(
        owned_worker_id: OwnedWorkerId,
        component_metadata: ComponentMetadata<CloudGolemTypes>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        _active_workers: Arc<ActiveWorkers<Self>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Weak<Worker<Self>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        component_service: Arc<dyn ComponentService<CloudGolemTypes>>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<CloudGolemTypes>>,
    ) -> Result<Self, GolemError> {
        let golem_ctx = DurableWorkerCtx::create(
            owned_worker_id.clone(),
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
            config.clone(),
            worker_config.clone(),
            execution_status,
            file_loader,
            plugins,
        )
        .await?;
        Ok(Self::new(
            golem_ctx,
            config,
            owned_worker_id.account_id,
            extra_deps.resource_limits(),
        ))
    }

    fn as_wasi_view(&mut self) -> impl WasiView {
        self.durable_ctx.as_wasi_view()
    }

    fn as_wasi_http_view(&mut self) -> impl WasiHttpView {
        self.durable_ctx.as_wasi_http_view()
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

    fn owned_worker_id(&self) -> &OwnedWorkerId {
        self.durable_ctx.owned_worker_id()
    }

    fn component_metadata(&self) -> &ComponentMetadata<CloudGolemTypes> {
        self.durable_ctx.component_metadata()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<Context>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.durable_ctx.worker_proxy()
    }

    async fn generate_unique_local_worker_id(
        &mut self,
        remote_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError> {
        self.durable_ctx
            .generate_unique_local_worker_id(remote_worker_id)
            .await
    }
}
