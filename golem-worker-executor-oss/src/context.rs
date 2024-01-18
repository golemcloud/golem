use std::sync::{Arc, RwLock};

use crate::services::AdditionalDeps;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::{AccountId, VersionedWorkerId};
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, HasDurableWorkerCtx, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::model::{ExecutionStatus, WorkerConfig};
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
use golem_worker_executor_base::workerctx::{FuelManagement, WorkerCtx};
use wasmtime::ResourceLimiterAsync;

pub struct Context {
    pub golem_ctx: DurableWorkerCtx<Context>,
}

impl HasDurableWorkerCtx for Context {
    type ExtraDeps = AdditionalDeps;

    fn durable_worker_ctx(&self) -> &DurableWorkerCtx<Self> {
        &self.golem_ctx
    }

    fn durable_worker_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Self> {
        &mut self.golem_ctx
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
            config,
            worker_config,
            execution_status,
        )
            .await?;
        Ok(Self { golem_ctx })
    }

    fn get_public_state(&self) -> &Self::PublicState {
        self.golem_ctx.get_public_state()
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &VersionedWorkerId {
        self.golem_ctx.worker_id()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<Context>::is_exit(error)
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
