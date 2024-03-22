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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::model::InterruptKind;
use crate::services::rpc::Rpc;
use crate::services::{
    active_workers, blob_store, golem_config, invocation_key, key_value, oplog, promise, scheduler,
    template, worker, HasActiveWorkers, HasAll, HasBlobStoreService, HasConfig, HasExtraDeps,
    HasInvocationKeyService, HasKeyValueService, HasOplogService, HasPromiseService,
    HasRecoveryManagement, HasRpc, HasSchedulerService, HasTemplateService, HasWasmtimeEngine,
    HasWorkerService,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_mutex::Mutex;
use async_trait::async_trait;
use golem_common::model::oplog::WorkerError;
use golem_common::model::{VersionedWorkerId, WorkerId, WorkerStatus};
use golem_common::retries::get_delay;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use wasmtime::Trap;

// TODO: move to crate::model?
#[derive(Clone, Debug)]
pub enum TrapType {
    Interrupt(InterruptKind),
    Exit,
    Error(WorkerError),
}

impl TrapType {
    pub fn from_error<Ctx: WorkerCtx>(error: &anyhow::Error) -> TrapType {
        match error.root_cause().downcast_ref::<InterruptKind>() {
            Some(kind) => TrapType::Interrupt(kind.clone()),
            None => match Ctx::is_exit(error) {
                Some(_) => TrapType::Exit,
                None => match error.root_cause().downcast_ref::<Trap>() {
                    Some(&Trap::StackOverflow) => TrapType::Error(WorkerError::StackOverflow),
                    _ => TrapType::Error(WorkerError::Unknown(format!("{:?}", error))),
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum RecoveryDecision {
    Immediate,
    Delayed(Duration),
    None,
}

#[async_trait]
pub trait RecoveryManagement {
    /// Makes a recovery decision in case the worker reached a trap. `previous_tries` is the number of retries already
    /// performed, and `trap_type` distinguishes errors, interrupts and exit signals.
    async fn schedule_recovery_on_trap(
        &self,
        previous_tries: u64,
        trap_type: &TrapType,
    ) -> RecoveryDecision;

    /// Makes a recovery decision when a worker gets started. `previous_tries` is the number of retries already
    /// performed and `WorkerError` is the error that caused the worker to fail in the last attempt.
    /// The other trap types are not relevant here, because interrupted workers can always be recovered,
    /// and exited workers can never.
    async fn schedule_recovery_on_startup(
        &self,
        previous_tries: u64,
        error: &WorkerError,
    ) -> RecoveryDecision;
}

pub struct RecoveryManagementDefault<Ctx: WorkerCtx> {
    scheduled_recoveries: Arc<Mutex<HashMap<VersionedWorkerId, JoinHandle<()>>>>,
    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    template_service: Arc<dyn template::TemplateService + Send + Sync>,
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    recovery_override: Option<Arc<dyn Fn(VersionedWorkerId) + Send + Sync>>,
    invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    rpc: Arc<dyn Rpc + Send + Sync>,
    extra_deps: Ctx::ExtraDeps,
}

impl<Ctx: WorkerCtx> Clone for RecoveryManagementDefault<Ctx> {
    fn clone(&self) -> Self {
        Self {
            scheduled_recoveries: self.scheduled_recoveries.clone(),
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            template_service: self.template_service.clone(),
            worker_service: self.worker_service.clone(),
            oplog_service: self.oplog_service.clone(),
            promise_service: self.promise_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            golem_config: self.golem_config.clone(),
            recovery_override: self.recovery_override.clone(),
            invocation_key_service: self.invocation_key_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            rpc: self.rpc.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> HasActiveWorkers<Ctx> for RecoveryManagementDefault<Ctx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.active_workers.clone()
    }
}

impl<Ctx: WorkerCtx> HasTemplateService for RecoveryManagementDefault<Ctx> {
    fn template_service(&self) -> Arc<dyn template::TemplateService + Send + Sync> {
        self.template_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for RecoveryManagementDefault<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for RecoveryManagementDefault<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService + Send + Sync> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasInvocationKeyService for RecoveryManagementDefault<Ctx> {
    fn invocation_key_service(
        &self,
    ) -> Arc<dyn invocation_key::InvocationKeyService + Send + Sync> {
        self.invocation_key_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for RecoveryManagementDefault<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService + Send + Sync> {
        self.promise_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWasmtimeEngine<Ctx> for RecoveryManagementDefault<Ctx> {
    fn engine(&self) -> Arc<wasmtime::Engine> {
        self.engine.clone()
    }

    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>> {
        self.linker.clone()
    }

    fn runtime(&self) -> Handle {
        self.runtime.clone()
    }
}

impl<Ctx: WorkerCtx> HasKeyValueService for RecoveryManagementDefault<Ctx> {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService + Send + Sync> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for RecoveryManagementDefault<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService + Send + Sync> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for RecoveryManagementDefault<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService + Send + Sync> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for RecoveryManagementDefault<Ctx> {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRecoveryManagement for RecoveryManagementDefault<Ctx> {
    fn recovery_management(&self) -> Arc<dyn RecoveryManagement + Send + Sync> {
        Arc::new(self.clone())
    }
}

impl<Ctx: WorkerCtx> HasRpc for RecoveryManagementDefault<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.rpc.clone()
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for RecoveryManagementDefault<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> RecoveryManagementDefault<Ctx> {
    pub fn new(
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        template_service: Arc<dyn template::TemplateService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            scheduled_recoveries: Arc::new(Mutex::new(HashMap::new())),
            active_workers,
            engine,
            linker,
            runtime,
            template_service,
            worker_service,
            oplog_service,
            promise_service,
            scheduler_service,
            invocation_key_service,
            key_value_service,
            blob_store_service,
            golem_config,
            recovery_override: None,
            rpc,
            extra_deps,
        }
    }

    #[cfg(test)]
    pub fn new_with_override<F>(
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        template_service: Arc<dyn template::TemplateService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        extra_deps: Ctx::ExtraDeps,
        recovery_override: F,
    ) -> Self
    where
        F: Fn(VersionedWorkerId) + Send + Sync + 'static,
    {
        Self {
            scheduled_recoveries: Arc::new(Mutex::new(HashMap::new())),
            active_workers,
            engine,
            linker,
            runtime,
            template_service,
            worker_service,
            oplog_service,
            promise_service,
            scheduler_service,
            invocation_key_service,
            key_value_service,
            blob_store_service,
            golem_config,
            recovery_override: Some(Arc::new(recovery_override)),
            rpc,
            extra_deps,
        }
    }

    async fn schedule_recovery(
        &self,
        worker_id: &VersionedWorkerId,
        decision: RecoveryDecision,
    ) -> RecoveryDecision {
        match decision {
            RecoveryDecision::Immediate => {
                // NOTE: Even immediate recovery must be spawned to allow the original worker to get dropped first
                let clone = self.clone();
                let worker_id_clone = worker_id.clone();
                let handle = tokio::spawn(async move {
                    clone
                        .scheduled_recoveries
                        .lock()
                        .await
                        .remove(&worker_id_clone);
                    match &clone.recovery_override {
                        Some(f) => f(worker_id_clone.clone()),
                        None => {
                            let interrupted = clone
                                .is_marked_as_interrupted(&worker_id_clone.worker_id)
                                .await;
                            if !interrupted {
                                recover_worker(&clone, &worker_id_clone).await;
                            }
                        }
                    }
                });
                self.cancel_scheduled_recovery(worker_id).await;
                self.scheduled_recoveries
                    .lock()
                    .await
                    .insert(worker_id.clone(), handle);
            }
            RecoveryDecision::Delayed(duration) => {
                let clone = self.clone();
                let worker_id_clone = worker_id.clone();
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    clone
                        .scheduled_recoveries
                        .lock()
                        .await
                        .remove(&worker_id_clone);
                    match &clone.recovery_override {
                        Some(f) => f(worker_id_clone.clone()),
                        None => {
                            let interrupted = clone
                                .is_marked_as_interrupted(&worker_id_clone.worker_id)
                                .await;
                            if !interrupted {
                                recover_worker(&clone, &worker_id_clone).await;
                            }
                        }
                    }
                });
                self.cancel_scheduled_recovery(worker_id).await;
                self.scheduled_recoveries
                    .lock()
                    .await
                    .insert(worker_id.clone(), handle);
            }
            RecoveryDecision::None => {}
        }

        decision
    }

    async fn cancel_scheduled_recovery(&self, worker_id: &VersionedWorkerId) {
        if let Some(handle) = self.scheduled_recoveries.lock().await.remove(worker_id) {
            handle.abort();
        }
    }

    async fn is_marked_as_interrupted(&self, worker_id: &WorkerId) -> bool {
        let worker_metadata = self.worker_service().get(worker_id).await;
        Ctx::compute_latest_worker_status(self, worker_id, &worker_metadata)
            .await
            .map(|s| s.status == WorkerStatus::Interrupted)
            .unwrap_or(false)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> RecoveryManagement for RecoveryManagementDefault<Ctx> {
    async fn schedule_recovery_on_trap(
        &self,
        previous_tries: u64,
        trap_type: &TrapType,
    ) -> RecoveryDecision {
        match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt) => RecoveryDecision::None,
            TrapType::Interrupt(InterruptKind::Suspend) => RecoveryDecision::None,
            TrapType::Interrupt(InterruptKind::Restart) => RecoveryDecision::Immediate,
            TrapType::Interrupt(InterruptKind::Jump) => RecoveryDecision::Immediate,
            TrapType::Exit => RecoveryDecision::None,
            TrapType::Error(WorkerError::StackOverflow) => RecoveryDecision::None,
            TrapType::Error(_) => {
                let retry_config = &self.golem_config.retry;
                match get_delay(retry_config, previous_tries) {
                    Some(delay) => RecoveryDecision::Delayed(delay),
                    None => RecoveryDecision::None,
                }
            }
        }
    }

    async fn schedule_recovery_on_startup(
        &self,
        previous_tries: u64,
        error: &WorkerError,
    ) -> RecoveryDecision {
        match error {
            WorkerError::Unknown(_) => {
                if previous_tries < (self.golem_config.retry.max_attempts as u64) {
                    RecoveryDecision::Immediate
                } else {
                    RecoveryDecision::None
                }
            }
            WorkerError::StackOverflow => RecoveryDecision::None,
        }
    }
}

async fn recover_worker<Ctx: WorkerCtx, T>(this: &T, worker_id: &VersionedWorkerId)
where
    T: HasAll<Ctx> + Clone + Send + Sync + 'static,
{
    info!("Recovering instance: {worker_id}");

    match this.worker_service().get(&worker_id.worker_id).await {
        Some(worker) => {
            let worker_details = Worker::get_or_create_with_config(
                this,
                &worker_id.worker_id.clone(),
                worker.args,
                worker.env,
                Some(worker_id.template_version),
                worker.account_id,
            )
            .await;

            if let Err(e) = worker_details {
                warn!("Failed to recover worker {}: {:?}", worker_id, e);
            }
        }
        None => {
            warn!("Worker {} not found", worker_id);
        }
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct RecoveryManagementMock;

#[cfg(any(feature = "mocks", test))]
impl Default for RecoveryManagementMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl RecoveryManagementMock {
    #[allow(unused)]
    pub fn new() -> Self {
        RecoveryManagementMock
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl RecoveryManagement for RecoveryManagementMock {
    async fn schedule_recovery_on_trap(
        &self,
        _previous_tries: u64,
        _trap_type: &TrapType,
    ) -> RecoveryDecision {
        todo!()
    }

    async fn schedule_recovery_on_startup(
        &self,
        _previous_tries: u64,
        _error: &WorkerError,
    ) -> RecoveryDecision {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::string::FromUtf8Error;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

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
    use crate::services::{
        All, HasAll, HasBlobStoreService, HasConfig, HasExtraDeps, HasInvocationKeyService,
        HasKeyValueService, HasPromiseService, HasRpc, HasTemplateService, HasWasmtimeEngine,
        HasWorkerService,
    };
    use crate::workerctx::{
        ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
        PublicWorkerIo, StatusManagement, WorkerCtx,
    };
    use anyhow::Error;
    use async_trait::async_trait;
    use bytes::Bytes;
    use golem_common::model::oplog::WorkerError;
    use golem_common::model::{
        AccountId, CallingConvention, InvocationKey, TemplateId, VersionedWorkerId, WorkerId,
        WorkerMetadata, WorkerStatus, WorkerStatusRecord,
    };
    use golem_wasm_rpc::wasmtime::ResourceStore;
    use golem_wasm_rpc::{Uri, Value};
    use tokio::runtime::Handle;
    use tokio::time::{timeout, Instant};
    use wasmtime::component::{Instance, ResourceAny};
    use wasmtime::{AsContextMut, ResourceLimiterAsync};

    use crate::services::oplog::{OplogService, OplogServiceMock};
    use crate::services::recovery::{RecoveryManagement, RecoveryManagementDefault};
    use crate::services::rpc::Rpc;
    use crate::services::scheduler;
    use crate::services::scheduler::SchedulerService;

    struct EmptyContext {
        worker_id: VersionedWorkerId,
        public_state: EmptyPublicState,
        rpc: Arc<dyn Rpc + Send + Sync>,
    }

    #[derive(Clone)]
    struct EmptyPublicState;

    #[async_trait]
    impl PublicWorkerIo for EmptyPublicState {
        fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
            unimplemented!()
        }

        async fn enqueue(&self, _message: Bytes, _invocation_key: InvocationKey) {
            unimplemented!()
        }
    }

    #[async_trait]
    impl FuelManagement for EmptyContext {
        fn is_out_of_fuel(&self, _current_level: i64) -> bool {
            unimplemented!()
        }

        async fn borrow_fuel(&mut self) -> Result<(), GolemError> {
            unimplemented!()
        }

        fn borrow_fuel_sync(&mut self) {
            unimplemented!()
        }

        async fn return_fuel(&mut self, _current_level: i64) -> Result<i64, GolemError> {
            unimplemented!()
        }
    }

    #[async_trait]
    impl InvocationManagement for EmptyContext {
        async fn set_current_invocation_key(&mut self, _invocation_key: Option<InvocationKey>) {
            unimplemented!()
        }

        async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
            unimplemented!()
        }

        async fn interrupt_invocation_key(&mut self, _key: &InvocationKey) {
            unimplemented!()
        }

        async fn resume_invocation_key(&mut self, _key: &InvocationKey) {
            unimplemented!()
        }

        async fn confirm_invocation_key(
            &mut self,
            _key: &InvocationKey,
            _vals: Result<Vec<Value>, GolemError>,
        ) {
            unimplemented!()
        }
    }

    #[async_trait]
    impl IoCapturing for EmptyContext {
        async fn start_capturing_stdout(&mut self, _provided_stdin: String) {
            unimplemented!()
        }

        async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error> {
            unimplemented!()
        }
    }

    #[async_trait]
    impl StatusManagement for EmptyContext {
        fn check_interrupt(&self) -> Option<InterruptKind> {
            unimplemented!()
        }

        fn set_suspended(&self) {
            unimplemented!()
        }

        fn set_running(&self) {
            unimplemented!()
        }

        async fn get_worker_status(&self) -> WorkerStatus {
            unimplemented!()
        }

        async fn store_worker_status(&self, _status: WorkerStatus) {
            unimplemented!()
        }

        async fn deactivate(&self) {
            unimplemented!()
        }
    }

    #[async_trait]
    impl InvocationHooks for EmptyContext {
        async fn on_exported_function_invoked(
            &mut self,
            _full_function_name: &str,
            _function_input: &Vec<Value>,
            _calling_convention: Option<&CallingConvention>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }

        async fn on_invocation_failure(&mut self, _error: &Error) -> Result<(), Error> {
            unimplemented!()
        }

        async fn on_invocation_failure_deactivated(
            &mut self,
            _error: &Error,
        ) -> Result<WorkerStatus, Error> {
            unimplemented!()
        }

        async fn on_invocation_success(
            &mut self,
            _full_function_name: &str,
            _function_input: &Vec<Value>,
            _consumed_fuel: i64,
            _output: Vec<Value>,
        ) -> Result<Option<Vec<Value>>, Error> {
            unimplemented!()
        }
    }

    #[async_trait]
    impl ExternalOperations<Self> for EmptyContext {
        type ExtraDeps = ();

        async fn set_worker_status<T: HasAll<Self> + Send + Sync>(
            _this: &T,
            _worker_id: &WorkerId,
            _status: WorkerStatus,
        ) -> Result<(), GolemError> {
            unimplemented!()
        }

        async fn get_worker_retry_count<T: HasAll<Self> + Send + Sync>(
            _this: &T,
            _worker_id: &WorkerId,
        ) -> u64 {
            unimplemented!()
        }

        async fn compute_latest_worker_status<T: HasAll<Self> + Send + Sync>(
            _this: &T,
            _worker_id: &WorkerId,
            _metadata: &Option<WorkerMetadata>,
        ) -> Result<WorkerStatusRecord, GolemError> {
            unimplemented!()
        }

        async fn prepare_instance(
            _worker_id: &VersionedWorkerId,
            _instance: &Instance,
            _store: &mut (impl AsContextMut<Data = Self> + Send),
        ) -> Result<(), GolemError> {
            unimplemented!()
        }

        async fn record_last_known_limits<T: HasExtraDeps<Self> + Send + Sync>(
            _this: &T,
            _account_id: &AccountId,
            _last_known_limits: &CurrentResourceLimits,
        ) -> Result<(), GolemError> {
            unimplemented!()
        }

        async fn on_worker_deleted<T: HasExtraDeps<Self> + Send + Sync>(
            _this: &T,
            _worker_id: &WorkerId,
        ) -> Result<(), GolemError> {
            unimplemented!()
        }

        async fn on_shard_assignment_changed<T: HasAll<Self> + Send + Sync>(
            _this: &T,
        ) -> Result<(), Error> {
            unimplemented!()
        }
    }

    #[async_trait]
    impl WorkerCtx for EmptyContext {
        type PublicState = EmptyPublicState;

        async fn create(
            _worker_id: VersionedWorkerId,
            _account_id: AccountId,
            _promise_service: Arc<dyn PromiseService + Send + Sync>,
            _invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
            _worker_service: Arc<dyn WorkerService + Send + Sync>,
            _key_value_service: Arc<dyn KeyValueService + Send + Sync>,
            _blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
            _event_service: Arc<dyn WorkerEventService + Send + Sync>,
            _active_workers: Arc<ActiveWorkers<Self>>,
            _oplog_service: Arc<dyn OplogService + Send + Sync>,
            _scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
            _recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
            rpc: Arc<dyn Rpc + Send + Sync>,
            _extra_deps: Self::ExtraDeps,
            _config: Arc<GolemConfig>,
            _worker_config: WorkerConfig,
            _execution_status: Arc<RwLock<ExecutionStatus>>,
        ) -> Result<Self, GolemError> {
            Ok(EmptyContext {
                worker_id: create_test_id(),
                public_state: EmptyPublicState,
                rpc,
            })
        }

        fn get_public_state(&self) -> &Self::PublicState {
            &self.public_state
        }

        fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
            self
        }

        fn worker_id(&self) -> &VersionedWorkerId {
            &self.worker_id
        }

        fn is_exit(_error: &Error) -> Option<i32> {
            None
        }

        fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
            self.rpc.clone()
        }
    }

    #[async_trait]
    impl ResourceLimiterAsync for EmptyContext {
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

    impl ResourceStore for EmptyContext {
        fn self_uri(&self) -> Uri {
            todo!()
        }

        fn add(&mut self, _resource: ResourceAny) -> u64 {
            todo!()
        }

        fn get(&mut self, _resource_id: u64) -> Option<ResourceAny> {
            todo!()
        }

        fn borrow(&self, _resource_id: u64) -> Option<ResourceAny> {
            todo!()
        }
    }

    async fn create_recovery_management<F>(
        recovery_fn: F,
    ) -> RecoveryManagementDefault<EmptyContext>
    where
        F: Fn(VersionedWorkerId) + Send + Sync + 'static,
    {
        let deps: All<EmptyContext> = All::mocked(()).await;
        let active_workers = Arc::new(ActiveWorkers::bounded(100, 0.01, Duration::from_secs(60)));
        let linker = Arc::new(wasmtime::component::Linker::new(&deps.engine()));
        let oplog = Arc::new(OplogServiceMock::new());
        let scheduler = Arc::new(scheduler::SchedulerServiceMock::new());

        let runtime = Handle::current();

        RecoveryManagementDefault::new_with_override(
            active_workers,
            deps.engine(),
            linker,
            runtime,
            deps.template_service(),
            deps.worker_service(),
            oplog,
            deps.promise_service(),
            scheduler,
            deps.invocation_key_service(),
            deps.key_value_service(),
            deps.blob_store_service(),
            deps.config(),
            deps.rpc(),
            (),
            recovery_fn,
        )
    }

    fn create_test_id() -> VersionedWorkerId {
        let uuid = uuid::Uuid::parse_str("14e55083-2ff5-44ec-a414-595a748b19a0").unwrap();

        VersionedWorkerId {
            worker_id: WorkerId {
                template_id: TemplateId(uuid),
                worker_name: "test-worker".to_string(),
            },
            template_version: 1,
        }
    }

    #[tokio::test]
    async fn immediately_recovers_worker_on_startup_with_no_errors() {
        let start_time = Instant::now();
        let (sender, mut receiver) =
            tokio::sync::broadcast::channel::<(VersionedWorkerId, Duration)>(1);
        let svc = create_recovery_management(move |id| {
            let schedule_time = Instant::now();
            let elapsed = schedule_time.duration_since(start_time);
            sender.send((id, elapsed)).unwrap();
        })
        .await;
        let _ = svc
            .schedule_recovery_on_startup(0, &WorkerError::Unknown("x".to_string()))
            .await;
        let (id, elapsed) = receiver.recv().await.unwrap();
        assert_eq!(id, test_id);
        assert!(elapsed.as_millis() < 100, "elapsed time was {:?}", elapsed);
    }

    #[tokio::test]
    async fn does_not_recover_worker_on_startup_with_many_errors() {
        let test_id = create_test_id();
        let start_time = Instant::now();
        let (sender, mut receiver) =
            tokio::sync::broadcast::channel::<(VersionedWorkerId, Duration)>(1);
        let svc = create_recovery_management(move |id| {
            let schedule_time = Instant::now();
            let elapsed = schedule_time.duration_since(start_time);
            sender.send((id, elapsed)).unwrap();
        })
        .await;
        let _ = svc.schedule_recovery_on_startup(&test_id, 100).await;
        let res = timeout(Duration::from_secs(1), receiver.recv()).await;
        assert!(res.is_err());
    }
}
