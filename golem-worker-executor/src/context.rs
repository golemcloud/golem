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

use anyhow::Error;
use async_trait::async_trait;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{Uri, Value};
use wasmtime::component::{Instance, ResourceAny};
use wasmtime::{AsContextMut, ResourceLimiterAsync};

use golem_common::model::{
    AccountId, CallingConvention, ComponentVersion, IdempotencyKey, OwnedWorkerId, WorkerId,
    WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, TrapType, WorkerConfig,
};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::component::ComponentMetadata;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::{Oplog, OplogService};
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::rpc::Rpc;
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::{worker_enumeration, HasAll};
use golem_worker_executor_base::worker::{RecoveryDecision, Worker};
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FuelManagement, IndexedResourceStore, InvocationHooks,
    InvocationManagement, IoCapturing, StatusManagement, UpdateManagement, WorkerCtx,
};

use crate::services::AdditionalDeps;

pub struct Context {
    pub durable_ctx: DurableWorkerCtx<Context>,
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
impl ExternalOperations<Context> for Context {
    type ExtraDeps = AdditionalDeps;

    async fn get_last_error_and_retry_count<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &OwnedWorkerId,
    ) -> Option<LastError> {
        DurableWorkerCtx::<Context>::get_last_error_and_retry_count(this, worker_id).await
    }

    async fn compute_latest_worker_status<T: HasAll<Context> + Send + Sync>(
        this: &T,
        worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        DurableWorkerCtx::<Context>::compute_latest_worker_status(this, worker_id, metadata).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Context> + Send),
    ) -> Result<RecoveryDecision, GolemError> {
        DurableWorkerCtx::<Context>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<T: HasAll<Context> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<Context>::record_last_known_limits(this, account_id, last_known_limits)
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
impl InvocationManagement for Context {
    async fn set_current_idempotency_key(&mut self, idempotency_key: IdempotencyKey) {
        self.durable_ctx
            .set_current_idempotency_key(idempotency_key)
            .await
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.durable_ctx.get_current_idempotency_key().await
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
        calling_convention: Option<CallingConvention>,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input, calling_convention)
            .await
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> RecoveryDecision {
        self.durable_ctx.on_invocation_failure(trap_type).await
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
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
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(target_version, new_component_size)
            .await
    }
}

impl IndexedResourceStore for Context {
    fn get_indexed_resource(&self, resource_name: &str, resource_params: &[String]) -> Option<u64> {
        self.durable_ctx
            .get_indexed_resource(resource_name, resource_params)
    }

    fn store_indexed_resource(
        &mut self,
        resource_name: &str,
        resource_params: &[String],
        resource: u64,
    ) {
        self.durable_ctx
            .store_indexed_resource(resource_name, resource_params, resource)
    }

    fn drop_indexed_resource(&mut self, resource_name: &str, resource_params: &[String]) {
        self.durable_ctx
            .drop_indexed_resource(resource_name, resource_params)
    }
}

#[async_trait]
impl WorkerCtx for Context {
    type PublicState = PublicDurableWorkerState<Context>;

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
        _active_workers: Arc<ActiveWorkers<Context>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Weak<Worker<Context>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
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
            config,
            worker_config,
            execution_status,
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
        DurableWorkerCtx::<Context>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.durable_ctx.worker_proxy()
    }
}

#[async_trait]
impl ResourceLimiterAsync for Context {
    async fn memory_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
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

impl ResourceStore for Context {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    fn add(&mut self, resource: ResourceAny) -> u64 {
        self.durable_ctx.add(resource)
    }

    fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.get(resource_id)
    }

    fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.borrow(resource_id)
    }
}
