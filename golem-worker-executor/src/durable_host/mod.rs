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

// WASI Host implementation for Golem, delegating to the core WASI implementation (wasmtime_wasi)
// implementing the Golem specific instrumentation on top of it.

pub mod blobstore;
mod cli;
mod clocks;
mod config;
pub mod durability;
mod dynamic_linking;
mod filesystem;
pub mod golem;
pub mod http;
pub mod io;
pub mod keyvalue;
mod logging;
mod random;
pub mod rdbms;
mod replay_state;
pub mod serialized;
mod sockets;
pub mod wasm_rpc;

use crate::durable_host::http::serialized::SerializableHttpRequest;
use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::replay_state::ReplayState;
use crate::durable_host::serialized::SerializableError;
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::model::event::InternalWorkerEvent;
use crate::model::{
    CurrentResourceLimits, ExecutionStatus, InvocationContext, LastError, ReadFileResult, TrapType,
    WorkerConfig,
};
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentService;
use crate::services::file_loader::{FileLoader, FileUseToken};
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, OplogService};
use crate::services::plugins::Plugins;
use crate::services::projects::ProjectService;
use crate::services::promise::PromiseService;
use crate::services::rdbms::RdbmsService;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_fork::WorkerForkService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{
    worker_enumeration, HasAll, HasConfig, HasOplog, HasProjectService, HasWorker,
};
use crate::services::{HasOplogService, HasPlugins};
use crate::wasi_host;
use crate::worker::invocation::{invoke_observed_and_traced, InvokeResult};
use crate::worker::status::calculate_last_known_status;
use crate::worker::{interpret_function_result, is_worker_error_retriable, RetryDecision, Worker};
use crate::workerctx::{
    AgentStore, ExternalOperations, FileSystemReading, IndexedResourceStore,
    InvocationContextManagement, InvocationHooks, InvocationManagement, LogEventEmitBehaviour,
    PublicWorkerIo, StatusManagement, UpdateManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::BytesMut;
use chrono::{DateTime, Utc};
pub use durability::*;
use futures::future::try_join_all;
use futures::TryFutureExt;
use futures::TryStreamExt;
use golem_common::model::agent::DataValue;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::{
    DurableFunctionType, IndexedResourceKey, LogLevel, OplogEntry, OplogIndex, PersistenceLevel,
    TimestampedUpdateDescription, UpdateDescription, WorkerError, WorkerResourceId,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AccountId, AgentInstanceDescription, AgentInstanceKey, ExportedResourceInstanceDescription,
    ExportedResourceInstanceKey, PluginInstallationId, ProjectId, WorkerResourceKey,
};
use golem_common::model::{
    ComponentFilePath, ComponentFilePermissions, ComponentFileSystemNode,
    ComponentFileSystemNodeDetails, ComponentId, ComponentType, ComponentVersion,
    FailedUpdateRecord, GetFileSystemNodeResult, IdempotencyKey, InitialComponentFile,
    OwnedWorkerId, ScanCursor, ScheduledAction, SuccessfulUpdateRecord, Timestamp, WorkerFilter,
    WorkerId, WorkerMetadata, WorkerResourceDescription, WorkerStatus, WorkerStatusRecord,
};
use golem_common::model::{RetryConfig, TargetWorkerId};
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_wasm_rpc::wasmtime::{ResourceStore, ResourceTypeId};
use golem_wasm_rpc::{Uri, Value, ValueAndType};
use replay_state::ReplayEvent;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Add;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use std::vec;
use tempfile::TempDir;
use tokio::sync::RwLock as TRwLock;
use tokio_util::codec::{BytesCodec, FramedRead};
use tracing::{debug, info, span, warn, Instrument, Level};
use wasmtime::component::{Instance, Resource, ResourceAny};
use wasmtime::{AsContext, AsContextMut};
use wasmtime_wasi::p2::bindings::filesystem::preopens::Descriptor;
use wasmtime_wasi::p2::{FsResult, Stderr, Stdout, WasiCtx, WasiImpl, WasiView};
use wasmtime_wasi::{I32Exit, IoCtx, IoImpl, IoView, ResourceTable, ResourceTableError};
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::types::{
    default_send_request, HostFutureIncomingResponse, OutgoingRequestConfig,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpImpl, WasiHttpView};

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: Arc<Mutex<ResourceTable>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi: Arc<Mutex<WasiCtx>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    io_ctx: Arc<Mutex<IoCtx>>,
    wasi_http: WasiHttpCtx,
    pub owned_worker_id: OwnedWorkerId,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState,
    temp_dir: Arc<TempDir>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        owned_worker_id: OwnedWorkerId,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Ctx>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins>,
        worker_fork: Arc<dyn WorkerForkService>,
        project_service: Arc<dyn ProjectService>,
    ) -> Result<Self, WorkerExecutorError> {
        let temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
            |e| WorkerExecutorError::runtime(format!("Failed to create temporary directory: {e}")),
        )?);
        debug!(
            "Created temporary file system root at {:?}",
            temp_dir.path()
        );

        debug!(
            "Worker {} initialized with deleted regions {}",
            owned_worker_id.worker_id, worker_config.deleted_regions
        );

        debug!(
            "Worker {} starting replay from component version {}",
            owned_worker_id.worker_id, worker_config.component_version_for_replay
        );

        let component_metadata = component_service
            .get_metadata(
                &owned_worker_id.project_id,
                &owned_worker_id.component_id(),
                Some(worker_config.component_version_for_replay),
            )
            .await?;

        let files = prepare_filesystem(
            &file_loader,
            &owned_worker_id.project_id,
            temp_dir.path(),
            &component_metadata.files,
        )
        .await?;

        // TODO: pass config vars from component metadata
        let wasi_config_vars = effective_wasi_config_vars(
            worker_config.initial_wasi_config_vars.clone(),
            BTreeMap::new(),
        );

        let stdin = ManagedStdIn::disabled();
        let stdout = ManagedStdOut::from_stdout(Stdout);
        let stderr = ManagedStdErr::from_stderr(Stderr);

        let last_oplog_index = oplog.current_oplog_index().await;

        let (wasi, io_ctx, table) = wasi_host::create_context(
            &worker_config.args,
            &worker_config.env,
            temp_dir.path().to_path_buf(),
            stdin,
            stdout,
            stderr,
            |duration| anyhow!(SuspendForSleep(duration)),
            config.suspend.suspend_after,
        )
        .map_err(|e| WorkerExecutorError::runtime(format!("Could not create WASI context: {e}")))?;
        let wasi_http = WasiHttpCtx::new();
        Ok(DurableWorkerCtx {
            table: Arc::new(Mutex::new(table)),
            wasi: Arc::new(Mutex::new(wasi)),
            io_ctx: Arc::new(Mutex::new(io_ctx)),
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
                rdbms_service,
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
                worker_fork,
                RwLock::new(compute_read_only_paths(&files)),
                TRwLock::new(files),
                file_loader,
                project_service,
                worker_config.created_by.clone(),
                worker_config.initial_wasi_config_vars,
                wasi_config_vars,
            )
            .await,
            temp_dir,
            execution_status,
        })
    }

    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    fn check_if_file_is_readonly(
        &mut self,
        fd: &Resource<Descriptor>,
    ) -> Result<bool, ResourceTableError> {
        let table = Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");

        match table.get(fd)? {
            Descriptor::File(f) => {
                let read_only = self.state.read_only_paths.read().unwrap().contains(&f.path);

                Ok(read_only)
            }
            Descriptor::Dir(_) => Ok(false),
        }
    }

    fn fail_if_read_only(&mut self, fd: &Resource<Descriptor>) -> FsResult<()> {
        if self.check_if_file_is_readonly(fd)? {
            Err(wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NotPermitted.into())
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

    fn io_ctx(&mut self) -> &mut IoCtx {
        Arc::get_mut(&mut self.io_ctx)
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

    pub fn created_by(&self) -> &AccountId {
        &self.state.created_by
    }

    pub fn component_metadata(&self) -> &golem_service_base::model::Component {
        &self.state.component_metadata
    }

    pub fn is_exit(error: &anyhow::Error) -> Option<i32> {
        error
            .root_cause()
            .downcast_ref::<I32Exit>()
            .map(|exit| exit.0)
    }

    pub fn as_wasi_view(&mut self) -> WasiImpl<DurableWorkerCtxWasiView<'_, Ctx>> {
        WasiImpl(IoImpl(DurableWorkerCtxWasiView(self)))
    }

    pub fn as_wasi_http_view(&mut self) -> WasiHttpImpl<DurableWorkerCtxWasiHttpView<'_, Ctx>> {
        WasiHttpImpl(IoImpl(DurableWorkerCtxWasiHttpView(self)))
    }

    pub fn get_worker_status_record(&self) -> WorkerStatusRecord {
        self.execution_status
            .read()
            .unwrap()
            .last_known_status()
            .clone()
    }

    pub async fn update_worker_status(&self, f: impl FnOnce(&mut WorkerStatusRecord)) {
        let mut status = self.get_worker_status_record();

        let mut skipped_regions = self.state.replay_state.skipped_regions().await;
        let (pending_updates, extra_skipped_regions) =
            self.public_state.worker().pending_updates().await;
        skipped_regions.set_override(extra_skipped_regions);

        status.skipped_regions = skipped_regions;
        status
            .overridden_retry_config
            .clone_from(&self.state.overridden_retry_policy);
        status.pending_invocations = self.public_state.worker().pending_invocations().await;
        status.invocation_results = self.public_state.worker().invocation_results().await;
        status.pending_updates = pending_updates;
        status
            .current_idempotency_key
            .clone_from(&self.state.current_idempotency_key);
        status.total_linear_memory_size = self.state.total_linear_memory_size;
        status.oplog_idx = self.state.oplog.current_oplog_index().await;
        f(&mut status);
        self.public_state.worker().update_status(status).await;
    }

    pub fn rpc(&self) -> Arc<dyn Rpc> {
        self.state.rpc.clone()
    }

    pub fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.state.worker_proxy.clone()
    }

    pub fn component_service(&self) -> Arc<dyn ComponentService> {
        self.state.component_service.clone()
    }

    pub fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.state.worker_fork.clone()
    }

    pub fn scheduler_service(&self) -> Arc<dyn SchedulerService> {
        self.state.scheduler_service.clone()
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
            TrapType::Error(WorkerError::OutOfMemory) => RetryDecision::ReacquirePermits,
            TrapType::Error(WorkerError::InvalidRequest(_)) => RetryDecision::None,
            TrapType::Error(WorkerError::StackOverflow) => RetryDecision::None,
            TrapType::Error(WorkerError::Unknown(_)) => {
                let retryable = previous_tries < (retry_config.max_attempts as u64);
                if retryable {
                    match get_delay(retry_config, previous_tries) {
                        Some(delay) => RetryDecision::Delayed(delay),
                        None => RetryDecision::None,
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

    async fn emit_log_event(&self, event: InternalWorkerEvent) {
        if let Some(entry) = event.as_oplog_entry() {
            if let OplogEntry::Log {
                level,
                context,
                message,
                ..
            } = &entry
            {
                match Ctx::LOG_EVENT_EMIT_BEHAVIOUR {
                    LogEventEmitBehaviour::LiveOnly => {
                        // Stdout and stderr writes are persistent and overwritten by sending the data to the event
                        // service instead of the real output stream

                        if self.state.is_live()
                        // If the worker is in live mode we always emit events
                        {
                            if !self
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
                    LogEventEmitBehaviour::Always => {
                        self.public_state
                            .event_service
                            .emit_event(event.clone(), true);

                        if self.state.is_live()
                            && !self
                                .state
                                .replay_state
                                .seen_log(*level, context, message)
                                .await
                        {
                            self.state.oplog.add(entry).await;
                        }
                    }
                }
            }
        }
    }

    pub async fn generate_unique_local_worker_id(
        &mut self,
        remote_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, WorkerExecutorError> {
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

    /// Counts the number of Error entries that are at the end of the oplog. This equals to the number of retries that have been attempted.
    /// It also returns the last error stored in these entries.
    pub async fn trailing_error_count(&self) -> u64 {
        let status = self.get_worker_status_record();
        last_error_and_retry_count(&self.state, &self.owned_worker_id, &status)
            .await
            .map(|last_error| last_error.retry_count)
            .unwrap_or_default()
    }

    pub fn wasi_config_vars(&self) -> BTreeMap<String, String> {
        self.state.wasi_config_vars.read().unwrap().clone()
    }
}

impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> DurableWorkerCtx<Ctx> {
    pub async fn finalize_pending_snapshot_update(
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> RetryDecision {
        let pending_update = store
            .as_context()
            .data()
            .durable_ctx()
            .public_state
            .worker()
            .pop_pending_update()
            .await;
        match pending_update {
            Some(TimestampedUpdateDescription {
                description: description @ UpdateDescription::SnapshotBased { .. },
                ..
            }) => {
                let target_version = *description.target_version();

                match store
                    .as_context_mut()
                    .data_mut()
                    .get_public_state()
                    .oplog()
                    .get_upload_description_payload(&description)
                    .await
                {
                    Ok(Some(data)) => {
                        let component_metadata = store
                            .as_context()
                            .data()
                            .component_metadata()
                            .metadata
                            .clone();

                        let failed = match component_metadata.load_snapshot().await {
                            Ok(Some(load_snapshot)) => {
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
                                let load_result = invoke_observed_and_traced(
                                    load_snapshot.name.to_string(),
                                    vec![Value::List(data.iter().map(|b| Value::U8(*b)).collect())],
                                    store,
                                    instance,
                                    &component_metadata,
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
                                        if let Some(output) = output {
                                            match output {
                                                Value::Result(Err(Some(boxed_error_value))) => {
                                                    match &*boxed_error_value {
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
                            }
                            Ok(None) => {
                                Some("Failed to find exported load-snapshot function".to_string())
                            }
                            Err(err) => Some(format!(
                                "Failed to find exported load-snapshot function: {err}"
                            )),
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
                                    &description,
                                    component_metadata.component_size,
                                    HashSet::from_iter(
                                        component_metadata
                                            .installed_plugins
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
            }
            _ => {
                panic!("`finalize_pending_snapshot_update` can only be called with a snapshot update description")
            }
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn process_pending_replay_events(&mut self) -> Result<(), WorkerExecutorError> {
        debug!("Applying pending side effects accumulated during replay");

        let replay_events = self.state.replay_state.take_new_replay_events().await;
        for event in replay_events {
            match event {
                ReplayEvent::UpdateReplayed { new_version } => {
                    debug!("Updating worker state to component metadata version {new_version}");
                    self.update_state_to_new_component_version(new_version)
                        .await?;
                }
                ReplayEvent::ReplayFinished => {
                    debug!("Replaying oplog finished");

                    let pending_update = self.public_state.worker().peek_pending_update().await;

                    let pending_update = if let Some(pending_update) = pending_update {
                        pending_update
                    } else {
                        continue;
                    };

                    match pending_update.description {
                        UpdateDescription::Automatic { target_version } => {
                            debug!("Finalizing pending automatic update");
                            self.public_state.worker().pop_pending_update().await;

                            if let Err(error) = self
                                .update_state_to_new_component_version(target_version)
                                .await
                            {
                                let stringified_error =
                                    format!("Applying worker update failed: {error}");

                                self.on_worker_update_failed(
                                    target_version,
                                    Some(stringified_error),
                                )
                                .await;

                                Err(error)?
                            };

                            let component_metadata = self.component_metadata().clone();

                            self.on_worker_update_succeeded(
                                &pending_update.description,
                                component_metadata.component_size,
                                HashSet::from_iter(
                                    component_metadata
                                        .installed_plugins
                                        .into_iter()
                                        .map(|installation| installation.id),
                                ),
                            )
                            .await;

                            debug!("Finalizing automatic update to version {target_version}");
                        }
                        _ => {
                            panic!("Expected automatic update description")
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn update_state_to_new_component_version(
        &mut self,
        new_version: ComponentVersion,
    ) -> Result<(), WorkerExecutorError> {
        let current_metadata = &self.state.component_metadata;

        if new_version <= current_metadata.versioned_component_id.version {
            debug!("Update {new_version} was already applied, skipping");
            return Ok(());
        };

        let new_metadata = self
            .component_service()
            .get_metadata(
                &self.owned_worker_id.project_id,
                &self.owned_worker_id.component_id(),
                Some(new_version),
            )
            .await?;

        let mut current_files = self.state.files.write().await;
        update_filesystem(
            &mut current_files,
            &self.state.file_loader,
            &self.owned_worker_id.project_id,
            self.temp_dir.path(),
            &new_metadata.files,
        )
        .await?;

        let mut read_only_paths = self.state.read_only_paths.write().unwrap();
        *read_only_paths = compute_read_only_paths(&current_files);

        // TODO: take config vars from component metadata
        let mut wasi_config_vars = self.state.wasi_config_vars.write().unwrap();
        *wasi_config_vars = effective_wasi_config_vars(
            self.state.initial_wasi_config_vars.clone(),
            BTreeMap::new(),
        );

        self.state.component_metadata = new_metadata;

        Ok(())
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

    async fn set_current_invocation_context(
        &mut self,
        invocation_context: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError> {
        let (invocation_context, current_span_id) =
            InvocationContext::from_stack(invocation_context)
                .map_err(WorkerExecutorError::runtime)?;

        self.state.invocation_context.switch_to(invocation_context);
        self.state.current_span_id = current_span_id;

        Ok(())
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.state
            .invocation_context
            .get_stack(&self.state.current_span_id)
            .unwrap()
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
        let execution_status = self.execution_status.read().unwrap();
        match &*execution_status {
            ExecutionStatus::Interrupting { interrupt_kind, .. } => Some(interrupt_kind.clone()),
            _ => None,
        }
    }

    async fn set_suspended(&self) -> Result<(), WorkerExecutorError> {
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
                        account_id: self.state.created_by.clone(),
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
    ) -> Result<(), WorkerExecutorError> {
        if self.state.snapshotting_mode.is_none() {
            let proto_function_input: Vec<golem_wasm_rpc::protobuf::Val> = function_input
                .iter()
                .map(|value| value.clone().into())
                .collect();

            let stack = self.get_current_invocation_context().await;

            self.state
                .oplog
                .add_exported_function_invoked(
                    full_function_name.to_string(),
                    &proto_function_input,
                    self.get_current_idempotency_key().await.ok_or(anyhow!(
                        "No active invocation key is associated with the worker"
                    ))?,
                    stack,
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
        let previous_tries = self.trailing_error_count().await;
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
        output: Option<ValueAndType>,
    ) -> Result<(), WorkerExecutorError> {
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
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
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

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        let id = self.state.add(resource, name.clone()).await;
        let resource_id = WorkerResourceId(id);
        if self.state.is_live() {
            let entry = OplogEntry::create_resource(resource_id, name.clone());
            self.state.oplog.add(entry.clone()).await;
            self.update_worker_status(move |status| {
                status.owned_resources.insert(
                    WorkerResourceKey::ExportedResourceInstanceKey(ExportedResourceInstanceKey {
                        resource_id,
                    }),
                    WorkerResourceDescription::ExportedResourceInstance(
                        ExportedResourceInstanceDescription {
                            created_at: entry.timestamp(),
                            resource_owner: name.owner,
                            resource_name: name.name,
                            resource_params: None,
                        },
                    ),
                );
            })
            .await;
        }
        id
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        let result = self.state.borrow(resource_id).await;
        if let Some((resource_type_id, _)) = &result {
            let id = WorkerResourceId(resource_id);
            if self.state.is_live() {
                self.state
                    .oplog
                    .add(OplogEntry::drop_resource(id, resource_type_id.clone()))
                    .await;
                self.update_worker_status(move |status| {
                    status
                        .owned_resources
                        .remove(&WorkerResourceKey::ExportedResourceInstanceKey(
                            ExportedResourceInstanceKey { resource_id: id },
                        ));
                })
                .await;
            }
        }
        result
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
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
            });

            // As part of performing a manual update, after the executor called the save-snapshot function
            // it marks the whole history of the worker as "skipped" and reloads the worker with the new
            // version. As the history is skipped it immediately can start with calling load-snapshot on the
            // saved binary. However, if this fails, we have to revert the worker to the original version. As part
            // of this we restore the original set of skipped regions by dropping the override.
            //
            // Note that we can always recalculate the old set of skipped regions from the oplog - the "override layer"
            // is just an optimization so we don't have to.
            if status.skipped_regions.is_overridden() {
                status.skipped_regions.drop_override()
            }
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
        update: &UpdateDescription,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    ) {
        let target_version = *update.target_version();
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
            status.active_plugins = new_active_plugins;

            // As part of performing a manual update, after the executor called the save-snapshot function
            // it marks the whole history of the worker as "skipped" and reloads the worker with the new
            // version. As the history is skipped it immediately can start with calling load-snapshot on the
            // saved binary. Once that succeeds, the update is considered done and we make this skipped region
            // "final" by merging it into the set of skipped regions.
            if status.skipped_regions.is_overridden() {
                status.skipped_regions.merge_override()
            }

            // After a manual update we need to start replays with the new version
            // (as the previous updates that would go from 0 -> target_version are now in deleted regions)
            if let UpdateDescription::SnapshotBased { .. } = update {
                status.component_version_for_replay = target_version
            }
        })
        .await;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> IndexedResourceStore for DurableWorkerCtx<Ctx> {
    fn get_indexed_resource(
        &self,
        resource_owner: &str,
        resource_name: &str,
        resource_params: &[String],
    ) -> Option<WorkerResourceId> {
        let key = IndexedResourceKey {
            resource_owner: resource_owner.to_string(),
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.get(&key).copied()
    }

    async fn store_indexed_resource(
        &mut self,
        resource_owner: &str,
        resource_name: &str,
        resource_params: &[String],
        resource: WorkerResourceId,
    ) {
        let key = IndexedResourceKey {
            resource_owner: resource_owner.to_string(),
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.insert(key.clone(), resource);
        if self.state.is_live() {
            self.state
                .oplog
                .add(OplogEntry::describe_resource(
                    resource,
                    ResourceTypeId {
                        owner: resource_owner.to_string(),
                        name: resource_name.to_string(),
                    },
                    key.resource_params.clone(),
                ))
                .await;
            self.update_worker_status(|status| {
                if let Some(WorkerResourceDescription::ExportedResourceInstance(description)) =
                    status
                        .owned_resources
                        .get_mut(&WorkerResourceKey::ExportedResourceInstanceKey(
                            ExportedResourceInstanceKey {
                                resource_id: resource,
                            },
                        ))
                {
                    description.resource_params = Some(resource_params.to_vec());
                }
            })
            .await;
        }
    }

    fn drop_indexed_resource(
        &mut self,
        resource_owner: &str,
        resource_name: &str,
        resource_params: &[String],
    ) {
        let key = IndexedResourceKey {
            resource_owner: resource_owner.to_string(),
            resource_name: resource_name.to_string(),
            resource_params: resource_params.to_vec(),
        };
        self.state.indexed_resources.remove(&key);
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> AgentStore for DurableWorkerCtx<Ctx> {
    async fn store_agent_instance(
        &mut self,
        agent_type: String,
        agent_id: String,
        parameters: DataValue,
    ) {
        debug!(%agent_type, %agent_id, "Agent instance created");

        let key = AgentInstanceKey {
            agent_type,
            agent_id,
        };
        let description = AgentInstanceDescription {
            created_at: Timestamp::now_utc(),
            agent_parameters: parameters.clone(),
        };
        self.state
            .agent_instances
            .insert(key.clone(), description.clone());
        if self.state.is_live() {
            self.state
                .oplog
                .add(OplogEntry::create_agent_instance(key.clone(), parameters))
                .await;
            self.update_worker_status(|status| {
                status.owned_resources.insert(
                    WorkerResourceKey::AgentInstanceKey(key),
                    WorkerResourceDescription::AgentInstance(description.clone()),
                );
            })
            .await;
        }
    }

    async fn remove_agent_instance(
        &mut self,
        agent_type: String,
        agent_id: String,
        _parameters: DataValue,
    ) {
        debug!(%agent_type, %agent_id, "Agent instance dropped");

        let key = AgentInstanceKey {
            agent_type,
            agent_id,
        };

        self.state.agent_instances.remove(&key);

        if self.state.is_live() {
            self.state
                .oplog
                .add(OplogEntry::drop_agent_instance(key.clone()))
                .await;
            self.update_worker_status(|status| {
                status
                    .owned_resources
                    .remove(&WorkerResourceKey::AgentInstanceKey(key));
            })
            .await;
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationContextManagement for DurableWorkerCtx<Ctx> {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
        let span_id = self.state.current_span_id.clone();
        let span = self.start_child_span(&span_id, initial_attributes).await?;
        self.state.current_span_id = span.span_id().clone();
        Ok(span)
    }

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
        let current_span_id = &self.state.current_span_id;

        let is_live = self.is_live();

        // Using try_get_oplog_entry here to preserve backward compatibility - starting and finishing
        // spans has been added to existing operations (such as wasi-http and rpc) and old oplogs
        // does not have the StartSpan/FinishSpan paris persisted.
        let span = if is_live {
            self.state
                .invocation_context
                .start_span(current_span_id, None)
                .map_err(WorkerExecutorError::runtime)?
        } else if let Some((_, entry)) = self
            .state
            .replay_state
            .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::StartSpan { .. }))
            .await
        {
            let (timestamp, span_id) = match entry {
                OplogEntry::StartSpan {
                    timestamp, span_id, ..
                } => (timestamp, span_id),
                _ => unreachable!(),
            };

            let span = InvocationContextSpan::local()
                .with_span_id(span_id)
                .with_start(timestamp)
                .with_parent(self.state.invocation_context.get(current_span_id).unwrap())
                .build();
            self.state.invocation_context.add_span(span.clone());
            span
        } else {
            self.state
                .invocation_context
                .start_span(current_span_id, None)
                .map_err(WorkerExecutorError::runtime)?
        };

        if current_span_id != parent
            && !self
                .state
                .invocation_context
                .has_in_stack(current_span_id, parent)
        {
            // The parent span is not in the current invocation stack. This can happen if it was created in a previous
            // invocation and stored in some global state.
            // To preserve the current invocation context stack but also have the information from the desired parent
            // span, we add a _link_ to the newly created span.

            self.state
                .invocation_context
                .add_link(span.span_id(), parent)
                .map_err(WorkerExecutorError::runtime)?;
        };

        for (name, value) in initial_attributes {
            span.set_attribute(name.clone(), value.clone());
        }

        if is_live {
            self.state
                .oplog
                .add(OplogEntry::start_span(
                    span.start().unwrap_or(Timestamp::now_utc()),
                    span.span_id().clone(),
                    Some(current_span_id.clone()),
                    span.linked_context().map(|link| link.span_id().clone()),
                    HashMap::from_iter(initial_attributes.iter().cloned()),
                ))
                .await;
        }

        Ok(span)
    }

    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        if &self.state.current_span_id == span_id {
            self.state.current_span_id = self
                .state
                .invocation_context
                .get(span_id)
                .unwrap()
                .parent()
                .map(|p| p.span_id().clone())
                .unwrap_or_else(|| self.state.invocation_context.root.span_id().clone());
        }
        let _ = self
            .state
            .invocation_context
            .finish_span(span_id)
            .map_err(WorkerExecutorError::runtime);
        Ok(())
    }

    async fn finish_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        if self.is_live() {
            self.state
                .oplog
                .add(OplogEntry::finish_span(span_id.clone()))
                .await;
        } else {
            // Using try_get_oplog_entry here to preserve backward compatibility - starting and finishing
            // spans has been added to existing operations (such as wasi-http and rpc) and old oplogs
            // does not have the StartSpan/FinishSpan paris persisted.
            let _ = self
                .state
                .replay_state
                .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::FinishSpan { .. }))
                .await;
        }

        if &self.state.current_span_id == span_id {
            self.state.current_span_id = self
                .state
                .invocation_context
                .get(span_id)
                .unwrap()
                .parent()
                .map(|p| p.span_id().clone())
                .unwrap_or_else(|| self.state.invocation_context.root.span_id().clone());
        }
        let _ = self
            .state
            .invocation_context
            .finish_span(span_id)
            .map_err(WorkerExecutorError::runtime);
        Ok(())
    }

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), WorkerExecutorError> {
        self.state
            .invocation_context
            .set_attribute(span_id, key.to_string(), value.clone())
            .map_err(WorkerExecutorError::runtime)?;
        if self.is_live() {
            self.state
                .oplog
                .add(OplogEntry::set_span_attribute(
                    span_id.clone(),
                    key.to_string(),
                    value,
                ))
                .await;
        } else {
            crate::get_oplog_entry!(self.state.replay_state, OplogEntry::SetSpanAttribute)?;
        }
        Ok(())
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
        latest_worker_status: &WorkerStatusRecord,
    ) -> Option<LastError> {
        last_error_and_retry_count(this, owned_worker_id, latest_worker_status).await
    }

    async fn compute_latest_worker_status<T: HasOplogService + HasConfig + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, WorkerExecutorError> {
        calculate_last_known_status(this, owned_worker_id, metadata).await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<RetryDecision, WorkerExecutorError> {
        let mut number_of_replayed_functions = 0;

        if refresh_replay_target {
            let new_target = store
                .as_context()
                .data()
                .durable_ctx()
                .state
                .oplog
                .current_oplog_index()
                .await;
            store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .replay_state
                .set_replay_target(new_target);
        }

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
                    Ok(None) => {
                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .process_pending_replay_events()
                            .await?;
                        break Ok(RetryDecision::None);
                    }
                    Ok(Some(replay_state::ExportedFunctionInvoked {
                        function_name,
                        function_input,
                        idempotency_key,
                        invocation_context,
                    })) => {
                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .process_pending_replay_events()
                            .await?;

                        debug!("Replaying function {function_name}");
                        let span = span!(Level::INFO, "replaying", function = function_name);
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_idempotency_key(idempotency_key)
                            .await;

                        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_invocation_context(invocation_context)
                            .await?;

                        let component_metadata = store
                            .as_context()
                            .data()
                            .component_metadata()
                            .metadata
                            .clone();

                        let full_function_name = function_name.to_string();
                        let invoke_result = invoke_observed_and_traced(
                            full_function_name.clone(),
                            function_input.clone(),
                            store,
                            instance,
                            &component_metadata,
                        )
                        .instrument(span)
                        .await;

                        // We are removing the spans introduced by the invocation. Not calling `finish_span` here,
                        // as it would add FinishSpan oplog entries without corresponding StartSpan ones. Instead,
                        // the oplog processor should assume that spans implicitly created by ExportedFunctionInvoked
                        // are finished at ExportedFunctionCompleted.
                        for span_id in local_span_ids {
                            store.as_context_mut().data_mut().remove_span(&span_id)?;
                        }
                        for span_id in inherited_span_ids {
                            store.as_context_mut().data_mut().remove_span(&span_id)?;
                        }

                        match invoke_result {
                            Ok(InvokeResult::Succeeded {
                                output,
                                consumed_fuel,
                            }) => {
                                let component_metadata =
                                    store.as_context().data().component_metadata();

                                match component_metadata
                                    .metadata
                                    .find_function(&full_function_name)
                                    .await
                                {
                                    Ok(value) => {
                                        if let Some(value) = value {
                                            let result = interpret_function_result(
                                                output,
                                                value.analysed_export.result,
                                            )
                                            .map_err(|e| WorkerExecutorError::ValueMismatch {
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

                                            break Err(WorkerExecutorError::invalid_request(
                                                format!("Function {full_function_name} not found"),
                                            ));
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

                                        break Err(WorkerExecutorError::invalid_request(format!(
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
                                                        break Err(WorkerExecutorError::runtime(
                                                            "Interrupted via the Golem API",
                                                        ));
                                                    } else {
                                                        break Err(WorkerExecutorError::runtime(format!("The worker could not finish replaying a function {function_name}")));
                                                    }
                                                }
                                                TrapType::Exit => {
                                                    break Err(WorkerExecutorError::runtime(
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
                                                    break Err(
                                                        WorkerExecutorError::InvocationFailed {
                                                            error,
                                                            stderr,
                                                        },
                                                    );
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
                store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .process_pending_replay_events()
                    .await?;
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
    ) -> Result<RetryDecision, WorkerExecutorError> {
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
                .switch_to_live()
                .await;

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
            let pending_update = store
                .as_context()
                .data()
                .durable_ctx()
                .public_state
                .worker()
                .peek_pending_update()
                .await;
            let prepare_result = match pending_update {
                Some(timestamped_update) => {
                    match &timestamped_update.description {
                        UpdateDescription::SnapshotBased { .. } => {
                            // If a snapshot based update is pending, no replay should be necessary
                            assert!(store.as_context().data().durable_ctx().is_live());

                            Ok(Self::finalize_pending_snapshot_update(instance, store).await)
                        }
                        UpdateDescription::Automatic { target_version, .. } => {
                            // snapshot update will be succeeded as part of the replay.
                            let result = Self::resume_replay(store, instance, false).await;
                            record_resume_worker(start.elapsed());

                            match result {
                                Err(error) => {
                                    // replay failed. There are two cases here:
                                    // 1. We failed before the update has succeeded. In this case we fail the update and retry the replay.
                                    // 2. We failed after the update has succeeded. In this case we can the original failure.
                                    let final_pending_update = store
                                        .as_context()
                                        .data()
                                        .durable_ctx()
                                        .public_state
                                        .worker()
                                        .peek_pending_update()
                                        .await;
                                    match final_pending_update {
                                        Some(final_pending_update)
                                            if final_pending_update == timestamped_update =>
                                        {
                                            // We failed before the update has succeeded. Mark the update as failed and retry
                                            store
                                                .as_context()
                                                .data()
                                                .durable_ctx()
                                                .public_state
                                                .worker()
                                                .pop_pending_update()
                                                .await;

                                            store
                                                .as_context_mut()
                                                .data_mut()
                                                .on_worker_update_failed(
                                                    *target_version,
                                                    Some(format!(
                                                        "Automatic update failed: {error}"
                                                    )),
                                                )
                                                .await;

                                            debug!("Retrying prepare_instance after failed update attempt");

                                            Ok(RetryDecision::Immediate)
                                        }
                                        Some(_) => {
                                            // There is another pending update. Maybe that one can fix the worker.
                                            debug!("Immediately retrying failed worker with next pending update");
                                            Ok(RetryDecision::Immediate)
                                        }
                                        _ => Err(error),
                                    }
                                }
                                _ => result,
                            }
                        }
                    }
                }
                None => {
                    let result = Self::resume_replay(store, instance, false).await;
                    record_resume_worker(start.elapsed());

                    result
                }
            };
            match prepare_result {
                Ok(RetryDecision::None) => {
                    store.as_context_mut().data_mut().set_suspended().await?;
                    Ok(RetryDecision::None)
                }
                Ok(other) => Ok(other),
                Err(error) => Err(WorkerExecutorError::failed_to_resume_worker(
                    worker_id.clone(),
                    error,
                )),
            }
        }
    }

    async fn record_last_known_limits<T: HasAll<Ctx> + Send + Sync>(
        _this: &T,
        _project_id: &ProjectId,
        _last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn on_worker_deleted<T: HasAll<Ctx> + Send + Sync>(
        _this: &T,
        _worker_id: &WorkerId,
    ) -> Result<(), WorkerExecutorError> {
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
            let created_by = worker.created_by.clone();
            let latest_worker_status =
                calculate_last_known_status(this, &owned_worker_id, &Some(worker)).await?;
            let last_error =
                Self::get_last_error_and_retry_count(this, &owned_worker_id, &latest_worker_status)
                    .await;
            let decision = Self::get_recovery_decision_on_startup(
                latest_worker_status
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
                        &created_by,
                        &owned_worker_id,
                        None,
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

    async fn on_worker_update_failed_to_start<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        target_version: ComponentVersion,
        details: Option<String>,
    ) -> Result<(), WorkerExecutorError> {
        let worker = this
            .worker_activator()
            .get_or_create_suspended(account_id, owned_worker_id, None, None, None, None, None)
            .await?;

        let entry = OplogEntry::failed_update(target_version, details.clone());
        let timestamp = entry.timestamp();
        let failed_update_oplog_idx = worker.oplog().add_and_commit(entry).await;
        let metadata = worker.get_metadata()?;
        let mut status = metadata.last_known_status;
        status.failed_updates.push(FailedUpdateRecord {
            timestamp,
            target_version,
            details: details.clone(),
        });

        // ensure our status is in sync with the oplog idx
        status.oplog_idx = failed_update_oplog_idx;

        if status.skipped_regions.is_overridden() {
            status.skipped_regions.drop_override()
        }

        worker.update_status(status).await;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> FileSystemReading for DurableWorkerCtx<Ctx> {
    async fn get_file_system_node(
        &self,
        path: &ComponentFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        let root = self.temp_dir.path();
        let target = root.join(PathBuf::from(path.to_rel_string()));

        {
            let exists = tokio::fs::try_exists(&target).await.map_err(|e| {
                WorkerExecutorError::FileSystemError {
                    path: path.to_string(),
                    reason: format!("Failed to check whether file exists: {e}"),
                }
            })?;
            if !exists {
                return Ok(GetFileSystemNodeResult::NotFound);
            };
        }

        let metadata = tokio::fs::metadata(&target).await.map_err(|e| {
            WorkerExecutorError::FileSystemError {
                path: path.to_string(),
                reason: format!("Failed to get metadata: {e}"),
            }
        })?;

        if metadata.is_file() {
            let is_readonly_by_host = metadata.permissions().readonly();
            let is_readonly_by_us = self.state.read_only_paths.read().unwrap().contains(&target);

            let permissions = if is_readonly_by_host || is_readonly_by_us {
                ComponentFilePermissions::ReadOnly
            } else {
                ComponentFilePermissions::ReadWrite
            };

            let last_modified = metadata.modified().ok().unwrap_or(SystemTime::UNIX_EPOCH);
            let file_name = target
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let file_node = ComponentFileSystemNode {
                name: file_name,
                last_modified,
                details: ComponentFileSystemNodeDetails::File {
                    size: metadata.len(),
                    permissions,
                },
            };

            return Ok(GetFileSystemNodeResult::File(file_node));
        }

        let mut entries = tokio::fs::read_dir(target).await.map_err(|e| {
            WorkerExecutorError::FileSystemError {
                path: path.to_string(),
                reason: format!("Failed to list directory: {e}"),
            }
        })?;

        let mut result = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let metadata =
                entry
                    .metadata()
                    .await
                    .map_err(|e| WorkerExecutorError::FileSystemError {
                        path: path.to_string(),
                        reason: format!("Failed to get file metadata {e}"),
                    })?;

            let entry_name = entry.file_name().to_string_lossy().to_string();

            let last_modified = metadata.modified().ok().unwrap_or(SystemTime::UNIX_EPOCH);

            if metadata.is_file() {
                let is_readonly_by_host = metadata.permissions().readonly();
                // additionally consider permissions we maintain ourselves
                let is_readonly_by_us = self
                    .state
                    .read_only_paths
                    .read()
                    .unwrap()
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
        Ok(GetFileSystemNodeResult::Ok(result))
    }

    async fn read_file(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        let root = self.temp_dir.path();
        let target = root.join(PathBuf::from(path.to_rel_string()));

        {
            let exists = tokio::fs::try_exists(&target).await.map_err(|e| {
                WorkerExecutorError::FileSystemError {
                    path: path.to_string(),
                    reason: format!("Failed to check whether file exists: {e}"),
                }
            })?;
            if !exists {
                return Ok(ReadFileResult::NotFound);
            };
        }

        {
            let metadata = tokio::fs::metadata(&target).await.map_err(|e| {
                WorkerExecutorError::FileSystemError {
                    path: path.to_string(),
                    reason: format!("Failed to get metadata: {e}"),
                }
            })?;
            if !metadata.is_file() {
                return Ok(ReadFileResult::NotAFile);
            };
        }

        let path_clone = path.clone();

        let stream = tokio::fs::File::open(target)
            .map_ok(|file| FramedRead::new(file, BytesCodec::new()).map_ok(BytesMut::freeze))
            .try_flatten_stream()
            .map_err(move |e| WorkerExecutorError::FileSystemError {
                path: path_clone.to_string(),
                reason: format!("Failed to open file: {e}"),
            });

        Ok(ReadFileResult::Ok(Box::pin(stream)))
    }
}

async fn last_error_and_retry_count<T: HasOplogService + HasConfig>(
    this: &T,
    owned_worker_id: &OwnedWorkerId,
    latest_worker_status: &WorkerStatusRecord,
) -> Option<LastError> {
    let mut idx = this.oplog_service().get_last_index(owned_worker_id).await;
    let mut retry_count = 0;
    if idx == OplogIndex::NONE {
        None
    } else {
        let mut first_error = None;
        let mut last_error_index = idx;
        loop {
            if latest_worker_status
                .deleted_regions
                .is_in_deleted_region(idx)
            {
                if idx > OplogIndex::INITIAL {
                    idx = idx.previous();
                    continue;
                } else {
                    break;
                }
            } else {
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
                            break;
                        }
                    }
                    Some((_, entry)) if entry.is_hint() => {
                        // Skipping hint entries as they can randomly interleave the error entries (such as incoming invocation requests, etc)
                        if idx > OplogIndex::INITIAL {
                            idx = idx.previous();
                            continue;
                        } else {
                            break;
                        }
                    }
                    Some((_, _)) => break,
                    None => {
                        // This is possible if the oplog has been deleted between the get_last_index and the read call
                        break;
                    }
                }
            }
        }
        match first_error {
            Some(error) => Some(LastError {
                error,
                retry_count,
                stderr: recover_stderr_logs(this, owned_worker_id, last_error_index).await,
            }),
            None => None,
        }
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
    /// The BeginRemoteWrite entry's index
    pub begin_index: OplogIndex,
    /// Information about the request to be included in the oplog
    pub request: SerializableHttpRequest,
    /// SpanId
    pub span_id: SpanId,
}

struct PrivateDurableWorkerState {
    oplog_service: Arc<dyn OplogService>,
    oplog: Arc<dyn Oplog>,
    promise_service: Arc<dyn PromiseService>,
    scheduler_service: Arc<dyn SchedulerService>,
    worker_service: Arc<dyn WorkerService>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
    key_value_service: Arc<dyn KeyValueService>,
    blob_store_service: Arc<dyn BlobStoreService>,
    rdbms_service: Arc<dyn RdbmsService>,
    component_service: Arc<dyn ComponentService>,
    plugins: Arc<dyn Plugins>,
    config: Arc<GolemConfig>,
    owned_worker_id: OwnedWorkerId,
    created_by: AccountId,
    current_idempotency_key: Option<IdempotencyKey>,
    rpc: Arc<dyn Rpc>,
    worker_proxy: Arc<dyn WorkerProxy>,
    resources: HashMap<WorkerResourceId, (ResourceTypeId, ResourceAny)>,
    last_resource_id: WorkerResourceId,
    replay_state: ReplayState,
    overridden_retry_policy: Option<RetryConfig>,
    persistence_level: PersistenceLevel,
    assume_idempotence: bool,

    /// State of ongoing http requests, key is the resource id it is most recently associated with (one state object can belong to multiple resources, but just one at once)
    open_http_requests: HashMap<u32, HttpRequestState>,

    snapshotting_mode: Option<PersistenceLevel>,

    indexed_resources: HashMap<IndexedResourceKey, WorkerResourceId>,
    agent_instances: HashMap<AgentInstanceKey, AgentInstanceDescription>,

    component_metadata: golem_service_base::model::Component,

    total_linear_memory_size: u64,

    invocation_context: InvocationContext,
    current_span_id: SpanId,
    forward_trace_context_headers: bool,
    set_outgoing_http_idempotency_key: bool,

    worker_fork: Arc<dyn WorkerForkService>,

    read_only_paths: RwLock<HashSet<PathBuf>>,
    files: TRwLock<HashMap<PathBuf, IFSWorkerFile>>,
    file_loader: Arc<FileLoader>,

    project_service: Arc<dyn ProjectService>,
    /// The initial config vars that the worker was configured with
    initial_wasi_config_vars: BTreeMap<String, String>,
    /// The current config vars of the worker, taking into account component version, etc.
    wasi_config_vars: RwLock<BTreeMap<String, String>>,
}

impl PrivateDurableWorkerState {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        promise_service: Arc<dyn PromiseService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        component_service: Arc<dyn ComponentService>,
        plugins: Arc<dyn Plugins>,
        config: Arc<GolemConfig>,
        owned_worker_id: OwnedWorkerId,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        deleted_regions: DeletedRegions,
        last_oplog_index: OplogIndex,
        component_metadata: golem_service_base::model::Component,
        total_linear_memory_size: u64,
        worker_fork: Arc<dyn WorkerForkService>,
        read_only_paths: RwLock<HashSet<PathBuf>>,
        files: TRwLock<HashMap<PathBuf, IFSWorkerFile>>,
        file_loader: Arc<FileLoader>,
        project_service: Arc<dyn ProjectService>,
        created_by: AccountId,
        initial_wasi_config_vars: BTreeMap<String, String>,
        wasi_config_vars: BTreeMap<String, String>,
    ) -> Self {
        let replay_state = ReplayState::new(
            owned_worker_id.clone(),
            oplog_service.clone(),
            oplog.clone(),
            deleted_regions,
            last_oplog_index,
        )
        .await;
        let invocation_context = InvocationContext::new(None);
        let current_span_id = invocation_context.root.span_id().clone();
        Self {
            oplog_service,
            oplog: oplog.clone(),
            promise_service,
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
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
            open_http_requests: HashMap::new(),
            snapshotting_mode: None,
            indexed_resources: HashMap::new(),
            agent_instances: HashMap::new(),
            component_metadata,
            total_linear_memory_size,
            replay_state,
            invocation_context,
            current_span_id,
            forward_trace_context_headers: true,
            set_outgoing_http_idempotency_key: true,
            worker_fork,
            read_only_paths,
            files,
            file_loader,
            project_service,
            created_by,
            initial_wasi_config_vars,
            wasi_config_vars: RwLock::new(wasi_config_vars),
        }
    }

    pub async fn begin_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        if (*function_type == DurableFunctionType::WriteRemote && !self.assume_idempotence)
            || matches!(
                *function_type,
                DurableFunctionType::WriteRemoteBatched(None)
            )
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
                        self.replay_state.switch_to_live().await;
                        Err(WorkerExecutorError::runtime(
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
                        self.replay_state.switch_to_live().await;

                        // But this is not enough, because if the retried batched write operation succeeds,
                        // and later we replay it, we need to skip the first attempt and only replay the second.
                        // Se we add a Jump entry to the oplog that registers a deleted region.
                        let deleted_region = OplogRegion {
                            start: begin_index.next(), // need to keep the BeginAtomicRegion entry
                            end: self.replay_state.replay_target().next(), // skipping the Jump entry too
                        };
                        self.replay_state
                            .add_skipped_region(deleted_region.clone())
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
    ) -> Result<(), WorkerExecutorError> {
        if (*function_type == DurableFunctionType::WriteRemote && !self.assume_idempotence)
            || matches!(
                *function_type,
                DurableFunctionType::WriteRemoteBatched(None)
            )
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

    pub async fn sleep_until(&self, when: DateTime<Utc>) -> Result<(), WorkerExecutorError> {
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
                    account_id: self.created_by.clone(),
                    project_id: self.owned_worker_id.project_id(),
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

    pub async fn get_workers(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), WorkerExecutorError> {
        self.worker_enumeration_service
            .get(
                &self.owned_worker_id.project_id,
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
        Uri {
            value: self.owned_worker_id.worker_id.to_worker_urn(),
        }
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        let id = self.last_resource_id;
        self.last_resource_id = self.last_resource_id.next();
        self.resources.insert(id, (name, resource));
        id.0
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        let resource_id = WorkerResourceId(resource_id);
        self.resources.remove(&resource_id)
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        self.resources.get(&WorkerResourceId(resource_id)).cloned()
    }
}

impl HasOplogService for PrivateDurableWorkerState {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        self.oplog_service.clone()
    }
}

impl HasOplog for PrivateDurableWorkerState {
    fn oplog(&self) -> Arc<dyn Oplog> {
        self.oplog.clone()
    }
}

impl HasConfig for PrivateDurableWorkerState {
    fn config(&self) -> Arc<GolemConfig> {
        self.config.clone()
    }
}

impl HasPlugins for PrivateDurableWorkerState {
    fn plugins(&self) -> Arc<dyn Plugins> {
        self.plugins.clone()
    }
}

impl HasProjectService for PrivateDurableWorkerState {
    fn project_service(&self) -> Arc<dyn ProjectService> {
        self.project_service.clone()
    }
}

pub struct PublicDurableWorkerState<Ctx: WorkerCtx> {
    promise_service: Arc<dyn PromiseService>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
    invocation_queue: Weak<Worker<Ctx>>,
    oplog: Arc<dyn Oplog>,
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
    fn oplog(&self) -> Arc<dyn Oplog> {
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

impl<Ctx: WorkerCtx> IoView for DurableWorkerCtxWasiView<'_, Ctx> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }

    fn io_ctx(&mut self) -> &mut IoCtx {
        self.0.io_ctx()
    }
}

// This wrapper forces the compiler to choose the wasmtime_wasi implementations for T: WasiView
impl<Ctx: WorkerCtx> WasiView for DurableWorkerCtxWasiView<'_, Ctx> {
    fn ctx(&mut self) -> &mut WasiCtx {
        self.0.ctx()
    }
}

impl<Ctx: WorkerCtx> IoView for DurableWorkerCtxWasiHttpView<'_, Ctx> {
    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.0.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    fn io_ctx(&mut self) -> &mut IoCtx {
        Arc::get_mut(&mut self.0.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail")
    }
}

impl<Ctx: WorkerCtx> WasiHttpView for DurableWorkerCtxWasiHttpView<'_, Ctx> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.0.wasi_http
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

/// File that was provisioned due to metadata. There might be additional files that the
/// worker created itself.
/// Ro files are symlinked to the proper location and might be garbage collected when the token is dropped.
/// Rw files are directly copied to the target location.
enum IFSWorkerFile {
    Ro {
        file: InitialComponentFile,
        _token: FileUseToken,
    },
    Rw,
}

async fn prepare_filesystem(
    file_loader: &Arc<FileLoader>,
    project_id: &ProjectId,
    root: &Path,
    files: &[InitialComponentFile],
) -> Result<HashMap<PathBuf, IFSWorkerFile>, WorkerExecutorError> {
    let futures = files.iter().map(|file| {
        let path = root.join(PathBuf::from(file.path.to_rel_string()));
        let file = file.clone();
        let permissions = file.permissions;
        let file_loader = file_loader.clone();
        async move {
            match permissions {
                ComponentFilePermissions::ReadOnly => {
                    debug!("Loading read-only file {}", path.display());
                    let token = file_loader
                        .get_read_only_to(project_id, &file.key, &path)
                        .await?;
                    Ok::<_, WorkerExecutorError>((
                        path,
                        IFSWorkerFile::Ro {
                            file,
                            _token: token,
                        },
                    ))
                }
                ComponentFilePermissions::ReadWrite => {
                    debug!("Loading read-write file {}", path.display());
                    file_loader
                        .get_read_write_to(project_id, &file.key, &path)
                        .await?;
                    Ok((path, IFSWorkerFile::Rw))
                }
            }
        }
    });
    Ok(HashMap::from_iter(try_join_all(futures).await?))
}

async fn update_filesystem(
    current_state: &mut HashMap<PathBuf, IFSWorkerFile>,
    file_loader: &Arc<FileLoader>,
    project_id: &ProjectId,
    root: &Path,
    files: &[InitialComponentFile],
) -> Result<(), WorkerExecutorError> {
    enum UpdateFileSystemResult {
        NoChanges,
        Remove(PathBuf),
        Replace { path: PathBuf, value: IFSWorkerFile },
    }

    let desired_paths: HashSet<PathBuf> = HashSet::from_iter(
        files
            .iter()
            .map(|f| root.join(PathBuf::from(f.path.to_rel_string()))),
    );

    // We do this in two phases to make errors less likely. First delete all files that are no longer needed and then create
    // new ones.
    let futures_phase_1 = current_state.iter().map(|(path, file)| {
        let path = path.clone();
        let should_keep = desired_paths.contains(&path);
        async move {
            match file {
                IFSWorkerFile::Ro { file, .. } if !should_keep => {
                    tokio::fs::remove_dir(&path).await.map_err(|e| {
                        WorkerExecutorError::FileSystemError {
                            path: file.path.to_rel_string(),
                            reason: format!("Failed deleting file during update: {e}"),
                        }
                    })?;
                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Remove(path))
                }
                _ => Ok(UpdateFileSystemResult::NoChanges),
            }
        }
    });

    let futures_phase_2 = files.iter().map(|file| {
        let path = root.join(PathBuf::from(file.path.to_rel_string()));
        let file = file.clone();
        let permissions = file.permissions;
        let file_loader = file_loader.clone();

        let existing = current_state.get(&path);

        async move {
            match (permissions, existing) {
                (ComponentFilePermissions::ReadOnly, None) => {
                    debug!("Loading read-only file {}", path.display());

                    let exists = tokio::fs::try_exists(&path).map_err(|e| WorkerExecutorError::FileSystemError { path: file.path.to_rel_string(), reason: format!("Failed checking whether path exists: {e}") }).await?;

                    if exists {
                        // Try removing it if it's an empty directory; this will fail otherwise, and we can report the error.
                        tokio::fs::remove_dir(&path).await.map_err(|e|
                            WorkerExecutorError::FileSystemError {
                                path: file.path.to_rel_string(),
                                reason: format!("Tried replacing an existing non-empty path with ro file during update: {e}"),
                            }
                        )?;
                    };

                    let token = file_loader
                        .get_read_only_to(project_id, &file.key, &path)
                        .await?;

                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Ro { file, _token: token } })
                }
                (ComponentFilePermissions::ReadOnly, Some(IFSWorkerFile::Ro { file: existing_file, .. })) => {
                    if existing_file.key == file.key {
                        Ok(UpdateFileSystemResult::NoChanges)
                    } else {
                        debug!("updating ro file {}", path.display());
                        tokio::fs::remove_file(&path).await.map_err(|e|
                            WorkerExecutorError::FileSystemError {
                                path: file.path.to_rel_string(),
                                reason: format!("Failed deleting file during update: {e}"),
                            }
                        )?;
                        let token = file_loader
                            .get_read_only_to(project_id, &file.key, &path)
                            .await?;
                        Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Ro { file, _token: token } })
                    }
                }
                (ComponentFilePermissions::ReadOnly, Some(IFSWorkerFile::Rw)) => {
                    Err(WorkerExecutorError::FileSystemError {
                        path: file.path.to_rel_string(),
                        reason: "Tried updating rw file to ro during update".to_string(),
                    })
                }
                (ComponentFilePermissions::ReadWrite, None) => {
                    debug!("Loading rw file {}", path.display());

                    let exists = tokio::fs::try_exists(&path).map_err(|e| WorkerExecutorError::FileSystemError { path: file.path.to_rel_string(), reason: format!("Failed checking whether path exists: {e}") }).await?;

                    if exists {
                        let metadata = tokio::fs::metadata(&path).await.map_err(|e|
                            WorkerExecutorError::FileSystemError {
                                path: file.path.to_rel_string(),
                                reason: format!("Failed getting metadata of path: {e}"),
                            }
                        )?;

                        if metadata.is_file() {
                            return Ok(UpdateFileSystemResult::NoChanges);
                        }

                        // Try removing it if it's an empty directory, this will fail otherwise, and we can report the error.
                        tokio::fs::remove_dir(&path).await.map_err(|e|
                            WorkerExecutorError::FileSystemError {
                                path: file.path.to_rel_string(),
                                reason: format!("Tried replacing an existing non-empty path with rw file during update: {e}"),
                            }
                        )?;
                    }

                    file_loader
                        .get_read_write_to(project_id, &file.key, &path)
                        .await?;
                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Rw })
                }
                (ComponentFilePermissions::ReadWrite, Some(IFSWorkerFile::Ro { .. })) => {
                    debug!("Updating ro file to rw {}", path.display());
                    tokio::fs::remove_file(&path).await.map_err(|e|
                        WorkerExecutorError::FileSystemError {
                            path: file.path.to_rel_string(),
                            reason: format!("Failed deleting file during update: {e}"),
                        }
                    )?;
                    file_loader
                        .get_read_write_to(project_id, &file.key, &path)
                        .await?;
                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Rw })
                }
                (ComponentFilePermissions::ReadWrite, Some(IFSWorkerFile::Rw)) => {
                    debug!("Updating rw file {}", path.display());
                    Ok(UpdateFileSystemResult::NoChanges)
                }
            }
        }
    });

    let mut results = try_join_all(futures_phase_1).await?;
    results.extend(try_join_all(futures_phase_2).await?);

    for result in results {
        match result {
            UpdateFileSystemResult::NoChanges => {}
            UpdateFileSystemResult::Remove(path) => {
                current_state.remove(&path);
            }
            UpdateFileSystemResult::Replace { path, value } => {
                current_state.insert(path, value);
            }
        }
    }

    Ok(())
}

fn compute_read_only_paths(files: &HashMap<PathBuf, IFSWorkerFile>) -> HashSet<PathBuf> {
    let ro_paths = files.iter().filter_map(|(p, f)| match f {
        IFSWorkerFile::Ro { .. } => Some(p.clone()),
        _ => None,
    });
    HashSet::from_iter(ro_paths)
}

fn effective_wasi_config_vars(
    worker_wasi_config_vars: BTreeMap<String, String>,
    component_wasi_config_vars: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();

    for (k, v) in component_wasi_config_vars {
        result.insert(k, v);
    }

    for (k, v) in worker_wasi_config_vars {
        result.insert(k, v);
    }

    result
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
                    break Err(golem_service_base::error::worker_executor::WorkerExecutorError::unexpected_oplog_entry(
                        stringify!($($cases |)+),
                        format!("{:?}", oplog_entry),
                    ));
                }
            }
        }
    };
}
