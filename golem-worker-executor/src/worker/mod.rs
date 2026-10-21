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

mod acquisition;
mod ingress;
mod persistence;
mod residency;
mod runtime;
mod startup;
mod streaming;

#[cfg(test)]
use acquisition::snapshot_policy_of_mode;
#[cfg(test)]
use durable_stream_producer::attachments::wait_for_durable_stream_maintenance;
#[cfg(test)]
use ingress::into_pending_invocation_parts;
#[cfg(test)]
use lifecycle::retirement::{epoch_superseded, newly_lost, shard_epoch_to_assert};
#[cfg(test)]
use lifecycle::revert_checked_head;
#[cfg(test)]
use lifecycle::stop::{
    acknowledge_no_store_interruption, complete_stopping_worker, merge_final_worker_state,
    merge_run_loop_failure, terminal_write_failure,
};
#[cfg(test)]
use outcome::{
    is_infrastructure_recovery_error, lookup_result_from_cached_result, recovery_agent_error,
};
#[cfg(test)]
use persistence::scheduled_archive_is_current;
#[cfg(test)]
use residency::{
    classify_target_charge, component_charge_revision, running_worker_can_be_evicted,
    startup_component_charge_revision,
};
#[cfg(test)]
use runtime::cleanup_typed_agent_filesystem;

pub mod agent_config;
pub mod cut_point;
mod durable_stream_producer;
pub use durable_stream_producer::EphemeralResponseLease;
pub mod durable_stream_slots;
pub use durable_stream_slots::{
    AppendStreamSlotOutcome, AppendStreamSlotPayload, AppendToStreamSlotRequest,
    AppendToStreamSlotResult, CreateStreamSessionResult, ExportStreamControlRequest,
    ExportStreamControlResult, ReadStreamSlotRequest, ReadStreamSlotResult,
    StreamSessionCreationIntent, StreamSlotItem, StreamSlotItemContent, StreamSlotProducer,
    StreamSlotReadAdmission,
};
pub mod entity_invocation;
pub mod entity_slot;
pub(crate) mod filesystem_snapshots;
pub mod instance;
mod interrupt;
pub mod invocation;
mod invocation_loop;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
mod monthly_limits;
mod outcome;
pub mod owner_lane;
pub(crate) mod owner_wallet;
pub mod read_only_cache;
pub(crate) mod snapshot_selection;
pub(crate) mod start_outcome;
mod state_actor;
pub mod status;
pub mod status_checkpointer;
pub mod status_flusher;
pub(crate) mod suspension;
pub(crate) mod tasks;

#[cfg(test)]
pub(crate) use invocation_loop::{MonthlyResourceAdmission, monthly_resource_admission};
pub use lifecycle::UpdateMode as WorkerUpdateMode;

use self::agent_config::{
    effective_agent_config, ensure_required_agent_secrets_are_configured,
    parse_worker_creation_agent_config,
};
use crate::durable_host::durable_session::{DurableStreamConsumerJournal, StreamSession};
use crate::durable_host::durable_stream::{
    AttachedStreamSegmentSource, CommittedProducerStreamEvent, ConsumerAttachmentStatus,
    DbDirectStreamAttachmentConsumerProbe, DurableStreamPublishedHorizon, DurableStreamStore,
    ProducerRegistrationRequest, RoutedStreamAttachmentControl, SessionControlMetadata,
    SessionError, StreamAttachmentConsumerProbe, StreamAttachmentControl, StreamStoreError,
};
use crate::durable_host::schema_value_stream::contains_stream;
use crate::durable_host::tool::operation::OwnerFailureWinner;
use crate::durable_host::{
    agent_effective_surface_from_component_metadata, agent_monomorphization_context,
    recover_stderr_logs,
};
use crate::metrics::workers::AdmissionPhase;
use crate::model::{AgentConfig, ExecutionStatus, HydratedUpdate, LookupResult, TrapType};
use crate::sandbox_filesystem::{SandboxFilesystem, SandboxFilesystemAdapter};
use crate::services::active_agents::{
    ActiveAgent, MemoryGrant, RegisteredConcurrentAccount, WorkerComponentCharge,
};
use crate::services::agent_filesystem::{
    AccessMode, FilesystemGenerationHandle, Follow, ObjectKind, OpenOptions, PathTarget,
    ReconstructingFilesystem, ResidentFilesystem, ResidentFilesystemActivity, SealedFilesystem,
    abort_reconstruction, bind_configured_resource_usage_metering,
    delete as delete_agent_filesystem, delete_created, finish_reconstruction, finish_replay,
    materialize_baseline, open as open_agent_filesystem, open_resource_usage_window,
    prepare_initial_files, provision_initial_files, reconstruction_generation_handle,
    resident_generation_handle, tree_mark,
};
use crate::services::agent_filesystem_snapshots;
use crate::services::card_interest::CardInterestIndex;
use crate::services::component::component_support_error;
use crate::services::events::{Event, EventsSubscription};
use crate::services::golem_config::SnapshotPolicy;
use crate::services::linear_memory::{LinearMemoryTracker, SHARED_LINEAR_MEMORY_ERROR};
use crate::services::oplog::plugin::ForwardingOplog;
use crate::services::oplog::{
    ArchiveWait, CommitLevel, EphemeralOplog, MultiLayerOplog, Oplog, OplogError, OplogFence,
    OplogLifecycleGuard, OplogOps, OplogPayloadDownloadError, OplogRefusal, UnreadableOplog,
    downcast_oplog,
};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::ResourceUsageAccount;
use crate::services::rpc::DurableStreamRemoteError;
use crate::services::worker::{
    GetWorkerMetadataResult, InvocationResultIndexLookup, WorkerService,
};
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveAgents, HasAgentFilesystemSnapshots, HasAgentTypesService,
    HasAgentWebhooksService, HasAll, HasBlobStoreService, HasCardService, HasComponentService,
    HasConfig, HasEnvironmentStateService, HasEvents, HasExtraDeps, HasFileLoader,
    HasHttpConnectionPool, HasKeyValueService, HasNativeToolCatalog, HasOplog, HasOplogService,
    HasPromiseService, HasQuotaService, HasRdbmsService, HasResourceLimits, HasRpc,
    HasSchedulerService, HasShardService, HasShutdownToken, HasWasmtimeEngine,
    HasWebSocketConnectionPool, HasWorkerEnumerationService, HasWorkerForkService, HasWorkerProxy,
    HasWorkerService, UsesAllDeps,
};
use crate::worker::instance::{OwnerExecution, OwnerRuntimeResources};
use crate::worker::interrupt::Interrupts;
use crate::worker::invocation_loop::{
    ConcurrentAgentPermitState, InvocationLoop, UnloadCleanupFailure, run_invocation_loop_task,
};
use crate::worker::snapshot_selection::{SnapshotExclusions, StartDecision, StartSelection};
use crate::worker::status::update_queue::is_unselected_automatic;
use crate::worker::status::{
    calculate_last_known_status_with_checkpoint, fold_invocation_result_entries,
    revert_validation_regions, update_status_with_new_entries,
};
use crate::workerctx::{WorkerCtx, WorkerCtxExecutable, WorkerFilesystemContext};
use futures::{
    FutureExt,
    channel::oneshot,
    future::{BoxFuture, Shared},
};
use golem_common::base_model::agent::CachePolicy;
use golem_common::base_model::durable_stream::{
    AttachedStreamSegmentRequest, AttemptId, DURABLE_STREAM_FORMAT_VERSION, LocalStreamReaderId,
    PersistedStreamInvocationDescriptor, ResumeAttemptDescriptor, SessionStreamRole,
    StartAttemptDescriptor, StreamAttachmentControlOperation, StreamAttachmentControlRequest,
    StreamAttachmentFinalizationReason, StreamAttachmentKey, StreamBindingRecord,
    StreamConsumerDeletingRecord, StreamRecordReference, StreamRegistrationInvocation,
    StreamSessionAttachedRecord, StreamSessionExpiryPolicy, StreamSessionKey,
    StreamSessionMappingRecord, StreamSessionPreparedRecord, StreamSessionRecord,
    StreamSessionResumeAttemptRecord,
};
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::base_model::oplog::QueuedCardEvent;
use golem_common::cache::PendingOrFinal;
use golem_common::model::AgentStatus;
use golem_common::model::RetryConfig;
use golem_common::model::agent::{AgentConfigEntryDto, TypedAgentConfigEntry};
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, OwnerKind, ParsedAgentId, Principal,
    ResolvedOwnerContext, Snapshotting, SnapshottingConfig, ephemeral_invocation_phantom_id,
};
use golem_common::model::card::{CardId, StoredCard, card_matches_agent_recipient};
use golem_common::model::component::CanonicalFilePath;
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{
    ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnerRuntime,
};
use golem_common::model::filesystem::{FileByteSelection, FileReadError, validate_file_read_path};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    DurableStreamEventSummary, FilesystemSnapshotName, OplogEntry, OplogErrorKind, OplogIndex,
    OplogPayload, ReadOnlyViolationError, SnapshotAssistedUpdateDetails, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::tool::{ToolBindingOwner, ToolName};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationPayload,
    AgentInvocationResult, AgentMetadata, AgentStatusRecord, IdempotencyKey, OwnedAgentId,
    PendingInvocationRef, PendingUpdateKind, PendingUpdateRef, RetryPolicyState, ShardAssignment,
    ShardEpoch, ShardId, Timestamp,
};
use golem_common::one_shot::OneShotEvent;
use golem_common::read_only_lock;
use golem_common::related_span;
use golem_common::retries::get_delay;
use golem_common::schema::TypedSchemaValue;
use golem_common::tracing::TraceOrigin;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::FileReadResponse;
use golem_service_base::model::GetFileSystemNodeResult;
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, MutexGuard, OnceCell, OwnedMutexGuard, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Level, debug, info, span, warn};
use uuid::Uuid;
use wasmtime::Store;
use wasmtime::component::Instance;

pub const PERMISSION_CARD_TRANSFER_PAYLOAD_CONFLICT: &str =
    "permission card transfer payload conflict";
pub const PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH: &str = "install-recipient-mismatch";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkerCreationMode {
    ComponentAgent,
    EphemeralExternalTool,
}

/// Resolved read-only `AgentMethod` invocation data needed to build the
/// cache key and entry.
#[derive(Clone)]
struct ReadOnlyContext {
    method_name: String,
    input: golem_common::schema::SchemaValue,
    principal: Principal,
    cfg: golem_common::base_model::agent::ReadOnlyConfig,
    component_revision: ComponentRevision,
    cacheable: bool,
}

/// Fully resolved durable streaming invocation admitted by the executor service.
pub struct DurableStreamingInvocationRequest {
    pub attempt: StartAttemptDescriptor,
    pub public_session_id: String,
    pub expiry_policy: StreamSessionExpiryPolicy,
    pub expiry_deadline_millis: Option<u64>,
    pub registrations: Vec<(u64, ProducerRegistrationRequest)>,
    pub foreign_mappings: Vec<StreamSessionMappingRecord>,
    pub input_schema: Arc<golem_schema::schema::SchemaGraph>,
    pub input_element_types: Vec<(u64, golem_schema::schema::SchemaType)>,
    pub invocation: AgentInvocation,
    pub acceptance_committed: tokio::sync::oneshot::Sender<()>,
}

/// Durable state returned after accepting a streaming invocation.
pub struct DurableStreamingInvocationAcceptance {
    pub prepared: StreamSessionPreparedRecord,
    pub streams: StreamSession,
    pub replayed: bool,
    pub joined_origin_observer: bool,
}

/// Durable state returned after resuming or taking over a streaming invocation.
pub struct DurableStreamingResumeAcceptance {
    pub prepared: StreamSessionPreparedRecord,
    pub mappings: Vec<StreamSessionMappingRecord>,
    pub streams: StreamSession,
    pub epoch: u64,
    pub replayed: bool,
}

/// How a pending-update target's metadata-resolution outcome should drive the
/// startup component charge.
#[derive(Debug, PartialEq, Eq)]
enum TargetChargeAction {
    /// The target resolved: charge it with the resolved module size.
    ChargeTarget(ResolvedComponentCharge),
    /// The target cannot load for good (it does not exist, the component service
    /// refused it, the executor does not support it, or metadata failed with any error
    /// but an unavailable component service):
    /// `create_instance` will fail the update and load the current revision, so
    /// charge the current revision instead.
    FallBackToCurrent,
    /// The component service is unavailable: `create_instance` may still load
    /// the target, so retry rather than charging the current revision.
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedComponentCharge {
    module_bytes: u64,
    initial_linear_memory_bytes: u64,
    reserved_linear_memory_bytes: u64,
    supported: bool,
}

struct StartupComponentChargeRequirement {
    component_id: ComponentId,
    component_revision: ComponentRevision,
    module_bytes: u64,
    startup_linear_memory_bytes: u64,
    reserved_linear_memory_bytes: u64,
}

/// How often a caller parked in [`Worker::wait_for_invocation_result`] re-checks
/// that this executor still owns the agent it is waiting for.
///
/// Reached only when the wait is otherwise idle. A tick that finds the agent
/// still owned costs one set lookup for the ownership check, then re-enters the
/// loop and re-runs `lookup_invocation_result`, which loads the published status
/// record. Cheap, but not free, so this is not a millisecond knob.
pub const INVOCATION_OWNERSHIP_RECHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Upper bound on telling one stream producer that this consumer is being deleted.
///
/// The control is retried while the producer is unavailable, which covers a producer that is
/// recovering, migrating or being deleted itself. A producer that stays unavailable longer than
/// this, typically because its own deletion failed, fails this deletion attempt instead of
/// leaving the consumer in `Deleting` without a result; the deletion can then be retried.
const CONSUMER_DELETION_FINALIZATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct PendingMemoryGrowth {
    delta: AtomicU64,
    job_queued: AtomicBool,
}

#[derive(Default)]
struct StartupAttemptTracker {
    state: StdMutex<StartupAttemptState>,
}

#[derive(Default)]
struct StartupAttemptState {
    pending: Option<Uuid>,
    failure: Option<WorkerExecutorError>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeletionHandle {
    completion: Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>,
}

pub(crate) enum DeleteOutcome {
    Started(DeletionHandle),
    AlreadyDeleting(DeletionHandle),
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkerDeletionStage {
    Claimed,
    ExecutionFenced,
    BarriersClosed,
    RuntimeStopped,
    StreamsCleaned,
    OwnedWorkJoined,
    DurableStateRemoved,
    CacheRemoved,
}

/// Test-harness coordination at retryable worker-deletion stage boundaries.
#[doc(hidden)]
#[async_trait::async_trait]
pub trait WorkerDeletionHook: Send + Sync {
    async fn before_claim(&self, _owned_agent_id: &OwnedAgentId) {}

    fn claimed(&self, _owned_agent_id: &OwnedAgentId, _started: bool) {}

    fn observe_result(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _result: BoxFuture<'static, Result<(), WorkerExecutorError>>,
    ) {
    }

    fn unload_deadline(&self, _owned_agent_id: &OwnedAgentId, deadline: Instant) -> Instant {
        deadline
    }

    async fn before_filesystem_cleanup(&self, _owned_agent_id: &OwnedAgentId) {}

    async fn before_stage(
        &self,
        owned_agent_id: &OwnedAgentId,
        stage: WorkerDeletionStage,
    ) -> Result<(), WorkerExecutorError>;
}

#[derive(Debug)]
struct DeletingWorker {
    runtime: Box<WorkerInstance>,
    handle: DeletionHandle,
    completed_stage: WorkerDeletionStage,
}

/// Represents worker that may be running or suspended.
///
/// It is responsible for receiving incoming worker invocations in a non-blocking way,
/// persisting them and also making sure that all the enqueued invocations eventually get
/// processed, in the same order as they came in.
///
/// Invocations have an associated idempotency key used to ensure that the same invocation
/// is not processed multiple times.
///
/// If the queue is empty, the service can trigger invocations directly as an optimization.
///
/// Every worker invocation should be done through this service.
pub struct Worker<Ctx: WorkerCtx> {
    owned_agent_id: OwnedAgentId,
    deps: All<Ctx>,
    card_interest_index: Arc<CardInterestIndex>,
    instance: Arc<Mutex<WorkerInstance>>,
    unload_cleanup: StdMutex<Option<invocation_loop::UnloadCleanup>>,
    resolved: OnceCell<ResolvedWorkerData<Ctx>>,
    initialization_complete: AtomicBool,
    initialization_request: StdMutex<Option<(InvocationContextStack, Principal)>>,
}

/// Metadata-dependent worker fields installed atomically after acquisition resolves.
///
/// This is public only because it is the target of `Worker`'s public `Deref` implementation; its
/// fields remain private and it is not a construction or lifecycle API.
#[doc(hidden)]
pub struct ResolvedWorkerData<Ctx: WorkerCtx> {
    owner_context: ResolvedOwnerContext,
    parsed_agent_id: Option<ParsedAgentId>,

    oplog: Arc<dyn Oplog>,
    pub(crate) tasks: tasks::WorkerTasks,
    worker_event_service: Arc<dyn WorkerEventService + Send + Sync>,

    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    /// How each not-yet-completed external invocation should be related to the
    /// trace of whatever enqueued it, so the invocation loop can attach the
    /// invocation's spans correctly when it picks the work up.
    ///
    /// Holds captured origins rather than `tracing::Span`s: a durable invocation can
    /// be picked up long after the request that enqueued it returned. See
    /// [`TraceOrigin`].
    external_invocation_origins: Arc<RwLock<HashMap<IdempotencyKey, TraceOrigin>>>,

    hydrated_invocation_results: Arc<RwLock<HydratedInvocationResultCache>>,
    ephemeral_invocation: StdMutex<EphemeralInvocationState>,
    initial_worker_metadata: AgentMetadata,
    resource_entry: Arc<AtomicResourceEntry>,
    registered_concurrent_account: RegisteredConcurrentAccount,
    /// The published worker status. Read lock-free from any context; written only by the
    /// worker-state actor's status task (and during construction, before the actor exists).
    last_known_status: Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
    /// Shared with the worker-state actor's status task, which records status transitions on it.
    /// Held here so the by-status worker count gauge stays incremented for exactly as long as
    /// this worker exists (its `Drop` decrements the gauge).
    #[allow(dead_code)]
    metrics_status: Arc<WorkerStatusMetric>,
    last_known_status_detached: Arc<AtomicBool>,
    status_flusher: Arc<status_flusher::AgentStatusFlusher>,
    status_checkpointer: status_checkpointer::StatusCheckpointer,
    // Note: std lock for wasmtime reasons
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    /// Owns the commit + status-fold transaction and the fire-and-forget lifecycle jobs
    /// (invocation-loop notification, memory-growth persistence). See [`state_actor`] for the
    /// concurrency invariants.
    state_actor: Arc<state_actor::WorkerStateActor<Ctx>>,
    owner_execution: Arc<OwnerExecution>,
    owner_runtime_resources: Arc<OwnerRuntimeResources>,
    /// Serializes permission-card event appends with the durable boundaries that consume them.
    card_event_boundary_lock: Arc<Mutex<()>>,
    /// Serializes public Durable Streams binding resolution with creation acceptance. The oplog
    /// remains authoritative; this lock only prevents concurrent requests from minting two live
    /// invocation keys for one public session ID.
    stream_session_creation_lock: Arc<Mutex<()>>,
    /// Release-published by the status actor after committed card authority
    /// entries are folded into worker status.
    published_authority_generation: Arc<AtomicU64>,

    /// Prevents weak-reference background work from starting while an unloaded
    /// worker is being conditionally removed from `ActiveAgents`.
    cache_retirement_in_progress: AtomicBool,
    startup_attempt: StartupAttemptTracker,
    linear_memory_grant: StdMutex<Option<Arc<StdMutex<MemoryGrant>>>>,
    /// Lifecycle request shared across resident worker generations. A terminal request is retained
    /// until the worker stops so permit reacquisition cannot lose it between `RunningWorker`s.
    interrupt_signal: Arc<Interrupts>,
    interrupt_establishment_tail: StdMutex<Arc<InterruptEstablishment>>,
    release_scope: StdMutex<crate::metrics::resource_release::ReleaseScope>,
    monthly_window: StdMutex<Option<Arc<monthly_limits::MonthlyWindowTarget>>>,
    monthly_stop: StdMutex<
        Option<(
            InterruptKind,
            crate::services::resource_limits::MonthlyResourceExhaustion,
        )>,
    >,
    runtime_append_tasks: Arc<StdMutex<Arc<tasks::RuntimeAppendTasks>>>,
    infrastructure_recovery_retry_config: RetryConfig,
    infrastructure_recovery_attempt: AtomicU32,
    oom_retry_config: RetryConfig,
    snapshot_policy: SnapshotPolicy,

    last_resume_request: Mutex<Timestamp>,
    /// The automatic snapshot entries that the starts of the agent exclude.
    snapshot_exclusions: StdMutex<SnapshotExclusions>,
    startup_linear_memory_bytes: AtomicU64,
    memory_growth: StdMutex<Arc<PendingMemoryGrowth>>,
    memory_limit_interrupt_queued: AtomicBool,

    /// Snapshot of the active component, refreshed by `create_instance`.
    /// Used by the read-only cache lookup without taking the wasm `Store`
    /// lock and while the worker is `Unloaded`.
    current_component: Arc<arc_swap::ArcSwap<golem_service_base::model::component::Component>>,

    /// Per-worker read-only method result cache. See
    /// [`crate::worker::read_only_cache`] for the design notes.
    read_only_cache: golem_common::cache::Cache<
        read_only_cache::ReadOnlyCacheKey,
        read_only_cache::PendingReadOnlyCacheEntry,
        Arc<read_only_cache::ReadOnlyCacheEntry>,
        WorkerExecutorError,
    >,

    /// Participates in the read-only cache key. Bumped before any mutating
    /// invocation's pending oplog entry becomes visible, so stale entries
    /// are invalidated lazily on the next lookup.
    read_only_cache_epoch: Arc<AtomicU64>,
    durable_stream_producer: Arc<durable_stream_producer::DurableStreamProducerSlot>,
    export_fork_receipt: tokio::sync::OnceCell<
        Option<golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotSuccess>,
    >,
    owner_retirement: Arc<std::sync::OnceLock<OwnerRetirement>>,
    owner_cleanup: Mutex<OwnerCleanupState>,
    owner_retirement_requested: CancellationToken,
    /// Why the shard of the worker moved to another executor, once it has: the only store of the
    /// lost shard. The status flusher, the status checkpointer and an upload of a filesystem
    /// snapshot read it through receivers.
    lost_shard: tokio::sync::watch::Sender<Option<RetirementReason>>,
    durable_stream_attachment_reconciler: DurableStreamAttachmentReconciler,
    durable_topology_recovery: Arc<Mutex<DurableTopologyRecoveryCache>>,
}

#[derive(Default, PartialEq, Eq)]
enum OwnerCleanupState {
    #[default]
    PreRemoval,
    Retired,
}

/// A receiver of why the shard of a worker moved to another executor, once it has.
pub(crate) type LostShard = tokio::sync::watch::Receiver<Option<RetirementReason>>;

/// Why this executor stops owning an agent.
#[derive(Clone, Debug)]
pub(crate) enum RetirementReason {
    /// Asked for: an interrupt through the Golem API, or an environment unload.
    Requested,
    /// The oplog refused a write, because another executor claimed it at a newer epoch. `None`
    /// when the refusal arrived without its epochs, as a `ShardLost` trap.
    Fenced(Option<OplogFence>),
    /// The shard manager revoked the shard.
    ShardRevoked,
    /// A delivered assignment no longer holds the shard, or holds it at a newer epoch.
    ShardNotAssigned,
}

/// Why this worker retires as its owner, and the shared stop that carries it out. A fence found
/// under the worker lifecycle lock records the reason there; the stop starts once
/// `interrupt_and_retire` runs.
///
/// The interrupt is recorded once (the first wins), but the shard can be lost after that: an API
/// interrupt or an environment unload may already be retiring the agent when its shard moves, and
/// its waiters must still be sent to the new owner. So the lost shard is not part of this record:
/// it is the watch of the worker, set once and later than the rest when it has to be.
pub(super) struct OwnerRetirement {
    kind: InterruptKind,
    stop: tokio::sync::OnceCell<
        futures::future::Shared<
            futures::future::BoxFuture<'static, Result<(), WorkerExecutorError>>,
        >,
    >,
}

impl<Ctx: WorkerCtx> std::ops::Deref for Worker<Ctx> {
    type Target = ResolvedWorkerData<Ctx>;

    fn deref(&self) -> &Self::Target {
        self.resolved
            .get()
            .expect("worker operation requires completed initialization")
    }
}

#[derive(Default)]
pub(crate) struct DurableTopologyRecoveryCache {
    initialized: bool,
    covered_through: OplogIndex,
    consumer_deleting: bool,
    pub(crate) sessions: HashMap<StreamSessionKey, SessionControlMetadata>,
    pub(crate) dirty: HashSet<StreamSessionKey>,
}

/// Owns event-driven stream recovery and failed-operation retries so retirement can join them.
struct DurableStreamAttachmentReconciler {
    shutdown: CancellationToken,
    handle: Mutex<Option<JoinHandle<()>>>,
}

struct WorkerDurableStreamConsumerJournal<Ctx: WorkerCtx> {
    state_actor: Arc<state_actor::WorkerStateActor<Ctx>>,
    status: Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
    worker_service: Arc<dyn WorkerService>,
    owner: OwnedAgentId,
    mode: AgentMode,
    fingerprint: AgentFingerprint,
}

impl<Ctx: WorkerCtx> HasOplog for Worker<Ctx> {
    fn oplog(&self) -> Arc<dyn Oplog> {
        self.oplog.clone()
    }
}

impl<Ctx: WorkerCtx> UsesAllDeps for Worker<Ctx> {
    type Ctx = Ctx;

    fn all(&self) -> &All<Self::Ctx> {
        &self.deps
    }
}

/// An invocation hydrated from this owner's committed Pending entry. The payload is retained
/// unchanged for Started; lowering may transform only a separate invocation clone.
pub struct HydratedInvocation {
    pub(crate) invocation: AgentInvocation,
    pub(crate) payload: OplogPayload<AgentInvocationPayload>,
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) fn is_resolved(&self) -> bool {
        self.initialization_complete.load(Ordering::Acquire)
    }

    pub(crate) fn can_discard_unresolved(&self) -> bool {
        self.instance.try_lock().is_ok_and(|instance| {
            matches!(&*instance, WorkerInstance::Unresolved)
                && self.initialization_request.lock().unwrap().is_none()
        })
    }

    /// Builds the span context this worker's phase spans share.
    fn trace(&self, startup_origin: TraceOrigin) -> WorkerTrace {
        WorkerTrace {
            startup_origin,
            agent_type: self.agent_type_label(),
        }
    }

    /// The agent type as a span field value, `-` for an agent id that could not be
    /// parsed into one. Shared so every span labels it the same way.
    pub(crate) fn agent_type_label(&self) -> String {
        self.parsed_agent_id
            .as_ref()
            .map(|id| id.agent_type.to_string())
            .unwrap_or_else(|| "-".to_string())
    }

    /// Whether `cell` is this worker's published status. Each generation shares its cell with its
    /// own worker-state actor and nothing else, so the cell identifies the generation to code that
    /// holds it but not the worker.
    pub(crate) fn shares_status_cell(
        &self,
        cell: &Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
    ) -> bool {
        Arc::ptr_eq(&self.last_known_status, cell)
    }

    pub fn agent_id(&self) -> AgentId {
        self.owned_agent_id.agent_id()
    }

    pub(crate) fn component_id(&self) -> ComponentId {
        self.owned_agent_id.component_id()
    }

    pub fn owner_execution(&self) -> Arc<OwnerExecution> {
        self.owner_execution.clone()
    }

    pub fn owner_runtime_resources(&self) -> Arc<OwnerRuntimeResources> {
        self.owner_runtime_resources.clone()
    }

    pub fn oom_retry_config(&self) -> &RetryConfig {
        &self.oom_retry_config
    }

    pub(crate) fn next_infrastructure_recovery_delay(&self) -> Duration {
        let attempt = self
            .infrastructure_recovery_attempt
            .fetch_add(1, Ordering::AcqRel);
        let capped_attempt = attempt.min(
            self.infrastructure_recovery_retry_config
                .max_attempts
                .saturating_sub(1),
        );
        get_delay(&self.infrastructure_recovery_retry_config, capped_attempt)
            .unwrap_or(self.infrastructure_recovery_retry_config.max_delay)
    }

    pub(crate) fn reset_infrastructure_recovery_backoff(&self) {
        self.infrastructure_recovery_attempt
            .store(0, Ordering::Release);
    }

    pub(crate) fn snapshot_policy(&self) -> &SnapshotPolicy {
        &self.snapshot_policy
    }

    pub fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.worker_event_service.clone()
    }

    pub fn is_loading(&self) -> bool {
        matches!(
            *self.execution_status.read().unwrap(),
            ExecutionStatus::Loading { .. }
        )
    }

    pub fn get_initial_worker_metadata(&self) -> AgentMetadata {
        self.initial_worker_metadata.clone()
    }

    pub(crate) fn owned_agent_id(&self) -> &OwnedAgentId {
        &self.owned_agent_id
    }

    pub fn runtime_append_tasks(&self) -> Arc<tasks::RuntimeAppendTasks> {
        self.runtime_append_tasks.lock().unwrap().clone()
    }

    pub fn agent_mode(&self) -> AgentMode {
        self.execution_status.read().unwrap().agent_mode()
    }

    /// Gets the timestamp of the last time the execution status changed
    pub fn last_execution_state_change(&self) -> Timestamp {
        self.execution_status.read().unwrap().timestamp()
    }

    pub(crate) fn release_scope(&self) -> crate::metrics::resource_release::ReleaseScope {
        self.release_scope.lock().unwrap().clone()
    }
}

#[derive(Debug)]
struct WorkerStatusMetric {
    status: StdMutex<AgentStatus>,
}

impl WorkerStatusMetric {
    fn new(status: AgentStatus) -> Self {
        crate::metrics::workers::inc_worker_count_by_status(status);
        Self {
            status: StdMutex::new(status),
        }
    }

    fn status(&self) -> AgentStatus {
        *self.status.lock().expect("metrics status lock poisoned")
    }

    fn update(&self, previous_status: AgentStatus, current_status: AgentStatus) {
        let mut status = self.status.lock().expect("metrics status lock poisoned");
        debug_assert_eq!(*status, previous_status);
        crate::metrics::workers::record_worker_status_transition(previous_status, current_status);
        *status = current_status;
    }
}

impl Drop for WorkerStatusMetric {
    fn drop(&mut self) {
        crate::metrics::workers::dec_worker_count_by_status(self.status());
    }
}

#[derive(Debug)]
enum WorkerInstance {
    Unresolved,
    Initializing(Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>),
    Unloaded {
        startup_failure: Option<WorkerExecutorError>,
    },
    CleanupFailed(WorkerExecutorError),
    WaitingForPermit(WaitingWorker),
    Running(RunningWorker),
    Stopping(StoppingWorker),
    StoppedForDeletion,
    Deleting(DeletingWorker),
}

impl WorkerInstance {
    fn startup_failure(&self) -> Option<&WorkerExecutorError> {
        match self {
            Self::Unloaded {
                startup_failure: Some(err),
            }
            | Self::CleanupFailed(err) => Some(err),
            _ => None,
        }
    }
}

/// What a resident worker's phase spans need in order to be traceable: the startup
/// they link back to, and the agent fields they carry.
///
/// Built while the worker is admitted and passed to the loop rather than captured
/// there, because the loop task has no ambient span by design.
#[derive(Debug, Clone)]
pub struct WorkerTrace {
    pub startup_origin: TraceOrigin,
    pub agent_type: String,
}

#[derive(Debug)]
struct WaitingWorker {
    handle: Option<JoinHandle<()>>,
    start_attempt: Uuid,
}

#[derive(Debug)]
struct InterruptEstablishment {
    predecessor: StdMutex<Option<Arc<InterruptEstablishment>>>,
    complete: AtomicBool,
    completed: tokio::sync::Notify,
}

#[derive(Clone, Debug)]
struct PendingWorkerInterrupt {
    kind: InterruptKind,
    reacquire_permits: bool,
    unload_request: UnloadRequest,
    establishment: Arc<InterruptEstablishment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnloadReason {
    Deleting,
    ExplicitStop,
    Failure,
    FilesystemLimit,
    FilesystemPressure,
    Idle,
    Interrupt,
    MemoryLimit,
    MemoryPressure,
    OutOfMemory,
    Panic,
    Restart,
    ShardLost,
    Suspend,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnloadRequest {
    pub(crate) reason: UnloadReason,
    pub(crate) deadline: Instant,
}

#[derive(Debug, Default)]
enum WorkerInterruptState {
    #[default]
    Idle,
    Pending(PendingWorkerInterrupt),
    /// A terminal request has been taken by the invocation loop and remains authoritative until
    /// that worker generation stops. This prevents a late permit-reacquisition restart from
    /// superseding an interrupt that is already being handled.
    TerminalClaimed,
}

#[derive(Debug)]
struct RunningWorker {
    handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<WorkerCommand>,
    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    waiting_for_command: Arc<AtomicBool>,
    concurrent_agent_permit_held: Arc<AtomicBool>,
    filesystem_activity: Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    /// The filesystem snapshots of the generation that runs.
    filesystem_snapshot_slot: filesystem_snapshots::SnapshotSlot,
    unload_request: Arc<StdMutex<Option<UnloadRequest>>>,
    idle_since_millis: Arc<AtomicU64>,
    interrupts: Arc<Interrupts>,
    /// `ResumeReplay` is signalled directly through the command channel rather
    /// than the internal queue, so eviction must treat it as pending work.
    resume_replay_pending: Arc<AtomicBool>,
    start_attempt: Uuid,
    owner_generation_started: bool,
}

struct RunningAgent<Runtime, Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    runtime: Runtime,
    filesystem: ResidentFilesystem<Adapter>,
}

struct RunningAgentRuntime<Ctx: WorkerCtx> {
    instance: Instance,
    store: ObservedPrimaryStore<async_lock::Mutex<Store<Ctx>>>,
}

pub(crate) struct ObservedPrimaryStore<T> {
    store: Option<T>,
    receipts: Option<(
        crate::metrics::resource_release::ReleaseReceipt,
        crate::metrics::resource_release::ReleaseReceipt,
    )>,
}

type WorkerRunningAgent<Ctx> = RunningAgent<RunningAgentRuntime<Ctx>>;

/// The filesystem of a start: the plan of its baseline, made before the agent filesystem exists,
/// and the restore that [`StartFilesystem::materialize`] applies to the new filesystem.
///
/// The two steps are apart because the start builds the filesystem between them, with the
/// initial files of the replay revision that the selection gives. The replay inputs come from the
/// same plan as the restore.
struct StartFilesystem {
    selection: StartSelection,
    plan: filesystem_snapshots::StartPlan,
    restore: Option<filesystem_snapshots::StartRestore>,
}

/// Why a start could not plan its filesystem.
enum StartFilesystemError {
    /// The baseline names a filesystem snapshot, and this executor keeps none.
    Disabled,
    Other(WorkerExecutorError),
}

/// A materialization of a start baseline that failed, with the filesystem to clean up, the error
/// of the agent filesystem and the replay inputs of the start.
struct StartFilesystemFailure {
    filesystem: SealedFilesystem,
    error: crate::services::agent_filesystem::Error,
    replay: filesystem_snapshots::ReplayBaseline,
}

pub(crate) enum CreateWorkerInstanceError {
    ReacquireMemory,
    Failed {
        error: WorkerExecutorError,
        filesystem_cleanup_failure: Option<UnloadCleanupFailure>,
    },
}

struct LinearMemoryGrantRegistration<Ctx: WorkerCtx> {
    worker: Arc<Worker<Ctx>>,
    grant: Arc<StdMutex<MemoryGrant>>,
}

struct FilesystemGenerationPublication {
    resources: Arc<OwnerRuntimeResources>,
    clear_on_drop: bool,
}

/// Classification of a loaded worker for eviction ordering.
///
/// Under memory/filesystem pressure, workers are evicted in priority order:
/// 1. `LoadedIdle` — no pending work, lowest cost to evict.
/// 2. `WarmRunnable` — has durable pending invocations but is not actively
///    executing. Evicting requires oplog recovery on next start, so it is the
///    expensive fallback path.
///
/// Workers with non-durable in-memory work (internal queue, `ResumeReplay`,
/// interrupt) or that are actively executing are never evictable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionClass {
    /// Resident in memory, not executing, no durable pending work.
    LoadedIdle,
    /// Resident in memory, not executing, has durable pending invocations.
    WarmRunnable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvictionStopOutcome {
    Ineligible,
    Unloaded,
    CleanupFailed,
}

pub(crate) struct WorkerCacheRetirement<'a> {
    in_progress: &'a AtomicBool,
    changed: &'a tokio::sync::Notify,
    committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilesystemPressureEligibility {
    idle_since: u64,
    last_effect_completion: u64,
}

#[derive(Debug)]
pub(crate) enum FinalWorkerState {
    Unloaded {
        startup_failure: Option<WorkerExecutorError>,
    },
    CleanupFailed(WorkerExecutorError),
    Deleting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingLiveInvocationDisposition {
    Fail,
    Preserve,
}

#[derive(Debug)]
struct StoppingWorker {
    notify: OneShotEvent,
    final_state: FinalWorkerState,
    pending_live_invocations: PendingLiveInvocationDisposition,
}

#[derive(Debug, Clone)]
struct FailedInvocationResult {
    pub trap_type: TrapType,
    pub stderr: String,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
enum InvocationResult {
    Cached {
        result: Result<AgentInvocationOutput, FailedInvocationResult>,
    },
    Lazy {
        oplog_idx: OplogIndex,
    },
}

/// Bounded cache of invocation result payloads loaded from the oplog. The status membership only
/// stores result oplog indexes; this cache avoids repeatedly loading and decoding those entries.
struct HydratedInvocationResultCache {
    values: HashMap<IdempotencyKey, HydratedInvocationResultCacheEntry>,
    insertion_order: VecDeque<IdempotencyKey>,
    capacity: usize,
    /// Committed completions the published status may not include yet, by the oplog index of
    /// their result. The early commit receipt publishes a completion before its fold, and a failed
    /// fold leaves the status detached with the key still current. Payload eviction must not make
    /// such a key look unresolved, or terminal cleanup would fail an invocation that succeeded.
    unprojected_completions: HashMap<IdempotencyKey, UnprojectedCompletion>,
}

struct UnprojectedCompletion {
    revert_generation: u64,
    oplog_idx: OplogIndex,
}

struct HydratedInvocationResultCacheEntry {
    result: Arc<InvocationResult>,
    /// Oplog branch generation in which this result was produced.
    revert_generation: u64,
    oplog_idx: OplogIndex,
}

#[cfg(test)]
mod tests;

/// What follows a failed invocation.
#[derive(Clone, Debug)]
pub enum InvocationFailureDecision {
    /// Retry the invocation, or stop, as the retry decision says.
    Retry(RetryDecision),
    /// The oplog refused the record of the failure. A `RecoveryRequired` error, a storage that did
    /// not answer, sends the agent to infrastructure recovery, and the invocation stays pending, so
    /// it continues when the storage answers again. An `OplogFailed` error stops the agent, and the
    /// stop gives the error, with its cause, to the waiting callers.
    Infrastructure(WorkerExecutorError),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum RetryDecision {
    /// Immediately retry by recreating the instance using the existing permits
    Immediate,
    /// Retry after a given delay by recreating the instance using the existing permits
    Delayed(Duration),
    /// No retry possible
    None,
    /// Try to stop if the worker does not get any resume request after the given timestamp,
    /// but allow resuming if needed (unlike with None)
    TryStop(Timestamp),
    /// Retry immediately but drop and reacquire permits
    ReacquirePermits,
}

struct ResolvedAgentProperties {
    agent_mode: AgentMode,
    snapshot_policy: SnapshotPolicy,
}

#[derive(Debug)]
enum WorkerCommand {
    WorkAvailable,
    InternalStatusChanged,
    ResumeReplay,
    UpdateFilesystemLimit {
        allocated_bytes: u64,
        sender: oneshot::Sender<Result<(), WorkerExecutorError>>,
    },
}

#[derive(Debug)]
pub enum QueuedWorkerInvocation {
    GetFileSystemNode {
        path: CanonicalFilePath,
        sender: oneshot::Sender<Result<GetFileSystemNodeResult, WorkerExecutorError>>,
    },
    GetWalletCards {
        sender: oneshot::Sender<Result<Vec<StoredCard>, WorkerExecutorError>>,
    },
    // Holds resident execution ownership until production has copied all selected bytes into the
    // bounded response, or until cancellation/failure.
    ReadFile {
        path: CanonicalFilePath,
        selection: FileByteSelection,
        sender: tokio::sync::oneshot::Sender<Result<FileReadResponse, FileReadError>>,
    },
    // Waits for the invocation loop to pick up this message, ensuring that the worker is ready to process followup commands.
    // The sender will be called with Ok if the worker is in a running state.
    // If the worker initialization fails and will not recover without manual intervention, it will be called with Err.
    AwaitReadyToProcessCommands {
        sender: oneshot::Sender<Result<(), WorkerExecutorError>>,
    },
    SaveSnapshot,
}

impl QueuedWorkerInvocation {
    /// Only transient commands are abandoned on disconnect; durable invocations live in the oplog.
    fn is_abandoned(&self) -> bool {
        match self {
            Self::ReadFile { sender, .. } => sender.is_closed(),
            Self::GetFileSystemNode { sender, .. } => sender.is_canceled(),
            Self::GetWalletCards { sender } => sender.is_canceled(),
            Self::AwaitReadyToProcessCommands { sender } => sender.is_canceled(),
            Self::SaveSnapshot => false,
        }
    }

    fn fail(self, error: &WorkerExecutorError) {
        match self {
            Self::ReadFile { sender, .. } => {
                let _ = sender.send(Err(FileReadError::Lifecycle));
            }
            Self::GetFileSystemNode { sender, .. } => {
                let _ = sender.send(Err(error.clone()));
            }
            Self::GetWalletCards { sender } => {
                let _ = sender.send(Err(error.clone()));
            }
            Self::AwaitReadyToProcessCommands { sender } => {
                let _ = sender.send(Err(error.clone()));
            }
            Self::SaveSnapshot => {}
        }
    }
}

#[derive(Clone, Copy)]
enum DurableStreamingAcceptanceMatch {
    ExactAttempt,
    StreamSlotSession,
}

#[allow(clippy::large_enum_variant)]
pub enum ResultOrSubscription {
    Finished(Result<AgentInvocationOutput, WorkerExecutorError>),
    Pending(EventsSubscription),
}

pub(crate) struct GetOrCreateWorkerResult {
    pub(crate) initial_worker_metadata: AgentMetadata,
    pub(crate) current_status: Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
    /// The status value currently persisted in the live cache, used as the first delta baseline.
    persisted_status: Option<AgentStatusRecord>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    owner_context: ResolvedOwnerContext,
    snapshot_policy: SnapshotPolicy,
    pub(crate) oplog: Arc<dyn Oplog>,
    /// Loaded during `get_or_create_worker_metadata` and stored on the
    /// [`Worker`] so the read-only cache can resolve metadata without a new
    /// `component_service` lookup.
    initial_component: Arc<golem_service_base::model::component::Component>,
    /// Ephemeral agents are fail-stop: reconstructing one from its lower oplog is allowed for
    /// observation and invocation-result lookup, but the instance must never be started again.
    reconstructed_ephemeral: bool,
    newly_created: bool,
}

pub(crate) const INACTIVE_EPHEMERAL_AGENT_ERROR: &str =
    "An ephemeral agent cannot accept another invocation or be resumed";

#[derive(Debug)]
enum EphemeralInvocationState {
    Available,
    Accepted(Option<IdempotencyKey>),
}

#[derive(Debug)]
enum StopResult {
    AlreadyStopping {
        notify: OneShotEvent,
    },
    Stopped,
    NeedsWaitForLoopExit {
        run_loop_handle: JoinHandle<()>,
        notify: OneShotEvent,
    },
}

#[cfg(test)]
mod resource_release_tests;

pub use acquisition::merge_agent_env_with_default_env;
pub(crate) use ingress::require_expected_tool_deployment_revision;
use ingress::{
    inactive_ephemeral_agent_error, replace_invocation_input, validate_resolved_invocation_identity,
};
use interrupt::register_interrupt_request;
use lifecycle::retirement::owned_shard_epoch;
pub(crate) use lifecycle::retirement::retired_by_assignment;
use lifecycle::stop::invocation_keys_to_fail;
use read_only_cache::{build_read_only_cache_entry, is_no_cache, populate_read_only_cache};
use runtime::shared_linear_memory_error;
use streaming::sessions::{
    durable_stream_attempt_error_outcome, persisted_stream_descriptor_matches,
    stream_attempt_matches, stream_output_consumer_is_observer, stream_slot_session_matches,
    validate_stream_session_record,
};
pub(crate) use streaming::sessions::{
    same_origin_stream_invocation, stream_session_record_key, stream_session_record_keys,
    stream_session_record_reference,
};
