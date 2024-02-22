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
use std::sync::{Arc, RwLock};

use crate::services::AdditionalDeps;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, VersionedWorkerId, WorkerId, WorkerMetadata,
    WorkerStatus,
};
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{Uri, Value};
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
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
use golem_worker_executor_base::services::rpc::Rpc;
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::HasAll;
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
    StatusManagement, WorkerCtx,
};
use wasmtime::component::{Instance, ResourceAny};
use wasmtime::{AsContextMut, ResourceLimiterAsync};

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

    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, GolemError> {
        Ok(current_level)
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
        store: &mut (impl AsContextMut<Data = Context> + Send),
    ) -> Result<(), GolemError> {
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

    async fn on_shard_assignment_changed<T: HasAll<Context> + Send + Sync>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<Context>::on_shard_assignment_changed(this).await
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
        vals: Result<Vec<Value>, GolemError>,
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
        function_input: &Vec<Value>,
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
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<Option<Vec<Value>>, Error> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
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
        rpc: Arc<dyn Rpc + Send + Sync>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let golem_ctx = DurableWorkerCtx::create(
            worker_id,
            account_id,
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
            rpc,
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

    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.durable_ctx.rpc()
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

    fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.borrow(resource_id)
    }

    fn remove(&mut self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.remove(resource_id)
    }
}
