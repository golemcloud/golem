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
mod calls;
mod construction;
mod entity_context;
mod error_history;
mod filesystem;
mod invocation;
mod recovery;
mod replay_to_live;
mod resources;
mod snapshots;
mod wasi;
pub(crate) use authorization::context::{
    add_wallet_card, adopt_recorded_wallet_generation,
    agent_effective_surface_from_component_metadata, agent_initial_card_from_component_metadata,
    agent_monomorphization_context, apply_invocation_wallet_pin, authority_snapshot_is_current_at,
    authorize_effective_surface, card_holder_is_agent, next_drainable_card_events,
    owner_effective_surface_from_component_metadata, owner_monomorphization_context,
    owner_revoked_card_ids, record_permission_decisions, remove_wallet_card, remove_wallet_cards,
    replace_wallet_cards, transfer_started_removes_source_membership,
};
#[cfg(test)]
use authorization::context::{
    assert_live_authorization, clear_invocation_scope_state, derive_permission_uuid,
    derive_permission_uuid_for_sequence, expired_wallet_card_ids_at,
    remove_invocation_scope_for_revoked_roots,
};
use authorization::context::{
    component_baseline_effective_surface_from_wallet, live_scope_root_cards_from_states,
};
pub(crate) use error_history::{last_error, recover_stderr_logs};
#[cfg(test)]
use filesystem::component_file_node;
#[cfg(test)]
pub(crate) use invocation::{ephemeral_suspend_trap, terminal_entry_refusal};
#[cfg(test)]
use invocation::{ephemeral_terminal_decision, ephemeral_terminal_trap};
pub(crate) use recovery::selected_recovery_error;
#[cfg(test)]
use recovery::{
    prepare_failure, recovered_restart, recovered_status, should_replay_invocation,
    should_restart_after_shard_assignment_change,
};
use replay_to_live::{begin_local_live_continuation, begin_replay_to_live, store_is_live};
pub(crate) use replay_to_live::{
    is_tool_attachment_live_admission_rejection, tool_attachment_live_admission_cancelled_error,
    tool_attachment_live_admission_rejected_error,
};
#[cfg(test)]
use resources::validate_unshared_memory_growth;
pub(crate) use snapshots::perform_at_update_point;
#[cfg(test)]
use snapshots::{payload_download_failure, snapshot_entry_read_failure};
mod cli;
mod clocks;
pub(crate) mod concurrent;
mod config;
pub mod durability;
pub mod durable_session;
pub mod durable_stream;
pub mod entity;
pub mod external_durable_stream;
pub mod golem;
pub mod http;
mod input_observation;
pub mod io;
pub mod keyvalue;
mod logging;
pub mod p3;
pub(crate) mod permissions;
pub mod quota;
mod random;
pub mod rdbms;
pub(crate) mod replay_application;
pub(crate) mod replay_state;
#[cfg(feature = "test-utils")]
pub struct TestEntityBoundaryClaim(pub(crate) concurrent::ReplayCallHandle);
#[cfg(feature = "test-utils")]
impl TestEntityBoundaryClaim {
    pub fn start_idx(&self) -> OplogIndex {
        self.0.start_idx()
    }
}
#[cfg(feature = "test-utils")]
pub use replay_state::ReplayEvent as TestReplayEvent;
pub(crate) mod revision_update;
pub mod schema_value_stream;
mod secrets;
pub use schema_value_stream::CoreTypesHost;
pub mod rpc;
mod sockets;
pub mod stream_bus;
pub mod stream_session;
pub mod stream_transport;
mod suspendable_wait;
pub mod tail_work;
pub mod tool;
pub mod websocket;

tokio::task_local! {
    static ENTITY_CANCELLATION_MASKED: ();
}

/// The per-call quota an outbound durable call is charged against: the per-invocation call
/// limits and the monthly account budgets exist for outgoing HTTP and for agent RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaClass {
    Http,
    Rpc,
}

impl QuotaClass {
    /// The quota class a recorded durable call is charged against, or `None` when calls of this
    /// kind are not quota-charged. This is the single mapping from recorded call kind to quota:
    /// the charging sites name their class directly, and the replay cursor uses this to report
    /// which classes still have an unclaimed retained `Start` waiting for its owner (see
    /// `WorkerState::durable_call_is_fresh`), so the two must agree on the recorded kinds.
    ///
    pub(crate) fn of_recorded_call(function_name: &HostFunctionName) -> Option<Self> {
        match function_name {
            HostFunctionName::P3HttpClientSend
            | HostFunctionName::McpToolCall
            | HostFunctionName::GolemAgentDurableStreamReaderRead
            | HostFunctionName::GolemAgentDurableStreamWriterAppend => Some(Self::Http),
            HostFunctionName::GolemRpcWasmRpcInvoke
            | HostFunctionName::GolemRpcWasmRpcInvokeAndAwaitResult => Some(Self::Rpc),
            _ => None,
        }
    }
}

pub(crate) async fn without_entity_cancellation<F: Future>(future: F) -> F::Output {
    ENTITY_CANCELLATION_MASKED.scope((), future).await
}

/// How a refused write of the failure of an invocation, or of the status after it, ends that
/// invocation. A refusal of the agent's own oplog takes the classification of its agent error:
/// `OplogFailed` is terminal, and `RecoveryRequired` is an infrastructure recovery.
#[derive(Debug)]
pub(crate) enum TerminalEntryRefusal {
    /// The oplog failed for good: an archive layer did not store it, or it does not read. The
    /// invocation fails with its trap, and the agent stops with this error.
    OplogFailed(WorkerExecutorError),
    /// The oplog storage did not answer. The agent recovers with this error, and the invocation
    /// stays pending.
    RecoveryRequired(WorkerExecutorError),
    /// A fence, or a refusal that says nothing of the oplog. Nothing is published: after a fence,
    /// the new owner of the shard runs the invocation.
    Unpublished,
}

use crate::durable_host::durability::collect_named_retry_policies;
use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::durable_host::replay_state::{
    ReplayStartClaimOutcome, ReplayState, ReplayToLiveOutcome, ReplayToLiveRole, StartClaim,
};
use crate::metrics::ephemeral::record_non_suspending_failure;
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::model::event::InternalWorkerEvent;
use crate::model::{
    AgentConfig, ExecutionStatus, HydratedUpdate, InvocationContext, LastError,
    SnapshotReplayPurpose, TrapType,
};
use crate::services::active_agents::MemoryGrant;
use crate::services::agent_filesystem::FilesystemGenerationHandle;
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
use crate::services::oplog::{
    CommitLevel, Oplog, OplogError, OplogOps, OplogPayloadDownloadError, OplogService,
    UnreadableOplog,
};
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
use crate::worker::agent_config::effective_agent_config;
use crate::worker::instance::{OwnerExecution, OwnerRuntimeResources};
use crate::worker::invocation::{
    AgentExportFuncs, InvocationMode, InvokeResult, invocation_uses_streams,
    invoke_observed_and_traced, load_load_snapshot_guest, lower_invocation,
    materialize_streaming_result,
};
use crate::worker::owner_lane::{OwnerInvocationId, OwnerInvocationPermit};
use crate::worker::owner_wallet::{
    AuthorityScopeInputs, CardEventBoundary, OwnerAuthoritySnapshot, WalletCards, WalletSnapshot,
};
use crate::worker::start_outcome::{self, BaselineRole, SnapshotRecoveryResult};
use crate::worker::status::{StatusError, calculate_last_known_status_with_checkpoint};
use crate::worker::{InvocationFailureDecision, RetryDecision, Worker};
use crate::workerctx::{
    ExternalOperations, FileSystemReading, InvocationContextManagement, InvocationHooks,
    InvocationManagement, PublicWorkerIo, StatusManagement, UpdateManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
pub(crate) use concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
pub use durability::*;
use golem_common::base_model::oplog::QueuedCardEvent;
use golem_common::model::TransactionId;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::TypedAgentConfigEntry;
use golem_common::model::agent::{
    AgentMode, AgentPrincipal, ParsedAgentId, Principal, ResolvedOwnerContext,
};
use golem_common::model::card::{
    AgentCardHolder, CardHolder, CardId, InvocationWalletPin, PermissionTarget, ScopeCard,
    StoredCard, WalletVersionToken,
};
use golem_common::model::component::{CanonicalFilePath, ComponentId, ComponentRevision};
use golem_common::model::entity::{
    AgentEntity, EntityInvocationScope, FilesystemCapability, InvocationExecutionMode, OwnerRuntime,
};
#[cfg(test)]
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{
    InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::payload::types::{
    SerializableToolAdmissionRefusal, SerializableToolPositionalRequest,
    SerializableWebsocketReconstructionPolicy,
};
use golem_common::model::oplog::{
    AgentError, AgentResourceId, DurableFunctionType, DurableInvocationId,
    EphemeralCannotSuspendError, LogLevel, OplogEntry, OplogErrorKind, OplogIndex, RawSnapshotData,
    SnapshotAssistedUpdateDetails, UpdateDescription,
};
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::{
    AgentFilter, AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult,
    AgentMetadata, AgentStatus, AgentStatusRecord, IdempotencyKey, OwnedAgentId,
    PendingCardEventRef, PendingUpdateRef, RetryContext, RetryVerdict, ScanCursor, ScheduledAction,
    Timestamp,
};
use golem_common::model::{PredicateValue, RetryPolicyState, RetryProperties};
use golem_common::resource_runtime::Uri;
use golem_common::resource_runtime::{ResourceStore, ResourceTypeId};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::component::Component;
use golem_service_base::model::{
    ComponentFileSystemNode, ComponentFileSystemNodeDetails, GetFileSystemNodeResult,
};
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use std::vec;

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
    executable: crate::workerctx::WorkerCtxExecutable,
    filesystem: FilesystemCapability,
    entity_invocation_scope: Option<EntityInvocationScope>,
    entity_tool_operation: Option<tool::operation::OwnerToolOperation>,
    entity_cancellation: Option<tokio_util::sync::CancellationToken>,
    pub(crate) entity_body_finalization: crate::worker::entity_invocation::EntityBodyFinalization,
    pub(crate) tool_trap_disposal: Arc<AtomicBool>,
    producer_disposal_tasks: Option<crate::worker::tasks::NativeReconstructionTasks>,
    primary_invocation_start_index: Option<OplogIndex>,
    invocation_principal: Option<Principal>,
    owner_execution: Arc<OwnerExecution>,
    pub(crate) runtime_suspension: Option<Arc<crate::worker::suspension::RuntimeStore>>,
    _owner_resources: Arc<OwnerRuntimeResources>,
    entity_reconstruction_claim_hook:
        Option<Arc<dyn crate::workerctx::EntityReconstructionClaimHook>>,
    replay_admission_hook: Option<Arc<dyn crate::workerctx::ReplayAdmissionHook>>,
    pub public_state: PublicDurableWorkerState<Ctx>,
    state: PrivateDurableWorkerState,
    filesystem_generation_handle: FilesystemGenerationHandle,
    filesystem_preopen: Option<AgentDescriptor>,
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

/// Guard for the per-invocation wall-clock deadline; see
/// [`DurableWorkerCtx::arm_invocation_deadline`]. Holds the shared latch and the timer task;
/// dropping it aborts the timer and clears the latch so the deadline never outlives its
/// invocation.
pub struct InvocationDeadline {
    latch: Arc<AtomicBool>,
    duration: Option<Duration>,
    timer: Option<tokio::task::JoinHandle<()>>,
}

/// Guard for the post-completion tail-work deadline. When the deadline fires it cooperatively
/// interrupts store tasks; dropping the guard aborts the timer and clears the latch before the
/// next guest call.
pub(crate) struct TailWorkDeadline {
    latch: Arc<AtomicBool>,
    duration: Duration,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    #[cfg(feature = "test-utils")]
    pub fn test_stage_replay_applications(
        &mut self,
        events: Vec<TestReplayEvent>,
    ) -> Result<(), WorkerExecutorError> {
        self.state.replay_state.test_stage_replay_events(events);
        replay_application::reserve(self)
    }

    #[cfg(feature = "test-utils")]
    pub fn test_replay_application_state(&self) -> (Vec<(usize, bool)>, u64, u64, bool) {
        use futures::FutureExt;
        (
            self.state
                .replay_applications
                .iter()
                .map(|application| {
                    (
                        Arc::as_ptr(application) as usize,
                        application.result.clone().now_or_never().is_some(),
                    )
                })
                .collect(),
            self.state.wallet_generation,
            self.state.owner_wallet.snapshot().generation,
            self.state.owner_wallet.validate_current().is_ok(),
        )
    }

    #[cfg(feature = "test-utils")]
    pub fn test_set_historical_wallet_generation(&mut self, generation: u64) {
        assert!(self.state.replay_applications.is_empty());
        self.state.wallet_generation = generation;
    }

    #[cfg(feature = "test-utils")]
    pub fn test_refuse_next_metadata_admission(&self) {
        self.public_state
            .worker()
            .tasks
            .test_refuse_next_metadata_admission();
    }

    pub(crate) fn suspension_policy(
        &self,
    ) -> Option<(
        WakeupScheduler,
        crate::services::golem_config::SuspendConfig,
    )> {
        (self.agent_mode() == AgentMode::Durable).then(|| {
            (
                self.state.wakeup_scheduler(),
                self.state.config.suspend.clone(),
            )
        })
    }

    #[cfg(feature = "test-utils")]
    pub fn test_install_entity_tool_operation(
        &mut self,
        parent: OwnerInvocationId,
        call_mode: crate::worker::owner_lane::EntityCallMode,
    ) -> Result<(), WorkerExecutorError> {
        let scope = self.entity_invocation_scope().cloned().ok_or_else(|| {
            WorkerExecutorError::runtime("Entity invocation scope is not installed")
        })?;
        let operation = self.owner_execution.tool_operations().create(
            tool::operation::OwnerToolOperationContext {
                parent,
                call_mode,
                activation: scope.activation().clone(),
                principal: self.invocation_principal(),
                descriptor: golem_common::model::entity::EntityInvocationDescriptor::Tool(
                    golem_common::model::entity::ToolInvocationDescriptor {
                        attempt_ordinal: 0,
                        command_path: Vec::new(),
                        args: Vec::new(),
                        has_stdin: false,
                        has_stdout: false,
                        declares_stdout: false,
                        has_stderr: false,
                        declares_stderr: false,
                        output_contract: golem_common::model::entity::ToolOutputContract {
                            result: None,
                            errors: Vec::new(),
                        },
                    },
                ),
                input: golem_common::schema::TypedSchemaValue::new(
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::tuple(Vec::new()),
                    ),
                    golem_common::schema::SchemaValue::Tuple {
                        elements: Vec::new(),
                    },
                ),
            },
        );
        let operation = operation
            .accept(scope.invocation_id().clone())
            .ok_or_else(|| WorkerExecutorError::runtime("Tool operation registration failed"))?;
        self.set_entity_tool_operation(operation)
    }

    pub(crate) fn entity_reconstruction_claim_hook(
        &self,
    ) -> Option<Arc<dyn crate::workerctx::EntityReconstructionClaimHook>> {
        self.entity_reconstruction_claim_hook.clone()
    }

    pub(crate) fn replay_admission_hook(
        &self,
    ) -> Option<Arc<dyn crate::workerctx::ReplayAdmissionHook>> {
        self.replay_admission_hook.clone()
    }

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
        self.filesystem_preopen
            .as_ref()
            .expect("filesystem preopen requested after guest resource disposal")
            .clone()
    }

    pub(crate) fn dispose_trapped_guest_resources(&mut self) {
        *self.table.lock().unwrap() = ResourceTable::new();
        drop(self.filesystem_preopen.take());
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

    pub fn created_by(&self) -> AccountId {
        self.state.created_by
    }

    pub(crate) fn account_resource_limits(&self) -> Arc<AtomicResourceEntry> {
        self.resource_limits.clone()
    }

    pub fn created_by_email(&self) -> &AccountEmail {
        &self.state.created_by_email
    }

    pub fn parsed_agent_id(&self) -> Option<ParsedAgentId> {
        self.state.owner_context.agent().cloned()
    }

    pub fn owner_context(&self) -> &ResolvedOwnerContext {
        &self.state.owner_context
    }

    pub fn agent_mode(&self) -> AgentMode {
        self.execution_status.read().unwrap().agent_mode()
    }

    pub fn component_metadata(&self) -> &Component {
        &self.state.component_metadata
    }

    pub fn executable_component_metadata(&self) -> Option<&Component> {
        match &self.executable {
            crate::workerctx::WorkerCtxExecutable::Component(component) => Some(component),
            crate::workerctx::WorkerCtxExecutable::Native { .. } => None,
        }
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

    pub(crate) fn selected_tool_owner_failure(
        &self,
    ) -> Option<crate::durable_host::tool::operation::OwnerFailureWinner> {
        self.owner_execution
            .tool_operations()
            .selected_owner_failure()
    }

    pub fn agent_type_provision_config(&self) -> Option<&AgentTypeProvisionConfig> {
        self.state.owner_context.agent().and_then(|agent_id| {
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

    pub(crate) fn begin_guest_resource_usage(
        &self,
    ) -> crate::services::linear_memory::GuestResourceUsageGuard {
        self.linear_memory.begin_guest_body()
    }

    pub(crate) fn activate_live_resource_usage(&self) {
        if self.is_live() {
            self.linear_memory.activate_live_usage_if_guest_body();
        }
    }

    async fn emit_log_event(&self, event: InternalWorkerEvent) {
        let trace_context = self
            .state
            .invocation_context
            .span_origin(&self.state.current_span_id)
            .map(
                |(trace_id, _)| golem_common::model::oplog::LogTraceContext {
                    trace_id,
                    span_id: self.state.current_span_id.clone(),
                },
            );
        logging::policy::emit_log_event_with_state::<Ctx>(
            event,
            self.owner_component_metadata()
                .metadata
                .has_oplog_processor(),
            &self.owned_agent_id,
            &self.public_state,
            &self.state.replay_state,
            &self.state.oplog,
            self.state.is_live(),
            self.entity_parent_start_index(),
            trace_context,
        )
        .await;
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn live_stream_event_capacity(&self) -> usize {
        self.state
            .config
            .limits
            .live_stream_event_broadcast_capacity
            .get()
    }

    /// Whether the worker is currently being interrupted through the Golem API
    /// (`ExecutionStatus::Interrupting`), independent of the invocation-deadline latch.
    pub fn is_interrupting(&self) -> bool {
        matches!(
            &*self.execution_status.read().unwrap(),
            ExecutionStatus::Interrupting { .. }
        )
    }

    pub(crate) fn register_open_websocket(
        &mut self,
        rep: u32,
        url: String,
        headers: Option<Vec<(String, String)>>,
        reconstruction_policy: SerializableWebsocketReconstructionPolicy,
    ) {
        self.state.open_websocket_connections.insert(
            rep,
            WebSocketConnectionState {
                url,
                headers,
                reconstruction_policy,
            },
        );
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
                reconstruction_policy: state.reconstruction_policy,
            })
    }
}

pub trait DurableWorkerCtxView<Ctx: WorkerCtx> {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Ctx>;
    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Ctx>;
}

pub(crate) struct OperatorAuthorizedOplogProcessorInvocationGuard {
    active: Arc<AtomicBool>,
}

/// Invocation scope an entity Store inherits from the Store that started its invocation. Tool and
/// middleware code acts as if it were inlined in the owner agent, so the inherited scope follows
/// the primary Store's rules: it applies as soon as it is installed, its roots are checked at the
/// entity Store's first live authority boundary, and a revoked or expired root removes the scope.
#[derive(Clone, Debug)]
struct InheritedInvocationScope {
    card: ScopeCard,
    /// The live root cards, once checked at a live authority boundary.
    roots: Option<BTreeMap<CardId, StoredCard>>,
}

pub(crate) enum BeginReplayToLive {
    ReplayResumed,
    Pending(PendingReplayToLive),
}

// Callers immediately destructure this transient result; avoid allocating each oplog entry.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum PositionalRead {
    Entry(OplogIndex, OplogEntry),
    ReplayEnded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FinishReplayToLive {
    Live,
    AttachmentAdmissionRejected,
    Cancelled,
}

#[must_use = "replay cleanup must finish before live attachment admission is finalized"]
pub(crate) struct PendingReplayToLive {
    #[allow(dead_code)] // The fixed transition target remains available for diagnostics.
    replay_target: OplogIndex,
    role: ReplayToLiveRole,
    replaying_incomplete_entity: bool,
    tool_entity: bool,
    tool_operation: Option<tool::operation::OwnerToolOperation>,
    local_live_tail: Arc<AtomicBool>,
    linear_memory: LinearMemoryTracker,
}

const TOOL_ATTACHMENT_LIVE_ADMISSION_REJECTED: &str =
    "Tool attachment live-memory admission was exhausted";

const TOOL_ATTACHMENT_LIVE_ADMISSION_CANCELLED: &str =
    "Tool attachment live-memory admission was cancelled";

/// A failed replay of the recorded invocations of a start.
struct ReplayFailure {
    error: WorkerExecutorError,
    /// Whether the replay after an application snapshot diverged from the oplog with a typed
    /// divergence while the start still replayed it.
    diverged: bool,
}

/// Why a pending automatic update did not reach the end of its replay.
enum AttemptFailure {
    /// The application snapshot of the baseline did not load.
    Load(SnapshotRecoveryResult),
    /// The replay failed.
    Replay(ReplayFailure),
}

#[cfg(test)]
mod tests;

/// Resident ownership of a transaction protocol's terminal and checkpoint blocker.
struct TransactionProtocol {
    _invocation_id: Option<DurableInvocationId>,
    replay_end: Option<concurrent::ReplayCallHandle>,
    _live_call_permit: concurrent::LiveCallPermit,
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
    pub reconstruction_policy: SerializableWebsocketReconstructionPolicy,
}

#[derive(Debug, Clone)]
pub(crate) struct WebSocketConnectionInfo {
    pub url: String,
    pub headers: Option<Vec<(String, String)>>,
    pub reconstruction_policy: SerializableWebsocketReconstructionPolicy,
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
    owner_context: ResolvedOwnerContext,
    created_by_email: AccountEmail,
    current_idempotency_key: Option<IdempotencyKey>,
    /// Store-local child positions under an entity seed.
    entity_logical_key_position: Option<OplogIndex>,
    rpc: Arc<dyn Rpc>,
    worker_proxy: Arc<dyn WorkerProxy>,
    resources: HashMap<AgentResourceId, (ResourceTypeId, ResourceAny)>,
    last_resource_id: AgentResourceId,
    replay_state: ReplayState,
    /// The invocation's fixed initial mode for entity Stores. Primary Stores follow the shared
    /// published replay state instead.
    entity_execution_mode: Option<InvocationExecutionMode>,
    /// Per-Store live continuation after this entity's matching historical call was removed by a
    /// replay jump while sibling Stores still have owner-oplog reconstruction work.
    local_live_tail: Arc<AtomicBool>,

    /// Custom durable invocations whose committed logical `Start` is currently executing live.
    active_custom_invocations: HashMap<OplogIndex, ActiveCustomInvocation>,
    /// Next custom invocation ordinal in each logical parent namespace for the current top-level
    /// agent invocation. `None` is the root namespace.
    invocation_ordinals: HashMap<OplogIndex, u64>,
    /// Next tool invocation attempt ordinal in each durable parent namespace.
    tool_invocation_attempt_ordinals: HashMap<OwnerInvocationId, u64>,
    /// Live custom invocation ownership scopes, keyed by their process-local resource identity.
    custom_invocation_scopes: HashMap<u64, OpenCustomInvocationScope>,
    next_custom_invocation_scope_id: u64,

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

    tcp_connect_replay: sockets::tcp::TcpConnectReplay,
    open_tcp_input_streams: HashSet<u32>,
    open_tcp_output_streams: HashSet<u32>,
    /// Deadlines of executor-owned P2 timer pollables, removed only after resource deletion.
    p2_timer_deadlines: HashMap<u32, std::time::Instant>,

    /// Shadow of the wasmtime P3 TCP one-shot `send`/`receive` stream-taken flags,
    /// keyed by TCP socket resource rep. The durable wrappers replay `send`/`receive`
    /// from the oplog instead of invoking the native host call, so the native
    /// "taken" state is not advanced on replay. This map records which directions
    /// were acquired so a post-replay second call returns `InvalidState` exactly as
    /// uninterrupted execution would. Reconstructed on replay from the durable
    /// acquire calls and cleared when the socket resource is dropped.
    tcp_taken_streams: HashMap<u32, TcpTakenStreams>,

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
    owner_wallet: Arc<crate::worker::owner_wallet::OwnerWallet>,
    entity_replay_wallet: Option<Arc<crate::worker::owner_wallet::EntityReplayWallet>>,
    authority_wallet: Arc<WalletSnapshot>,
    card_boundary: Weak<CardEventBoundary>,
    replay_applications: VecDeque<Arc<replay_application::ReplayApplication>>,
    agent_effective_surface: golem_common::model::card::EffectiveSurface,
    agent_wallet_cards: WalletCards,
    invocation_scope_card: Option<ScopeCard>,
    invocation_scope_root_cards: BTreeMap<CardId, StoredCard>,
    invocation_scope_handles: HashMap<u32, Vec<CardId>>,
    /// Invocation scope an entity Store inherits from the Store that started its current entity
    /// invocation. It bounds the entity's effective surface but is not exposed as a wallet handle.
    inherited_invocation_scope: Option<InheritedInvocationScope>,
    scope_interest: Option<Arc<crate::services::card_interest::CardInterestContribution>>,
    /// Cards the owner worker has revoked. Revocation events are drained by whichever of the
    /// owner's Stores reaches a boundary first, so every Store excludes these cards from its
    /// surfaces instead of relying on draining every event itself.
    owner_revoked_card_ids: HashSet<CardId>,
    wallet_id_hash: [u8; 32],
    wallet_generation: u64,
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

    /// The **global fallback** retry point: the index attached to an `Error` entry for a trap that
    /// happens outside any in-flight durable call. It is maintained by `begin_function` /
    /// transaction begin and the explicit HTTP/RPC retry-point writes, so it normally tracks the
    /// last persisted side effect / open scope `Start`.
    ///
    /// An in-flight durable call no longer mirrors its own retry point into this field: a concurrent
    /// durable call carries its retry grouping in its call-owned `execution_scope`
    /// (`durable_host::concurrent`, read by `ScopedRetryHost`) and in the semantic-trap error
    /// marker, so an overlapping call completing cannot clobber a sibling's grouping.
    current_retry_point: OplogIndex,

    /// Transaction-owned protocol terminals. Each entry holds a host-call permit until closed.
    transaction_protocols: HashMap<OplogIndex, TransactionProtocol>,

    /// Number of live durable host calls currently in flight, including marker persistence.
    live_host_calls: Arc<AtomicUsize>,

    /// Activity tracking for Golem-spawned store background tasks. The invocation completion
    /// path drains the store's event loop until no spawned task is active (see
    /// [`tail_work::TailWorkTracker`]) before `AgentInvocationFinished` is written.
    tail_work: tail_work::TailWorkTracker,

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
    dropped_call_event_backlog: VecDeque<concurrent::DropEvent>,
    completion_marker_recorder: concurrent::CompletionMarkerRecorder,

    // Update that is pending and should be applied at the end of replay.
    // Other parts of the worker configuration already reflect the worker state implied by the update (component version, env vars, ifs, etc.)
    pending_update: tokio::sync::Mutex<Option<HydratedUpdate>>,

    /// Remains set after finalization takes `pending_update`, until success is committed. Fresh
    /// wallet-independent calls must not extend target-only history while that outcome is pending.
    automatic_update_unsettled: bool,

    /// Stores the phantom ID associated with the currently replayed oplog region. Forks can change it
    current_phantom_id: Option<Uuid>,
    last_snapshot_index: Option<OplogIndex>,
    /// The baseline of the start: the column of the start outcome table.
    baseline_role: BaselineRole,
    /// The purpose of the speculative replay of this start: the replay after a periodic record,
    /// the replay after the record of a snapshot-assisted update, or the full replay of a pending
    /// automatic update, or `None` when the replay is not speculative. A periodic recovery can
    /// reject a divergent record; an update replay fails the update or retries it, as the outcome
    /// table decides, and records no application failure.
    snapshot_replay_purpose: SnapshotReplayPurpose,

    /// A protocol refusal ended replay without granting live authority.
    authoritative_replay_failure: Arc<AtomicBool>,

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
            .await?;

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
        let mut events: Vec<_> = self.dropped_call_event_backlog.drain(..).collect();
        while let Ok(event) = self.dropped_call_events.1.try_recv() {
            events.push(event);
        }
        events
    }

    fn take_next_dropped_call_event(&mut self) -> Option<concurrent::DropEvent> {
        self.dropped_call_event_backlog
            .pop_front()
            .or_else(|| self.dropped_call_events.1.try_recv().ok())
    }

    /// The retry point to associate with an error. The durable-call machinery keeps it at the
    /// enclosing scope `Start` (or the call's own `Start` when unscoped). Durable scopes do **not**
    /// add a tier here: with overlapping sibling scopes there is no meaningful "innermost" scope, so
    /// grouping is driven by the explicitly-maintained `current_retry_point` instead.
    fn effective_retry_point(&self) -> OplogIndex {
        self.current_retry_point
    }

    pub fn current_idempotency_key_oplog_index(&mut self, oplog_index: OplogIndex) -> OplogIndex {
        if let Some(position) = self.entity_logical_key_position.as_mut() {
            let result = *position;
            *position = position.next();
            result
        } else {
            oplog_index
        }
    }

    /// Enriches retry properties with the worker's `agent-type`.
    /// Should be called on all executor-constructed retry property bags before policy resolution.
    pub fn enrich_retry_properties(&self, props: &mut RetryProperties) {
        if let Some(agent_id) = self.owner_context.agent() {
            props.set(
                "agent-type",
                PredicateValue::Text(agent_id.agent_type.to_string()),
            );
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
        store_is_live(
            self.entity_execution_mode,
            self.local_live_tail.load(Ordering::Acquire),
            self.replay_state.is_live_published(),
        )
    }

    pub(crate) fn local_live_tail(&self) -> Arc<AtomicBool> {
        self.local_live_tail.clone()
    }

    /// Whether a durable call that would claim a recorded `Start` may skip replay and record a new
    /// `Start` directly.
    ///
    /// This is stricter than [`Self::is_live`]: while the cursor has retained `Start`s (recorded
    /// `Start`s that a positional reader stepped over without consuming and that no owner has
    /// claimed yet), a call arriving after the live transition may still be the replayed owner of
    /// one of them. Such calls must go through the replay claim path so they can adopt their
    /// retained `Start` instead of duplicating it. Entity Stores in `Live` mode never replay, so
    /// they always answer `true` once live.
    ///
    /// Positional readers, authorization and snapshot decisions keep using [`Self::is_live`],
    /// which describes the Store's position, not a call's admission. Per-call quotas use
    /// [`Self::durable_call_is_fresh`], which narrows the retained-`Start` exception to the
    /// call's own quota class.
    pub fn durable_call_is_live(&self) -> bool {
        self.is_live()
            && (self.entity_execution_mode == Some(InvocationExecutionMode::Live)
                || !self.replay_state.has_unclaimed_retained_starts())
    }

    /// Whether a durable call charged to `quota` that this Store is about to issue is known to
    /// create fresh work: the Store is live and no unclaimed retained `Start` charged to the same
    /// quota remains for a late owner to adopt. Per-call quotas are charged before a call is
    /// admitted (so that a refused call leaves no oplog entry), and use this predicate: a call
    /// that may still adopt a retained recorded `Start` is not charged again, while retained
    /// `Start`s charged to other quotas (or to none) never suppress a charge. Retained `Start`s
    /// of the same quota class but a different owner (another Store of this agent, or a different
    /// call kind sharing the quota) also suppress the charge; that residual under-charge is
    /// bounded by the recorded prefix of one recovered invocation.
    pub fn durable_call_is_fresh(&self, quota: QuotaClass) -> bool {
        self.is_live()
            && (self.entity_execution_mode == Some(InvocationExecutionMode::Live)
                || !self.replay_state.retains_unclaimed_start_charged_to(quota))
    }

    /// Whether this Store is an incomplete entity that already continued live locally while the
    /// shared cursor may still replay other owners' records. See
    /// [`ReplayState::claim_start_for_store`].
    pub(crate) fn store_continued_live(&self) -> bool {
        self.entity_execution_mode == Some(InvocationExecutionMode::ReplayingIncomplete)
            && self.is_live()
    }

    fn durability_is_suppressed(&self) -> bool {
        self.snapshotting_mode
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
        self.invocation_ordinals.clear();
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
    /// A snapshotting function (save/load) call is already in progress.
    snapshotting: bool,
    /// A live durable host call is in flight: its `Start` may precede the cut while its
    /// terminal entry lands after it.
    in_flight_host_call: bool,
    /// Attachment endpoints or pending regeneration need history that the snapshot would skip.
    open_tool_attachment: bool,
}

/// A single condition that blocks taking a snapshot, used in snapshot rejection diagnostics.
/// See [`SnapshotBoundaryConditions`] for what each condition means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotBoundaryBlocker {
    Replaying,
    Snapshotting,
    InFlightHostCall,
    OpenToolAttachment,
}

impl Display for SnapshotBoundaryBlocker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Replaying => write!(f, "the worker is still replaying its oplog"),
            Self::Snapshotting => write!(f, "a snapshot function call is already in progress"),
            Self::InFlightHostCall => write!(f, "a durable host call is still in flight"),
            Self::OpenToolAttachment => {
                write!(f, "a tool attachment still requires replay history")
            }
        }
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

/// Helper macro for expecting a given type of OplogEntry as the next entry in the oplog during
/// replay, while skipping hint entries. The reader is the `DurableWorkerCtx` performing the
/// positional read: its entity attribution decides which interleaved entries it may consume.
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
    ($ctx:expr, $($cases:path),+) => {
        $crate::get_oplog_entry!(@reader ($ctx).state.replay_state.get_oplog_entry(($ctx).entity_parent_start_index()); $($cases),+)
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
