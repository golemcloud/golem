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

use async_mutex::Mutex;
use async_trait::async_trait;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tracing::{info, warn, Instrument};

use golem_common::config::RetryConfig;
use golem_common::model::oplog::WorkerError;
use golem_common::model::{OwnedWorkerId, WorkerId, WorkerStatus};
use golem_common::retries::get_delay;

use crate::model::{InterruptKind, LastError, TrapType};
use crate::services::events::Events;
use crate::services::rpc::Rpc;
use crate::services::{
    active_workers, blob_store, component, golem_config, key_value, oplog, promise, scheduler,
    worker, worker_activator, worker_enumeration, worker_proxy, HasActiveWorkers, HasAll,
    HasBlobStoreService, HasComponentService, HasConfig, HasEvents, HasExtraDeps,
    HasKeyValueService, HasOplogService, HasPromiseService, HasRecoveryManagement, HasRpc,
    HasRunningWorkerEnumerationService, HasSchedulerService, HasWasmtimeEngine, HasWorkerActivator,
    HasWorkerEnumerationService, HasWorkerProxy, HasWorkerService,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;

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
        owned_worker_id: &OwnedWorkerId,
        retry_config: &RetryConfig,
        previous_tries: u64,
        trap_type: &TrapType,
    ) -> RecoveryDecision;

    /// Makes a recovery decision when a worker gets started. `previous_tries` is the number of retries already
    /// performed and `WorkerError` is the error that caused the worker to fail in the last attempt.
    /// The other trap types are not relevant here, because interrupted workers can always be recovered,
    /// and exited workers can never.
    async fn schedule_recovery_on_startup(
        &self,
        owned_worker_id: &OwnedWorkerId,
        retry_config: &RetryConfig,
        last_error: &Option<LastError>,
    ) -> RecoveryDecision;
}

pub struct RecoveryManagementDefault<Ctx: WorkerCtx> {
    scheduled_recoveries: Arc<Mutex<HashMap<WorkerId, JoinHandle<()>>>>,
    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    component_service: Arc<dyn component::ComponentService + Send + Sync>,
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    recovery_override: Option<Arc<dyn Fn(WorkerId) + Send + Sync>>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    rpc: Arc<dyn Rpc + Send + Sync>,
    worker_activator: Arc<dyn worker_activator::WorkerActivator + Send + Sync>,
    worker_proxy: Arc<dyn worker_proxy::WorkerProxy + Send + Sync>,
    events: Arc<Events>,
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
            component_service: self.component_service.clone(),
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            oplog_service: self.oplog_service.clone(),
            promise_service: self.promise_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            golem_config: self.golem_config.clone(),
            recovery_override: self.recovery_override.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            rpc: self.rpc.clone(),
            worker_activator: self.worker_activator.clone(),
            worker_proxy: self.worker_proxy.clone(),
            events: self.events.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> HasActiveWorkers<Ctx> for RecoveryManagementDefault<Ctx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.active_workers.clone()
    }
}

impl<Ctx: WorkerCtx> HasComponentService for RecoveryManagementDefault<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService + Send + Sync> {
        self.component_service.clone()
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

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for RecoveryManagementDefault<Ctx> {
    fn worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for RecoveryManagementDefault<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync> {
        self.running_worker_enumeration_service.clone()
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

impl<Ctx: WorkerCtx> HasWorkerActivator for RecoveryManagementDefault<Ctx> {
    fn worker_activator(&self) -> Arc<dyn worker_activator::WorkerActivator + Send + Sync> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for RecoveryManagementDefault<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn worker_proxy::WorkerProxy + Send + Sync> {
        self.worker_proxy.clone()
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

impl<Ctx: WorkerCtx> HasEvents for RecoveryManagementDefault<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
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
        component_service: Arc<dyn component::ComponentService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_activator: Arc<dyn worker_activator::WorkerActivator + Send + Sync>,
        worker_proxy: Arc<dyn worker_proxy::WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        golem_config: Arc<golem_config::GolemConfig>,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            scheduled_recoveries: Arc::new(Mutex::new(HashMap::new())),
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            oplog_service,
            promise_service,
            scheduler_service,
            key_value_service,
            blob_store_service,
            golem_config,
            recovery_override: None,
            rpc,
            worker_activator,
            worker_proxy,
            events,
            extra_deps,
        }
    }

    #[cfg(test)]
    pub fn new_with_override<F>(
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn component::ComponentService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_activator: Arc<dyn worker_activator::WorkerActivator + Send + Sync>,
        worker_proxy: Arc<dyn worker_proxy::WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        extra_deps: Ctx::ExtraDeps,
        recovery_override: F,
    ) -> Self
    where
        F: Fn(WorkerId) + Send + Sync + 'static,
    {
        Self {
            scheduled_recoveries: Arc::new(Mutex::new(HashMap::new())),
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            oplog_service,
            promise_service,
            scheduler_service,
            key_value_service,
            blob_store_service,
            golem_config,
            recovery_override: Some(Arc::new(recovery_override)),
            rpc,
            worker_activator,
            worker_proxy,
            events,
            extra_deps,
        }
    }

    fn get_recovery_decision_on_trap(
        &self,
        retry_config: &RetryConfig,
        previous_tries: u64,
        trap_type: &TrapType,
    ) -> RecoveryDecision {
        match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt) => RecoveryDecision::None,
            TrapType::Interrupt(InterruptKind::Suspend) => RecoveryDecision::None,
            TrapType::Interrupt(InterruptKind::Restart) => RecoveryDecision::Immediate,
            TrapType::Interrupt(InterruptKind::Jump) => RecoveryDecision::Immediate,
            TrapType::Exit => RecoveryDecision::None,
            TrapType::Error(error) => {
                if is_worker_error_retriable(retry_config, error, previous_tries) {
                    match get_delay(retry_config, previous_tries) {
                        Some(delay) => RecoveryDecision::Delayed(delay),
                        None => RecoveryDecision::None,
                    }
                } else {
                    RecoveryDecision::None
                }
            }
        }
    }

    fn get_recovery_decision_on_startup(
        &self,
        retry_config: &RetryConfig,
        last_error: &Option<LastError>,
    ) -> RecoveryDecision {
        match last_error {
            Some(last_error) => {
                if is_worker_error_retriable(
                    retry_config,
                    &last_error.error,
                    last_error.retry_count,
                ) {
                    RecoveryDecision::Immediate
                } else {
                    RecoveryDecision::None
                }
            }
            None => RecoveryDecision::Immediate,
        }
    }

    async fn schedule_recovery(
        &self,
        owned_worker_id: &OwnedWorkerId,
        decision: RecoveryDecision,
    ) -> RecoveryDecision {
        match decision {
            RecoveryDecision::Immediate => {
                let span = tracing::info_span!("recovery", decision = "immediate");

                // NOTE: Even immediate recovery must be spawned to allow the original worker to get dropped first
                let clone = self.clone();
                let owned_worker_id_clone = owned_worker_id.clone();
                let worker_id_clone = owned_worker_id.worker_id();

                let handle = tokio::spawn(
                    async move {
                        clone
                            .scheduled_recoveries
                            .lock()
                            .await
                            .remove(&worker_id_clone);
                        match &clone.recovery_override {
                            Some(f) => f(worker_id_clone.clone()),
                            None => {
                                let interrupted =
                                    clone.is_marked_as_interrupted(&owned_worker_id_clone).await;
                                if !interrupted {
                                    info!(
                                    "Initiating immediate recovery for worker: {worker_id_clone}"
                                );
                                    recover_worker(&clone, &owned_worker_id_clone).await;
                                }
                            }
                        }
                    }
                    .instrument(span),
                );
                self.cancel_scheduled_recovery(&owned_worker_id.worker_id)
                    .await;
                self.scheduled_recoveries
                    .lock()
                    .await
                    .insert(owned_worker_id.worker_id(), handle);
            }
            RecoveryDecision::Delayed(duration) => {
                let span = tracing::info_span!(
                    "recovery",
                    decision = "delayed",
                    duration = format!("{:?}", duration)
                );

                let clone = self.clone();
                let owned_worker_id_clone = owned_worker_id.clone();
                let worker_id_clone = owned_worker_id.worker_id();

                let handle = tokio::spawn(
                    async move {
                        tokio::time::sleep(duration).await;
                        clone
                            .scheduled_recoveries
                            .lock()
                            .await
                            .remove(&worker_id_clone);
                        match &clone.recovery_override {
                            Some(f) => f(worker_id_clone.clone()),
                            None => {
                                let interrupted =
                                    clone.is_marked_as_interrupted(&owned_worker_id_clone).await;
                                if !interrupted {
                                    info!(
                                    "Initiating scheduled recovery for worker: {worker_id_clone}"
                                );
                                }
                            }
                        }
                    }
                    .instrument(span),
                );
                self.cancel_scheduled_recovery(&owned_worker_id.worker_id)
                    .await;
                self.scheduled_recoveries
                    .lock()
                    .await
                    .insert(owned_worker_id.worker_id(), handle);
            }
            RecoveryDecision::None => {}
        }

        decision
    }

    async fn cancel_scheduled_recovery(&self, worker_id: &WorkerId) {
        if let Some(handle) = self.scheduled_recoveries.lock().await.remove(worker_id) {
            handle.abort();
        }
    }

    async fn is_marked_as_interrupted(&self, owned_worker_id: &OwnedWorkerId) -> bool {
        let worker_metadata = self.worker_service().get(owned_worker_id).await;
        Ctx::compute_latest_worker_status(self, owned_worker_id, &worker_metadata)
            .await
            .map(|s| s.status == WorkerStatus::Interrupted)
            .unwrap_or(false)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> RecoveryManagement for RecoveryManagementDefault<Ctx> {
    async fn schedule_recovery_on_trap(
        &self,
        owned_worker_id: &OwnedWorkerId,
        retry_config: &RetryConfig,
        previous_tries: u64,
        trap_type: &TrapType,
    ) -> RecoveryDecision {
        self.schedule_recovery(
            owned_worker_id,
            self.get_recovery_decision_on_trap(retry_config, previous_tries, trap_type),
        )
        .await
    }

    async fn schedule_recovery_on_startup(
        &self,
        owned_worker_id: &OwnedWorkerId,
        retry_config: &RetryConfig,
        previous_error: &Option<LastError>,
    ) -> RecoveryDecision {
        self.schedule_recovery(
            owned_worker_id,
            self.get_recovery_decision_on_startup(retry_config, previous_error),
        )
        .await
    }
}

async fn recover_worker<Ctx: WorkerCtx, T>(this: &T, owned_worker_id: &OwnedWorkerId)
where
    T: HasAll<Ctx> + Clone + Send + Sync + 'static,
{
    info!("Recovering worker");

    match this.worker_service().get(owned_worker_id).await {
        Some(_) => {
            let worker_details =
                Worker::get_or_create(this, owned_worker_id, None, None, None).await;

            if let Err(e) = worker_details {
                warn!("Failed to recover worker: {:?}", e);
            }
        }
        None => {
            warn!("Worker not found");
        }
    }
}

pub fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &WorkerError,
    retry_count: u64,
) -> bool {
    match error {
        WorkerError::Unknown(_) => retry_count < (retry_config.max_attempts as u64),
        WorkerError::StackOverflow => false,
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
        _owned_worker_id: &OwnedWorkerId,
        _retry_config: &RetryConfig,
        _previous_tries: u64,
        _trap_type: &TrapType,
    ) -> RecoveryDecision {
        unimplemented!()
    }

    async fn schedule_recovery_on_startup(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _retry_config: &RetryConfig,
        _previous_error: &Option<LastError>,
    ) -> RecoveryDecision {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use std::string::FromUtf8Error;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    use anyhow::Error;
    use async_trait::async_trait;
    use golem_wasm_rpc::wasmtime::ResourceStore;
    use golem_wasm_rpc::{Uri, Value};
    use tokio::runtime::Handle;
    use tokio::time::{timeout, Instant};
    use wasmtime::component::{Instance, ResourceAny};
    use wasmtime::{AsContextMut, ResourceLimiterAsync};

    use golem_common::config::RetryConfig;
    use golem_common::model::oplog::WorkerError;
    use golem_common::model::{
        AccountId, CallingConvention, ComponentId, ComponentVersion, IdempotencyKey, OwnedWorkerId,
        WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord,
    };

    use crate::error::GolemError;
    use crate::model::{
        CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, LookupResult,
        WorkerConfig,
    };
    use crate::services::active_workers::ActiveWorkers;
    use crate::services::blob_store::BlobStoreService;
    use crate::services::events::Events;
    use crate::services::golem_config::GolemConfig;
    use crate::services::invocation_queue::InvocationQueue;
    use crate::services::key_value::KeyValueService;
    use crate::services::oplog::mock::OplogServiceMock;
    use crate::services::oplog::{Oplog, OplogService};
    use crate::services::promise::PromiseService;
    use crate::services::recovery::{RecoveryManagement, RecoveryManagementDefault, TrapType};
    use crate::services::rpc::Rpc;
    use crate::services::scheduler::SchedulerService;
    use crate::services::worker::WorkerService;
    use crate::services::worker_event::WorkerEventService;
    use crate::services::worker_proxy::WorkerProxy;
    use crate::services::{scheduler, HasEvents};
    use crate::services::{
        worker_enumeration, All, HasAll, HasBlobStoreService, HasComponentService, HasConfig,
        HasExtraDeps, HasInvocationQueue, HasKeyValueService, HasOplog, HasPromiseService, HasRpc,
        HasRunningWorkerEnumerationService, HasWasmtimeEngine, HasWorkerActivator,
        HasWorkerEnumerationService, HasWorkerProxy, HasWorkerService,
    };
    use crate::workerctx::{
        ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
        PublicWorkerIo, StatusManagement, UpdateManagement, WorkerCtx,
    };

    struct EmptyContext {
        worker_id: WorkerId,
        public_state: EmptyPublicState,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
    }

    #[derive(Clone)]
    struct EmptyPublicState;

    #[async_trait]
    impl PublicWorkerIo for EmptyPublicState {
        fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
            unimplemented!()
        }
    }

    impl HasInvocationQueue<EmptyContext> for EmptyPublicState {
        fn invocation_queue(&self) -> Arc<InvocationQueue<EmptyContext>> {
            unimplemented!()
        }
    }

    impl HasOplog for EmptyPublicState {
        fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
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
        async fn set_current_idempotency_key(&mut self, _key: IdempotencyKey) {
            unimplemented!()
        }

        async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
            unimplemented!()
        }

        async fn lookup_invocation_result(&self, _key: &IdempotencyKey) -> LookupResult {
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

        async fn update_pending_invocations(&self) {
            unimplemented!()
        }

        async fn update_pending_updates(&self) {
            unimplemented!()
        }

        async fn deactivate(&self) {
            unimplemented!()
        }
    }

    #[async_trait]
    impl InvocationHooks for EmptyContext {
        type FailurePayload = ();

        async fn on_exported_function_invoked(
            &mut self,
            _full_function_name: &str,
            _function_input: &Vec<Value>,
            _calling_convention: Option<CallingConvention>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }

        async fn on_invocation_failure(
            &mut self,
            _trap_type: &TrapType,
        ) -> Result<Self::FailurePayload, Error> {
            unimplemented!()
        }

        async fn on_invocation_failure_deactivated(
            &mut self,
            _payload: &Self::FailurePayload,
            _trap_type: &TrapType,
        ) -> Result<WorkerStatus, Error> {
            unimplemented!()
        }

        async fn on_invocation_failure_final(
            &mut self,
            _payload: &Self::FailurePayload,
            _trap_type: &TrapType,
        ) -> Result<(), Error> {
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
    impl UpdateManagement for EmptyContext {
        fn begin_call_snapshotting_function(&mut self) {
            unimplemented!()
        }

        fn end_call_snapshotting_function(&mut self) {
            unimplemented!()
        }

        async fn on_worker_update_failed(
            &self,
            _target_version: ComponentVersion,
            _details: Option<String>,
        ) {
            unimplemented!()
        }

        async fn on_worker_update_succeeded(&self, _target_version: ComponentVersion) {
            unimplemented!()
        }
    }

    #[async_trait]
    impl ExternalOperations<Self> for EmptyContext {
        type ExtraDeps = ();

        async fn set_worker_status<T: HasAll<Self> + Send + Sync>(
            _this: &T,
            _owned_worker_id: &OwnedWorkerId,
            _status: WorkerStatus,
        ) -> Result<(), GolemError> {
            unimplemented!()
        }

        async fn get_last_error_and_retry_count<T: HasAll<Self> + Send + Sync>(
            _this: &T,
            _owned_worker_id: &OwnedWorkerId,
        ) -> Option<LastError> {
            unimplemented!()
        }

        async fn compute_latest_worker_status<T: HasAll<Self> + Send + Sync>(
            _this: &T,
            _owned_worker_id: &OwnedWorkerId,
            _metadata: &Option<WorkerMetadata>,
        ) -> Result<WorkerStatusRecord, GolemError> {
            unimplemented!()
        }

        async fn prepare_instance(
            _worker_id: &WorkerId,
            _instance: &Instance,
            _store: &mut (impl AsContextMut<Data = Self> + Send),
        ) -> Result<bool, GolemError> {
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
            _owned_worker_id: OwnedWorkerId,
            _promise_service: Arc<dyn PromiseService + Send + Sync>,
            _events: Arc<Events>,
            _worker_service: Arc<dyn WorkerService + Send + Sync>,
            _worker_enumeration_service: Arc<
                dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
            >,
            _key_value_service: Arc<dyn KeyValueService + Send + Sync>,
            _blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
            _event_service: Arc<dyn WorkerEventService + Send + Sync>,
            _active_workers: Arc<ActiveWorkers<Self>>,
            _oplog_service: Arc<dyn OplogService + Send + Sync>,
            _oplog: Arc<dyn Oplog + Send + Sync>,
            _invocation_queue: Arc<InvocationQueue<EmptyContext>>,
            _scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
            _recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
            rpc: Arc<dyn Rpc + Send + Sync>,
            worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
            _extra_deps: Self::ExtraDeps,
            _config: Arc<GolemConfig>,
            _worker_config: WorkerConfig,
            _execution_status: Arc<RwLock<ExecutionStatus>>,
        ) -> Result<Self, GolemError> {
            Ok(EmptyContext {
                worker_id: create_test_id().worker_id,
                public_state: EmptyPublicState,
                rpc,
                worker_proxy,
            })
        }

        fn get_public_state(&self) -> &Self::PublicState {
            &self.public_state
        }

        fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
            self
        }

        fn worker_id(&self) -> &WorkerId {
            &self.worker_id
        }

        fn is_exit(_error: &Error) -> Option<i32> {
            None
        }

        fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
            self.rpc.clone()
        }

        fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
            self.worker_proxy.clone()
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
            unimplemented!()
        }

        fn add(&mut self, _resource: ResourceAny) -> u64 {
            unimplemented!()
        }

        fn get(&mut self, _resource_id: u64) -> Option<ResourceAny> {
            unimplemented!()
        }

        fn borrow(&self, _resource_id: u64) -> Option<ResourceAny> {
            unimplemented!()
        }
    }

    async fn create_recovery_management<F>(
        recovery_fn: F,
    ) -> RecoveryManagementDefault<EmptyContext>
    where
        F: Fn(WorkerId) + Send + Sync + 'static,
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
            deps.component_service(),
            deps.worker_service(),
            deps.worker_enumeration_service(),
            deps.running_worker_enumeration_service(),
            oplog,
            deps.promise_service(),
            scheduler,
            deps.key_value_service(),
            deps.blob_store_service(),
            deps.config(),
            deps.rpc(),
            deps.worker_activator(),
            deps.worker_proxy(),
            deps.events(),
            (),
            recovery_fn,
        )
    }

    fn create_test_id() -> OwnedWorkerId {
        let uuid = uuid::Uuid::parse_str("14e55083-2ff5-44ec-a414-595a748b19a0").unwrap();

        let account_id = AccountId {
            value: "test-account".to_string(),
        };
        let worker_id = WorkerId {
            component_id: ComponentId(uuid),
            worker_name: "test-worker".to_string(),
        };
        OwnedWorkerId {
            account_id,
            worker_id,
        }
    }

    #[tokio::test]
    async fn immediately_recovers_worker_on_startup_with_no_errors() {
        let test_id = create_test_id();
        let start_time = Instant::now();
        let (sender, mut receiver) = tokio::sync::broadcast::channel::<(WorkerId, Duration)>(1);
        let svc = create_recovery_management(move |id| {
            let schedule_time = Instant::now();
            let elapsed = schedule_time.duration_since(start_time);
            sender.send((id, elapsed)).unwrap();
        })
        .await;
        let _ = svc
            .schedule_recovery_on_startup(&test_id, &RetryConfig::default(), &None)
            .await;
        let (id, elapsed) = receiver.recv().await.unwrap();
        assert_eq!(id, test_id.worker_id);
        assert!(elapsed.as_millis() < 100, "elapsed time was {:?}", elapsed);
    }

    #[tokio::test]
    async fn does_not_recover_worker_on_startup_with_many_errors() {
        let test_id = create_test_id();
        let start_time = Instant::now();
        let (sender, mut receiver) = tokio::sync::broadcast::channel::<(WorkerId, Duration)>(1);
        let svc = create_recovery_management(move |id| {
            let schedule_time = Instant::now();
            let elapsed = schedule_time.duration_since(start_time);
            sender.send((id, elapsed)).unwrap();
        })
        .await;
        let _ = svc
            .schedule_recovery_on_startup(
                &test_id,
                &RetryConfig::default(),
                &Some(LastError {
                    error: WorkerError::Unknown("x".to_string()),
                    retry_count: 100,
                }),
            )
            .await;
        let res = timeout(Duration::from_secs(1), receiver.recv()).await;
        assert!(res.is_err());
    }
}
