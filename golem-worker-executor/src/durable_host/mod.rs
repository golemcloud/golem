// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
mod filesystem;
pub mod golem;
pub mod http;
pub mod io;
pub mod keyvalue;
mod logging;
pub mod quota;
mod random;
pub mod rdbms;
mod replay_state;
mod sockets;
pub mod wasm_rpc;
pub mod websocket;

use self::golem::v1x::GetPromiseResultEntry;
use crate::durable_host::durability::collect_named_retry_policies;
use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::replay_state::{OplogEntryLookupResult, ReplayState};
use crate::metrics::ephemeral::record_non_suspending_failure;
use crate::metrics::storage::{
    STORAGE_TYPE_FILESYSTEM, record_storage_bytes_deleted, record_storage_bytes_written,
};
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::model::event::InternalWorkerEvent;
use crate::model::{
    AgentConfig, ExecutionStatus, InvocationContext, LastError, ReadFileResult, TrapType,
};
use crate::services::agent_types::AgentTypesService;
use crate::services::agent_webhooks::AgentWebhooksService;
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentService;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::file_loader::{FileLoader, FileUseToken};
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, OplogService};
use crate::services::promise::PromiseService;
use crate::services::quota::QuotaService;
use crate::services::rdbms::RdbmsService;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::shard::ShardService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_fork::WorkerForkService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{HasAll, HasConfig, HasOplog, HasWorker, worker_enumeration};
use crate::services::{HasComponentService, HasOplogService, HasWorkerService};
use crate::wasi_host;
use crate::worker::agent_config::{effective_agent_config, validate_agent_config};
use crate::worker::invocation::{
    AgentExportFuncs, InvocationMode, InvokeResult, invoke_observed_and_traced, lower_invocation,
};
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::worker::{RetryDecision, Worker};
use crate::workerctx::{
    ExternalOperations, FileSystemReading, InvocationContextManagement, InvocationHooks,
    InvocationManagement, LogEventEmitBehaviour, PublicWorkerIo, StatusManagement,
    UpdateManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::BytesMut;
use chrono::{DateTime, Utc};
pub use durability::*;
use futures::TryFutureExt;
use futures::TryStreamExt;
use futures::future::try_join_all;
use golem_common::model::TransactionId;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, ParsedAgentId, Principal};
use golem_common::model::component::{
    AgentFilePermissions, CanonicalFilePath, ComponentId, ComponentRevision, InitialAgentFile,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::{
    AgentError, AgentResourceId, DurableFunctionType, HostRequestHttpRequest, LogLevel, OplogEntry,
    OplogIndex, PersistenceLevel, RawSnapshotData, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_common::model::{
    AgentFilter, AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult,
    AgentMetadata, AgentStatus, AgentStatusRecord, IdempotencyKey, OwnedAgentId, RetryContext,
    RetryVerdict, ScanCursor, ScheduledAction, Timestamp,
};
use golem_common::model::{PredicateValue, RetryPolicyState, RetryProperties};
use golem_common::resource_runtime::{ResourceStore, ResourceTypeId};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::component::Component;
use golem_service_base::model::{
    ComponentFileSystemNode, ComponentFileSystemNodeDetails, GetFileSystemNodeResult,
};
use golem_wasm::Uri;
use replay_state::ReplayEvent;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use std::vec;
use tempfile::TempDir;
use tokio::sync::RwLock as TRwLock;

/// A worker's filesystem root directory. Either a random OS temp directory
/// (the default) or a deterministic path derived from the agent id.
///
/// In both cases the directory is removed when this value is dropped.
enum WorkerDir {
    /// Random temp dir created by `tempfile`. Auto-deleted on drop.
    Temp(TempDir),
    /// Deterministic directory. Deleted explicitly on drop.
    Deterministic(PathBuf),
}

impl WorkerDir {
    fn path(&self) -> &Path {
        match self {
            WorkerDir::Temp(td) => td.path(),
            WorkerDir::Deterministic(p) => p,
        }
    }
}

impl Drop for WorkerDir {
    fn drop(&mut self) {
        if let WorkerDir::Deterministic(p) = self
            && p.exists()
        {
            let _ = std::fs::remove_dir_all(p);
        }
        // WorkerDir::Temp is dropped automatically by TempDir's own Drop impl
    }
}

use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
use tokio_util::codec::{BytesCodec, FramedRead};
use tracing::{Instrument, Level, debug, error, info, span, warn};
use try_match::try_match;
use uuid::Uuid;
use wasmtime::component::{Instance, Resource, ResourceAny};
use wasmtime::{AsContext, AsContextMut};
use wasmtime_wasi::p2::FsResult;
use wasmtime_wasi::p2::bindings::filesystem::preopens::Descriptor;
use wasmtime_wasi::{
    I32Exit, IoCtx, IoData, IoView, ResourceTable, ResourceTableError, WasiCtx, WasiCtxView,
    WasiView,
};
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{
    BodyCompletionReceiver, HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
    default_send_request_with_pool,
};
use wasmtime_wasi_http::{HttpConnectionPool, WasiHttpCtx};

/// Hooks providing the custom HTTP request handling needed for durable
/// execution. Stored on `DurableWorkerCtx` and exposed via `WasiHttpCtxView`
/// for `wasmtime-wasi-http`.
pub struct DurableHttpHooks {
    /// Connection pool used for outgoing HTTP requests. Mirror of
    /// `WasiHttpCtx::connection_pool` so that `WasiHttpHooks::send_request`
    /// can construct the deferred future without re-borrowing `WasiHttpCtx`.
    pub connection_pool: Option<HttpConnectionPool>,
    /// Shared replay flag that durable execution toggles when transitioning
    /// between live and replay modes. When `true`, outgoing HTTP requests are
    /// deferred so that they can be replayed from the oplog instead.
    pub is_replay: Arc<AtomicBool>,
}

impl WasiHttpHooks for DurableHttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
        body_completion: Option<BodyCompletionReceiver>,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let connection_pool = self.connection_pool.clone();
        if self.is_replay.load(std::sync::atomic::Ordering::Acquire) {
            // If this is a replay, we must not actually send the request, but we have to store it in the
            // FutureIncomingResponse because it is possible that there wasn't any response recorded in the oplog.
            // If that is the case, the request has to be sent as soon as we get into live mode and trying to await
            // or poll the response future.
            Ok(HostFutureIncomingResponse::deferred(Box::new(move || {
                Ok(default_send_request_with_pool(
                    request,
                    config,
                    body_completion,
                    connection_pool,
                ))
            })))
        } else {
            Ok(default_send_request_with_pool(
                request,
                config,
                body_completion,
                connection_pool,
            ))
        }
    }

    fn connection_pool(&self) -> Option<&HttpConnectionPool> {
        self.connection_pool.as_ref()
    }
}

/// Controls how strictly the host filters side-effects performed by user code during an
/// agent invocation.
///
/// `Normal` is the default and applies to every invocation that is not explicitly marked
/// read-only. `ReadOnly` is set automatically by the worker-executor around the invocation
/// of any agent method whose [`AgentMethod::read_only`] metadata is `Some(_)`. While
/// `ReadOnly` is active, outgoing HTTP and RPC host calls are trapped before they are
/// performed and before any oplog entry is written, surfacing a typed
/// [`AgentError::ReadOnlyViolation`] to the SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationStrictness {
    /// No additional restrictions beyond the normal durability/persistence machinery.
    Normal,
    /// The invocation is restricted to read-only host calls. Outgoing HTTP and RPC calls
    /// trap immediately with [`AgentError::ReadOnlyViolation`].
    ReadOnly,
}

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: Arc<Mutex<ResourceTable>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi: Arc<Mutex<WasiCtx>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    io_ctx: Arc<Mutex<IoCtx>>,
    wasi_http: WasiHttpCtx,
    http_hooks: DurableHttpHooks,
    pub owned_agent_id: OwnedAgentId,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState,
    worker_dir: Arc<WorkerDir>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
    pub websocket_connection_pool: websocket::WebSocketConnectionPool,
    resource_limits: Arc<AtomicResourceEntry>,
    /// Per-instance cache of resolved typed guest export handles, populated
    /// lazily on first use during invocation dispatch.
    agent_export_funcs: AgentExportFuncs,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn derive_idempotency_key(&mut self, oplog_index: OplogIndex) -> IdempotencyKey {
        let current_idempotency_key = self
            .state
            .get_current_idempotency_key()
            .unwrap_or(IdempotencyKey::fresh());
        let idempotency_key_oplog_index =
            self.state.current_idempotency_key_oplog_index(oplog_index);
        IdempotencyKey::derived(&current_idempotency_key, idempotency_key_oplog_index)
    }

    /// Returns the per-instance cache of resolved typed guest export handles.
    pub(crate) fn agent_export_funcs(&self) -> &AgentExportFuncs {
        &self.agent_export_funcs
    }

    /// Returns a mutable reference to the per-instance cache of resolved typed
    /// guest export handles.
    pub(crate) fn agent_export_funcs_mut(&mut self) -> &mut AgentExportFuncs {
        &mut self.agent_export_funcs
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        owned_agent_id: OwnedAgentId,
        agent_id: Option<ParsedAgentId>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        quota_service: Arc<dyn QuotaService>,
        event_service: Arc<dyn WorkerEventService>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Ctx>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        resource_limits: Arc<AtomicResourceEntry>,
        config: Arc<GolemConfig>,
        worker_config: AgentConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        worker_fork: Arc<dyn WorkerForkService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        shard_service: Arc<dyn ShardService>,
        http_connection_pool: Option<HttpConnectionPool>,
        websocket_connection_pool: websocket::WebSocketConnectionPool,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<Uuid>,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
    ) -> Result<Self, WorkerExecutorError> {
        let worker_dir = Arc::new(
            if let Some(root) = &config.filesystem_storage.deterministic_root_dir {
                let dir = root
                    .join(owned_agent_id.environment_id.to_string())
                    .join(owned_agent_id.agent_id.component_id.to_string())
                    .join(owned_agent_id.agent_id.agent_name_encoded());
                std::fs::create_dir_all(&dir).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to create deterministic directory {}: {e}",
                        dir.display()
                    ))
                })?;
                WorkerDir::Deterministic(dir)
            } else {
                WorkerDir::Temp(tempfile::Builder::new().prefix("golem").tempdir().map_err(
                    |e| {
                        WorkerExecutorError::runtime(format!(
                            "Failed to create temporary directory: {e}",
                        ))
                    },
                )?)
            },
        );
        debug!("Created file system root at {:?}", worker_dir.path());

        debug!(
            "Worker {} initialized with deleted regions {}",
            owned_agent_id.agent_id, worker_config.deleted_regions
        );

        debug!(
            "Worker {} starting replay from component revision {}",
            owned_agent_id.agent_id, worker_config.component_revision_for_replay
        );
        let component_metadata = component_service
            .get_metadata(
                owned_agent_id.component_id(),
                Some(worker_config.component_revision_for_replay),
            )
            .await?;

        let agent_type_provision_configs = agent_id.as_ref().and_then(|agent_id| {
            component_metadata
                .metadata
                .agent_type_provision_configs()
                .get(&agent_id.agent_type)
                .cloned()
        });
        let files = prepare_filesystem(
            &file_loader,
            owned_agent_id.environment_id,
            worker_dir.path(),
            agent_type_provision_configs
                .as_ref()
                .map(|c| c.files.as_slice())
                .unwrap_or_default(),
        )
        .await?;

        // Acquire storage semaphore permits for read-write initial component files.
        //
        // Read-only files are hardlinked from the FileLoader shared cache, so
        // they occupy disk space only once per unique content hash regardless of
        // how many workers reference them. FileLoader acquires the semaphore
        // permit on the first cache miss and releases it when the last
        // FileUseToken for that entry is dropped — no per-worker charge here.
        //
        // Read-write files are copied per-worker (each worker gets its own
        // private inode and data blocks), so they must be charged individually.
        if let Some(worker) = invocation_queue.upgrade() {
            let rw_bytes: u64 = agent_type_provision_configs
                .as_ref()
                .map(|c| c.files.as_slice())
                .unwrap_or_default()
                .iter()
                .filter(|f| f.permissions == AgentFilePermissions::ReadWrite)
                .map(|f| f.size)
                .sum();
            if rw_bytes > 0 {
                worker
                    .acquire_initial_filesystem_storage(rw_bytes)
                    .await
                    .map_err(|trap| WorkerExecutorError::runtime(trap.to_string()))?;
            }
        }

        let agent_config = if agent_id.is_some() {
            effective_agent_config(
                worker_config.initial_agent_config.clone(),
                agent_type_provision_configs
                    .as_ref()
                    .map(|c| c.config.clone())
                    .unwrap_or_default(),
            )
        } else {
            HashMap::new()
        };

        let stdin = ManagedStdIn::disabled();
        let stdout = ManagedStdOut::from_stdout(tokio::io::stdout());
        let stderr = ManagedStdErr::from_stderr(tokio::io::stderr());
        let suspend_threshold = match execution_status.read().unwrap().agent_mode() {
            AgentMode::Durable => config.suspend.suspend_after,
            AgentMode::Ephemeral => config.suspend.ephemeral_max_sleep,
        };
        let (wasi, io_ctx, table) = wasi_host::create_context(
            &[] as &[&str],
            worker_dir.path().to_path_buf(),
            stdin,
            stdout,
            stderr,
            |duration| wasmtime::Error::from(SuspendForSleep(duration)),
            suspend_threshold,
        )
        .map_err(|e| WorkerExecutorError::runtime(format!("Could not create WASI context: {e}")))?;
        let mut wasi_http = WasiHttpCtx::new();
        wasi_http.connection_pool = http_connection_pool.clone();
        let http_hooks = DurableHttpHooks {
            connection_pool: http_connection_pool,
            is_replay: Arc::new(AtomicBool::new(false)),
        };
        Ok(DurableWorkerCtx {
            table: Arc::new(Mutex::new(table)),
            wasi: Arc::new(Mutex::new(wasi)),
            io_ctx: Arc::new(Mutex::new(io_ctx)),
            wasi_http,
            http_hooks,
            owned_agent_id: owned_agent_id.clone(),
            websocket_connection_pool,
            public_state: PublicDurableWorkerState {
                promise_service: promise_service.clone(),
                event_service,
                invocation_queue,
                oplog: oplog.clone(),
            },
            state: PrivateDurableWorkerState::new(
                agent_id,
                oplog_service,
                oplog,
                promise_service,
                scheduler_service,
                worker_service,
                worker_enumeration_service,
                key_value_service,
                blob_store_service,
                rdbms_service,
                quota_service,
                component_service,
                agent_types_service,
                environment_state_service,
                agent_webhooks_service,
                config.clone(),
                owned_agent_id.clone(),
                rpc,
                worker_proxy,
                worker_config.deleted_regions.clone(),
                component_metadata,
                worker_config.total_linear_memory_size,
                worker_config.current_filesystem_storage_usage,
                worker_fork,
                RwLock::new(compute_read_only_paths(&files)),
                TRwLock::new(files),
                file_loader,
                worker_config.created_by,
                worker_config.created_by_email,
                worker_config.initial_agent_config,
                agent_config,
                shard_service,
                pending_update,
                original_phantom_id,
                worker_config.last_snapshot_index,
                per_invocation_http_call_limit,
                per_invocation_rpc_call_limit,
                resource_limits.clone(),
            )
            .await?,
            worker_dir,
            execution_status,
            resource_limits,
            agent_export_funcs: AgentExportFuncs::default(),
        })
    }

    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    /// Resets the per-invocation HTTP and RPC call counters to zero.
    ///
    /// Delegates to `PrivateDurableWorkerState::reset_invocation_call_counts`.
    pub fn reset_invocation_call_counts(&mut self) {
        self.state.reset_invocation_call_counts();
    }

    /// Records one outgoing HTTP call against the monthly account quota.
    ///
    /// Returns `Err(WorkerMonthlyHttpCallBudgetExhausted)` if the monthly budget
    /// is exhausted. This trap maps to `RetryDecision::TryStop` — the worker is
    /// suspended (same as filesystem `NodeOutOfFilesystemStorage` -> `ReacquirePermits`),
    /// and will be resumed when the registry replenishes the budget.
    pub fn record_monthly_http_call(&mut self) -> anyhow::Result<()> {
        if self.state.is_live() && !self.state.resource_limit_entry.record_http_call() {
            Err(anyhow!(
                GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted
            ))
        } else {
            Ok(())
        }
    }

    /// Records one outgoing RPC call against the monthly account quota.
    ///
    /// Returns `Err(WorkerMonthlyRpcCallBudgetExhausted)` if the monthly budget
    /// is exhausted.
    pub fn record_monthly_rpc_call(&mut self) -> anyhow::Result<()> {
        if self.state.is_live() && !self.state.resource_limit_entry.record_rpc_call() {
            Err(anyhow!(
                GolemSpecificWasmTrap::WorkerMonthlyRpcCallBudgetExhausted
            ))
        } else {
            Ok(())
        }
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

    fn io_ctx(&mut self) -> &mut IoCtx {
        Arc::get_mut(&mut self.io_ctx)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail")
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.owned_agent_id.agent_id
    }

    pub fn owned_agent_id(&self) -> &OwnedAgentId {
        &self.owned_agent_id
    }

    pub fn created_by(&self) -> AccountId {
        self.state.created_by
    }

    pub fn created_by_email(&self) -> &AccountEmail {
        &self.state.created_by_email
    }

    pub fn parsed_agent_id(&self) -> Option<ParsedAgentId> {
        self.state.agent_id.clone()
    }

    pub fn agent_mode(&self) -> AgentMode {
        self.execution_status.read().unwrap().agent_mode()
    }

    pub fn component_metadata(&self) -> &Component {
        &self.state.component_metadata
    }

    pub fn agent_type_provision_config(&self) -> Option<&AgentTypeProvisionConfig> {
        self.state.agent_id.as_ref().and_then(|agent_id| {
            self.component_metadata()
                .metadata
                .agent_type_provision_config(&agent_id.agent_type)
        })
    }

    pub fn is_exit(error: &anyhow::Error) -> Option<i32> {
        error
            .root_cause()
            .downcast_ref::<I32Exit>()
            .map(|exit| exit.0)
    }

    pub fn as_wasi_view(&mut self) -> DurableWorkerCtxWasiView<'_, Ctx> {
        DurableWorkerCtxWasiView(self)
    }

    pub fn as_wasi_http_view(&mut self) -> WasiHttpCtxView<'_> {
        // Sync the replay flag observed by `WasiHttpHooks::send_request` with
        // the current durable execution state before exposing the view to
        // wasmtime-wasi-http.
        let is_replay = self.state.is_replay();
        self.http_hooks
            .is_replay
            .store(is_replay, std::sync::atomic::Ordering::Release);
        let inner = &mut *self;
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        WasiHttpCtxView {
            ctx: &mut inner.wasi_http,
            table,
            hooks: &mut inner.http_hooks,
        }
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

    pub fn agent_types_service(&self) -> Arc<dyn AgentTypesService> {
        self.state.agent_types_service.clone()
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

    pub fn current_filesystem_storage_usage(&self) -> u64 {
        self.state.current_filesystem_storage_usage
    }

    pub fn max_disk_space(&self) -> u64 {
        self.resource_limits.max_disk_space_limit()
    }

    /// Check whether acquiring `new_bytes` would breach the per-plan storage
    /// limit. Returns `WorkerAgentExceededFilesystemStorageLimit` (permanent) if so.
    /// Does NOT check the executor semaphore pool — that is done by
    /// `acquire_filesystem_space`.
    ///
    /// No-op during replay.
    pub fn check_filesystem_storage_quota(&self, new_bytes: u64) -> anyhow::Result<()> {
        if self.state.is_replay() {
            return Ok(());
        }
        let after = self
            .state
            .current_filesystem_storage_usage
            .saturating_add(new_bytes);
        if after > self.resource_limits.max_disk_space_limit() {
            Err(anyhow!(
                GolemSpecificWasmTrap::WorkerAgentExceededFilesystemStorageLimit
            ))
        } else {
            Ok(())
        }
    }

    /// Acquire `new_bytes` of storage from the executor semaphore pool.
    ///
    /// - During replay: no-op (permits were pre-acquired at startup).
    /// - During live execution: calls `Worker::acquire_filesystem_space`, which
    ///   tries the semaphore non-blockingly. On failure returns
    ///   `NodeOutOfFilesystemStorage` (retriable via `ReacquirePermits`).
    ///
    /// Call `check_filesystem_quota` before calling this to enforce the per-plan
    /// limit (`WorkerAgentExceededFilesystemStorageLimit`). This method only checks the
    /// executor-wide semaphore pool (`NodeOutOfFilesystemStorage`).
    pub async fn acquire_filesystem_storage_space(&mut self, new_bytes: u64) -> anyhow::Result<()> {
        if self.state.is_replay() {
            return Ok(());
        }
        // Acquire the semaphore permit first (non-blocking try). Writing the
        // oplog entry after a confirmed acquire ensures the oplog accurately
        // reflects only committed storage changes — a failed acquire leaves no
        // phantom delta that would inflate `current_filesystem_storage_usage` on restart.
        self.public_state
            .worker()
            .acquire_filesystem_storage_space(new_bytes)
            .await?;
        self.public_state
            .worker()
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(
                new_bytes as i64,
            ))
            .await;
        self.state.current_filesystem_storage_usage += new_bytes;
        let account_id = self.created_by().to_string();
        let environment_id = self.state.owned_agent_id.environment_id().to_string();
        record_storage_bytes_written(
            STORAGE_TYPE_FILESYSTEM,
            &account_id,
            &environment_id,
            new_bytes,
        );
        Ok(())
    }

    /// Release `freed_bytes` back to the executor semaphore pool.
    /// Called when files are deleted or truncated.
    /// During replay this is a no-op.
    pub async fn release_filesystem_storage_space(&mut self, freed_bytes: u64) {
        if self.state.is_replay() {
            return;
        }
        let freed_bytes = freed_bytes.min(self.state.current_filesystem_storage_usage);
        if freed_bytes == 0 {
            return;
        }
        self.public_state
            .worker()
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(
                -(freed_bytes as i64),
            ))
            .await;
        self.public_state
            .worker()
            .release_filesystem_storage_space(freed_bytes)
            .await;
        self.state.current_filesystem_storage_usage -= freed_bytes;
        let account_id = self.created_by().to_string();
        let environment_id = self.state.owned_agent_id.environment_id().to_string();
        record_storage_bytes_deleted(
            STORAGE_TYPE_FILESYSTEM,
            &account_id,
            &environment_id,
            freed_bytes,
        );
    }

    /// Check the per-agent storage quota and acquire permits from the
    /// executor-wide semaphore pool in a single step.
    ///
    /// This combines `check_filesystem_storage_quota` (per-plan limit) and
    /// `acquire_filesystem_storage_space` (executor semaphore) — the two must
    /// always be called together in this order.
    pub async fn reserve_filesystem_storage(&mut self, new_bytes: u64) -> anyhow::Result<()> {
        self.check_filesystem_storage_quota(new_bytes)?;
        self.acquire_filesystem_storage_space(new_bytes).await
    }

    pub async fn increase_memory(&mut self, delta: u64) -> anyhow::Result<()> {
        if self.state.is_replay() {
            // The increased amount was already recorded in live mode, so our worker
            // was initialized with the correct amount of memory.
            Ok(())
        } else {
            // In live mode we need to try to get more memory permits and if we can't,
            // we fail the worker, unload it from memory and schedule a retry.
            // let current_size = self.update_worker_status();
            self.public_state
                .worker()
                .add_to_oplog(OplogEntry::grow_memory(delta))
                .await;

            self.public_state.worker().increase_memory(delta).await?;
            self.state.total_linear_memory_size += delta;
            Ok(())
        }
    }

    /// Returns the deterministic, policy-independent recovery decision for a
    /// trap type — i.e. the cases where the answer does not depend on retry
    /// state or any retry policy. For trap-error variants whose decision is
    /// driven by named retry policies (`Unknown`, `TransientError`, and
    /// `DeterministicTrap` inside an atomic region), this returns `None` and
    /// the caller falls through to policy-based resolution.
    pub(crate) fn fixed_decision_for_trap_type(
        trap_type: &TrapType,
        in_atomic_region: bool,
    ) -> Option<RetryDecision> {
        match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt(ts)) => Some(RetryDecision::TryStop(*ts)),
            TrapType::Interrupt(InterruptKind::Suspend(ts)) => Some(RetryDecision::TryStop(*ts)),
            TrapType::Interrupt(InterruptKind::Restart) => Some(RetryDecision::Immediate),
            TrapType::Interrupt(InterruptKind::Jump) => Some(RetryDecision::Immediate),
            TrapType::Exit => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::OutOfMemory,
                ..
            } => Some(RetryDecision::ReacquirePermits),
            TrapType::Error {
                error: AgentError::InvalidRequest(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::StackOverflow,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededMemoryLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededTableLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::NodeOutOfFilesystemStorage,
                ..
            } => Some(RetryDecision::ReacquirePermits),
            TrapType::Error {
                error: AgentError::AgentExceededFilesystemStorageLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::AgentTerminatedByQuota(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error:
                    AgentError::EphemeralSleepTooLong(_)
                    | AgentError::EphemeralFuelExhausted(_)
                    | AgentError::EphemeralCannotSuspend(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ReadOnlyViolation(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::InternalError(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededHttpCallLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededRpcCallLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::PermanentError(_),
                ..
            } => Some(RetryDecision::None),
            // DeterministicTrap *outside* an atomic region is never retried;
            // *inside* an atomic region it is retried via the named-policy
            // path (handled by the caller).
            TrapType::Error {
                error: AgentError::DeterministicTrap(_),
                ..
            } if !in_atomic_region => Some(RetryDecision::None),
            TrapType::Error {
                error:
                    AgentError::Unknown(_)
                    | AgentError::TransientError(_)
                    | AgentError::DeterministicTrap(_),
                ..
            } => None,
        }
    }

    fn semantic_trap_type_name(error: &AgentError) -> &'static str {
        match error {
            AgentError::OutOfMemory => "out-of-memory",
            AgentError::InvalidRequest(_) => "invalid-request",
            AgentError::StackOverflow => "stack-overflow",
            AgentError::ExceededMemoryLimit => "exceeded-memory-limit",
            AgentError::ExceededTableLimit => "exceeded-table-limit",
            AgentError::ExceededHttpCallLimit => "exceeded-http-call-limit",
            AgentError::ExceededRpcCallLimit => "exceeded-rpc-call-limit",
            AgentError::NodeOutOfFilesystemStorage => "node-out-of-filesystem-storage",
            AgentError::AgentExceededFilesystemStorageLimit => {
                "agent-exceeded-filesystem-storage-limit"
            }
            AgentError::InternalError(_) => "internal-error",
            AgentError::DeterministicTrap(_) => "deterministic-trap",
            AgentError::PermanentError(_) => "permanent-error",
            AgentError::Unknown(_) => "unknown",
            AgentError::TransientError(_) => "transient-error",
            AgentError::AgentTerminatedByQuota(_) => "agent-terminated-by-quota",
            AgentError::EphemeralSleepTooLong(_) => "ephemeral-sleep-too-long",
            AgentError::EphemeralFuelExhausted(_) => "ephemeral-fuel-exhausted",
            AgentError::EphemeralCannotSuspend(_) => "ephemeral-cannot-suspend",
            AgentError::ReadOnlyViolation(_) => "read-only-violation",
        }
    }

    async fn get_recovery_decision_on_trap_with_semantic(
        &mut self,
        retry_state_with_current_attempt: &HashMap<OplogIndex, RetryPolicyState>,
        trap_type: &TrapType,
        in_atomic_region: bool,
        full_function_name: &str,
    ) -> (RetryDecision, Option<RetryPolicyState>) {
        // Cases whose decision does not depend on retry policy at all
        // (Interrupt, Exit, deterministic AgentError variants like
        // OutOfMemory, InvalidRequest, …). Returns `None` when policy
        // resolution is required.
        if let Some(decision) = Self::fixed_decision_for_trap_type(trap_type, in_atomic_region) {
            return (decision, None);
        }

        // Only Error variants whose decision is policy-driven reach this point
        // (Unknown, TransientError, and DeterministicTrap-in-atomic-region).
        let TrapType::Error {
            error,
            retry_from,
            semantic_trap_retry_override,
        } = trap_type
        else {
            // Should be unreachable: `fixed_decision_for_trap_type` returns
            // `Some(...)` for every non-Error trap variant. Treat as "give up"
            // defensively.
            return (RetryDecision::None, None);
        };

        // (B) — host-originated trap carrying an already-resolved verdict.
        // The host call resolved the named policy with full properties (e.g.
        // HTTP `status-code`) before escalating to trap+replay; honour that
        // exact verdict so the inline path and the trap path stay
        // semantically equivalent.
        if let Some(override_) = semantic_trap_retry_override {
            let decision = match &override_.verdict {
                crate::durable_host::durability::SemanticTrapRetryVerdict::Retry(delay) => {
                    debug!(
                        retry_policy = %override_.policy_name,
                        retry_path = "trap",
                        retry_policy_source = "host-override",
                        retry_decision = "retry",
                        delay_ms = delay.as_millis() as u64,
                        trap = ?trap_type,
                        "Semantic trap retry: delaying (override carried from host call)"
                    );
                    RetryDecision::Delayed(*delay)
                }
                crate::durable_host::durability::SemanticTrapRetryVerdict::GiveUp => {
                    debug!(
                        retry_policy = %override_.policy_name,
                        retry_path = "trap",
                        retry_policy_source = "host-override",
                        retry_decision = "give-up",
                        trap = ?trap_type,
                        "Semantic trap retry: exhausted (override carried from host call)"
                    );
                    RetryDecision::None
                }
            };
            return (decision, Some(override_.retry_policy_state.clone()));
        }

        // (A) — guest-originated trap, or host-originated trap whose
        // `try_trigger_retry` did not produce an override (e.g. eval-error
        // fallthrough). Build a `RetryContext::trap` and resolve through the
        // named retry policies. The synthesized default-from-config has
        // `Predicate::True` so resolution is guaranteed to find a match for
        // any properties this branch produces.
        let named_retry_policies = self.named_retry_policies().await;

        let mut properties = RetryContext::trap(
            Self::semantic_trap_type_name(error),
            Some(full_function_name),
        );
        self.state.enrich_retry_properties(&mut properties);

        // Status-code-keyed user policies are deliberately skipped here (no
        // `status-code` is present in trap context). The synthesized default
        // policy then provides the fallback.
        let named_policy = match golem_common::model::NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
            &named_retry_policies,
            &properties,
        ) {
            Ok(Some(named_policy)) => named_policy,
            Ok(None) => {
                warn!(
                    trap = ?trap_type,
                    "No named retry policy matched the trap context (including the synthesized default); giving up"
                );
                return (RetryDecision::None, None);
            }
            Err(error) => {
                warn!(
                    ?error,
                    trap = ?trap_type,
                    "Failed resolving semantic trap retry policy; giving up"
                );
                return (RetryDecision::None, None);
            }
        };

        let current_state = retry_state_with_current_attempt.get(retry_from).cloned();
        let total_attempts_with_current = current_state
            .as_ref()
            .map(|s| s.retry_count())
            .unwrap_or_default();
        let total_attempts_before_current = total_attempts_with_current.saturating_sub(1);

        match evaluate_named_policy_step_resetting_on_invalid_state(
            named_policy,
            &properties,
            current_state.as_ref(),
        ) {
            Ok((new_state, RetryVerdict::Retry(delay))) => {
                debug!(
                    retry_policy = %named_policy.name,
                    retry_path = "trap",
                    retry_policy_source = "worker-local",
                    retry_decision = "retry",
                    delay_ms = delay.as_millis() as u64,
                    attempt = total_attempts_before_current + 1,
                    trap = ?trap_type,
                    "Semantic trap retry: delaying"
                );
                (RetryDecision::Delayed(delay), Some(new_state))
            }
            Ok((new_state, RetryVerdict::GiveUp)) => {
                debug!(
                    retry_policy = %named_policy.name,
                    retry_path = "trap",
                    retry_policy_source = "worker-local",
                    retry_decision = "give-up",
                    attempt = total_attempts_before_current + 1,
                    trap = ?trap_type,
                    "Semantic trap retry: exhausted"
                );
                (RetryDecision::None, Some(new_state.exhausted()))
            }
            Ok((_new_state, RetryVerdict::Error(error))) => {
                warn!(
                    retry_policy = %named_policy.name,
                    ?error,
                    retry_path = "trap",
                    fallback_reason = "eval-error",
                    trap = ?trap_type,
                    "Semantic trap retry policy evaluation returned an error verdict; giving up"
                );
                (RetryDecision::None, None)
            }
            Err(error) => {
                warn!(
                    retry_policy = %named_policy.name,
                    ?error,
                    retry_path = "trap",
                    fallback_reason = "eval-error",
                    trap = ?trap_type,
                    "Failed evaluating semantic trap retry policy; giving up"
                );
                (RetryDecision::None, None)
            }
        }
    }

    async fn emit_log_event(&self, event: InternalWorkerEvent) {
        if let Some(entry) = event.as_oplog_entry()
            && let OplogEntry::Log {
                level,
                context,
                message,
                ..
            } = &entry
        {
            // Oplog processor plugin logs are emitted into the server log because
            // they cannot be easily watched with CLI tools.
            if self.state.component_metadata.metadata.has_oplog_processor() {
                let agent_id = &self.owned_agent_id;
                match level {
                    LogLevel::Stdout | LogLevel::Debug | LogLevel::Trace => {
                        tracing::debug!(
                            plugin_agent = %agent_id,
                            context,
                            "Plugin: {message}"
                        );
                    }
                    LogLevel::Stderr | LogLevel::Info => {
                        tracing::info!(
                            plugin_agent = %agent_id,
                            context,
                            "Plugin: {message}"
                        );
                    }
                    LogLevel::Warn => {
                        tracing::warn!(
                            plugin_agent = %agent_id,
                            context,
                            "Plugin: {message}"
                        );
                    }
                    LogLevel::Error | LogLevel::Critical => {
                        tracing::error!(
                            plugin_agent = %agent_id,
                            context,
                            "Plugin: {message}"
                        );
                    }
                }
            }

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
                            self.public_state.worker().add_to_oplog(entry).await;
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
                        & !self
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

    pub async fn begin_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        if (*function_type == DurableFunctionType::WriteRemote && !self.state.assume_idempotence)
            || matches!(
                *function_type,
                DurableFunctionType::WriteRemoteBatched(None)
            )
        {
            let result = if self.is_live() {
                let begin_index = self
                    .public_state
                    .worker()
                    .add_and_commit_oplog(OplogEntry::begin_remote_write())
                    .await;
                Ok(begin_index)
            } else {
                let (begin_index, _) =
                    crate::get_oplog_entry!(self.state.replay_state, OplogEntry::BeginRemoteWrite)?;
                if !self.state.assume_idempotence
                    && !matches!(
                        *function_type,
                        DurableFunctionType::WriteRemoteBatched(None)
                    )
                {
                    let end_index = self
                        .state
                        .replay_state
                        .lookup_oplog_entry(begin_index, OplogEntry::is_end_remote_write)
                        .await;
                    if end_index.is_none() {
                        // Must switch to live mode before failing to be able to commit an Error entry
                        self.state.replay_state.switch_to_live().await;
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
                    let lookup_result = self
                        .state
                        .replay_state
                        .lookup_oplog_entry_with_condition_and_state(
                            begin_index,
                            OplogEntry::is_end_remote_write_s::<PersistenceLevel>,
                            OplogEntry::no_concurrent_side_effect,
                            self.state.persistence_level,
                            OplogEntry::track_persistence_level,
                        )
                        .await;
                    match lookup_result {
                        OplogEntryLookupResult::Found { index, .. } => {
                            debug!(
                                "Remote write operation {begin_index} already completed at {index}, continue replaying"
                            );
                            Ok(begin_index)
                        }
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: true,
                        } => {
                            // Must switch to live mode before failing to be able to commit an Error entry
                            self.state.replay_state.switch_to_live().await;
                            Err(WorkerExecutorError::runtime(
                                "Non-idempotent remote write operation was not completed, cannot retry",
                            ))
                        }
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: false,
                        } if self.state.assume_idempotence => {
                            // We need to jump to the end of the oplog
                            self.state.replay_state.switch_to_live().await;

                            // But this is not enough, because if the retried batched write operation succeeds,
                            // and later we replay it, we need to skip the first attempt and only replay the second.
                            // Se we add a Jump entry to the oplog that registers a deleted region.
                            let deleted_region = OplogRegion {
                                start: begin_index.next(), // need to keep the BeginAtomicRegion entry
                                end: self.state.replay_state.replay_target().next(), // skipping the Jump entry too
                            };

                            self.public_state
                                .worker()
                                .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                                .await;

                            // TODO: this recomputation should not be necessary.
                            self.public_state.worker().reattach_worker_status().await;
                            Ok(begin_index)
                        }
                        OplogEntryLookupResult::NotFound { .. } => {
                            // assume_idempotence is false and the operation was not completed —
                            // we cannot safely retry a non-idempotent batched write.
                            self.state.replay_state.switch_to_live().await;
                            Err(WorkerExecutorError::runtime(
                                "Non-idempotent remote write operation was not completed, cannot retry",
                            ))
                        }
                    }
                } else {
                    Ok(begin_index)
                }
            }?;

            // A `BeginRemoteWrite` region (remote write / HTTP request) is now open until the
            // matching `end_function`; the tip is inside it, so block mid-invocation checkpoints.
            self.state.open_rollback_regions.insert(result);

            // The current retry point will point to the BeginRemoteWrite entry
            self.state.current_retry_point = result;
            Ok(result)
        } else {
            // When there is no BeginRemoteWrite entry, the current retry point can only
            // point to the last written non-hint entry. Hint entries must be ignored
            // because they are nondeterministic.
            // If the entry belongs to an open batched write or transaction, we need to
            // set the current retry point to the index of the begin entry.
            // The returned index, however, is going to be the current / last replayed index.

            let begin_index = if self.state.replay_state.is_live() {
                self.state.oplog.current_oplog_index().await
            } else {
                self.state.replay_state.last_replayed_non_hint_index()
            };

            let new_retry_point = match function_type {
                DurableFunctionType::WriteRemoteBatched(Some(idx)) => *idx,
                DurableFunctionType::WriteRemoteTransaction(Some(idx)) => *idx,
                _ => self
                    .state
                    .oplog
                    .last_added_non_hint_entry()
                    .await
                    .unwrap_or(self.state.replay_state.last_replayed_non_hint_index()),
            };
            self.state.current_retry_point = new_retry_point;

            Ok(begin_index)
        }
    }

    pub async fn end_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if (*function_type == DurableFunctionType::WriteRemote && !self.state.assume_idempotence)
            || matches!(
                *function_type,
                DurableFunctionType::WriteRemoteBatched(None)
            )
        {
            if self.is_live() {
                self.state
                    .oplog
                    .add(OplogEntry::end_remote_write(begin_index))
                    .await;
            } else {
                let (_, _) =
                    crate::get_oplog_entry!(self.state.replay_state, OplogEntry::EndRemoteWrite)?;
            }
            // The `BeginRemoteWrite` region opened in `begin_function` is now closed.
            self.state.open_rollback_regions.remove(&begin_index);
            Ok(())
        } else {
            Ok(())
        }
    }

    /// Best-effort mid-invocation clean status checkpoint. Called from `end_durable_function` after
    /// it commits, so the worker's `last_known_status` reflects the committed tip. Writes a
    /// checkpoint only when we are at a structurally clean boundary (no open rollback region) and
    /// the committed tip is at/below the `get_oplog_index` marker watermark; otherwise it is a
    /// cheap no-op. The actual write is further throttled by the checkpointer.
    async fn maybe_mid_invocation_checkpoint(&self) {
        if !self.state.at_clean_checkpoint_boundary() {
            return;
        }
        self.public_state
            .worker()
            .checkpoint_status_mid_invocation(self.state.min_exposed_marker)
            .await;
    }

    pub async fn begin_transaction_function<Tx, Err>(
        &mut self,
        handler: impl RemoteTransactionHandler<Tx, Err>,
    ) -> Result<(OplogIndex, Tx), Err>
    where
        Err: From<WorkerExecutorError>,
    {
        if self.is_live() {
            let (tx_id, tx) = handler.create_new().await?;
            let begin_index = self
                .public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::begin_remote_transaction(tx_id, None))
                .await;

            // A remote transaction region is now open until commit/rollback; block checkpoints.
            self.state.open_rollback_regions.insert(begin_index);
            self.state.current_retry_point = begin_index;

            Ok((begin_index, tx))
        } else {
            let (begin_index, begin_entry) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::BeginRemoteTransaction
            )?;
            let original_begin_index = if let OplogEntry::BeginRemoteTransaction {
                original_begin_index: Some(idx),
                ..
            } = &begin_entry
            {
                *idx
            } else {
                begin_index
            };

            let assume_idempotence = self.state.assume_idempotence;

            let pre_entry = self
                .state
                .replay_state
                .lookup_oplog_entry_with_condition_and_state(
                    original_begin_index,
                    OplogEntry::is_pre_remote_transaction_s,
                    OplogEntry::no_concurrent_side_effect,
                    self.state.persistence_level,
                    OplogEntry::track_persistence_level,
                )
                .await;

            let tx_id = try_match!(
                begin_entry,
                OplogEntry::BeginRemoteTransaction {
                    timestamp: _,
                    transaction_id,
                    original_begin_index: _,
                }
            )
            .map_err(|_| WorkerExecutorError::runtime("Unexpected oplog entry"))?;

            let (tx_id, tx) = handler.create_replay(&tx_id).await?;

            let mut should_restart = false;

            match pre_entry {
                OplogEntryLookupResult::Found {
                    entry: pre_entry, ..
                } => {
                    let end_entry = self
                        .state
                        .replay_state
                        .lookup_oplog_entry_with_condition_and_state(
                            original_begin_index,
                            OplogEntry::is_end_remote_transaction_s,
                            OplogEntry::no_concurrent_side_effect,
                            self.state.persistence_level,
                            OplogEntry::track_persistence_level,
                        )
                        .await;

                    match end_entry {
                        OplogEntryLookupResult::Found { .. } => {}
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: false,
                        } => {
                            if pre_entry.is_pre_commit_remote_transaction(original_begin_index) {
                                // if we can not confirm the transaction was committed, we need to restart
                                should_restart = !handler.is_committed(&tx_id).await?;
                            } else if pre_entry
                                .is_pre_commit_remote_transaction(original_begin_index)
                            {
                                // if we can not confirm the transaction was rolled back, we need to restart
                                should_restart = !handler.is_rolled_back(&tx_id).await?;
                            }
                        }
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: true,
                        } => {
                            // Must switch to live mode before failing to be able to commit an Error entry
                            self.state.replay_state.switch_to_live().await;
                            return Err(WorkerExecutorError::runtime(
                                "Transaction overlapped with other side effects was not completed, cannot retry",
                            ).into());
                        }
                    }
                }
                OplogEntryLookupResult::NotFound {
                    violates_for_all: false,
                } => {
                    should_restart = true;
                }
                OplogEntryLookupResult::NotFound {
                    violates_for_all: true,
                } => {
                    // Must switch to live mode before failing to be able to commit an Error entry
                    self.state.replay_state.switch_to_live().await;
                    return Err(WorkerExecutorError::runtime(
                        "Transaction overlapped with other side effects was not completed, cannot retry",
                    ).into());
                }
            };

            let (result, tx) = if should_restart {
                // We need to jump to the end of the oplog
                self.state.replay_state.switch_to_live().await;

                if !assume_idempotence {
                    Err(WorkerExecutorError::runtime(
                        "Non-idempotent remote write operation was not completed, cannot retry",
                    ))
                } else {
                    // But this is not enough, because if the retried batched write operation succeeds,
                    // and later we replay it, we need to skip the first attempt and only replay the second.
                    // Se we add a Jump entry to the oplog that registers a deleted region.
                    let deleted_region = OplogRegion {
                        start: begin_index, // need to delete the previous BeginRemoteTransaction entry, because we'll get a new TX id
                        end: self.state.replay_state.replay_target().next(), // skipping the Jump entry too
                    };

                    self.public_state
                        .worker()
                        .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                        .await;

                    // TODO: this recomputation should not be necessary.
                    self.public_state.worker().reattach_worker_status().await;

                    let (tx_id, tx) = handler.create_new().await?;
                    let _ = self
                        .public_state
                        .worker()
                        .add_and_commit_oplog(OplogEntry::begin_remote_transaction(
                            tx_id,
                            Some(original_begin_index),
                        ))
                        .await;

                    Ok((original_begin_index, tx))
                }
            } else {
                Ok((original_begin_index, tx))
            }?;

            // The (possibly re-begun) remote transaction region is open until commit/rollback.
            self.state.open_rollback_regions.insert(result);
            self.state.current_retry_point = original_begin_index;

            Ok((result, tx))
        }
    }

    pub async fn pre_commit_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            self.state
                .oplog
                .fallible_add(OplogEntry::pre_commit_remote_transaction(begin_index))
                .await
                .map_err(WorkerExecutorError::runtime)?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
            Ok(())
        } else {
            let (_, _) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::PreCommitRemoteTransaction
            )?;
            Ok(())
        }
    }

    pub async fn pre_rollback_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            self.state
                .oplog
                .fallible_add(OplogEntry::pre_rollback_remote_transaction(begin_index))
                .await
                .map_err(WorkerExecutorError::runtime)?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
            Ok(())
        } else {
            let (_, _) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::PreRollbackRemoteTransaction
            )?;
            Ok(())
        }
    }

    pub async fn committed_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            self.state
                .oplog
                .fallible_add(OplogEntry::committed_remote_transaction(begin_index))
                .await
                .map_err(WorkerExecutorError::runtime)?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
        } else {
            let (_, _) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::CommittedRemoteTransaction
            )?;
        }
        // The remote transaction region opened in `begin_transaction_function` is now closed: the
        // `CommittedRemoteTransaction` entry has been durably committed (live) or replayed, so the
        // tip is no longer inside a jumpable region on its account.
        self.state.open_rollback_regions.remove(&begin_index);
        // The live branch above just committed/updated the status, so this is a clean boundary at
        // the committed tip (the helper is a no-op during replay and while any other region is
        // open) — a good place to advance the mid-invocation checkpoint for transaction-heavy
        // invocations.
        self.maybe_mid_invocation_checkpoint().await;
        Ok(())
    }

    pub async fn rolled_back_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            self.state
                .oplog
                .fallible_add(OplogEntry::rolled_back_remote_transaction(begin_index))
                .await
                .map_err(WorkerExecutorError::runtime)?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
        } else {
            let (_, _) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::RolledBackRemoteTransaction
            )?;
        }
        // The remote transaction region opened in `begin_transaction_function` is now closed: the
        // `RolledBackRemoteTransaction` entry has been durably committed (live) or replayed, so the
        // tip is no longer inside a jumpable region on its account.
        self.state.open_rollback_regions.remove(&begin_index);
        // The live branch above just committed/updated the status, so this is a clean boundary at
        // the committed tip (the helper is a no-op during replay and while any other region is
        // open) — a good place to advance the mid-invocation checkpoint for transaction-heavy
        // invocations.
        self.maybe_mid_invocation_checkpoint().await;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn finalize_pending_snapshot_update(
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        let pending_update = store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .state
            .pending_update
            .lock()
            .await
            .take();
        match pending_update {
            Some(TimestampedUpdateDescription {
                description: description @ UpdateDescription::SnapshotBased { .. },
                ..
            }) => {
                let target_revision = *description.target_revision();

                debug!("Finalizing snapshot update to revision {target_revision}");

                match store
                    .as_context_mut()
                    .data_mut()
                    .get_public_state()
                    .oplog()
                    .get_upload_description_payload(description)
                    .await
                {
                    Ok(Some((data, mime_type))) => {
                        let component_metadata = store
                            .as_context()
                            .data()
                            .component_metadata()
                            .metadata
                            .clone();

                        let idempotency_key = IdempotencyKey::fresh();
                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .set_current_idempotency_key(idempotency_key.clone())
                            .await;

                        let load_snapshot_invocation = AgentInvocation::LoadSnapshot {
                            idempotency_key,
                            snapshot: RawSnapshotData { data, mime_type },
                        };
                        let agent_id = store.as_context().data().parsed_agent_id();
                        let lowered = match lower_invocation(
                            load_snapshot_invocation,
                            &component_metadata,
                            agent_id.as_ref(),
                        ) {
                            Ok(lowered) => lowered,
                            Err(err) => {
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_worker_update_failed(
                                        target_revision,
                                        Some(format!(
                                            "Manual update failed to lower load-snapshot invocation: {err}"
                                        )),
                                    )
                                    .await;
                                return Ok(Some(RetryDecision::Immediate));
                            }
                        };

                        let invocation_context = InvocationContextStack::fresh();
                        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
                        if let Err(err) = store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .set_current_invocation_context(invocation_context)
                            .await
                        {
                            store
                                .as_context_mut()
                                .data_mut()
                                .on_worker_update_failed(
                                    target_revision,
                                    Some(format!(
                                        "Manual update failed to install invocation context: {err}"
                                    )),
                                )
                                .await;
                            return Ok(Some(RetryDecision::Immediate));
                        }

                        store
                            .as_context_mut()
                            .data_mut()
                            .begin_call_snapshotting_function();

                        let load_result = invoke_observed_and_traced(
                            lowered,
                            store,
                            instance,
                            InvocationMode::Replay,
                        )
                        .await;

                        store
                            .as_context_mut()
                            .data_mut()
                            .end_call_snapshotting_function();

                        for span_id in local_span_ids {
                            let _ = store
                                .as_context_mut()
                                .data_mut()
                                .durable_ctx_mut()
                                .remove_span(&span_id);
                        }
                        for span_id in inherited_span_ids {
                            let _ = store
                                .as_context_mut()
                                .data_mut()
                                .durable_ctx_mut()
                                .remove_span(&span_id);
                        }

                        let failed = match load_result {
                            Err(error) => {
                                Some(format!("Manual update failed to load snapshot: {error}"))
                            }
                            Ok(InvokeResult::Failed { error, .. }) => {
                                let stderr = store
                                    .as_context()
                                    .data()
                                    .get_public_state()
                                    .event_service()
                                    .get_last_invocation_errors();
                                let error = error.to_string(&stderr);
                                Some(format!("Manual update failed to load snapshot: {error}"))
                            }
                            Ok(InvokeResult::Succeeded {
                                result: AgentInvocationResult::LoadSnapshot { error },
                                ..
                            }) => {
                                error.map(|e| format!("Manual update failed to load snapshot: {e}"))
                            }
                            Ok(InvokeResult::Succeeded { .. }) => Some(
                                "Unexpected result value from the snapshot load function"
                                    .to_string(),
                            ),
                            _ => None,
                        };

                        if let Some(error) = failed {
                            store
                                .as_context_mut()
                                .data_mut()
                                .on_worker_update_failed(target_revision, Some(error))
                                .await;
                            Ok(Some(RetryDecision::Immediate))
                        } else {
                            let component_metadata =
                                store.as_context().data().component_metadata().clone();
                            let agent_type_provision_config = store
                                .as_context()
                                .data()
                                .agent_type_provision_config()
                                .cloned();

                            store
                                .as_context_mut()
                                .data_mut()
                                .on_worker_update_succeeded(
                                    target_revision,
                                    component_metadata.component_size,
                                    HashSet::from_iter(
                                        agent_type_provision_config
                                            .into_iter()
                                            .flat_map(|c| c.plugins)
                                            .map(|installation| {
                                                installation.environment_plugin_grant_id
                                            }),
                                    ),
                                )
                                .await;
                            Ok(None)
                        }
                    }
                    Ok(None) => {
                        store
                            .as_context_mut()
                            .data_mut()
                            .on_worker_update_failed(
                                target_revision,
                                Some("Failed to find snapshot data for update".to_string()),
                            )
                            .await;
                        Ok(Some(RetryDecision::Immediate))
                    }
                    Err(error) => {
                        store
                            .as_context_mut()
                            .data_mut()
                            .on_worker_update_failed(target_revision, Some(error))
                            .await;
                        Ok(Some(RetryDecision::Immediate))
                    }
                }
            }
            _ => Err(WorkerExecutorError::runtime(
                "`finalize_pending_snapshot_update` can only be called with a snapshot update description",
            )),
        }
    }

    async fn try_load_snapshot(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        instance: &Instance,
    ) -> SnapshotRecoveryResult {
        let snapshot_index = store
            .as_context()
            .data()
            .durable_ctx()
            .state
            .last_snapshot_index;

        let snapshot_index = match snapshot_index {
            Some(idx) => idx,
            None => return SnapshotRecoveryResult::NotAttempted,
        };

        debug!("Attempting snapshot-based recovery from oplog index {snapshot_index}");

        let oplog_entry = store
            .as_context()
            .data()
            .get_public_state()
            .oplog()
            .read(snapshot_index)
            .await;

        let (data_payload, mime_type) = match oplog_entry {
            OplogEntry::Snapshot {
                data, mime_type, ..
            } => (data, mime_type),
            OplogEntry::PendingUpdate {
                description:
                    UpdateDescription::SnapshotBased {
                        payload, mime_type, ..
                    },
                ..
            } => (payload, mime_type),
            _ => {
                warn!(
                    "Expected Snapshot entry at oplog index {snapshot_index}, found different entry; falling back to full replay"
                );
                if let Err(err) = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .replay_state
                    .drop_override_and_restart()
                    .await
                {
                    warn!("Failed to restart replay state after invalid snapshot entry: {err}");
                    return SnapshotRecoveryResult::Failed;
                }
                return SnapshotRecoveryResult::NotAttempted;
            }
        };

        let data = match store
            .as_context()
            .data()
            .get_public_state()
            .oplog()
            .download_payload(data_payload)
            .await
        {
            Ok(data) => data,
            Err(err) => {
                warn!("Failed to download snapshot payload: {err}; falling back to full replay");
                if let Err(err) = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .replay_state
                    .drop_override_and_restart()
                    .await
                {
                    warn!("Failed to restart replay state after snapshot download failure: {err}");
                    return SnapshotRecoveryResult::Failed;
                }
                return SnapshotRecoveryResult::NotAttempted;
            }
        };

        let component_metadata = store
            .as_context()
            .data()
            .component_metadata()
            .metadata
            .clone();

        let idempotency_key = IdempotencyKey::fresh();
        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .set_current_idempotency_key(idempotency_key.clone())
            .await;

        let load_snapshot_invocation = AgentInvocation::LoadSnapshot {
            idempotency_key,
            snapshot: RawSnapshotData { data, mime_type },
        };
        let agent_id = store.as_context().data().parsed_agent_id();
        let lowered = match lower_invocation(
            load_snapshot_invocation,
            &component_metadata,
            agent_id.as_ref(),
        ) {
            Ok(lowered) => lowered,
            Err(err) => {
                warn!("Snapshot recovery failed to lower load-snapshot invocation: {err}");
                return SnapshotRecoveryResult::Failed;
            }
        };

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        if let Err(err) = store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .set_current_invocation_context(invocation_context)
            .await
        {
            warn!("Snapshot recovery failed to install invocation context: {err}");
            return SnapshotRecoveryResult::Failed;
        }

        store
            .as_context_mut()
            .data_mut()
            .begin_call_snapshotting_function();

        let load_result =
            invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await;

        store
            .as_context_mut()
            .data_mut()
            .end_call_snapshotting_function();

        for span_id in local_span_ids {
            let _ = store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .remove_span(&span_id);
        }
        for span_id in inherited_span_ids {
            let _ = store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .remove_span(&span_id);
        }

        let failed = match load_result {
            Err(error) => Some(format!(
                "Snapshot recovery failed to load snapshot: {error}"
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
                    "Snapshot recovery failed to load snapshot: {error}"
                ))
            }
            Ok(InvokeResult::Succeeded {
                result: AgentInvocationResult::LoadSnapshot { error },
                ..
            }) => error.map(|e| format!("Snapshot recovery load-snapshot returned error: {e}")),
            Ok(InvokeResult::Succeeded { .. }) => {
                Some("Unexpected result value from load-snapshot function".to_string())
            }
            Ok(_) => Some("Snapshot recovery interrupted".to_string()),
        };

        if let Some(error) = failed {
            warn!("{error}; re-creating instance for full replay");
            SnapshotRecoveryResult::Failed
        } else {
            debug!("Snapshot loaded successfully from oplog index {snapshot_index}");
            SnapshotRecoveryResult::Success
        }
    }
}

enum SnapshotRecoveryResult {
    Success,
    NotAttempted,
    Failed,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn register_open_websocket(
        &mut self,
        rep: u32,
        url: String,
        headers: Option<Vec<(String, String)>>,
    ) {
        self.state
            .open_websocket_connections
            .insert(rep, WebSocketConnectionState { url, headers });
    }

    /// Returns `Ok(())` if the host is in normal strictness mode, or if the host is in read-only
    /// strictness but the call site has not been restricted (this function is a no-op in normal
    /// mode).
    ///
    /// Returns `Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation)` if the host is in read-only
    /// strictness mode. The error carries the agent method name and the host function name, so
    /// the trap can later be converted to a typed `AgentError::ReadOnlyViolation`.
    ///
    /// Call sites should invoke this at the very top of any host function that introduces a
    /// remote side effect (outgoing HTTP, RPC) before any durability machinery runs.
    pub fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        if self.state.invocation_strictness == InvocationStrictness::ReadOnly {
            let method = self.state.read_only_method_name.clone().unwrap_or_default();
            Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function: host_function.to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Returns the current invocation strictness mode.
    pub fn invocation_strictness(&self) -> InvocationStrictness {
        self.state.invocation_strictness
    }

    pub(crate) fn unregister_open_websocket(&mut self, rep: u32) {
        self.state.open_websocket_connections.remove(&rep);
    }

    pub(crate) fn websocket_connection_info(&self, rep: u32) -> Option<WebSocketConnectionInfo> {
        self.state
            .open_websocket_connections
            .get(&rep)
            .map(|state| WebSocketConnectionInfo {
                url: state.url.clone(),
                headers: state.headers.clone(),
            })
    }

    pub async fn process_pending_replay_events(&mut self) -> Result<(), WorkerExecutorError> {
        let replay_events = self.state.replay_state.take_new_replay_events().await;
        if !replay_events.is_empty() {
            debug!("Applying pending side effects accumulated during replay");
        }
        for event in replay_events {
            match event {
                ReplayEvent::UpdateReplayed { new_revision } => {
                    debug!("Updating worker state to component metadata revision {new_revision}");
                    self.update_state_to_new_component_revision(new_revision)
                        .await?;
                }
                ReplayEvent::ForkReplayed { new_phantom_id } => {
                    debug!("Updating the replay's current phantom id to {new_phantom_id}");
                    self.update_state_to_new_phantom_id(new_phantom_id).await?;
                }
                ReplayEvent::ReplayFinished => {
                    debug!("Replaying oplog finished");
                    let pending_update = self.state.pending_update.lock().await.take();

                    let pending_update = if let Some(pending_update) = pending_update {
                        pending_update
                    } else {
                        continue;
                    };

                    match pending_update.description {
                        UpdateDescription::Automatic { target_revision } => {
                            debug!("Finalizing pending automatic update");

                            if let Err(error) = self
                                .update_state_to_new_component_revision(target_revision)
                                .await
                            {
                                let stringified_error =
                                    format!("Applying worker update failed: {error}");

                                self.on_worker_update_failed(
                                    target_revision,
                                    Some(stringified_error),
                                )
                                .await;

                                Err(error)?
                            };

                            let component_metadata = self.component_metadata().clone();

                            self.on_worker_update_succeeded(
                                target_revision,
                                component_metadata.component_size,
                                HashSet::from_iter({
                                    self.agent_type_provision_config()
                                        .map(|c| c.plugins.as_slice())
                                        .unwrap_or_default()
                                        .iter()
                                        .map(|installation| {
                                            installation.environment_plugin_grant_id
                                        })
                                }),
                            )
                            .await;

                            debug!("Finalizing automatic update to revision {target_revision}");
                        }
                        _ => {
                            return Err(WorkerExecutorError::runtime(
                                "pending replay event finalization expected an automatic update description",
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn update_state_to_new_phantom_id(
        &mut self,
        new_phantom_id: Uuid,
    ) -> Result<(), WorkerExecutorError> {
        self.state.current_phantom_id = Some(new_phantom_id);
        Ok(())
    }

    pub async fn update_state_to_new_component_revision(
        &mut self,
        new_revision: ComponentRevision,
    ) -> Result<(), WorkerExecutorError> {
        let current_metadata = &self.state.component_metadata;

        if new_revision <= current_metadata.revision {
            debug!("Update {new_revision} was already applied, skipping");
            return Ok(());
        };

        let new_metadata = self
            .component_service()
            .get_metadata(self.owned_agent_id.component_id(), Some(new_revision))
            .await?;

        let new_agent_type_provision_configs = self.parsed_agent_id().and_then(|aid| {
            new_metadata
                .metadata
                .agent_type_provision_configs()
                .get(&aid.agent_type)
                .cloned()
        });

        let mut current_files = self.state.files.write().await;
        update_filesystem(
            &mut current_files,
            &self.state.file_loader,
            self.owned_agent_id.environment_id,
            self.worker_dir.path(),
            new_agent_type_provision_configs
                .as_ref()
                .map(|c| c.files.as_slice())
                .unwrap_or_default(),
        )
        .await?;

        let mut read_only_paths = self.state.read_only_paths.write().unwrap();
        *read_only_paths = compute_read_only_paths(&current_files);

        if let Some(agent_id) = self.parsed_agent_id() {
            let agent_type = new_metadata
                .metadata
                .find_agent_type_by_name(&agent_id.agent_type)
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(format!(
                        "Agent type {} not found in updated agent metadata",
                        agent_id.agent_type
                    ))
                })?;

            let updated_agent_config = effective_agent_config(
                self.state.initial_agent_config.clone(),
                new_agent_type_provision_configs
                    .as_ref()
                    .map(|c| c.config.clone())
                    .unwrap_or_default(),
            );

            validate_agent_config(&updated_agent_config, &agent_type)?;

            self.state.agent_config = updated_agent_config;
            self.state.cached_agent_config_retry_policies = None;
        };

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
        let invocation_context = invocation_context
            .limit_depth(self.state.config.limits.max_invocation_context_stack_depth);
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
            .limit_depth(self.state.config.limits.max_invocation_context_stack_depth)
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
            ExecutionStatus::Interrupting { interrupt_kind, .. } => Some(*interrupt_kind),
            _ => None,
        }
    }

    fn set_suspended(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { agent_mode, .. } => {
                *execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };
            }
            ExecutionStatus::Suspended { .. } => {}
            ExecutionStatus::Interrupting {
                agent_mode,
                await_interruption,
                ..
            } => {
                *execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };
                await_interruption.send(()).ok();
            }
            ExecutionStatus::Loading { agent_mode, .. } => {
                *execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };
            }
        };
    }

    fn set_running(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { .. } => {}
            ExecutionStatus::Suspended { agent_mode, .. } => {
                let (tx, _) = tokio::sync::broadcast::channel(128);
                let interrupt_signal = Arc::new(tx);
                *execution_status = ExecutionStatus::Running {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal,
                };
            }
            ExecutionStatus::Interrupting { .. } => {}
            ExecutionStatus::Loading { agent_mode, .. } => {
                let (tx, _) = tokio::sync::broadcast::channel(128);
                let interrupt_signal = Arc::new(tx);
                *execution_status = ExecutionStatus::Running {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal,
                };
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_agent_invocation_started(
        &mut self,
        mut invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.snapshotting_mode.is_none() {
            let stack = self.get_current_invocation_context().await;

            match &mut invocation {
                AgentInvocation::AgentInitialization {
                    invocation_context, ..
                } => {
                    *invocation_context = stack;
                }
                AgentInvocation::AgentMethod {
                    invocation_context, ..
                } => {
                    *invocation_context = stack;
                }
                _ => {}
            }

            self.public_state
                .worker()
                .oplog()
                .add_agent_invocation_started(invocation)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "could not encode agent invocation on {}: {err}",
                        self.agent_id()
                    )
                });

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
        }
        Ok(())
    }

    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> RetryDecision {
        let current_idempotency_key = self.get_current_idempotency_key().await;

        if let TrapType::Error { error, .. } = trap_type {
            match error {
                AgentError::EphemeralSleepTooLong(_) => {
                    record_non_suspending_failure("sleep-too-long")
                }
                AgentError::EphemeralFuelExhausted(_) => {
                    record_non_suspending_failure("fuel-exhausted")
                }
                AgentError::EphemeralCannotSuspend(_) => {
                    record_non_suspending_failure("cannot-suspend")
                }
                _ => {}
            }
        }

        // Special case: jumping is always immediate and may not have a non-detached status.
        if matches!(trap_type, TrapType::Interrupt(InterruptKind::Jump)) {
            return RetryDecision::Immediate;
        }

        let in_atomic_region = !self.state.active_atomic_regions.is_empty();

        let latest_status_before = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        let (decision, retry_policy_state) = self
            .get_recovery_decision_on_trap_with_semantic(
                &latest_status_before.current_retry_state,
                trap_type,
                in_atomic_region,
                full_function_name,
            )
            .await;

        let oplog_entry = match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt(_)) => Some(OplogEntry::interrupted()),
            TrapType::Interrupt(InterruptKind::Suspend(_)) => Some(OplogEntry::suspend()),
            TrapType::Interrupt(InterruptKind::Jump) => None,
            TrapType::Interrupt(InterruptKind::Restart) => None,
            TrapType::Exit => Some(OplogEntry::exited()),
            TrapType::Error {
                error: AgentError::InvalidRequest(_),
                ..
            } => current_idempotency_key.map(OplogEntry::cancel_pending_invocation),
            TrapType::Error {
                error, retry_from, ..
            } => {
                let inside_atomic_region = self.state.outermost_atomic_region_has_side_effects();
                Some(OplogEntry::error(
                    error.clone(),
                    *retry_from,
                    inside_atomic_region,
                    retry_policy_state,
                ))
            }
        };

        if let Some(entry) = oplog_entry {
            self.public_state.worker().add_and_commit_oplog(entry).await;
        };

        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;

        let giving_up = matches!(
            trap_type,
            TrapType::Error {
                error: AgentError::InvalidRequest(_),
                ..
            }
        ) || matches!(
            latest_status.status,
            AgentStatus::Interrupted | AgentStatus::Exited
        ) || decision == RetryDecision::None;

        if giving_up {
            // Giving up, associating the stored result with the current and upcoming invocations
            if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                self.public_state
                    .worker()
                    .store_invocation_failure(&idempotency_key, trap_type)
                    .await;

                self.public_state.event_service().emit_invocation_finished(
                    full_function_name,
                    &idempotency_key,
                    self.is_live(),
                );
            }
        }

        debug!(
            "Recovery decision for {trap_type:?} with {:?} retries (in_atomic_region={in_atomic_region}): {:?}",
            latest_status_before.current_retry_state, decision
        );

        decision
    }

    fn enter_read_only_mode(&mut self, method_name: String) {
        if self.state.invocation_strictness == InvocationStrictness::ReadOnly {
            warn!(
                "enter_read_only_mode called while already in read-only mode (current method: {:?}, new method: {})",
                self.state.read_only_method_name, method_name
            );
        }
        self.state.invocation_strictness = InvocationStrictness::ReadOnly;
        self.state.read_only_method_name = Some(method_name);
    }

    fn exit_read_only_mode(&mut self) {
        match self.state.invocation_strictness {
            InvocationStrictness::ReadOnly => {
                self.state.invocation_strictness = InvocationStrictness::Normal;
                self.state.read_only_method_name = None;
            }
            InvocationStrictness::Normal => {
                warn!(
                    "exit_read_only_mode called without a matching enter_read_only_mode; \
                     invocation strictness left as Normal"
                );
            }
        }
    }

    async fn on_agent_invocation_success(
        &mut self,
        full_function_name: &str,
        consumed_fuel: u64,
        output: &mut AgentInvocationOutput,
    ) -> Result<(), WorkerExecutorError> {
        let is_live = self.state.is_live();

        if is_live {
            if self.state.snapshotting_mode.is_none() {
                let component_revision = output.component_revision.ok_or_else(|| {
                    WorkerExecutorError::runtime(
                        "component_revision missing in AgentInvocationOutput during replay",
                    )
                })?;

                // Classify the just-completed invocation up front so we can
                // bump the read-only cache epoch on successful mutating
                // completion. For non-AgentMethod results
                // (initialization, manual update, snapshot
                // load/save, oplog processing) we always invalidate — these
                // are all state-changing. For AgentMethod results we ask the
                // worker whether the method is `read_only`.
                let invalidates_read_only_cache = match &output.result {
                    AgentInvocationResult::AgentMethod { .. } => self
                        .public_state
                        .worker()
                        .agent_method_invalidates_read_only_cache(full_function_name),
                    AgentInvocationResult::AgentInitialization
                    | AgentInvocationResult::ManualUpdate
                    | AgentInvocationResult::LoadSnapshot { .. }
                    | AgentInvocationResult::SaveSnapshot { .. }
                    | AgentInvocationResult::ProcessOplogEntries { .. } => true,
                };

                // Only `AgentMethod` results need the method name persisted so the
                // public oplog renderer can resolve the correct output schema.
                let method_name = match &output.result {
                    AgentInvocationResult::AgentMethod { .. } => {
                        Some(full_function_name.to_string())
                    }
                    _ => None,
                };

                self.public_state
                    .worker()
                    .oplog()
                    .add_agent_invocation_finished(
                        &output.result,
                        method_name,
                        consumed_fuel,
                        component_revision,
                    )
                    .await
                    .unwrap_or_else(|err| {
                        panic!("could not encode function result for {full_function_name}: {err}")
                    });

                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::Always)
                    .await;

                // Bump the read-only cache epoch after the
                // `AgentInvocationFinished` entry is committed, but *before*
                // we publish `InvocationCompleted` to waiters via
                // `store_invocation_success`. Ordering matters: any client
                // that observes the completion event must also see an
                // invalidated cache, otherwise it could read a stale cached
                // result for the now-mutated state.
                if invalidates_read_only_cache {
                    self.public_state.worker().bump_read_only_cache_epoch();
                }

                // Capture the agent's oplog index right after
                // `AgentInvocationFinished` was committed, together with the
                // worker's per-instance fingerprint, so the response carries
                // an unambiguous identification of the agent state it was
                // produced from.
                output.oplog_index = Some(
                    self.public_state
                        .worker()
                        .oplog()
                        .current_oplog_index()
                        .await,
                );
                output.agent_fingerprint = Some(
                    self.public_state
                        .worker()
                        .get_initial_worker_metadata()
                        .fingerprint,
                );

                if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                    self.public_state
                        .worker()
                        .store_invocation_success(&idempotency_key, output.clone())
                        .await;

                    self.public_state.event_service().emit_invocation_finished(
                        full_function_name,
                        &idempotency_key,
                        is_live,
                    );
                }
            }
        } else {
            let response = self
                .state
                .replay_state
                .get_oplog_entry_agent_invocation_finished()
                .await?;
            if let Some(recorded_result) = response
                && !recorded_result.replay_equivalent(&output.result)
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!("{full_function_name} => {recorded_result:?}"),
                    format!("{full_function_name} => {:?}", output.result),
                ));
            }
        }
        debug!("Function {full_function_name} finished");

        Ok(())
    }

    async fn get_current_retry_point(&self) -> OplogIndex {
        if let Some(region) = self.state.active_atomic_regions.last() {
            region.begin_index
        } else {
            self.state.current_retry_point
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> ResourceStore for DurableWorkerCtx<Ctx> {
    fn self_uri(&self) -> Uri {
        self.state.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        let id = self.state.add(resource, name.clone()).await;
        let resource_id = AgentResourceId(id);
        if self.state.is_live() {
            let entry = OplogEntry::create_resource(resource_id, name.clone());
            self.public_state.worker().add_to_oplog(entry).await;
        }
        id
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        let result = self.state.borrow(resource_id).await;
        if let Some((resource_type_id, _)) = &result {
            let id = AgentResourceId(resource_id);
            if self.state.is_live() {
                let entry = OplogEntry::drop_resource(id, resource_type_id.clone());
                self.public_state.worker().add_to_oplog(entry).await;
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
        match self.state.snapshotting_mode.take() {
            Some(level) => {
                self.state.persistence_level = level;
            }
            None => {
                warn!(
                    "end_call_snapshotting_function called without a matching begin_call_snapshotting_function; \
                     leaving persistence level unchanged"
                );
            }
        }
    }

    async fn on_worker_update_failed(
        &self,
        target_revision: ComponentRevision,
        details: Option<String>,
    ) {
        let entry = OplogEntry::failed_update(target_revision, details.clone());
        self.public_state.worker().add_and_commit_oplog(entry).await;

        warn!(
            "Worker failed to update to {}: {}, update attempt aborted",
            target_revision,
            details.unwrap_or_else(|| "?".to_string())
        );
    }

    async fn on_worker_update_succeeded(
        &self,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<
            golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
        >,
    ) {
        info!("Worker update to {} finished successfully", target_revision);
        let entry = OplogEntry::successful_update(
            target_revision,
            new_component_size,
            new_active_plugins.clone(),
        );
        self.public_state.worker().add_and_commit_oplog(entry).await;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationContextManagement for DurableWorkerCtx<Ctx> {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
        activate: bool,
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
        let span_id = self.state.current_span_id.clone();
        let span = self.start_child_span(&span_id, initial_attributes).await?;
        if activate {
            self.state.current_span_id = span.span_id().clone();
        }
        Ok(span)
    }

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
        let current_span_id = &self.state.current_span_id;

        let is_live = self.is_live();

        let span = if is_live {
            self.state
                .invocation_context
                .start_span(parent, None)
                .map_err(WorkerExecutorError::runtime)?
        } else {
            let (_, entry) =
                crate::get_oplog_entry!(self.state.replay_state, OplogEntry::StartSpan)?;

            let (timestamp, span_id) = match entry {
                OplogEntry::StartSpan {
                    timestamp, span_id, ..
                } => (timestamp, span_id),
                other => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "StartSpan",
                        format!("{other:?}"),
                    ));
                }
            };

            let parent_span = self.state.invocation_context.get(parent).map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "parent span {parent} missing during StartSpan replay: {err}"
                ))
            })?;
            let span = InvocationContextSpan::local()
                .with_span_id(span_id)
                .with_start(timestamp)
                .with_parent(parent_span)
                .build();
            self.state.invocation_context.add_span(span.clone());
            span
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
            self.public_state
                .worker()
                .add_to_oplog(OplogEntry::StartSpan {
                    timestamp: span.start().unwrap_or(Timestamp::now_utc()),
                    span_id: span.span_id().clone(),
                    parent: Some(parent.clone()),
                    linked_context_id: span.linked_context().map(|link| link.span_id().clone()),
                    attributes: HashMap::from_iter(initial_attributes.iter().cloned()).into(),
                })
                .await;
        }

        Ok(span)
    }

    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        if &self.state.current_span_id == span_id {
            // Walk up to the parent if it still exists in the invocation context;
            // otherwise fall back to the root.
            let parent_id = self
                .state
                .invocation_context
                .get(span_id)
                .ok()
                .and_then(|span| span.parent().map(|p| p.span_id().clone()));

            self.state.current_span_id = parent_id
                .filter(|id| self.state.invocation_context.get(id).is_ok())
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
            self.public_state
                .worker()
                .add_to_oplog(OplogEntry::finish_span(span_id.clone()))
                .await;
        } else {
            crate::get_oplog_entry!(self.state.replay_state, OplogEntry::FinishSpan)?;
        }

        if &self.state.current_span_id == span_id {
            let span = self.state.invocation_context.get(span_id).map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "span {span_id} missing during finish_span replay: {err}"
                ))
            })?;
            self.state.current_span_id = span
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
            self.public_state
                .worker()
                .add_to_oplog(OplogEntry::set_span_attribute(
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

    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        self.state
            .invocation_context
            .clone_as_inherited_stack(current_span_id)
            .limit_depth(self.state.config.limits.max_invocation_context_stack_depth)
    }
}

pub trait DurableWorkerCtxView<Ctx: WorkerCtx> {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Ctx>;
    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Ctx>;
}

#[async_trait]
impl<Ctx: WorkerCtx> ExternalOperations<Ctx> for DurableWorkerCtx<Ctx> {
    type ExtraDeps = Ctx::ExtraDeps;

    async fn get_last_error_and_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        latest_worker_status: &AgentStatusRecord,
    ) -> Option<LastError> {
        last_error(this, owned_agent_id, agent_mode, latest_worker_status).await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        let mut number_of_replayed_functions = 0;

        if refresh_replay_target {
            let new_target = store
                .as_context()
                .data()
                .durable_ctx()
                .public_state
                .worker()
                .oplog()
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

        let (agent_mode, is_agent) = {
            let component = store.as_context().data().component_metadata();
            (
                store.as_context().data().agent_mode(),
                component.metadata.is_agent(),
            )
        };

        let resume_result = loop {
            let cont = store.as_context().data().durable_ctx().state.is_replay() && // replay while not live
                (agent_mode == AgentMode::Durable || // durable components are fully replayed
                    (number_of_replayed_functions == 0 && is_agent)); // ephemeral agents replay the first (initialize), other ephemerals nothing (deprecated)

            if cont {
                let oplog_entry = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .replay_state
                    .get_oplog_entry_agent_invocation_started()
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
                        break Ok(None);
                    }
                    Ok(Some(replay_state::AgentInvocationStartedEntry {
                        idempotency_key,
                        invocation_payload,
                        invocation_context,
                    })) => {
                        let agent_invocation = AgentInvocation::from_parts(
                            idempotency_key.clone(),
                            invocation_payload,
                            invocation_context.clone(),
                        );

                        let component_metadata = store
                            .as_context()
                            .data()
                            .component_metadata()
                            .metadata
                            .clone();

                        let agent_id = store.as_context().data().parsed_agent_id();
                        let lowered = lower_invocation(
                            agent_invocation,
                            &component_metadata,
                            agent_id.as_ref(),
                        )?;
                        let full_function_name = lowered.display_name.clone();

                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .process_pending_replay_events()
                            .await?;

                        debug!("Replaying function {}", &full_function_name);
                        debug!(
                            "Replay state: {:?}",
                            store.as_context().data().durable_ctx().state.replay_state
                        );
                        let span = span!(
                            Level::INFO,
                            "replaying",
                            function = full_function_name.as_str()
                        );
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
                        let invoke_result = invoke_observed_and_traced(
                            lowered,
                            store,
                            instance,
                            InvocationMode::Replay,
                        )
                        .instrument(span)
                        .await;

                        // We are removing the spans introduced by the invocation. Not calling `finish_span` here,
                        // as it would add FinishSpan oplog entries without corresponding StartSpan ones. Instead,
                        // the oplog processor should assume that spans implicitly created by AgentInvocationStarted
                        // are finished at AgentInvocationFinished.
                        for span_id in local_span_ids {
                            store.as_context_mut().data_mut().remove_span(&span_id)?;
                        }
                        for span_id in inherited_span_ids {
                            store.as_context_mut().data_mut().remove_span(&span_id)?;
                        }

                        match invoke_result {
                            Ok(InvokeResult::Succeeded {
                                result: invocation_result,
                                consumed_fuel,
                            }) => {
                                let component_revision =
                                    store.as_context().data().component_metadata().revision;
                                let mut output = AgentInvocationOutput {
                                    result: invocation_result,
                                    consumed_fuel: Some(consumed_fuel),
                                    invocation_status: None,
                                    component_revision: Some(component_revision),
                                    oplog_index: None,
                                    agent_fingerprint: None,
                                };
                                if let Err(err) = store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_agent_invocation_success(
                                        &full_function_name,
                                        consumed_fuel,
                                        &mut output,
                                    )
                                    .await
                                {
                                    break Err(err);
                                }
                                number_of_replayed_functions += 1;
                                continue;
                            }
                            _ => {
                                let trap_type = match invoke_result {
                                    Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
                                    Err(error) => Some(TrapType::from_error::<Ctx>(
                                        &anyhow!(error),
                                        OplogIndex::INITIAL,
                                        store.as_context().data().agent_mode(),
                                    )),
                                };
                                let decision = match trap_type {
                                    Some(trap_type) => {
                                        let decision = store
                                            .as_context_mut()
                                            .data_mut()
                                            .on_invocation_failure(&full_function_name, &trap_type)
                                            .await;

                                        if decision == RetryDecision::None {
                                            // Cannot retry so we need to fail
                                            match trap_type {
                                                TrapType::Interrupt(_interrupt_kind) => {
                                                    // In case of an interrupt, we return with RetryDecision::None
                                                    // as it is not an error.
                                                }
                                                TrapType::Exit => {
                                                    break Err(WorkerExecutorError::runtime(
                                                        "Process exited",
                                                    ));
                                                }
                                                TrapType::Error { error, .. } => {
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

                                        Some(decision)
                                    }
                                    None => None,
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
                break Ok(None);
            }
        };

        record_number_of_replayed_functions(number_of_replayed_functions);

        resume_result
    }

    async fn prepare_instance(
        agent_id: &AgentId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        debug!("Starting prepare_instance");
        let start = Instant::now();
        store.as_context_mut().data_mut().set_running();

        let prepare_result = if store.as_context().data().agent_mode() == AgentMode::Ephemeral {
            // Ephemeral workers cannot be recovered

            // We have to replay the initialize call for agents:
            let replay_decision = Self::resume_replay(store, instance, false).await;
            record_resume_worker(start.elapsed());

            if replay_decision == Ok(None) {
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

                Ok(None)
            } else {
                replay_decision
            }
        } else {
            let pending_update = store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .pending_update
                .lock()
                .await
                .clone();

            match pending_update {
                Some(timestamped_update) => {
                    match &timestamped_update.description {
                        UpdateDescription::SnapshotBased { .. } => {
                            // If a snapshot based update is pending, no replay should be necessary
                            if !store.as_context().data().durable_ctx().is_live() {
                                return Err(WorkerExecutorError::runtime(
                                    "snapshot-based pending update expected replay state to already be live",
                                ));
                            }

                            Self::finalize_pending_snapshot_update(instance, store).await
                        }
                        UpdateDescription::Automatic {
                            target_revision, ..
                        } => {
                            let replay_result = async {
                                if let SnapshotRecoveryResult::Failed =
                                    Self::try_load_snapshot(store, instance).await
                                {
                                    return Err(WorkerExecutorError::failed_to_resume_worker(
                                        agent_id.clone(),
                                        WorkerExecutorError::runtime("loading snapshot failed"),
                                    ));
                                };
                                // automatic update will be succeeded as part of the replay.
                                let result = Self::resume_replay(store, instance, false).await?;

                                record_resume_worker(start.elapsed());

                                Ok(result)
                            }
                            .await;

                            match replay_result {
                                Err(error) => {
                                    // replay failed. There are two cases here:
                                    // 1. We failed before the update has succeeded. In this case we fail the update and retry the replay.
                                    // 2. We failed after the update has succeeded. In this case we can the original failure.
                                    let final_pending_update = store
                                        .as_context_mut()
                                        .data_mut()
                                        .durable_ctx_mut()
                                        .state
                                        .pending_update
                                        .lock()
                                        .await
                                        .take();

                                    match final_pending_update {
                                        Some(_) => {
                                            // We failed before the update has succeeded. Mark the update as failed and retry
                                            store
                                                .as_context_mut()
                                                .data_mut()
                                                .on_worker_update_failed(
                                                    *target_revision,
                                                    Some(format!(
                                                        "Automatic update failed: {error}"
                                                    )),
                                                )
                                                .await;

                                            debug!(
                                                "Retrying prepare_instance after failed update attempt"
                                            );

                                            Ok(Some(RetryDecision::Immediate))
                                        }
                                        _ => Err(error),
                                    }
                                }
                                _ => replay_result,
                            }
                        }
                    }
                }
                None => match Self::try_load_snapshot(store, instance).await {
                    SnapshotRecoveryResult::Success | SnapshotRecoveryResult::NotAttempted => {
                        let result = Self::resume_replay(store, instance, false).await;
                        record_resume_worker(start.elapsed());
                        result
                    }
                    SnapshotRecoveryResult::Failed => {
                        store
                            .as_context()
                            .data()
                            .get_public_state()
                            .worker()
                            .snapshot_recovery_disabled
                            .store(true, std::sync::atomic::Ordering::Release);
                        Ok(Some(RetryDecision::Immediate))
                    }
                },
            }
        };
        match prepare_result {
            Ok(None) => {
                store.as_context_mut().data_mut().set_suspended();
                Ok(None)
            }
            Ok(other) => Ok(other),
            Err(error) => Err(WorkerExecutorError::failed_to_resume_worker(
                agent_id.clone(),
                error,
            )),
        }
    }

    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), anyhow::Error> {
        this.oplog_processor_plugin()
            .on_shard_assignment_changed()
            .await?;
        let workers = this.worker_service().get_running_workers_in_shards().await;

        debug!(workers = ?workers, "Recovering running workers");

        for worker in workers {
            let owned_agent_id = worker.initial_worker_metadata.owned_agent_id();
            let agent_mode = worker.initial_worker_metadata.agent_mode;
            // A running worker should always have a recoverable oplog (a `Create` entry), so a
            // `None` here is an unexpected invariant violation (e.g. a corrupt/partially-deleted
            // oplog). Isolate the failure to this one agent instead of aborting recovery of every
            // other worker on this executor (which propagating would do — and would also fail
            // executor startup or the shard-assignment RPC, since one poison worker could
            // permanently block this executor from serving its shards).
            let Some(latest_worker_status) = calculate_last_known_status_with_checkpoint(
                this,
                &owned_agent_id,
                agent_mode,
                worker.last_known_status,
            )
            .await
            else {
                error!(
                    agent_id = %owned_agent_id,
                    "Failed to calculate worker status during shard-assignment recovery; skipping agent"
                );
                continue;
            };

            // TODO: there is probably a race here between assignment changing and a suspended worker getting woken up.
            if should_restart_after_shard_assignment_change(&latest_worker_status)
                && let Err(err) = Worker::get_or_create_running(
                    this,
                    &owned_agent_id,
                    None,
                    Vec::new(),
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                    Principal::anonymous(),
                )
                .await
            {
                // Same isolation rationale: don't let one worker that fails to restart abort
                // recovery of the rest. It will be retried on demand on its next invocation.
                error!(
                    agent_id = %owned_agent_id,
                    error = %err,
                    "Failed to restart worker during shard-assignment recovery; skipping agent"
                );
            }
        }

        Ok(())
    }
}

fn should_restart_after_shard_assignment_change(status: &AgentStatusRecord) -> bool {
    matches!(
        status.status,
        AgentStatus::Running | AgentStatus::Idle | AgentStatus::Retrying | AgentStatus::Interrupted
    ) || status.has_pending_work()
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::{PendingInvocationRef, PendingUpdateKind, PendingUpdateRef};
    use test_r::test;

    #[test]
    fn shard_assignment_recovery_restarts_idle_workers_with_pending_invocations() {
        let mut status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };
        status.pending_invocations.push(PendingInvocationRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::INITIAL,
            idempotency_key: None,
            manual_update_target_revision: Some(ComponentRevision::INITIAL),
        });

        assert!(should_restart_after_shard_assignment_change(&status));
    }

    #[test]
    fn shard_assignment_recovery_restarts_idle_workers_with_pending_updates() {
        let mut status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };
        status.pending_updates.push_back(PendingUpdateRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::INITIAL,
            target_revision: ComponentRevision::INITIAL,
            kind: PendingUpdateKind::Automatic,
        });

        assert!(should_restart_after_shard_assignment_change(&status));
    }

    #[test]
    fn shard_assignment_recovery_skips_suspended_workers_without_pending_work() {
        let status = AgentStatusRecord {
            status: AgentStatus::Suspended,
            ..AgentStatusRecord::default()
        };

        assert!(!should_restart_after_shard_assignment_change(&status));
    }

    #[test]
    fn atomic_region_idempotency_key_indexes_start_after_region_begin() {
        let original_region_begin = OplogIndex::from_u64(10);
        let mut next_idempotency_key_oplog_index = original_region_begin.next();

        let first =
            next_atomic_region_idempotency_key_oplog_index(&mut next_idempotency_key_oplog_index);
        let second =
            next_atomic_region_idempotency_key_oplog_index(&mut next_idempotency_key_oplog_index);

        assert_eq!(first, OplogIndex::from_u64(11));
        assert_eq!(second, OplogIndex::from_u64(12));
    }

    #[test]
    fn atomic_region_idempotency_key_indexes_advance_after_each_derivation() {
        let mut next_idempotency_key_oplog_index = OplogIndex::from_u64(11);

        assert_eq!(
            next_atomic_region_idempotency_key_oplog_index(&mut next_idempotency_key_oplog_index),
            OplogIndex::from_u64(11)
        );
        assert_eq!(next_idempotency_key_oplog_index, OplogIndex::from_u64(12));
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> FileSystemReading for DurableWorkerCtx<Ctx> {
    async fn get_file_system_node(
        &self,
        path: &CanonicalFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        let root = self.worker_dir.path();
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
                AgentFilePermissions::ReadOnly
            } else {
                AgentFilePermissions::ReadWrite
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
                    AgentFilePermissions::ReadOnly
                } else {
                    AgentFilePermissions::ReadWrite
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
        path: &CanonicalFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        let root = self.worker_dir.path();
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

// TODO: optimize this and keep the relevant indices for recovering logs in the AgentStatusRecord
async fn last_error<T: HasOplogService + HasConfig>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    latest_worker_status: &AgentStatusRecord,
) -> Option<LastError> {
    let mut idx = this
        .oplog_service()
        .get_last_index(owned_agent_id, agent_mode)
        .await;
    if idx == OplogIndex::NONE {
        None
    } else {
        let mut first_error = None;
        let mut first_retry_from = OplogIndex::NONE;
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
                let oplog_entry = this
                    .oplog_service()
                    .read(owned_agent_id, agent_mode, idx, 1)
                    .await;
                match oplog_entry.first_key_value() {
                    Some((
                        _,
                        OplogEntry::Error {
                            error, retry_from, ..
                        },
                    )) => {
                        if first_retry_from == OplogIndex::NONE || first_retry_from == *retry_from {
                            last_error_index = idx;
                            if first_error.is_none() {
                                first_error = Some(error.clone());
                                first_retry_from = *retry_from;
                            }
                            if idx > OplogIndex::INITIAL {
                                idx = idx.previous();
                                continue;
                            } else {
                                break;
                            }
                        } else {
                            // Found an error entry belonging to another retry point
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
                    Some((
                        _,
                        OplogEntry::AgentInvocationStarted { .. }
                        | OplogEntry::AgentInvocationFinished { .. },
                    )) => {
                        // Retry counting never gets across invocation boundaries
                        break;
                    }
                    Some((_, _)) => {
                        // Skipping non-hint entries as well, but only up to the first error entry that's different, or the beginning
                        // of the last invocation
                        if idx > OplogIndex::INITIAL {
                            idx = idx.previous();
                            continue;
                        } else {
                            break;
                        }
                    }
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
                stderr: recover_stderr_logs(this, owned_agent_id, agent_mode, last_error_index)
                    .await,
                retry_from: first_retry_from,
            }),
            None => None,
        }
    }
}

fn next_atomic_region_idempotency_key_oplog_index(
    next_idempotency_key_oplog_index: &mut OplogIndex,
) -> OplogIndex {
    let result = *next_idempotency_key_oplog_index;
    *next_idempotency_key_oplog_index = next_idempotency_key_oplog_index.next();
    result
}

/// Reads back oplog entries starting from `last_oplog_idx` and collects stderr logs, with a maximum
/// number of entries, and at most until the beginning of the last invocation.
pub(crate) async fn recover_stderr_logs<T: HasOplogService + HasConfig>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_oplog_idx: OplogIndex,
) -> String {
    let max_count = this.config().limits.event_history_size;

    // This might overestimate the size of stderr_entries by the size of current_stderr_entries_batch, but fine as we
    // have at most one pending batch we discard.
    let mut collected_count = 0;
    let mut idx = last_oplog_idx;
    let mut stderr_entries = Vec::new();
    let mut current_stderr_entries_batch = Vec::new();
    let mut first_seen_invocation = None;

    loop {
        // TODO: this could be read in batches to speed up the process
        let oplog_entry = this
            .oplog_service()
            .read(owned_agent_id, agent_mode, idx, 1)
            .await;

        // Because of retries we might have multiple invocation start entries.
        // Read until the first invocation start entry which does not belong to the same invocation (using the trace id)
        match oplog_entry.first_key_value() {
            Some((
                _,
                OplogEntry::Log {
                    level,
                    message,
                    context,
                    ..
                },
            )) if (level == &LogLevel::Warn
                || level == &LogLevel::Error
                || level == &LogLevel::Critical
                || level == &LogLevel::Stderr)
                && collected_count < max_count =>
            {
                if level == &LogLevel::Stderr {
                    current_stderr_entries_batch.push(message.clone());
                } else {
                    let line = format!(
                        "[{}] [{}] {}\n",
                        format!("{level:?}").to_uppercase(),
                        context,
                        message
                    );
                    current_stderr_entries_batch.push(line);
                }
                collected_count += 1;
            }
            Some((
                _,
                OplogEntry::AgentInvocationStarted {
                    idempotency_key, ..
                },
            )) => match &first_seen_invocation {
                None => {
                    first_seen_invocation = Some(idempotency_key.clone());
                    stderr_entries.extend(std::mem::take(&mut current_stderr_entries_batch));
                    if stderr_entries.len() >= max_count {
                        break;
                    };
                }
                Some(expected_idempotency_key) if idempotency_key == expected_idempotency_key => {
                    stderr_entries.extend(std::mem::take(&mut current_stderr_entries_batch));
                    if stderr_entries.len() >= max_count {
                        break;
                    };
                }
                Some(_) => break,
            },
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
    FutureTrailersDrop,
}

/// Tracks conditions that affect whether an HTTP request is eligible for
/// transparent inline retry. Each flag records an event during the request
/// lifecycle that disqualifies one or more retry zones.
#[derive(Debug, Clone, Default)]
pub(crate) struct HttpRetryEligibility {
    /// Whether this request has an in-task retry loop running in the background.
    /// When true, transient errors that reach `get()` are the final result and
    /// should not trigger trap+replay.
    pub has_background_retry: bool,
    /// Set to true when splice()/blocking_splice() is called on the outgoing body stream.
    /// When true, body bytes cannot be fully reconstructed from the oplog.
    pub has_unreconstructable_body: bool,
    /// Set to true when subscribe() is called on the outgoing body output stream.
    /// When true, output stream inline retry is disabled because the pollable
    /// would become stale after resource replacement.
    pub output_stream_subscribed: bool,
    /// Set to true when skip()/blocking_skip() is called on the response body.
    /// When true, resuming-response-body inline retry is disabled because we
    /// cannot verify
    /// the skipped bytes against the retry response.
    pub had_body_skip: bool,
    /// Set to true when OutgoingBody::finish() is called with Some(trailers).
    /// When true, inline retry is disabled because trailers are not persisted
    /// in the oplog and cannot be reconstructed.
    pub has_outgoing_trailers: bool,
    /// Set to true when OutgoingBody::finish() is called.
    /// Awaiting-response retry requires the body to be fully finished before
    /// retrying.
    pub body_finished: bool,
    /// Set to true when the outgoing body resource is dropped before
    /// OutgoingBody::finish() succeeds. Once this happens, the request body is
    /// not fully replayable.
    pub body_closed_without_finish: bool,
    /// Set to true when outgoing body stream writes are replayed from oplog
    /// (rather than executed live). When true, the actual body pipe does NOT
    /// contain the replayed bytes, so the request must be rebuilt from oplog
    /// before finishing the body.
    pub replayed_body_writes: bool,
}

/// Shared state used by the HTTP response future wrapper for requests whose
/// response can arrive before the outgoing body is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpOutgoingBodyState {
    Open,
    Finished,
    Closed,
}

/// Decision computed by the pending-status response wrapper task after the
/// first response arrives early (before the outgoing body is finished).
///
/// The wrapper task starts in [`PendingStatusRetryDecision::Pending`] and
/// transitions exactly once to either [`PendingStatusRetryDecision::Matched`]
/// (an explicit `status-code` retry policy applies to the early response) or
/// [`PendingStatusRetryDecision::NotMatched`] (no policy applies, so the
/// response should be exposed normally). Consumers of the receiving end
/// (`io::streams` write/flush paths) deterministically wait for the
/// transition out of `Pending` instead of relying on cooperative scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingStatusRetryDecision {
    Pending,
    Matched,
    NotMatched,
}

/// State associated with ongoing http requests, on top of the underlying wasi-http implementation
#[derive(Debug, Clone)]
pub(crate) struct HttpRequestState {
    /// Who is responsible for calling end_function and removing entries from the table
    pub close_owner: HttpRequestCloseOwner,
    /// The BeginRemoteWrite entry's index
    pub begin_index: OplogIndex,
    /// Information about the request to be included in the oplog
    pub request: HostRequestHttpRequest,
    /// SpanId
    pub span_id: SpanId,
    /// When tracking is transferred from IncomingBody to InputStream via stream(),
    /// this records the IncomingBody handle so that on stream close we can transfer
    /// tracking back to the body (enabling finish() to then transfer to FutureTrailers).
    pub body_handle: Option<u32>,
    /// The original response status observed by the guest before body consumption.
    /// Response-body resumption only swaps the body stream, so inline retry must
    /// not resume from a retried response that changes the status code visible via
    /// IncomingResponse.
    pub response_status: Option<u16>,
    /// The outgoing body resource handle associated with this request, set when
    /// outgoing_handler::handle() resolves the pending body mapping.
    pub outgoing_body_rep: Option<u32>,
    /// The outgoing body output stream resource handle, set when outgoing_body::write()
    /// creates the stream from the outgoing body.
    pub output_stream_rep: Option<u32>,
    pub use_tls: bool,
    pub connect_timeout: std::time::Duration,
    pub first_byte_timeout: std::time::Duration,
    pub between_bytes_timeout: std::time::Duration,
    /// Notifies a wrapped response future when the outgoing body becomes fully
    /// replayable, or when it is closed before finish and therefore cannot be
    /// held back for status-code retry anymore.
    pub outgoing_body_state: Option<tokio::sync::watch::Sender<HttpOutgoingBodyState>>,
    /// Watched by the pending-status response wrapper to publish whether an
    /// early response (received while the outgoing body is still open) has
    /// matched an explicit status-code retry policy. When the watch transitions
    /// to [`PendingStatusRetryDecision::Matched`], body stream writes may be
    /// accepted into the oplog even if the original transport has already
    /// closed the body pipe; the fully captured body will be used by the
    /// subsequent status-code retry. The decision is published exactly once,
    /// deterministically, so write/flush paths can `wait_for` it instead of
    /// polling and relying on scheduler yields.
    pub pending_status_retry_decision:
        Option<tokio::sync::watch::Receiver<PendingStatusRetryDecision>>,
    /// Retry eligibility flags tracked during the request lifecycle.
    pub retry: HttpRetryEligibility,
}

impl HttpRequestState {
    pub fn outgoing_request_config(&self) -> OutgoingRequestConfig {
        OutgoingRequestConfig {
            use_tls: self.use_tls,
            connect_timeout: self.connect_timeout,
            first_byte_timeout: self.first_byte_timeout,
            between_bytes_timeout: self.between_bytes_timeout,
        }
    }
}

/// Extracted view of the begin_index and request from an HttpRequestState,
/// used when processing outgoing body output stream operations.
#[derive(Debug, Clone)]
pub(crate) struct HttpOutputStreamState {
    pub request_handle: u32,
    pub begin_index: OplogIndex,
    pub request: HostRequestHttpRequest,
}

#[derive(Debug, Clone)]
struct ActiveAtomicRegion {
    begin_index: OplogIndex,
    next_idempotency_key_oplog_index: OplogIndex,
    has_side_effects: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct FilesystemOutputStreamState {
    pub descriptor_rep: u32,
    pub position: Option<u64>,
    pub pending_reservation: Option<PendingFilesystemReservation>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingFilesystemReservation {
    pub base_size: u64,
    pub reserved_growth: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct WebSocketConnectionState {
    pub url: String,
    pub headers: Option<Vec<(String, String)>>,
}

#[derive(Debug, Clone)]
pub(crate) struct WebSocketConnectionInfo {
    pub url: String,
    pub headers: Option<Vec<(String, String)>>,
}

struct PrivateDurableWorkerState {
    // IMPORTANT: commits to the oplog must go via self.public_state.worker().commit_oplog_and_update_state
    oplog_service: Arc<dyn OplogService>,
    oplog: Arc<dyn Oplog>,
    promise_service: Arc<dyn PromiseService>,
    scheduler_service: Arc<dyn SchedulerService>,
    worker_service: Arc<dyn WorkerService>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
    key_value_service: Arc<dyn KeyValueService>,
    blob_store_service: Arc<dyn BlobStoreService>,
    rdbms_service: Arc<dyn RdbmsService>,
    quota_service: Arc<dyn QuotaService>,
    component_service: Arc<dyn ComponentService>,
    agent_types_service: Arc<dyn AgentTypesService>,
    agent_webhooks_service: Arc<AgentWebhooksService>,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    config: Arc<GolemConfig>,
    owned_agent_id: OwnedAgentId,
    created_by: AccountId,
    agent_id: Option<ParsedAgentId>,
    created_by_email: AccountEmail,
    current_idempotency_key: Option<IdempotencyKey>,
    rpc: Arc<dyn Rpc>,
    worker_proxy: Arc<dyn WorkerProxy>,
    resources: HashMap<AgentResourceId, (ResourceTypeId, ResourceAny)>,
    last_resource_id: AgentResourceId,
    replay_state: ReplayState,
    persistence_level: PersistenceLevel,
    assume_idempotence: bool,

    /// State of ongoing http requests, key is the resource id it is most recently associated with (one state object can belong to multiple resources, but just one at once)
    open_http_requests: HashMap<u32, HttpRequestState>,

    /// WebSocket connection state indexed by websocket resource rep.
    open_websocket_connections: HashMap<u32, WebSocketConnectionState>,

    /// Maps outgoing request rep → outgoing body rep, set during outgoing_request::body()
    /// before outgoing_handler::handle() is called and the HttpRequestState is created.
    pending_http_outgoing_request_body: HashMap<u32, u32>,

    /// Tracks file-backed wasi output streams so quota charging can be based on
    /// actual file growth instead of requested write size.
    open_filesystem_output_streams: HashMap<u32, FilesystemOutputStreamState>,

    /// Maps outgoing body rep → output stream rep, set during outgoing_body::write()
    /// before outgoing_handler::handle() is called. Used by handle() to populate
    /// output_stream_rep in HttpRequestState for streams created before dispatch.
    pending_http_outgoing_body_stream: HashMap<u32, u32>,

    /// Retry eligibility flags accumulated before outgoing_handler::handle() creates
    /// the HttpRequestState. Keyed by outgoing request rep.
    pending_http_retry_eligibility: HashMap<u32, HttpRetryEligibility>,

    snapshotting_mode: Option<PersistenceLevel>,

    /// Tracks whether the currently executing invocation is restricted to read-only side effects.
    /// When `ReadOnly`, outgoing HTTP and RPC host calls are trapped before any oplog entry is
    /// written. Defaults to `Normal` and is reset on every invocation exit path.
    invocation_strictness: InvocationStrictness,

    /// Name of the agent method currently being invoked under read-only strictness. Captured at
    /// the invocation entry point so it can be reported in `AgentError::ReadOnlyViolation`.
    read_only_method_name: Option<String>,

    component_metadata: Component,

    total_linear_memory_size: u64,
    /// Running total of storage bytes acquired from the executor semaphore pool
    /// by this worker since it last started. Incremented on every successful
    /// write; decremented when files are deleted or truncated.
    current_filesystem_storage_usage: u64,

    invocation_context: InvocationContext,
    current_span_id: SpanId,
    forward_trace_context_headers: bool,
    set_outgoing_http_idempotency_key: bool,

    worker_fork: Arc<dyn WorkerForkService>,

    read_only_paths: RwLock<HashSet<PathBuf>>,
    files: TRwLock<HashMap<PathBuf, IFSWorkerFile>>,
    file_loader: Arc<FileLoader>,

    shard_service: Arc<dyn ShardService>,

    // The initial local agent config that the worker was configured with
    initial_agent_config: Vec<TypedAgentConfigEntry>,
    /// The current local agent config of the worker, taking the component revision into account
    agent_config: HashMap<Vec<String>, golem_wasm::ValueAndType>,

    /// Cached named retry policies derived from `agent_config` only. Lazily populated and
    /// invalidated whenever `agent_config` is reassigned.
    cached_agent_config_retry_policies: Option<Vec<NamedRetryPolicy>>,

    /// Runtime overlay of named retry policy mutations applied via oplog entries.
    /// `Some(policy)` = set/overwrite, `None` = tombstone (removed).
    /// Applied on top of base policies from agent_config during `named_retry_policies()`.
    runtime_retry_policy_mutations: std::collections::BTreeMap<String, Option<NamedRetryPolicy>>,

    /// Maps child pollable rep → parent FutureInvokeResult rep.
    /// Used to finalize deferred parent deletion when a child pollable is dropped.
    rpc_pollable_to_parent: HashMap<u32, u32>,

    // ResourceIds of all DynPollables that are backed by GetPromiseResultEntries
    promise_backed_pollables: TRwLock<HashMap<u32, GetPromiseResultEntry>>,
    // Map from resource_id to the dyn_pollables that wrap it
    promise_dyn_pollables: TRwLock<HashMap<u32, HashSet<u32>>>,

    /// Marks a retry point in the oplog to be attached to an Error entry in case a failure happens.
    /// As the error can happen both in the host or in the user code, we attach the last known value every time,
    /// which normally points to the last persisted side effect or the beginning of a region.
    current_retry_point: OplogIndex,

    /// Tracks the active atomic regions by their begin index. This is used together with `current_retry_point` to
    /// determine the effective retry point associated with an error; while `current_retry_point` is changed for each
    /// persisted host call, if there is an active atomic region, the error is associated with that. Otherwise retried
    /// failures within atomic regions would not be grouped by the same retry point as the whole atomic region gets retried
    /// from scratch.
    active_atomic_regions: Vec<ActiveAtomicRegion>,

    /// Begin indices of currently open rollback-capable regions other than atomic regions: remote
    /// writes / HTTP requests (`BeginRemoteWrite`..`EndRemoteWrite`) and remote transactions
    /// (`BeginRemoteTransaction`..`Committed`/`RolledBackRemoteTransaction`). Maintained by
    /// `begin_function`/`end_function` and the transaction lifecycle functions. While any such
    /// region is open, the current oplog tip sits inside it, so a later trap/replay can append a
    /// jump that deletes the tip — making a mid-invocation status checkpoint at the tip unsafe (see
    /// `at_clean_checkpoint_boundary`). Keyed by begin index so begin/end are self-balancing across
    /// the messy replay/restart paths; a fresh state is built per worker incarnation, so a region
    /// left open by a trap is cleared on restart.
    open_rollback_regions: HashSet<OplogIndex>,

    /// The minimum oplog index handed to the guest via `get_oplog_index` during the current
    /// invocation (the `NoOp` marker it plants). It is the only realistic `set_oplog_index` target,
    /// which deletes `(M.next()..source]` and preserves `M`, so a checkpoint at an index `<= M`
    /// survives such a jump. Mid-invocation checkpoints are not advanced past this watermark. Reset
    /// at the start of every invocation (a marker held across invocations only costs graceful
    /// fallback, never correctness).
    min_exposed_marker: Option<OplogIndex>,

    // Update that is pending and should be applied at the end of replay.
    // Other parts of the worker configuration already reflect the worker state implied by the update (component version, env vars, ifs, etc.)
    pending_update: tokio::sync::Mutex<Option<TimestampedUpdateDescription>>,

    /// Stores the phantom ID associated with the currently replayed oplog region. Forks can change it
    current_phantom_id: Option<Uuid>,
    last_snapshot_index: Option<OplogIndex>,

    /// Number of outgoing HTTP calls made in the current invocation (live only, not replayed).
    /// Reset to 0 at the start of each exported function invocation.
    http_call_count: u64,
    /// Per-invocation HTTP call limit from the account's Plan.
    per_invocation_http_call_limit: u64,

    /// Number of RPC calls made in the current invocation (live only, not replayed).
    /// Reset to 0 at the start of each exported function invocation.
    rpc_call_count: u64,
    /// Per-invocation RPC call limit from the account's Plan.
    per_invocation_rpc_call_limit: u64,

    /// Shared per-account resource limit entry. Used to record monthly HTTP/RPC call consumption
    /// and to check remaining budgets from the epoch callback.
    resource_limit_entry: Arc<AtomicResourceEntry>,
}

impl PrivateDurableWorkerState {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        agent_id: Option<ParsedAgentId>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        promise_service: Arc<dyn PromiseService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        quota_service: Arc<dyn QuotaService>,
        component_service: Arc<dyn ComponentService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        config: Arc<GolemConfig>,
        owned_agent_id: OwnedAgentId,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        deleted_regions: DeletedRegions,
        component_metadata: Component,
        total_linear_memory_size: u64,
        current_filesystem_storage_usage: u64,
        worker_fork: Arc<dyn WorkerForkService>,
        read_only_paths: RwLock<HashSet<PathBuf>>,
        files: TRwLock<HashMap<PathBuf, IFSWorkerFile>>,
        file_loader: Arc<FileLoader>,
        created_by: AccountId,
        created_by_email: AccountEmail,
        initial_agent_config: Vec<TypedAgentConfigEntry>,
        agent_config: HashMap<Vec<String>, golem_wasm::ValueAndType>,
        shard_service: Arc<dyn ShardService>,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<Uuid>,
        last_snapshot_index: Option<OplogIndex>,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        resource_limit_entry: Arc<AtomicResourceEntry>,
    ) -> Result<Self, WorkerExecutorError> {
        let deleted_regions = if let Some(snapshot_idx) = last_snapshot_index {
            let mut regions = deleted_regions;
            let snapshot_skip =
                DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                    OplogIndex::INITIAL.next()..=snapshot_idx,
                )])
                .build();
            regions.set_override(snapshot_skip);
            regions
        } else {
            deleted_regions
        };
        let replay_state =
            ReplayState::new(owned_agent_id.clone(), oplog.clone(), deleted_regions).await?;
        let invocation_context = InvocationContext::new(None);
        let current_span_id = invocation_context.root.span_id().clone();
        Ok(Self {
            oplog_service,
            oplog,
            agent_id,
            http_call_count: 0,
            per_invocation_http_call_limit,
            rpc_call_count: 0,
            per_invocation_rpc_call_limit,
            promise_service,
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            quota_service,
            component_service,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            agent_config,
            owned_agent_id,
            current_idempotency_key: None,
            rpc,
            worker_proxy,
            resources: HashMap::new(),
            last_resource_id: AgentResourceId::INITIAL,
            persistence_level: PersistenceLevel::Smart,
            assume_idempotence: true,
            open_http_requests: HashMap::new(),
            open_websocket_connections: HashMap::new(),
            pending_http_outgoing_request_body: HashMap::new(),
            pending_http_outgoing_body_stream: HashMap::new(),
            pending_http_retry_eligibility: HashMap::new(),
            open_filesystem_output_streams: HashMap::new(),
            snapshotting_mode: None,
            invocation_strictness: InvocationStrictness::Normal,
            read_only_method_name: None,
            component_metadata,
            total_linear_memory_size,
            current_filesystem_storage_usage,
            replay_state,
            invocation_context,
            current_span_id,
            forward_trace_context_headers: true,
            set_outgoing_http_idempotency_key: true,
            worker_fork,
            read_only_paths,
            files,
            file_loader,
            created_by,
            created_by_email,
            initial_agent_config,
            config,
            cached_agent_config_retry_policies: None,
            runtime_retry_policy_mutations: std::collections::BTreeMap::new(),
            rpc_pollable_to_parent: HashMap::new(),
            shard_service,
            promise_backed_pollables: TRwLock::new(HashMap::new()),
            promise_dyn_pollables: TRwLock::new(HashMap::new()),
            pending_update: tokio::sync::Mutex::new(pending_update),
            current_retry_point: OplogIndex::INITIAL,
            active_atomic_regions: Vec::new(),
            open_rollback_regions: HashSet::new(),
            min_exposed_marker: None,
            current_phantom_id: original_phantom_id,
            last_snapshot_index,
            resource_limit_entry,
        })
    }

    /// Returns the agent-config-derived retry policies (cached, cheap).
    pub fn agent_config_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        if let Some(ref cached) = self.cached_agent_config_retry_policies {
            cached.clone()
        } else {
            let policies = collect_named_retry_policies(&self.agent_config);
            self.cached_agent_config_retry_policies = Some(policies.clone());
            policies
        }
    }

    /// Returns the named retry policies derived from the default config-based catch-all,
    /// agent config, environment-level policies (fetched dynamically via EnvironmentStateService),
    /// and runtime overlay.
    pub async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        // Tier 0: default catch-all policy from GolemConfig (priority 0, Predicate::True)
        let default_policy = NamedRetryPolicy::default_from_config(&self.config.retry);

        // Tier 1: agent_config policies (cached; invalidated on component update)
        let agent_config_policies =
            if let Some(ref cached) = self.cached_agent_config_retry_policies {
                cached.clone()
            } else {
                let policies = collect_named_retry_policies(&self.agent_config);
                self.cached_agent_config_retry_policies = Some(policies.clone());
                policies
            };

        // Tier 2: environment-level policies (fetched dynamically)
        let environment_policies = self
            .environment_state_service
            .get_retry_policies(self.owned_agent_id.environment_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to fetch environment retry policies: {e}");
                vec![]
            });

        // Tier 3: runtime overlay (highest precedence)
        let mut deduped = std::collections::BTreeMap::new();
        deduped.insert(default_policy.name.clone(), default_policy);
        for policy in agent_config_policies {
            deduped.insert(policy.name.clone(), policy);
        }
        for policy in environment_policies {
            deduped.insert(policy.name.clone(), policy);
        }
        for (name, mutation) in &self.runtime_retry_policy_mutations {
            match mutation {
                Some(policy) => {
                    deduped.insert(name.clone(), policy.clone());
                }
                None => {
                    deduped.remove(name);
                }
            }
        }
        let mut policies: Vec<NamedRetryPolicy> = deduped.into_values().collect();
        policies.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        policies
    }

    /// Apply a set-retry-policy mutation (from oplog replay or live execution).
    pub fn apply_set_retry_policy(&mut self, policy: NamedRetryPolicy) {
        self.runtime_retry_policy_mutations
            .insert(policy.name.clone(), Some(policy));
    }

    /// Apply a remove-retry-policy mutation (from oplog replay or live execution).
    pub fn apply_remove_retry_policy(&mut self, name: &str) {
        self.runtime_retry_policy_mutations
            .insert(name.to_string(), None);
    }

    /// Returns whether the outermost active atomic region has side effects
    pub fn outermost_atomic_region_has_side_effects(&self) -> bool {
        self.active_atomic_regions
            .first()
            .is_some_and(|region| region.has_side_effects)
    }

    pub fn current_idempotency_key_oplog_index(&mut self, oplog_index: OplogIndex) -> OplogIndex {
        if let Some(outermost_atomic_region) = self.active_atomic_regions.first_mut() {
            next_atomic_region_idempotency_key_oplog_index(
                &mut outermost_atomic_region.next_idempotency_key_oplog_index,
            )
        } else {
            oplog_index
        }
    }

    pub fn current_atomic_region_idempotency_key_oplog_index(&self) -> Option<OplogIndex> {
        self.active_atomic_regions
            .first()
            .map(|region| region.next_idempotency_key_oplog_index)
    }

    /// Enriches retry properties with worker-local context: `agent-type` and `is-idempotent`.
    /// Should be called on all executor-constructed retry property bags before policy resolution.
    pub fn enrich_retry_properties(&self, props: &mut RetryProperties) {
        if let Some(agent_id) = &self.agent_id {
            props.set(
                "agent-type",
                PredicateValue::Text(agent_id.agent_type.to_string()),
            );
        }
        props.set(
            "is-idempotent",
            PredicateValue::Boolean(self.assume_idempotence),
        );
    }

    /// Mark the outermost active atomic region as having side effects
    pub fn mark_atomic_region_has_side_effects(&mut self) {
        if let Some(region) = self.active_atomic_regions.first_mut() {
            region.has_side_effects = true;
        }
    }

    /// Find the open_http_requests entry key for a given outgoing body rep.
    fn find_request_handle_by_outgoing_body(&self, body_rep: u32) -> Option<u32> {
        self.open_http_requests
            .iter()
            .find(|(_, state)| state.outgoing_body_rep == Some(body_rep))
            .map(|(&handle, _)| handle)
    }

    /// Find the open_http_requests entry key for a given output stream rep.
    fn find_request_handle_by_output_stream(&self, stream_rep: u32) -> Option<u32> {
        self.open_http_requests
            .iter()
            .find(|(_, state)| state.output_stream_rep == Some(stream_rep))
            .map(|(&handle, _)| handle)
    }

    /// Find the pending outgoing request rep for a given outgoing body rep.
    fn find_pending_request_rep_by_outgoing_body(&self, body_rep: u32) -> Option<u32> {
        self.pending_http_outgoing_request_body
            .iter()
            .find(|(_, pending_body_rep)| **pending_body_rep == body_rep)
            .map(|(&request_rep, _)| request_rep)
    }

    /// Find the pending outgoing body rep for a given output stream rep.
    fn find_pending_body_rep_by_output_stream(&self, stream_rep: u32) -> Option<u32> {
        self.pending_http_outgoing_body_stream
            .iter()
            .find(|(_, pending_stream_rep)| **pending_stream_rep == stream_rep)
            .map(|(&body_rep, _)| body_rep)
    }

    /// Find the pending outgoing request rep for a given output stream rep.
    fn find_pending_request_rep_by_output_stream(&self, stream_rep: u32) -> Option<u32> {
        let body_rep = self.find_pending_body_rep_by_output_stream(stream_rep)?;
        self.find_pending_request_rep_by_outgoing_body(body_rep)
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

    /// Increments the HTTP call counter for the current invocation if in live mode.
    ///
    /// Returns `Err` if the per-invocation HTTP call limit would be exceeded.
    /// The check and increment are performed only during live execution; replay
    /// mode is a no-op so that recovering workers are not penalised for calls
    /// already made in a prior execution.
    pub fn check_and_increment_http_call_count(&mut self) -> Result<(), GolemSpecificWasmTrap> {
        if !self.is_live() {
            return Ok(());
        }
        if self.per_invocation_http_call_limit != u64::MAX
            && self.http_call_count >= self.per_invocation_http_call_limit
        {
            return Err(GolemSpecificWasmTrap::WorkerExceededHttpCallLimit);
        }
        self.http_call_count = self.http_call_count.saturating_add(1);
        Ok(())
    }

    /// Increments the RPC call counter for the current invocation if in live mode.
    ///
    /// Returns `Err` if the per-invocation RPC call limit would be exceeded.
    pub fn check_and_increment_rpc_call_count(&mut self) -> Result<(), GolemSpecificWasmTrap> {
        if !self.is_live() {
            return Ok(());
        }
        if self.per_invocation_rpc_call_limit != u64::MAX
            && self.rpc_call_count >= self.per_invocation_rpc_call_limit
        {
            return Err(GolemSpecificWasmTrap::WorkerExceededRpcCallLimit);
        }
        self.rpc_call_count = self.rpc_call_count.saturating_add(1);
        Ok(())
    }

    pub fn reset_invocation_call_counts(&mut self) {
        self.http_call_count = 0;
        self.rpc_call_count = 0;
        // The `get_oplog_index` marker watermark is per-invocation: a marker captured in a previous
        // invocation only costs a graceful checkpoint fallback if jumped to, never correctness.
        self.min_exposed_marker = None;
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.replay_state.is_live()
    }

    /// Whether the current oplog tip is a structurally clean boundary at which a mid-invocation
    /// status checkpoint may be taken: we are live, no rollback-capable region is open (so no later
    /// trap/replay can append a jump that deletes the tip), and we are not in a persistence regime
    /// whose entries are not folded normally. The `get_oplog_index` marker watermark is checked
    /// separately by the caller against the committed status tip.
    pub fn at_clean_checkpoint_boundary(&self) -> bool {
        self.is_live()
            && self.active_atomic_regions.is_empty()
            && self.open_rollback_regions.is_empty()
            && self.persistence_level != PersistenceLevel::PersistNothing
            && self.snapshotting_mode.is_none()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    pub async fn sleep_until(&self, when: DateTime<Utc>) -> Result<(), WorkerExecutorError> {
        let promise_id = self
            .promise_service
            .create(
                &self.owned_agent_id.agent_id,
                self.current_oplog_index().await,
            )
            .await;

        let schedule_id = self
            .scheduler_service
            .schedule(
                when,
                ScheduledAction::CompletePromise {
                    account_id: self.created_by,
                    environment_id: self.owned_agent_id.environment_id(),
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
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<AgentMetadata>), WorkerExecutorError> {
        self.worker_enumeration_service
            .get(
                &self.owned_agent_id.environment_id,
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
            value: self.owned_agent_id.agent_id.to_agent_urn(),
        }
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        let id = self.last_resource_id;
        self.last_resource_id = self.last_resource_id.next();
        self.resources.insert(id, (name, resource));
        id.0
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        let resource_id = AgentResourceId(resource_id);
        self.resources.remove(&resource_id)
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        self.resources.get(&AgentResourceId(resource_id)).cloned()
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

impl HasComponentService for PrivateDurableWorkerState {
    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }
}

impl HasWorkerService for PrivateDurableWorkerState {
    fn worker_service(&self) -> Arc<dyn WorkerService> {
        self.worker_service.clone()
    }
}

pub struct PublicDurableWorkerState<Ctx: WorkerCtx> {
    promise_service: Arc<dyn PromiseService>,
    event_service: Arc<dyn WorkerEventService>,
    invocation_queue: Weak<Worker<Ctx>>,
    // IMPORTANT: commits to the oplog must go via self.public_state.worker().commit_oplog_and_update_state
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

    fn io_data(&mut self) -> IoData<'_> {
        let inner = &mut *self.0;
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        let io_ctx = Arc::get_mut(&mut inner.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail");
        IoData { table, io_ctx }
    }
}

// This wrapper forces the compiler to choose the wasmtime_wasi implementations for T: WasiView
impl<Ctx: WorkerCtx> WasiView for DurableWorkerCtxWasiView<'_, Ctx> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        let inner = &mut *self.0;
        let ctx = Arc::get_mut(&mut inner.wasi)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail");
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        let io_ctx = Arc::get_mut(&mut inner.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail");
        WasiCtxView { ctx, table, io_ctx }
    }
}

impl<Ctx: WorkerCtx> WasiHttpView for DurableWorkerCtx<Ctx> {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        self.as_wasi_http_view()
    }
}

/// File that was provisioned due to metadata. There might be additional files that the
/// worker created itself.
/// Ro files are symlinked to the proper location and might be garbage collected when the token is dropped.
/// Rw files are directly copied to the target location.
enum IFSWorkerFile {
    Ro {
        file: InitialAgentFile,
        _token: FileUseToken,
    },
    Rw,
}

async fn prepare_filesystem(
    file_loader: &Arc<FileLoader>,
    environment_id: EnvironmentId,
    root: &Path,
    files: &[InitialAgentFile],
) -> Result<HashMap<PathBuf, IFSWorkerFile>, WorkerExecutorError> {
    let futures = files.iter().map(|file| {
        let path = root.join(PathBuf::from(file.path.to_rel_string()));
        let file = file.clone();
        let permissions = file.permissions;
        let file_loader = file_loader.clone();
        async move {
            match permissions {
                AgentFilePermissions::ReadOnly => {
                    debug!("Loading read-only file {}", path.display());
                    let token = file_loader
                        .get_read_only_to(environment_id, file.content_hash, &path, file.size)
                        .await?;
                    Ok::<_, WorkerExecutorError>((
                        path,
                        IFSWorkerFile::Ro {
                            file,
                            _token: token,
                        },
                    ))
                }
                AgentFilePermissions::ReadWrite => {
                    debug!("Loading read-write file {}", path.display());
                    file_loader
                        .get_read_write_to(environment_id, file.content_hash, &path)
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
    environment_id: EnvironmentId,
    root: &Path,
    files: &[InitialAgentFile],
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

    // We do this in two phases to make errors less likely. First, delete all files that are no longer needed and then create
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
                (AgentFilePermissions::ReadOnly, None) => {
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
                        .get_read_only_to(environment_id, file.content_hash, &path, file.size)
                        .await?;

                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Ro { file, _token: token } })
                }
                (AgentFilePermissions::ReadOnly, Some(IFSWorkerFile::Ro { file: existing_file, .. })) => {
                    if existing_file.content_hash == file.content_hash {
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
                            .get_read_only_to(environment_id, file.content_hash, &path, file.size)
                            .await?;
                        Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Ro { file, _token: token } })
                    }
                }
                (AgentFilePermissions::ReadOnly, Some(IFSWorkerFile::Rw)) => {
                    Err(WorkerExecutorError::FileSystemError {
                        path: file.path.to_rel_string(),
                        reason: "Tried updating rw file to ro during update".to_string(),
                    })
                }
                (AgentFilePermissions::ReadWrite, None) => {
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
                        .get_read_write_to(environment_id, file.content_hash, &path)
                        .await?;
                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Rw })
                }
                (AgentFilePermissions::ReadWrite, Some(IFSWorkerFile::Ro { .. })) => {
                    debug!("Updating ro file to rw {}", path.display());
                    tokio::fs::remove_file(&path).await.map_err(|e|
                        WorkerExecutorError::FileSystemError {
                            path: file.path.to_rel_string(),
                            reason: format!("Failed deleting file during update: {e}"),
                        }
                    )?;
                    file_loader
                        .get_read_write_to(environment_id, file.content_hash, &path)
                        .await?;
                    Ok::<_, WorkerExecutorError>(UpdateFileSystemResult::Replace { path, value: IFSWorkerFile::Rw })
                }
                (AgentFilePermissions::ReadWrite, Some(IFSWorkerFile::Rw)) => {
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

/// Helper macro for expecting a given type of OplogEntry as the next entry in the oplog during
/// replay, while skipping hint entries.
/// The macro expression's type is `Result<(OplogIndex, OplogEntry), WorkerExecutorError>` and it fails if the next non-hint
/// entry was not the expected one.
#[macro_export]
macro_rules! get_oplog_entry {
    ($replay_state:expr, $($cases:path),+) => {
        loop {
            let (oplog_index, oplog_entry) = $replay_state.get_oplog_entry().await?;
            match oplog_entry {
                $($cases { .. } => {
                    break Ok((oplog_index, oplog_entry));
                })+
                _ => {
                    tracing::error!("Unexpected oplog entry - expected {}, got {:?}", stringify!($($cases |)+), oplog_entry);
                    break Err(golem_service_base::error::worker_executor::WorkerExecutorError::unexpected_oplog_entry(
                        stringify!($($cases |)+),
                        format!("{:?}", oplog_entry),
                    ));
                }
            }
        }
    };
}

#[async_trait]
pub trait RemoteTransactionHandler<Tx, Err>
where
    Err: From<WorkerExecutorError>,
{
    async fn create_new(&self) -> Result<(TransactionId, Tx), Err>;

    async fn create_replay(
        &self,
        transaction_id: &TransactionId,
    ) -> Result<(TransactionId, Tx), Err>;

    async fn is_committed(&self, transaction_id: &TransactionId) -> Result<bool, Err>;

    async fn is_rolled_back(&self, transaction_id: &TransactionId) -> Result<bool, Err>;
}
