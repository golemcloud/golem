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

use super::{AgentStore, LogEventEmitBehaviour};
use crate::durable_host::{DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState};
use crate::metrics::wasm::record_allocated_memory;
use crate::model::{
    CurrentResourceLimits, ExecutionStatus, LastError, ReadFileResult, TrapType, WorkerConfig,
};
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentService;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{Oplog, OplogService};
use crate::services::plugins::Plugins;
use crate::services::projects::ProjectService;
use crate::services::promise::PromiseService;
use crate::services::rdbms::RdbmsService;
use crate::services::resource_limits::ResourceLimits;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_fork::WorkerForkService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{worker_enumeration, HasAll, HasConfig, HasOplogService, NoAdditionalDeps};
use crate::worker::{RetryDecision, Worker};
use crate::workerctx::{
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, IndexedResourceStore,
    InvocationContextManagement, InvocationHooks, InvocationManagement, StatusManagement,
    UpdateManagement, WorkerCtx,
};
use anyhow::Error;
use async_trait::async_trait;
use golem_common::base_model::ProjectId;
use golem_common::model::agent::DataValue;
use golem_common::model::invocation_context::{
    self, AttributeValue, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::UpdateDescription;
use golem_common::model::oplog::WorkerResourceId;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentVersion, GetFileSystemNodeResult, IdempotencyKey,
    OwnedWorkerId, PluginInstallationId, TargetWorkerId, WorkerId, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord,
};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_wasm_rpc::golem_rpc_0_2_x::types::{
    Datetime, FutureInvokeResult, HostFutureInvokeResult, Pollable, WasmRpc,
};
use golem_wasm_rpc::wasmtime::{ResourceStore, ResourceTypeId};
use golem_wasm_rpc::{
    CancellationTokenEntry, ComponentId, HostWasmRpc, RpcError, Uri, Value, ValueAndType, WitValue,
};
use std::collections::HashSet;
use std::sync::{Arc, RwLock, Weak};
use tracing::debug;
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::p2::WasiView;
use wasmtime_wasi_http::WasiHttpView;

pub struct Context {
    pub durable_ctx: DurableWorkerCtx<Context>,
    config: Arc<GolemConfig>,
    project_owner_account_id: AccountId,
    resource_limits: Arc<dyn ResourceLimits>,
    last_fuel_level: i64,
    min_fuel_level: i64,
}

impl Context {
    pub fn new(
        golem_ctx: DurableWorkerCtx<Context>,
        config: Arc<GolemConfig>,
        project_owner_account_id: AccountId,
        resource_limits: Arc<dyn ResourceLimits + Send + Sync>,
    ) -> Self {
        Self {
            durable_ctx: golem_ctx,
            config,
            project_owner_account_id,
            resource_limits,
            last_fuel_level: i64::MAX,
            min_fuel_level: i64::MAX,
        }
    }

    pub async fn get_max_memory(&self) -> Result<usize, WorkerExecutorError> {
        self.resource_limits
            .get_max_memory(&self.project_owner_account_id)
            .await
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

    async fn borrow_fuel(&mut self) -> Result<(), WorkerExecutorError> {
        let amount = self
            .resource_limits
            .borrow_fuel(
                &self.project_owner_account_id,
                self.config.limits.fuel_to_borrow,
            )
            .await?;
        self.min_fuel_level -= amount;
        debug!(
            "borrowed fuel for {}: {}",
            self.project_owner_account_id, amount
        );
        Ok(())
    }

    fn borrow_fuel_sync(&mut self) {
        let amount = self.resource_limits.borrow_fuel_sync(
            &self.project_owner_account_id,
            self.config.limits.fuel_to_borrow,
        );
        match amount {
            Some(amount) => {
                debug!("borrowed fuel for {}: {}", self.project_owner_account_id, amount);
                self.min_fuel_level -= amount;
            }
            None => panic!("Illegal state: account's resource limits are not available when borrow_fuel_sync is called")
        }
    }

    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, WorkerExecutorError> {
        let unused = current_level - self.min_fuel_level;
        if unused > 0 {
            debug!("current_level: {current_level}");
            debug!("min_fuel_level: {}", self.min_fuel_level);
            debug!("last_fuel_level: {}", self.last_fuel_level);
            debug!(
                "returning unused fuel for {}: {}",
                self.project_owner_account_id, unused
            );
            self.resource_limits
                .return_fuel(&self.project_owner_account_id, unused)
                .await?
        }
        let consumed = self.last_fuel_level - current_level;
        self.last_fuel_level = current_level;
        debug!(
            "reset fuel mark for {}: {}",
            self.project_owner_account_id, current_level
        );
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

    async fn set_current_invocation_context(
        &mut self,
        stack: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.set_current_invocation_context(stack).await
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.durable_ctx.get_current_invocation_context().await
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

    async fn set_suspended(&self) -> Result<(), WorkerExecutorError> {
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
    ) -> Result<(), WorkerExecutorError> {
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
        output: Option<ValueAndType>,
    ) -> Result<(), WorkerExecutorError> {
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
    type ExtraDeps = NoAdditionalDeps;

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
    ) -> Result<WorkerStatusRecord, WorkerExecutorError> {
        DurableWorkerCtx::<Context>::compute_latest_worker_status(this, worker_id, metadata).await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Context> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<RetryDecision, WorkerExecutorError> {
        DurableWorkerCtx::<Context>::resume_replay(store, instance, refresh_replay_target).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<RetryDecision, WorkerExecutorError> {
        DurableWorkerCtx::<Context>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<T: HasAll<Context> + Send + Sync>(
        this: &T,
        project_id: &ProjectId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), WorkerExecutorError> {
        let project_owner = this.project_service().get_project_owner(project_id).await?;
        this.resource_limits()
            .update_last_known_limits(&project_owner, last_known_limits)
            .await
    }

    async fn on_worker_deleted<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), WorkerExecutorError> {
        DurableWorkerCtx::<Context>::on_worker_deleted(this, worker_id).await
    }

    async fn on_shard_assignment_changed<T: HasAll<Context> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<Context>::on_shard_assignment_changed(this).await
    }

    async fn on_worker_update_failed_to_start<T: HasAll<Context> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        target_version: ComponentVersion,
        details: Option<String>,
    ) -> Result<(), WorkerExecutorError> {
        DurableWorkerCtx::<Context>::on_worker_update_failed_to_start(
            this,
            account_id,
            owned_worker_id,
            target_version,
            details,
        )
        .await
    }
}

#[async_trait]
impl ResourceStore for Context {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        self.durable_ctx.add(resource, name).await
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        ResourceStore::get(&mut self.durable_ctx, resource_id).await
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
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
        update: &UpdateDescription,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(update, new_component_size, new_active_plugins)
            .await
    }
}

#[async_trait]
impl IndexedResourceStore for Context {
    fn get_indexed_resource(
        &self,
        resource_owner: &str,
        resource_name: &str,
        resource_params: &[String],
    ) -> Option<WorkerResourceId> {
        self.durable_ctx
            .get_indexed_resource(resource_owner, resource_name, resource_params)
    }

    async fn store_indexed_resource(
        &mut self,
        resource_owner: &str,
        resource_name: &str,
        resource_params: &[String],
        resource: WorkerResourceId,
    ) {
        self.durable_ctx
            .store_indexed_resource(resource_owner, resource_name, resource_params, resource)
            .await
    }

    fn drop_indexed_resource(
        &mut self,
        resource_owner: &str,
        resource_name: &str,
        resource_params: &[String],
    ) {
        self.durable_ctx
            .drop_indexed_resource(resource_owner, resource_name, resource_params)
    }
}

#[async_trait]
impl AgentStore for Context {
    async fn store_agent_instance(
        &mut self,
        agent_type: String,
        agent_id: String,
        parameters: DataValue,
    ) {
        self.durable_ctx
            .store_agent_instance(agent_type, agent_id, parameters)
            .await
    }

    async fn remove_agent_instance(
        &mut self,
        agent_type: String,
        agent_id: String,
        parameters: DataValue,
    ) {
        self.durable_ctx
            .remove_agent_instance(agent_type, agent_id, parameters)
            .await
    }
}

#[async_trait]
impl FileSystemReading for Context {
    async fn get_file_system_node(
        &self,
        path: &ComponentFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        self.durable_ctx.get_file_system_node(path).await
    }

    async fn read_file(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        self.durable_ctx.read_file(path).await
    }
}

impl HostWasmRpc for Context {
    async fn new(
        &mut self,
        worker_id: golem_wasm_rpc::WorkerId,
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
        component_metadata: &golem_service_base::model::Component,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .link(engine, linker, component, component_metadata)
    }
}

#[async_trait]
impl InvocationContextManagement for Context {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx.start_span(initial_attributes).await
    }

    async fn start_child_span(
        &mut self,
        parent: &invocation_context::SpanId,
        initial_attributes: &[(String, invocation_context::AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx
            .start_child_span(parent, initial_attributes)
            .await
    }

    fn remove_span(
        &mut self,
        span_id: &invocation_context::SpanId,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.remove_span(span_id)
    }

    async fn finish_span(
        &mut self,
        span_id: &invocation_context::SpanId,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.finish_span(span_id).await
    }

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .set_span_attribute(span_id, key, value)
            .await
    }
}

#[async_trait]
impl WorkerCtx for Context {
    type PublicState = PublicDurableWorkerState<Context>;

    const LOG_EVENT_EMIT_BEHAVIOUR: LogEventEmitBehaviour = LogEventEmitBehaviour::LiveOnly;

    async fn create(
        account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        event_service: Arc<dyn WorkerEventService>,
        _active_workers: Arc<ActiveWorkers<Self>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Self>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins>,
        worker_fork: Arc<dyn WorkerForkService>,
        resource_limits: Arc<dyn ResourceLimits>,
        project_service: Arc<dyn ProjectService>,
    ) -> Result<Self, WorkerExecutorError> {
        let golem_ctx = DurableWorkerCtx::create(
            owned_worker_id.clone(),
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
            config.clone(),
            worker_config.clone(),
            execution_status,
            file_loader,
            plugins,
            worker_fork,
            project_service,
        )
        .await?;
        Ok(Self::new(golem_ctx, config, account_id, resource_limits))
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

    fn component_metadata(&self) -> &golem_service_base::model::Component {
        self.durable_ctx.component_metadata()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<Context>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.durable_ctx.worker_proxy()
    }

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.durable_ctx.component_service()
    }

    fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.durable_ctx.worker_fork()
    }

    async fn generate_unique_local_worker_id(
        &mut self,
        remote_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, WorkerExecutorError> {
        self.durable_ctx
            .generate_unique_local_worker_id(remote_worker_id)
            .await
    }
}
