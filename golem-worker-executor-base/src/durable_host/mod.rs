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

// WASI Host implementation for Golem, delegating to the core WASI implementation (wasmtime_wasi)
// implementing the Golem specific instrumentation on top of it.

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Add;
use std::string::FromUtf8Error;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use crate::error::GolemError;
use crate::invocation::{invoke_worker, InvokeResult};
use crate::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, PersistenceLevel, TrapType,
    WorkerConfig,
};
use crate::services::blob_store::BlobStoreService;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::promise::PromiseService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::{worker_enumeration, HasAll, HasOplog, HasWorker};
use crate::wasi_host::managed_stdio::ManagedStandardIo;
use crate::workerctx::{
    ExternalOperations, IndexedResourceStore, InvocationHooks, InvocationManagement, IoCapturing,
    PublicWorkerIo, StatusManagement, UpdateManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::config::RetryConfig;
use golem_common::model::oplog::{OplogEntry, OplogIndex, UpdateDescription, WrappedFunctionType};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AccountId, CallingConvention, ComponentId, ComponentVersion, FailedUpdateRecord,
    IdempotencyKey, OwnedWorkerId, ScanCursor, ScheduledAction, SuccessfulUpdateRecord,
    WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{Uri, Value};
use tempfile::TempDir;
use tracing::{debug, info, span, warn, Instrument, Level};
use wasmtime::component::{Instance, ResourceAny};
use wasmtime::AsContextMut;
use wasmtime_wasi::{I32Exit, ResourceTable, Stderr, WasiCtx, WasiView};
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::types::{
    default_send_request, HostFutureIncomingResponse, OutgoingRequestConfig,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpView};

use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::wasm_rpc::UriExtensions;
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::services::oplog::{Oplog, OplogOps, OplogService};
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::HasOplogService;
use crate::wasi_host;
use crate::worker::{calculate_last_known_status, is_worker_error_retriable};

pub mod blobstore;
mod cli;
mod clocks;
mod filesystem;
pub mod golem;
mod http;
pub mod io;
pub mod keyvalue;
mod logging;
mod random;
pub mod serialized;
mod sockets;
pub mod wasm_rpc;

mod durability;
use crate::services::component::ComponentMetadata;
use crate::services::worker_proxy::WorkerProxy;
use crate::worker::{RecoveryDecision, Worker};
pub use durability::*;
use golem_common::retries::get_delay;

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: Arc<Mutex<ResourceTable>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi: Arc<Mutex<WasiCtx>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi_http: WasiHttpCtx,
    pub owned_worker_id: OwnedWorkerId,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState,
    #[allow(unused)] // note: need to keep reference to it to keep the temp dir alive
    temp_dir: Arc<TempDir>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn create(
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
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Weak<Worker<Ctx>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
            |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
        )?);
        debug!(
            "Created temporary file system root at {:?}",
            temp_dir.path()
        );

        debug!(
            "Worker {} initialized with deleted regions {}",
            owned_worker_id.worker_id, worker_config.deleted_regions
        );

        let stdio = ManagedStandardIo::new(owned_worker_id.worker_id());
        let stdin = ManagedStdIn::from_standard_io(stdio.clone()).await;
        let stdout = ManagedStdOut::from_standard_io(stdio.clone());
        let stderr = ManagedStdErr::from_stderr(Stderr);

        let last_oplog_index = oplog.current_oplog_index().await;

        wasi_host::create_context(
            &worker_config.args,
            &worker_config.env,
            temp_dir.path().to_path_buf(),
            stdin,
            stdout,
            stderr,
            |duration| anyhow!(SuspendForSleep(duration)),
            config.suspend.suspend_after,
            |wasi, table| {
                let wasi_http = WasiHttpCtx::new();
                DurableWorkerCtx {
                    table: Arc::new(Mutex::new(table)),
                    wasi: Arc::new(Mutex::new(wasi)),
                    wasi_http,
                    owned_worker_id: owned_worker_id.clone(),
                    public_state: PublicDurableWorkerState {
                        promise_service: promise_service.clone(),
                        event_service,
                        managed_stdio: stdio,
                        invocation_queue,
                        oplog: oplog.clone(),
                    },
                    state: PrivateDurableWorkerState::new(
                        oplog_service,
                        oplog,
                        promise_service,
                        scheduler_service,
                        worker_service,
                        worker_enumeration_service,
                        key_value_service,
                        blob_store_service,
                        config.clone(),
                        owned_worker_id.clone(),
                        rpc,
                        worker_proxy,
                        worker_config.deleted_regions.clone(),
                        last_oplog_index,
                        component_metadata,
                    ),
                    temp_dir,
                    execution_status,
                }
            },
        )
        .map_err(|e| GolemError::runtime(format!("Could not create WASI context: {e}")))
    }

    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        Arc::get_mut(&mut self.wasi)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail")
    }

    pub fn worker_id(&self) -> &WorkerId {
        &self.owned_worker_id.worker_id
    }

    pub fn component_metadata(&self) -> &ComponentMetadata {
        &self.state.component_metadata
    }

    pub fn is_exit(error: &anyhow::Error) -> Option<i32> {
        error
            .root_cause()
            .downcast_ref::<I32Exit>()
            .map(|exit| exit.0)
    }

    pub fn as_wasi_view(&mut self) -> DurableWorkerCtxWasiView<Ctx> {
        DurableWorkerCtxWasiView(self)
    }

    pub fn as_wasi_http_view(&mut self) -> DurableWorkerCtxWasiHttpView<Ctx> {
        DurableWorkerCtxWasiHttpView(self)
    }

    pub fn check_interrupt(&self) -> Option<InterruptKind> {
        let execution_status = self.execution_status.read().unwrap().clone();
        match execution_status {
            ExecutionStatus::Interrupting { interrupt_kind, .. } => Some(interrupt_kind),
            _ => None,
        }
    }

    pub fn set_suspended(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { last_known_status } => {
                *execution_status = ExecutionStatus::Suspended { last_known_status };
            }
            ExecutionStatus::Suspended { .. } => {}
            ExecutionStatus::Interrupting {
                await_interruption,
                last_known_status,
                ..
            } => {
                *execution_status = ExecutionStatus::Suspended { last_known_status };
                await_interruption.send(()).ok();
            }
        }
    }

    pub fn set_running(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { .. } => {}
            ExecutionStatus::Suspended { last_known_status } => {
                *execution_status = ExecutionStatus::Running { last_known_status };
            }
            ExecutionStatus::Interrupting { .. } => {}
        }
    }

    pub async fn get_worker_status(&self) -> WorkerStatus {
        match self.state.worker_service.get(&self.owned_worker_id).await {
            Some(metadata) => {
                if metadata.last_known_status.oplog_idx
                    == self.state.oplog.current_oplog_index().await
                {
                    metadata.last_known_status.status
                } else {
                    WorkerStatus::Running
                }
            }
            None => WorkerStatus::Idle,
        }
    }

    pub async fn update_worker_status(&self, f: impl FnOnce(&mut WorkerStatusRecord)) {
        let mut status = self
            .execution_status
            .read()
            .unwrap()
            .last_known_status()
            .clone();

        let mut deleted_regions = self.state.deleted_regions.clone();
        let (pending_updates, extra_deleted_regions) = self.public_state.worker().pending_updates();
        deleted_regions.set_override(extra_deleted_regions);

        status.deleted_regions = deleted_regions;
        status
            .overridden_retry_config
            .clone_from(&self.state.overridden_retry_policy);
        status.pending_invocations = self.public_state.worker().pending_invocations();
        status.invocation_results = self.public_state.worker().invocation_results();
        status.pending_updates = pending_updates;
        status
            .current_idempotency_key
            .clone_from(&self.state.current_idempotency_key);
        status.oplog_idx = self.state.oplog.current_oplog_index().await;
        f(&mut status);
        self.public_state.worker().update_status(status).await;
    }

    pub async fn store_worker_status(&self, status: WorkerStatus) {
        self.update_worker_status(|s| s.status = status.clone())
            .await;
        if status == WorkerStatus::Idle
            || status == WorkerStatus::Failed
            || status == WorkerStatus::Exited
        {
            debug!("Scheduling oplog archive");
            let at = Utc::now().add(self.state.config.oplog.archive_interval);
            self.state
                .scheduler_service
                .schedule(
                    at,
                    ScheduledAction::ArchiveOplog {
                        owned_worker_id: self.owned_worker_id.clone(),
                        last_oplog_index: self.public_state.oplog.current_oplog_index().await,
                        next_after: self.state.config.oplog.archive_interval,
                    },
                )
                .await;
        }
    }

    pub async fn update_pending_invocations(&self) {
        self.update_worker_status(|_| {}).await;
    }

    pub async fn update_pending_updates(&self) {
        self.update_worker_status(|_| {}).await;
    }

    pub fn get_stdio(&self) -> ManagedStandardIo {
        self.public_state.managed_stdio.clone()
    }

    pub async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.state.get_current_idempotency_key()
    }

    pub fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.state.rpc.clone()
    }

    pub fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.state.worker_proxy.clone()
    }

    fn get_recovery_decision_on_trap(
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

    fn calculate_worker_status(
        retry_config: &RetryConfig,
        trap_type: &TrapType,
        previous_tries: u64,
    ) -> WorkerStatus {
        match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt) => WorkerStatus::Interrupted,
            TrapType::Interrupt(InterruptKind::Suspend) => WorkerStatus::Suspended,
            TrapType::Interrupt(InterruptKind::Jump) => WorkerStatus::Running,
            TrapType::Interrupt(InterruptKind::Restart) => WorkerStatus::Running,
            TrapType::Exit => WorkerStatus::Exited,
            TrapType::Error(error) => {
                if is_worker_error_retriable(retry_config, error, previous_tries) {
                    WorkerStatus::Retrying
                } else {
                    WorkerStatus::Failed
                }
            }
        }
    }
}

impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> DurableWorkerCtx<Ctx> {
    /// Records the result of an automatic update, if any was active, and returns whether the worker
    /// should be restarted to retry recovering without the pending update.
    pub async fn finalize_pending_update(
        result: &Result<RecoveryDecision, GolemError>,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> RecoveryDecision {
        let worker_id = store.as_context().data().worker_id().clone();
        let pending_update = store
            .as_context()
            .data()
            .durable_ctx()
            .public_state
            .worker()
            .pop_pending_update();
        match pending_update {
            Some(pending_update) => match result {
                Ok(RecoveryDecision::None) => {
                    if let UpdateDescription::SnapshotBased { .. } = &pending_update.description {
                        let target_version = *pending_update.description.target_version();

                        match store
                            .as_context_mut()
                            .data_mut()
                            .get_public_state()
                            .oplog()
                            .get_upload_description_payload(&pending_update.description)
                            .await
                        {
                            Ok(Some(data)) => {
                                let idempotency_key = IdempotencyKey::fresh();
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .durable_ctx_mut()
                                    .set_current_idempotency_key(idempotency_key.clone())
                                    .await;

                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .begin_call_snapshotting_function();
                                let load_result = invoke_worker(
                                    "golem:api/load-snapshot@0.2.0.{load}".to_string(),
                                    vec![Value::List(data.iter().map(|b| Value::U8(*b)).collect())],
                                    store,
                                    instance,
                                    CallingConvention::Component,
                                    true,
                                )
                                .await;
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .end_call_snapshotting_function();

                                let failed = match load_result {
                                    Err(error) => Some(format!(
                                        "Manual update failed to load snapshot: {error}"
                                    )),
                                    Ok(InvokeResult::Failed { error, .. }) => Some(format!(
                                        "Manual update failed to load snapshot: {error}"
                                    )),
                                    Ok(InvokeResult::Succeeded { output, .. }) => {
                                        if output.len() == 1 {
                                            match &output[0] {
                                                    Value::Result(Err(Some(boxed_error_value))) => {
                                                        match &**boxed_error_value {
                                                            Value::String(error) =>
                                                                Some(format!("Manual update failed to load snapshot: {error}")),
                                                            _ =>
                                                                Some("Unexpected result value from the snapshot load function".to_string())
                                                        }
                                                    }
                                                    _ => None
                                                }
                                        } else {
                                            Some("Unexpected result value from the snapshot load function".to_string())
                                        }
                                    }
                                    _ => None,
                                };

                                if let Some(error) = failed {
                                    store
                                        .as_context_mut()
                                        .data_mut()
                                        .on_worker_update_failed(target_version, Some(error))
                                        .await;
                                    RecoveryDecision::Immediate
                                } else {
                                    let component_metadata =
                                        store.as_context().data().component_metadata().clone();
                                    store
                                        .as_context_mut()
                                        .data_mut()
                                        .on_worker_update_succeeded(
                                            target_version,
                                            component_metadata.size,
                                        )
                                        .await;
                                    RecoveryDecision::None
                                }
                            }
                            Ok(None) => {
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_worker_update_failed(
                                        target_version,
                                        Some("Failed to find snapshot data for update".to_string()),
                                    )
                                    .await;
                                RecoveryDecision::Immediate
                            }
                            Err(error) => {
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_worker_update_failed(target_version, Some(error))
                                    .await;
                                RecoveryDecision::Immediate
                            }
                        }
                    } else {
                        // Automatic update succeeded
                        let target_version = *pending_update.description.target_version();
                        let component_metadata =
                            store.as_context().data().component_metadata().clone();
                        store
                            .as_context_mut()
                            .data_mut()
                            .on_worker_update_succeeded(target_version, component_metadata.size)
                            .await;
                        RecoveryDecision::None
                    }
                }
                Ok(_) => {
                    // TODO: we loose knowledge of the error here
                    // Failure that triggered a retry
                    let target_version = *pending_update.description.target_version();

                    store
                        .as_context_mut()
                        .data_mut()
                        .on_worker_update_failed(
                            target_version,
                            Some("Automatic update failed".to_string()),
                        )
                        .await;
                    RecoveryDecision::Immediate
                }
                Err(error) => {
                    let target_version = *pending_update.description.target_version();

                    store
                        .as_context_mut()
                        .data_mut()
                        .on_worker_update_failed(
                            target_version,
                            Some(format!("Automatic update failed: {error}")),
                        )
                        .await;
                    RecoveryDecision::Immediate
                }
            },
            None => {
                debug!("No pending updates to finalize for {}", worker_id);
                RecoveryDecision::None
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationManagement for DurableWorkerCtx<Ctx> {
    async fn set_current_idempotency_key(&mut self, key: IdempotencyKey) {
        self.state.set_current_idempotency_key(key)
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.get_current_idempotency_key().await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> IoCapturing for DurableWorkerCtx<Ctx> {
    async fn start_capturing_stdout(&mut self, provided_stdin: String) {
        self.public_state
            .managed_stdio
            .start_single_stdio_call(provided_stdin)
            .await
    }

    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error> {
        self.public_state
            .managed_stdio
            .finish_single_stdio_call()
            .await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> StatusManagement for DurableWorkerCtx<Ctx> {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        self.check_interrupt()
    }

    fn set_suspended(&self) {
        self.set_suspended()
    }

    fn set_running(&self) {
        self.set_running()
    }

    async fn get_worker_status(&self) -> WorkerStatus {
        self.get_worker_status().await
    }

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.store_worker_status(status).await
    }

    async fn update_pending_invocations(&self) {
        self.update_pending_invocations().await
    }

    async fn update_pending_updates(&self) {
        self.update_pending_invocations().await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        calling_convention: Option<CallingConvention>,
    ) -> Result<(), GolemError> {
        if self.state.snapshotting_mode.is_none() {
            let proto_function_input: Vec<golem_wasm_rpc::protobuf::Val> = function_input
                .iter()
                .map(|value| value.clone().into())
                .collect();

            self.state
                .oplog
                .add_exported_function_invoked(
                    full_function_name.to_string(),
                    &proto_function_input,
                    self.get_current_idempotency_key().await.ok_or(anyhow!(
                        "No active invocation key is associated with the worker"
                    ))?,
                    calling_convention,
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "could not encode function input for {full_function_name} on {}: {err}",
                        self.worker_id()
                    )
                });
            self.state.oplog.commit().await;
        }
        Ok(())
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> RecoveryDecision {
        let needs_commit = match trap_type {
            TrapType::Error(error) => Some((OplogEntry::error(error.clone()), true)),
            TrapType::Interrupt(InterruptKind::Interrupt) => {
                Some((OplogEntry::interrupted(), true))
            }
            TrapType::Interrupt(InterruptKind::Suspend) => Some((OplogEntry::suspend(), false)),
            TrapType::Exit => Some((OplogEntry::exited(), true)),
            _ => None,
        };

        let oplog_idx = if let Some((entry, store)) = needs_commit {
            let oplog_idx = self.state.oplog.add_and_commit(entry).await;

            if store {
                Some(oplog_idx)
            } else {
                None
            }
        } else {
            None
        };

        let previous_tries = self.state.trailing_error_count().await;
        let default_retry_config = &self.state.config.retry;
        let retry_config = self
            .state
            .overridden_retry_policy
            .as_ref()
            .unwrap_or(default_retry_config)
            .clone();
        let decision =
            Self::get_recovery_decision_on_trap(&retry_config, previous_tries, trap_type);

        debug!(
            "Recovery decision after {} tries: {:?}",
            previous_tries, decision
        );

        let updated_worker_status =
            Self::calculate_worker_status(&retry_config, trap_type, previous_tries);

        self.store_worker_status(updated_worker_status.clone())
            .await;

        if updated_worker_status != WorkerStatus::Retrying
            && updated_worker_status != WorkerStatus::Running
        {
            // Giving up, associating the stored result with the current and upcoming invocations
            if let Some(oplog_idx) = oplog_idx {
                if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                    self.public_state
                        .worker()
                        .store_invocation_failure(&idempotency_key, trap_type, oplog_idx)
                        .await;
                }
            }
        }

        decision
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<(), GolemError> {
        let is_live_after = self.state.is_live();

        if is_live_after {
            if self.state.snapshotting_mode.is_none() {
                let proto_output: Vec<golem_wasm_rpc::protobuf::Val> =
                    output.iter().map(|value| value.clone().into()).collect();

                self.state
                    .oplog
                    .add_exported_function_completed(&proto_output, consumed_fuel)
                    .await
                    .unwrap_or_else(|err| {
                        panic!("could not encode function result for {full_function_name}: {err}")
                    });
                self.state.oplog.commit().await;
                let oplog_idx = self.state.oplog.current_oplog_index().await;

                if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                    self.public_state
                        .worker()
                        .store_invocation_success(&idempotency_key, output.clone(), oplog_idx)
                        .await;
                }
            }
        } else {
            let response = self
                .state
                .get_oplog_entry_exported_function_completed()
                .await?;

            if let Some(function_output) = response {
                let is_diverged = function_output != output;
                if is_diverged {
                    return Err(GolemError::unexpected_oplog_entry(
                        format!("{full_function_name}({function_input:?}) => {function_output:?}"),
                        format!("{full_function_name}({function_input:?}) => {output:?}"),
                    ));
                }
            }
        }

        self.store_worker_status(WorkerStatus::Idle).await;

        debug!("Function {full_function_name} finished with {output:?}");

        // Return indicating that it is done
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> ResourceStore for DurableWorkerCtx<Ctx> {
    fn self_uri(&self) -> Uri {
        self.state.self_uri()
    }

    fn add(&mut self, resource: ResourceAny) -> u64 {
        self.state.add(resource)
    }

    fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        self.state.borrow(resource_id)
    }

    fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.state.borrow(resource_id)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> UpdateManagement for DurableWorkerCtx<Ctx> {
    fn begin_call_snapshotting_function(&mut self) {
        // While calling a snapshotting function (load/save), we completely turn off persistence
        // In addition to the user-controllable persistence level we also skip writing the
        // oplog entries marking the exported function call.
        let previous_level = self.state.persistence_level.clone();
        self.state.snapshotting_mode = Some(previous_level);
        self.state.persistence_level = PersistenceLevel::PersistNothing;
    }

    fn end_call_snapshotting_function(&mut self) {
        // Restoring the state of persistence after calling a snapshotting function
        self.state.persistence_level = self
            .state
            .snapshotting_mode
            .take()
            .expect("Not in snapshotting mode");
    }

    async fn on_worker_update_failed(
        &self,
        target_version: ComponentVersion,
        details: Option<String>,
    ) {
        let entry = OplogEntry::failed_update(target_version, details.clone());
        let timestamp = entry.timestamp();
        self.public_state.oplog.add_and_commit(entry).await;
        self.update_worker_status(|status| {
            status.failed_updates.push(FailedUpdateRecord {
                timestamp,
                target_version,
                details: details.clone(),
            })
        })
        .await;

        warn!(
            "Worker failed to update to {}: {}, update attempt aborted",
            target_version,
            details.unwrap_or_else(|| "?".to_string())
        );
    }

    async fn on_worker_update_succeeded(
        &self,
        target_version: ComponentVersion,
        new_component_size: u64,
    ) {
        info!("Worker update to {} finished successfully", target_version);

        let entry = OplogEntry::successful_update(target_version, new_component_size);
        let timestamp = entry.timestamp();
        self.public_state.oplog.add_and_commit(entry).await;
        self.update_worker_status(|status| {
            status.component_version = target_version;
            status.successful_updates.push(SuccessfulUpdateRecord {
                timestamp,
                target_version,
            })
        })
        .await;
    }
}

impl<Ctx: WorkerCtx> IndexedResourceStore for DurableWorkerCtx<Ctx> {
    fn get_indexed_resource(&self, resource_name: &str, resource_params: &[String]) -> Option<u64> {
        let key = IndexedResourceKey {
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.get(&key).copied()
    }

    fn store_indexed_resource(
        &mut self,
        resource_name: &str,
        resource_params: &[String],
        resource: u64,
    ) {
        let key = IndexedResourceKey {
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.insert(key, resource);
    }

    fn drop_indexed_resource(&mut self, resource_name: &str, resource_params: &[String]) {
        let key = IndexedResourceKey {
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.remove(&key);
    }
}

pub trait DurableWorkerCtxView<Ctx: WorkerCtx> {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Ctx>;
    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Ctx>;
}

#[async_trait]
impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> ExternalOperations<Ctx> for DurableWorkerCtx<Ctx> {
    type ExtraDeps = Ctx::ExtraDeps;

    async fn get_last_error_and_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
    ) -> Option<LastError> {
        last_error_and_retry_count(this, owned_worker_id).await
    }

    async fn compute_latest_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        calculate_last_known_status(this, owned_worker_id, metadata).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<RecoveryDecision, GolemError> {
        debug!("Starting prepare_instance");
        let start = Instant::now();
        let mut count = 0;

        // Handle the case when recovery immediately starts in a deleted region
        // (for example due to a manual update)
        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .state
            .get_out_of_deleted_region();

        let result = loop {
            let cont = store.as_context().data().durable_ctx().state.is_replay();

            if cont {
                let oplog_entry = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .get_oplog_entry_exported_function_invoked()
                    .await;
                match oplog_entry {
                    Err(error) => break Err(error),
                    Ok(None) => break Ok(RecoveryDecision::None),
                    Ok(Some((
                        function_name,
                        function_input,
                        idempotency_key,
                        calling_convention,
                    ))) => {
                        debug!("Replaying function {function_name}");
                        let span = span!(Level::INFO, "replaying", function = function_name);
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_idempotency_key(idempotency_key)
                            .await;

                        let full_function_name = function_name.to_string();
                        let invoke_result = invoke_worker(
                            full_function_name.clone(),
                            function_input.clone(),
                            store,
                            instance,
                            calling_convention.unwrap_or(CallingConvention::Component),
                            false, // we know it was not live before, because cont=true
                        )
                        .instrument(span)
                        .await;

                        match invoke_result {
                            Ok(InvokeResult::Succeeded {
                                output,
                                consumed_fuel,
                            }) => {
                                if let Err(err) = store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_invocation_success(
                                        &full_function_name,
                                        &function_input,
                                        consumed_fuel,
                                        output,
                                    )
                                    .await
                                {
                                    break Err(err);
                                }
                                count += 1;
                                continue;
                            }
                            _ => {
                                let trap_type = match invoke_result {
                                    Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
                                    Err(error) => {
                                        Some(TrapType::from_error::<Ctx>(&anyhow!(error)))
                                    }
                                };
                                let decision = match trap_type {
                                    Some(trap_type) => {
                                        let decision = store
                                            .as_context_mut()
                                            .data_mut()
                                            .on_invocation_failure(&trap_type)
                                            .await;

                                        if decision == RecoveryDecision::None {
                                            // Cannot retry so we need to fail
                                            match trap_type {
                                                TrapType::Interrupt(interrupt_kind) => {
                                                    if interrupt_kind == InterruptKind::Interrupt {
                                                        break Err(GolemError::runtime(
                                                            "Interrupted via the Golem API",
                                                        ));
                                                    } else {
                                                        break Err(GolemError::runtime("The worker could not finish replaying a function {function_name}"));
                                                    }
                                                }
                                                TrapType::Exit => {
                                                    break Err(GolemError::runtime(
                                                        "Process exited",
                                                    ))
                                                }
                                                TrapType::Error(error) => {
                                                    break Err(GolemError::runtime(
                                                        error.to_string(),
                                                    ))
                                                }
                                            }
                                        }

                                        decision
                                    }
                                    None => RecoveryDecision::None,
                                };

                                break Ok(decision);
                            }
                        }
                    }
                }
            } else {
                break Ok(RecoveryDecision::None);
            }
        };
        record_resume_worker(start.elapsed());
        record_number_of_replayed_functions(count);

        let final_decision = Self::finalize_pending_update(&result, instance, store).await;

        // The update finalization has the right to override the Err result with an explicit retry request
        if final_decision != RecoveryDecision::None {
            debug!("Retrying prepare_instance after failed update attempt");
            Ok(final_decision)
        } else {
            debug!("Finished prepare_instance");
            result.map_err(|err| GolemError::failed_to_resume_worker(worker_id.clone(), err))
        }
    }

    async fn record_last_known_limits<T: HasAll<Ctx> + Send + Sync>(
        _this: &T,
        _account_id: &AccountId,
        _last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn on_worker_deleted<T: HasAll<Ctx> + Send + Sync>(
        _this: &T,
        _worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), anyhow::Error> {
        info!("Recovering workers");

        let workers = this.worker_service().get_running_workers_in_shards().await;

        debug!("Recovering running workers: {:?}", workers);

        let default_retry_config = &this.config().retry;
        for worker in workers {
            let owned_worker_id = worker.owned_worker_id();
            let actualized_metadata =
                calculate_last_known_status(this, &owned_worker_id, &Some(worker)).await?;
            let last_error = Self::get_last_error_and_retry_count(this, &owned_worker_id).await;
            let decision = Self::get_recovery_decision_on_startup(
                actualized_metadata
                    .overridden_retry_config
                    .as_ref()
                    .unwrap_or(default_retry_config),
                &last_error,
            );
            if let Some(last_error) = last_error {
                debug!("Recovery decision after {last_error}: {decision:?}");
            }

            match decision {
                RecoveryDecision::Immediate => {
                    let _ = Worker::get_or_create_running(
                        this,
                        &owned_worker_id,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
                }
                RecoveryDecision::Delayed(_) => {
                    panic!("Delayed recovery on startup is not supported currently")
                }
                RecoveryDecision::None => {}
            }
        }

        info!("Finished recovering workers");
        Ok(())
    }
}

async fn last_error_and_retry_count<T: HasOplogService>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
) -> Option<LastError> {
    let mut idx = this.oplog_service().get_last_index(owned_worker_id).await;
    let mut retry_count = 0;
    if idx == OplogIndex::NONE {
        None
    } else {
        let mut first_error = None;
        let result = loop {
            let oplog_entry = this.oplog_service().read(owned_worker_id, idx, 1).await;
            match oplog_entry.first_key_value()
                .unwrap_or_else(|| panic!("Internal error: op log for {} has size greater than zero but no entry at last index", owned_worker_id.worker_id)) {
                (_, OplogEntry::Error { error, .. } )=> {
                    retry_count += 1;
                    if first_error.is_none() {
                        first_error = Some(error.clone());
                    }
                    if idx > OplogIndex::INITIAL {
                        idx = idx.previous();
                        continue;
                    } else {
                        break Some(
                            LastError {
                                error: first_error.unwrap(),
                                retry_count
                            }
                        );
                    }
                }
                _ => {
                    match first_error {
                        Some(error) => break Some(LastError { error, retry_count }),
                        None => break None
                    }
                }
            }
        };
        result
    }
}

pub struct PrivateDurableWorkerState {
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    oplog: Arc<dyn Oplog + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    key_value_service: Arc<dyn KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
    config: Arc<GolemConfig>,
    owned_worker_id: OwnedWorkerId,
    current_idempotency_key: Option<IdempotencyKey>,
    rpc: Arc<dyn Rpc + Send + Sync>,
    worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
    resources: HashMap<u64, ResourceAny>,
    last_resource_id: u64,
    deleted_regions: DeletedRegions,
    next_deleted_region: Option<OplogRegion>,
    overridden_retry_policy: Option<RetryConfig>,
    persistence_level: PersistenceLevel,
    assume_idempotence: bool,
    open_function_table: HashMap<u32, OplogIndex>,
    replay_target: OplogIndex,

    /// The oplog index of the last replayed entry
    last_replayed_index: OplogIndex,
    snapshotting_mode: Option<PersistenceLevel>,

    indexed_resources: HashMap<IndexedResourceKey, u64>,
    component_metadata: ComponentMetadata,
}

impl PrivateDurableWorkerState {
    pub fn new(
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        config: Arc<GolemConfig>,
        owned_worker_id: OwnedWorkerId,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        deleted_regions: DeletedRegions,
        last_oplog_index: OplogIndex,
        component_metadata: ComponentMetadata,
    ) -> Self {
        let mut result = Self {
            oplog_service,
            oplog,
            promise_service,
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            config,
            owned_worker_id,
            current_idempotency_key: None,
            rpc,
            worker_proxy,
            resources: HashMap::new(),
            last_resource_id: 0,
            deleted_regions: deleted_regions.clone(),
            next_deleted_region: deleted_regions.find_next_deleted_region(OplogIndex::NONE),
            overridden_retry_policy: None,
            persistence_level: PersistenceLevel::Smart,
            assume_idempotence: true,
            open_function_table: HashMap::new(),
            last_replayed_index: OplogIndex::NONE,
            replay_target: last_oplog_index,
            snapshotting_mode: None,
            indexed_resources: HashMap::new(),
            component_metadata,
        };
        result.move_replay_idx(OplogIndex::INITIAL); // By this we handle initial deleted regions applied by manual updates correctly
        result
    }

    pub async fn begin_function(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
    ) -> Result<OplogIndex, GolemError> {
        if !self.assume_idempotence
            && *wrapped_function_type == WrappedFunctionType::WriteRemote
            && self.persistence_level != PersistenceLevel::PersistNothing
        {
            if self.is_live() {
                self.oplog
                    .add_and_commit(OplogEntry::begin_remote_write())
                    .await;
                let begin_index = self.oplog.current_oplog_index().await;
                Ok(begin_index)
            } else {
                let (begin_index, _) = crate::get_oplog_entry!(self, OplogEntry::BeginRemoteWrite)?;
                let end_index = self
                    .lookup_oplog_entry(begin_index, OplogEntry::is_end_remote_write)
                    .await;
                if end_index.is_none() {
                    // Must switch to live mode before failing to be able to commit an Error entry
                    self.last_replayed_index = self.replay_target;
                    debug!("[4] REPLAY_IDX = {}", self.last_replayed_index);
                    Err(GolemError::runtime(
                        "Non-idempotent remote write operation was not completed, cannot retry",
                    ))
                } else {
                    Ok(begin_index)
                }
            }
        } else {
            let begin_index = self.oplog.current_oplog_index().await;
            Ok(begin_index)
        }
    }

    pub async fn end_function(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
        begin_index: OplogIndex,
    ) -> Result<(), GolemError> {
        if !self.assume_idempotence
            && *wrapped_function_type == WrappedFunctionType::WriteRemote
            && self.persistence_level != PersistenceLevel::PersistNothing
        {
            if self.is_live() {
                self.oplog
                    .add(OplogEntry::end_remote_write(begin_index))
                    .await;
                Ok(())
            } else {
                let (_, _) = crate::get_oplog_entry!(self, OplogEntry::EndRemoteWrite)?;
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    /// In live mode it returns the last oplog index (index of the entry last added).
    /// In replay mode it returns the current replay index (index of the entry last read).
    pub async fn current_oplog_index(&self) -> OplogIndex {
        if self.is_live() {
            self.oplog.current_oplog_index().await
        } else {
            self.last_replayed_index
        }
    }

    async fn read_oplog(&self, idx: OplogIndex, n: u64) -> Vec<OplogEntry> {
        self.oplog_service
            .read(&self.owned_worker_id, idx, n)
            .await
            .into_values()
            .collect()
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.last_replayed_index == self.replay_target
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    fn get_out_of_deleted_region(&mut self) {
        if self.is_replay() {
            let update_next_deleted_region = match &self.next_deleted_region {
                Some(region) if region.start == (self.last_replayed_index.next()) => {
                    let target = region.end.next(); // we want to continue reading _after_ the region
                    debug!(
                        "Worker {} reached deleted region at {}, jumping to {} (oplog size: {})",
                        self.owned_worker_id.worker_id, region.start, target, self.replay_target
                    );
                    self.last_replayed_index = target.previous(); // so we set the last replayed index to the end of the region

                    true
                }
                _ => false,
            };

            if update_next_deleted_region {
                self.next_deleted_region = self
                    .deleted_regions
                    .find_next_deleted_region(self.last_replayed_index);
            }
        }
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    async fn get_oplog_entry(&mut self) -> (OplogIndex, OplogEntry) {
        let read_idx = self.last_replayed_index.next();
        let entry = self.internal_get_next_oplog_entry().await;

        // Skipping hint entries
        while self.is_replay() {
            let saved_replay_idx = self.last_replayed_index;
            let saved_next_deleted_region = self.next_deleted_region.clone();
            let entry = self.internal_get_next_oplog_entry().await;
            if !entry.is_hint() {
                // TODO: cache the last hint entry to avoid reading it again
                self.last_replayed_index = saved_replay_idx;
                self.next_deleted_region = saved_next_deleted_region;
                break;
            }
        }

        (read_idx, entry)
    }

    /// Gets the next oplog entry, no matter if it is hint or not, following jumps
    async fn internal_get_next_oplog_entry(&mut self) -> OplogEntry {
        assert!(self.is_replay());

        let read_idx = self.last_replayed_index.next();

        let oplog_entries = self.read_oplog(read_idx, 1).await;
        let oplog_entry = oplog_entries.into_iter().next().unwrap();
        self.move_replay_idx(read_idx);

        oplog_entry
    }

    fn move_replay_idx(&mut self, new_idx: OplogIndex) {
        self.last_replayed_index = new_idx;
        self.get_out_of_deleted_region();
    }

    async fn lookup_oplog_entry(
        &mut self,
        begin_idx: OplogIndex,
        check: impl Fn(&OplogEntry, OplogIndex) -> bool,
    ) -> Option<OplogIndex> {
        let mut start = self.last_replayed_index.next();
        const CHUNK_SIZE: u64 = 1024;
        while start < self.replay_target {
            let entries = self
                .oplog_service
                .read(&self.owned_worker_id, start, CHUNK_SIZE)
                .await;
            for (idx, entry) in &entries {
                // TODO: handle deleted regions
                if check(entry, begin_idx) {
                    return Some(*idx);
                }
            }
            start = start.range_end(entries.len() as u64).next();
        }

        None
    }

    async fn get_oplog_entry_exported_function_invoked(
        &mut self,
    ) -> Result<
        Option<(
            String,
            Vec<Value>,
            IdempotencyKey,
            Option<CallingConvention>,
        )>,
        GolemError,
    > {
        loop {
            if self.is_replay() {
                let (_, oplog_entry) = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionInvoked {
                        function_name,
                        idempotency_key,
                        calling_convention,
                        ..
                    } => {
                        let request: Vec<golem_wasm_rpc::protobuf::Val> = self
                            .oplog
                            .get_payload_of_entry(&oplog_entry)
                            .await
                            .expect("failed to deserialize function request payload")
                            .unwrap();
                        let request = request
                            .into_iter()
                            .map(|val| {
                                val.try_into()
                                    .expect("failed to decode serialized protobuf value")
                            })
                            .collect::<Vec<Value>>();
                        break Ok(Some((
                            function_name.to_string(),
                            request,
                            idempotency_key.clone(),
                            *calling_convention,
                        )));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(GolemError::unexpected_oplog_entry(
                            "ExportedFunctionInvoked",
                            format!("{:?}", oplog_entry),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    async fn get_oplog_entry_exported_function_completed(
        &mut self,
    ) -> Result<Option<Vec<Value>>, GolemError> {
        loop {
            if self.is_replay() {
                let (_, oplog_entry) = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionCompleted { .. } => {
                        let response: Vec<golem_wasm_rpc::protobuf::Val> = self
                            .oplog
                            .get_payload_of_entry(&oplog_entry)
                            .await
                            .expect("failed to deserialize function response payload")
                            .unwrap();
                        let response = response
                            .into_iter()
                            .map(|val| {
                                val.try_into()
                                    .expect("failed to decode serialized protobuf value")
                            })
                            .collect();
                        break Ok(Some(response));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(GolemError::unexpected_oplog_entry(
                            "ExportedFunctionCompleted",
                            format!("{:?}", oplog_entry),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    pub async fn sleep_until(&self, when: DateTime<Utc>) -> Result<(), GolemError> {
        let promise_id = self
            .promise_service
            .create(
                &self.owned_worker_id.worker_id,
                self.current_oplog_index().await,
            )
            .await;

        let schedule_id = self
            .scheduler_service
            .schedule(
                when,
                ScheduledAction::CompletePromise {
                    account_id: self.owned_worker_id.account_id(),
                    promise_id,
                },
            )
            .await;
        debug!(
            "Schedule added to awake suspended worker at {} with id {}",
            when.to_rfc3339(),
            schedule_id
        );

        Ok(())
    }

    pub fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.current_idempotency_key.clone()
    }

    pub fn set_current_idempotency_key(&mut self, invocation_key: IdempotencyKey) {
        self.current_idempotency_key = Some(invocation_key);
    }

    /// Counts the number of Error entries that are at the end of the oplog. This equals to the number of retries that have been attempted.
    /// It also returns the last error stored in these entries.
    pub async fn trailing_error_count(&self) -> u64 {
        last_error_and_retry_count(self, &self.owned_worker_id)
            .await
            .map(|last_error| last_error.retry_count)
            .unwrap_or_default()
    }

    pub async fn get_workers(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GolemError> {
        self.worker_enumeration_service
            .get(
                &self.owned_worker_id.account_id,
                component_id,
                filter,
                cursor,
                count,
                precise,
            )
            .await
    }
}

#[async_trait]
impl ResourceStore for PrivateDurableWorkerState {
    fn self_uri(&self) -> Uri {
        Uri::golem_uri(&self.owned_worker_id.worker_id, None)
    }

    fn add(&mut self, resource: ResourceAny) -> u64 {
        let id = self.last_resource_id;
        self.last_resource_id += 1;
        self.resources.insert(id, resource);
        id
    }

    fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        self.resources.remove(&resource_id)
    }

    fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.resources.get(&resource_id).cloned()
    }
}

impl HasOplogService for PrivateDurableWorkerState {
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl HasOplog for PrivateDurableWorkerState {
    fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
        self.oplog.clone()
    }
}

pub struct PublicDurableWorkerState<Ctx: WorkerCtx> {
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
    managed_stdio: ManagedStandardIo,
    invocation_queue: Weak<Worker<Ctx>>,
    oplog: Arc<dyn Oplog + Send + Sync>,
}

impl<Ctx: WorkerCtx> Clone for PublicDurableWorkerState<Ctx> {
    fn clone(&self) -> Self {
        Self {
            promise_service: self.promise_service.clone(),
            event_service: self.event_service.clone(),
            managed_stdio: self.managed_stdio.clone(),
            invocation_queue: self.invocation_queue.clone(),
            oplog: self.oplog.clone(),
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> PublicWorkerIo for PublicDurableWorkerState<Ctx> {
    fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.event_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorker<Ctx> for PublicDurableWorkerState<Ctx> {
    fn worker(&self) -> Arc<Worker<Ctx>> {
        // NOTE: We store the back-reference as a weak reference here to avoid a reference cycle,
        // but this should always work as the wasmtime store holding the DurableWorkerCtx is owned
        // by the InvocationQueue's run loop.
        self.invocation_queue
            .upgrade()
            .expect("InvocationQueue dropped")
    }
}

impl<Ctx: WorkerCtx> HasOplog for PublicDurableWorkerState<Ctx> {
    fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
        self.oplog.clone()
    }
}

pub struct DurableWorkerCtxWasiView<'a, Ctx: WorkerCtx>(&'a mut DurableWorkerCtx<Ctx>);

pub struct DurableWorkerCtxWasiHttpView<'a, Ctx: WorkerCtx>(&'a mut DurableWorkerCtx<Ctx>);

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
pub struct SuspendForSleep(Duration);

impl Display for SuspendForSleep {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Suspended for sleep {} ms", self.0.as_millis())
    }
}

impl Error for SuspendForSleep {}

// This wrapper forces the compiler to choose the wasmtime_wasi implementations for T: WasiView
impl<'a, Ctx: WorkerCtx> WasiView for DurableWorkerCtxWasiView<'a, Ctx> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        self.0.ctx()
    }
}

impl<'a, Ctx: WorkerCtx> WasiHttpView for DurableWorkerCtxWasiHttpView<'a, Ctx> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.0.wasi_http
    }

    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.0.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse>
    where
        Self: Sized,
    {
        if self.0.state.is_replay() {
            // If this is a replay, we must not actually send the request, but we have to store it in the
            // FutureIncomingResponse because it is possible that there wasn't any response recorded in the oplog.
            // If that is the case, the request has to be sent as soon as we get into live mode and trying to await
            // or poll the response future.
            Ok(HostFutureIncomingResponse::deferred(request, config))
        } else {
            Ok(default_send_request(request, config))
        }
    }
}

/// Helper macro for expecting a given type of OplogEntry as the next entry in the oplog during
/// replay, while skipping hint entries.
/// The macro expression's type is `Result<OplogEntry, GolemError>` and it fails if the next non-hint
/// entry was not the expected one.
#[macro_export]
macro_rules! get_oplog_entry {
    ($private_state:expr, $case:path) => {
        loop {
            let (oplog_index, oplog_entry) = $private_state.get_oplog_entry().await;
            match oplog_entry {
                $case { .. } => {
                    break Ok((oplog_index, oplog_entry));
                }
                entry if entry.is_hint() => {}
                _ => {
                    break Err($crate::error::GolemError::unexpected_oplog_entry(
                        stringify!($case),
                        format!("{:?}", oplog_entry),
                    ));
                }
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IndexedResourceKey {
    pub resource_name: String,
    pub resource_params: Vec<String>,
}
