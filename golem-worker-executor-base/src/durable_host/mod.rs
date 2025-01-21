// Copyright 2024-2025 Golem Cloud
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

use crate::durable_host::http::serialized::SerializableHttpRequest;
use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::replay_state::ReplayState;
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::wasm_rpc::UrnExtensions;
use crate::error::GolemError;
use crate::function_result_interpreter::interpret_function_results;
use crate::invocation::{find_first_available_function, invoke_worker, InvokeResult};
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, ListDirectoryResult,
    PersistenceLevel, ReadFileResult, TrapType, WorkerConfig,
};
use crate::services::blob_store::BlobStoreService;
use crate::services::component::{ComponentMetadata, ComponentService};
use crate::services::file_loader::{FileLoader, FileUseToken};
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, OplogService};
use crate::services::plugins::Plugins;
use crate::services::promise::PromiseService;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{worker_enumeration, HasAll, HasConfig, HasOplog, HasWorker};
use crate::services::{HasOplogService, HasPlugins};
use crate::wasi_host;
use crate::worker::{calculate_last_known_status, is_worker_error_retriable};
use crate::worker::{RetryDecision, Worker};
use crate::workerctx::{
    ExternalOperations, FileSystemReading, IndexedResourceStore, InvocationHooks,
    InvocationManagement, PublicWorkerIo, StatusManagement, UpdateManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::BytesMut;
use chrono::{DateTime, Utc};
pub use durability::*;
use futures::future::try_join_all;
use futures_util::TryFutureExt;
use futures_util::TryStreamExt;
use golem_common::model::component::ComponentOwner;
use golem_common::model::oplog::{
    DurableFunctionType, IndexedResourceKey, LogLevel, OplogEntry, OplogIndex, UpdateDescription,
    WorkerError, WorkerResourceId,
};
use golem_common::model::plugin::{PluginOwner, PluginScope};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{exports, PluginInstallationId};
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentFilePermissions, ComponentFileSystemNode,
    ComponentFileSystemNodeDetails, ComponentId, ComponentType, ComponentVersion,
    FailedUpdateRecord, IdempotencyKey, InitialComponentFile, OwnedWorkerId, ScanCursor,
    ScheduledAction, SuccessfulUpdateRecord, Timestamp, WorkerEvent, WorkerFilter, WorkerId,
    WorkerMetadata, WorkerResourceDescription, WorkerStatus, WorkerStatusRecord,
};
use golem_common::model::{RetryConfig, TargetWorkerId};
use golem_common::retries::get_delay;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{Uri, Value};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Add;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use std::vec;
use tempfile::TempDir;
use tokio_util::codec::{BytesCodec, FramedRead};
use tracing::{debug, info, span, warn, Instrument, Level};
use wasmtime::component::{Instance, Resource, ResourceAny};
use wasmtime::{AsContext, AsContextMut};
use wasmtime_wasi::bindings::filesystem::preopens::Descriptor;
use wasmtime_wasi::{
    FsResult, I32Exit, ResourceTable, ResourceTableError, Stderr, Stdout, WasiCtx, WasiImpl,
    WasiView,
};
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::types::{
    default_send_request, HostFutureIncomingResponse, OutgoingRequestConfig,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpImpl, WasiHttpView};

pub mod blobstore;
mod cli;
mod clocks;
mod filesystem;
pub mod golem;
pub mod http;
pub mod io;
pub mod keyvalue;
mod logging;
mod random;
pub mod serialized;
mod sockets;
pub mod wasm_rpc;

mod durability;
mod dynamic_linking;
mod replay_state;

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: Arc<Mutex<ResourceTable>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi: Arc<Mutex<WasiCtx>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi_http: WasiHttpCtx,
    pub owned_worker_id: OwnedWorkerId,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState<
        <Ctx::ComponentOwner as ComponentOwner>::PluginOwner,
        Ctx::PluginScope,
    >,
    _temp_dir: Arc<TempDir>,
    _used_files: Vec<FileUseToken>,
    read_only_paths: Arc<RwLock<HashSet<PathBuf>>>,
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
        component_service: Arc<dyn ComponentService + Send + Sync>,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<
            dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
                + Send
                + Sync,
        >,
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

        let (file_use_tokens, read_only_paths) = prepare_filesystem(
            file_loader,
            &owned_worker_id.account_id,
            temp_dir.path(),
            &component_metadata.files,
        )
        .await?;

        let stdin = ManagedStdIn::disabled();
        let stdout = ManagedStdOut::from_stdout(Stdout);
        let stderr = ManagedStdErr::from_stderr(Stderr);

        let last_oplog_index = oplog.current_oplog_index().await;

        let (wasi, table) = wasi_host::create_context(
            &worker_config.args,
            &worker_config.env,
            temp_dir.path().to_path_buf(),
            stdin,
            stdout,
            stderr,
            |duration| anyhow!(SuspendForSleep(duration)),
            config.suspend.suspend_after,
        )
        .map_err(|e| GolemError::runtime(format!("Could not create WASI context: {e}")))?;
        let wasi_http = WasiHttpCtx::new();
        Ok(DurableWorkerCtx {
            table: Arc::new(Mutex::new(table)),
            wasi: Arc::new(Mutex::new(wasi)),
            wasi_http,
            owned_worker_id: owned_worker_id.clone(),
            public_state: PublicDurableWorkerState {
                promise_service: promise_service.clone(),
                event_service,
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
                component_service,
                plugins,
                config.clone(),
                owned_worker_id.clone(),
                rpc,
                worker_proxy,
                worker_config.deleted_regions.clone(),
                last_oplog_index,
                component_metadata,
                worker_config.total_linear_memory_size,
            )
            .await,
            _temp_dir: temp_dir,
            _used_files: file_use_tokens,
            read_only_paths: Arc::new(RwLock::new(read_only_paths)),
            execution_status,
        })
    }

    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    fn is_read_only(&mut self, fd: &Resource<Descriptor>) -> Result<bool, ResourceTableError> {
        let table = Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");

        match table.get(fd)? {
            Descriptor::File(f) => {
                let read_only = self
                    .read_only_paths
                    .read()
                    .expect("There should be no writers to read_only_paths")
                    .contains(&f.path);
                Ok(read_only)
            }
            Descriptor::Dir(_) => Ok(false),
        }
    }

    fn fail_if_read_only(&mut self, fd: &Resource<Descriptor>) -> FsResult<()> {
        if self.is_read_only(fd)? {
            Err(wasmtime_wasi::bindings::filesystem::types::ErrorCode::NotPermitted.into())
        } else {
            Ok(())
        }
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

    pub fn owned_worker_id(&self) -> &OwnedWorkerId {
        &self.owned_worker_id
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

    pub fn as_wasi_view(&mut self) -> WasiImpl<DurableWorkerCtxWasiView<Ctx>> {
        WasiImpl(DurableWorkerCtxWasiView(self))
    }

    pub fn as_wasi_http_view(&mut self) -> WasiHttpImpl<DurableWorkerCtxWasiHttpView<Ctx>> {
        WasiHttpImpl(DurableWorkerCtxWasiHttpView(self))
    }

    pub async fn update_worker_status(&self, f: impl FnOnce(&mut WorkerStatusRecord)) {
        let mut status = self
            .execution_status
            .read()
            .unwrap()
            .last_known_status()
            .clone();

        let mut deleted_regions = self.state.replay_state.deleted_regions().await;
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
        status.total_linear_memory_size = self.state.total_linear_memory_size;
        status.oplog_idx = self.state.oplog.current_oplog_index().await;
        f(&mut status);
        self.public_state.worker().update_status(status).await;
    }

    pub fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.state.rpc.clone()
    }

    pub fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.state.worker_proxy.clone()
    }

    pub fn total_linear_memory_size(&self) -> u64 {
        self.state.total_linear_memory_size
    }

    pub async fn increase_memory(&mut self, delta: u64) -> anyhow::Result<bool> {
        if self.state.is_replay() {
            // The increased amount was already recorded in live mode, so our worker
            // was initialized with the correct amount of memory.
            Ok(true)
        } else {
            // In live mode we need to try to get more memory permits and if we can't,
            // we fail the worker, unload it from memory and schedule a retry.
            // let current_size = self.update_worker_status();
            self.state
                .oplog
                .add_and_commit(OplogEntry::grow_memory(delta))
                .await;
            self.update_worker_status(|_| {}).await;

            self.public_state.worker().increase_memory(delta).await?;
            self.state.total_linear_memory_size += delta;
            Ok(true)
        }
    }

    fn get_recovery_decision_on_trap(
        retry_config: &RetryConfig,
        previous_tries: u64,
        trap_type: &TrapType,
    ) -> RetryDecision {
        match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt) => RetryDecision::None,
            TrapType::Interrupt(InterruptKind::Suspend) => RetryDecision::None,
            TrapType::Interrupt(InterruptKind::Restart) => RetryDecision::Immediate,
            TrapType::Interrupt(InterruptKind::Jump) => RetryDecision::Immediate,
            TrapType::Exit => RetryDecision::None,
            TrapType::Error(error) => {
                if is_worker_error_retriable(retry_config, error, previous_tries) {
                    if error == &WorkerError::OutOfMemory {
                        RetryDecision::ReacquirePermits
                    } else {
                        match get_delay(retry_config, previous_tries) {
                            Some(delay) => RetryDecision::Delayed(delay),
                            None => RetryDecision::None,
                        }
                    }
                } else {
                    RetryDecision::None
                }
            }
        }
    }

    fn get_recovery_decision_on_startup(
        retry_config: &RetryConfig,
        last_error: &Option<LastError>,
    ) -> RetryDecision {
        match last_error {
            Some(last_error) => {
                if is_worker_error_retriable(
                    retry_config,
                    &last_error.error,
                    last_error.retry_count,
                ) {
                    RetryDecision::Immediate
                } else {
                    RetryDecision::None
                }
            }
            None => RetryDecision::Immediate,
        }
    }

    async fn emit_log_event(&self, event: WorkerEvent) {
        if let Some(entry) = event.as_oplog_entry() {
            if let OplogEntry::Log {
                level,
                context,
                message,
                ..
            } = &entry
            {
                // Stdout and stderr writes are persistent and overwritten by sending the data to the event
                // service instead of the real output stream

                if self.state.is_live()
                // If the worker is still in replay mode we never emit events.
                {
                    if self.state.persistence_level == PersistenceLevel::PersistNothing
                    // If persistence is off, we always emit events
                    {
                        // Emit the event and write a special oplog entry
                        self.public_state
                            .event_service
                            .emit_event(event.clone(), true);
                        self.state.oplog.add(entry).await;
                    } else if !self
                        .state
                        .replay_state
                        .seen_log(*level, context, message)
                        .await
                    {
                        // haven't seen this log before
                        self.public_state
                            .event_service
                            .emit_event(event.clone(), true);
                        self.state.oplog.add(entry).await;
                    } else {
                        // we have persisted emitting this log before, so we mark it as non-live and
                        // remove the entry from the seen log set.
                        // note that we still call emit_event because we need replayed log events for
                        // improved error reporting in case of invocation failures
                        self.public_state
                            .event_service
                            .emit_event(event.clone(), false);
                        self.state
                            .replay_state
                            .remove_seen_log(*level, context, message)
                            .await;
                    }
                }
            }
        }
    }

    pub async fn generate_unique_local_worker_id(
        &mut self,
        remote_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError> {
        match remote_worker_id.clone().try_into_worker_id() {
            Some(worker_id) => Ok(worker_id),
            None => {
                let durability = Durability::<WorkerId, SerializableError>::new(
                    self,
                    "golem::rpc::wasm-rpc",
                    "generate_unique_local_worker_id",
                    DurableFunctionType::ReadLocal,
                )
                .await?;
                let worker_id = if durability.is_live() {
                    let result = self
                        .rpc()
                        .generate_unique_local_worker_id(remote_worker_id)
                        .await;
                    durability.persist(self, (), result).await
                } else {
                    durability.replay(self).await
                }?;

                Ok(worker_id)
            }
        }
    }
}

impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> DurableWorkerCtx<Ctx> {
    /// Records the result of an automatic update, if any was active, and returns whether the worker
    /// should be restarted to retry recovering without the pending update.
    pub async fn finalize_pending_update(
        result: &Result<RetryDecision, GolemError>,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> RetryDecision {
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
                Ok(RetryDecision::None) => {
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
                                let failed = if let Some(load_snapshot) =
                                    find_first_available_function(
                                        store,
                                        instance,
                                        vec![
                                            "golem:api/load-snapshot@1.1.0.{load}".to_string(),
                                            "golem:api/load-snapshot@0.2.0.{load}".to_string(),
                                        ],
                                    ) {
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
                                        load_snapshot,
                                        vec![Value::List(
                                            data.iter().map(|b| Value::U8(*b)).collect(),
                                        )],
                                        store,
                                        instance,
                                    )
                                    .await;
                                    store
                                        .as_context_mut()
                                        .data_mut()
                                        .end_call_snapshotting_function();

                                    match load_result {
                                        Err(error) => Some(format!(
                                            "Manual update failed to load snapshot: {error}"
                                        )),
                                        Ok(InvokeResult::Failed { error, .. }) => {
                                            let stderr = store
                                                .as_context()
                                                .data()
                                                .get_public_state()
                                                .event_service()
                                                .get_last_invocation_errors();
                                            let error = error.to_string(&stderr);
                                            Some(format!(
                                                "Manual update failed to load snapshot: {error}"
                                            ))
                                        }
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
                                    }
                                } else {
                                    Some(
                                        "Failed to find exported load-snapshot function"
                                            .to_string(),
                                    )
                                };

                                if let Some(error) = failed {
                                    store
                                        .as_context_mut()
                                        .data_mut()
                                        .on_worker_update_failed(target_version, Some(error))
                                        .await;
                                    RetryDecision::Immediate
                                } else {
                                    let component_metadata =
                                        store.as_context().data().component_metadata().clone();
                                    store
                                        .as_context_mut()
                                        .data_mut()
                                        .on_worker_update_succeeded(
                                            target_version,
                                            component_metadata.size,
                                            HashSet::from_iter(
                                                component_metadata
                                                    .plugin_installations
                                                    .into_iter()
                                                    .map(|installation| installation.id),
                                            ),
                                        )
                                        .await;
                                    RetryDecision::None
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
                                RetryDecision::Immediate
                            }
                            Err(error) => {
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_worker_update_failed(target_version, Some(error))
                                    .await;
                                RetryDecision::Immediate
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
                            .on_worker_update_succeeded(
                                target_version,
                                component_metadata.size,
                                HashSet::from_iter(
                                    component_metadata
                                        .plugin_installations
                                        .into_iter()
                                        .map(|installation| installation.id),
                                ),
                            )
                            .await;
                        RetryDecision::None
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
                    RetryDecision::Immediate
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
                    RetryDecision::Immediate
                }
            },
            None => {
                debug!("No pending updates to finalize for {}", worker_id);
                RetryDecision::None
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
        self.state.get_current_idempotency_key()
    }

    fn is_live(&self) -> bool {
        self.state.is_live()
    }

    fn is_replay(&self) -> bool {
        self.state.is_replay()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> StatusManagement for DurableWorkerCtx<Ctx> {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        let execution_status = self.execution_status.read().unwrap().clone();
        match execution_status {
            ExecutionStatus::Interrupting { interrupt_kind, .. } => Some(interrupt_kind),
            _ => None,
        }
    }

    async fn set_suspended(&self) -> Result<(), GolemError> {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running {
                last_known_status, ..
            } => {
                *execution_status = ExecutionStatus::Suspended {
                    last_known_status,
                    component_type: self.component_metadata().component_type,
                    timestamp: Timestamp::now_utc(),
                };
            }
            ExecutionStatus::Suspended { .. } => {}
            ExecutionStatus::Interrupting {
                await_interruption,
                last_known_status,
                ..
            } => {
                *execution_status = ExecutionStatus::Suspended {
                    last_known_status,
                    component_type: self.component_metadata().component_type,
                    timestamp: Timestamp::now_utc(),
                };
                await_interruption.send(()).ok();
            }
            ExecutionStatus::Loading {
                last_known_status, ..
            } => {
                *execution_status = ExecutionStatus::Suspended {
                    last_known_status,
                    component_type: self.component_metadata().component_type,
                    timestamp: Timestamp::now_utc(),
                };
            }
        };

        Ok(())
    }

    fn set_running(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { .. } => {}
            ExecutionStatus::Suspended {
                last_known_status, ..
            } => {
                *execution_status = ExecutionStatus::Running {
                    last_known_status,
                    component_type: self.component_metadata().component_type,
                    timestamp: Timestamp::now_utc(),
                };
            }
            ExecutionStatus::Interrupting { .. } => {}
            ExecutionStatus::Loading {
                last_known_status, ..
            } => {
                *execution_status = ExecutionStatus::Running {
                    last_known_status,
                    component_type: self.component_metadata().component_type,
                    timestamp: Timestamp::now_utc(),
                };
            }
        }
    }

    async fn get_worker_status(&self) -> WorkerStatus {
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

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.update_worker_status(|s| s.status = status.clone())
            .await;
        if (status == WorkerStatus::Idle
            || status == WorkerStatus::Failed
            || status == WorkerStatus::Exited)
            && self.component_metadata().component_type == ComponentType::Durable
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

    async fn update_pending_invocations(&self) {
        self.update_worker_status(|_| {}).await;
    }

    async fn update_pending_updates(&self) {
        self.update_worker_status(|_| {}).await;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
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
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "could not encode function input for {full_function_name} on {}: {err}",
                        self.worker_id()
                    )
                });
            self.state.oplog.commit(CommitLevel::Always).await;
        }
        Ok(())
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> RetryDecision {
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
        let (updated_worker_status, oplog_entry, store_result) = match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt) => (
                WorkerStatus::Interrupted,
                Some(OplogEntry::interrupted()),
                true,
            ),
            TrapType::Interrupt(InterruptKind::Suspend) => {
                (WorkerStatus::Suspended, Some(OplogEntry::suspend()), false)
            }
            TrapType::Interrupt(InterruptKind::Jump) => (WorkerStatus::Running, None, false),
            TrapType::Interrupt(InterruptKind::Restart) => (WorkerStatus::Running, None, false),
            TrapType::Exit => (WorkerStatus::Exited, Some(OplogEntry::exited()), true),
            TrapType::Error(WorkerError::InvalidRequest(_)) => (WorkerStatus::Running, None, true),
            TrapType::Error(error) => {
                let status = if is_worker_error_retriable(&retry_config, error, previous_tries) {
                    WorkerStatus::Retrying
                } else {
                    WorkerStatus::Failed
                };
                let store_error = status == WorkerStatus::Failed;
                (status, Some(OplogEntry::error(error.clone())), store_error)
            }
        };

        let oplog_idx = if let Some(entry) = oplog_entry {
            let oplog_idx = self.state.oplog.add_and_commit(entry).await;
            Some(oplog_idx)
        } else {
            None
        };

        self.store_worker_status(updated_worker_status.clone())
            .await;

        if store_result {
            // Giving up, associating the stored result with the current and upcoming invocations
            if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                self.public_state
                    .worker()
                    .store_invocation_failure(
                        &idempotency_key,
                        trap_type,
                        oplog_idx.unwrap_or(OplogIndex::NONE),
                    )
                    .await;
            }
        }

        decision
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: TypeAnnotatedValue,
    ) -> Result<(), GolemError> {
        let is_live_after = self.state.is_live();

        if is_live_after {
            if self.state.snapshotting_mode.is_none() {
                self.state
                    .oplog
                    .add_exported_function_completed(&output, consumed_fuel)
                    .await
                    .unwrap_or_else(|err| {
                        panic!("could not encode function result for {full_function_name}: {err}")
                    });
                self.state.oplog.commit(CommitLevel::Always).await;
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
                .replay_state
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

    async fn add(&mut self, resource: ResourceAny) -> u64 {
        let id = self.state.add(resource).await;
        let resource_id = WorkerResourceId(id);
        if self.state.is_live() {
            let entry = OplogEntry::create_resource(resource_id);
            self.state.oplog.add(entry.clone()).await;
            self.update_worker_status(move |status| {
                status.owned_resources.insert(
                    resource_id,
                    WorkerResourceDescription {
                        created_at: entry.timestamp(),
                        indexed_resource_key: None,
                    },
                );
            })
            .await;
        }
        id
    }

    async fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        let result = self.state.borrow(resource_id).await;
        if result.is_some() {
            let id = WorkerResourceId(resource_id);
            if self.state.is_live() {
                self.state.oplog.add(OplogEntry::drop_resource(id)).await;
                self.update_worker_status(move |status| {
                    status.owned_resources.remove(&id);
                })
                .await;
            }
        }
        result
    }

    async fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.state.borrow(resource_id).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> UpdateManagement for DurableWorkerCtx<Ctx> {
    fn begin_call_snapshotting_function(&mut self) {
        // While calling a snapshotting function (load/save), we completely turn off persistence
        // In addition to the user-controllable persistence level we also skip writing the
        // oplog entries marking the exported function call.
        let previous_level = self.state.persistence_level;
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
        new_active_plugins: HashSet<PluginInstallationId>,
    ) {
        info!("Worker update to {} finished successfully", target_version);

        let entry = OplogEntry::successful_update(
            target_version,
            new_component_size,
            new_active_plugins.clone(),
        );
        let timestamp = entry.timestamp();
        self.public_state.oplog.add_and_commit(entry).await;
        self.update_worker_status(|status| {
            status.component_version = target_version;
            status.successful_updates.push(SuccessfulUpdateRecord {
                timestamp,
                target_version,
            });
            *status.active_plugins_mut() = new_active_plugins;
        })
        .await;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> IndexedResourceStore for DurableWorkerCtx<Ctx> {
    fn get_indexed_resource(
        &self,
        resource_name: &str,
        resource_params: &[String],
    ) -> Option<WorkerResourceId> {
        let key = IndexedResourceKey {
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.get(&key).copied()
    }

    async fn store_indexed_resource(
        &mut self,
        resource_name: &str,
        resource_params: &[String],
        resource: WorkerResourceId,
    ) {
        let key = IndexedResourceKey {
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.insert(key.clone(), resource);
        if self.state.is_live() {
            self.state
                .oplog
                .add(OplogEntry::describe_resource(resource, key.clone()))
                .await;
            self.update_worker_status(|status| {
                if let Some(description) = status.owned_resources.get_mut(&resource) {
                    description.indexed_resource_key = Some(key);
                }
            })
            .await;
        }
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

    async fn compute_latest_worker_status<T: HasOplogService + HasConfig + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        calculate_last_known_status(this, owned_worker_id, metadata).await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        instance: &Instance,
    ) -> Result<RetryDecision, GolemError> {
        let mut number_of_replayed_functions = 0;

        let resume_result = loop {
            let cont = store.as_context().data().durable_ctx().state.is_replay();

            if cont {
                let oplog_entry = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .replay_state
                    .get_oplog_entry_exported_function_invoked()
                    .await;
                match oplog_entry {
                    Err(error) => break Err(error),
                    Ok(None) => break Ok(RetryDecision::None),
                    Ok(Some((function_name, function_input, idempotency_key))) => {
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
                        )
                        .instrument(span)
                        .await;

                        match invoke_result {
                            Ok(InvokeResult::Succeeded {
                                output,
                                consumed_fuel,
                            }) => {
                                let component_metadata =
                                    store.as_context().data().component_metadata();

                                match exports::function_by_name(
                                    &component_metadata.exports,
                                    &full_function_name,
                                ) {
                                    Ok(value) => {
                                        if let Some(value) = value {
                                            let result =
                                                interpret_function_results(output, value.results)
                                                    .map_err(|e| GolemError::ValueMismatch {
                                                    details: e.join(", "),
                                                })?;
                                            if let Err(err) = store
                                                .as_context_mut()
                                                .data_mut()
                                                .on_invocation_success(
                                                    &full_function_name,
                                                    &function_input,
                                                    consumed_fuel,
                                                    result,
                                                )
                                                .await
                                            {
                                                break Err(err);
                                            }
                                        } else {
                                            let trap_type = TrapType::Error(
                                                WorkerError::InvalidRequest(format!(
                                                    "Function {full_function_name} not found"
                                                )),
                                            );

                                            let _ = store
                                                .as_context_mut()
                                                .data_mut()
                                                .on_invocation_failure(&trap_type)
                                                .await;

                                            break Err(GolemError::invalid_request(format!(
                                                "Function {full_function_name} not found"
                                            )));
                                        }
                                    }
                                    Err(err) => {
                                        let trap_type =
                                            TrapType::Error(WorkerError::InvalidRequest(format!(
                                                "Function {full_function_name} not found: {err}"
                                            )));

                                        let _ = store
                                            .as_context_mut()
                                            .data_mut()
                                            .on_invocation_failure(&trap_type)
                                            .await;

                                        break Err(GolemError::invalid_request(format!(
                                            "Function {full_function_name} not found: {err}"
                                        )));
                                    }
                                }
                                number_of_replayed_functions += 1;
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

                                        if decision == RetryDecision::None {
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
                                                    let stderr = store
                                                        .as_context()
                                                        .data()
                                                        .get_public_state()
                                                        .event_service()
                                                        .get_last_invocation_errors();
                                                    break Err(GolemError::runtime(
                                                        error.to_string(&stderr),
                                                    ));
                                                }
                                            }
                                        }

                                        decision
                                    }
                                    None => RetryDecision::None,
                                };

                                break Ok(decision);
                            }
                        }
                    }
                }
            } else {
                break Ok(RetryDecision::None);
            }
        };

        record_number_of_replayed_functions(number_of_replayed_functions);

        resume_result
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<RetryDecision, GolemError> {
        debug!("Starting prepare_instance");
        let start = Instant::now();
        store.as_context_mut().data_mut().set_running();

        if store
            .as_context()
            .data()
            .component_metadata()
            .component_type
            == ComponentType::Ephemeral
        {
            // Ephemeral workers cannot be recovered

            // Moving to the end of the oplog
            store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .replay_state
                .switch_to_live();

            // Appending a Restart marker
            store
                .as_context_mut()
                .data_mut()
                .get_public_state()
                .oplog()
                .add(OplogEntry::restart())
                .await;

            Ok(RetryDecision::None)
        } else {
            // Handle the case when recovery immediately starts in a deleted region
            // (for example due to a manual update)
            store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .replay_state
                .get_out_of_deleted_region()
                .await;

            let result = Self::resume_replay(store, instance).await;

            record_resume_worker(start.elapsed());

            let final_decision = Self::finalize_pending_update(&result, instance, store).await;

            // The update finalization has the right to override the Err result with an explicit retry request
            if final_decision != RetryDecision::None {
                debug!("Retrying prepare_instance after failed update attempt");
                Ok(final_decision)
            } else {
                store.as_context_mut().data_mut().set_suspended().await?;
                debug!("Finished prepare_instance");
                result.map_err(|err| GolemError::failed_to_resume_worker(worker_id.clone(), err))
            }
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
        this.oplog_processor_plugin()
            .on_shard_assignment_changed()
            .await?;

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
                RetryDecision::Immediate | RetryDecision::ReacquirePermits => {
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
                RetryDecision::Delayed(_) => {
                    panic!("Delayed recovery on startup is not supported currently")
                }
                RetryDecision::None => {}
            }
        }

        info!("Finished recovering workers");
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> FileSystemReading for DurableWorkerCtx<Ctx> {
    async fn list_directory(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ListDirectoryResult, GolemError> {
        let root = self._temp_dir.path();
        let target = root.join(PathBuf::from(path.to_rel_string()));

        {
            let exists =
                tokio::fs::try_exists(&target)
                    .await
                    .map_err(|e| GolemError::FileSystemError {
                        path: path.to_string(),
                        reason: format!("Failed to check whether file exists: {e}"),
                    })?;
            if !exists {
                return Ok(ListDirectoryResult::NotFound);
            };
        }

        {
            let metadata =
                tokio::fs::metadata(&target)
                    .await
                    .map_err(|e| GolemError::FileSystemError {
                        path: path.to_string(),
                        reason: format!("Failed to get metadata: {e}"),
                    })?;
            if !metadata.is_dir() {
                return Ok(ListDirectoryResult::NotADirectory);
            };
        }

        let mut entries =
            tokio::fs::read_dir(target)
                .await
                .map_err(|e| GolemError::FileSystemError {
                    path: path.to_string(),
                    reason: format!("Failed to list directory: {e}"),
                })?;

        let mut result = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry
                .metadata()
                .await
                .map_err(|e| GolemError::FileSystemError {
                    path: path.to_string(),
                    reason: format!("Failed to get file metadata {e}"),
                })?;

            let entry_name = entry.file_name().to_string_lossy().to_string();

            let last_modified = metadata.modified().ok().unwrap_or(SystemTime::UNIX_EPOCH);

            if metadata.is_file() {
                let is_readonly_by_host = metadata.permissions().readonly();
                // additionally consider permissions we maintain ourselves
                let is_readonly_by_us = self
                    .read_only_paths
                    .read()
                    .expect("There should be no writers to read_only_paths")
                    .contains(&entry.path());

                let permissions = if is_readonly_by_host || is_readonly_by_us {
                    ComponentFilePermissions::ReadOnly
                } else {
                    ComponentFilePermissions::ReadWrite
                };

                result.push(ComponentFileSystemNode {
                    name: entry_name,
                    last_modified,
                    details: ComponentFileSystemNodeDetails::File {
                        size: metadata.len(),
                        permissions,
                    },
                });
            } else {
                result.push(ComponentFileSystemNode {
                    name: entry_name,
                    last_modified,
                    details: ComponentFileSystemNodeDetails::Directory,
                });
            };
        }
        Ok(ListDirectoryResult::Ok(result))
    }

    async fn read_file(&self, path: &ComponentFilePath) -> Result<ReadFileResult, GolemError> {
        let root = self._temp_dir.path();
        let target = root.join(PathBuf::from(path.to_rel_string()));

        {
            let exists =
                tokio::fs::try_exists(&target)
                    .await
                    .map_err(|e| GolemError::FileSystemError {
                        path: path.to_string(),
                        reason: format!("Failed to check whether file exists: {e}"),
                    })?;
            if !exists {
                return Ok(ReadFileResult::NotFound);
            };
        }

        {
            let metadata =
                tokio::fs::metadata(&target)
                    .await
                    .map_err(|e| GolemError::FileSystemError {
                        path: path.to_string(),
                        reason: format!("Failed to get metadata: {e}"),
                    })?;
            if !metadata.is_file() {
                return Ok(ReadFileResult::NotAFile);
            };
        }

        let path_clone = path.clone();

        let stream = tokio::fs::File::open(target)
            .map_ok(|file| FramedRead::new(file, BytesCodec::new()).map_ok(BytesMut::freeze))
            .try_flatten_stream()
            .map_err(move |e| GolemError::FileSystemError {
                path: path_clone.to_string(),
                reason: format!("Failed to open file: {e}"),
            });

        Ok(ReadFileResult::Ok(Box::pin(stream)))
    }
}

async fn last_error_and_retry_count<T: HasOplogService + HasConfig>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
) -> Option<LastError> {
    let mut idx = this.oplog_service().get_last_index(owned_worker_id).await;
    let mut retry_count = 0;
    if idx == OplogIndex::NONE {
        None
    } else {
        let mut first_error = None;
        let mut last_error_index = idx;
        let result = loop {
            let oplog_entry = this.oplog_service().read(owned_worker_id, idx, 1).await;
            match oplog_entry.first_key_value() {
                Some((_, OplogEntry::Error { error, .. })) => {
                    retry_count += 1;
                    last_error_index = idx;
                    if first_error.is_none() {
                        first_error = Some(error.clone());
                    }
                    if idx > OplogIndex::INITIAL {
                        idx = idx.previous();
                        continue;
                    } else {
                        break Some(LastError {
                            error: first_error.unwrap(),
                            retry_count,
                            stderr: recover_stderr_logs(this, owned_worker_id, last_error_index)
                                .await,
                        });
                    }
                }
                Some((_, entry)) if entry.is_hint() => {
                    // Skipping hint entries as they can randomly interleave the error entries (such as incoming invocation requests, etc)
                    if idx > OplogIndex::INITIAL {
                        idx = idx.previous();
                        continue;
                    } else {
                        match first_error {
                            Some(error) => {
                                break Some(LastError {
                                    error,
                                    retry_count,
                                    stderr: recover_stderr_logs(
                                        this,
                                        owned_worker_id,
                                        last_error_index,
                                    )
                                    .await,
                                })
                            }
                            None => break None,
                        }
                    }
                }
                Some((_, _)) => match first_error {
                    Some(error) => {
                        break Some(LastError {
                            error,
                            retry_count,
                            stderr: recover_stderr_logs(this, owned_worker_id, last_error_index)
                                .await,
                        })
                    }
                    None => break None,
                },
                None => {
                    // This is possible if the oplog has been deleted between the get_last_index and the read call
                    break None;
                }
            }
        };
        result
    }
}

/// Reads back oplog entries starting from `last_oplog_idx` and collects stderr logs, with a maximum
/// number of entries, and at most until the first invocation start entry.
pub(crate) async fn recover_stderr_logs<T: HasOplogService + HasConfig>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    last_oplog_idx: OplogIndex,
) -> String {
    let max_count = this.config().limits.event_history_size;
    let mut idx = last_oplog_idx;
    let mut stderr_entries = Vec::new();
    loop {
        // TODO: this could be read in batches to speed up the process
        let oplog_entry = this.oplog_service().read(owned_worker_id, idx, 1).await;
        match oplog_entry.first_key_value() {
            Some((
                _,
                OplogEntry::Log {
                    level: LogLevel::Stderr,
                    message,
                    ..
                },
            )) => {
                stderr_entries.push(message.clone());
                if stderr_entries.len() >= max_count {
                    break;
                }
            }
            Some((_, OplogEntry::ExportedFunctionInvoked { .. })) => break,
            _ => {}
        }
        if idx > OplogIndex::INITIAL {
            idx = idx.previous();
        } else {
            break;
        }
    }
    stderr_entries.reverse();
    stderr_entries.join("")
}

/// Indicates which step of the http request handling is responsible for closing an open
/// http request (by calling end_function)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HttpRequestCloseOwner {
    FutureIncomingResponseDrop,
    IncomingResponseDrop,
    IncomingBodyDropOrFinish,
    InputStreamClosed,
}

/// State associated with ongoing http requests, on top of the underlying wasi-http implementation
#[derive(Debug, Clone)]
struct HttpRequestState {
    /// Who is responsible for calling end_function and removing entries from the table
    pub close_owner: HttpRequestCloseOwner,
    /// The handle of the FutureIncomingResponse that is registered into the open_function_table
    pub root_handle: u32,
    /// Information about the request to be included in the oplog
    pub request: SerializableHttpRequest,
}

pub struct PrivateDurableWorkerState<Owner: PluginOwner, Scope: PluginScope> {
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    oplog: Arc<dyn Oplog + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    key_value_service: Arc<dyn KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
    component_service: Arc<dyn ComponentService + Send + Sync>,
    plugins: Arc<dyn Plugins<Owner, Scope> + Send + Sync>,
    config: Arc<GolemConfig>,
    owned_worker_id: OwnedWorkerId,
    current_idempotency_key: Option<IdempotencyKey>,
    rpc: Arc<dyn Rpc + Send + Sync>,
    worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
    resources: HashMap<WorkerResourceId, ResourceAny>,
    last_resource_id: WorkerResourceId,
    replay_state: ReplayState,
    overridden_retry_policy: Option<RetryConfig>,
    persistence_level: PersistenceLevel,
    assume_idempotence: bool,
    open_function_table: HashMap<u32, OplogIndex>,

    /// State of ongoing http requests, key is the resource id it is most recently associated with (one state object can belong to multiple resources, but just one at once)
    open_http_requests: HashMap<u32, HttpRequestState>,

    snapshotting_mode: Option<PersistenceLevel>,

    indexed_resources: HashMap<IndexedResourceKey, WorkerResourceId>,
    component_metadata: ComponentMetadata,

    total_linear_memory_size: u64,
}

impl<Owner: PluginOwner, Scope: PluginScope> PrivateDurableWorkerState<Owner, Scope> {
    pub async fn new(
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
        component_service: Arc<dyn ComponentService + Send + Sync>,
        plugins: Arc<dyn Plugins<Owner, Scope> + Send + Sync>,
        config: Arc<GolemConfig>,
        owned_worker_id: OwnedWorkerId,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        deleted_regions: DeletedRegions,
        last_oplog_index: OplogIndex,
        component_metadata: ComponentMetadata,
        total_linear_memory_size: u64,
    ) -> Self {
        let replay_state = ReplayState::new(
            owned_worker_id.clone(),
            oplog_service.clone(),
            oplog.clone(),
            deleted_regions,
            last_oplog_index,
        )
        .await;
        Self {
            oplog_service,
            oplog: oplog.clone(),
            promise_service,
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            component_service,
            plugins,
            config,
            owned_worker_id,
            current_idempotency_key: None,
            rpc,
            worker_proxy,
            resources: HashMap::new(),
            last_resource_id: WorkerResourceId::INITIAL,
            overridden_retry_policy: None,
            persistence_level: PersistenceLevel::Smart,
            assume_idempotence: true,
            open_function_table: HashMap::new(),
            open_http_requests: HashMap::new(),
            snapshotting_mode: None,
            indexed_resources: HashMap::new(),
            component_metadata,
            total_linear_memory_size,
            replay_state,
        }
    }

    pub async fn begin_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, GolemError> {
        if self.persistence_level != PersistenceLevel::PersistNothing
            && ((*function_type == DurableFunctionType::WriteRemote && !self.assume_idempotence)
                || matches!(
                    *function_type,
                    DurableFunctionType::WriteRemoteBatched(None)
                ))
        {
            if self.is_live() {
                self.oplog
                    .add_and_commit(OplogEntry::begin_remote_write())
                    .await;
                let begin_index = self.oplog.current_oplog_index().await;
                Ok(begin_index)
            } else {
                let (begin_index, _) =
                    crate::get_oplog_entry!(self.replay_state, OplogEntry::BeginRemoteWrite)?;
                if !self.assume_idempotence {
                    let end_index = self
                        .replay_state
                        .lookup_oplog_entry(begin_index, OplogEntry::is_end_remote_write)
                        .await;
                    if end_index.is_none() {
                        // Must switch to live mode before failing to be able to commit an Error entry
                        self.replay_state.switch_to_live();
                        Err(GolemError::runtime(
                            "Non-idempotent remote write operation was not completed, cannot retry",
                        ))
                    } else {
                        Ok(begin_index)
                    }
                } else if matches!(
                    *function_type,
                    DurableFunctionType::WriteRemoteBatched(None)
                ) {
                    let end_index = self
                        .replay_state
                        .lookup_oplog_entry_with_condition(
                            begin_index,
                            OplogEntry::is_end_remote_write,
                            OplogEntry::no_concurrent_side_effect,
                        )
                        .await;
                    if end_index.is_none() {
                        // We need to jump to the end of the oplog
                        self.replay_state.switch_to_live();

                        // But this is not enough, because if the retried batched write operation succeeds,
                        // and later we replay it, we need to skip the first attempt and only replay the second.
                        // Se we add a Jump entry to the oplog that registers a deleted region.
                        let deleted_region = OplogRegion {
                            start: begin_index.next(), // need to keep the BeginAtomicRegion entry
                            end: self.replay_state.replay_target().next(), // skipping the Jump entry too
                        };
                        self.replay_state
                            .add_deleted_region(deleted_region.clone())
                            .await;
                        self.oplog
                            .add_and_commit(OplogEntry::jump(deleted_region))
                            .await;
                    }

                    Ok(begin_index)
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
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Result<(), GolemError> {
        if self.persistence_level != PersistenceLevel::PersistNothing
            && ((*function_type == DurableFunctionType::WriteRemote && !self.assume_idempotence)
                || matches!(
                    *function_type,
                    DurableFunctionType::WriteRemoteBatched(None)
                ))
        {
            if self.is_live() {
                self.oplog
                    .add(OplogEntry::end_remote_write(begin_index))
                    .await;
                Ok(())
            } else {
                let (_, _) =
                    crate::get_oplog_entry!(self.replay_state, OplogEntry::EndRemoteWrite)?;
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
            self.replay_state.last_replayed_index()
        }
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.replay_state.is_live()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
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
impl<Owner: PluginOwner, Scope: PluginScope> ResourceStore
    for PrivateDurableWorkerState<Owner, Scope>
{
    fn self_uri(&self) -> Uri {
        Uri::golem_urn(&self.owned_worker_id.worker_id, None)
    }

    async fn add(&mut self, resource: ResourceAny) -> u64 {
        let id = self.last_resource_id;
        self.last_resource_id = self.last_resource_id.next();
        self.resources.insert(id, resource);
        id.0
    }

    async fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        let resource_id = WorkerResourceId(resource_id);
        self.resources.remove(&resource_id)
    }

    async fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.resources.get(&WorkerResourceId(resource_id)).cloned()
    }
}

impl<Owner: PluginOwner, Scope: PluginScope> HasOplogService
    for PrivateDurableWorkerState<Owner, Scope>
{
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl<Owner: PluginOwner, Scope: PluginScope> HasOplog for PrivateDurableWorkerState<Owner, Scope> {
    fn oplog(&self) -> Arc<dyn Oplog + Send + Sync> {
        self.oplog.clone()
    }
}

impl<Owner: PluginOwner, Scope: PluginScope> HasConfig for PrivateDurableWorkerState<Owner, Scope> {
    fn config(&self) -> Arc<GolemConfig> {
        self.config.clone()
    }
}

impl<Owner: PluginOwner, Scope: PluginScope> HasPlugins<Owner, Scope>
    for PrivateDurableWorkerState<Owner, Scope>
{
    fn plugins(&self) -> Arc<dyn Plugins<Owner, Scope> + Send + Sync> {
        self.plugins.clone()
    }
}

pub struct PublicDurableWorkerState<Ctx: WorkerCtx> {
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
    invocation_queue: Weak<Worker<Ctx>>,
    oplog: Arc<dyn Oplog + Send + Sync>,
}

impl<Ctx: WorkerCtx> Clone for PublicDurableWorkerState<Ctx> {
    fn clone(&self) -> Self {
        Self {
            promise_service: self.promise_service.clone(),
            event_service: self.event_service.clone(),
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
impl<Ctx: WorkerCtx> WasiView for DurableWorkerCtxWasiView<'_, Ctx> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        self.0.ctx()
    }
}

impl<Ctx: WorkerCtx> WasiHttpView for DurableWorkerCtxWasiHttpView<'_, Ctx> {
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

async fn prepare_filesystem(
    file_loader: Arc<FileLoader>,
    account_id: &AccountId,
    root: &Path,
    files: &[InitialComponentFile],
) -> Result<(Vec<FileUseToken>, HashSet<PathBuf>), GolemError> {
    let futures = files.iter().map(|file| {
        let path = root.join(PathBuf::from(file.path.to_rel_string()));
        let key = file.key.clone();
        let permissions = file.permissions;
        let file_loader = file_loader.clone();
        async move {
            match permissions {
                ComponentFilePermissions::ReadOnly => {
                    debug!("Loading read-only file {}", path.display());
                    let token = file_loader
                        .get_read_only_to(account_id, &key, &path)
                        .await?;
                    Ok::<_, GolemError>(Some((token, path)))
                }
                ComponentFilePermissions::ReadWrite => {
                    debug!("Loading read-write file {}", path.display());
                    file_loader
                        .get_read_write_to(account_id, &key, &path)
                        .await?;
                    Ok(None)
                }
            }
        }
    });

    let results = try_join_all(futures).await?;

    let mut read_only_files = HashSet::with_capacity(files.len());
    let mut file_use_tokens = Vec::new();

    for (token, path) in results.into_iter().flatten() {
        read_only_files.insert(path);
        file_use_tokens.push(token);
    }
    Ok((file_use_tokens, read_only_files))
}

/// Helper macro for expecting a given type of OplogEntry as the next entry in the oplog during
/// replay, while skipping hint entries.
/// The macro expression's type is `Result<OplogEntry, GolemError>` and it fails if the next non-hint
/// entry was not the expected one.
#[macro_export]
macro_rules! get_oplog_entry {
    ($private_state:expr, $($cases:path),+) => {
        loop {
            let (oplog_index, oplog_entry) = $private_state.get_oplog_entry().await;
            match oplog_entry {
                $($cases { .. } => {
                    break Ok((oplog_index, oplog_entry));
                })+
                entry if entry.is_hint() => {}
                _ => {
                    break Err($crate::error::GolemError::unexpected_oplog_entry(
                        stringify!($($cases |)+),
                        format!("{:?}", oplog_entry),
                    ));
                }
            }
        }
    };
}
