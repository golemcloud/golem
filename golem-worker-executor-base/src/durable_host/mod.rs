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
use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::error::GolemError;
use crate::invocation::invoke_worker;
use crate::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, PersistenceLevel, TrapType,
    WorkerConfig,
};
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::golem_config::GolemConfig;
use crate::services::invocation_key::{InvocationKeyService, LookupResult};
use crate::services::key_value::KeyValueService;
use crate::services::promise::PromiseService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::{worker_enumeration, HasAll, HasInvocationQueue, HasOplog};
use crate::wasi_host::managed_stdio::ManagedStandardIo;
use crate::workerctx::{
    ExternalOperations, InvocationHooks, InvocationManagement, IoCapturing, PublicWorkerIo,
    StatusManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use cap_std::ambient_authority;
use golem_common::config::RetryConfig;
use golem_common::model::oplog::{OplogEntry, WrappedFunctionType};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AccountId, CallingConvention, ComponentId, FailedUpdateRecord, InvocationKey, SuccessfulUpdateRecord,
    WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{Uri, Value};
use tempfile::TempDir;
use tracing::{debug, info, warn};
use wasmtime::component::{Instance, Resource, ResourceAny};
use wasmtime::AsContextMut;
use wasmtime_wasi::preview2::{I32Exit, ResourceTable, Stderr, Subscribe, WasiCtx, WasiView};
use wasmtime_wasi_http::types::{
    default_send_request, HostFutureIncomingResponse, OutgoingRequest,
};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::wasm_rpc::UriExtensions;
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::services::oplog::{Oplog, OplogService};
use crate::services::recovery::RecoveryManagement;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::HasOplogService;
use crate::wasi_host;
use crate::worker::{calculate_last_known_status, calculate_worker_status};

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
use crate::services::invocation_queue::InvocationQueue;
pub use durability::*;

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: ResourceTable,
    wasi: WasiCtx,
    wasi_http: WasiHttpCtx,
    pub worker_id: WorkerId,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState<Ctx>,
    #[allow(unused)] // note: need to keep reference to it to keep the temp dir alive
    temp_dir: Arc<TempDir>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn create(
        worker_id: WorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<Ctx>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Arc<InvocationQueue<Ctx>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
            |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
        )?);
        debug!(
            "Created temporary file system root for {worker_id} at {:?}",
            temp_dir.path()
        );
        let root_dir = cap_std::fs::Dir::open_ambient_dir(temp_dir.path(), ambient_authority())
            .map_err(|e| GolemError::runtime(format!("Failed to open temporary directory: {e}")))?;

        debug!(
            "Worker {} initialized with deleted regions {}",
            worker_id, worker_config.deleted_regions
        );

        let stdio = ManagedStandardIo::new(worker_id.clone(), invocation_key_service.clone());
        let stdin = ManagedStdIn::from_standard_io(stdio.clone()).await;
        let stdout = ManagedStdOut::from_standard_io(stdio.clone());
        let stderr = ManagedStdErr::from_stderr(Stderr);

        let oplog_size = oplog.current_oplog_index().await;

        wasi_host::create_context(
            &worker_config.args,
            &worker_config.env,
            root_dir,
            temp_dir.path().to_path_buf(),
            stdin,
            stdout,
            stderr,
            |duration| anyhow!(SuspendForSleep(duration)),
            config.suspend.suspend_after,
            |wasi, table| {
                let wasi_http = WasiHttpCtx;
                DurableWorkerCtx {
                    table,
                    wasi,
                    wasi_http,
                    worker_id: worker_id.clone(),
                    public_state: PublicDurableWorkerState {
                        promise_service: promise_service.clone(),
                        event_service,
                        managed_stdio: stdio,
                        invocation_queue,
                        oplog: oplog.clone(),
                    },
                    state: PrivateDurableWorkerState {
                        oplog_service,
                        oplog,
                        promise_service,
                        scheduler_service,
                        worker_service,
                        worker_enumeration_service,
                        invocation_key_service,
                        key_value_service,
                        blob_store_service,
                        config: config.clone(),
                        worker_id: worker_id.clone(),
                        account_id: account_id.clone(),
                        current_invocation_key: None,
                        active_workers: active_workers.clone(),
                        recovery_management,
                        rpc,
                        resources: HashMap::new(),
                        last_resource_id: 0,
                        deleted_regions: worker_config.deleted_regions.clone(),
                        next_deleted_region: worker_config
                            .deleted_regions
                            .find_next_deleted_region(0),
                        overridden_retry_policy: None,
                        persistence_level: PersistenceLevel::Smart,
                        assume_idempotence: true,
                        open_function_table: HashMap::new(),
                        replay_idx: 1,
                        replay_target: oplog_size,
                    },
                    temp_dir,
                    execution_status,
                }
            },
        )
        .map_err(|e| GolemError::runtime(format!("Could not create WASI context: {e}")))
    }

    pub fn get_public_state(&self) -> &PublicDurableWorkerState<Ctx> {
        &self.public_state
    }

    pub fn worker_id(&self) -> &WorkerId {
        &self.worker_id
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
            ExecutionStatus::Interrupted { interrupt_kind, .. } => Some(interrupt_kind),
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
                interrupt_kind,
                await_interruption,
                last_known_status,
            } => {
                *execution_status = ExecutionStatus::Interrupted {
                    interrupt_kind,
                    last_known_status,
                };
                await_interruption.send(()).ok();
            }
            ExecutionStatus::Interrupted { .. } => {}
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
            ExecutionStatus::Interrupted { .. } => {}
        }
    }

    pub async fn get_worker_status(&self) -> WorkerStatus {
        match self.state.worker_service.get(&self.worker_id).await {
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

        status.deleted_regions = self.state.deleted_regions.clone();
        status.overridden_retry_config = self.state.overridden_retry_policy.clone();
        status.pending_invocations = self.public_state.invocation_queue().pending_invocations();
        status.pending_updates = self.public_state.invocation_queue().pending_updates();
        status.oplog_idx = self.state.oplog.current_oplog_index().await;
        f(&mut status);
        self.state
            .worker_service
            .update_status(&self.worker_id, &status)
            .await;

        let mut execution_status = self.execution_status.write().unwrap();
        execution_status.set_last_known_status(status);
    }

    pub async fn store_worker_status(&self, status: WorkerStatus) {
        self.update_worker_status(|s| s.status = status).await;
    }

    pub async fn update_pending_invocations(&self) {
        self.update_worker_status(|_| {}).await;
    }

    pub fn get_stdio(&self) -> ManagedStandardIo {
        self.public_state.managed_stdio.clone()
    }

    pub async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.get_stdio()
            .get_current_invocation_key()
            .await
            .or(self.state.get_current_invocation_key())
    }

    pub fn get_current_invocation_result(&self) -> LookupResult {
        match &self.state.current_invocation_key {
            Some(key) => self
                .state
                .invocation_key_service
                .lookup_key(&self.state.worker_id, key),
            None => LookupResult::Invalid,
        }
    }

    pub fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.state.rpc.clone()
    }

    /// Records the result of an automatic update, if any was active, and returns whether the worker
    /// should be restarted to retry recovering without the pending update.
    pub async fn finalize_automatic_update(&mut self, result: &Result<(), GolemError>) -> bool {
        let pending_update = self.public_state.invocation_queue.pop_pending_update();
        match pending_update {
            Some(pending_update) => match result {
                Ok(_) => {
                    let target_version = *pending_update.description.target_version();
                    info!(
                        "Automatic update to {} finished successfully for {}",
                        target_version, self.worker_id
                    );
                    let entry = OplogEntry::successful_update(target_version);
                    let timestamp = entry.timestamp();
                    self.public_state.oplog.add(entry).await;
                    self.update_worker_status(|status| {
                        status.component_version = target_version;
                        status.successful_updates.push(SuccessfulUpdateRecord {
                            timestamp,
                            target_version,
                        })
                    })
                    .await;
                    false
                }
                Err(error) => {
                    let target_version = *pending_update.description.target_version();

                    warn!(
                        "Automatic update to {} failed for {}: {}",
                        target_version, self.worker_id, error
                    );
                    let entry = OplogEntry::failed_update(target_version, Some(error.to_string()));
                    let timestamp = entry.timestamp();
                    self.public_state.oplog.add(entry).await;
                    self.update_worker_status(|status| {
                        status.failed_updates.push(FailedUpdateRecord {
                            timestamp,
                            target_version,
                            details: Some(error.to_string()),
                        })
                    })
                    .await;
                    true
                }
            },
            None => {
                debug!("No pending updates to finalize for {}", self.worker_id);
                false
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationManagement for DurableWorkerCtx<Ctx> {
    async fn set_current_invocation_key(&mut self, invocation_key: InvocationKey) {
        self.state.set_current_invocation_key(invocation_key)
    }

    async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.get_current_invocation_key().await
    }

    async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.state.interrupt_invocation_key(key).await
    }

    async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.state.resume_invocation_key(key).await
    }

    async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Value>, GolemError>,
    ) {
        self.state.confirm_invocation_key(key, vals).await
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

    async fn deactivate(&self) {
        debug!("deactivating worker {}", self.worker_id);
        self.state.active_workers.remove(&self.worker_id);
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        calling_convention: Option<CallingConvention>,
    ) -> anyhow::Result<()> {
        let proto_function_input: Vec<golem_wasm_rpc::protobuf::Val> = function_input
            .iter()
            .map(|value| value.clone().into())
            .collect();
        let oplog_entry = OplogEntry::exported_function_invoked(
            full_function_name.to_string(),
            &proto_function_input,
            self.get_current_invocation_key().await.ok_or(anyhow!(
                "No active invocation key is associated with the worker"
            ))?,
            calling_convention,
        )
        .unwrap_or_else(|err| {
            panic!(
                "could not encode function input for {full_function_name} on {}: {err}",
                self.worker_id()
            )
        });

        self.state.oplog.add(oplog_entry).await;
        self.state.oplog.commit().await;
        Ok(())
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> Result<(), anyhow::Error> {
        self.state.consume_hint_entries().await;

        if self.state.is_live() {
            let needs_commit = match trap_type {
                TrapType::Error(error) => Some(OplogEntry::error(error.clone())),
                TrapType::Interrupt(InterruptKind::Interrupt) => Some(OplogEntry::interrupted()),
                TrapType::Interrupt(InterruptKind::Suspend) => Some(OplogEntry::suspend()),
                TrapType::Exit => Some(OplogEntry::exited()),
                _ => None,
            };

            if let Some(entry) = needs_commit {
                self.state.oplog.add(entry).await;
                self.state.oplog.commit().await;
            }
        }

        Ok(())
    }

    async fn on_invocation_failure_deactivated(
        &mut self,
        error: &TrapType,
    ) -> Result<WorkerStatus, anyhow::Error> {
        let previous_tries = self.state.trailing_error_count().await;
        let default_retry_config = &self.state.config.retry;
        let retry_config = self
            .state
            .overridden_retry_policy
            .as_ref()
            .unwrap_or(default_retry_config)
            .clone();
        let decision = self
            .state
            .recovery_management
            .schedule_recovery_on_trap(&self.worker_id, &retry_config, previous_tries, error)
            .await;

        debug!(
            "Recovery decision for {} because of error {:?} after {} tries: {:?}",
            self.worker_id, error, previous_tries, decision
        );

        Ok(calculate_worker_status(
            &retry_config,
            error,
            previous_tries,
        ))
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<Option<Vec<Value>>, anyhow::Error> {
        self.state.consume_hint_entries().await;
        let is_live_after = self.state.is_live();

        if is_live_after {
            let proto_output: Vec<golem_wasm_rpc::protobuf::Val> =
                output.iter().map(|value| value.clone().into()).collect();
            let oplog_entry = OplogEntry::exported_function_completed(&proto_output, consumed_fuel)
                .unwrap_or_else(|err| {
                    panic!("could not encode function result for {full_function_name}: {err}")
                });

            self.state.oplog.add(oplog_entry).await;
            self.state.oplog.commit().await;
        } else {
            let response = self
                .state
                .get_oplog_entry_exported_function_completed()
                .await?;

            if let Some(function_output) = response {
                let is_diverged = function_output != output;
                if is_diverged {
                    return Err(anyhow!(GolemError::unexpected_oplog_entry(
                        format!("{full_function_name}({function_input:?}) => {function_output:?}"),
                        format!("{full_function_name}({function_input:?}) => {output:?}"),
                    )));
                }
            }
        }

        debug!(
            "Function {}/{full_function_name} finished with {:?}",
            self.worker_id, output
        );

        // Return indicating that it is done
        Ok(Some(output))
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

pub trait DurableWorkerCtxView<Ctx: WorkerCtx> {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Ctx>;
    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Ctx>;
}

#[async_trait]
impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> ExternalOperations<Ctx> for DurableWorkerCtx<Ctx> {
    type ExtraDeps = Ctx::ExtraDeps;

    async fn set_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    ) -> Result<(), GolemError> {
        let metadata = this.worker_service().get(worker_id).await;
        let mut latest_status = calculate_last_known_status(this, worker_id, &metadata).await?;
        latest_status.status = status;
        this.worker_service()
            .update_status(worker_id, &latest_status)
            .await;
        Ok(())
    }

    async fn get_last_error_and_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Option<LastError> {
        last_error_and_retry_count(this, worker_id).await
    }

    async fn compute_latest_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        calculate_last_known_status(this, worker_id, metadata).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<(), GolemError> {
        debug!("Starting prepare_instance for {worker_id}");
        let start = Instant::now();
        let mut count = 0;
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
                    Ok(None) => break Ok(()),
                    Ok(Some((
                        function_name,
                        function_input,
                        invocation_key,
                        calling_convention,
                    ))) => {
                        debug!("prepare_instance invoking function {function_name} on {worker_id}");
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_invocation_key(invocation_key)
                            .await;

                        let finished = invoke_worker(
                            function_name.to_string(),
                            function_input,
                            store,
                            instance,
                            calling_convention.unwrap_or(CallingConvention::Component),
                            false, // we know it was not live before, because cont=true
                        )
                        .await;

                        if !finished {
                            break Err(GolemError::runtime(format!(
                                "The worker could not finish replaying a function {function_name}"
                            )));
                        } else {
                            let result = store
                                .as_context()
                                .data()
                                .durable_ctx()
                                .get_current_invocation_result();
                            if let LookupResult::Complete(Err(error)) = result {
                                break Err(error);
                            }
                        }

                        count += 1;
                    }
                }
            } else {
                break Ok(());
            }
        };
        record_resume_worker(start.elapsed());
        record_number_of_replayed_functions(count);

        let retry = store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .finalize_automatic_update(&result)
            .await;

        if retry {
            debug!("Retrying prepare_instance for {worker_id} after failed update attempt");
            Self::prepare_instance(worker_id, instance, store).await
        } else {
            debug!("Finished prepare_instance for {worker_id}");
            result.map_err(|err| GolemError::failed_to_resume_instance(worker_id.clone(), err))
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

    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
    ) -> Result<(), anyhow::Error> {
        info!("Recovering workers");

        let workers = this.worker_service().get_running_workers_in_shards().await;

        debug!("Recovering running workers: {:?}", workers);

        let default_retry_config = &this.config().retry;
        for worker in workers {
            let worker_id = worker.worker_id.clone();
            let actualized_metadata =
                calculate_last_known_status(this, &worker_id, &Some(worker)).await?;
            let last_error = Self::get_last_error_and_retry_count(this, &worker_id).await;
            let decision = this
                .recovery_management()
                .schedule_recovery_on_startup(
                    &worker_id,
                    actualized_metadata
                        .overridden_retry_config
                        .as_ref()
                        .unwrap_or(default_retry_config),
                    &last_error,
                )
                .await;
            if let Some(last_error) = last_error {
                debug!("Recovery decision for {worker_id} after {last_error}: {decision:?}");
            }
        }

        info!("Finished recovering workers");
        Ok(())
    }
}

async fn last_error_and_retry_count<T: HasOplogService>(
    this: &T,
    worker_id: &WorkerId,
) -> Option<LastError> {
    let mut idx = this.oplog_service().get_size(worker_id).await;
    let mut retry_count = 0;
    if idx == 0 {
        None
    } else {
        let mut first_error = None;
        loop {
            let oplog_entry = this.oplog_service().read(worker_id, idx - 1, 1).await;
            match oplog_entry.first()
                .unwrap_or_else(|| panic!("Internal error: op log for {} has size greater than zero but no entry at last index", worker_id)) {
                OplogEntry::Error { error, .. } => {
                    retry_count += 1;
                    if first_error.is_none() {
                        first_error = Some(error.clone());
                    }
                    if idx > 0 {
                        idx -= 1;
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
        }
    }
}

pub struct PrivateDurableWorkerState<Ctx: WorkerCtx> {
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    oplog: Arc<dyn Oplog + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    key_value_service: Arc<dyn KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
    config: Arc<GolemConfig>,
    worker_id: WorkerId,
    account_id: AccountId,
    current_invocation_key: Option<InvocationKey>,
    active_workers: Arc<ActiveWorkers<Ctx>>,
    recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
    rpc: Arc<dyn Rpc + Send + Sync>,
    resources: HashMap<u64, ResourceAny>,
    last_resource_id: u64,
    deleted_regions: DeletedRegions,
    next_deleted_region: Option<OplogRegion>,
    overridden_retry_policy: Option<RetryConfig>,
    persistence_level: PersistenceLevel,
    assume_idempotence: bool,
    open_function_table: HashMap<u32, u64>,
    replay_target: u64,
    replay_idx: u64,
}

impl<Ctx: WorkerCtx> PrivateDurableWorkerState<Ctx> {
    pub async fn begin_function(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
    ) -> Result<u64, GolemError> {
        let begin_index = self.oplog.current_oplog_index().await;
        if !self.assume_idempotence
            && *wrapped_function_type == WrappedFunctionType::WriteRemote
            && self.persistence_level != PersistenceLevel::PersistNothing
        {
            if self.is_live() {
                self.oplog.add(OplogEntry::begin_remote_write()).await;
                self.oplog.commit().await;
                Ok(begin_index)
            } else {
                let _ = crate::get_oplog_entry!(self, OplogEntry::BeginRemoteWrite)?;
                let end_index = self
                    .lookup_oplog_entry(begin_index, OplogEntry::is_end_remote_write)
                    .await;
                if end_index.is_none() {
                    // Must switch to live mode before failing to be able to commit an Error entry
                    self.replay_idx = self.replay_target;
                    Err(GolemError::runtime(
                        "Non-idempotent remote write operation was not completed, cannot retry",
                    ))
                } else {
                    Ok(begin_index)
                }
            }
        } else {
            Ok(begin_index)
        }
    }

    pub async fn end_function(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
        begin_index: u64,
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
                let _ = crate::get_oplog_entry!(self, OplogEntry::EndRemoteWrite)?;
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    /// In live mode it returns the last oplog index.
    /// In replay mode it returns the current replay index.
    pub async fn current_oplog_index(&self) -> u64 {
        if self.is_live() {
            self.oplog.current_oplog_index().await
        } else {
            self.replay_idx
        }
    }

    pub async fn read_oplog(&self, idx: u64, n: u64) -> Vec<OplogEntry> {
        self.oplog_service.read(&self.worker_id, idx, n).await
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.replay_idx == self.replay_target
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    async fn get_oplog_entry(&mut self) -> OplogEntry {
        assert!(self.is_replay());

        let oplog_entries = self.read_oplog(self.replay_idx, 1).await;
        let oplog_entry = oplog_entries[0].clone();

        let update_next_deleted_region = match &self.next_deleted_region {
            Some(region) if region.start == self.replay_idx => {
                let target = region.end + 1;
                debug!(
                    "Worker {} reached deleted region at {}, jumping to {} (oplog size: {})",
                    self.worker_id, self.replay_idx, target, self.replay_target
                );
                self.replay_idx = target;
                true
            }
            _ => {
                self.replay_idx += 1;
                false
            }
        };

        if update_next_deleted_region {
            self.next_deleted_region = self
                .deleted_regions
                .find_next_deleted_region(self.replay_idx);
        }

        oplog_entry
    }

    async fn lookup_oplog_entry(
        &mut self,
        begin_idx: u64,
        check: impl Fn(&OplogEntry, u64) -> bool,
    ) -> Option<u64> {
        let mut start = self.replay_idx;
        const CHUNK_SIZE: u64 = 1024;
        while start < self.replay_target {
            let entries = self
                .oplog_service
                .read(&self.worker_id, start, CHUNK_SIZE)
                .await;
            for (n, entry) in entries.iter().enumerate() {
                if check(entry, begin_idx) {
                    return Some(start + n as u64);
                }
            }
            start += entries.len() as u64;
        }

        None
    }

    async fn get_oplog_entry_exported_function_invoked(
        &mut self,
    ) -> Result<Option<(String, Vec<Value>, InvocationKey, Option<CallingConvention>)>, GolemError>
    {
        loop {
            if self.is_replay() {
                let oplog_entry = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionInvoked {
                        function_name,
                        invocation_key,
                        calling_convention,
                        ..
                    } => {
                        let request: Vec<golem_wasm_rpc::protobuf::Val> = oplog_entry
                            .payload()
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
                            invocation_key.clone(),
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
                let oplog_entry = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionCompleted { .. } => {
                        let response: Vec<golem_wasm_rpc::protobuf::Val> = oplog_entry
                            .payload()
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

    /// Consumes Suspend, Error and Interrupt entries which are hints for the server to decide whether to
    /// keep workers in memory or allow them to rerun etc., but contain no actionable information
    /// during replay
    async fn consume_hint_entries(&mut self) {
        loop {
            if self.is_replay() {
                let oplog_entry = self.get_oplog_entry().await;
                match oplog_entry {
                    entry if entry.is_hint() => {}
                    _ => {
                        self.replay_idx -= 1;
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    pub async fn sleep_until(&self, when: chrono::DateTime<chrono::Utc>) -> Result<(), GolemError> {
        let promise_id = self
            .promise_service
            .create(&self.worker_id, self.current_oplog_index().await)
            .await;

        let schedule_id = self.scheduler_service.schedule(when, promise_id).await;
        debug!(
            "Schedule added to awake suspended worker at {} with id {}",
            when.to_rfc3339(),
            schedule_id
        );

        Ok(())
    }

    pub async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Value>, GolemError>,
    ) {
        self.invocation_key_service
            .confirm_key(&self.worker_id, key, vals)
    }

    pub async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.invocation_key_service
            .interrupt_key(&self.worker_id, key)
    }

    pub async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.invocation_key_service.resume_key(&self.worker_id, key)
    }

    pub fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.current_invocation_key.clone()
    }

    pub fn set_current_invocation_key(&mut self, invocation_key: InvocationKey) {
        self.current_invocation_key = Some(invocation_key);
    }

    /// Counts the number of Error entries that are at the end of the oplog. This equals to the number of retries that have been attempted.
    /// It also returns the last error stored in these entries.
    pub async fn trailing_error_count(&self) -> u64 {
        last_error_and_retry_count(self, &self.worker_id)
            .await
            .map(|last_error| last_error.retry_count)
            .unwrap_or_default()
    }

    pub async fn get_workers(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GolemError> {
        self.worker_enumeration_service
            .get(component_id, filter, cursor, count, precise)
            .await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> ResourceStore for PrivateDurableWorkerState<Ctx> {
    fn self_uri(&self) -> Uri {
        Uri::golem_uri(&self.worker_id, None)
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

impl<Ctx: WorkerCtx> HasOplogService for PrivateDurableWorkerState<Ctx> {
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplog for PrivateDurableWorkerState<Ctx> {
    fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
        self.oplog.clone()
    }
}

pub struct PublicDurableWorkerState<Ctx: WorkerCtx> {
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
    managed_stdio: ManagedStandardIo,
    invocation_queue: Arc<InvocationQueue<Ctx>>,
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

    async fn enqueue(&self, message: Bytes, invocation_key: InvocationKey) {
        self.managed_stdio.enqueue(message, invocation_key).await
    }
}

impl<Ctx: WorkerCtx> HasInvocationQueue<Ctx> for PublicDurableWorkerState<Ctx> {
    fn invocation_queue(&self) -> Arc<InvocationQueue<Ctx>> {
        self.invocation_queue.clone()
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
    fn table(&self) -> &ResourceTable {
        &self.0.table
    }

    fn table_mut(&mut self) -> &mut ResourceTable {
        &mut self.0.table
    }

    fn ctx(&self) -> &WasiCtx {
        &self.0.wasi
    }

    fn ctx_mut(&mut self) -> &mut WasiCtx {
        &mut self.0.wasi
    }
}

impl<'a, Ctx: WorkerCtx> WasiHttpView for DurableWorkerCtxWasiHttpView<'a, Ctx> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.0.wasi_http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.0.table
    }

    fn send_request(
        &mut self,
        request: OutgoingRequest,
    ) -> anyhow::Result<Resource<HostFutureIncomingResponse>>
    where
        Self: Sized,
    {
        if self.0.state.is_replay() {
            // If this is a replay, we must not actually send the request, but we have to store it in the
            // FutureIncomingResponse because it is possible that there wasn't any response recorded in the oplog.
            // If that is the case, the request has to be sent as soon as we get into live mode and trying to await
            // or poll the response future.
            let fut = self
                .table()
                .push(HostFutureIncomingResponse::deferred(request))?;
            Ok(fut)
        } else {
            default_send_request(self, request)
        }
    }
}

struct Ready {}

#[async_trait]
impl Subscribe for Ready {
    async fn ready(&mut self) {}
}

/// Helper macro for expecting a given type of OplogEntry as the next entry in the oplog during
/// replay, while skipping hint entries.
/// The macro expression's type is `Result<OplogEntry, GolemError>` and it fails if the next non-hint
/// entry was not the expected one.
#[macro_export]
macro_rules! get_oplog_entry {
    ($private_state:expr, $case:path) => {
        loop {
            let oplog_entry = $private_state.get_oplog_entry().await;
            match oplog_entry {
                $case { .. } => {
                    break Ok(oplog_entry);
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
