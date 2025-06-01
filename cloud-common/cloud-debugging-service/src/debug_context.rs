use crate::additional_deps::AdditionalDeps;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::invocation_context::{
    self, AttributeValue, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::WorkerResourceId;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentVersion, IdempotencyKey, OwnedWorkerId,
    PluginInstallationId, TargetWorkerId, WorkerId, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord,
};
use golem_wasm_rpc::golem_rpc_0_2_x::types::{
    Datetime, FutureInvokeResult, HostFutureInvokeResult, Pollable, WasmRpc,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{CancellationTokenEntry, ComponentId, Value};
use golem_wasm_rpc::{HostWasmRpc, RpcError, Uri, WitValue};
use golem_worker_executor::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor::error::GolemError;
use golem_worker_executor::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, ListDirectoryResult,
    ReadFileResult, TrapType, WorkerConfig,
};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::{ComponentMetadata, ComponentService};
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::{Oplog, OplogService};
use golem_worker_executor::services::plugins::Plugins;
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::rdbms::RdbmsService;
use golem_worker_executor::services::resource_limits::ResourceLimits;
use golem_worker_executor::services::rpc::Rpc;
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_event::WorkerEventService;
use golem_worker_executor::services::worker_fork::WorkerForkService;
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::services::{worker_enumeration, HasAll, HasConfig, HasOplogService};
use golem_worker_executor::worker::{RetryDecision, Worker};
use golem_worker_executor::workerctx::{
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, IndexedResourceStore,
    InvocationContextManagement, InvocationHooks, InvocationManagement, StatusManagement,
    UpdateManagement, WorkerCtx,
};
use golem_worker_executor::GolemTypes;
use std::collections::HashSet;
use std::sync::{Arc, RwLock, Weak};
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::WasiView;
use wasmtime_wasi_http::WasiHttpView;

pub struct DebugContext<T: GolemTypes> {
    pub durable_ctx: DurableWorkerCtx<Self>,
}

impl<T: GolemTypes> DurableWorkerCtxView<DebugContext<T>> for DebugContext<T> {
    fn durable_ctx(&self) -> &DurableWorkerCtx<DebugContext<T>> {
        &self.durable_ctx
    }

    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<DebugContext<T>> {
        &mut self.durable_ctx
    }
}

#[async_trait]
impl<T: GolemTypes> FuelManagement for DebugContext<T> {
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
impl<T: GolemTypes> ExternalOperations<Self> for DebugContext<T> {
    type ExtraDeps = AdditionalDeps;

    async fn get_last_error_and_retry_count<This: HasAll<Self> + Send + Sync>(
        this: &This,
        worker_id: &OwnedWorkerId,
        latest_worker_status: &WorkerStatusRecord,
    ) -> Option<LastError> {
        DurableWorkerCtx::<Self>::get_last_error_and_retry_count(
            this,
            worker_id,
            latest_worker_status,
        )
        .await
    }

    async fn compute_latest_worker_status<This: HasOplogService + HasConfig + Send + Sync>(
        this: &This,
        worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        DurableWorkerCtx::<Self>::compute_latest_worker_status(this, worker_id, metadata).await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Self> + Send),
        instance: &Instance,
    ) -> Result<RetryDecision, GolemError> {
        DurableWorkerCtx::<Self>::resume_replay(store, instance).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<RetryDecision, GolemError> {
        DurableWorkerCtx::<Self>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<This: HasAll<Self> + Send + Sync>(
        this: &This,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<Self>::record_last_known_limits(this, account_id, last_known_limits)
            .await
    }

    async fn on_worker_deleted<This: HasAll<Self> + Send + Sync>(
        this: &This,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<Self>::on_worker_deleted(this, worker_id).await
    }

    async fn on_shard_assignment_changed<This: HasAll<Self> + Send + Sync + 'static>(
        this: &This,
    ) -> Result<(), anyhow::Error> {
        DurableWorkerCtx::<Self>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl<T: GolemTypes> InvocationManagement for DebugContext<T> {
    async fn set_current_idempotency_key(&mut self, idempotency_key: IdempotencyKey) {
        self.durable_ctx
            .set_current_idempotency_key(idempotency_key)
            .await
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
impl<T: GolemTypes> StatusManagement for DebugContext<T> {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        if self.is_live() {
            Some(InterruptKind::Suspend)
        } else {
            self.durable_ctx.check_interrupt()
        }
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
impl<T: GolemTypes> InvocationHooks for DebugContext<T> {
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
impl<T: GolemTypes> UpdateManagement for DebugContext<T> {
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
impl<T: GolemTypes> IndexedResourceStore for DebugContext<T> {
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
impl<T: GolemTypes> ResourceStore for DebugContext<T> {
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
impl<T: GolemTypes> FileSystemReading for DebugContext<T> {
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
impl<T: GolemTypes> ResourceLimiterAsync for DebugContext<T> {
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
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

impl<T: GolemTypes> HostWasmRpc for DebugContext<T> {
    async fn new(
        &mut self,
        worker_id: golem_wasm_rpc::golem_rpc_0_2_x::types::WorkerId,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.new(worker_id).await
    }

    async fn ephemeral(&mut self, component_id: ComponentId) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.ephemeral(component_id).await
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

impl<T: GolemTypes> HostFutureInvokeResult for DebugContext<T> {
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
impl<T: GolemTypes> DynamicLinking<Self> for DebugContext<T> {
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Self>,
        component: &Component,
        component_metadata: &ComponentMetadata<T>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .link(engine, linker, component, component_metadata)
    }
}

#[async_trait]
impl<T: GolemTypes> InvocationContextManagement for DebugContext<T> {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, invocation_context::AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, GolemError> {
        self.durable_ctx.start_span(initial_attributes).await
    }

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, GolemError> {
        self.durable_ctx
            .start_child_span(parent, initial_attributes)
            .await
    }

    async fn finish_span(
        &mut self,
        span_id: &invocation_context::SpanId,
    ) -> Result<(), GolemError> {
        self.durable_ctx.finish_span(span_id).await
    }

    fn remove_span(&mut self, span_id: &invocation_context::SpanId) -> Result<(), GolemError> {
        self.durable_ctx.remove_span(span_id)
    }

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .set_span_attribute(span_id, key, value)
            .await
    }
}

#[async_trait]
impl<T: GolemTypes> WorkerCtx for DebugContext<T> {
    type PublicState = PublicDurableWorkerState<Self>;
    type Types = T;

    async fn create(
        owned_worker_id: OwnedWorkerId,
        component_metadata: ComponentMetadata<T>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        _active_workers: Arc<ActiveWorkers<Self>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Self>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService<T>>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<T>>,
        worker_fork: Arc<dyn WorkerForkService>,
        _resource_limits: Arc<dyn ResourceLimits>,
    ) -> Result<Self, GolemError> {
        let golem_ctx = DurableWorkerCtx::create(
            owned_worker_id,
            component_metadata,
            promise_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
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
            worker_fork,
        )
        .await?;
        Ok(Self {
            durable_ctx: golem_ctx,
        })
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

    fn component_metadata(&self) -> &ComponentMetadata<T> {
        self.durable_ctx.component_metadata()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<Self>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.durable_ctx.worker_proxy()
    }

    fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.durable_ctx.worker_fork()
    }

    async fn generate_unique_local_worker_id(
        &mut self,
        remote_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError> {
        self.durable_ctx
            .generate_unique_local_worker_id(remote_worker_id)
            .await
    }

    fn component_service(&self) -> Arc<dyn ComponentService<Self::Types> + Send + Sync> {
        self.durable_ctx().component_service()
    }
}
