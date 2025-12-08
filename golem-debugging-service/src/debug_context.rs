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

use crate::additional_deps::AdditionalDeps;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::base_model::OplogIndex;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentId, AgentMode};
use golem_common::model::component::{ComponentDto, ComponentRevision};
use golem_common::model::component::{ComponentFilePath, PluginPriority};
use golem_common::model::invocation_context::{
    self, AttributeValue, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::TimestampedUpdateDescription;
use golem_common::model::{IdempotencyKey, OwnedWorkerId, Timestamp, WorkerId, WorkerStatusRecord};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::GetFileSystemNodeResult;
use golem_wasm::golem_rpc_0_2_x::types::{
    Datetime, FutureInvokeResult, HostFutureInvokeResult, Pollable, WasmRpc,
};
use golem_wasm::wasmtime::{ResourceStore, ResourceTypeId};
use golem_wasm::{CancellationTokenEntry, Value, ValueAndType};
use golem_wasm::{HostWasmRpc, RpcError, Uri, WitValue};
use golem_worker_executor::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor::model::{
    ExecutionStatus, LastError, ReadFileResult, TrapType, WorkerConfig,
};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::agent_types::AgentTypesService;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::{Oplog, OplogService};
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::rdbms::RdbmsService;
use golem_worker_executor::services::resource_limits::ResourceLimits;
use golem_worker_executor::services::rpc::Rpc;
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_event::WorkerEventService;
use golem_worker_executor::services::worker_fork::WorkerForkService;
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::services::{worker_enumeration, HasAll};
use golem_worker_executor::worker::{RetryDecision, Worker};
use golem_worker_executor::workerctx::{
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, HasWasiConfigVars,
    InvocationContextManagement, InvocationHooks, InvocationManagement, LogEventEmitBehaviour,
    StatusManagement, UpdateManagement, WorkerCtx,
};
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock, Weak};
use uuid;
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::p2::WasiView;
use wasmtime_wasi_http::WasiHttpView;

pub struct DebugContext {
    pub durable_ctx: DurableWorkerCtx<Self>,
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

    async fn borrow_fuel(&mut self, _current_level: i64) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    fn borrow_fuel_sync(&mut self, _current_level: i64) {}

    async fn return_fuel(&mut self, _current_level: i64) -> Result<i64, WorkerExecutorError> {
        Ok(0)
    }
}

#[async_trait]
impl ExternalOperations<Self> for DebugContext {
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

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Self> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        DurableWorkerCtx::<Self>::resume_replay(store, instance, refresh_replay_target).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        DurableWorkerCtx::<Self>::prepare_instance(worker_id, instance, store).await
    }

    async fn on_shard_assignment_changed<This: HasAll<Self> + Send + Sync + 'static>(
        this: &This,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<Self>::on_shard_assignment_changed(this).await
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
impl StatusManagement for DebugContext {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        if self.is_live() {
            Some(InterruptKind::Suspend(Timestamp::now_utc()))
        } else {
            self.durable_ctx.check_interrupt()
        }
    }

    fn set_suspended(&self) {
        self.durable_ctx.set_suspended()
    }

    fn set_running(&self) {
        self.durable_ctx.set_running()
    }
}

#[async_trait]
impl InvocationHooks for DebugContext {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input)
            .await
    }

    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> RetryDecision {
        self.durable_ctx
            .on_invocation_failure(full_function_name, trap_type)
            .await
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

    async fn get_current_retry_point(&self) -> OplogIndex {
        self.durable_ctx.get_current_retry_point().await
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
        target_version: ComponentRevision,
        details: Option<String>,
    ) {
        self.durable_ctx
            .on_worker_update_failed(target_version, details)
            .await
    }

    async fn on_worker_update_succeeded(
        &self,
        target_version: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginPriority>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(target_version, new_component_size, new_active_plugins)
            .await
    }
}

#[async_trait]
impl ResourceStore for DebugContext {
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
impl FileSystemReading for DebugContext {
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
            self.durable_ctx.increase_memory(delta).await?;
            Ok(true)
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

impl HostWasmRpc for DebugContext {
    async fn new(
        &mut self,
        worker_id: golem_wasm::golem_rpc_0_2_x::types::AgentId,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.new(worker_id).await
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

impl HostFutureInvokeResult for DebugContext {
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

impl wasmtime_wasi::p2::bindings::cli::environment::Host for DebugContext {
    fn get_environment(
        &mut self,
    ) -> impl Future<Output = anyhow::Result<Vec<(String, String)>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(&mut self.durable_ctx)
    }

    fn get_arguments(&mut self) -> impl Future<Output = anyhow::Result<Vec<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_arguments(&mut self.durable_ctx)
    }

    fn initial_cwd(&mut self) -> impl Future<Output = anyhow::Result<Option<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::initial_cwd(&mut self.durable_ctx)
    }
}

#[async_trait]
impl DynamicLinking<Self> for DebugContext {
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Self>,
        component: &Component,
        component_metadata: &ComponentDto,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .link(engine, linker, component, component_metadata)
    }
}

#[async_trait]
impl InvocationContextManagement for DebugContext {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, invocation_context::AttributeValue)],
        activate: bool,
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx
            .start_span(initial_attributes, activate)
            .await
    }

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
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

    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        self.durable_ctx.clone_as_inherited_stack(current_span_id)
    }
}

impl HasWasiConfigVars for DebugContext {
    fn wasi_config_vars(&self) -> BTreeMap<String, String> {
        self.durable_ctx.wasi_config_vars()
    }
}

#[async_trait]
impl WorkerCtx for DebugContext {
    type PublicState = PublicDurableWorkerState<Self>;

    const LOG_EVENT_EMIT_BEHAVIOUR: LogEventEmitBehaviour = LogEventEmitBehaviour::Always;

    async fn create(
        _account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        agent_id: Option<AgentId>,
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
        worker_fork: Arc<dyn WorkerForkService>,
        _resource_limits: Arc<dyn ResourceLimits>,
        agent_types_service: Arc<dyn AgentTypesService>,
        shard_service: Arc<dyn ShardService>,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<uuid::Uuid>,
    ) -> Result<Self, WorkerExecutorError> {
        let golem_ctx = DurableWorkerCtx::create(
            owned_worker_id,
            agent_id,
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
            worker_fork,
            agent_types_service,
            shard_service,
            pending_update,
            original_phantom_id,
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

    fn agent_id(&self) -> Option<AgentId> {
        self.durable_ctx.agent_id()
    }

    fn agent_mode(&self) -> AgentMode {
        self.durable_ctx.agent_mode()
    }

    fn created_by(&self) -> &AccountId {
        self.durable_ctx.created_by()
    }

    fn component_metadata(&self) -> &ComponentDto {
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

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.durable_ctx().component_service()
    }

    fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.durable_ctx.worker_fork()
    }
}
