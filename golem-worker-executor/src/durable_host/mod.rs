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

pub(crate) mod authorization;
pub mod blobstore;
mod call_coordinator;
mod cli;
mod clocks;
mod concurrent;
mod config;
pub mod durability;
pub(crate) mod durable_session;
pub(crate) mod durable_stream;
pub mod entity;
pub mod golem;
pub mod http;
pub mod io;
pub mod keyvalue;
mod logging;
pub mod p3;
mod permissions;
pub mod quota;
mod random;
pub mod rdbms;
pub(crate) mod replay_state;
pub(crate) mod schema_value_stream;
mod secrets;
pub use schema_value_stream::CoreTypesHost;
mod sockets;
pub(crate) mod stream_bus;
pub(crate) mod stream_session;
pub(crate) mod stream_transport;
mod suspendable_wait;
pub mod tail_work;
pub mod tool;
pub mod wasm_rpc;
pub mod websocket;

use self::golem::v1x::GetPromiseResultEntry;
use crate::durable_host::durability::collect_named_retry_policies;
use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::replay_state::{OplogEntryLookupResult, ReplayState};
use crate::metrics::ephemeral::record_non_suspending_failure;
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::model::event::InternalWorkerEvent;
use crate::model::{
    AgentConfig, ExecutionStatus, InvocationContext, LastError, ReadFileResult, SnapshotSource,
    TrapType,
};
use crate::services::active_agents::MemoryGrant;
use crate::services::agent_filesystem::{FilesystemGenerationHandle, update_initial_files};
use crate::services::agent_types::AgentTypesService;
use crate::services::agent_webhooks::AgentWebhooksService;
use crate::services::blob_store::BlobStoreService;
use crate::services::card::{CardService, CardState};
use crate::services::card_interest::CardInterestIndex;
use crate::services::component::ComponentService;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::linear_memory::{
    LinearMemoryTracker, SHARED_LINEAR_MEMORY_ERROR, UnsharedMemoryGrowth,
};
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
use crate::services::{
    HasActiveAgents, HasAll, HasConfig, HasOplog, HasWorker, worker_enumeration,
};
use crate::services::{HasComponentService, HasOplogService, HasWorkerService};
use crate::wasi_filesystem::AgentDescriptor;
use crate::wasi_host;
use crate::worker::agent_config::{effective_agent_config, validate_agent_config};
use crate::worker::instance::{OwnerExecution, OwnerRuntimeResources};
use crate::worker::invocation::{
    AgentExportFuncs, InvocationMode, InvokeResult, invoke_observed_and_traced, lower_invocation,
};
use crate::worker::owner_lane::{OwnerInvocationId, OwnerInvocationPermit};
use crate::worker::status::{
    calculate_last_known_status_with_checkpoint, calculate_pending_card_events,
};
use crate::worker::{RetryDecision, Worker};
use crate::workerctx::{
    ExternalOperations, FileSystemReading, InvocationContextManagement, InvocationHooks,
    InvocationManagement, PublicWorkerIo, StatusManagement, UpdateManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
pub(crate) use concurrent::{
    CallReplayOutcome, DurableCallSession, NotCancellable,
    authorize_live_permissions_at_serialized_access,
};
pub use durability::*;
use golem_common::base_model::oplog::{CardInstallFailure, QueuedCardEvent};
use golem_common::model::TransactionId;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, ParsedAgentId, Principal};
use golem_common::model::card::{
    AgentCardHolder, CardHolder, CardId, InvocationWalletPin, PermissionTarget, ScopeCard,
    StoredCard, WalletVersionToken,
};
use golem_common::model::component::{CanonicalFilePath, ComponentId, ComponentRevision};
use golem_common::model::entity::{EntityInvocationScope, FilesystemCapability, OwnerRuntime};
#[cfg(test)]
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    AgentError, AgentResourceId, DurableFunctionType, HostRequestHttpRequest, LogLevel, OplogEntry,
    OplogIndex, RawSnapshotData, ScopeScanState, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegionsBuilder, OplogRegion};
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_common::model::{
    AgentFilter, AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult,
    AgentMetadata, AgentStatus, AgentStatusRecord, IdempotencyKey, OwnedAgentId,
    PendingCardEventRef, RetryContext, RetryVerdict, ScanCursor, ScheduledAction, Timestamp,
};
use golem_common::model::{PredicateValue, RetryPolicyState, RetryProperties};
use golem_common::resource_runtime::Uri;
use golem_common::resource_runtime::{ResourceStore, ResourceTypeId};
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::component::Component;
use golem_service_base::model::{
    ComponentFileSystemNode, ComponentFileSystemNodeDetails, GetFileSystemNodeResult,
};
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use replay_state::ReplayEvent;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use std::vec;
use tokio::sync::RwLock as TRwLock;

use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use tracing::{Instrument, Level, debug, error, info, span, warn};
use try_match::try_match;
use uuid::Uuid;
use wasmtime::component::{Instance, Resource, ResourceAny};
use wasmtime::{AsContext, AsContextMut, MemoryKind, Store};
use wasmtime_wasi::{
    I32Exit, IoCtx, IoData, IoView, ResourceTable, WasiCtx, WasiCtxView, WasiView,
};
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{
    BodyCompletionReceiver, HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
    default_send_request_with_pool,
};
use wasmtime_wasi_http::p3::RequestOptions;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;
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
        if self.is_replay.load(Ordering::Acquire) {
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

impl wasmtime_wasi_http::p3::WasiHttpHooks for DurableHttpHooks {
    fn send_request(
        &mut self,
        request: ::http::Request<UnsyncBoxBody<Bytes, ErrorCode>>,
        options: Option<RequestOptions>,
        fut: Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (
                        ::http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
                        Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
                    ),
                    wasmtime_wasi::TrappableError<ErrorCode>,
                >,
            > + Send,
    > {
        _ = fut;
        let connection_pool = self.connection_pool.clone();
        Box::new(async move {
            match connection_pool {
                Some(pool) => {
                    let (response, io, _pooled_connection) =
                        pool.pooled_send_request_p3(request, options).await?;
                    Ok((response, io))
                }
                None => {
                    let (res, io) =
                        wasmtime_wasi_http::p3::default_send_request(request, options).await?;
                    let io: Box<dyn Future<Output = Result<(), ErrorCode>> + Send> = Box::new(io);
                    Ok((res.map(BodyExt::boxed_unsync), io))
                }
            }
        })
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

/// Proof that one stable live authority snapshot admitted a semantic host operation.
/// It deliberately carries no authority state and does not retain the boundary lock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LiveAuthorizationPermit {
    pub(crate) _private: (),
}

pub(crate) fn agent_effective_surface_from_component_metadata(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    agent_id: &ParsedAgentId,
) -> Result<golem_common::model::card::EffectiveSurface, WorkerExecutorError> {
    let context = agent_monomorphization_context(component, owned_agent_id, agent_id);
    let card = agent_initial_card_from_component_metadata(component, agent_id)?;
    Ok(golem_common::model::card::agent_effective_surface_from_wallet(&context, [&card]))
}

pub(crate) fn agent_monomorphization_context(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    agent_id: &ParsedAgentId,
) -> golem_common::model::card::AgentPermissionMonomorphizationContext {
    golem_common::model::card::AgentPermissionMonomorphizationContext {
        account: component.account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component.component_name.clone(),
        agent_name: owned_agent_id.agent_id.agent_id.clone(),
        agent_type: agent_id.agent_type.clone(),
    }
}

fn agent_initial_card_from_component_metadata(
    component: &Component,
    agent_id: &ParsedAgentId,
) -> Result<StoredCard, WorkerExecutorError> {
    let card = component
        .metadata
        .agent_type_initial_permission_card(&agent_id.agent_type)
        .cloned()
        .ok_or_else(|| missing_agent_initial_card_error(component, agent_id))?;
    Ok(StoredCard::Polymorphic(card))
}

fn missing_agent_initial_card_error(
    component: &Component,
    agent_id: &ParsedAgentId,
) -> WorkerExecutorError {
    WorkerExecutorError::invalid_request(format!(
        "Missing initial permission card for agent type {} in component {} revision {}",
        agent_id.agent_type, component.id, component.revision
    ))
}

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: Arc<Mutex<ResourceTable>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    wasi: Arc<Mutex<WasiCtx>>, // Required because of the dropped Sync constraints in https://github.com/bytecodealliance/wasmtime/pull/7802
    io_ctx: Arc<Mutex<IoCtx>>,
    stdin: ManagedStdIn,
    wasi_http: WasiHttpCtx,
    http_hooks: DurableHttpHooks,
    pub owned_agent_id: OwnedAgentId,
    runtime: OwnerRuntime,
    filesystem: FilesystemCapability,
    entity_invocation_scope: Option<EntityInvocationScope>,
    primary_invocation_start_index: Option<OplogIndex>,
    owner_execution: Arc<OwnerExecution>,
    _owner_resources: Arc<OwnerRuntimeResources>,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState,
    filesystem_generation_handle: FilesystemGenerationHandle,
    filesystem_preopen: AgentDescriptor,
    execution_status: Arc<RwLock<ExecutionStatus>>,
    stream_runtime_teardown: Arc<AtomicBool>,
    pub websocket_connection_pool: websocket::WebSocketConnectionPool,
    resource_limits: Arc<AtomicResourceEntry>,
    linear_memory: LinearMemoryTracker,
    /// Per-instance cache of resolved typed guest export handles, populated
    /// lazily on first use during invocation dispatch.
    agent_export_funcs: AgentExportFuncs,
    _store_alive_guard: StoreAliveGuard,
}

pub(crate) struct PrimaryInvocationBody {
    permit: Option<OwnerInvocationPermit>,
}

impl<Ctx: WorkerCtx> Drop for DurableWorkerCtx<Ctx> {
    fn drop(&mut self) {
        self.linear_memory.clear_limit_exceeded_callback();
    }
}

impl PrimaryInvocationBody {
    pub(crate) async fn complete(mut self) {
        if let Some(permit) = self.permit.take() {
            permit.complete_and_wait().await;
        }
    }
}

/// Golem's memory accounting covers guest linear memory only.
///
/// Wasmtime also grows its internal GC heaps through the same limiter
/// callbacks, tagged `MemoryKind::GcHeap`. That capacity belongs to the
/// collector rather than to memory the guest declared, so it is admitted
/// without taking a grant and correspondingly never releases one. Golem's
/// engine config leaves the GC proposal disabled (see
/// `golem_common::wasmtime_config`), so no store should allocate a GC heap in
/// the first place; the arm is here so that enabling it does not silently start
/// billing collector capacity as guest memory.
pub trait DurableResourceLimiter<Ctx: WorkerCtx> {
    fn durable_worker_ctx(&mut self) -> &mut DurableWorkerCtx<Ctx>;

    fn durable_memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
        kind: MemoryKind,
    ) -> impl Future<Output = wasmtime::Result<bool>> + Send {
        let ctx = self.durable_worker_ctx();
        async move {
            match kind {
                MemoryKind::LinearMemory => {
                    ctx.admit_unshared_memory_growth(current, desired, maximum)
                        .await
                }
                MemoryKind::GcHeap => Ok(true),
            }
        }
    }

    fn durable_memory_grown(&mut self, current: usize, desired: usize, kind: MemoryKind) -> bool {
        if kind != MemoryKind::LinearMemory {
            return false;
        }
        let delta = desired.saturating_sub(current) as u64;
        if delta > 0 {
            self.durable_worker_ctx().increase_memory(delta);
            true
        } else {
            false
        }
    }

    fn durable_memory_grow_failed(&mut self, kind: MemoryKind) -> wasmtime::Result<()> {
        if kind == MemoryKind::LinearMemory {
            self.durable_worker_ctx().unshared_memory_growth_failed();
        }
        Ok(())
    }
}

/// Increments the live-`Store` gauge on construction and decrements it on drop.
/// Held as a field of [`DurableWorkerCtx`], which is the data of the wasmtime
/// `Store`, so the gauge follows the `Store`'s true lifetime regardless of which
/// reference keeps it alive. A persistent gap above the resident-worker count
/// indicates `Store`s retained after their worker was deleted.
struct StoreAliveGuard {
    entity_kind: Option<&'static str>,
}

impl StoreAliveGuard {
    fn new(runtime: &OwnerRuntime) -> Self {
        crate::metrics::workers::inc_worker_store_alive();
        let entity_kind = match runtime {
            OwnerRuntime::Agent => {
                crate::metrics::workers::inc_primary_store_alive();
                None
            }
            OwnerRuntime::Entity(entity) => {
                let entity_kind = entity.kind_label();
                crate::metrics::workers::inc_entity_store_alive(entity_kind);
                Some(entity_kind)
            }
        };
        StoreAliveGuard { entity_kind }
    }
}

impl Drop for StoreAliveGuard {
    fn drop(&mut self) {
        crate::metrics::workers::dec_worker_store_alive();
        match self.entity_kind {
            Some(entity_kind) => crate::metrics::workers::dec_entity_store_alive(entity_kind),
            None => crate::metrics::workers::dec_primary_store_alive(),
        }
    }
}

const DERIVED_CARD_ID_CONTEXT: &str = "golem:permissions:derived-card-id:v1";
const TRANSFER_ID_CONTEXT: &str = "golem:permissions:transfer-id:v1";
const INSTALLED_CHILD_CARD_ID_CONTEXT: &str = "golem:permissions:installed-child-card-id:v1";
const SCOPE_CARD_ID_CONTEXT: &str = "golem:permissions:scope-card-id:v1";
const UUID_V7_MAX_TIMESTAMP: u64 = (1_u64 << 48) - 1;

fn derive_permission_uuid(
    context: &'static str,
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    oplog_index: OplogIndex,
) -> Uuid {
    derive_permission_uuid_for_sequence(
        context,
        owned_agent_id,
        invocation_key,
        oplog_index.as_u64(),
    )
}

fn derive_permission_uuid_for_sequence(
    context: &'static str,
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    sequence: u64,
) -> Uuid {
    let agent_name = owned_agent_id.agent_id.agent_id.as_bytes();
    let invocation_key = invocation_key.value.as_bytes();
    let mut hasher = blake3::Hasher::new_derive_key(context);
    hasher.update(owned_agent_id.environment_id.0.as_bytes());
    hasher.update(owned_agent_id.agent_id.component_id.0.as_bytes());
    hasher.update(&(agent_name.len() as u64).to_be_bytes());
    hasher.update(agent_name);
    hasher.update(&(invocation_key.len() as u64).to_be_bytes());
    hasher.update(invocation_key);
    hasher.update(&sequence.to_be_bytes());

    let mut bytes = [0_u8; 16];
    let timestamp = sequence.min(UUID_V7_MAX_TIMESTAMP);
    bytes[..6].copy_from_slice(&timestamp.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&hasher.finalize().as_bytes()[..10]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Guard for the per-invocation wall-clock deadline; see
/// [`DurableWorkerCtx::arm_invocation_deadline`]. Holds the shared latch and the timer task;
/// dropping it aborts the timer and clears the latch so the deadline never outlives its
/// invocation.
pub struct InvocationDeadline {
    latch: Arc<AtomicBool>,
    duration: Option<Duration>,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl InvocationDeadline {
    /// Whether the deadline fired during this invocation.
    pub fn exceeded(&self) -> bool {
        self.latch.load(Ordering::Acquire)
    }

    /// The configured maximum invocation duration, if any.
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

impl Drop for InvocationDeadline {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        self.latch.store(false, Ordering::Release);
    }
}

/// Guard for the post-completion tail-work deadline. When the deadline fires it cooperatively
/// interrupts store tasks; dropping the guard aborts the timer and clears the latch before the
/// next guest call.
pub(crate) struct TailWorkDeadline {
    latch: Arc<AtomicBool>,
    duration: Duration,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl TailWorkDeadline {
    pub(crate) fn exceeded(&self) -> bool {
        self.latch.load(Ordering::Acquire)
    }

    pub(crate) fn duration(&self) -> Duration {
        self.duration
    }
}

impl Drop for TailWorkDeadline {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        self.latch.store(false, Ordering::Release);
    }
}

fn validate_unshared_memory_growth(
    growth: Option<UnsharedMemoryGrowth>,
    worker_limit: u64,
    desired: usize,
    memory_maximum: Option<usize>,
) -> Option<UnsharedMemoryGrowth> {
    growth.filter(|growth| {
        growth.protected_total <= worker_limit
            && memory_maximum.is_none_or(|maximum| desired <= maximum)
    })
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn is_live(&self) -> bool {
        self.state.is_live()
    }

    pub(crate) fn filesystem_generation_handle(&self) -> FilesystemGenerationHandle {
        self.filesystem_generation_handle.clone()
    }

    pub(crate) fn activate_resident_generation_handle(
        &mut self,
        generation_handle: FilesystemGenerationHandle,
    ) {
        self.filesystem_generation_handle = generation_handle;
    }

    pub(crate) fn filesystem_preopen(&self) -> AgentDescriptor {
        self.filesystem_preopen.clone()
    }

    pub(crate) fn derive_card_id(
        &self,
        invocation_key: &IdempotencyKey,
        oplog_index: OplogIndex,
    ) -> CardId {
        CardId(derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &self.owned_agent_id,
            invocation_key,
            oplog_index,
        ))
    }

    pub(crate) fn derive_transfer_id(
        &self,
        invocation_key: &IdempotencyKey,
        oplog_index: OplogIndex,
    ) -> Uuid {
        derive_permission_uuid(
            TRANSFER_ID_CONTEXT,
            &self.owned_agent_id,
            invocation_key,
            oplog_index,
        )
    }

    pub(crate) fn derive_installed_child_card_id(
        &self,
        invocation_key: &IdempotencyKey,
        oplog_index: OplogIndex,
    ) -> CardId {
        CardId(derive_permission_uuid(
            INSTALLED_CHILD_CARD_ID_CONTEXT,
            &self.owned_agent_id,
            invocation_key,
            oplog_index,
        ))
    }

    pub(crate) fn derive_scope_card_id(
        &self,
        invocation_key: &IdempotencyKey,
        ordinal: u64,
    ) -> CardId {
        CardId(derive_permission_uuid_for_sequence(
            SCOPE_CARD_ID_CONTEXT,
            &self.owned_agent_id,
            invocation_key,
            ordinal,
        ))
    }

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
        card_service: Arc<dyn CardService>,
        card_interest_index: Arc<CardInterestIndex>,
        component_service: Arc<dyn ComponentService>,
        resource_limits: Arc<AtomicResourceEntry>,
        config: Arc<GolemConfig>,
        filesystem: crate::workerctx::WorkerFilesystemContext,
        linear_memory: LinearMemoryTracker,
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
        runtime: OwnerRuntime,
        owner_execution: Arc<OwnerExecution>,
        owner_resources: Arc<OwnerRuntimeResources>,
        filesystem_capability: FilesystemCapability,
        executable_component: Component,
        entity_activation: Option<Arc<golem_common::model::entity::EntityActivation>>,
    ) -> Result<Self, WorkerExecutorError> {
        let crate::workerctx::WorkerFilesystemContext {
            generation_handle: filesystem_generation_handle,
            preopen: filesystem_preopen,
        } = filesystem;
        if runtime == OwnerRuntime::Agent && filesystem_capability != FilesystemCapability::Capable
        {
            return Err(WorkerExecutorError::runtime(
                "The primary Store must be filesystem-capable",
            ));
        }

        debug!(
            "Worker {} initialized with deleted regions {}",
            owned_agent_id.agent_id, worker_config.deleted_regions
        );

        debug!(
            "Worker {} starting replay from component revision {}",
            owned_agent_id.agent_id, worker_config.component_revision_for_replay
        );
        if executable_component.revision != worker_config.component_revision_for_replay {
            return Err(WorkerExecutorError::runtime(format!(
                "Executable component revision {} does not match context revision {}",
                executable_component.revision, worker_config.component_revision_for_replay
            )));
        }
        if runtime == OwnerRuntime::Agent
            && executable_component.id != owned_agent_id.component_id()
        {
            return Err(WorkerExecutorError::runtime(
                "Primary Store executable must be the owner component",
            ));
        }
        match (&runtime, &entity_activation) {
            (OwnerRuntime::Agent, None) => {}
            (OwnerRuntime::Entity(entity), Some(activation)) => {
                if entity != &activation.entity() {
                    return Err(WorkerExecutorError::runtime(
                        "Entity Store activation does not match its runtime selector",
                    ));
                }
            }
            (OwnerRuntime::Agent, Some(_)) => {
                return Err(WorkerExecutorError::runtime(
                    "Primary Store cannot carry an entity activation",
                ));
            }
            (OwnerRuntime::Entity(_), _) => {
                return Err(WorkerExecutorError::runtime(
                    "Entity Store requires an activation matching its runtime selector",
                ));
            }
        }
        match (&runtime, &worker_config.owner_component_metadata) {
            (OwnerRuntime::Agent, None) | (OwnerRuntime::Entity(_), Some(_)) => {}
            (OwnerRuntime::Agent, Some(_)) => {
                return Err(WorkerExecutorError::runtime(
                    "Primary Store cannot carry separate owner component metadata",
                ));
            }
            (OwnerRuntime::Entity(_), None) => {
                return Err(WorkerExecutorError::runtime(
                    "Entity Store requires owner component metadata pinned at dispatch",
                ));
            }
        }
        let component_metadata = executable_component;

        if component_metadata.metadata.has_shared_linear_memory() {
            return Err(WorkerExecutorError::worker_creation_failed(
                owned_agent_id.agent_id.clone(),
                SHARED_LINEAR_MEMORY_ERROR,
            ));
        }

        let initial_linear_memory = component_metadata.metadata.initial_linear_memory_bytes();
        if initial_linear_memory > resource_limits.max_memory_limit() as u64 {
            return Err(WorkerExecutorError::worker_creation_failed(
                owned_agent_id.agent_id.clone(),
                format!(
                    "Linear memories require {initial_linear_memory} bytes, exceeding the per-agent limit of {} bytes",
                    resource_limits.max_memory_limit()
                ),
            ));
        }

        let agent_type_provision_configs = match &runtime {
            OwnerRuntime::Agent => agent_id.as_ref().and_then(|agent_id| {
                component_metadata
                    .metadata
                    .agent_type_provision_configs()
                    .get(&agent_id.agent_type)
                    .cloned()
            }),
            OwnerRuntime::Entity(_) => None,
        };
        let agent_config = if agent_id.is_some() {
            effective_agent_config(
                worker_config.initial_agent_config.clone(),
                agent_type_provision_configs
                    .as_ref()
                    .map(|c| c.config.clone())
                    .unwrap_or_default(),
            )?
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
            stdin.clone(),
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
        let deleted_regions = if let Some(snapshot_idx) = worker_config.last_snapshot_index {
            let mut regions = worker_config.deleted_regions.clone();
            let snapshot_skip =
                DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                    OplogIndex::INITIAL.next()..=snapshot_idx,
                )])
                .build();
            regions.set_override(snapshot_skip);
            regions
        } else {
            worker_config.deleted_regions.clone()
        };
        let replay_state = match &runtime {
            OwnerRuntime::Agent => {
                owner_execution
                    .begin_replay_generation(deleted_regions, worker_config.last_snapshot_index)
                    .await?
            }
            OwnerRuntime::Entity(_) => owner_execution.replay().await?,
        };
        let worker = invocation_queue
            .upgrade()
            .expect("worker must remain alive while creating its context");
        let card_event_boundary_lock = worker.card_event_boundary_lock();
        let published_authority_generation = worker.published_authority_generation();
        let state = PrivateDurableWorkerState::new(
            agent_id,
            oplog_service,
            oplog.clone(),
            promise_service.clone(),
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            quota_service,
            card_service,
            card_interest_index,
            component_service,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            config.clone(),
            owned_agent_id.clone(),
            rpc,
            worker_proxy,
            replay_state,
            runtime.clone(),
            component_metadata,
            worker_config.owner_component_metadata,
            worker_config.agent_effective_surface,
            worker_fork,
            file_loader,
            worker_config.created_by,
            worker_config.created_by_email,
            worker_config.initial_agent_config,
            agent_config,
            shard_service,
            pending_update,
            original_phantom_id,
            worker_config.last_snapshot_index,
            worker_config.last_snapshot_source,
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
            resource_limits.clone(),
            card_event_boundary_lock,
            published_authority_generation,
        )
        .await?;
        if state.is_live() {
            linear_memory.switch_to_live();
        }
        let weak_worker = Arc::downgrade(&worker);
        let memory_limits = linear_memory.clone();
        linear_memory.set_limit_exceeded_callback(Arc::new(move || {
            if let Some(worker) = weak_worker.upgrade() {
                worker.request_memory_limit_interrupt(memory_limits.clone());
            }
        }));
        let store_alive_guard = StoreAliveGuard::new(&runtime);

        Ok(DurableWorkerCtx {
            table: Arc::new(Mutex::new(table)),
            wasi: Arc::new(Mutex::new(wasi)),
            io_ctx: Arc::new(Mutex::new(io_ctx)),
            stdin,
            wasi_http,
            http_hooks,
            owned_agent_id: owned_agent_id.clone(),
            runtime,
            filesystem: filesystem_capability,
            entity_invocation_scope: None,
            primary_invocation_start_index: None,
            owner_execution,
            _owner_resources: owner_resources,
            websocket_connection_pool,
            public_state: PublicDurableWorkerState {
                promise_service: promise_service.clone(),
                event_service,
                invocation_queue,
                oplog: oplog.clone(),
            },
            state,
            filesystem_generation_handle,
            filesystem_preopen: AgentDescriptor::new(filesystem_preopen, PathBuf::new()),
            execution_status,
            stream_runtime_teardown: Arc::new(AtomicBool::new(false)),
            resource_limits,
            linear_memory,
            agent_export_funcs: AgentExportFuncs::default(),
            _store_alive_guard: store_alive_guard,
        })
    }

    pub(crate) fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    pub(crate) fn register_filesystem_input_stream(&mut self, rep: u32) {
        self.state.open_filesystem_input_streams.insert(rep);
    }

    pub(crate) fn register_filesystem_output_stream(&mut self, rep: u32) {
        self.state
            .open_filesystem_output_streams
            .insert(rep, FilesystemOutputStreamState);
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
    /// is exhausted. This trap maps to `RetryDecision::TryStop`; the worker is
    /// suspended and resumed when the registry replenishes the budget.
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

    /// Returns whether the given direction of a TCP socket's one-shot stream has
    /// already been acquired (taken). Used by the durable P3 socket wrappers to
    /// gate `send`/`receive` so a second call returns `InvalidState`.
    pub(crate) fn is_tcp_stream_taken(
        &self,
        socket_rep: u32,
        direction: TcpSocketStreamDirection,
    ) -> bool {
        self.state
            .tcp_taken_streams
            .get(&socket_rep)
            .map(|taken| match direction {
                TcpSocketStreamDirection::Send => taken.send,
                TcpSocketStreamDirection::Receive => taken.receive,
            })
            .unwrap_or(false)
    }

    /// Marks the given direction of a TCP socket's one-shot stream as acquired.
    pub(crate) fn mark_tcp_stream_taken(
        &mut self,
        socket_rep: u32,
        direction: TcpSocketStreamDirection,
    ) {
        let taken = self.state.tcp_taken_streams.entry(socket_rep).or_default();
        match direction {
            TcpSocketStreamDirection::Send => taken.send = true,
            TcpSocketStreamDirection::Receive => taken.receive = true,
        }
    }

    /// Drops the shadow taken-state for a TCP socket resource. Called from the
    /// socket `drop` so a later resource-table rep reuse cannot inherit stale
    /// taken flags.
    pub(crate) fn forget_tcp_taken_streams(&mut self, socket_rep: u32) {
        self.state.tcp_taken_streams.remove(&socket_rep);
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

    pub fn runtime(&self) -> &OwnerRuntime {
        &self.runtime
    }

    pub fn filesystem_capability(&self) -> FilesystemCapability {
        self.filesystem
    }

    pub(crate) async fn acquire_owner_filesystem_inspection(
        &self,
    ) -> Result<crate::worker::owner_lane::OwnerLaneExclusiveGuard, WorkerExecutorError> {
        Ok(self.owner_execution.lane().acquire_exclusive().await)
    }

    pub fn set_entity_invocation_scope(
        &mut self,
        scope: Option<EntityInvocationScope>,
    ) -> Result<(), WorkerExecutorError> {
        match (&self.runtime, &self.entity_invocation_scope, &scope) {
            (OwnerRuntime::Agent, _, Some(_)) => Err(WorkerExecutorError::runtime(
                "Cannot install an entity invocation scope in the primary Store",
            )),
            (OwnerRuntime::Entity(_), Some(_), Some(_)) => Err(WorkerExecutorError::runtime(
                "Entity invocation scope is already installed",
            )),
            (OwnerRuntime::Entity(_), None, None) => Err(WorkerExecutorError::runtime(
                "Entity invocation scope is not installed",
            )),
            _ => {
                self.entity_invocation_scope = scope;
                Ok(())
            }
        }
    }

    pub fn entity_invocation_scope(&self) -> Option<&EntityInvocationScope> {
        self.entity_invocation_scope.as_ref()
    }

    pub(crate) fn child_parent_start_index(
        &self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Option<OplogIndex> {
        self.state
            .child_parent_start_index(function_type, begin_index)
            .or_else(|| {
                self.entity_invocation_scope
                    .as_ref()
                    .map(|scope| scope.invocation_id().start_index())
            })
    }

    fn entity_parent_start_index(&self) -> Option<OplogIndex> {
        self.entity_invocation_scope
            .as_ref()
            .map(|scope| scope.invocation_id().start_index())
    }

    pub(crate) fn owner_invocation_id(&self) -> Result<OwnerInvocationId, WorkerExecutorError> {
        match &self.runtime {
            OwnerRuntime::Entity(_) => self
                .entity_invocation_scope
                .as_ref()
                .map(|scope| OwnerInvocationId::Entity(scope.invocation_id().clone()))
                .ok_or_else(|| {
                    WorkerExecutorError::runtime(
                        "Entity Store has no active entity invocation scope",
                    )
                }),
            OwnerRuntime::Agent => self
                .primary_invocation_start_index
                .filter(|index| *index != OplogIndex::NONE)
                .map(OwnerInvocationId::Agent)
                .ok_or_else(|| {
                    WorkerExecutorError::runtime("Primary Store has no active invocation Start")
                }),
        }
    }

    pub(crate) async fn enter_primary_invocation_body(
        &self,
    ) -> Result<Option<PrimaryInvocationBody>, WorkerExecutorError> {
        if self.runtime != OwnerRuntime::Agent {
            return Ok(None);
        }
        let start_index = if self.state.snapshotting_mode {
            self.state.oplog.current_oplog_index().await
        } else {
            match self.owner_invocation_id()? {
                OwnerInvocationId::Agent(index) => index,
                OwnerInvocationId::Entity(_) => unreachable!(),
            }
        };
        let ticket = self
            .owner_execution
            .lane()
            .enter_primary(start_index)
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let permit = ticket
            .acquire()
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(Some(PrimaryInvocationBody {
            permit: Some(permit),
        }))
    }

    pub fn created_by(&self) -> AccountId {
        self.state.created_by
    }

    pub fn created_by_email(&self) -> &AccountEmail {
        &self.state.created_by_email
    }

    pub fn agent_effective_surface(&self) -> golem_common::model::card::EffectiveSurface {
        self.state.agent_effective_surface.clone()
    }

    pub fn agent_auth_ctx(&self) -> AuthCtx {
        let delegation_surface = if let Some(agent_id) = self.state.agent_id.as_ref() {
            let context = agent_monomorphization_context(
                &self.state.component_metadata,
                &self.owned_agent_id,
                agent_id,
            );
            golem_common::model::card::agent_delegation_surface_from_wallet(
                &context,
                self.state.agent_wallet_cards.values(),
            )
        } else {
            golem_common::model::card::DelegationSurface::default()
        };

        AuthCtx::agent_with_permission_surfaces(
            self.created_by(),
            self.created_by_email().clone(),
            self.agent_effective_surface(),
            delegation_surface,
        )
    }

    pub(crate) fn agent_wallet_cards_snapshot(&self) -> Vec<StoredCard> {
        self.state.agent_wallet_cards.values().cloned().collect()
    }

    pub(crate) fn wallet_id_hash(&self) -> [u8; 32] {
        self.state.wallet_id_hash
    }

    pub(crate) fn wallet_generation(&self) -> u64 {
        self.state.wallet_generation
    }

    pub(crate) async fn active_agent_wallet_cards_snapshot(
        &mut self,
    ) -> Result<Vec<StoredCard>, WorkerExecutorError> {
        let _boundary_guard = self
            .lock_synchronized_card_event_boundary_with_authority()
            .await?;
        if self.state.is_replay() {
            return Ok(self.agent_wallet_cards_snapshot());
        }

        self.public_state.worker().reattach_worker_status().await;
        self.check_post_replay_wallet_liveness().await?;
        self.drain_card_events_at_boundary().await?;
        let pending_revoked_cards = self
            .pending_card_events_at_boundary()
            .await?
            .into_iter()
            .filter_map(|pending_event| match pending_event.event {
                QueuedCardEvent::Revoke(event) => Some(event.card_id),
                QueuedCardEvent::Install(_)
                | QueuedCardEvent::TransferStarted(_)
                | QueuedCardEvent::TransferReceived(_) => None,
            })
            .collect::<HashSet<_>>();
        let wallet = self
            .agent_wallet_cards_snapshot()
            .into_iter()
            .filter(|card| !pending_revoked_cards.contains(&card.card_id()))
            .collect::<Vec<_>>();
        if wallet.is_empty() {
            return Ok(wallet);
        }

        let card_states = self
            .state
            .card_service
            .check_cards(wallet.iter().map(StoredCard::card_id).collect())
            .await?;

        Ok(wallet
            .into_iter()
            .filter(|card| matches!(card_states.get(&card.card_id()), Some(CardState::Live(_))))
            .collect())
    }

    /// Synchronizes the invocation-pinned wallet at the authorization linearization point.
    /// Replay applies only recorded events; live execution also drains queued registry events.
    pub(crate) async fn synchronize_agent_wallet_at_boundary(
        &mut self,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.is_live() {
            self.capture_live_agent_authority_at_boundary(&mut |_| ())
                .await?;
        } else {
            let _boundary_guard = self.lock_synchronized_card_event_boundary().await?;
        }
        Ok(())
    }

    pub(crate) async fn capture_agent_auth_ctx_at_boundary(
        &mut self,
    ) -> Result<Option<AuthCtx>, WorkerExecutorError> {
        self.capture_live_agent_authority_at_boundary(&mut |ctx| ctx.agent_auth_ctx())
            .await
    }

    pub(crate) async fn capture_live_agent_authority_at_boundary<T>(
        &mut self,
        capture: &mut impl FnMut(&mut Self) -> T,
    ) -> Result<Option<T>, WorkerExecutorError> {
        if !self.state.is_live() {
            return Ok(None);
        }

        let published_generation = self
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        let now = Utc::now();
        if authority_snapshot_is_current_at(
            self.state.authority_initialized,
            self.state.card_interest_index.authority_is_open(),
            self.state.processed_authority_generation,
            published_generation,
            self.state.next_authority_expiration,
            now,
        ) {
            let captured = capture(self);
            if self.authority_snapshot_is_stable(published_generation) {
                crate::metrics::wasm::record_agent_permission_authority_fast_path();
                return Ok(Some(captured));
            }
        }

        let started = Instant::now();
        loop {
            let boundary_guard = self
                .lock_synchronized_card_event_boundary_with_authority()
                .await?;
            let generation = self
                .state
                .published_authority_generation
                .load(Ordering::Acquire);
            let captured = self.state.is_live().then(|| capture(self));
            self.refresh_authority_expiration_deadline();
            if self.authority_snapshot_is_stable(generation) {
                self.adopt_authority_generation(generation);
                crate::metrics::wasm::record_agent_permission_authority_slow_path(
                    started.elapsed(),
                );
                return Ok(captured);
            }
            drop(boundary_guard);
        }
    }

    fn authority_snapshot_is_stable(&self, generation: u64) -> bool {
        authority_snapshot_is_current_at(
            true,
            self.state.card_interest_index.authority_is_open(),
            generation,
            self.state
                .published_authority_generation
                .load(Ordering::Acquire),
            self.state.next_authority_expiration,
            Utc::now(),
        )
    }

    fn refresh_authority_expiration_deadline(&mut self) {
        self.state.next_authority_expiration = self
            .state
            .agent_wallet_cards
            .values()
            .chain(self.state.invocation_scope_root_cards.values())
            .filter_map(StoredCard::expires_at)
            .min();
    }

    fn adopt_authority_generation(&mut self, generation: u64) {
        self.refresh_authority_expiration_deadline();
        self.state.processed_authority_generation = generation;
        self.state.authority_initialized = true;
    }

    pub(crate) async fn authorize_live_permission(
        &mut self,
        target: &PermissionTarget,
    ) -> Result<Result<LiveAuthorizationPermit, AuthorizationError>, WorkerExecutorError> {
        self.authorize_live_permissions(std::slice::from_ref(target))
            .await
    }

    pub(crate) async fn authorize_live_permissions(
        &mut self,
        targets: &[PermissionTarget],
    ) -> Result<Result<LiveAuthorizationPermit, AuthorizationError>, WorkerExecutorError> {
        assert_live_authorization(self.state.is_live());

        if self.operator_authorizes_current_invocation() {
            record_permission_decisions(targets, true);
            return Ok(Ok(LiveAuthorizationPermit { _private: () }));
        }

        let published_generation = self
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        let now = Utc::now();
        if authority_snapshot_is_current_at(
            self.state.authority_initialized,
            self.state.card_interest_index.authority_is_open(),
            self.state.processed_authority_generation,
            published_generation,
            self.state.next_authority_expiration,
            now,
        ) {
            let result = authorize_effective_surface(&self.state.agent_effective_surface, targets);
            if self.authority_snapshot_is_stable(published_generation) {
                crate::metrics::wasm::record_agent_permission_authority_fast_path();
                record_permission_decisions(targets, result.is_ok());
                return Ok(result.map(|()| LiveAuthorizationPermit { _private: () }));
            }
        }

        let started = Instant::now();
        loop {
            let boundary_guard = self
                .lock_synchronized_card_event_boundary_with_authority()
                .await?;
            let generation = self
                .state
                .published_authority_generation
                .load(Ordering::Acquire);
            let result = authorize_effective_surface(&self.state.agent_effective_surface, targets);
            // The previous cached deadline may be due even though synchronization just removed
            // every expired card. Refresh it before validating the synchronized snapshot.
            self.refresh_authority_expiration_deadline();
            if self.authority_snapshot_is_stable(generation) {
                self.adopt_authority_generation(generation);
                crate::metrics::wasm::record_agent_permission_authority_slow_path(
                    started.elapsed(),
                );
                record_permission_decisions(targets, result.is_ok());
                return Ok(result.map(|()| LiveAuthorizationPermit { _private: () }));
            }
            drop(boundary_guard);
        }
    }

    pub(crate) async fn filter_live_permissions(
        &mut self,
        targets: &[PermissionTarget],
    ) -> Result<Vec<bool>, WorkerExecutorError> {
        assert!(self.state.is_live());
        if self.operator_authorizes_current_invocation() {
            record_permission_decisions(targets, true);
            return Ok(vec![true; targets.len()]);
        }
        self.capture_live_agent_authority_at_boundary(&mut |ctx| {
            targets
                .iter()
                .map(|target| {
                    ctx.state
                        .agent_effective_surface
                        .authorize(target)
                        .unwrap_or(false)
                })
                .collect()
        })
        .await?
        .ok_or_else(|| WorkerExecutorError::runtime("authorization left live execution"))
    }

    pub(crate) async fn with_agent_authority_at_boundary<T>(
        &mut self,
        capture: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, WorkerExecutorError> {
        let _boundary_guard = self
            .lock_synchronized_card_event_boundary_with_authority()
            .await?;
        let result = capture(self);
        self.state.authority_initialized = false;
        Ok(result)
    }

    pub(crate) async fn try_agent_auth_ctx_at_boundary(
        &mut self,
    ) -> Result<Option<AuthCtx>, WorkerExecutorError> {
        let Some(_boundary_guard) = self
            .try_lock_synchronized_card_event_boundary_with_authority()
            .await?
        else {
            return Ok(None);
        };
        let generation = self
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        self.refresh_authority_expiration_deadline();
        if !self.authority_snapshot_is_stable(generation) {
            return Ok(None);
        }
        self.adopt_authority_generation(generation);
        Ok(Some(self.agent_auth_ctx()))
    }

    async fn lock_synchronized_card_event_boundary(
        &mut self,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, WorkerExecutorError> {
        self.lock_synchronized_card_event_boundary_inner(false, true)
            .await?
            .ok_or_else(|| WorkerExecutorError::runtime("unrestricted card boundary was closed"))
    }

    async fn lock_synchronized_card_event_boundary_with_authority(
        &mut self,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, WorkerExecutorError> {
        self.lock_synchronized_card_event_boundary_inner(true, true)
            .await?
            .ok_or_else(|| WorkerExecutorError::runtime("card authority boundary did not reopen"))
    }

    async fn try_lock_synchronized_card_event_boundary_with_authority(
        &mut self,
    ) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, WorkerExecutorError> {
        self.lock_synchronized_card_event_boundary_inner(true, false)
            .await
    }

    async fn lock_synchronized_card_event_boundary_inner(
        &mut self,
        requires_agent_authority: bool,
        wait_for_authority: bool,
    ) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, WorkerExecutorError> {
        loop {
            let boundary_guard = self
                .state
                .card_event_boundary_lock
                .clone()
                .lock_owned()
                .await;
            let authority_checked = requires_agent_authority && self.state.is_live();
            if authority_checked && !self.state.card_interest_index.authority_is_open() {
                drop(boundary_guard);
                if !wait_for_authority {
                    return Ok(None);
                }
                self.state
                    .card_interest_index
                    .wait_until_authority_open()
                    .await;
                continue;
            }
            self.process_pending_replay_events_locked().await?;
            self.drain_card_events_at_boundary().await?;
            if requires_agent_authority
                && !authority_checked
                && self.state.is_live()
                && !self.state.card_interest_index.authority_is_open()
            {
                drop(boundary_guard);
                if !wait_for_authority {
                    return Ok(None);
                }
                self.state
                    .card_interest_index
                    .wait_until_authority_open()
                    .await;
                continue;
            }
            let retries = if self.has_pending_source_card_transfers_at_boundary() {
                permissions::prepare_pending_source_card_transfers(self).await?
            } else {
                Vec::new()
            };
            if retries.is_empty() {
                return Ok(Some(boundary_guard));
            }
            // Delivery acquires the target worker's boundary lock. Release the source lock to
            // avoid self-transfer and opposite-direction transfer deadlocks, then loop so no
            // caller can cross this boundary until the source confirmation is visible.
            drop(boundary_guard);
            permissions::complete_pending_source_card_transfers(self, retries).await?;
        }
    }

    fn has_pending_source_card_transfers_at_boundary(&self) -> bool {
        self.state
            .card_event_boundary_scan
            .as_ref()
            .is_some_and(|scan| {
                scan.pending
                    .iter()
                    .any(|pending| matches!(&pending.event, QueuedCardEvent::TransferStarted(_)))
            })
    }

    fn rederive_agent_effective_surface_from_wallet(&mut self) {
        if matches!(self.runtime, OwnerRuntime::Entity(_)) {
            return;
        }
        self.state.agent_effective_surface = if let Some(agent_id) = self.state.agent_id.as_ref() {
            let context = agent_monomorphization_context(
                self.owner_component_metadata(),
                &self.owned_agent_id,
                agent_id,
            );
            golem_common::model::card::agent_effective_surface_from_wallet_and_scope(
                &context,
                self.state.agent_wallet_cards.values(),
                self.state.invocation_scope_card.as_ref(),
            )
        } else {
            golem_common::model::card::EffectiveSurface::default()
        };
    }

    fn interested_card_ids(&self) -> Vec<CardId> {
        let mut card_ids = self
            .state
            .agent_wallet_cards
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if let Some(scope_card) = &self.state.invocation_scope_card {
            card_ids.extend(scope_card.root_card_ids.iter().copied());
        }
        card_ids.into_iter().collect()
    }

    async fn refresh_card_interest(&self) {
        self.state
            .card_interest_index
            .set_card_interest(self.owned_agent_id.clone(), &self.interested_card_ids())
            .await;
    }

    async fn install_invocation_scope_card(
        &mut self,
        scope_card: Option<ScopeCard>,
        root_cards: Vec<StoredCard>,
    ) {
        // Invocation scope is part of the effective authority but is not represented by a
        // worker-status generation. Force the first live authorization for this invocation
        // through the synchronized path so the cached expiration deadline includes its roots.
        self.state.authority_initialized = false;
        let (_, handles) = clear_invocation_scope_state(
            &mut self.state.invocation_scope_card,
            &mut self.state.invocation_scope_handles,
        );
        for rep in handles {
            let _ = self
                .table()
                .delete(Resource::<PermissionCardHandleRep>::new_own(rep));
        }
        self.state.invocation_scope_card = scope_card;
        self.state.invocation_scope_root_cards = root_cards
            .into_iter()
            .map(|card| (card.card_id(), card))
            .collect();
        self.rederive_agent_effective_surface_from_wallet();
        self.refresh_card_interest().await;
    }

    async fn clear_invocation_scope_card(&mut self) {
        self.state.authority_initialized = false;
        let (scope_changed, handles) = clear_invocation_scope_state(
            &mut self.state.invocation_scope_card,
            &mut self.state.invocation_scope_handles,
        );
        for rep in handles {
            let _ = self
                .table()
                .delete(Resource::<PermissionCardHandleRep>::new_own(rep));
        }
        if scope_changed {
            self.state.invocation_scope_root_cards.clear();
            self.rederive_agent_effective_surface_from_wallet();
        }
        self.refresh_card_interest().await;
    }

    fn clear_invocation_scope_if_roots_include(&mut self, card_ids: &[CardId]) -> bool {
        let (scope_changed, handles) = remove_invocation_scope_for_revoked_roots(
            &mut self.state.invocation_scope_card,
            &mut self.state.invocation_scope_handles,
            card_ids,
        );
        for rep in handles {
            let _ = self
                .table()
                .delete(Resource::<PermissionCardHandleRep>::new_own(rep));
        }
        if scope_changed {
            self.state.invocation_scope_root_cards.clear();
        }
        scope_changed
    }

    async fn drain_card_events_at_boundary(&mut self) -> Result<(), WorkerExecutorError> {
        if !self.state.is_live() {
            return Ok(());
        }

        loop {
            let pending_events =
                next_drainable_card_events(self.pending_card_events_at_boundary().await?);
            let Some(pending_event) = pending_events.first() else {
                break;
            };
            match &pending_event.event {
                QueuedCardEvent::Revoke(_) => {
                    let card_ids = pending_events
                        .into_iter()
                        .filter_map(|pending_event| match pending_event.event {
                            QueuedCardEvent::Revoke(event) => Some(event.card_id),
                            QueuedCardEvent::Install(_)
                            | QueuedCardEvent::TransferStarted(_)
                            | QueuedCardEvent::TransferReceived(_) => None,
                        })
                        .collect::<Vec<_>>();
                    self.apply_card_revoked_cascade(&card_ids, true).await?;
                }
                QueuedCardEvent::Install(event) => {
                    let Some(card) = event.card.clone() else {
                        return Err(WorkerExecutorError::runtime(
                            "queued card install is missing card payload",
                        ));
                    };
                    let _ = self
                        .apply_card_install(Some(pending_event.oplog_index), card)
                        .await?;
                }
                QueuedCardEvent::TransferReceived(event) => {
                    let Some(card) = event.card.clone() else {
                        return Err(WorkerExecutorError::runtime(
                            "received card transfer is missing card payload",
                        ));
                    };
                    let _ = self
                        .apply_received_card_transfer(
                            pending_event.oplog_index,
                            event.transfer_id,
                            event.source_card_id,
                            card,
                        )
                        .await?;
                }
                QueuedCardEvent::TransferStarted(_) => {
                    unreachable!("filtered above")
                }
            }
        }

        self.remove_expired_cards().await?;
        self.remove_expired_invocation_scope_roots().await?;
        Ok(())
    }

    async fn remove_expired_invocation_scope_roots(&mut self) -> Result<(), WorkerExecutorError> {
        let expired_root_ids =
            expired_wallet_card_ids_at(&self.state.invocation_scope_root_cards, Utc::now());
        if !expired_root_ids.is_empty() {
            self.apply_card_revoked_cascade(&expired_root_ids, true)
                .await?;
        }
        Ok(())
    }

    async fn pending_card_events_at_boundary(
        &mut self,
    ) -> Result<Vec<PendingCardEventRef>, WorkerExecutorError> {
        let status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        let status_idx = status.oplog_idx;
        let status_pending = status.pending_card_events;

        let oplog = self.public_state.worker().oplog();
        let current_idx = oplog.current_oplog_index().await;

        match &mut self.state.card_event_boundary_scan {
            Some(scan) => scan.synchronize(status_idx, &status_pending, current_idx),
            None => {
                self.state.card_event_boundary_scan =
                    Some(CardEventBoundaryScan::new(status_idx, status_pending));
            }
        }

        let unread_range = self
            .state
            .card_event_boundary_scan
            .as_ref()
            .expect("card event boundary scan must be initialized")
            .unread_range(current_idx);

        if let Some((start, count)) = unread_range {
            let entries = oplog.read_exact(start, count).await;
            self.state
                .card_event_boundary_scan
                .as_mut()
                .expect("card event boundary scan must be initialized")
                .fold_through(current_idx, &entries);
        }

        Ok(self
            .state
            .card_event_boundary_scan
            .as_ref()
            .expect("card event boundary scan must be initialized")
            .pending
            .clone())
    }

    async fn admit_card_to_wallet(
        &mut self,
        card: &StoredCard,
    ) -> Result<Result<(), CardInstallFailure>, WorkerExecutorError> {
        let card_id = card.card_id();
        let mut candidate_wallet_card_ids = self.interested_card_ids();
        if !candidate_wallet_card_ids.contains(&card_id) {
            candidate_wallet_card_ids.push(card_id);
        }
        self.state
            .card_interest_index
            .set_card_interest(self.owned_agent_id.clone(), &candidate_wallet_card_ids)
            .await;

        let card_state = self
            .state
            .card_service
            .check_cards(vec![card_id])
            .await?
            .remove(&card_id);

        let failure = match card_state {
            Some(CardState::Live(registered_card)) if registered_card.as_ref() == card => None,
            Some(CardState::Live(_)) => Some(CardInstallFailure::NotPermitted),
            Some(CardState::Revoked) => Some(CardInstallFailure::CardRevoked),
            Some(CardState::Unknown) | None => Some(CardInstallFailure::NotFound),
        };
        if let Some(failure) = failure {
            self.refresh_card_interest().await;
            return Ok(Err(failure));
        }

        if add_wallet_card(
            &mut self.state.agent_wallet_cards,
            &mut self.state.wallet_generation,
            card.clone(),
        )? {
            self.rederive_agent_effective_surface_from_wallet();
        }
        self.refresh_card_interest().await;

        Ok(Ok(()))
    }

    pub(crate) async fn apply_card_install(
        &mut self,
        queued_event_index: Option<OplogIndex>,
        card: StoredCard,
    ) -> Result<Result<(), CardInstallFailure>, WorkerExecutorError> {
        let card_id = card.card_id();
        if let Err(reason) = self.admit_card_to_wallet(&card).await? {
            if let Some(queued_event_index) = queued_event_index {
                self.public_state
                    .worker()
                    .add_and_commit_oplog(OplogEntry::card_install_failed(
                        queued_event_index,
                        card_id,
                        reason,
                    ))
                    .await;
            }
            Ok(Err(reason))
        } else {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::card_installed(
                    queued_event_index,
                    card,
                    Some(self.state.wallet_generation),
                ))
                .await;
            Ok(Ok(()))
        }
    }

    async fn apply_received_card_transfer(
        &mut self,
        queued_event_index: OplogIndex,
        transfer_id: uuid::Uuid,
        source_card_id: Option<CardId>,
        card: StoredCard,
    ) -> Result<Result<(), CardInstallFailure>, WorkerExecutorError> {
        let card_id = card.card_id();
        if let Err(reason) = self.admit_card_to_wallet(&card).await? {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::card_install_failed(
                    queued_event_index,
                    card_id,
                    reason,
                ))
                .await;
            return Ok(Err(reason));
        }

        self.public_state
            .worker()
            .add_and_commit_oplog(OplogEntry::card_transferred(
                transfer_id,
                source_card_id,
                card_id,
                CardHolder::Agent(AgentCardHolder {
                    agent_id: self.owned_agent_id.agent_id.clone(),
                }),
                card,
                Some(self.state.wallet_generation),
            ))
            .await;
        Ok(Ok(()))
    }

    async fn apply_card_revoked(
        &mut self,
        card_id: CardId,
        queued_event_index: OplogIndex,
        is_live: bool,
    ) -> Result<(), WorkerExecutorError> {
        let was_in_wallet = remove_wallet_card(
            &mut self.state.agent_wallet_cards,
            &mut self.state.wallet_generation,
            card_id,
        )?;

        let scope_changed = self.clear_invocation_scope_if_roots_include(&[card_id]);
        if was_in_wallet || scope_changed {
            self.rederive_agent_effective_surface_from_wallet();
        }

        if is_live {
            self.refresh_card_interest().await;

            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::card_revoked(
                    queued_event_index,
                    card_id,
                    Some(self.state.wallet_generation),
                ))
                .await;
        }

        Ok(())
    }

    pub(crate) async fn apply_card_revoked_cascade(
        &mut self,
        card_ids: &[CardId],
        commit_immediately: bool,
    ) -> Result<(), WorkerExecutorError> {
        let mut card_ids = card_ids.to_vec();
        card_ids.sort_unstable();
        card_ids.dedup();
        if card_ids.is_empty() {
            return Ok(());
        }

        let wallet_changed = remove_wallet_cards(
            &mut self.state.agent_wallet_cards,
            &mut self.state.wallet_generation,
            &card_ids,
        )?;
        let scope_changed = self.clear_invocation_scope_if_roots_include(&card_ids);
        if wallet_changed || scope_changed {
            self.rederive_agent_effective_surface_from_wallet();
        }

        self.refresh_card_interest().await;

        let affected_wallets = if wallet_changed {
            vec![CardHolder::Agent(AgentCardHolder {
                agent_id: self.owned_agent_id.agent_id.clone(),
            })]
        } else {
            Vec::new()
        };
        let entry = OplogEntry::CardRevokedCascade {
            timestamp: Timestamp::now_utc(),
            revoked_card_ids: card_ids,
            affected_wallets,
            local_wallet_generation: Some(self.state.wallet_generation),
        };
        if commit_immediately {
            self.public_state.worker().add_and_commit_oplog(entry).await;
        } else {
            self.public_state.worker().add_to_oplog(entry).await;
        }

        Ok(())
    }

    pub(crate) async fn remove_expired_cards(&mut self) -> Result<(), WorkerExecutorError> {
        let cards_to_expire =
            expired_wallet_card_ids_at(&self.state.agent_wallet_cards, Utc::now());

        if cards_to_expire.is_empty() {
            return Ok(());
        }

        let mut expired_card_generations = Vec::with_capacity(cards_to_expire.len());
        for card_id in cards_to_expire {
            if remove_wallet_card(
                &mut self.state.agent_wallet_cards,
                &mut self.state.wallet_generation,
                card_id,
            )? {
                expired_card_generations.push((card_id, self.state.wallet_generation));
            }
        }

        if !expired_card_generations.is_empty() {
            self.rederive_agent_effective_surface_from_wallet();
        }

        self.refresh_card_interest().await;

        for (card_id, wallet_generation) in expired_card_generations {
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::card_expired(card_id, Some(wallet_generation)))
                .await;
        }
        Ok(())
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

    pub fn owner_component_metadata(&self) -> &Component {
        match &self.runtime {
            OwnerRuntime::Agent => &self.state.component_metadata,
            OwnerRuntime::Entity(_) => self
                .state
                .owner_component_metadata
                .as_deref()
                .expect("Entity Store must pin owner component metadata at dispatch"),
        }
    }

    pub fn agent_type_provision_config(&self) -> Option<&AgentTypeProvisionConfig> {
        self.state.agent_id.as_ref().and_then(|agent_id| {
            self.owner_component_metadata()
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
            .store(is_replay, Ordering::Release);
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

    pub fn as_wasi_http_view_p3(&mut self) -> wasmtime_wasi_http::p3::WasiHttpCtxView<'_> {
        let is_replay = self.state.is_replay();
        self.http_hooks
            .is_replay
            .store(is_replay, std::sync::atomic::Ordering::Release);
        let inner = &mut *self;
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        wasmtime_wasi_http::p3::WasiHttpCtxView {
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

    pub fn card_service(&self) -> Arc<dyn CardService> {
        self.state.card_service.clone()
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
        self.linear_memory.current_bytes()
    }

    pub fn max_linear_memory_size(&self) -> u64 {
        self.resource_limits.max_memory_limit() as u64
    }

    pub fn linear_memory_tracker(&self) -> LinearMemoryTracker {
        self.linear_memory.clone()
    }

    async fn switch_to_live(&self) {
        self.state.replay_state.switch_to_live().await;
        if self.runtime == OwnerRuntime::Agent {
            // Incomplete concurrent calls must be woken first so their reconstruction tasks can
            // repair the original Starts. The primary remains fenced at this await and cannot
            // admit live guest execution until every historical entity coordinator exits.
            self.owner_execution
                .wait_for_historical_reconstructions()
                .await;
        }
        self.linear_memory.switch_to_live();
    }

    fn cleanup_custom_durability_state(&mut self) {
        let resources: Vec<_> = self
            .state
            .custom_invocation_scopes
            .drain()
            .map(|(_, scope)| scope.resource_rep)
            .collect();
        for resource_rep in resources {
            let _ = self
                .table()
                .delete(Resource::<durability::LiveCustomDurableInvocation>::new_own(resource_rep));
        }
        self.state.active_custom_invocations.clear();
    }
    pub fn increase_memory(&mut self, delta: u64) {
        let (_, reconciling) = self.linear_memory.grow(delta, Instant::now());
        if self.runtime == OwnerRuntime::Agent && self.state.is_live() && !reconciling {
            // This is called from the `memory.grow` async resource limiter, which
            // Wasmtime runs through a blocking libcall on the store's fiber. While
            // that libcall waits, the store cannot make progress, so nothing may be
            // awaited here (see https://github.com/bytecodealliance/wasmtime/issues/11869).
            // The oplog hint runs as a fire-and-forget job on the worker-state
            // actor. Host-capacity admission completed before Wasmtime committed
            // this growth.
            self.public_state.worker().request_memory_grow(delta);
        }
    }

    pub(crate) async fn try_acquire_linear_memory(&self, delta: u64) -> Option<MemoryGrant> {
        self.public_state
            .worker()
            .active_agents()
            .try_acquire(delta)
            .await
    }

    pub async fn admit_unshared_memory_growth(
        &self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let tracker = &self.linear_memory;
        let growth = tracker.prepare_unshared_growth(current, desired);
        let Some(growth) = validate_unshared_memory_growth(
            growth,
            self.max_linear_memory_size(),
            desired,
            maximum,
        ) else {
            tracker.memory_grow_failed();
            return Err(GolemSpecificWasmTrap::WorkerExceededMemoryLimit.into());
        };

        if growth.admission_delta > 0 {
            let Some(grant) = self.try_acquire_linear_memory(growth.admission_delta).await else {
                tracker.memory_grow_failed();
                crate::metrics::workers::record_worker_memory_grow_rejected();
                return Err(GolemSpecificWasmTrap::WorkerOutOfMemory.into());
            };
            tracker.retain_growth_grant(grant);
        }
        Ok(true)
    }

    pub fn unshared_memory_growth_failed(&self) {
        self.linear_memory.memory_grow_failed();
    }

    /// Returns the deterministic, policy-independent recovery decision for a
    /// trap type — i.e. the cases where the answer does not depend on retry
    /// state or any retry policy. For trap-error variants whose decision is
    /// driven by named retry policies (`Unknown`, `TransientError`, and
    /// `DeterministicTrap` inside an atomic region), this returns `None` and
    /// the caller falls through to policy-based resolution.
    pub(crate) fn fixed_decision_for_trap_type(trap_type: &TrapType) -> Option<RetryDecision> {
        match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt(_)) => Some(RetryDecision::None),
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
                error: AgentError::PermissionDenied(_),
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
            // path (handled by the caller). Membership comes from the trap
            // itself (the call's own region for a durable-call trap, the
            // ambient state otherwise), not from "any region currently active".
            TrapType::Error {
                error: AgentError::DeterministicTrap(_),
                in_atomic_region: false,
                ..
            } => Some(RetryDecision::None),
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
            AgentError::PermissionDenied(_) => "permission-denied",
        }
    }

    async fn get_recovery_decision_on_trap_with_semantic(
        &mut self,
        retry_state_with_current_attempt: &HashMap<OplogIndex, RetryPolicyState>,
        trap_type: &TrapType,
        full_function_name: &str,
    ) -> (RetryDecision, Option<RetryPolicyState>) {
        // Cases whose decision does not depend on retry policy at all
        // (Interrupt, Exit, deterministic AgentError variants like
        // OutOfMemory, InvalidRequest, …). Returns `None` when policy
        // resolution is required.
        if let Some(decision) = Self::fixed_decision_for_trap_type(trap_type) {
            return (decision, None);
        }

        // Only Error variants whose decision is policy-driven reach this point
        // (Unknown, TransientError, and DeterministicTrap-in-atomic-region).
        let TrapType::Error {
            error,
            retry_from,
            semantic_trap_retry_override,
            ..
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
        logging::policy::emit_log_event_with_state::<Ctx>(
            event,
            self.state.component_metadata.metadata.has_oplog_processor(),
            &self.owned_agent_id,
            &self.public_state,
            &self.state.replay_state,
            &self.state.oplog,
            self.state.is_live(),
            self.entity_parent_start_index(),
        )
        .await;
    }

    pub async fn begin_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        if self.state.durability_is_suppressed() {
            let begin_index = self.state.current_oplog_index().await;
            self.state.current_retry_point = begin_index;
            return Ok(begin_index);
        }

        if self.state.opens_durable_scope(function_type) {
            // During replay, the scope `End` is folded into the resolver: claiming the scope
            // `Start` registers an awaiter keyed by its `begin_index`, and `end_function` awaits it
            // instead of reading the `End` positionally. The handle is carried in the active scope
            // and only stored when the scope continues replaying (not when recovery switches to live
            // and re-runs the body, which appends a fresh `End` live).
            let mut scope_replay_handle: Option<concurrent::ReplayCallHandle> = None;
            let result = if self.is_live() {
                // Durable scopes are siblings rather than nested under other durable scopes. In an
                // entity body they are direct children of the outer entity invocation; their own
                // host calls point back at the scope Start through `child_parent_start_index`.
                let entry = OplogEntry::Start {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index: self.entity_parent_start_index(),
                    function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
                    invocation_id: None,
                    observational_owner: None,
                    request: None,
                    durable_function_type: function_type.clone(),
                };
                let begin_index = self.public_state.worker().add_and_commit_oplog(entry).await;
                Ok(begin_index)
            } else {
                let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
                let (begin_index, scope_handle) = self
                    .state
                    .replay_state
                    .claim_scope_start(&scope_name, function_type, self.entity_parent_start_index())
                    .await?;
                // The begin-side completion / legality probe stays a non-consuming forward scan: it
                // decides whether the scope is safe to continue replaying or must be retried *before*
                // the scope body is replayed. Only the `End` *consumption* moves to the resolver.
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
                        self.switch_to_live().await;
                        Err(WorkerExecutorError::runtime(
                            "Non-idempotent remote write operation was not completed, cannot retry",
                        ))
                    } else {
                        scope_replay_handle = Some(scope_handle);
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
                            OplogEntry::is_end_remote_write_s::<ScopeScanState>,
                            OplogEntry::no_concurrent_side_effect,
                            ScopeScanState::new(begin_index),
                            OplogEntry::track_scope_membership,
                        )
                        .await;
                    match lookup_result {
                        OplogEntryLookupResult::Found { index, .. } => {
                            debug!(
                                "Remote write operation {begin_index} already completed at {index}, continue replaying"
                            );
                            scope_replay_handle = Some(scope_handle);
                            Ok(begin_index)
                        }
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: true,
                        } => {
                            // Must switch to live mode before failing to be able to commit an Error entry
                            self.switch_to_live().await;
                            Err(WorkerExecutorError::runtime(
                                "Non-idempotent remote write operation was not completed, cannot retry",
                            ))
                        }
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: false,
                        } if self.state.assume_idempotence => {
                            // We need to jump to the end of the oplog
                            self.switch_to_live().await;

                            // But this is not enough, because if the retried batched write operation succeeds,
                            // and later we replay it, we need to skip the first attempt and only replay the second.
                            // Se we add a Jump entry to the oplog that registers a deleted region.
                            let deleted_region = OplogRegion {
                                start: begin_index.next(), // keep the durable scope `Start` at `begin_index`
                                end: self.state.replay_state.replay_target().next(), // skipping the Jump entry too
                            };

                            self.public_state
                                .worker()
                                .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                                .await;

                            // TODO: this recomputation should not be necessary.
                            self.public_state.worker().reattach_worker_status().await;
                            // Switched to live and re-running the body: the scope `End` will be
                            // appended live by `end_function`, so do not store the (now incomplete)
                            // replay handle.
                            Ok(begin_index)
                        }
                        OplogEntryLookupResult::NotFound { .. } => {
                            // assume_idempotence is false and the operation was not completed —
                            // we cannot safely retry a non-idempotent batched write.
                            self.switch_to_live().await;
                            Err(WorkerExecutorError::runtime(
                                "Non-idempotent remote write operation was not completed, cannot retry",
                            ))
                        }
                    }
                } else {
                    scope_replay_handle = Some(scope_handle);
                    Ok(begin_index)
                }
            }?;

            // A durable scope (remote write / HTTP request) is now open until the matching
            // `end_function`; the tip is inside it, so block mid-invocation checkpoints, and any
            // `Start` written while it is open links back to it via `parent_start_index`.
            let kind = if matches!(
                *function_type,
                DurableFunctionType::WriteRemoteBatched(None)
            ) {
                DurableScopeKind::BatchedWrite
            } else {
                DurableScopeKind::NonIdempotentWrite
            };
            self.state
                .push_durable_scope(result, kind, scope_replay_handle);

            // The effective retry point now derives from the open scope; keep the global fallback
            // pointing at the scope `Start` so it survives the scope being closed.
            self.state.current_retry_point = result;
            Ok(result)
        } else {
            // When there is no scope `Start` entry, the current retry point can only
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
        if self.state.durability_is_suppressed() {
            return Ok(());
        }

        if self.state.opens_durable_scope(function_type) {
            if self.is_live() {
                let entry = OplogEntry::End {
                    timestamp: Timestamp::now_utc(),
                    start_index: begin_index,
                    response: None,
                    forced_commit: true,
                };
                self.state.oplog.add(entry).await;
                // The durable scope opened in `begin_function` is now closed.
                self.state.remove_durable_scope(begin_index)?;
            } else {
                // The scope `End` was folded into the resolver at scope-open, so consume it
                // through the resolver (never positionally, which under overlap could steal a
                // concurrently-replaying sibling call's terminal). This also repairs a
                // crash-induced half-pair and closes the in-memory scope.
                self.close_durable_scope_replay(begin_index).await?;
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    /// Closes a durable scope during replay by awaiting its `End` through the resolver, then
    /// removing the in-memory scope. The scope `End` was registered as a resolver awaiter when its
    /// `Start` was claimed (`claim_scope_start`), so it is delivered here whether it is the entry at
    /// the cursor head or was already auto-drained to this scope's handle by another cursor driver.
    ///
    /// A crash between a scope's terminal marker and its `End` (`add_pair` gives contiguity, not
    /// crash atomicity) truncates the oplog at the marker, so the awaited `End` resolves as
    /// `Incomplete`; rather than hard-failing we append the missing `End` live to repair the pair for
    /// future replays. A `None` handle means the scope was opened live (or recovery switched to
    /// live at scope-open), in which case there is no recorded `End` to await.
    async fn close_durable_scope_replay(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        match self.state.take_durable_scope_replay_handle(begin_index) {
            Some(handle) => {
                match self
                    .state
                    .replay_state
                    .await_resolution_outcome(handle)
                    .await?
                {
                    concurrent::ResolutionOutcome::Resolved(
                        concurrent::Resolution::Completed { .. },
                    ) => {}
                    concurrent::ResolutionOutcome::Resolved(
                        concurrent::Resolution::Cancelled { .. },
                    ) => {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            format!("End {{ start_index: {begin_index} }}"),
                            format!("Cancelled {{ start_index: {begin_index} }}"),
                        ));
                    }
                    concurrent::ResolutionOutcome::Resolved(
                        concurrent::Resolution::CompletedButDiscarded {
                            end_idx,
                            marker_idx,
                            ..
                        },
                    ) => {
                        // Discarded completions are recorded only for accessor completion
                        // futures; a marker referencing a durable scope `Start` means the oplog
                        // does not match this code path.
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            format!("End {{ start_index: {begin_index} }}"),
                            format!(
                                "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a durable scope"
                            ),
                        ));
                    }
                    concurrent::ResolutionOutcome::Incomplete => {
                        // Half-pair recovery: the scope `Start` (and any terminal marker) is
                        // committed but the scope `End` was lost to a crash. Replay has reached the
                        // end of the oplog, so append the missing `End` live to complete the pair.
                        self.state
                            .oplog
                            .add(OplogEntry::End {
                                timestamp: Timestamp::now_utc(),
                                start_index: begin_index,
                                response: None,
                                forced_commit: true,
                            })
                            .await;
                    }
                }
            }
            None => {
                // Opened live (or recovery switched to live at scope-open): no recorded `End` to
                // await.
            }
        }
        self.state.remove_durable_scope(begin_index)
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
        if self.state.durability_is_suppressed() {
            let (_, tx) = handler.create_new().await?;
            let begin_index = self.state.current_oplog_index().await;
            Ok((begin_index, tx))
        } else if self.is_live() {
            let (tx_id, tx) = handler.create_new().await?;
            // A transaction is a durable scope: append the scope `Start` and the
            // `BeginRemoteTransaction` marker atomically so the pair is never split across a crash
            // boundary. The scope `Start` index is the stable begin index for the whole transaction.
            // Like other scope Starts it is a direct child of the entity invocation, when present;
            // its child host calls point back at it via `WriteRemoteTransaction(Some(begin_index))`.
            let scope_start = OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: self.entity_parent_start_index(),
                function_name: HostFunctionName::Custom("<scope:transaction>".to_string()),
                invocation_id: None,
                observational_owner: None,
                request: None,
                durable_function_type: DurableFunctionType::WriteRemoteTransaction(None),
            };
            let (begin_index, _) = self
                .public_state
                .worker()
                .oplog()
                .add_pair(
                    scope_start,
                    Box::new(move |_start_index| OplogEntry::begin_remote_transaction(tx_id, None)),
                )
                .await;
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;

            // The transaction scope is now open until commit/rollback; block checkpoints. Opened
            // live, so there is no recorded scope `End` to await on close.
            self.state
                .push_durable_scope(begin_index, DurableScopeKind::Transaction, None);
            self.state.current_retry_point = begin_index;

            Ok((begin_index, tx))
        } else {
            // The transaction scope `Start` is preserved across restarts, so its index is the
            // stable original begin index that keys every transaction marker. Its `End` is folded
            // into the resolver: `claim_scope_start` consumes the `Start`, validates the exact
            // `<scope:transaction>` shape `begin_transaction_function` writes (so a corrupt or
            // interleaved oplog fails here instead of silently driving the recovery logic with the
            // wrong scope), and registers an awaiter the transaction terminal awaits instead of
            // reading the scope `End` positionally. The handle is stored only when the transaction
            // continues replaying (not when recovery restarts it live).
            let mut scope_replay_handle: Option<concurrent::ReplayCallHandle> = None;
            let scope_name = HostFunctionName::Custom("<scope:transaction>".to_string());
            let (scope_start_index, scope_handle) = self
                .state
                .replay_state
                .claim_scope_start(
                    &scope_name,
                    &DurableFunctionType::WriteRemoteTransaction(None),
                    self.entity_parent_start_index(),
                )
                .await?;
            let (begin_index, begin_entry) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::BeginRemoteTransaction
            )?;
            // The `BeginRemoteTransaction` right after the scope `Start` either starts a fresh
            // transaction (`original_begin_index: None`) or, after a restart, points back at this
            // scope `Start`.
            if let OplogEntry::BeginRemoteTransaction {
                original_begin_index: Some(idx),
                ..
            } = &begin_entry
                && *idx != scope_start_index
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!(
                        "BeginRemoteTransaction {{ original_begin_index: None | Some({scope_start_index}) }}"
                    ),
                    format!("BeginRemoteTransaction {{ original_begin_index: Some({idx}) }}"),
                )
                .into());
            }
            let original_begin_index = scope_start_index;

            let assume_idempotence = self.state.assume_idempotence;

            let pre_entry = self
                .state
                .replay_state
                .lookup_oplog_entry_with_condition_and_state(
                    original_begin_index,
                    OplogEntry::is_pre_remote_transaction_s,
                    OplogEntry::no_concurrent_side_effect,
                    ScopeScanState::new(original_begin_index),
                    OplogEntry::track_scope_membership,
                )
                .await;

            let tx_id = try_match!(
                begin_entry,
                OplogEntry::BeginRemoteTransaction { transaction_id, .. }
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
                            ScopeScanState::new(original_begin_index),
                            OplogEntry::track_scope_membership,
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
                                .is_pre_rollback_remote_transaction(original_begin_index)
                            {
                                // if we can not confirm the transaction was rolled back, we need to restart
                                should_restart = !handler.is_rolled_back(&tx_id).await?;
                            }
                        }
                        OplogEntryLookupResult::NotFound {
                            violates_for_all: true,
                        } => {
                            // Must switch to live mode before failing to be able to commit an Error entry
                            self.switch_to_live().await;
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
                    self.switch_to_live().await;
                    return Err(WorkerExecutorError::runtime(
                        "Transaction overlapped with other side effects was not completed, cannot retry",
                    ).into());
                }
            };

            let (result, tx) = if should_restart {
                // We need to jump to the end of the oplog
                self.switch_to_live().await;

                if !assume_idempotence {
                    Err(WorkerExecutorError::runtime(
                        "Non-idempotent remote write operation was not completed, cannot retry",
                    ))
                } else {
                    // But this is not enough, because if the retried batched write operation succeeds,
                    // and later we replay it, we need to skip the first attempt and only replay the second.
                    // Se we add a Jump entry to the oplog that registers a deleted region.
                    let deleted_region = OplogRegion {
                        // Delete the previous `BeginRemoteTransaction` entry (and everything after),
                        // because we'll get a new tx id. The transaction scope `Start` lives at
                        // `scope_start_index < begin_index`, so it is preserved.
                        start: begin_index,
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

                    // Restarted live (jump + fresh `BeginRemoteTransaction`): the scope `End` will
                    // be appended live by the transaction terminal, so do not store the (now
                    // incomplete) replay handle.
                    Ok((original_begin_index, tx))
                }
            } else {
                scope_replay_handle = Some(scope_handle);
                Ok((original_begin_index, tx))
            }?;

            // The (possibly re-begun) transaction scope is open until commit/rollback.
            self.state.push_durable_scope(
                result,
                DurableScopeKind::Transaction,
                scope_replay_handle,
            );
            self.state.current_retry_point = original_begin_index;

            Ok((result, tx))
        }
    }

    pub async fn pre_commit_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.durability_is_suppressed() {
            Ok(())
        } else if self.is_live() {
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
        if self.state.durability_is_suppressed() {
            Ok(())
        } else if self.is_live() {
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
        if self.state.durability_is_suppressed() {
            return Ok(());
        } else if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            // The final marker and the scope `End` are appended as an atomic pair so they can never
            // be split across a crash boundary (which would leave a marker without its `End`).
            self.state
                .oplog
                .fallible_add_pair(
                    OplogEntry::committed_remote_transaction(begin_index),
                    OplogEntry::End {
                        timestamp: Timestamp::now_utc(),
                        start_index: begin_index,
                        response: None,
                        forced_commit: true,
                    },
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
            // The transaction scope opened in `begin_transaction_function` is now closed: the
            // `CommittedRemoteTransaction` marker and the scope `End` have been durably committed,
            // so the tip is no longer inside a jumpable scope on its account.
            self.state.remove_durable_scope(begin_index)?;
        } else {
            let (_, _) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::CommittedRemoteTransaction
            )?;
            // The scope `End` was folded into the resolver at scope-open, so await it (the
            // terminal marker stays positional). If a crash split the marker/`End` pair, the
            // `End` resolves as `Incomplete` and is repaired live. Also closes the in-memory scope.
            self.close_durable_scope_replay(begin_index).await?;
        }
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
        if self.state.durability_is_suppressed() {
            return Ok(());
        } else if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            // The final marker and the scope `End` are appended as an atomic pair so they can never
            // be split across a crash boundary (which would leave a marker without its `End`).
            self.state
                .oplog
                .fallible_add_pair(
                    OplogEntry::rolled_back_remote_transaction(begin_index),
                    OplogEntry::End {
                        timestamp: Timestamp::now_utc(),
                        start_index: begin_index,
                        response: None,
                        forced_commit: true,
                    },
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
            // The transaction scope opened in `begin_transaction_function` is now closed: the
            // `RolledBackRemoteTransaction` marker and the scope `End` have been durably committed,
            // so the tip is no longer inside a jumpable scope on its account.
            self.state.remove_durable_scope(begin_index)?;
        } else {
            let (_, _) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::RolledBackRemoteTransaction
            )?;
            // The scope `End` was folded into the resolver at scope-open, so await it (the
            // terminal marker stays positional). If a crash split the marker/`End` pair, the
            // `End` resolves as `Incomplete` and is repaired live. Also closes the in-memory scope.
            self.close_durable_scope_replay(begin_index).await?;
        }
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
                            .durable_ctx_mut()
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
                            .durable_ctx_mut()
                            .end_call_snapshotting_function_if_active();

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
        let (snapshot_index, snapshot_source) = {
            let state = &store.as_context().data().durable_ctx().state;
            (state.last_snapshot_index, state.last_snapshot_source)
        };

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
                let error = format!(
                    "Expected Snapshot entry at oplog index {snapshot_index}, found different entry; falling back to full replay"
                );
                warn!("{error}");
                Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
                if snapshot_source == Some(SnapshotSource::Automatic) {
                    return SnapshotRecoveryResult::Failed;
                }
                if let Err(err) = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .restart_replay_without_snapshot()
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
                let error = format!(
                    "Failed to download snapshot payload: {err}; falling back to full replay"
                );
                warn!("{error}");
                Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
                if snapshot_source == Some(SnapshotSource::Automatic) {
                    return SnapshotRecoveryResult::Failed;
                }
                if let Err(err) = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .restart_replay_without_snapshot()
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
                let error =
                    format!("Snapshot recovery failed to lower load-snapshot invocation: {err}");
                warn!("{error}");
                Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
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
            let error = format!("Snapshot recovery failed to install invocation context: {err}");
            warn!("{error}");
            Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
            return SnapshotRecoveryResult::Failed;
        }

        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .begin_call_snapshotting_function();

        let load_result =
            invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await;

        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .end_call_snapshotting_function_if_active();

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
            Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
            SnapshotRecoveryResult::Failed
        } else {
            debug!("Snapshot loaded successfully from oplog index {snapshot_index}");
            Self::emit_snapshot_recovery_event(store, snapshot_index, true, None);
            if snapshot_source == Some(SnapshotSource::Automatic) {
                store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .replaying_automatic_snapshot_tail = true;
            }
            SnapshotRecoveryResult::Success
        }
    }

    /// Abandons an automatic snapshot whose replayed tail diverged from the recorded oplog: the
    /// worker is recreated with automatic snapshot recovery disabled so it replays the full oplog.
    fn abandon_diverged_automatic_snapshot(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        full_function_name: &str,
        error: &AgentError,
    ) -> RetryDecision {
        let snapshot_index = store
            .as_context()
            .data()
            .durable_ctx()
            .state
            .last_snapshot_index
            .expect("an automatic snapshot tail is only replayed after loading a snapshot");
        let error = format!(
            "Replaying {full_function_name} after loading the snapshot diverged from the recorded oplog: {}; falling back to full replay",
            error.message()
        );
        warn!("{error}");
        Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
        store
            .as_context()
            .data()
            .get_public_state()
            .worker()
            .snapshot_recovery_disabled
            .store(true, Ordering::Release);
        RetryDecision::Immediate
    }

    fn emit_snapshot_recovery_event(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        snapshot_index: OplogIndex,
        succeeded: bool,
        error: Option<String>,
    ) {
        store
            .as_context_mut()
            .data_mut()
            .get_public_state()
            .event_service()
            .emit_event(
                if succeeded {
                    InternalWorkerEvent::snapshot_recovery_succeeded(snapshot_index)
                } else {
                    InternalWorkerEvent::snapshot_recovery_failed(
                        snapshot_index,
                        error.unwrap_or_else(|| "unknown".to_string()),
                    )
                },
                true,
            );
    }
}

enum SnapshotRecoveryResult {
    Success,
    NotAttempted,
    Failed,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    async fn restart_replay_without_snapshot(&mut self) -> Result<(), WorkerExecutorError> {
        self.state.replay_state.drop_override_and_restart().await?;

        self.state.agent_wallet_cards = match self.state.agent_id.as_ref() {
            Some(agent_id) => {
                let card = agent_initial_card_from_component_metadata(
                    &self.state.component_metadata,
                    agent_id,
                )?;
                BTreeMap::from([(card.card_id(), card)])
            }
            None => BTreeMap::new(),
        };
        self.state.wallet_generation = 0;
        self.rederive_agent_effective_surface_from_wallet();

        Ok(())
    }

    /// Activity tracker for Golem-spawned store background tasks; see
    /// [`tail_work::TailWorkTracker`].
    pub fn tail_work_tracker(&self) -> tail_work::TailWorkTracker {
        self.state.tail_work_tracker()
    }

    pub(crate) fn live_stream_event_capacity(&self) -> usize {
        self.state
            .config
            .limits
            .live_stream_event_broadcast_capacity
            .get()
    }

    pub(crate) fn stream_runtime_teardown_probe(
        &self,
    ) -> Arc<dyn Fn() -> bool + Send + Sync + 'static> {
        let stream_runtime_teardown = self.stream_runtime_teardown.clone();
        Arc::new(move || stream_runtime_teardown.load(Ordering::Acquire))
    }

    pub(crate) fn begin_stream_runtime_teardown(&self) {
        self.stream_runtime_teardown.store(true, Ordering::Release);
    }

    /// Arms the optional per-invocation wall-clock deadline (`limits.max_invocation_duration`)
    /// and returns its guard. Called at the start of every guest invocation.
    ///
    /// When configured, a timer task is spawned that, once the deadline elapses, latches the
    /// shared `invocation_deadline_exceeded` flag and broadcasts a *synthetic* interrupt on the
    /// running execution status's interrupt-signal channel — without changing the execution
    /// status itself, so the worker is never externally observed as `Interrupted`. The broadcast
    /// wakes every cooperative host park point already racing the interrupt signal; the latched
    /// flag makes `check_interrupt` (epoch callback, CPU-bound wasm) and every *subsequently
    /// created* interrupt signal observe the deadline too. The invocation boundary converts the
    /// resulting synthetic interrupt unwind into a typed timeout failure (see
    /// `apply_invocation_deadline` in `worker::invocation`).
    ///
    /// Dropping the guard (at the invocation boundary) aborts the timer and clears the latch;
    /// arming also clears it first, so a stale latch from a lost abort race cannot leak into the
    /// next invocation.
    pub fn arm_invocation_deadline(&self) -> InvocationDeadline {
        let latch = self.state.invocation_deadline_exceeded.clone();
        latch.store(false, Ordering::Release);
        let duration = self.state.config.limits.max_invocation_duration;
        let timer = duration.map(|duration| {
            let latch = latch.clone();
            let execution_status = self.execution_status.clone();
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                latch.store(true, Ordering::Release);
                let interrupt_signal = {
                    let status = execution_status.read().unwrap();
                    match &*status {
                        ExecutionStatus::Running {
                            interrupt_signal, ..
                        } => Some(interrupt_signal.clone()),
                        _ => None,
                    }
                };
                if let Some(interrupt_signal) = interrupt_signal {
                    let _ = interrupt_signal.send(InterruptKind::Interrupt(Timestamp::now_utc()));
                }
            })
        });
        InvocationDeadline {
            latch,
            duration,
            timer,
        }
    }

    /// Arms a deadline that starts when `drain_started` is notified after the guest export has
    /// returned. When it fires, the shared latch and interrupt broadcast make both existing and
    /// subsequently-created cooperative park points unwind their durable call handles safely.
    pub(crate) fn arm_tail_work_deadline(
        &self,
        drain_started: Arc<tokio::sync::Notify>,
    ) -> TailWorkDeadline {
        let latch = self.state.tail_work_deadline_exceeded.clone();
        latch.store(false, Ordering::Release);
        let timer_latch = latch.clone();
        let execution_status = self.execution_status.clone();
        let duration = self.state.config.limits.tail_work_settle_timeout;
        let timer = tokio::spawn(async move {
            drain_started.notified().await;
            tokio::time::sleep(duration).await;
            timer_latch.store(true, Ordering::Release);
            let interrupt_signal = {
                let status = execution_status.read().unwrap();
                match &*status {
                    ExecutionStatus::Running {
                        interrupt_signal, ..
                    } => Some(interrupt_signal.clone()),
                    _ => None,
                }
            };
            if let Some(interrupt_signal) = interrupt_signal {
                let _ = interrupt_signal.send(InterruptKind::Interrupt(Timestamp::now_utc()));
            }
        });
        TailWorkDeadline {
            latch,
            duration,
            timer: Some(timer),
        }
    }

    /// Whether the worker is currently being interrupted through the Golem API
    /// (`ExecutionStatus::Interrupting`), independent of the invocation-deadline latch.
    pub fn is_interrupting(&self) -> bool {
        matches!(
            &*self.execution_status.read().unwrap(),
            ExecutionStatus::Interrupting { .. }
        )
    }

    pub(crate) fn end_call_snapshotting_function_if_active(&mut self) {
        if self.state.snapshotting_mode {
            self.end_call_snapshotting_function();
        }
    }

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

    pub(crate) fn enter_operator_authorized_oplog_processor_invocation(
        &mut self,
    ) -> OperatorAuthorizedOplogProcessorInvocationGuard {
        OperatorAuthorizedOplogProcessorInvocationGuard::enter(
            &self.state.operator_authorized_oplog_processor_invocation,
        )
    }

    pub(crate) fn operator_authorizes_current_invocation(&self) -> bool {
        self.state
            .operator_authorized_oplog_processor_invocation
            .load(Ordering::Acquire)
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

    /// Whether some task currently holds an open replay-cursor transaction. See
    /// [`ReplayState::has_open_cursor_transaction`]; used by the invocation completion path to
    /// keep the store's event loop alive until no store-spawned task holds the cursor lock.
    pub fn has_open_replay_cursor_transaction(&self) -> bool {
        self.state.replay_state.has_open_cursor_transaction()
    }

    pub async fn process_pending_replay_events(&mut self) -> Result<(), WorkerExecutorError> {
        loop {
            let boundary_guard = self
                .state
                .card_event_boundary_lock
                .clone()
                .lock_owned()
                .await;
            self.process_pending_replay_events_locked().await?;
            let retries = permissions::prepare_pending_source_card_transfers(self).await?;
            if retries.is_empty() {
                return Ok(());
            }
            drop(boundary_guard);
            permissions::complete_pending_source_card_transfers(self, retries).await?;
        }
    }

    async fn process_pending_replay_events_locked(&mut self) -> Result<(), WorkerExecutorError> {
        let replay_events = self.state.replay_state.take_new_replay_events();
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
                ReplayEvent::InvocationWalletPinned { wallet_pin } => {
                    debug!(
                        generation = wallet_pin.wallet_token.generation,
                        card_count = wallet_pin.pinned_card_ids.len(),
                        "Restoring the replayed invocation wallet pin"
                    );
                    if apply_invocation_wallet_pin(
                        &mut self.state.agent_wallet_cards,
                        self.state.wallet_id_hash,
                        &mut self.state.wallet_generation,
                        wallet_pin,
                    )? {
                        self.rederive_agent_effective_surface_from_wallet();
                    }
                }
                ReplayEvent::CardInstalled {
                    card,
                    wallet_generation,
                } => {
                    let card_id = card.card_id();
                    debug!(card_id = %card_id, "Applying replayed card installation");
                    if add_wallet_card(
                        &mut self.state.agent_wallet_cards,
                        &mut self.state.wallet_generation,
                        card,
                    )? {
                        self.rederive_agent_effective_surface_from_wallet();
                    }
                    adopt_recorded_wallet_generation(
                        &mut self.state.wallet_generation,
                        wallet_generation,
                    )?;
                }
                ReplayEvent::CardDerived {
                    card,
                    wallet_generation,
                } => {
                    let card_id = card.card_id();
                    debug!(card_id = %card_id, "Applying replayed card derivation");
                    adopt_recorded_wallet_generation(
                        &mut self.state.wallet_generation,
                        wallet_generation,
                    )?;
                }
                ReplayEvent::CardTransferStarted {
                    card_id,
                    source_holder,
                    source_wallet_generation,
                    ..
                } => {
                    if source_holder.as_ref().is_none_or(|source_holder| {
                        card_holder_is_agent(source_holder, &self.owned_agent_id.agent_id)
                    }) {
                        if transfer_started_removes_source_membership(
                            self.state.agent_wallet_cards.get(&card_id),
                            &source_holder,
                            &self.owned_agent_id.agent_id,
                        ) {
                            debug!(card_id = %card_id, "Applying replayed card transfer start");
                            if remove_wallet_card(
                                &mut self.state.agent_wallet_cards,
                                &mut self.state.wallet_generation,
                                card_id,
                            )? {
                                self.rederive_agent_effective_surface_from_wallet();
                            }
                        }
                        adopt_recorded_wallet_generation(
                            &mut self.state.wallet_generation,
                            source_wallet_generation,
                        )?;
                    }
                }
                ReplayEvent::CardTransferred {
                    target_holder,
                    card,
                    target_wallet_generation,
                    ..
                } => {
                    if card_holder_is_agent(&target_holder, &self.owned_agent_id.agent_id) {
                        let card_id = card.card_id();
                        debug!(card_id = %card_id, "Applying replayed card transfer completion");
                        if add_wallet_card(
                            &mut self.state.agent_wallet_cards,
                            &mut self.state.wallet_generation,
                            card,
                        )? {
                            self.rederive_agent_effective_surface_from_wallet();
                        }
                        adopt_recorded_wallet_generation(
                            &mut self.state.wallet_generation,
                            target_wallet_generation,
                        )?;
                    }
                }
                ReplayEvent::CardTransferConfirmed { transfer_id, .. } => {
                    debug!(%transfer_id, "Applying replayed card transfer receipt");
                }
                ReplayEvent::CardRevokedCascade {
                    card_ids,
                    local_wallet_generation,
                } => {
                    debug!(
                        count = card_ids.len(),
                        "Applying replayed card revocation cascade"
                    );
                    let wallet_changed = remove_wallet_cards(
                        &mut self.state.agent_wallet_cards,
                        &mut self.state.wallet_generation,
                        &card_ids,
                    )?;
                    let scope_changed = self.clear_invocation_scope_if_roots_include(&card_ids);
                    if wallet_changed || scope_changed {
                        self.rederive_agent_effective_surface_from_wallet();
                    }
                    adopt_recorded_wallet_generation(
                        &mut self.state.wallet_generation,
                        local_wallet_generation,
                    )?;
                }
                ReplayEvent::CardRevoked {
                    card_id,
                    wallet_generation,
                } => {
                    debug!(card_id = %card_id, "Applying replayed card revocation");
                    self.apply_card_revoked(card_id, OplogIndex::NONE, false)
                        .await?;
                    adopt_recorded_wallet_generation(
                        &mut self.state.wallet_generation,
                        wallet_generation,
                    )?;
                }
                ReplayEvent::CardExpired {
                    card_id,
                    wallet_generation,
                } => {
                    debug!(card_id = %card_id, "Applying replayed card expiry");
                    self.apply_card_revoked(card_id, OplogIndex::NONE, false)
                        .await?;
                    adopt_recorded_wallet_generation(
                        &mut self.state.wallet_generation,
                        wallet_generation,
                    )?;
                }
                ReplayEvent::ReplayFinished => {
                    debug!("Replaying oplog finished");
                    self.linear_memory.switch_to_live();
                    let pending_update = self.state.pending_update.lock().await.take();
                    if let Some(pending_update) = pending_update {
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

                    self.check_post_replay_wallet_liveness().await?;
                }
            }
        }

        Ok(())
    }

    async fn check_post_replay_wallet_liveness(&mut self) -> Result<(), WorkerExecutorError> {
        let interested_card_ids = self.interested_card_ids();
        self.refresh_card_interest().await;

        if interested_card_ids.is_empty() || !self.state.card_interest_index.authority_is_open() {
            return Ok(());
        }

        let card_states = self
            .state
            .card_service
            .check_cards(interested_card_ids)
            .await?;
        self.state.invocation_scope_root_cards = live_scope_root_cards_from_states(
            self.state.invocation_scope_card.as_ref(),
            &card_states,
        )?;

        let revoked_card_ids = card_states
            .into_iter()
            .filter_map(|(card_id, state)| (state == CardState::Revoked).then_some(card_id))
            .collect::<Vec<_>>();
        self.public_state
            .worker()
            .queue_card_revocations_locked(&revoked_card_ids)
            .await;

        Ok(())
    }

    pub async fn update_state_to_new_phantom_id(
        &mut self,
        new_phantom_id: Uuid,
    ) -> Result<(), WorkerExecutorError> {
        self.state.current_phantom_id = Some(new_phantom_id);
        Ok(())
    }

    async fn update_state_to_new_component_revision(
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

        let updated_agent_state = if let Some(agent_id) = self.parsed_agent_id() {
            let agent_type = new_metadata
                .metadata
                .find_agent_type_by_name_ref(&agent_id.agent_type)
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
            )?;

            validate_agent_config(&updated_agent_config, agent_type)?;

            let initial_card =
                agent_initial_card_from_component_metadata(&new_metadata, &agent_id)?;
            let initial_wallet_cards = BTreeMap::from([(initial_card.card_id(), initial_card)]);
            Some((updated_agent_config, initial_wallet_cards))
        } else {
            None
        };

        update_initial_files(
            &self.filesystem_generation_handle,
            Arc::clone(&self.state.file_loader),
            self.owned_agent_id.environment_id,
            new_agent_type_provision_configs
                .as_ref()
                .map(|c| c.files.clone())
                .unwrap_or_default(),
        )
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;

        self.state.component_metadata = new_metadata;

        if let Some((updated_agent_config, initial_wallet_cards)) = updated_agent_state {
            self.state.agent_config = updated_agent_config;
            self.state.cached_agent_config_retry_policies = None;
            replace_wallet_cards(
                &mut self.state.agent_wallet_cards,
                &mut self.state.wallet_generation,
                initial_wallet_cards,
            )?;
            self.rederive_agent_effective_surface_from_wallet();
        };

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
        {
            let execution_status = self.execution_status.read().unwrap();
            if let ExecutionStatus::Interrupting { interrupt_kind, .. } = &*execution_status {
                return Some(*interrupt_kind);
            }
        }
        // An exceeded invocation or tail-work deadline surfaces as a synthetic interrupt so work
        // traps at the next epoch check. The corresponding invocation boundary converts the
        // unwind into the appropriate timeout failure.
        if self
            .state
            .invocation_deadline_exceeded
            .load(Ordering::Acquire)
            || self
                .state
                .tail_work_deadline_exceeded
                .load(Ordering::Acquire)
        {
            return Some(InterruptKind::Interrupt(Timestamp::now_utc()));
        }
        None
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
        if !self.state.durability_is_suppressed() {
            let stack = self.get_current_invocation_context().await;

            let scope_card = match &invocation {
                AgentInvocation::AgentMethod { scope_card, .. } => scope_card.clone(),
                _ => None,
            };
            let scope_root_cards = if let Some(scope_card) = &scope_card {
                crate::services::card::validate_scope_card(
                    self.state.card_service.as_ref(),
                    scope_card,
                )
                .await?
            } else {
                Vec::new()
            };
            self.install_invocation_scope_card(scope_card.clone(), scope_root_cards)
                .await;

            let input = match &invocation {
                AgentInvocation::AgentInitialization { input, .. }
                | AgentInvocation::AgentMethod { input, .. } => Some(input),
                _ => None,
            };
            if let Some(input) = input
                && !self.secret_holds_allowed_for_value(input).await?
            {
                self.clear_invocation_scope_card().await;
                return Err(WorkerExecutorError::permission_denied("permission denied"));
            }

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

            let (start_index, _) = self
                .public_state
                .worker()
                .oplog()
                .add_agent_invocation_started_with_index(
                    invocation,
                    InvocationWalletPin {
                        wallet_token: WalletVersionToken {
                            wallet_id_hash: self.state.wallet_id_hash,
                            generation: self.state.wallet_generation,
                        },
                        pinned_card_ids: self.state.agent_wallet_cards.keys().copied().collect(),
                        scope_card_id: scope_card.map(|card| card.scope_card_id),
                    },
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "could not encode agent invocation on {}: {err}",
                        self.agent_id()
                    )
                });
            self.primary_invocation_start_index = Some(start_index);

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
        }
        Ok(())
    }

    async fn on_agent_invocation_finished(&mut self) {
        self.clear_invocation_scope_card().await;
    }

    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> RetryDecision {
        let current_idempotency_key = self.get_current_idempotency_key().await;

        if self.state.is_live()
            && !self.state.snapshotting_mode
            && let Err(err) = concurrent::drain_queued_dropped_call_events(self).await
        {
            error!("failed to drain dropped durable calls before invocation failure entry: {err}");
            return RetryDecision::None;
        }

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

        let latest_status_before = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        let (decision, retry_policy_state) = self
            .get_recovery_decision_on_trap_with_semantic(
                &latest_status_before.current_retry_state,
                trap_type,
                full_function_name,
            )
            .await;

        let permission_denial_persisted = if let (
            Some(idempotency_key),
            TrapType::Error {
                error: AgentError::PermissionDenied(error),
                retry_from,
                atomic_region_had_side_effects,
                ..
            },
        ) = (&current_idempotency_key, trap_type)
        {
            self.state
                .oplog
                .add_pair(
                    OplogEntry::cancel_pending_invocation(idempotency_key.clone()),
                    Box::new({
                        let error = error.clone();
                        let retry_policy_state = retry_policy_state.clone();
                        let retry_from = *retry_from;
                        let inside_atomic_region = *atomic_region_had_side_effects;
                        move |_| {
                            OplogEntry::error(
                                AgentError::PermissionDenied(error),
                                retry_from,
                                inside_atomic_region,
                                retry_policy_state,
                            )
                        }
                    }),
                )
                .await;
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;
            true
        } else {
            false
        };

        let oplog_entry = match trap_type {
            TrapType::Interrupt(InterruptKind::Interrupt(_)) => Some(OplogEntry::interrupted()),
            TrapType::Interrupt(InterruptKind::Suspend(_)) => Some(OplogEntry::suspend()),
            TrapType::Interrupt(InterruptKind::Jump) => None,
            TrapType::Interrupt(InterruptKind::Restart) => None,
            TrapType::Exit => Some(OplogEntry::exited()),
            TrapType::Error {
                error: AgentError::PermissionDenied(_),
                ..
            } if permission_denial_persisted => None,
            TrapType::Error { .. } if trap_type.is_invocation_rejection() => {
                current_idempotency_key.map(OplogEntry::cancel_pending_invocation)
            }
            TrapType::Error {
                error,
                retry_from,
                atomic_region_had_side_effects,
                ..
            } => Some(OplogEntry::error(
                error.clone(),
                *retry_from,
                *atomic_region_had_side_effects,
                retry_policy_state,
            )),
        };

        if let Some(entry) = oplog_entry {
            self.public_state.worker().add_and_commit_oplog(entry).await;
        };

        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;

        let giving_up = trap_type.is_invocation_rejection()
            || matches!(
                latest_status.status,
                AgentStatus::Interrupted | AgentStatus::Exited
            )
            || decision == RetryDecision::None;

        if giving_up {
            // Terminal worker failures also fail queued invocations. A rejected request completes
            // only its own idempotency key so later queued work remains available.
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
            "Recovery decision for {trap_type:?} with {:?} retries: {:?}",
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
        if is_live && !self.state.snapshotting_mode {
            concurrent::drain_queued_dropped_call_events(self)
                .await
                .map_err(|err| err.source)?;
        }
        if !self.state.active_custom_invocations.is_empty()
            || !self.state.custom_invocation_scopes.is_empty()
        {
            self.cleanup_custom_durability_state();
            return Err(WorkerExecutorError::runtime(
                "agent invocation returned while a custom durable invocation ownership scope was still open",
            ));
        }

        if is_live {
            if !self.state.durability_is_suppressed() {
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
            // Mirror the live-path drain: events enqueued from synchronous drops during replay
            // (e.g. `DropEvent::FinishSpan` for a p3 HTTP response dropped unconsumed) must consume
            // their positional entries (recorded by the live drain at this same point) before the
            // `AgentInvocationFinished` entry is read.
            concurrent::drain_queued_dropped_call_events(self)
                .await
                .map_err(|err| err.source)?;

            let response = self
                .state
                .replay_state
                .get_oplog_entry_agent_invocation_finished()
                .await?;
            if let Some(recorded_result) = response
                && !recorded_result.replay_equivalent(&output.result)
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!(
                        "{full_function_name} => {:?}",
                        recorded_result.redacted_debug()
                    ),
                    format!(
                        "{full_function_name} => {:?}",
                        output.result.redacted_debug()
                    ),
                ));
            }
        }
        debug!("Function {full_function_name} finished");

        Ok(())
    }

    async fn get_current_retry_point(&self) -> OplogIndex {
        self.state.effective_retry_point()
    }

    fn current_in_atomic_region(&self) -> bool {
        !self.state.active_atomic_regions.is_empty()
    }

    fn current_atomic_region_had_side_effects(&self) -> bool {
        self.state.outermost_atomic_region_has_side_effects()
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
    fn snapshot_boundary_blocker(&self) -> Option<SnapshotBoundaryBlocker> {
        self.state.snapshot_boundary_blocker()
    }

    fn begin_call_snapshotting_function(&mut self) {
        if self.state.snapshotting_mode {
            warn!(
                "begin_call_snapshotting_function called while snapshotting is already active; \
                 leaving snapshotting active"
            );
            return;
        }
        self.state.snapshotting_mode = true;
    }

    fn end_call_snapshotting_function(&mut self) {
        if self.state.snapshotting_mode {
            self.state.snapshotting_mode = false;
        } else {
            warn!(
                "end_call_snapshotting_function called without a matching begin_call_snapshotting_function"
            );
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
        let worker = self.public_state.worker();
        worker
            .persist_successful_update(
                &self.linear_memory,
                target_revision,
                new_component_size,
                new_active_plugins,
            )
            .await;
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
                    parent_start_index: self.entity_parent_start_index(),
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
                .add_to_oplog(OplogEntry::finish_span(
                    self.entity_parent_start_index(),
                    span_id.clone(),
                ))
                .await;
        } else if !self.is_live() {
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
                    self.entity_parent_start_index(),
                    span_id.clone(),
                    key.to_string(),
                    value,
                ))
                .await;
        } else if !self.is_live() {
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
        store: &mut Store<Ctx>,
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
                .as_context()
                .data()
                .durable_ctx()
                .state
                .replay_state
                .set_replay_target(new_target)
                .await?;
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
                        oplog_index,
                        idempotency_key,
                        invocation_payload,
                        invocation_context,
                        wallet_pin,
                    })) => {
                        let agent_invocation = AgentInvocation::from_parts(
                            idempotency_key.clone(),
                            invocation_payload,
                            invocation_context.clone(),
                        );
                        let scope_card = match &agent_invocation {
                            AgentInvocation::AgentMethod { scope_card, .. } => scope_card.clone(),
                            _ => None,
                        };
                        let recorded_scope_card_id = wallet_pin.and_then(|pin| pin.scope_card_id);
                        let payload_scope_card_id =
                            scope_card.as_ref().map(|card| card.scope_card_id);
                        if payload_scope_card_id != recorded_scope_card_id {
                            break Err(WorkerExecutorError::unexpected_oplog_entry(
                                "matching invocation scope-card payload and wallet pin",
                                format!(
                                    "payload scope-card ID {payload_scope_card_id:?}, recorded scope-card ID {recorded_scope_card_id:?}"
                                ),
                            ));
                        }

                        let component_metadata = store
                            .as_context()
                            .data()
                            .component_metadata()
                            .metadata
                            .clone();

                        let worker = store.as_context().data().get_public_state().worker();
                        let agent_invocation = worker
                            .rehydrate_durable_streaming_invocation(agent_invocation)
                            .await?;
                        let agent_id = store.as_context().data().parsed_agent_id();
                        let lowered = lower_invocation(
                            agent_invocation,
                            &component_metadata,
                            agent_id.as_ref(),
                        )?;
                        let full_function_name = lowered.display_name.clone();

                        let mut store_context = store.as_context_mut();
                        let durable_ctx = store_context.data_mut().durable_ctx_mut();
                        durable_ctx
                            .install_invocation_scope_card(scope_card, Vec::new())
                            .await;
                        if let Err(error) = durable_ctx.process_pending_replay_events().await {
                            durable_ctx.clear_invocation_scope_card().await;
                            break Err(error);
                        }

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
                            .set_current_idempotency_key(idempotency_key.clone())
                            .await;

                        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_invocation_context(invocation_context)
                            .await?;
                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .primary_invocation_start_index = Some(oplog_index);
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
                                result: mut invocation_result,
                                consumed_fuel,
                            }) => {
                                if let AgentInvocationResult::AgentMethod { output } =
                                    &mut invocation_result
                                {
                                    let (graph, root, component_revision) = {
                                        let component = store.data().component_metadata();
                                        let agent_id = store.data().parsed_agent_id();
                                        let agent_type = agent_id
                                            .as_ref()
                                            .and_then(|agent_id| {
                                                component
                                                    .metadata
                                                    .find_agent_type_by_name_ref(
                                                        &agent_id.agent_type,
                                                    )
                                            })
                                            .ok_or_else(|| {
                                                WorkerExecutorError::runtime(
                                                    "durable invocation result schema is unavailable",
                                                )
                                            })?;
                                        let method = agent_type
                                            .methods
                                            .iter()
                                            .find(|method| method.name == full_function_name)
                                            .ok_or_else(|| {
                                                WorkerExecutorError::runtime(
                                                    "durable invocation result method schema is unavailable",
                                                )
                                            })?;
                                        (
                                            agent_type.schema.clone(),
                                            method.output_schema.schema().cloned().unwrap_or_else(
                                                || {
                                                    golem_common::schema::SchemaType::tuple(
                                                        Vec::new(),
                                                    )
                                                },
                                            ),
                                            component.revision,
                                        )
                                    };
                                    let worker = worker.clone();
                                    let result_value = output.clone();
                                    let replay_idempotency_key = idempotency_key.clone();
                                    *output = store
                                        .run_concurrent(async move |_accessor| {
                                            worker
                                                .materialize_durable_streaming_result(
                                                    &replay_idempotency_key,
                                                    result_value,
                                                    &graph,
                                                    &root,
                                                    component_revision,
                                                )
                                                .await
                                        })
                                        .await
                                        .map_err(|error| {
                                            WorkerExecutorError::runtime(error.to_string())
                                        })??;
                                }
                                let component_revision =
                                    store.as_context().data().component_metadata().revision;
                                let mut output = AgentInvocationOutput {
                                    result: invocation_result,
                                    consumed_fuel: Some(consumed_fuel),
                                    invocation_status: None,
                                    component_revision: Some(component_revision),
                                    agent_id: None,
                                    idempotency_key: None,
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
                                // An invocation that was interrupted by a crash reaches its end
                                // here, in live mode, instead of in the invocation loop, so its
                                // durable Stream Session must be finished here as well;
                                // otherwise resumed session clients never observe completion.
                                if store.as_context().data().durable_ctx().is_live()
                                    && let Err(error) = worker
                                        .complete_durable_streaming_session(&idempotency_key)
                                        .await
                                {
                                    error!(%error, "Failed to complete durable streaming session");
                                    break Err(error);
                                }
                                number_of_replayed_functions += 1;
                                continue;
                            }
                            _ => {
                                let details = format!("{invoke_result:?}");
                                let trap_type = match invoke_result {
                                    Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
                                    Err(error) => {
                                        Some(TrapType::from_worker_executor_error::<Ctx>(
                                            error,
                                            OplogIndex::INITIAL,
                                            false,
                                            false,
                                            store.as_context().data().agent_mode(),
                                        ))
                                    }
                                };
                                let decision = match trap_type {
                                    // A recorded invocation that fails while its entries are still
                                    // being replayed after an automatic snapshot load most likely
                                    // diverged from the recorded execution because the guest state
                                    // restored from the snapshot differs from the original one. The
                                    // recorded oplog is authoritative, so instead of committing the
                                    // failure the snapshot is abandoned and the worker replays from
                                    // the beginning, which reproduces the recorded outcome.
                                    Some(TrapType::Error { error, .. })
                                        if store
                                            .as_context()
                                            .data()
                                            .durable_ctx()
                                            .state
                                            .replaying_automatic_snapshot_tail
                                            && !store
                                                .as_context()
                                                .data()
                                                .durable_ctx()
                                                .is_live() =>
                                    {
                                        Some(Self::abandon_diverged_automatic_snapshot(
                                            store,
                                            &full_function_name,
                                            &error,
                                        ))
                                    }
                                    Some(trap_type) => {
                                        let decision = store
                                            .as_context_mut()
                                            .data_mut()
                                            .on_invocation_failure(&full_function_name, &trap_type)
                                            .await;

                                        if decision == RetryDecision::None {
                                            // Like the invocation loop, permanently fail the
                                            // durable Stream Session of an invocation that was
                                            // interrupted by a crash and cannot be retried.
                                            if store.as_context().data().durable_ctx().is_live() {
                                                let _ = worker
                                                    .fail_durable_streaming_session(
                                                        &idempotency_key,
                                                        details,
                                                    )
                                                    .await;
                                            }
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

        if matches!(resume_result, Ok(None)) {
            store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .replaying_automatic_snapshot_tail = false;
        }

        resume_result
    }

    async fn prepare_instance(
        agent_id: &AgentId,
        instance: &Instance,
        store: &mut Store<Ctx>,
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
                            .store(true, Ordering::Release);
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

fn card_holder_is_agent(holder: &CardHolder, agent_id: &AgentId) -> bool {
    matches!(holder, CardHolder::Agent(holder) if holder.agent_id == *agent_id)
}

pub(crate) struct OperatorAuthorizedOplogProcessorInvocationGuard {
    active: Arc<AtomicBool>,
}

impl OperatorAuthorizedOplogProcessorInvocationGuard {
    fn enter(active: &Arc<AtomicBool>) -> Self {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .expect("operator-authorized oplog-processor invocation mode cannot be nested");
        Self {
            active: active.clone(),
        }
    }
}

impl Drop for OperatorAuthorizedOplogProcessorInvocationGuard {
    fn drop(&mut self) {
        let was_active = self.active.swap(false, Ordering::AcqRel);
        debug_assert!(
            was_active,
            "operator-authorized oplog-processor invocation mode was not active"
        );
    }
}

pub(crate) fn authorize_effective_surface(
    surface: &golem_common::model::card::EffectiveSurface,
    targets: &[PermissionTarget],
) -> Result<(), AuthorizationError> {
    for target in targets {
        let allowed = surface.authorize(target).map_err(|error| {
            AuthorizationError::PermissionEvaluationFailed {
                target: Box::new(target.clone()),
                error,
            }
        })?;
        if !allowed {
            return Err(AuthorizationError::PermissionNotAllowed(Box::new(
                target.clone(),
            )));
        }
    }
    Ok(())
}

pub(crate) fn authority_snapshot_is_current_at(
    initialized: bool,
    authority_open: bool,
    processed_generation: u64,
    published_generation: u64,
    next_expiration: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    initialized
        && authority_open
        && processed_generation == published_generation
        && next_expiration.is_none_or(|expiration| now < expiration)
}

fn assert_live_authorization(is_live: bool) {
    assert!(
        is_live,
        "live permission authorization must not run during replay"
    );
}

pub(crate) fn record_permission_decisions(targets: &[PermissionTarget], allowed: bool) {
    if allowed {
        for target in targets {
            crate::metrics::wasm::record_agent_permission_authorization(target.class_name(), true);
        }
    } else {
        // Multi-target operations are all-or-nothing. The authorization API intentionally
        // exposes only a low-cardinality class metric, never the denied resource.
        let permission_class = targets
            .first()
            .map(PermissionTarget::class_name)
            .unwrap_or("unknown");
        crate::metrics::wasm::record_agent_permission_authorization(permission_class, false);
    }
}

fn clear_invocation_scope_state(
    scope_card: &mut Option<ScopeCard>,
    scope_handles: &mut HashMap<u32, Vec<CardId>>,
) -> (bool, HashSet<u32>) {
    let scope_changed = scope_card.take().is_some();
    let handles = std::mem::take(scope_handles).into_keys().collect();
    (scope_changed, handles)
}

fn remove_invocation_scope_for_revoked_roots(
    scope_card: &mut Option<ScopeCard>,
    scope_handles: &mut HashMap<u32, Vec<CardId>>,
    revoked_card_ids: &[CardId],
) -> (bool, HashSet<u32>) {
    let affected = scope_card.as_ref().is_some_and(|scope_card| {
        scope_card
            .root_card_ids
            .iter()
            .any(|root_id| revoked_card_ids.contains(root_id))
    });
    if affected {
        *scope_card = None;
    }
    let handles = scope_handles
        .iter()
        .filter_map(|(rep, root_card_ids)| {
            root_card_ids
                .iter()
                .any(|root_id| revoked_card_ids.contains(root_id))
                .then_some(*rep)
        })
        .collect::<HashSet<_>>();
    for rep in &handles {
        scope_handles.remove(rep);
    }
    (affected, handles)
}

fn live_scope_root_cards_from_states(
    scope_card: Option<&ScopeCard>,
    card_states: &HashMap<CardId, CardState>,
) -> Result<BTreeMap<CardId, StoredCard>, WorkerExecutorError> {
    let Some(scope_card) = scope_card else {
        return Ok(BTreeMap::new());
    };

    let mut root_cards = BTreeMap::new();
    for card_id in &scope_card.root_card_ids {
        match card_states.get(card_id) {
            Some(CardState::Live(card)) if card.card_id() == *card_id => {
                root_cards.insert(*card_id, card.as_ref().clone());
            }
            Some(CardState::Revoked) => {}
            Some(CardState::Live(_)) | Some(CardState::Unknown) | None => {
                return Err(WorkerExecutorError::runtime(format!(
                    "scope-card root {card_id} could not be re-validated after replay"
                )));
            }
        }
    }
    Ok(root_cards)
}

fn transfer_started_removes_source_membership(
    source_card: Option<&StoredCard>,
    source_holder: &Option<CardHolder>,
    agent_id: &AgentId,
) -> bool {
    matches!(source_card, Some(StoredCard::Concrete(_)))
        && source_holder
            .as_ref()
            .is_none_or(|holder| card_holder_is_agent(holder, agent_id))
}

fn next_drainable_card_events(
    pending_events: Vec<PendingCardEventRef>,
) -> Vec<PendingCardEventRef> {
    let mut result = Vec::new();
    let mut collecting_revocations = false;

    for event in pending_events {
        match &event.event {
            QueuedCardEvent::TransferStarted(_) => continue,
            QueuedCardEvent::Install(_) | QueuedCardEvent::TransferReceived(_) => {
                if result.is_empty() {
                    result.push(event);
                }
                break;
            }
            QueuedCardEvent::Revoke(_) => {
                if result.is_empty() {
                    collecting_revocations = true;
                }
                if collecting_revocations {
                    result.push(event);
                } else {
                    break;
                }
            }
        }
    }

    result
}

fn add_wallet_card(
    wallet: &mut BTreeMap<CardId, StoredCard>,
    generation: &mut u64,
    card: StoredCard,
) -> Result<bool, WorkerExecutorError> {
    let card_id = card.card_id();
    if let Some(existing) = wallet.get(&card_id) {
        return if existing == &card {
            Ok(false)
        } else {
            Err(WorkerExecutorError::runtime(format!(
                "wallet card {card_id} conflicts with its existing payload"
            )))
        };
    }
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;
    wallet.insert(card_id, card);
    Ok(true)
}

fn remove_wallet_card(
    wallet: &mut BTreeMap<CardId, StoredCard>,
    generation: &mut u64,
    card_id: CardId,
) -> Result<bool, WorkerExecutorError> {
    if !wallet.contains_key(&card_id) {
        return Ok(false);
    }
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;
    wallet.remove(&card_id);
    Ok(true)
}

fn remove_wallet_cards(
    wallet: &mut BTreeMap<CardId, StoredCard>,
    generation: &mut u64,
    card_ids: &[CardId],
) -> Result<bool, WorkerExecutorError> {
    if !card_ids.iter().any(|card_id| wallet.contains_key(card_id)) {
        return Ok(false);
    }
    let next_generation = generation
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;

    for card_id in card_ids {
        wallet.remove(card_id);
    }
    *generation = next_generation;
    Ok(true)
}

fn expired_wallet_card_ids_at(
    wallet: &BTreeMap<CardId, StoredCard>,
    now: DateTime<Utc>,
) -> Vec<CardId> {
    wallet
        .iter()
        .filter_map(|(card_id, card)| {
            card.expires_at()
                .filter(|expires_at| *expires_at <= now)
                .map(|_| *card_id)
        })
        .collect()
}

fn apply_invocation_wallet_pin(
    wallet: &mut BTreeMap<CardId, StoredCard>,
    wallet_id_hash: [u8; 32],
    generation: &mut u64,
    wallet_pin: InvocationWalletPin,
) -> Result<bool, WorkerExecutorError> {
    if wallet_pin.wallet_token.wallet_id_hash != wallet_id_hash {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "invocation wallet pin for the local wallet",
            "wallet identity hash does not match the replaying agent",
        ));
    }
    if wallet_pin
        .pinned_card_ids
        .windows(2)
        .any(|card_ids| card_ids[0] >= card_ids[1])
    {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "canonically ordered invocation wallet pin",
            "pinned card ids are not strictly increasing",
        ));
    }
    if wallet_pin.wallet_token.generation < *generation {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "non-decreasing invocation wallet generation",
            format!(
                "pinned generation {} is behind replayed generation {}",
                wallet_pin.wallet_token.generation, *generation
            ),
        ));
    }

    let mut pinned_wallet = BTreeMap::new();
    for card_id in wallet_pin.pinned_card_ids {
        let card = wallet.get(&card_id).cloned().ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "invocation wallet pin backed by replayed card definitions",
                format!("pinned card {card_id} is missing from the replayed wallet"),
            )
        })?;
        pinned_wallet.insert(card_id, card);
    }

    let membership_changed = wallet.keys().ne(pinned_wallet.keys());
    *wallet = pinned_wallet;
    *generation = wallet_pin.wallet_token.generation;
    Ok(membership_changed)
}

fn adopt_recorded_wallet_generation(
    generation: &mut u64,
    recorded_generation: Option<u64>,
) -> Result<(), WorkerExecutorError> {
    if let Some(recorded_generation) = recorded_generation {
        if recorded_generation < *generation {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "non-decreasing wallet generation",
                format!(
                    "recorded generation {recorded_generation} is behind replayed generation {}",
                    *generation
                ),
            ));
        }
        *generation = recorded_generation;
    }
    Ok(())
}

fn replace_wallet_cards(
    wallet: &mut BTreeMap<CardId, StoredCard>,
    generation: &mut u64,
    replacement: BTreeMap<CardId, StoredCard>,
) -> Result<bool, WorkerExecutorError> {
    let removed_ids = wallet
        .keys()
        .filter(|card_id| !replacement.contains_key(card_id))
        .copied()
        .collect::<Vec<_>>();
    let added = replacement
        .values()
        .filter(|card| !wallet.contains_key(&card.card_id()))
        .count();
    let change_count = removed_ids
        .len()
        .checked_add(added)
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;
    if change_count == 0 {
        return Ok(false);
    }
    let next_generation = generation
        .checked_add(change_count)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;

    for card_id in removed_ids {
        wallet.remove(&card_id);
    }
    for card in replacement.into_values() {
        let card_id = card.card_id();
        wallet.entry(card_id).or_insert(card);
    }
    *generation = next_generation;
    Ok(true)
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
    use bytes::Bytes;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::card::owner::{AgentOwnerPattern, EmptyOwnerPattern};
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{
        AgentCardHolder, AgentClass, AgentPermissionMonomorphizationContext, AgentResourcePattern,
        AgentVerb, Card, ClassPermissionPattern, ClassPermissionTarget, GrantSurface,
        NetworkResourcePattern, NetworkVerb, PermissionPattern, PermissionTarget, PolymorphicCard,
        PortPattern,
    };
    use golem_common::model::component::ComponentName;
    use golem_common::model::environment::EnvironmentName;
    use golem_common::model::{PendingInvocationRef, PendingUpdateKind, PendingUpdateRef};
    use http_body::Frame;
    use std::collections::HashSet;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};
    use test_r::test;

    #[test]
    fn operator_authorized_invocation_guard_clears_mode_when_dropped() {
        let active = Arc::new(AtomicBool::new(false));
        let guard = OperatorAuthorizedOplogProcessorInvocationGuard::enter(&active);
        assert!(active.load(Ordering::Acquire));
        drop(guard);
        assert!(!active.load(Ordering::Acquire));
    }

    fn network_permission(host: &str) -> PermissionTarget {
        PermissionTarget::Network(ClassPermissionTarget {
            verb: Some(NetworkVerb::Connect),
            owner: EmptyOwnerPattern,
            resource: NetworkResourcePattern::host_port(host.to_string(), PortPattern::single(443)),
        })
    }

    #[test]
    fn effective_surface_authorization_preserves_algebra_and_batch_atomicity() {
        let first = network_permission("first.example.com");
        let second = network_permission("second.example.com");
        let unrelated = network_permission("unrelated.example.com");
        let lower_or_upper_and = golem_common::model::card::EffectiveSurface {
            source_card_ids: Vec::new(),
            lower: vec![
                GrantSurface {
                    positive: vec![first.clone()],
                    negative: Vec::new(),
                },
                GrantSurface {
                    positive: vec![second.clone()],
                    negative: Vec::new(),
                },
            ],
            upper: vec![
                GrantSurface {
                    positive: vec![first.clone(), second.clone()],
                    negative: Vec::new(),
                },
                GrantSurface {
                    positive: vec![first.clone(), second.clone(), unrelated],
                    negative: Vec::new(),
                },
            ],
        };

        assert!(
            authorize_effective_surface(&lower_or_upper_and, &[first.clone(), second.clone()])
                .is_ok()
        );

        let mut denied = lower_or_upper_and;
        denied.upper[1].negative.push(second.clone());
        assert!(authorize_effective_surface(&denied, std::slice::from_ref(&first)).is_ok());
        let error = authorize_effective_surface(&denied, &[first, second]);
        assert!(matches!(
            error,
            Err(AuthorizationError::PermissionNotAllowed(_))
        ));
    }

    #[test]
    fn authority_fast_path_requires_an_open_current_unexpired_snapshot() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let later = now + chrono::Duration::nanoseconds(1);

        assert!(authority_snapshot_is_current_at(
            true,
            true,
            7,
            7,
            Some(later),
            now,
        ));
        assert!(!authority_snapshot_is_current_at(
            false,
            true,
            7,
            7,
            Some(later),
            now,
        ));
        assert!(!authority_snapshot_is_current_at(
            true,
            false,
            7,
            7,
            Some(later),
            now,
        ));
        assert!(!authority_snapshot_is_current_at(
            true,
            true,
            6,
            7,
            Some(later),
            now,
        ));
        assert!(!authority_snapshot_is_current_at(
            true,
            true,
            7,
            7,
            Some(now),
            now,
        ));
    }

    #[test]
    fn generation_recheck_invalidates_in_progress_allow_and_deny() {
        let allowed = network_permission("allowed.example.com");
        let denied = network_permission("denied.example.com");
        let surface = golem_common::model::card::EffectiveSurface {
            source_card_ids: Vec::new(),
            lower: vec![GrantSurface {
                positive: vec![allowed.clone()],
                negative: Vec::new(),
            }],
            upper: Vec::new(),
        };
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        assert!(authorize_effective_surface(&surface, &[allowed]).is_ok());
        assert!(authorize_effective_surface(&surface, &[denied]).is_err());
        assert!(authority_snapshot_is_current_at(
            true, true, 11, 11, None, now,
        ));
        assert!(
            !authority_snapshot_is_current_at(true, true, 11, 12, None, now),
            "a newly published revocation must invalidate an in-progress allow"
        );
        assert!(
            !authority_snapshot_is_current_at(true, true, 11, 12, None, now),
            "a newly published installation must invalidate an in-progress denial"
        );
    }

    fn authorize_boundary_for_test(
        initialized: bool,
        authority_open: bool,
        processed_generation: &mut u64,
        published_generation: u64,
        expiration: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        slow_refreshes: &mut usize,
        surface: &golem_common::model::card::EffectiveSurface,
        target: &PermissionTarget,
    ) -> Result<(), AuthorizationError> {
        if authority_snapshot_is_current_at(
            initialized,
            authority_open,
            *processed_generation,
            published_generation,
            expiration,
            now,
        ) {
            return authorize_effective_surface(surface, std::slice::from_ref(target));
        }
        *slow_refreshes += 1;
        if !authority_open {
            return Err(AuthorizationError::PermissionNotAllowed(Box::new(
                target.clone(),
            )));
        }
        *processed_generation = published_generation;
        authorize_effective_surface(surface, std::slice::from_ref(target))
    }

    fn allowing_surface(target: PermissionTarget) -> golem_common::model::card::EffectiveSurface {
        golem_common::model::card::EffectiveSurface {
            source_card_ids: Vec::new(),
            lower: vec![GrantSurface {
                positive: vec![target],
                negative: Vec::new(),
            }],
            upper: Vec::new(),
        }
    }

    #[test]
    fn unchanged_authority_generation_uses_no_io_fast_path() {
        let target = network_permission("fast.example.com");
        let surface = allowing_surface(target.clone());
        let mut processed = 9;
        let mut slow_refreshes = 0;
        authorize_boundary_for_test(
            true,
            true,
            &mut processed,
            9,
            None,
            Utc::now(),
            &mut slow_refreshes,
            &surface,
            &target,
        )
        .unwrap();
        assert_eq!(slow_refreshes, 0);
    }

    #[test]
    fn authority_event_burst_causes_exactly_one_slow_refresh() {
        let target = network_permission("burst.example.com");
        let surface = allowing_surface(target.clone());
        let mut processed = 4;
        let mut slow_refreshes = 0;
        for _ in 0..2 {
            authorize_boundary_for_test(
                true,
                true,
                &mut processed,
                8,
                None,
                Utc::now(),
                &mut slow_refreshes,
                &surface,
                &target,
            )
            .unwrap();
        }
        assert_eq!(slow_refreshes, 1);
        assert_eq!(processed, 8);
    }

    #[test]
    fn closed_authority_cannot_allow() {
        let target = network_permission("closed.example.com");
        let surface = allowing_surface(target.clone());
        let mut processed = 3;
        let mut slow_refreshes = 0;
        assert!(
            authorize_boundary_for_test(
                true,
                false,
                &mut processed,
                3,
                None,
                Utc::now(),
                &mut slow_refreshes,
                &surface,
                &target,
            )
            .is_err()
        );
    }

    #[test]
    fn expiration_is_visible_at_first_due_live_boundary() {
        let target = network_permission("expired.example.com");
        let surface = allowing_surface(target.clone());
        let due = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut processed = 5;
        let mut slow_refreshes = 0;
        authorize_boundary_for_test(
            true,
            true,
            &mut processed,
            5,
            Some(due),
            due,
            &mut slow_refreshes,
            &surface,
            &target,
        )
        .unwrap();
        assert_eq!(slow_refreshes, 1, "the boundary at expires_at must refresh");
    }

    #[test]
    #[should_panic(expected = "live permission authorization must not run during replay")]
    fn replay_cannot_invoke_authorization_helper() {
        assert_live_authorization(false);
    }
    use test_r::timeout;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn aggregate_memory_growth_over_worker_limit_is_rejected() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            60,
            60,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 100, 0, 0, 0)),
            Arc::new(Mutex::new(MemoryGrant::inert(60))),
            now,
        );
        tracker.reconcile_at(60, now);

        let second_memory_growth = tracker.prepare_unshared_growth(0, 60);

        assert_eq!(
            validate_unshared_memory_growth(second_memory_growth, 100, 60, Some(100)),
            None,
            "two individually valid 60-byte memories must not bypass the 100-byte aggregate cap"
        );
    }

    #[test]
    fn snapshot_boundary_all_clear_has_no_blocker() {
        assert_eq!(SnapshotBoundaryConditions::default().blocker(), None);
    }

    #[test]
    fn snapshot_boundary_each_condition_blocks_alone() {
        let cases = [
            (
                SnapshotBoundaryConditions {
                    replaying: true,
                    ..Default::default()
                },
                SnapshotBoundaryBlocker::Replaying,
            ),
            (
                SnapshotBoundaryConditions {
                    open_atomic_region: true,
                    ..Default::default()
                },
                SnapshotBoundaryBlocker::OpenAtomicRegion,
            ),
            (
                SnapshotBoundaryConditions {
                    open_durable_scope: true,
                    ..Default::default()
                },
                SnapshotBoundaryBlocker::OpenDurableScope,
            ),
            (
                SnapshotBoundaryConditions {
                    snapshotting: true,
                    ..Default::default()
                },
                SnapshotBoundaryBlocker::Snapshotting,
            ),
            (
                SnapshotBoundaryConditions {
                    in_flight_host_call: true,
                    ..Default::default()
                },
                SnapshotBoundaryBlocker::InFlightHostCall,
            ),
        ];
        for (conditions, expected) in cases {
            assert_eq!(
                conditions.blocker(),
                Some(expected),
                "single blocking condition {conditions:?} must be reported as {expected:?}"
            );
        }
    }

    #[test]
    fn snapshot_boundary_any_condition_combination_blocks() {
        // Exhaustive truth table over the five conditions: a snapshot is admitted iff every
        // condition is clear, and the reported blocker is always one of the set conditions.
        for bits in 0u32..32 {
            let conditions = SnapshotBoundaryConditions {
                replaying: bits & 1 != 0,
                open_atomic_region: bits & 2 != 0,
                open_durable_scope: bits & 4 != 0,
                snapshotting: bits & 8 != 0,
                in_flight_host_call: bits & 16 != 0,
            };
            let blocker = conditions.blocker();
            assert_eq!(
                blocker.is_none(),
                bits == 0,
                "snapshot must be admitted iff no condition is set; conditions: {conditions:?}"
            );
            if let Some(blocker) = blocker {
                let named_condition_is_set = match blocker {
                    SnapshotBoundaryBlocker::Replaying => conditions.replaying,
                    SnapshotBoundaryBlocker::OpenAtomicRegion => conditions.open_atomic_region,
                    SnapshotBoundaryBlocker::OpenDurableScope => conditions.open_durable_scope,
                    SnapshotBoundaryBlocker::Snapshotting => conditions.snapshotting,
                    SnapshotBoundaryBlocker::InFlightHostCall => conditions.in_flight_host_call,
                };
                assert!(
                    named_condition_is_set,
                    "reported blocker {blocker:?} must name a set condition; conditions: {conditions:?}"
                );
            }
        }
    }

    #[test]
    fn checkpoint_boundary_truth_table() {
        // Exhaustive truth table over the four blocking conditions: a mid-invocation status
        // checkpoint is admitted iff every condition is clear.
        for bits in 0u32..16 {
            let replaying = bits & 1 != 0;
            let open_atomic_region = bits & 2 != 0;
            let open_durable_scope = bits & 4 != 0;
            let snapshotting = bits & 8 != 0;
            assert_eq!(
                PrivateDurableWorkerState::clean_checkpoint_boundary(
                    replaying,
                    open_atomic_region,
                    open_durable_scope,
                    snapshotting,
                ),
                bits == 0,
                "checkpoint must be admitted iff no condition is set; replaying: {replaying}, \
                 open_atomic_region: {open_atomic_region}, open_durable_scope: {open_durable_scope}, \
                 snapshotting: {snapshotting}"
            );
        }
    }

    #[test]
    fn snapshot_boundary_is_checkpoint_boundary_with_no_in_flight_host_call() {
        // The documented sync invariant between the two predicates: `blocker() == None` is
        // equivalent to `at_clean_checkpoint_boundary() && !has_in_flight_live_host_calls()`.
        for bits in 0u32..32 {
            let conditions = SnapshotBoundaryConditions {
                replaying: bits & 1 != 0,
                open_atomic_region: bits & 2 != 0,
                open_durable_scope: bits & 4 != 0,
                snapshotting: bits & 8 != 0,
                in_flight_host_call: bits & 16 != 0,
            };
            let at_checkpoint_boundary = PrivateDurableWorkerState::clean_checkpoint_boundary(
                conditions.replaying,
                conditions.open_atomic_region,
                conditions.open_durable_scope,
                conditions.snapshotting,
            );
            assert_eq!(
                conditions.blocker().is_none(),
                at_checkpoint_boundary && !conditions.in_flight_host_call,
                "snapshot admission must equal checkpoint admission plus no in-flight host call; \
                 conditions: {conditions:?}"
            );
        }
    }

    #[test]
    fn suspend_admission_truth_table() {
        let cases = [
            // (live_host_calls, suspendable_waits, open_durable_scope, pending_p3_tx, expected)
            (0, 0, false, false, true),
            (2, 2, false, false, true),
            // A live host call not parked in a suspendable wait blocks suspension.
            (1, 0, false, false, false),
            (3, 2, false, false, false),
            // An open durable scope blocks suspension even when all calls are parked.
            (0, 0, true, false, false),
            (2, 2, true, false, false),
            // A pending P3 HTTP request transmission blocks suspension.
            (0, 0, false, true, false),
            (2, 2, false, true, false),
            (1, 0, true, true, false),
        ];
        for (live_host_calls, suspendable_waits, open_durable_scope, pending_p3_tx, expected) in
            cases
        {
            assert_eq!(
                PrivateDurableWorkerState::suspend_admissible(
                    live_host_calls,
                    suspendable_waits,
                    open_durable_scope,
                    pending_p3_tx,
                ),
                expected,
                "live_host_calls: {live_host_calls}, suspendable_waits: {suspendable_waits}, \
                 open_durable_scope: {open_durable_scope}, pending_p3_tx: {pending_p3_tx}"
            );
        }
    }

    fn permission_id_test_inputs() -> (OwnedAgentId, IdempotencyKey) {
        (
            OwnedAgentId {
                environment_id: EnvironmentId(
                    Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap(),
                ),
                agent_id: AgentId {
                    component_id: ComponentId(
                        Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").unwrap(),
                    ),
                    agent_id: "cart/primary".to_string(),
                },
            },
            IdempotencyKey::new("checkout-invocation".to_string()),
        )
    }

    fn invocation_scope_card(root_card_ids: Vec<CardId>) -> ScopeCard {
        ScopeCard {
            scope_card_id: CardId(Uuid::from_u128(100)),
            root_card_ids,
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
        }
    }

    #[test]
    fn unrelated_revocation_preserves_invocation_scope_and_handles() {
        let scope_root = CardId(Uuid::from_u128(1));
        let unrelated = CardId(Uuid::from_u128(2));
        let mut scope_card = Some(invocation_scope_card(vec![scope_root]));
        let mut handles = HashMap::from([(7, vec![scope_root])]);

        assert_eq!(
            remove_invocation_scope_for_revoked_roots(&mut scope_card, &mut handles, &[unrelated],),
            (false, HashSet::new())
        );
        assert!(scope_card.is_some());
        assert_eq!(handles, HashMap::from([(7, vec![scope_root])]));
    }

    #[test]
    fn matching_revocation_removes_scope_and_dependent_handles_only() {
        let revoked_root = CardId(Uuid::from_u128(1));
        let unrelated_root = CardId(Uuid::from_u128(2));
        let mut scope_card = Some(invocation_scope_card(vec![revoked_root]));
        let mut handles = HashMap::from([
            (7, vec![revoked_root]),
            (11, vec![unrelated_root]),
            (13, vec![revoked_root, unrelated_root]),
        ]);

        assert_eq!(
            remove_invocation_scope_for_revoked_roots(
                &mut scope_card,
                &mut handles,
                &[revoked_root],
            ),
            (true, HashSet::from([7, 13]))
        );
        assert!(scope_card.is_none());
        assert_eq!(handles, HashMap::from([(11, vec![unrelated_root])]));
    }

    #[test]
    fn invocation_end_clears_scope_card_and_all_invocation_handles() {
        let mut scope_card = Some(invocation_scope_card(vec![CardId(Uuid::from_u128(1))]));
        let mut handles = HashMap::from([
            (7, vec![CardId(Uuid::from_u128(1))]),
            (11, vec![CardId(Uuid::from_u128(2))]),
        ]);

        let (scope_changed, removed_handles) =
            clear_invocation_scope_state(&mut scope_card, &mut handles);

        assert!(scope_changed);
        assert!(scope_card.is_none());
        assert!(handles.is_empty());
        assert_eq!(removed_handles, HashSet::from([7, 11]));
    }

    #[test]
    fn permission_id_derivation_has_golden_vectors() {
        let (owned_agent_id, invocation_key) = permission_id_test_inputs();
        let oplog_index = OplogIndex::from_u64(42);

        let card_id = derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        );
        let transfer_id = derive_permission_uuid(
            TRANSFER_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        );
        let installed_child_id = derive_permission_uuid(
            INSTALLED_CHILD_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        );
        let scope_card_id = derive_permission_uuid_for_sequence(
            SCOPE_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            42,
        );

        assert_eq!(
            card_id,
            Uuid::parse_str("00000000-002a-7cdd-b83d-c9e9f32d880c").unwrap()
        );
        assert_eq!(
            transfer_id,
            Uuid::parse_str("00000000-002a-7646-b8aa-b84e348196f6").unwrap()
        );
        assert_eq!(
            installed_child_id,
            Uuid::parse_str("00000000-002a-7607-aaa4-0b04c840169d").unwrap()
        );
        assert_eq!(
            scope_card_id,
            Uuid::parse_str("00000000-002a-7680-bbdb-7f78bd28fc43").unwrap()
        );
        assert_eq!(card_id.as_bytes()[6] >> 4, 7);
        assert_eq!(card_id.as_bytes()[8] >> 6, 2);
        assert_eq!(scope_card_id.as_bytes()[6] >> 4, 7);
        assert_eq!(scope_card_id.as_bytes()[8] >> 6, 2);
    }

    #[test]
    fn permission_card_ids_follow_oplog_order() {
        let (owned_agent_id, invocation_key) = permission_id_test_inputs();
        let derive = |index| {
            derive_permission_uuid(
                DERIVED_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                OplogIndex::from_u64(index),
            )
        };

        assert!(derive(41) < derive(42));
        assert!(derive(42) < derive(43));
    }

    #[test]
    fn scope_card_ids_follow_the_invocation_local_ordinal() {
        let (owned_agent_id, invocation_key) = permission_id_test_inputs();
        let derive = |ordinal| {
            derive_permission_uuid_for_sequence(
                SCOPE_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                ordinal,
            )
        };

        assert!(derive(0) < derive(1));
        assert!(derive(1) < derive(2));
        assert_ne!(derive(0), derive(1));
    }

    #[test]
    fn permission_id_derivation_accepts_the_full_oplog_index_domain() {
        let (owned_agent_id, invocation_key) = permission_id_test_inputs();

        derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            OplogIndex::from_u64(1_u64 << 48),
        );
    }

    #[test]
    fn permission_ids_remain_distinct_after_timestamp_saturation() {
        let (owned_agent_id, invocation_key) = permission_id_test_inputs();
        let ids = [UUID_V7_MAX_TIMESTAMP, 1_u64 << 48, u64::MAX].map(|index| {
            derive_permission_uuid(
                DERIVED_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                OplogIndex::from_u64(index),
            )
        });

        assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn permission_id_domains_are_distinct() {
        let (owned_agent_id, invocation_key) = permission_id_test_inputs();
        let oplog_index = OplogIndex::from_u64(42);
        let ids = [
            derive_permission_uuid(
                DERIVED_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                oplog_index,
            ),
            derive_permission_uuid(
                TRANSFER_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                oplog_index,
            ),
            derive_permission_uuid(
                INSTALLED_CHILD_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                oplog_index,
            ),
            derive_permission_uuid_for_sequence(
                SCOPE_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                oplog_index.as_u64(),
            ),
        ];

        assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 4);
    }

    #[test]
    fn permission_ids_do_not_collide_across_workers() {
        let (base_agent, invocation_key) = permission_id_test_inputs();
        let different_environment = OwnedAgentId {
            environment_id: EnvironmentId(
                Uuid::parse_str("10112233-4455-6677-8899-aabbccddeeff").unwrap(),
            ),
            ..base_agent.clone()
        };
        let different_component = OwnedAgentId {
            agent_id: AgentId {
                component_id: ComponentId(
                    Uuid::parse_str("efeeddcc-bbaa-9988-7766-554433221100").unwrap(),
                ),
                ..base_agent.agent_id.clone()
            },
            ..base_agent.clone()
        };
        let different_name = OwnedAgentId {
            agent_id: AgentId {
                agent_id: "cart/secondary".to_string(),
                ..base_agent.agent_id.clone()
            },
            ..base_agent.clone()
        };
        let oplog_index = OplogIndex::from_u64(42);
        let ids = [
            base_agent,
            different_environment,
            different_component,
            different_name,
        ]
        .map(|owned_agent_id| {
            derive_permission_uuid(
                DERIVED_CARD_ID_CONTEXT,
                &owned_agent_id,
                &invocation_key,
                oplog_index,
            )
        });

        assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 4);
    }

    fn concrete_card(card_id: CardId) -> StoredCard {
        Card {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        }
        .into()
    }

    fn concrete_card_expiring_at(card_id: CardId, expires_at: Option<DateTime<Utc>>) -> StoredCard {
        let mut card = concrete_card(card_id);
        match &mut card {
            StoredCard::Concrete(card) => card.expires_at = expires_at,
            StoredCard::Polymorphic(_) => unreachable!(),
        }
        card
    }

    fn agent_permission_pattern(
        owner: &str,
        recipient: &str,
        verb: Option<AgentVerb>,
    ) -> PermissionPattern {
        PermissionPattern::Agent(ClassPermissionPattern::<AgentClass> {
            verb,
            owner: AgentOwnerPattern::parse(owner).unwrap(),
            recipient: RecipientPattern::parse(recipient).unwrap(),
            resource: AgentResourcePattern::Any,
        })
    }

    fn agent_permission_card(
        card_id: CardId,
        owner: &str,
        recipient: &str,
        lower_verbs: Vec<Option<AgentVerb>>,
        upper_verbs: Vec<Option<AgentVerb>>,
    ) -> StoredCard {
        Card {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: lower_verbs
                .into_iter()
                .map(|verb| agent_permission_pattern(owner, recipient, verb))
                .collect(),
            lower_negative: Vec::new(),
            upper_positive: upper_verbs
                .into_iter()
                .map(|verb| agent_permission_pattern(owner, recipient, verb))
                .collect(),
            upper_negative: Vec::new(),
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        }
        .into()
    }

    fn agent_permission_test_context() -> AgentPermissionMonomorphizationContext {
        AgentPermissionMonomorphizationContext {
            account: AccountEmail::from("owner@example.com"),
            application: ApplicationName::try_from("shop").unwrap(),
            environment: EnvironmentName::try_from("prod").unwrap(),
            component: ComponentName("cart-svc".to_string()),
            agent_name: "Cart(alice)".to_string(),
            agent_type: AgentTypeName("Cart".to_string()),
        }
    }

    fn agent_permission_target(holder: &str, verb: AgentVerb) -> PermissionTarget {
        PermissionTarget::Agent(ClassPermissionTarget::<AgentClass> {
            verb: Some(verb),
            owner: AgentOwnerPattern::parse(holder).unwrap(),
            resource: AgentResourcePattern::Any,
        })
    }

    #[test]
    fn wallet_membership_helpers_bump_once_per_effective_change() {
        let card_id = CardId::new();
        let card = concrete_card(card_id);
        let mut wallet = BTreeMap::new();
        let mut generation = 10;

        assert!(add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
        assert_eq!(generation, 11);
        assert!(!add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
        assert_eq!(generation, 11);

        let mut replacement = card.clone();
        match &mut replacement {
            StoredCard::Concrete(card) => card.system_card = true,
            StoredCard::Polymorphic(_) => unreachable!(),
        }
        assert!(add_wallet_card(&mut wallet, &mut generation, replacement).is_err());
        assert_eq!(generation, 11);

        assert!(!remove_wallet_card(&mut wallet, &mut generation, CardId::new()).unwrap());
        assert_eq!(generation, 11);
        assert!(remove_wallet_card(&mut wallet, &mut generation, card_id).unwrap());
        assert_eq!(generation, 12);
    }

    #[test]
    fn wallet_cascade_removal_bumps_once_for_all_effective_removals() {
        let first = concrete_card(CardId::new());
        let second = concrete_card(CardId::new());
        let retained = concrete_card(CardId::new());
        let mut wallet = BTreeMap::from([
            (first.card_id(), first.clone()),
            (second.card_id(), second.clone()),
            (retained.card_id(), retained.clone()),
        ]);
        let mut generation = 10;

        assert!(
            remove_wallet_cards(
                &mut wallet,
                &mut generation,
                &[first.card_id(), second.card_id(), CardId::new()],
            )
            .unwrap()
        );
        assert_eq!(generation, 11);
        assert_eq!(wallet, BTreeMap::from([(retained.card_id(), retained)]));

        assert!(
            !remove_wallet_cards(
                &mut wallet,
                &mut generation,
                &[first.card_id(), second.card_id()],
            )
            .unwrap()
        );
        assert_eq!(generation, 11);
    }

    #[test]
    fn wallet_cascade_removal_is_atomic_when_generation_is_exhausted() {
        let card = concrete_card(CardId::new());
        let original_wallet = BTreeMap::from([(card.card_id(), card.clone())]);
        let mut wallet = original_wallet.clone();
        let mut generation = u64::MAX;

        assert!(remove_wallet_cards(&mut wallet, &mut generation, &[card.card_id()]).is_err());
        assert_eq!(wallet, original_wallet);
        assert_eq!(generation, u64::MAX);
    }

    #[test]
    fn wallet_cascade_preserves_independent_floor_releases_ceiling_and_replays_identically() {
        let holder = "owner@example.com/shop/prod/cart-svc/Cart(alice)";
        let recipient = "owner@example.com/shop/prod/cart-svc/Cart";
        let context = agent_permission_test_context();
        let view = agent_permission_target(holder, AgentVerb::View);
        let invoke = agent_permission_target(holder, AgentVerb::Invoke);
        let broad_floor =
            agent_permission_card(CardId::new(), holder, recipient, vec![None], Vec::new());
        let independent_view_floor = agent_permission_card(
            CardId::new(),
            holder,
            recipient,
            vec![Some(AgentVerb::View)],
            Vec::new(),
        );
        let view_ceiling = agent_permission_card(
            CardId::new(),
            holder,
            recipient,
            Vec::new(),
            vec![Some(AgentVerb::View)],
        );
        let original_wallet = BTreeMap::from([
            (broad_floor.card_id(), broad_floor.clone()),
            (
                independent_view_floor.card_id(),
                independent_view_floor.clone(),
            ),
            (view_ceiling.card_id(), view_ceiling.clone()),
        ]);
        let original_surface = golem_common::model::card::agent_effective_surface_from_wallet(
            &context,
            original_wallet.values(),
        );
        assert!(original_surface.authorize(&view).unwrap());
        assert!(!original_surface.authorize(&invoke).unwrap());

        let mut live_wallet = original_wallet.clone();
        let mut live_generation = 12;
        assert!(
            remove_wallet_cards(
                &mut live_wallet,
                &mut live_generation,
                &[broad_floor.card_id()],
            )
            .unwrap()
        );
        let live_surface = golem_common::model::card::agent_effective_surface_from_wallet(
            &context,
            live_wallet.values(),
        );
        assert_eq!(live_generation, 13);
        assert!(live_surface.authorize(&view).unwrap());
        assert!(!live_surface.authorize(&invoke).unwrap());

        let mut replayed_wallet = original_wallet.clone();
        let mut replayed_generation = 12;
        assert!(
            remove_wallet_cards(
                &mut replayed_wallet,
                &mut replayed_generation,
                &[broad_floor.card_id()],
            )
            .unwrap()
        );
        adopt_recorded_wallet_generation(&mut replayed_generation, Some(live_generation)).unwrap();
        let replayed_surface = golem_common::model::card::agent_effective_surface_from_wallet(
            &context,
            replayed_wallet.values(),
        );
        assert_eq!(replayed_wallet, live_wallet);
        assert_eq!(replayed_generation, live_generation);
        assert_eq!(
            replayed_surface.source_card_ids,
            live_surface.source_card_ids
        );
        assert_eq!(
            replayed_surface.authorize(&view).unwrap(),
            live_surface.authorize(&view).unwrap()
        );
        assert_eq!(
            replayed_surface.authorize(&invoke).unwrap(),
            live_surface.authorize(&invoke).unwrap()
        );

        let mut released_wallet = original_wallet;
        let mut released_generation = 12;
        assert!(
            remove_wallet_cards(
                &mut released_wallet,
                &mut released_generation,
                &[view_ceiling.card_id()],
            )
            .unwrap()
        );
        let released_surface = golem_common::model::card::agent_effective_surface_from_wallet(
            &context,
            released_wallet.values(),
        );
        assert_eq!(released_generation, 13);
        assert!(released_surface.authorize(&view).unwrap());
        assert!(released_surface.authorize(&invoke).unwrap());
    }

    #[test]
    fn wallet_generation_distinguishes_aba_membership_cycles() {
        let card = concrete_card(CardId::new());
        let mut wallet = BTreeMap::new();
        let mut generation = 0;

        assert!(add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
        let first_contents = wallet.clone();
        let first_generation = generation;

        assert!(remove_wallet_card(&mut wallet, &mut generation, card.card_id()).unwrap());
        assert!(add_wallet_card(&mut wallet, &mut generation, card).unwrap());

        assert_eq!(wallet, first_contents);
        assert_eq!(first_generation, 1);
        assert_eq!(generation, 3);
    }

    #[test]
    fn duplicate_wallet_delivery_does_not_bump_generation() {
        let card = concrete_card(CardId::new());
        let mut wallet = BTreeMap::new();
        let mut generation = 40;

        assert!(add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
        assert!(!add_wallet_card(&mut wallet, &mut generation, card).unwrap());

        assert_eq!(wallet.len(), 1);
        assert_eq!(generation, 41);
    }

    #[test]
    fn expiry_generation_outcome_is_stable_when_replayed() {
        let card = concrete_card(CardId::new());
        let card_id = card.card_id();
        let mut wallet = BTreeMap::from([(card_id, card)]);
        let mut generation = 7;

        assert!(remove_wallet_card(&mut wallet, &mut generation, card_id).unwrap());
        let recorded_expiry_generation = generation;

        assert!(!remove_wallet_card(&mut wallet, &mut generation, card_id).unwrap());
        adopt_recorded_wallet_generation(&mut generation, Some(recorded_expiry_generation))
            .unwrap();

        assert!(wallet.is_empty());
        assert_eq!(generation, 8);
    }

    #[test]
    fn expiry_boundary_is_inclusive_and_preserves_later_cards() {
        let boundary = DateTime::from_timestamp(1_700_000_000, 123).unwrap();
        let elapsed = concrete_card_expiring_at(
            CardId::new(),
            Some(boundary - chrono::Duration::nanoseconds(1)),
        );
        let at_boundary = concrete_card_expiring_at(CardId::new(), Some(boundary));
        let later = concrete_card_expiring_at(
            CardId::new(),
            Some(boundary + chrono::Duration::nanoseconds(1)),
        );
        let indefinite = concrete_card(CardId::new());
        let wallet = BTreeMap::from([
            (elapsed.card_id(), elapsed.clone()),
            (at_boundary.card_id(), at_boundary.clone()),
            (later.card_id(), later),
            (indefinite.card_id(), indefinite),
        ]);

        assert_eq!(
            expired_wallet_card_ids_at(&wallet, boundary)
                .into_iter()
                .collect::<HashSet<_>>(),
            HashSet::from([elapsed.card_id(), at_boundary.card_id()])
        );
    }

    #[test]
    fn recorded_expiry_events_replay_identically_with_an_earlier_clock() {
        let boundary = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let replay_clock = boundary - chrono::Duration::hours(1);
        let elapsed = concrete_card_expiring_at(
            CardId::new(),
            Some(boundary - chrono::Duration::nanoseconds(1)),
        );
        let at_boundary = concrete_card_expiring_at(CardId::new(), Some(boundary));
        let later =
            concrete_card_expiring_at(CardId::new(), Some(boundary + chrono::Duration::hours(1)));
        let original_wallet = BTreeMap::from([
            (elapsed.card_id(), elapsed),
            (at_boundary.card_id(), at_boundary),
            (later.card_id(), later),
        ]);

        let mut live_wallet = original_wallet.clone();
        let mut live_generation = 7;
        let mut recorded_expiries = Vec::new();
        for card_id in expired_wallet_card_ids_at(&live_wallet, boundary) {
            assert!(remove_wallet_card(&mut live_wallet, &mut live_generation, card_id).unwrap());
            recorded_expiries.push((card_id, live_generation));
        }

        let mut replayed_wallet = original_wallet;
        let mut replayed_generation = 7;
        assert!(expired_wallet_card_ids_at(&replayed_wallet, replay_clock).is_empty());
        for (card_id, recorded_generation) in recorded_expiries {
            assert!(
                remove_wallet_card(&mut replayed_wallet, &mut replayed_generation, card_id)
                    .unwrap()
            );
            adopt_recorded_wallet_generation(&mut replayed_generation, Some(recorded_generation))
                .unwrap();
        }

        assert_eq!(replayed_wallet, live_wallet);
        assert_eq!(replayed_generation, live_generation);
        assert_eq!(live_wallet.len(), 1);
        assert_eq!(live_generation, 9);
    }

    #[test]
    fn conflicting_payload_for_existing_card_id_is_rejected_without_generation_change() {
        let card_id = CardId::new();
        let card = concrete_card(card_id);
        let mut conflicting_payload = card.clone();
        match &mut conflicting_payload {
            StoredCard::Concrete(card) => card.system_card = true,
            StoredCard::Polymorphic(_) => unreachable!(),
        }
        let mut wallet = BTreeMap::from([(card_id, card.clone())]);
        let mut generation = 10;

        assert!(add_wallet_card(&mut wallet, &mut generation, conflicting_payload).is_err());

        assert_eq!(generation, 10);
        assert_eq!(wallet.get(&card_id), Some(&card));
    }

    #[test]
    fn wallet_replacement_bumps_for_each_membership_delta() {
        let retained = concrete_card(CardId::new());
        let mut conflicting_retained = retained.clone();
        match &mut conflicting_retained {
            StoredCard::Concrete(card) => card.system_card = true,
            StoredCard::Polymorphic(_) => unreachable!(),
        }
        let removed = concrete_card(CardId::new());
        let added = concrete_card(CardId::new());
        let mut wallet = BTreeMap::from([
            (retained.card_id(), retained.clone()),
            (removed.card_id(), removed),
        ]);
        let replacement = BTreeMap::from([
            (conflicting_retained.card_id(), conflicting_retained),
            (added.card_id(), added),
        ]);
        let mut generation = 20;

        assert!(replace_wallet_cards(&mut wallet, &mut generation, replacement).unwrap());
        assert_eq!(generation, 22);
        assert_eq!(wallet.len(), 2);
        assert_eq!(wallet.get(&retained.card_id()), Some(&retained));
    }

    #[test]
    fn wallet_replacement_is_atomic_when_generation_would_overflow() {
        let removed = concrete_card(CardId::new());
        let added = concrete_card(CardId::new());
        let mut wallet = BTreeMap::from([(removed.card_id(), removed)]);
        let original_wallet = wallet.clone();
        let replacement = BTreeMap::from([(added.card_id(), added)]);
        let mut generation = u64::MAX - 1;

        assert!(replace_wallet_cards(&mut wallet, &mut generation, replacement).is_err());
        assert_eq!((wallet, generation), (original_wallet, u64::MAX - 1));
    }

    #[test]
    fn replay_adopts_recorded_wallet_generation_and_defaults_legacy_entries() {
        let mut generation = 10;

        adopt_recorded_wallet_generation(&mut generation, None).unwrap();
        assert_eq!(generation, 10);

        adopt_recorded_wallet_generation(&mut generation, Some(10)).unwrap();
        assert_eq!(generation, 10);

        adopt_recorded_wallet_generation(&mut generation, Some(12)).unwrap();
        assert_eq!(generation, 12);
    }

    #[test]
    fn replay_rejects_decreasing_recorded_wallet_generation() {
        let mut generation = 10;

        assert!(adopt_recorded_wallet_generation(&mut generation, Some(9)).is_err());
        assert_eq!(generation, 10);
    }

    #[test]
    fn invocation_wallet_pin_establishes_base_before_ordered_mutations() {
        let wallet_id_hash = [0x42; 32];
        let base_card = concrete_card(CardId::new());
        let stale_card = concrete_card(CardId::new());
        let derived_card = concrete_card(CardId::new());
        let mut wallet = BTreeMap::from([
            (base_card.card_id(), base_card.clone()),
            (stale_card.card_id(), stale_card),
        ]);
        let mut generation = 1;

        assert!(
            apply_invocation_wallet_pin(
                &mut wallet,
                wallet_id_hash,
                &mut generation,
                InvocationWalletPin {
                    wallet_token: WalletVersionToken {
                        wallet_id_hash,
                        generation: 1,
                    },
                    pinned_card_ids: vec![base_card.card_id()],
                    scope_card_id: Some(CardId::new()),
                },
            )
            .unwrap()
        );
        assert_eq!(wallet.len(), 1);
        assert_eq!(wallet.get(&base_card.card_id()), Some(&base_card));
        assert_eq!(generation, 1);
        assert!(!wallet.contains_key(&derived_card.card_id()));

        assert!(add_wallet_card(&mut wallet, &mut generation, derived_card.clone()).unwrap());
        adopt_recorded_wallet_generation(&mut generation, Some(2)).unwrap();
        assert_eq!(wallet.len(), 2);
        assert_eq!(wallet.get(&base_card.card_id()), Some(&base_card));
        assert_eq!(wallet.get(&derived_card.card_id()), Some(&derived_card));
        assert_eq!(generation, 2);
    }

    fn polymorphic_card(card_id: CardId) -> StoredCard {
        StoredCard::Polymorphic(PolymorphicCard {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
        })
    }

    #[test]
    fn transfer_start_removes_only_concrete_card_from_matching_source_agent() {
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "source-agent".to_string(),
        };
        let source_holder = Some(CardHolder::Agent(AgentCardHolder {
            agent_id: agent_id.clone(),
        }));
        let concrete = concrete_card(CardId::new());
        let polymorphic = polymorphic_card(CardId::new());

        assert!(transfer_started_removes_source_membership(
            Some(&concrete),
            &source_holder,
            &agent_id,
        ));
        assert!(!transfer_started_removes_source_membership(
            Some(&polymorphic),
            &source_holder,
            &agent_id,
        ));
        assert!(transfer_started_removes_source_membership(
            Some(&concrete),
            &None,
            &agent_id,
        ));

        let different_source = Some(CardHolder::Agent(AgentCardHolder {
            agent_id: AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: agent_id.agent_id.clone(),
            },
        }));
        assert!(!transfer_started_removes_source_membership(
            Some(&concrete),
            &different_source,
            &agent_id,
        ));
    }

    #[test]
    fn legacy_transfer_start_removes_concrete_source_membership() {
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "legacy-source-agent".to_string(),
        };
        let concrete = concrete_card(CardId::new());

        assert!(transfer_started_removes_source_membership(
            Some(&concrete),
            &None,
            &agent_id,
        ));
    }

    #[test]
    fn pending_transfer_does_not_block_later_local_card_event() {
        let target_holder = CardHolder::Agent(AgentCardHolder {
            agent_id: AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "target-agent".to_string(),
            },
        });
        let transfer_card = concrete_card(CardId::new());
        let revoked_card_id = CardId::new();
        let pending_transfer = PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(1),
            event: QueuedCardEvent::transfer_started(Uuid::new_v4(), transfer_card, target_holder),
        };
        assert!(next_drainable_card_events(vec![pending_transfer.clone()]).is_empty());

        let pending_events = vec![
            pending_transfer,
            PendingCardEventRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(2),
                event: QueuedCardEvent::revoke(revoked_card_id),
            },
        ];

        let next = next_drainable_card_events(pending_events);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].oplog_index, OplogIndex::from_u64(2));
        assert!(matches!(
            &next[0].event,
            QueuedCardEvent::Revoke(event) if event.card_id == revoked_card_id
        ));
    }

    #[test]
    fn received_transfer_is_drained_as_a_target_wallet_event() {
        let source_card_id = CardId::new();
        let card = concrete_card(CardId::new());
        let receipt = PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(1),
            event: QueuedCardEvent::transfer_received(Uuid::new_v4(), source_card_id, card.clone()),
        };

        let next = next_drainable_card_events(vec![receipt.clone()]);

        assert_eq!(next, vec![receipt]);
        assert!(matches!(
            &next[0].event,
            QueuedCardEvent::TransferReceived(event)
                if event.source_card_id == Some(source_card_id)
                    && event.card_id == card.card_id()
                    && event.card.as_ref() == Some(&card)
        ));
    }

    #[test]
    fn pending_revocations_are_batched_without_crossing_an_install() {
        let first = CardId::new();
        let second = CardId::new();
        let after_install = CardId::new();
        let install = concrete_card(CardId::new());
        let pending_events = vec![
            PendingCardEventRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(1),
                event: QueuedCardEvent::revoke(first),
            },
            PendingCardEventRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(2),
                event: QueuedCardEvent::revoke(second),
            },
            PendingCardEventRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(3),
                event: QueuedCardEvent::install(install),
            },
            PendingCardEventRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(4),
                event: QueuedCardEvent::revoke(after_install),
            },
        ];

        let next = next_drainable_card_events(pending_events);
        assert_eq!(next.len(), 2);
        assert!(matches!(
            &next[0].event,
            QueuedCardEvent::Revoke(event) if event.card_id == first
        ));
        assert!(matches!(
            &next[1].event,
            QueuedCardEvent::Revoke(event) if event.card_id == second
        ));
    }

    #[test]
    fn card_event_boundary_scan_reads_each_entry_once() {
        let initial_idx = OplogIndex::from_u64(10);
        let mut scan = CardEventBoundaryScan::new(initial_idx, Vec::new());
        let mut scanned_entries = 0;

        for boundary in 1..=5_000 {
            let current_idx = OplogIndex::from_u64(initial_idx.as_u64() + boundary * 2);
            let (start, count) = scan.unread_range(current_idx).unwrap();
            assert_eq!(start, OplogIndex::from_u64(current_idx.as_u64() - 1));
            assert_eq!(count, 2);

            let entries = BTreeMap::from([
                (start, OplogEntry::no_op()),
                (current_idx, OplogEntry::no_op()),
            ]);
            scanned_entries += entries.len();
            scan.fold_through(current_idx, &entries);
        }

        assert_eq!(scanned_entries, 10_000);
        assert_eq!(scan.through, OplogIndex::from_u64(10_010));
        assert!(scan.unread_range(scan.through).is_none());
    }

    #[test]
    fn card_event_boundary_scan_folds_queued_and_terminal_entries_incrementally() {
        let card_id = CardId::new();
        let queued_idx = OplogIndex::from_u64(11);
        let terminal_idx = OplogIndex::from_u64(12);
        let mut scan = CardEventBoundaryScan::new(OplogIndex::from_u64(10), Vec::new());

        scan.fold_through(
            queued_idx,
            &BTreeMap::from([(
                queued_idx,
                OplogEntry::card_event_queued(QueuedCardEvent::revoke(card_id)),
            )]),
        );

        assert_eq!(scan.pending.len(), 1);
        assert_eq!(scan.pending[0].oplog_index, queued_idx);

        scan.fold_through(
            terminal_idx,
            &BTreeMap::from([(
                terminal_idx,
                OplogEntry::card_revoked(queued_idx, card_id, None),
            )]),
        );

        assert!(scan.pending.is_empty());
        assert_eq!(scan.through, terminal_idx);
    }

    #[test]
    fn card_event_boundary_scan_rebases_from_authoritative_status() {
        let cached_card_id = CardId::new();
        let status_card_id = CardId::new();
        let cached_pending = PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(9),
            event: QueuedCardEvent::revoke(cached_card_id),
        };
        let status_pending = PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(10),
            event: QueuedCardEvent::revoke(status_card_id),
        };
        let mut scan =
            CardEventBoundaryScan::new(OplogIndex::from_u64(10), vec![cached_pending.clone()]);

        scan.synchronize(
            OplogIndex::from_u64(9),
            std::slice::from_ref(&status_pending),
            OplogIndex::from_u64(10),
        );
        assert_eq!(scan.pending, vec![cached_pending]);

        scan.synchronize(
            OplogIndex::from_u64(10),
            std::slice::from_ref(&status_pending),
            OplogIndex::from_u64(10),
        );
        assert_eq!(scan.pending, vec![status_pending.clone()]);

        scan.synchronize(OplogIndex::from_u64(12), &[], OplogIndex::from_u64(12));
        assert_eq!(scan.through, OplogIndex::from_u64(12));
        assert!(scan.pending.is_empty());
    }

    #[test]
    fn card_event_boundary_scan_discards_cache_after_rewind() {
        let cached_pending = PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(20),
            event: QueuedCardEvent::revoke(CardId::new()),
        };
        let status_pending = PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(5),
            event: QueuedCardEvent::revoke(CardId::new()),
        };
        let mut scan = CardEventBoundaryScan::new(OplogIndex::from_u64(20), vec![cached_pending]);

        scan.synchronize(
            OplogIndex::from_u64(5),
            std::slice::from_ref(&status_pending),
            OplogIndex::from_u64(8),
        );

        assert_eq!(scan.through, OplogIndex::from_u64(5));
        assert_eq!(scan.pending, vec![status_pending]);
        assert_eq!(
            scan.unread_range(OplogIndex::from_u64(8)),
            Some((OplogIndex::from_u64(6), 3))
        );

        let fresh_scan = CardEventBoundaryScan::new(OplogIndex::from_u64(8), Vec::new());
        assert!(fresh_scan.pending.is_empty());
    }

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

    fn open_region(regions: &mut Vec<ActiveAtomicRegion>, begin: u64) -> OplogIndex {
        let begin_index = OplogIndex::from_u64(begin);
        regions.push(ActiveAtomicRegion::new(begin_index, begin_index.next()));
        begin_index
    }

    #[test]
    fn atomic_region_nested_close_transfers_pending_lease_to_parent() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);
        let inner = open_region(&mut regions, 20);

        let lease = register_atomic_region_call(&mut regions, inner, true).unwrap();
        assert_eq!(lease.owner(), Some(inner));

        close_atomic_region(&mut regions, inner);

        assert_eq!(lease.owner(), Some(outer));
        let survivors = atomic_region_surviving_members(&regions, outer);
        assert_eq!(survivors.len(), 1);
        assert!(std::sync::Arc::ptr_eq(&survivors[0], &lease));
    }

    #[test]
    fn atomic_region_outermost_close_detaches_replay_safe_call() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);

        let lease = register_atomic_region_call(&mut regions, outer, true).unwrap();
        assert!(!atomic_region_has_parent(&regions, outer));

        close_atomic_region(&mut regions, outer);

        // Detached: the call's retry grouping falls back to its own execution scope.
        assert_eq!(lease.owner(), None);
        assert!(regions.is_empty());
    }

    #[test]
    fn atomic_region_outermost_close_guard_sees_pending_unsafe_call() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);

        let _lease = register_atomic_region_call(&mut regions, outer, false).unwrap();

        // The live close path (mark_end_operation) rejects the close when the outermost region
        // still has a surviving non-repairable member; verify the guard predicate observes it.
        let blocked = !atomic_region_has_parent(&regions, outer)
            && atomic_region_surviving_members(&regions, outer)
                .iter()
                .any(|lease| !lease.repairable_when_incomplete());
        assert!(blocked);
    }

    #[test]
    fn atomic_region_nested_close_transfers_unsafe_call_without_blocking() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);
        let inner = open_region(&mut regions, 20);

        let lease = register_atomic_region_call(&mut regions, inner, false).unwrap();

        // A nested close never blocks: the unsafe call transfers to the parent, which becomes
        // responsible for it at its own close.
        assert!(atomic_region_has_parent(&regions, inner));
        close_atomic_region(&mut regions, inner);

        assert_eq!(lease.owner(), Some(outer));
        let blocked = !atomic_region_has_parent(&regions, outer)
            && atomic_region_surviving_members(&regions, outer)
                .iter()
                .any(|lease| !lease.repairable_when_incomplete());
        assert!(blocked);
    }

    #[test]
    fn atomic_region_release_after_transfer_removes_member_from_new_owner() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);
        let inner = open_region(&mut regions, 20);

        let lease = register_atomic_region_call(&mut regions, inner, false).unwrap();
        close_atomic_region(&mut regions, inner);
        assert_eq!(lease.owner(), Some(outer));

        // Completion / cancellation releases the lease from its *current* owner.
        lease.release();
        assert_eq!(lease.owner(), None);
        assert!(atomic_region_surviving_members(&regions, outer).is_empty());
    }

    #[test]
    fn atomic_region_released_lease_is_not_transferred_on_close() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);
        let inner = open_region(&mut regions, 20);

        let lease = register_atomic_region_call(&mut regions, inner, true).unwrap();
        lease.release();

        close_atomic_region(&mut regions, inner);

        assert_eq!(lease.owner(), None);
        assert!(atomic_region_surviving_members(&regions, outer).is_empty());
    }

    #[test]
    fn atomic_region_dropped_lease_leaves_no_stale_member() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);

        let lease = register_atomic_region_call(&mut regions, outer, false).unwrap();
        drop(lease);

        // The registry holds weak references only: a dropped handle's bookkeeping does not
        // survive as a stale blocker.
        assert!(atomic_region_surviving_members(&regions, outer).is_empty());
        let blocked = atomic_region_surviving_members(&regions, outer)
            .iter()
            .any(|lease| !lease.repairable_when_incomplete());
        assert!(!blocked);
    }

    #[test]
    fn atomic_region_nested_close_propagates_side_effects_to_parent() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);
        let inner = open_region(&mut regions, 20);

        assert!(mark_atomic_region_has_side_effects_for(&mut regions, inner));
        close_atomic_region(&mut regions, inner);

        assert!(
            regions
                .iter()
                .find(|region| region.begin_index == outer)
                .unwrap()
                .has_side_effects
        );
    }

    #[test]
    fn atomic_region_close_of_unknown_region_is_noop() {
        let mut regions = Vec::new();
        let outer = open_region(&mut regions, 10);
        let lease = register_atomic_region_call(&mut regions, outer, true).unwrap();

        close_atomic_region(&mut regions, OplogIndex::from_u64(99));

        assert_eq!(lease.owner(), Some(outer));
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn atomic_region_register_in_unknown_region_returns_none() {
        let mut regions = Vec::new();
        open_region(&mut regions, 10);

        assert!(
            register_atomic_region_call(&mut regions, OplogIndex::from_u64(99), true).is_none()
        );
    }

    #[test]
    fn atomic_region_two_level_transfer_follows_current_owner() {
        let mut regions = Vec::new();
        let outermost = open_region(&mut regions, 10);
        let middle = open_region(&mut regions, 20);
        let innermost = open_region(&mut regions, 30);

        let lease = register_atomic_region_call(&mut regions, innermost, true).unwrap();

        close_atomic_region(&mut regions, innermost);
        assert_eq!(lease.owner(), Some(middle));

        close_atomic_region(&mut regions, middle);
        assert_eq!(lease.owner(), Some(outermost));

        close_atomic_region(&mut regions, outermost);
        assert_eq!(lease.owner(), None);
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

    struct PendingRequestBody;

    impl http_body::Body for PendingRequestBody {
        type Data = Bytes;
        type Error = ErrorCode;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Pending
        }

        fn is_end_stream(&self) -> bool {
            false
        }
    }

    async fn poll_p3_send_request_io_with_open_body(
        connection_pool: Option<HttpConnectionPool>,
    ) -> Poll<Result<(), ErrorCode>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (server_done_tx, server_done_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            loop {
                let mut buf = [0; 1024];
                let n = stream.read(&mut buf).await.unwrap();
                assert_ne!(n, 0, "client closed before sending request headers");
                received.extend_from_slice(&buf[..n]);
                if received.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            let _ = server_done_rx.await;
        });

        let mut hooks = DurableHttpHooks {
            connection_pool,
            is_replay: Arc::new(AtomicBool::new(false)),
        };
        let request = ::http::Request::post(format!("http://{addr}/upload"))
            .body(PendingRequestBody.boxed_unsync())
            .unwrap();

        let send = wasmtime_wasi_http::p3::WasiHttpHooks::send_request(
            &mut hooks,
            request,
            None,
            Box::new(async { Ok(()) }),
        );
        let send = Box::into_pin(send);
        let (_response, io) = tokio::time::timeout(Duration::from_secs(5), send)
            .await
            .expect("server responded before request body completed")
            .expect("send_request should return response headers successfully");
        let mut io = Box::into_pin(io);

        let poll = io.as_mut().poll(&mut Context::from_waker(Waker::noop()));

        let _ = server_done_tx.send(());
        server.await.unwrap();

        poll
    }

    #[test]
    #[timeout(120000)]
    async fn p3_pooled_send_request_io_future_waits_for_open_request_body_transmission() {
        let _ = rustls::crypto::ring::default_provider().install_default();

        assert!(
            matches!(
                poll_p3_send_request_io_with_open_body(None).await,
                Poll::Pending
            ),
            "the default p3 request transmission future should remain pending while the request body is still open"
        );

        let pool = HttpConnectionPool::new(wasmtime_wasi_http::p2::HttpConnectionPoolConfig {
            max_idle_per_host: 1,
            idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            max_connections_per_host: 1,
            max_total_connections: 1,
            max_host_entries: 16,
        });

        assert!(
            matches!(
                poll_p3_send_request_io_with_open_body(Some(pool)).await,
                Poll::Pending
            ),
            "the p3 request transmission future must not resolve while the request body is still open"
        );
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
        use crate::services::agent_filesystem as agent_fs;

        let generation_handle = self.filesystem_generation_handle();
        let relative = PathBuf::from(path.to_rel_string());
        let target = agent_fs::PathTarget::at_root(&generation_handle, relative.clone())
            .map_err(|error| filesystem_read_error(path, error))?;
        let attributes = match agent_fs::attributes(
            &generation_handle,
            agent_fs::Target::Path(&target, agent_fs::Follow::Yes),
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        {
            Ok(attributes) => attributes,
            Err(error) if filesystem_error_is_not_found(&error) => {
                return Ok(GetFileSystemNodeResult::NotFound);
            }
            Err(error) => return Err(filesystem_read_error(path, error)),
        };

        if attributes.kind == agent_fs::ObjectKind::File {
            return Ok(GetFileSystemNodeResult::File(component_file_node(
                &generation_handle,
                relative,
                attributes,
            )?));
        }
        if attributes.kind != agent_fs::ObjectKind::Directory {
            return Ok(GetFileSystemNodeResult::NotFound);
        }

        let opened = agent_fs::open(
            &generation_handle,
            target,
            agent_fs::OpenOptions::Existing {
                expected: agent_fs::ObjectKind::Directory,
                access: agent_fs::AccessMode::Read,
                follow: agent_fs::Follow::Yes,
            },
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        .map_err(|error| filesystem_read_error(path, error))?;
        let agent_fs::OpenNode::Directory(directory) = opened.node else {
            unreachable!("directory open returned a non-directory node")
        };
        let entries = agent_fs::list_directory(&generation_handle, &directory)
            .map_err(|error| filesystem_read_error(path, error))?
            .await
            .map_err(|error| filesystem_read_error(path, error))?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in entries {
            let entry_relative = relative.join(&entry.name);
            let entry_target =
                agent_fs::PathTarget::at_root(&generation_handle, entry_relative.clone())
                    .map_err(|error| filesystem_read_error(path, error))?;
            let attributes = agent_fs::attributes(
                &generation_handle,
                agent_fs::Target::Path(&entry_target, agent_fs::Follow::Yes),
            )
            .map_err(|error| filesystem_read_error(path, error))?
            .await
            .map_err(|error| filesystem_read_error(path, error))?;
            result.push(component_file_node(
                &generation_handle,
                entry_relative,
                attributes,
            )?);
        }
        Ok(GetFileSystemNodeResult::Ok(result))
    }

    async fn read_file(
        &self,
        path: &CanonicalFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        use crate::services::agent_filesystem as agent_fs;

        let generation_handle = self.filesystem_generation_handle();
        let relative = PathBuf::from(path.to_rel_string());
        let target = agent_fs::PathTarget::at_root(&generation_handle, relative)
            .map_err(|error| filesystem_read_error(path, error))?;
        let attributes = match agent_fs::attributes(
            &generation_handle,
            agent_fs::Target::Path(&target, agent_fs::Follow::Yes),
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        {
            Ok(attributes) => attributes,
            Err(error) if filesystem_error_is_not_found(&error) => {
                return Ok(ReadFileResult::NotFound);
            }
            Err(error) => return Err(filesystem_read_error(path, error)),
        };
        if attributes.kind != agent_fs::ObjectKind::File {
            return Ok(ReadFileResult::NotAFile);
        }
        let opened = agent_fs::open(
            &generation_handle,
            target,
            agent_fs::OpenOptions::Existing {
                expected: agent_fs::ObjectKind::File,
                access: agent_fs::AccessMode::Read,
                follow: agent_fs::Follow::Yes,
            },
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        .map_err(|error| filesystem_read_error(path, error))?;
        let agent_fs::OpenNode::File(file) = opened.node else {
            unreachable!("file open returned a non-file node")
        };
        let length =
            usize::try_from(attributes.size).map_err(|_| WorkerExecutorError::FileSystemError {
                path: path.to_string(),
                reason: "File is too large to read on this executor".to_string(),
            })?;
        let bytes = agent_fs::read_file(
            &generation_handle,
            &file,
            agent_fs::ReadRange { offset: 0, length },
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        .map_err(|error| filesystem_read_error(path, error))?;
        let stream = futures::stream::once(async move { Ok::<Bytes, WorkerExecutorError>(bytes) });
        Ok(ReadFileResult::Ok(Box::pin(stream)))
    }
}

fn filesystem_error_is_not_found(error: &crate::services::agent_filesystem::Error) -> bool {
    matches!(
        error,
        crate::services::agent_filesystem::Error::Sandbox(source)
            if source.io_error().is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    )
}

fn filesystem_read_error(path: &CanonicalFilePath, error: impl Display) -> WorkerExecutorError {
    WorkerExecutorError::FileSystemError {
        path: path.to_string(),
        reason: error.to_string(),
    }
}

fn component_file_node(
    generation_handle: &FilesystemGenerationHandle,
    relative: PathBuf,
    attributes: crate::services::agent_filesystem::Attributes,
) -> Result<ComponentFileSystemNode, WorkerExecutorError> {
    use crate::services::agent_filesystem as agent_fs;

    let name = relative
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let last_modified = attributes.modified.unwrap_or(SystemTime::UNIX_EPOCH);
    let details = match attributes.kind {
        agent_fs::ObjectKind::File => ComponentFileSystemNodeDetails::File {
            size: attributes.size,
            permissions: agent_fs::path_permissions(generation_handle, &relative)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?,
        },
        agent_fs::ObjectKind::Directory | agent_fs::ObjectKind::Symlink => {
            ComponentFileSystemNodeDetails::Directory
        }
    };
    Ok(ComponentFileSystemNode {
        name,
        last_modified,
        details,
    })
}

/// Number of oplog entries read per backward-scan window. Sized to match the compressed oplog
/// archive's chunk and cache sizes so that each window generally costs a single chunk decompression.
const BACKWARD_OPLOG_SCAN_WINDOW: u64 = 4096;

/// Returns the start index of the inclusive backward-scan window ending at `end`, clamped to
/// [`OplogIndex::INITIAL`].
fn backward_scan_window_start(end: OplogIndex) -> OplogIndex {
    let initial = OplogIndex::INITIAL.as_u64();
    let start = end
        .as_u64()
        .saturating_sub(BACKWARD_OPLOG_SCAN_WINDOW - 1)
        .max(initial);
    OplogIndex::from_u64(start)
}

/// Finds the most recent error of the current invocation (if any) together with its retry point and
/// the surrounding stderr logs.
///
/// A possible future optimization is to maintain the relevant indices (last error, last invocation
/// start) directly in the [`AgentStatusRecord`] so that no backward oplog scan is needed at all.
async fn last_error<T: HasOplogService + HasConfig>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    latest_worker_status: &AgentStatusRecord,
) -> Option<LastError> {
    // Short-circuit: there is nothing to report unless the worker is currently in an error-bearing
    // state. `last_error` otherwise scans backward to the start of the current invocation, which is
    // unbounded for long-running invocations. A failed/retrying worker always has its error near the
    // tail, and a non-empty `current_retry_state` means an `Error` with a tracked retry policy was
    // recorded since the last invocation boundary; in every other case there is no error to find.
    if !matches!(
        latest_worker_status.status,
        AgentStatus::Failed | AgentStatus::Retrying
    ) && latest_worker_status.current_retry_state.is_empty()
    {
        return None;
    }

    let last_index = this
        .oplog_service()
        .get_last_index(owned_agent_id, agent_mode)
        .await;
    if last_index == OplogIndex::NONE {
        return None;
    }

    let mut first_error = None;
    let mut first_retry_from = OplogIndex::NONE;
    let mut last_error_index = last_index;

    // Walk the oplog backward in windows, reading each window in a single bulk range read instead of
    // one entry at a time (the latter thrashes the compressed-archive chunk cache).
    let mut window_end = last_index;
    'scan: loop {
        let window_start = backward_scan_window_start(window_end);
        let entries = this
            .oplog_service()
            .read_exact(
                owned_agent_id,
                agent_mode,
                window_start,
                window_end.as_u64() - window_start.as_u64() + 1,
            )
            .await;

        let mut idx = window_end;
        loop {
            if latest_worker_status
                .deleted_regions
                .is_in_deleted_region(idx)
            {
                // Skip entries in deleted regions without consulting the read range.
            } else {
                match entries.get(&idx) {
                    Some(OplogEntry::Error {
                        error, retry_from, ..
                    }) => {
                        if first_retry_from == OplogIndex::NONE || first_retry_from == *retry_from {
                            last_error_index = idx;
                            if first_error.is_none() {
                                first_error = Some(error.clone());
                                first_retry_from = *retry_from;
                            }
                        } else {
                            // Found an error entry belonging to another retry point
                            break 'scan;
                        }
                    }
                    Some(entry) if entry.is_hint() => {
                        // Skipping hint entries as they can randomly interleave the error entries (such as incoming invocation requests, etc)
                    }
                    Some(
                        OplogEntry::AgentInvocationStarted { .. }
                        | OplogEntry::AgentInvocationFinished { .. },
                    ) => {
                        // Retry counting never gets across invocation boundaries
                        break 'scan;
                    }
                    Some(_) => {
                        // Skipping non-hint entries as well, but only up to the first error entry that's different, or the beginning
                        // of the last invocation
                    }
                    None => {
                        // This is possible if the oplog has been deleted between the get_last_index and the read call
                        break 'scan;
                    }
                }
            }

            if idx == OplogIndex::INITIAL {
                break 'scan;
            }
            if idx == window_start {
                break;
            }
            idx = idx.previous();
        }

        window_end = window_start.previous();
    }

    match first_error {
        Some(error) => Some(LastError {
            error,
            stderr: recover_stderr_logs(this, owned_agent_id, agent_mode, last_error_index).await,
            retry_from: first_retry_from,
        }),
        None => None,
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
    let mut stderr_entries = Vec::new();
    let mut current_stderr_entries_batch = Vec::new();
    let mut first_seen_invocation = None;

    // Walk the oplog backward in windows, reading each window in a single bulk range read instead of
    // one entry at a time.
    let mut window_end = last_oplog_idx;
    'scan: loop {
        let window_start = backward_scan_window_start(window_end);
        let entries = this
            .oplog_service()
            .read_exact(
                owned_agent_id,
                agent_mode,
                window_start,
                window_end.as_u64() - window_start.as_u64() + 1,
            )
            .await;

        let mut idx = window_end;
        loop {
            // Because of retries we might have multiple invocation start entries.
            // Read until the first invocation start entry which does not belong to the same invocation (using the trace id)
            match entries.get(&idx) {
                Some(OplogEntry::Log {
                    level,
                    message,
                    context,
                    ..
                }) if (level == &LogLevel::Warn
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
                Some(OplogEntry::AgentInvocationStarted {
                    idempotency_key, ..
                }) => match &first_seen_invocation {
                    None => {
                        first_seen_invocation = Some(idempotency_key.clone());
                        stderr_entries.extend(std::mem::take(&mut current_stderr_entries_batch));
                        if stderr_entries.len() >= max_count {
                            break 'scan;
                        };
                    }
                    Some(expected_idempotency_key)
                        if idempotency_key == expected_idempotency_key =>
                    {
                        stderr_entries.extend(std::mem::take(&mut current_stderr_entries_batch));
                        if stderr_entries.len() >= max_count {
                            break 'scan;
                        };
                    }
                    Some(_) => break 'scan,
                },
                _ => {}
            }

            if idx == OplogIndex::INITIAL {
                break 'scan;
            }
            if idx == window_start {
                break;
            }
            idx = idx.previous();
        }

        window_end = window_start.previous();
    }
    stderr_entries.reverse();
    stderr_entries.join("")
}

/// Transferable ownership of a P2 HTTP request's durable scope. The request state carrying this
/// session moves from the response future to the response, body, stream, and trailers resources.
/// Synchronous resource drops defer closure through the worker's drop-event queue.
#[derive(Debug, Clone)]
pub(crate) struct HttpRequestSession {
    inner: Arc<HttpRequestSessionInner>,
}

#[derive(Debug)]
struct HttpRequestSessionInner {
    begin_index: OplogIndex,
    span_id: SpanId,
    phase: AtomicU8,
    drop_sink: Option<tokio::sync::mpsc::UnboundedSender<concurrent::DropEvent>>,
}

const HTTP_REQUEST_OPEN: u8 = 0;
const HTTP_REQUEST_SCOPE_CLOSED: u8 = 1;
const HTTP_REQUEST_CLOSED: u8 = 2;

impl HttpRequestSession {
    pub(crate) fn new(
        begin_index: OplogIndex,
        span_id: SpanId,
        drop_sink: Option<tokio::sync::mpsc::UnboundedSender<concurrent::DropEvent>>,
    ) -> Self {
        Self {
            inner: Arc::new(HttpRequestSessionInner {
                begin_index,
                span_id,
                phase: AtomicU8::new(HTTP_REQUEST_OPEN),
                drop_sink,
            }),
        }
    }

    pub(crate) fn begin_index(&self) -> OplogIndex {
        self.inner.begin_index
    }

    pub(crate) fn span_id(&self) -> &SpanId {
        &self.inner.span_id
    }

    pub(crate) fn mark_closed(&self) {
        self.inner
            .phase
            .store(HTTP_REQUEST_CLOSED, Ordering::Release);
    }

    pub(crate) fn mark_scope_closed(&self) {
        let _ = self.inner.phase.compare_exchange(
            HTTP_REQUEST_OPEN,
            HTTP_REQUEST_SCOPE_CLOSED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub(crate) fn defer_close(&self) {
        self.inner.enqueue_close();
    }
}

impl HttpRequestSessionInner {
    fn enqueue_close(&self) {
        let phase = self.phase.swap(HTTP_REQUEST_CLOSED, Ordering::AcqRel);
        if let Some(sink) = &self.drop_sink {
            let event = match phase {
                HTTP_REQUEST_OPEN => Some(concurrent::DropEvent::CloseDurableScope {
                    function_type: DurableFunctionType::WriteRemoteBatched(None),
                    begin_index: self.begin_index,
                    span_id: Some(self.span_id.clone()),
                }),
                HTTP_REQUEST_SCOPE_CLOSED => Some(concurrent::DropEvent::FinishSpan {
                    span_id: self.span_id.clone(),
                    durable: true,
                }),
                _ => None,
            };
            if let Some(event) = event {
                let _ = sink.send(event);
            }
        }
    }
}

impl Drop for HttpRequestSessionInner {
    fn drop(&mut self) {
        self.enqueue_close();
    }
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
    /// Durable scope ownership transferred with this state between WASI resources.
    pub session: HttpRequestSession,
    /// Information about the request to be included in the oplog
    pub request: HostRequestHttpRequest,
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
    pub connect_timeout: Duration,
    pub first_byte_timeout: Duration,
    pub between_bytes_timeout: Duration,
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
    pub fn begin_index(&self) -> OplogIndex {
        self.session.begin_index()
    }

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

/// A durable call's atomic-region membership as a *transferable retry lease*.
///
/// The lease tracks which open atomic region currently owns the call's retry grouping. It starts
/// out owned by the region the call was initiated in, and the owner can change over the call's
/// lifetime: when a region is closed (`mark-end-operation`) while member calls are still pending,
/// their leases transfer to the enclosing open atomic region, or detach entirely at the outermost
/// close (allowed only for calls that are safe to re-execute from an incomplete `Start` on
/// replay). A detached lease means the call's trap/retry grouping falls back to its own execution
/// scope (`retry_from`) and its late terminal marks no atomic-region side effects — it must never
/// retry "into" the already-committed region.
///
/// Reads and writes are store-free (interior mutability), so terminal paths, `Drop` impls, and
/// accessor tasks can release or consult the lease without borrowing the worker state.
#[derive(Debug)]
pub struct AtomicRegionLease {
    /// The begin index of the open atomic region that currently owns this call, or `None` once the
    /// call completed / was released or detached.
    owner: std::sync::Mutex<Option<OplogIndex>>,
    /// Whether the call may be safely re-executed when replay finds its `Start` committed but its
    /// terminal missing (see `InFunctionRetryController::can_reexecute_on_incomplete_replay`).
    /// Non-repairable calls (non-idempotent / batched / transactional writes) keep the outermost
    /// region close rejected while they are pending.
    repairable_when_incomplete: bool,
}

impl AtomicRegionLease {
    fn new(owner: OplogIndex, repairable_when_incomplete: bool) -> Self {
        Self {
            owner: std::sync::Mutex::new(Some(owner)),
            repairable_when_incomplete,
        }
    }

    /// The atomic region currently owning this call, if any.
    pub(crate) fn owner(&self) -> Option<OplogIndex> {
        *self.owner.lock().unwrap()
    }

    /// Releases the lease: the call reached a terminal (or its start was rolled back) and no
    /// longer counts as an in-flight member of any region. Idempotent and store-free, so it is
    /// safe from `Drop` impls and accessor tasks.
    pub(crate) fn release(&self) {
        *self.owner.lock().unwrap() = None;
    }

    /// Whether the call is safe to leave incomplete inside a committed (closed) atomic region.
    pub(crate) fn repairable_when_incomplete(&self) -> bool {
        self.repairable_when_incomplete
    }

    fn transfer(&self, new_owner: Option<OplogIndex>) {
        *self.owner.lock().unwrap() = new_owner;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveAtomicRegion {
    begin_index: OplogIndex,
    next_idempotency_key_oplog_index: OplogIndex,
    has_side_effects: bool,
    /// Leases of durable calls initiated in (or transferred into) this region. Weak so a finished
    /// call's bookkeeping does not outlive its handle; pruned lazily. A member is *surviving*
    /// (still in flight in this region) when the weak upgrades and the lease's current owner is
    /// this region.
    members: Vec<std::sync::Weak<AtomicRegionLease>>,
}

impl ActiveAtomicRegion {
    fn new(begin_index: OplogIndex, next_idempotency_key_oplog_index: OplogIndex) -> Self {
        Self {
            begin_index,
            next_idempotency_key_oplog_index,
            has_side_effects: false,
            members: Vec::new(),
        }
    }

    fn surviving_members(&self) -> Vec<std::sync::Arc<AtomicRegionLease>> {
        self.members
            .iter()
            .filter_map(|weak| weak.upgrade())
            .filter(|lease| lease.owner() == Some(self.begin_index))
            .collect()
    }
}

fn mark_atomic_region_has_side_effects_for(
    active_atomic_regions: &mut [ActiveAtomicRegion],
    begin_index: OplogIndex,
) -> bool {
    if let Some(region) = active_atomic_regions
        .iter_mut()
        .find(|region| region.begin_index == begin_index)
    {
        region.has_side_effects = true;
        true
    } else {
        false
    }
}

/// Registers a durable call as an in-flight member of the atomic region `begin_index` and returns
/// its ownership lease, or `None` when the region is not open.
fn register_atomic_region_call(
    active_atomic_regions: &mut [ActiveAtomicRegion],
    begin_index: OplogIndex,
    repairable_when_incomplete: bool,
) -> Option<std::sync::Arc<AtomicRegionLease>> {
    let region = active_atomic_regions
        .iter_mut()
        .find(|region| region.begin_index == begin_index)?;
    let lease = std::sync::Arc::new(AtomicRegionLease::new(
        begin_index,
        repairable_when_incomplete,
    ));
    region.members.push(std::sync::Arc::downgrade(&lease));
    Some(lease)
}

/// The leases of durable calls still in flight in the atomic region `begin_index`.
fn atomic_region_surviving_members(
    active_atomic_regions: &[ActiveAtomicRegion],
    begin_index: OplogIndex,
) -> Vec<std::sync::Arc<AtomicRegionLease>> {
    active_atomic_regions
        .iter()
        .find(|region| region.begin_index == begin_index)
        .map(|region| region.surviving_members())
        .unwrap_or_default()
}

/// Whether the atomic region `begin_index` is nested inside another open atomic region (which
/// would receive its surviving members on close).
fn atomic_region_has_parent(
    active_atomic_regions: &[ActiveAtomicRegion],
    begin_index: OplogIndex,
) -> bool {
    active_atomic_regions
        .iter()
        .position(|region| region.begin_index == begin_index)
        .is_some_and(|pos| pos > 0)
}

/// Closes the atomic region `begin_index`: transfers its surviving member leases (and its
/// side-effect bit) to the enclosing open atomic region if one exists, detaches them otherwise,
/// and removes the region. No-op when the region is not open.
fn close_atomic_region(
    active_atomic_regions: &mut Vec<ActiveAtomicRegion>,
    begin_index: OplogIndex,
) {
    let Some(pos) = active_atomic_regions
        .iter()
        .position(|region| region.begin_index == begin_index)
    else {
        return;
    };
    let closed = active_atomic_regions.remove(pos);
    let parent = if pos > 0 {
        Some(&mut active_atomic_regions[pos - 1])
    } else {
        None
    };
    match parent {
        Some(parent) => {
            let parent_begin = parent.begin_index;
            for weak in &closed.members {
                if let Some(lease) = weak.upgrade()
                    && lease.owner() == Some(begin_index)
                {
                    lease.transfer(Some(parent_begin));
                    parent.members.push(std::sync::Weak::clone(weak));
                }
            }
            // Entries persisted inside the closed region lie inside the parent's span too, so
            // the parent inherits the side-effect classification.
            parent.has_side_effects |= closed.has_side_effects;
        }
        None => {
            for weak in &closed.members {
                if let Some(lease) = weak.upgrade()
                    && lease.owner() == Some(begin_index)
                {
                    lease.release();
                }
            }
        }
    }
}

/// The kind of a durable scope, identified by the `OplogIndex` of its `Start` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableScopeKind {
    /// A batched remote write scope (`WriteRemoteBatched(None)`).
    BatchedWrite,
    /// A non-idempotent remote write scope (`WriteRemote` with `assume_idempotence == false`).
    NonIdempotentWrite,
    /// A remote transaction scope.
    Transaction,
}

/// A currently open durable scope. Durable scopes are first-class `Start`/`End` pairs
/// (batched writes, non-idempotent writes, transactions) identified by their `Start` index.
/// The innermost open scope provides the `parent_start_index` for any `Start` written while
/// it is open, and contributes to the effective retry point (see `effective_retry_point`).
#[derive(Debug)]
struct ActiveDurableScope {
    start_index: OplogIndex,
    #[allow(dead_code)]
    kind: DurableScopeKind,
    /// During replay, the resolver handle for this scope's `End`: registered when the scope
    /// `Start` is claimed, awaited (and taken) when the scope closes. `None` on the live path (the
    /// scope `End` is written, not replayed) and once the handle has been taken by the closing
    /// `end_function` / transaction terminal.
    replay_end: Option<concurrent::ReplayCallHandle>,
}

#[derive(Debug)]
pub(crate) struct FilesystemOutputStreamState;

/// Direction of a P3 TCP one-shot stream acquisition (`send` vs `receive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TcpSocketStreamDirection {
    Send,
    Receive,
}

/// Tracks which of a TCP socket's one-shot `send`/`receive` streams have been
/// acquired by the durable wrappers. Mirrors the wasmtime native per-socket
/// taken flags so replay can rehydrate them deterministically.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TcpTakenStreams {
    send: bool,
    receive: bool,
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

struct CardEventBoundaryScan {
    through: OplogIndex,
    pending: Vec<PendingCardEventRef>,
}

impl CardEventBoundaryScan {
    fn new(through: OplogIndex, pending: Vec<PendingCardEventRef>) -> Self {
        Self { through, pending }
    }

    fn synchronize(
        &mut self,
        status_idx: OplogIndex,
        status_pending: &[PendingCardEventRef],
        current_idx: OplogIndex,
    ) {
        if status_idx >= self.through || current_idx < self.through {
            self.through = status_idx;
            self.pending = status_pending.to_vec();
        }
    }

    fn unread_range(&self, current_idx: OplogIndex) -> Option<(OplogIndex, u64)> {
        (current_idx > self.through).then(|| {
            (
                self.through.next(),
                current_idx.as_u64() - self.through.as_u64(),
            )
        })
    }

    fn fold_through(
        &mut self,
        current_idx: OplogIndex,
        entries: &BTreeMap<OplogIndex, OplogEntry>,
    ) {
        self.pending = calculate_pending_card_events(std::mem::take(&mut self.pending), entries);
        self.through = current_idx;
    }
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
    card_service: Arc<dyn CardService>,
    card_interest_index: Arc<CardInterestIndex>,
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
    assume_idempotence: bool,

    /// Custom durable invocations whose committed logical `Start` is currently executing live.
    active_custom_invocations: HashMap<OplogIndex, ActiveCustomInvocation>,
    /// Next custom invocation ordinal in each logical parent namespace for the current top-level
    /// agent invocation. `None` is the root namespace.
    custom_invocation_ordinals: HashMap<Option<uuid::Uuid>, u64>,
    /// Live custom invocation ownership scopes, keyed by their process-local resource identity.
    custom_invocation_scopes: HashMap<u64, OpenCustomInvocationScope>,
    next_custom_invocation_scope_id: u64,

    /// State of ongoing http requests, key is the resource id it is most recently associated with (one state object can belong to multiple resources, but just one at once)
    open_http_requests: HashMap<u32, HttpRequestState>,

    /// State of open p3 HTTP responses created by the durable `client::send`, keyed by the p3
    /// response resource rep. Carries the `outgoing-http-request` invocation span (started by
    /// `client::send`, finished when the response body completes or the response resource is
    /// dropped unconsumed — mirroring the P2 `end_http_request` span lifecycle), the request
    /// method/URI for retry properties of body-transfer failures, and — for responses replayed
    /// from recorded headers — the information needed to re-issue the recorded request when the
    /// durable consume-body scope turns out to be incomplete after a restart.
    pub(crate) open_p3_http_responses:
        HashMap<u32, crate::durable_host::p3::http::OpenP3HttpResponseState>,

    /// Body-transmission wiring of open p3 outgoing HTTP requests, keyed by the request resource
    /// rep. Registered by the durable `request::new` (which interposes on the guest-facing
    /// transmission future) and detached by the host call that consumes the request:
    /// `client::send` records/replays the transmission result durably, while a guest-side
    /// `consume-body`/`drop` forwards the deterministic value with no recording.
    pub(crate) pending_p3_http_request_transmissions:
        HashMap<u32, crate::durable_host::p3::http::PendingHttpRequestBodyTransmission>,

    /// WebSocket connection state indexed by websocket resource rep.
    open_websocket_connections: HashMap<u32, WebSocketConnectionState>,

    /// Maps outgoing request rep → outgoing body rep, set during outgoing_request::body()
    /// before outgoing_handler::handle() is called and the HttpRequestState is created.
    pending_http_outgoing_request_body: HashMap<u32, u32>,

    /// Tracks file-backed wasi output streams so quota charging can be based on
    /// actual file growth instead of requested write size.
    open_filesystem_output_streams: HashMap<u32, FilesystemOutputStreamState>,

    /// Reps of file-backed wasi input streams created by `read_via_stream`. Used together with
    /// [`Self::file_stream_pollables`] to identify pollables whose backing operation re-executes
    /// during replay.
    open_filesystem_input_streams: HashSet<u32>,

    /// Reps of pollables subscribed to file-backed input/output streams. Reads/writes on file
    /// streams are not persisted — they re-execute against the restored filesystem during replay —
    /// but their readiness is driven by a background host task that a live `io::poll::poll` /
    /// `pollable::ready` actually awaited before its result was recorded. When such a poll is
    /// replayed, the executor must await the real readiness of these pollables before handing the
    /// guest the recorded result; otherwise the guest's read/poll loop observes a not-yet-ready
    /// stream after a "ready" poll and issues more polls than were recorded, diverging from the
    /// oplog.
    file_stream_pollables: HashSet<u32>,

    /// Shadow of the wasmtime P3 TCP one-shot `send`/`receive` stream-taken flags,
    /// keyed by TCP socket resource rep. The durable wrappers replay `send`/`receive`
    /// from the oplog instead of invoking the native host call, so the native
    /// "taken" state is not advanced on replay. This map records which directions
    /// were acquired so a post-replay second call returns `InvalidState` exactly as
    /// uninterrupted execution would. Reconstructed on replay from the durable
    /// acquire calls and cleared when the socket resource is dropped.
    tcp_taken_streams: HashMap<u32, TcpTakenStreams>,

    /// Maps outgoing body rep → output stream rep, set during outgoing_body::write()
    /// before outgoing_handler::handle() is called. Used by handle() to populate
    /// output_stream_rep in HttpRequestState for streams created before dispatch.
    pending_http_outgoing_body_stream: HashMap<u32, u32>,

    /// Retry eligibility flags accumulated before outgoing_handler::handle() creates
    /// the HttpRequestState. Keyed by outgoing request rep.
    pending_http_retry_eligibility: HashMap<u32, HttpRetryEligibility>,

    snapshotting_mode: bool,

    /// Tracks whether the currently executing invocation is restricted to read-only side effects.
    /// When `ReadOnly`, outgoing HTTP and RPC host calls are trapped before any oplog entry is
    /// written. Defaults to `Normal` and is reset on every invocation exit path.
    invocation_strictness: InvocationStrictness,

    /// Name of the agent method currently being invoked under read-only strictness. Captured at
    /// the invocation entry point so it can be reported in `AgentError::ReadOnlyViolation`.
    read_only_method_name: Option<String>,

    /// Oplog-processor plugin calls are admitted by the operator when the plugin is installed,
    /// outside the per-agent permission-card model. This is set only while dispatching the
    /// executor-created `ProcessOplogEntries` invocation and reset before invocation teardown.
    operator_authorized_oplog_processor_invocation: Arc<AtomicBool>,

    component_metadata: Component,
    owner_component_metadata: Option<Arc<Component>>,
    agent_effective_surface: golem_common::model::card::EffectiveSurface,
    agent_wallet_cards: BTreeMap<CardId, StoredCard>,
    invocation_scope_card: Option<ScopeCard>,
    invocation_scope_root_cards: BTreeMap<CardId, StoredCard>,
    invocation_scope_handles: HashMap<u32, Vec<CardId>>,
    wallet_id_hash: [u8; 32],
    wallet_generation: u64,
    card_event_boundary_scan: Option<CardEventBoundaryScan>,
    card_event_boundary_lock: Arc<tokio::sync::Mutex<()>>,
    published_authority_generation: Arc<AtomicU64>,
    processed_authority_generation: u64,
    authority_initialized: bool,
    next_authority_expiration: Option<DateTime<Utc>>,

    invocation_context: InvocationContext,
    current_span_id: SpanId,
    forward_trace_context_headers: bool,
    set_outgoing_http_idempotency_key: bool,

    worker_fork: Arc<dyn WorkerForkService>,

    file_loader: Arc<FileLoader>,

    shard_service: Arc<dyn ShardService>,

    // The initial local agent config that the worker was configured with
    initial_agent_config: Vec<TypedAgentConfigEntry>,
    /// The current local agent config of the worker, taking the component revision into account
    agent_config: HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,

    /// Cached named retry policies derived from `agent_config` only. Lazily populated and
    /// invalidated whenever `agent_config` is reassigned.
    cached_agent_config_retry_policies: Option<Vec<NamedRetryPolicy>>,

    /// Runtime overlay of named retry policy mutations applied via oplog entries.
    /// `Some(policy)` = set/overwrite, `None` = tombstone (removed).
    /// Applied on top of base policies from agent_config during `named_retry_policies()`.
    runtime_retry_policy_mutations: BTreeMap<String, Option<NamedRetryPolicy>>,

    /// Maps child pollable rep → parent FutureInvokeResult rep.
    /// Used to finalize deferred parent deletion when a child pollable is dropped.
    rpc_pollable_to_parent: HashMap<u32, u32>,

    // ResourceIds of all DynPollables that are backed by GetPromiseResultEntries
    promise_backed_pollables: TRwLock<HashMap<u32, GetPromiseResultEntry>>,
    // Map from resource_id to the dyn_pollables that wrap it
    promise_dyn_pollables: TRwLock<HashMap<u32, HashSet<u32>>>,

    /// The **global fallback** retry point: the index attached to an `Error` entry for a trap that
    /// happens outside any in-flight durable call. It is maintained by `begin_function` /
    /// transaction begin and the explicit HTTP/RPC retry-point writes, so it normally tracks the
    /// last persisted side effect / open scope `Start`.
    ///
    /// An in-flight durable call no longer mirrors its own retry point into this field: a concurrent
    /// durable call carries its retry grouping in its call-owned `execution_scope`
    /// (`durable_host::concurrent`, read by `ScopedRetryHost`) and in the semantic-trap error
    /// marker, so an overlapping call completing cannot clobber a sibling's grouping.
    ///
    /// This is *not* what is read directly at error time. Errors use
    /// [`PrivateDurableWorkerState::effective_retry_point`], which layers priority on top of this
    /// field: an active atomic region (whole region retried from its begin index) wins, and only
    /// otherwise does it fall back to `current_retry_point`. Keep them distinct: write
    /// `current_retry_point`, read `effective_retry_point()`.
    current_retry_point: OplogIndex,

    /// Tracks the active atomic regions by their begin index. This is used together with `current_retry_point` to
    /// determine the effective retry point associated with an error; while `current_retry_point` is changed for each
    /// persisted host call, if there is an active atomic region, the error is associated with that. Otherwise retried
    /// failures within atomic regions would not be grouped by the same retry point as the whole atomic region gets retried
    /// from scratch.
    active_atomic_regions: Vec<ActiveAtomicRegion>,

    /// Currently open durable scopes other than atomic regions: batched / remote writes
    /// (`Start`..`End`) and remote transactions (`Start`..`End` wrapping the transaction markers).
    /// Maintained by `begin_function`/`end_function` and the transaction lifecycle functions.
    ///
    /// While any such scope is open, the current oplog tip sits inside it, so a later trap/replay
    /// can append a jump that deletes the tip — making a mid-invocation status checkpoint at the tip
    /// unsafe (see `at_clean_checkpoint_boundary`). Only the *set* of open scopes matters for this.
    ///
    /// Durable scopes are **not** strictly nested: HTTP / RPC scopes are long-lived and overlap as
    /// siblings (one opens while another is still pending), closing in arbitrary order. So this is
    /// an order-independent collection, not a stack: `remove_durable_scope` removes the closed scope
    /// wherever it is and hard-errors only if it was never open. `parent_start_index` is therefore
    /// **not** derived from this collection (which scope is "innermost" is meaningless for siblings)
    /// — it is threaded explicitly from the owning call/resource. A fresh state is built per worker
    /// incarnation, so a scope left open by a trap is cleared on restart.
    active_durable_scopes: Vec<ActiveDurableScope>,

    /// Number of live durable host calls currently in flight. Used by suspendable P3 waits to
    /// detect when all in-flight work is parked in waits that can safely suspend the worker.
    live_host_calls: Arc<AtomicUsize>,

    /// Activity tracking for Golem-spawned store background tasks. The invocation completion
    /// path drains the store's event loop until no spawned task is active (see
    /// [`tail_work::TailWorkTracker`]) before `AgentInvocationFinished` is written.
    tail_work: tail_work::TailWorkTracker,

    /// Suspend-capable waits currently parked by P3 sleep / promise APIs. The value is the wall
    /// clock deadline for a scheduled wake, if the wait has one; pure promise waits have no
    /// deadline and are woken by promise completion.
    suspendable_waits: Arc<Mutex<BTreeMap<u64, Option<DateTime<Utc>>>>>,
    next_suspendable_wait_id: AtomicU64,

    /// Latched when the current invocation's wall-clock deadline
    /// (`limits.max_invocation_duration`) has been exceeded. Shared with the deadline timer task
    /// (see [`DurableWorkerCtx::arm_invocation_deadline`]); read by `check_interrupt` (epoch
    /// callback) and `create_interrupt_signal` so both executing wasm and newly created
    /// cooperative parks observe the deadline. Cleared when the deadline is (re-)armed and when
    /// its guard drops at the invocation boundary.
    invocation_deadline_exceeded: Arc<AtomicBool>,

    /// Latched when post-completion tail work exceeds its settlement deadline. Cooperative park
    /// points and the epoch callback observe this just like the invocation deadline, but the
    /// guest-call wrapper reports the dedicated tail-work timeout after the event loop unwinds.
    tail_work_deadline_exceeded: Arc<AtomicBool>,

    dropped_call_events: (
        tokio::sync::mpsc::UnboundedSender<concurrent::DropEvent>,
        tokio::sync::mpsc::UnboundedReceiver<concurrent::DropEvent>,
    ),
    completion_marker_recorder: concurrent::CompletionMarkerRecorder,

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
    last_snapshot_source: Option<SnapshotSource>,
    /// Set while the recorded invocations following a loaded automatic snapshot are being
    /// replayed. The snapshot restores the agent's own state but not every implementation detail
    /// of the guest (for example caches of an embedded database), so the replayed tail can issue a
    /// host-call sequence different from the recorded one. Such a divergence is a snapshot recovery
    /// failure: the snapshot is abandoned and the worker replays the full oplog instead.
    replaying_automatic_snapshot_tail: bool,

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

    /// Zero-based ordinal of the next successfully derived invocation-local scope card.
    scope_card_mint_ordinal: u64,

    /// Shared per-account resource limit entry. Used to record monthly HTTP/RPC call consumption
    /// and to check remaining budgets from the epoch callback.
    resource_limit_entry: Arc<AtomicResourceEntry>,
}

#[derive(Clone)]
pub(crate) struct WakeupScheduler {
    promise_service: Arc<dyn PromiseService>,
    scheduler_service: Arc<dyn SchedulerService>,
    oplog: Arc<dyn Oplog>,
    owned_agent_id: OwnedAgentId,
    created_by: AccountId,
}

impl WakeupScheduler {
    pub(crate) async fn sleep_until(&self, when: DateTime<Utc>) -> Result<(), WorkerExecutorError> {
        let promise_id = self
            .promise_service
            .create(
                &self.owned_agent_id.agent_id,
                self.oplog.current_oplog_index().await,
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
        card_service: Arc<dyn CardService>,
        card_interest_index: Arc<CardInterestIndex>,
        component_service: Arc<dyn ComponentService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        config: Arc<GolemConfig>,
        owned_agent_id: OwnedAgentId,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        replay_state: ReplayState,
        runtime: OwnerRuntime,
        component_metadata: Component,
        owner_component_metadata: Option<Arc<Component>>,
        configured_agent_effective_surface: golem_common::model::card::EffectiveSurface,
        worker_fork: Arc<dyn WorkerForkService>,
        file_loader: Arc<FileLoader>,
        created_by: AccountId,
        created_by_email: AccountEmail,
        initial_agent_config: Vec<TypedAgentConfigEntry>,
        agent_config: HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
        shard_service: Arc<dyn ShardService>,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<Uuid>,
        last_snapshot_index: Option<OplogIndex>,
        last_snapshot_source: Option<SnapshotSource>,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        resource_limit_entry: Arc<AtomicResourceEntry>,
        card_event_boundary_lock: Arc<tokio::sync::Mutex<()>>,
        published_authority_generation: Arc<AtomicU64>,
    ) -> Result<Self, WorkerExecutorError> {
        let completion_marker_recorder =
            concurrent::CompletionMarkerRecorder::new(oplog.clone(), replay_state.clone());
        let invocation_context = InvocationContext::new(None);
        let current_span_id = invocation_context.root.span_id().clone();
        let dropped_call_events = tokio::sync::mpsc::unbounded_channel();
        let (agent_wallet_cards, wallet_generation) = match &runtime {
            OwnerRuntime::Agent => {
                let initial_agent_wallet_cards =
                    || -> Result<BTreeMap<CardId, StoredCard>, WorkerExecutorError> {
                        match agent_id.as_ref() {
                            Some(agent_id) => {
                                let card = agent_initial_card_from_component_metadata(
                                    &component_metadata,
                                    agent_id,
                                )?;
                                Ok(BTreeMap::from([(card.card_id(), card)]))
                            }
                            None => Ok(BTreeMap::new()),
                        }
                    };
                if let Some(snapshot_idx) = last_snapshot_index {
                    match oplog.read(snapshot_idx).await {
                        OplogEntry::Snapshot {
                            active_cards,
                            wallet_generation,
                            ..
                        } => (
                            active_cards
                                .into_iter()
                                .map(|card| (card.card_id(), card))
                                .collect(),
                            wallet_generation,
                        ),
                        _ => (initial_agent_wallet_cards()?, 0),
                    }
                } else {
                    (initial_agent_wallet_cards()?, 0)
                }
            }
            OwnerRuntime::Entity(_) => (BTreeMap::new(), 0),
        };
        let wallet_id_hash = CardHolder::Agent(golem_common::model::card::AgentCardHolder {
            agent_id: owned_agent_id.agent_id.clone(),
        })
        .wallet_id_hash();
        let agent_effective_surface = match (&runtime, agent_id.as_ref()) {
            (OwnerRuntime::Agent, Some(agent_id)) => {
                let context =
                    agent_monomorphization_context(&component_metadata, &owned_agent_id, agent_id);
                golem_common::model::card::agent_effective_surface_from_wallet(
                    &context,
                    agent_wallet_cards.values(),
                )
            }
            (OwnerRuntime::Agent, None) => golem_common::model::card::EffectiveSurface::default(),
            (OwnerRuntime::Entity(_), _) => configured_agent_effective_surface,
        };
        Ok(Self {
            oplog_service,
            oplog,
            agent_id,
            http_call_count: 0,
            per_invocation_http_call_limit,
            rpc_call_count: 0,
            per_invocation_rpc_call_limit,
            scope_card_mint_ordinal: 0,
            promise_service,
            scheduler_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            quota_service,
            card_service,
            card_interest_index,
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
            assume_idempotence: true,
            active_custom_invocations: HashMap::new(),
            custom_invocation_ordinals: HashMap::new(),
            custom_invocation_scopes: HashMap::new(),
            next_custom_invocation_scope_id: 1,
            open_http_requests: HashMap::new(),
            open_p3_http_responses: HashMap::new(),
            pending_p3_http_request_transmissions: HashMap::new(),
            open_websocket_connections: HashMap::new(),
            pending_http_outgoing_request_body: HashMap::new(),
            pending_http_outgoing_body_stream: HashMap::new(),
            pending_http_retry_eligibility: HashMap::new(),
            open_filesystem_output_streams: HashMap::new(),
            open_filesystem_input_streams: HashSet::new(),
            file_stream_pollables: HashSet::new(),
            tcp_taken_streams: HashMap::new(),
            snapshotting_mode: false,
            invocation_strictness: InvocationStrictness::Normal,
            read_only_method_name: None,
            operator_authorized_oplog_processor_invocation: Arc::new(AtomicBool::new(false)),
            component_metadata,
            owner_component_metadata,
            agent_effective_surface,
            agent_wallet_cards,
            invocation_scope_card: None,
            invocation_scope_root_cards: BTreeMap::new(),
            invocation_scope_handles: HashMap::new(),
            wallet_id_hash,
            wallet_generation,
            card_event_boundary_scan: None,
            card_event_boundary_lock,
            published_authority_generation,
            processed_authority_generation: 0,
            authority_initialized: false,
            next_authority_expiration: None,
            replay_state,
            invocation_context,
            current_span_id,
            forward_trace_context_headers: true,
            set_outgoing_http_idempotency_key: true,
            worker_fork,
            file_loader,
            created_by,
            created_by_email,
            initial_agent_config,
            config,
            cached_agent_config_retry_policies: None,
            runtime_retry_policy_mutations: BTreeMap::new(),
            rpc_pollable_to_parent: HashMap::new(),
            shard_service,
            promise_backed_pollables: TRwLock::new(HashMap::new()),
            promise_dyn_pollables: TRwLock::new(HashMap::new()),
            pending_update: tokio::sync::Mutex::new(pending_update),
            current_retry_point: OplogIndex::INITIAL,
            active_atomic_regions: Vec::new(),
            active_durable_scopes: Vec::new(),
            live_host_calls: Arc::new(AtomicUsize::new(0)),
            tail_work: tail_work::TailWorkTracker::new(),
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            next_suspendable_wait_id: AtomicU64::new(1),
            invocation_deadline_exceeded: Arc::new(AtomicBool::new(false)),
            tail_work_deadline_exceeded: Arc::new(AtomicBool::new(false)),
            dropped_call_events,
            completion_marker_recorder,
            min_exposed_marker: None,
            current_phantom_id: original_phantom_id,
            last_snapshot_index,
            last_snapshot_source,
            replaying_automatic_snapshot_tail: false,
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
        let agent_config_policies = self.agent_config_retry_policies();

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
        merge_named_retry_policy_tiers(
            default_policy,
            agent_config_policies,
            environment_policies,
            &self.runtime_retry_policy_mutations,
        )
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

    /// The `parent_start_index` to attach to the host-call `Start` of a durable call, given the
    /// function type and the `begin_index` returned by `begin_function`. This is derived
    /// *explicitly* from the call itself, never from the set of temporally-open scopes (which scope
    /// is "innermost" is meaningless when long-lived sibling scopes overlap):
    ///
    /// - if the call opened its own durable scope (non-idempotent `WriteRemote` /
    ///   `WriteRemoteBatched(None)`), its host-call `Start` nests inside that scope (`begin_index`);
    /// - otherwise it nests inside the enclosing scope encoded in the function type
    ///   (`WriteRemoteBatched(Some)` / `WriteRemoteTransaction(Some)`), if any;
    /// - otherwise it is a top-level call with no parent.
    fn child_parent_start_index(
        &self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Option<OplogIndex> {
        if self.opens_durable_scope(function_type) {
            Some(begin_index)
        } else {
            match function_type {
                DurableFunctionType::WriteRemoteBatched(Some(idx))
                | DurableFunctionType::WriteRemoteTransaction(Some(idx)) => Some(*idx),
                _ => None,
            }
        }
    }

    /// Whether a durable function of this `function_type` opens a durable scope — a first-class
    /// `Start`/`End` pair, opened by [`DurableWorkerCtx::begin_function`] and closed by
    /// [`DurableWorkerCtx::end_function`] — namely a non-idempotent remote write or the first
    /// (`None`) call of a batched remote write.
    ///
    /// Unpersisted execution turns off persistence entirely, and `persist`/`replay` skip
    /// `end_function`, so no scope must be opened either: otherwise the scope `Start` would be
    /// committed with no matching `End`, corrupting later replay.
    /// An unpersisted execution never straddles a single scope's begin/end, so guarding both ends
    /// with the same predicate keeps the durable-scope stack balanced.
    fn opens_durable_scope(&self, function_type: &DurableFunctionType) -> bool {
        !self.durability_is_suppressed()
            && ((*function_type == DurableFunctionType::WriteRemote && !self.assume_idempotence)
                || matches!(
                    *function_type,
                    DurableFunctionType::WriteRemoteBatched(None)
                ))
    }

    /// Opens a durable scope identified by its `Start` index. Must be balanced by
    /// `remove_durable_scope` on the matching `End`/`Cancelled`. `replay_end` is the resolver handle
    /// for the scope `End` when the scope was claimed during replay, or `None` on the live
    /// path.
    fn push_durable_scope(
        &mut self,
        start_index: OplogIndex,
        kind: DurableScopeKind,
        replay_end: Option<concurrent::ReplayCallHandle>,
    ) {
        self.active_durable_scopes.push(ActiveDurableScope {
            start_index,
            kind,
            replay_end,
        });
    }

    /// Takes the resolver handle for the scope `End` of the open scope at `start_index`, if one was
    /// registered during replay. Leaves the scope open (it is closed by `remove_durable_scope`
    /// after the `End` has been awaited). Returns `None` if the scope was opened live or the handle
    /// was already taken.
    fn take_durable_scope_replay_handle(
        &mut self,
        start_index: OplogIndex,
    ) -> Option<concurrent::ReplayCallHandle> {
        self.active_durable_scopes
            .iter_mut()
            .find(|scope| scope.start_index == start_index)
            .and_then(|scope| scope.replay_end.take())
    }

    fn is_durable_scope_open(&self, start_index: OplogIndex) -> bool {
        self.active_durable_scopes
            .iter()
            .any(|scope| scope.start_index == start_index)
    }

    /// Closes the durable scope opened at `start_index`. Durable scopes are not strictly nested
    /// (long-lived HTTP / RPC scopes overlap as siblings and close in arbitrary order), so the
    /// closed scope is removed wherever it is in the collection. It is a hard error only if the
    /// scope was never open, which would mean the begin/end bookkeeping got out of sync.
    fn remove_durable_scope(&mut self, start_index: OplogIndex) -> Result<(), WorkerExecutorError> {
        match self
            .active_durable_scopes
            .iter()
            .position(|scope| scope.start_index == start_index)
        {
            Some(pos) => {
                self.active_durable_scopes.remove(pos);
                Ok(())
            }
            None => Err(WorkerExecutorError::runtime(format!(
                "Tried to close durable scope {start_index} but it is not open; open scopes: {:?}",
                self.active_durable_scopes
                    .iter()
                    .map(|s| s.start_index)
                    .collect::<Vec<_>>()
            ))),
        }
    }

    fn dropped_call_event_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedSender<concurrent::DropEvent>> {
        Some(self.dropped_call_events.0.clone())
    }

    fn completion_marker_recorder(&self) -> concurrent::CompletionMarkerRecorder {
        self.completion_marker_recorder.clone()
    }

    fn live_host_call_counter(&self) -> Arc<AtomicUsize> {
        self.live_host_calls.clone()
    }

    /// Whether any live durable host call is currently in flight (its `Start` may already be
    /// appended while its `End`/`Cancelled` is still pending). Used to guard operations that
    /// establish positional oplog boundaries which no durable call's `Start`/`End` pair may
    /// straddle.
    pub fn has_in_flight_live_host_calls(&self) -> bool {
        self.live_host_calls.load(Ordering::Acquire) > 0
    }

    /// Activity tracker for Golem-spawned store background tasks; used to delay invocation
    /// completion until every spawned task is either finished or parked in a recognized safe wait.
    pub fn tail_work_tracker(&self) -> tail_work::TailWorkTracker {
        self.tail_work.clone()
    }

    fn suspendable_waits(&self) -> Arc<Mutex<BTreeMap<u64, Option<DateTime<Utc>>>>> {
        self.suspendable_waits.clone()
    }

    fn next_suspendable_wait_id(&self) -> u64 {
        self.next_suspendable_wait_id.fetch_add(1, Ordering::AcqRel)
    }

    fn register_passive_suspendable_wait(&self) -> suspendable_wait::SuspendableWaitRegistration {
        suspendable_wait::SuspendableWaitRegistration::new(
            self.next_suspendable_wait_id(),
            None,
            self.suspendable_waits(),
        )
    }

    fn safe_to_suspend(&self) -> bool {
        Self::suspend_admissible(
            self.live_host_calls.load(Ordering::Acquire),
            self.suspendable_waits.lock().unwrap().len(),
            !self.active_durable_scopes.is_empty(),
            !self.pending_p3_http_request_transmissions.is_empty(),
        )
    }

    /// Pure form of [`Self::safe_to_suspend`], factored out so its truth table can be tested
    /// without constructing worker state: the worker may suspend when every live durable host
    /// call in flight is parked in a suspendable wait, no durable scope is open, and no P3 HTTP
    /// request transmission is pending.
    fn suspend_admissible(
        live_host_calls: usize,
        suspendable_waits: usize,
        open_durable_scope: bool,
        pending_p3_http_transmission: bool,
    ) -> bool {
        live_host_calls == suspendable_waits && !open_durable_scope && !pending_p3_http_transmission
    }

    fn wakeup_scheduler(&self) -> WakeupScheduler {
        WakeupScheduler {
            promise_service: self.promise_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            oplog: self.oplog.clone(),
            owned_agent_id: self.owned_agent_id.clone(),
            created_by: self.created_by,
        }
    }

    fn take_dropped_call_events(&mut self) -> Vec<concurrent::DropEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.dropped_call_events.1.try_recv() {
            events.push(event);
        }
        events
    }

    fn set_ambient_retry_point(&mut self, retry_point: OplogIndex) {
        self.current_retry_point = retry_point;
    }

    /// The retry point to associate with an error, with priority `atomic region > global`. While an
    /// atomic region is active the whole region is retried from its begin index; otherwise the error
    /// is grouped at `current_retry_point`, which the durable-call machinery keeps pointing at the
    /// enclosing scope `Start` (or the call's own `Start` when unscoped). Durable scopes do **not**
    /// add a tier here: with overlapping sibling scopes there is no meaningful "innermost" scope, so
    /// grouping is driven by the explicitly-maintained `current_retry_point` instead.
    fn effective_retry_point(&self) -> OplogIndex {
        if let Some(region) = self.active_atomic_regions.last() {
            region.begin_index
        } else {
            self.current_retry_point
        }
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

    pub fn mark_atomic_region_has_side_effects_for(&mut self, begin_index: OplogIndex) -> bool {
        mark_atomic_region_has_side_effects_for(&mut self.active_atomic_regions, begin_index)
    }

    /// Registers a durable call as an in-flight member of the atomic region `begin_index` and
    /// returns its ownership lease, or `None` when the region is not open. The caller keeps the
    /// returned `Arc` alive for the call's lifetime and `release()`s it when the call reaches a
    /// terminal; region close (`close_atomic_region`) transfers or detaches surviving leases.
    pub fn register_atomic_region_call(
        &mut self,
        begin_index: OplogIndex,
        repairable_when_incomplete: bool,
    ) -> Option<std::sync::Arc<AtomicRegionLease>> {
        register_atomic_region_call(
            &mut self.active_atomic_regions,
            begin_index,
            repairable_when_incomplete,
        )
    }

    /// The leases of durable calls still in flight in the atomic region `begin_index`.
    pub fn atomic_region_surviving_members(
        &self,
        begin_index: OplogIndex,
    ) -> Vec<std::sync::Arc<AtomicRegionLease>> {
        atomic_region_surviving_members(&self.active_atomic_regions, begin_index)
    }

    /// Whether the atomic region `begin_index` is nested inside another open atomic region (which
    /// would receive its surviving members on close).
    pub fn atomic_region_has_parent(&self, begin_index: OplogIndex) -> bool {
        atomic_region_has_parent(&self.active_atomic_regions, begin_index)
    }

    /// Closes the atomic region `begin_index`: transfers its surviving member leases (and its
    /// side-effect bit) to the enclosing open atomic region if one exists, detaches them
    /// otherwise, and removes the region. Run on both the live path (after the `EndAtomicRegion`
    /// entry is appended) and the replay path (after the entry is consumed), so replay performs
    /// the same ownership transitions as live execution did. No-op when the region is not open.
    pub fn close_atomic_region(&mut self, begin_index: OplogIndex) {
        close_atomic_region(&mut self.active_atomic_regions, begin_index)
    }

    /// Whether the atomic region identified by `begin_index` has recorded side effects. Used for
    /// membership-precise trap classification: a durable call carries its own region's begin index
    /// in its execution scope, so the persisted `inside_atomic_region` flag reflects the *call's*
    /// region rather than whatever region happens to be outermost at trap time.
    pub fn atomic_region_has_side_effects_for(&self, begin_index: OplogIndex) -> bool {
        self.active_atomic_regions
            .iter()
            .find(|region| region.begin_index == begin_index)
            .is_some_and(|region| region.has_side_effects)
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
        self.scope_card_mint_ordinal = 0;
        // The `get_oplog_index` marker watermark is per-invocation: a marker captured in a previous
        // invocation only costs a graceful checkpoint fallback if jumped to, never correctness.
        self.min_exposed_marker = None;
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.replay_state.is_live()
    }

    fn durability_is_suppressed(&self) -> bool {
        self.snapshotting_mode
    }

    /// Whether the current oplog tip is a structurally clean boundary at which a mid-invocation
    /// status checkpoint may be taken: we are live, no rollback-capable region is open (so no later
    /// trap/replay can append a jump that deletes the tip), and snapshotting is not active. The
    /// `get_oplog_index` marker watermark is checked separately by the caller against the committed
    /// status tip.
    pub fn at_clean_checkpoint_boundary(&self) -> bool {
        Self::clean_checkpoint_boundary(
            !self.is_live(),
            !self.active_atomic_regions.is_empty(),
            !self.active_durable_scopes.is_empty(),
            self.durability_is_suppressed(),
        )
    }

    /// Pure form of [`Self::at_clean_checkpoint_boundary`], factored out so its truth table can
    /// be tested without constructing worker state. Each argument is a *blocking* condition: the
    /// boundary is clean iff all of them are `false`.
    fn clean_checkpoint_boundary(
        replaying: bool,
        open_atomic_region: bool,
        open_durable_scope: bool,
        snapshotting: bool,
    ) -> bool {
        !replaying && !open_atomic_region && !open_durable_scope && !snapshotting
    }

    /// The first condition currently blocking a snapshot, or `None` when the worker is at a safe
    /// snapshot boundary.
    ///
    /// A committed snapshot is a replay cut point: snapshot-based recovery (and snapshot-based
    /// update) skips every oplog entry before the snapshot. The invariant is that no durable
    /// construct may span that cut — a durable call or scope whose `Start` precedes the snapshot
    /// but whose `End`/`Cancelled` is recorded after it would leave a terminal whose `Start` the
    /// post-snapshot replay never sees (the orphan terminal is drained harmlessly, but the call
    /// itself cannot be restored from the snapshot). Snapshots are therefore only taken at a
    /// clean checkpoint boundary (no open atomic regions or durable scopes, and no snapshotting)
    /// with no durable host call in flight.
    ///
    /// The sampled conditions must stay in sync with [`Self::at_clean_checkpoint_boundary`]:
    /// `blocker() == None` is equivalent to
    /// `at_clean_checkpoint_boundary() && !has_in_flight_live_host_calls()`.
    pub fn snapshot_boundary_blocker(&self) -> Option<SnapshotBoundaryBlocker> {
        SnapshotBoundaryConditions {
            replaying: !self.is_live(),
            open_atomic_region: !self.active_atomic_regions.is_empty(),
            open_durable_scope: !self.active_durable_scopes.is_empty(),
            snapshotting: self.snapshotting_mode,
            in_flight_host_call: self.has_in_flight_live_host_calls(),
        }
        .blocker()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    pub fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.current_idempotency_key.clone()
    }

    pub fn set_current_idempotency_key(&mut self, invocation_key: IdempotencyKey) {
        self.current_idempotency_key = Some(invocation_key);
        self.custom_invocation_ordinals.clear();
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

/// The snapshot admission conditions sampled from the store state, in diagnostic form.
///
/// Each field is a *blocking* condition (`true` blocks the snapshot); with all fields `false`
/// the worker is at a safe snapshot boundary. This exists solely so snapshot rejections can
/// name the specific condition instead of a generic "not at a safe boundary" message — it is
/// not a general boundary policy: checkpoint, suspend and settlement keep their own predicates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SnapshotBoundaryConditions {
    /// The worker is still replaying its oplog (snapshots are only taken in live mode).
    replaying: bool,
    /// An atomic region is open: a later trap could append a jump deleting the oplog tip.
    open_atomic_region: bool,
    /// A durable scope is open: its `Start`/`End` pair would straddle the snapshot cut.
    open_durable_scope: bool,
    /// A snapshotting function (save/load) call is already in progress.
    snapshotting: bool,
    /// A live durable host call is in flight: its `Start` may precede the cut while its
    /// terminal entry lands after it.
    in_flight_host_call: bool,
}

impl SnapshotBoundaryConditions {
    /// The first blocking condition in precedence order, or `None` when the worker is at a safe
    /// snapshot boundary.
    fn blocker(self) -> Option<SnapshotBoundaryBlocker> {
        if self.replaying {
            Some(SnapshotBoundaryBlocker::Replaying)
        } else if self.open_atomic_region {
            Some(SnapshotBoundaryBlocker::OpenAtomicRegion)
        } else if self.open_durable_scope {
            Some(SnapshotBoundaryBlocker::OpenDurableScope)
        } else if self.snapshotting {
            Some(SnapshotBoundaryBlocker::Snapshotting)
        } else if self.in_flight_host_call {
            Some(SnapshotBoundaryBlocker::InFlightHostCall)
        } else {
            None
        }
    }
}

/// A single condition that blocks taking a snapshot, used in snapshot rejection diagnostics.
/// See [`SnapshotBoundaryConditions`] for what each condition means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotBoundaryBlocker {
    Replaying,
    OpenAtomicRegion,
    OpenDurableScope,
    Snapshotting,
    InFlightHostCall,
}

impl Display for SnapshotBoundaryBlocker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Replaying => write!(f, "the worker is still replaying its oplog"),
            Self::OpenAtomicRegion => write!(f, "an atomic region is still open"),
            Self::OpenDurableScope => write!(f, "a durable scope is still open"),
            Self::Snapshotting => write!(f, "a snapshot function call is already in progress"),
            Self::InFlightHostCall => write!(f, "a durable host call is still in flight"),
        }
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

impl<Ctx: WorkerCtx> WasiView for DurableWorkerCtx<Ctx> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        let ctx = Arc::get_mut(&mut self.wasi)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail");
        let table = Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        let io_ctx = Arc::get_mut(&mut self.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail");
        WasiCtxView { ctx, table, io_ctx }
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

/// Helper macro for expecting a given type of OplogEntry as the next entry in the oplog during
/// replay, while skipping hint entries.
/// The macro expression's type is `Result<(OplogIndex, OplogEntry), WorkerExecutorError>` and it fails if the next non-hint
/// entry was not the expected one.
#[macro_export]
macro_rules! get_oplog_entry {
    (@reader $reader:expr; $($cases:path),+) => {
        loop {
            let (oplog_index, oplog_entry) = $reader.await?;
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
    ($replay_state:expr, $($cases:path),+) => {
        $crate::get_oplog_entry!(@reader ($replay_state).get_oplog_entry(); $($cases),+)
    };
}

/// [`get_oplog_entry!`] variant for call sites running inside Wasmtime accessor futures: reads
/// through [`crate::durable_host::replay_state::ReplayState::get_oplog_entry_owned`], whose cursor
/// transaction runs on an owned task, so the store-polled caller never queues on the cursor mutex
/// directly. Direct invocation-loop / p2 host-call readers keep using [`get_oplog_entry!`].
#[macro_export]
macro_rules! get_oplog_entry_owned {
    ($replay_state:expr, $($cases:path),+) => {
        $crate::get_oplog_entry!(@reader ($replay_state).get_oplog_entry_owned(); $($cases),+)
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
