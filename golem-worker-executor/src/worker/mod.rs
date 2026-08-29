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

pub mod agent_config;
pub mod cut_point;
pub mod entity_invocation;
pub mod entity_slot;
pub mod instance;
pub mod invocation;
mod invocation_loop;
mod lifecycle;
pub mod owner_lane;
pub mod read_only_cache;
mod state_actor;
pub mod status;
pub mod status_checkpointer;
pub mod status_flusher;

pub use lifecycle::UpdateMode as WorkerUpdateMode;

use self::agent_config::{
    effective_agent_config, ensure_required_agent_secrets_are_configured,
    parse_worker_creation_agent_config,
};
use crate::durable_host::durable_session::{DurableSessionStreams, DurableStreamConsumerJournal};
use crate::durable_host::durable_stream::{
    AttachedStreamSegmentSource, CommittedProducerStreamEventV1, ConsumerAttachmentStatus,
    DbDirectStreamAttachmentConsumerProbe, DurableStreamCommit, DurableStreamProducer,
    ProducerRegistrationRequestV1, RoutedStreamAttachmentControl, StreamAttachmentConsumerProbe,
    StreamAttachmentControl,
};
use crate::durable_host::schema_value_stream::contains_stream;
use crate::durable_host::{
    agent_effective_surface_from_component_metadata, agent_monomorphization_context,
    recover_stderr_logs,
};
use crate::metrics::workers::AdmissionPhase;
use crate::model::{AgentConfig, ExecutionStatus, LookupResult, ReadFileResult, TrapType};
use crate::sandbox_filesystem::{SandboxFilesystem, SandboxFilesystemAdapter};
use crate::services::active_agents::{
    MemoryGrant, RegisteredConcurrentAccount, WorkerComponentCharge,
};
use crate::services::agent_filesystem::{
    AccessMode, FilesystemGenerationHandle, Follow, ObjectKind, OpenOptions, PathTarget,
    ReconstructingFilesystem, ResidentFilesystem, ResidentFilesystemActivity, SealedFilesystem,
    abort_reconstruction, bind_configured_resource_usage_metering,
    delete as delete_agent_filesystem, delete_created, finish_reconstruction, finish_replay,
    materialize_initial_files, open as open_agent_filesystem, open_resource_usage_window,
    prepare_initial_files, provision_initial_files, reconstruction_generation_handle,
    resident_generation_handle,
};
use crate::services::card_interest::CardInterestIndex;
use crate::services::events::{Event, EventsSubscription};
use crate::services::golem_config::SnapshotPolicy;
use crate::services::linear_memory::{LinearMemoryTracker, SHARED_LINEAR_MEMORY_ERROR};
use crate::services::oplog::plugin::ForwardingOplog;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, downcast_oplog};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::ResourceUsageAccount;
use crate::services::worker::GetWorkerMetadataResult;
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveAgents, HasAgentTypesService, HasAgentWebhooksService, HasAll,
    HasBlobStoreService, HasCardService, HasComponentService, HasConfig,
    HasEnvironmentStateService, HasEvents, HasExtraDeps, HasFileLoader, HasHttpConnectionPool,
    HasKeyValueService, HasOplog, HasOplogService, HasPromiseService, HasQuotaService,
    HasRdbmsService, HasResourceLimits, HasRpc, HasSchedulerService, HasShardService,
    HasWasmtimeEngine, HasWebSocketConnectionPool, HasWorkerEnumerationService,
    HasWorkerForkService, HasWorkerProxy, HasWorkerService, UsesAllDeps,
};
use crate::worker::instance::{OwnerExecution, OwnerRuntimeResources};
use crate::worker::invocation_loop::{
    ConcurrentAgentPermitState, InvocationLoop, run_invocation_loop_task,
};
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::{WorkerCtx, WorkerFilesystemContext};
use futures::channel::oneshot;
use golem_common::base_model::agent::CachePolicy;
use golem_common::base_model::durable_stream::{
    AttachedStreamSegmentRequestV1, DURABLE_STREAM_FORMAT_VERSION,
    PersistedStreamInvocationDescriptorV1, ResumeAttemptDescriptorV1, SessionStreamRoleV1,
    StartAttemptDescriptorV1, StreamAttachmentControlOperationV1, StreamAttachmentControlRequestV1,
    StreamAttachmentFinalizationReasonV1, StreamAttachmentKeyV1, StreamConsumerDeletingRecordV1,
    StreamSessionAttachedRecordV1, StreamSessionMappingRecordV1, StreamSessionPreparedRecordV1,
    StreamSessionRecordV1, StreamSessionResumeAttemptRecordV1,
};
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::base_model::oplog::QueuedCardEvent;
use golem_common::cache::SimpleCache;
use golem_common::model::AgentStatus;
use golem_common::model::RetryConfig;
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, ParsedAgentId, Principal, Snapshotting,
    SnapshottingConfig, ephemeral_invocation_phantom_id,
};
use golem_common::model::card::{CardId, StoredCard, card_matches_agent_recipient};
use golem_common::model::component::CanonicalFilePath;
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::entity::{ExecutableTarget, FilesystemCapability, OwnerRuntime};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    AgentError, OplogEntry, OplogIndex, OplogPayload, TimestampedUpdateDescription,
    UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::worker::{
    AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget, TypedAgentConfigEntry,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult,
    AgentMetadata, AgentStatusRecord, IdempotencyKey, OwnedAgentId, PendingInvocationRef,
    PendingUpdateKind, PendingUpdateRef, Timestamp, TimestampedAgentInvocation,
};
use golem_common::one_shot::OneShotEvent;
use golem_common::read_only_lock;
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::GetFileSystemNodeResult;
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, MutexGuard, OnceCell, OwnedMutexGuard, RwLock};
use tokio::task::JoinHandle;
use tracing::{Instrument, Level, debug, info, span, warn};
use uuid::Uuid;
use wasmtime::Store;
use wasmtime::component::Instance;

pub const PERMISSION_CARD_TRANSFER_PAYLOAD_CONFLICT: &str =
    "permission card transfer payload conflict";
pub const PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH: &str = "install-recipient-mismatch";

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

pub(crate) struct DurableStreamingInvocationRequest {
    pub(crate) attempt: StartAttemptDescriptorV1,
    pub(crate) registrations: Vec<(u64, ProducerRegistrationRequestV1)>,
    pub(crate) foreign_mappings: Vec<StreamSessionMappingRecordV1>,
    pub(crate) input_schema: Arc<golem_schema::schema::SchemaGraph>,
    pub(crate) input_element_types: Vec<(u64, golem_schema::schema::SchemaType)>,
    pub(crate) invocation: AgentInvocation,
    pub(crate) acceptance_committed: tokio::sync::oneshot::Sender<()>,
}

pub(crate) struct DurableStreamingInvocationAcceptance {
    pub(crate) prepared: StreamSessionPreparedRecordV1,
    pub(crate) streams: DurableSessionStreams,
    pub(crate) replayed: bool,
}

pub(crate) struct DurableStreamingResumeAcceptance {
    pub(crate) prepared: StreamSessionPreparedRecordV1,
    pub(crate) mappings: Vec<StreamSessionMappingRecordV1>,
    pub(crate) streams: DurableSessionStreams,
    pub(crate) epoch: u64,
    pub(crate) replayed: bool,
}

/// `Ttl(0)` is folded in as it is equivalent to `NoCache`.
fn is_no_cache(policy: &CachePolicy) -> bool {
    match policy {
        CachePolicy::NoCache(_) => true,
        CachePolicy::Ttl(ttl) => ttl.duration_nanos == 0,
        CachePolicy::UntilWrite(_) => false,
    }
}

/// The component revision a starting worker should be charged/admitted against.
///
/// When a pending update is queued, `create_instance` instantiates the update's
/// `target_revision` rather than the last known revision, so admission must
/// reserve and key the component charge against the target. With no pending
/// update, the last known revision is the one that will be instantiated.
fn component_charge_revision(
    pending_target_revision: Option<ComponentRevision>,
    last_known_revision: ComponentRevision,
) -> ComponentRevision {
    pending_target_revision.unwrap_or(last_known_revision)
}

/// How a pending-update target's metadata-resolution outcome should drive the
/// startup component charge.
#[derive(Debug, PartialEq, Eq)]
enum TargetChargeAction {
    /// The target resolved: charge it with the resolved module size.
    ChargeTarget(ResolvedComponentCharge),
    /// The target does not exist: `create_instance` will fail the update and load
    /// the current revision, so charge the current revision instead.
    FallBackToCurrent,
    /// Resolution failed transiently: `create_instance` may still load the
    /// target, so retry rather than charging the current revision.
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedComponentCharge {
    module_bytes: u64,
    initial_linear_memory_bytes: u64,
    reserved_linear_memory_bytes: u64,
}

struct StartupComponentChargeRequirement {
    component_id: ComponentId,
    component_revision: ComponentRevision,
    module_bytes: u64,
    startup_linear_memory_bytes: u64,
    reserved_linear_memory_bytes: u64,
}

/// Classifies a `get_metadata(target)` result into the startup charge action,
/// preserving the invariant that admission charges the target revision whenever
/// `create_instance` can still load it. Only a definitely-absent target
/// (`ComponentNotFound`) falls back to the current revision; transient errors
/// are retried.
fn classify_target_charge(
    result: &Result<ResolvedComponentCharge, WorkerExecutorError>,
) -> TargetChargeAction {
    match result {
        Ok(charge) => TargetChargeAction::ChargeTarget(*charge),
        Err(WorkerExecutorError::ComponentNotFound { .. }) => TargetChargeAction::FallBackToCurrent,
        Err(_) => TargetChargeAction::Retry,
    }
}

fn startup_component_requirement(
    component_id: ComponentId,
    component_revision: ComponentRevision,
    module_bytes: u64,
    initial_linear_memory_bytes: u64,
    canonical_linear_memory_bytes: u64,
) -> StartupComponentChargeRequirement {
    StartupComponentChargeRequirement {
        component_id,
        component_revision,
        module_bytes,
        startup_linear_memory_bytes: canonical_linear_memory_bytes,
        reserved_linear_memory_bytes: canonical_linear_memory_bytes
            .max(initial_linear_memory_bytes),
    }
}

/// Inserts `output` into the cache under `epoch`, which must be the value
/// captured at enqueue (see
/// [`Worker::enqueue_worker_invocation_with_effect`]).
///
/// Performs a final epoch recheck against `read_only_cache_epoch` before
/// inserting and drops the populate if a mutating invocation has completed
/// in the meantime. This is the populate-time guard
/// for the "epoch is bumped on mutating completion, not enqueue" semantics.
///
/// Free function so the observer task does not pin the worker.
async fn populate_read_only_cache(
    cache: &golem_common::cache::Cache<
        read_only_cache::ReadOnlyCacheKey,
        (),
        Arc<read_only_cache::ReadOnlyCacheEntry>,
        WorkerExecutorError,
    >,
    read_only_cache_epoch: &AtomicU64,
    ro: &ReadOnlyContext,
    epoch: u64,
    output: AgentInvocationOutput,
) {
    // Stale-populate guard: if a mutating invocation has completed (and bumped
    // the epoch) between the read-only enqueue and now, do not store this
    // result. It would otherwise sit under the pre-mutation epoch and be
    // unreachable, but storing it would still let a future epoch wrap hit it
    // (defensive).
    if read_only_cache_epoch.load(Ordering::SeqCst) != epoch {
        return;
    }

    let principal_ref = if ro.cfg.uses_principal {
        Some(&ro.principal)
    } else {
        None
    };
    let key = read_only_cache::build_read_only_cache_key(
        &ro.method_name,
        &ro.input,
        principal_ref,
        ro.component_revision,
        epoch,
    );
    let entry = build_read_only_cache_entry(ro, output);
    // First-writer-wins.
    let _ = cache
        .get_or_insert_simple(&key, async move || Ok::<_, WorkerExecutorError>(entry))
        .await;
}

/// Builds a [`ReadOnlyCacheEntry`] for the given [`AgentInvocationOutput`]
/// using `ro.cfg.cache_policy` to derive the optional TTL expiry. Shared by
/// the detached observer (see [`populate_read_only_cache`]) and the
/// `invoke_and_await` coalescing path so both produce identical entries.
fn build_read_only_cache_entry(
    ro: &ReadOnlyContext,
    output: AgentInvocationOutput,
) -> Arc<read_only_cache::ReadOnlyCacheEntry> {
    let expires_at = match &ro.cfg.cache_policy {
        CachePolicy::Ttl(ttl) => {
            tokio::time::Instant::now().checked_add(Duration::from_nanos(ttl.duration_nanos))
        }
        CachePolicy::UntilWrite(_) | CachePolicy::NoCache(_) => None,
    };
    Arc::new(read_only_cache::ReadOnlyCacheEntry { output, expires_at })
}

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

impl StartupAttemptTracker {
    fn begin(&self, existing: Option<Uuid>) -> Uuid {
        let mut state = self.state.lock().unwrap();
        let attempt = existing.or(state.pending).unwrap_or_else(Uuid::new_v4);
        state.pending = Some(attempt);
        state.failure = None;
        attempt
    }

    fn pending(&self) -> Option<Uuid> {
        self.state.lock().unwrap().pending
    }

    fn current(&self) -> Result<Option<Uuid>, WorkerExecutorError> {
        let state = self.state.lock().unwrap();
        match (state.pending, state.failure.as_ref()) {
            (Some(attempt), _) => Ok(Some(attempt)),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => Ok(None),
        }
    }

    fn complete(&self, attempt: Uuid, result: &Result<(), WorkerExecutorError>) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.pending != Some(attempt) {
            return false;
        }
        state.failure = result.as_ref().err().cloned();
        state.pending = None;
        true
    }

    fn complete_success_if_active(&self, attempt: Uuid, active_attempt: Option<Uuid>) -> bool {
        if active_attempt != Some(attempt) {
            return false;
        }
        self.complete(attempt, &Ok(()))
    }
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
    parsed_agent_id: Option<ParsedAgentId>,

    oplog: Arc<dyn Oplog>,
    worker_event_service: Arc<dyn WorkerEventService + Send + Sync>,

    deps: All<Ctx>,

    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    /// How each not-yet-completed external invocation should be related to the
    /// trace of whatever enqueued it, so the invocation loop can attach the
    /// invocation's spans correctly when it picks the work up.
    ///
    /// Holds captured origins rather than `tracing::Span`s: a durable invocation can
    /// be picked up long after the request that enqueued it returned. See
    /// [`TraceOrigin`].
    external_invocation_origins: Arc<RwLock<HashMap<IdempotencyKey, TraceOrigin>>>,

    invocation_results: Arc<RwLock<HashMap<IdempotencyKey, InvocationResult>>>,
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
    card_interest_index: Arc<CardInterestIndex>,
    /// Serializes permission-card event appends with the durable boundaries that consume them.
    card_event_boundary_lock: Arc<Mutex<()>>,
    /// Release-published by the status actor after committed card authority
    /// entries are folded into worker status.
    published_authority_generation: Arc<AtomicU64>,

    // IMPORTANT: Every external operation must acquire the instance lock, even briefly, to confirm the worker isn’t deleting.
    instance: Arc<Mutex<WorkerInstance>>,
    startup_attempt: StartupAttemptTracker,
    linear_memory_grant: StdMutex<Option<Arc<StdMutex<MemoryGrant>>>>,
    /// Lifecycle request shared across resident worker generations. A terminal request is retained
    /// until the worker stops so permit reacquisition cannot lose it between `RunningWorker`s.
    interrupt_signal: Arc<async_lock::Mutex<WorkerInterruptState>>,
    oom_retry_config: RetryConfig,
    snapshot_policy: SnapshotPolicy,

    last_resume_request: Mutex<Timestamp>,
    pub(crate) snapshot_recovery_disabled: AtomicBool,
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
        (),
        Arc<read_only_cache::ReadOnlyCacheEntry>,
        WorkerExecutorError,
    >,

    /// Participates in the read-only cache key. Bumped before any mutating
    /// invocation's pending oplog entry becomes visible, so stale entries
    /// are invalidated lazily on the next lookup.
    read_only_cache_epoch: Arc<AtomicU64>,
    durable_stream_producer: OnceCell<Arc<DurableStreamProducer>>,
}

struct WorkerDurableStreamConsumerJournal<Ctx: WorkerCtx> {
    state_actor: Arc<state_actor::WorkerStateActor<Ctx>>,
    worker_service: Arc<dyn crate::services::worker::WorkerService>,
    oplog_service: Arc<dyn crate::services::oplog::OplogService>,
}

#[async_trait::async_trait]
impl<Ctx: WorkerCtx> DurableStreamConsumerJournal for WorkerDurableStreamConsumerJournal<Ctx> {
    async fn commit(&self) -> Result<(), String> {
        let (_, changed) = self
            .state_actor
            .commit_and_update_state(CommitLevel::Always)
            .await;
        if changed {
            self.state_actor.notify_status_changed();
        }
        Ok(())
    }

    async fn source_unavailable(
        &self,
        key: &golem_common::model::durable_stream::StreamAttachmentKeyV1,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffsetV1>, String> {
        DbDirectStreamAttachmentConsumerProbe::new(
            self.worker_service.clone(),
            self.oplog_service.clone(),
        )
        .journal_inspection(key)
        .await
        .map(|inspection| inspection.and_then(|inspection| inspection.source_unavailable))
        .map_err(|error| error.to_string())
    }
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

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) fn durable_stream_consumer_journal(&self) -> Arc<dyn DurableStreamConsumerJournal> {
        Arc::new(WorkerDurableStreamConsumerJournal {
            state_actor: self.state_actor.clone(),
            worker_service: self.worker_service(),
            oplog_service: self.oplog_service(),
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

    pub(crate) async fn remove_from_active_agents(&self) {
        self.deps.active_agents().remove(&self.owned_agent_id).await;
    }

    /// Gets or creates a worker, but does not start it
    pub async fn get_or_create_suspended<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        Self::get_or_create_suspended_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            InvocationFreshnessDisposition::MayExist,
        )
        .await
    }

    pub async fn get_or_create_suspended_with_freshness<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        deps.active_agents()
            .get_or_add_with_freshness(
                deps,
                owned_agent_id,
                worker_env,
                worker_agent_config,
                component_revision,
                parent,
                invocation_context_stack,
                principal,
                freshness_disposition,
            )
            .await
    }

    /// Gets or creates a worker and makes sure it is running
    pub async fn get_or_create_running<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::get_or_create_running_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            InvocationFreshnessDisposition::MayExist,
        )
        .await
    }

    pub async fn get_or_create_running_with_freshness<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker = Self::get_or_create_suspended_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            freshness_disposition,
        )
        .await?;
        Self::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    pub async fn validate_invocation_freshness<T: HasComponentService + Sync>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: &IdempotencyKey,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<(), WorkerExecutorError> {
        let component = deps
            .component_service()
            .get_metadata(owned_agent_id.component_id(), None)
            .await?;
        let parsed_agent_id =
            match ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata) {
                Ok(parsed_agent_id) => parsed_agent_id,
                Err(_) if freshness_disposition == InvocationFreshnessDisposition::MayExist => {
                    return Ok(());
                }
                Err(err) => {
                    crate::metrics::ephemeral::record_known_fresh_validation_failure();
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "KnownFresh requires a valid ephemeral agent id: {err}"
                    )));
                }
            };
        let Some(agent_type) = component
            .metadata
            .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
        else {
            if freshness_disposition == InvocationFreshnessDisposition::MayExist {
                return Ok(());
            }
            crate::metrics::ephemeral::record_known_fresh_validation_failure();
            return Err(WorkerExecutorError::invalid_request(
                "KnownFresh can only be used for an ephemeral agent invocation",
            ));
        };

        if agent_type.mode == AgentMode::Ephemeral {
            crate::metrics::ephemeral::record_invocation_attempt(freshness_disposition);
        }
        let result = validate_resolved_invocation_identity(
            agent_type.mode,
            parsed_agent_id.phantom_id,
            idempotency_key,
            freshness_disposition,
        );
        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh && result.is_err() {
            crate::metrics::ephemeral::record_known_fresh_validation_failure();
        }
        result
    }

    pub async fn get_latest_metadata<T: HasAll<Ctx>>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentMetadata> {
        if let Some(worker) = deps.active_agents().try_get(owned_agent_id).await {
            Some(worker.get_latest_worker_metadata().await)
        } else if let Some(GetWorkerMetadataResult {
            mut initial_worker_metadata,
            last_known_status,
        }) = deps.worker_service().get(owned_agent_id).await
        {
            // update with latest data from oplog
            let agent_mode = initial_worker_metadata.agent_mode;
            let last_known_status = calculate_last_known_status_with_checkpoint(
                deps,
                owned_agent_id,
                agent_mode,
                last_known_status,
            )
            .await
            .expect("Failed to calculate worker status for worker even though it is initialized");

            initial_worker_metadata.last_known_status = last_known_status;

            Some(initial_worker_metadata)
        } else {
            None
        }
    }

    pub async fn new<T: HasAll<Ctx>>(
        deps: &T,
        card_interest_index: Arc<CardInterestIndex>,
        owned_agent_id: OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Self, WorkerExecutorError> {
        let start = std::time::Instant::now();
        let GetOrCreateWorkerResult {
            initial_worker_metadata,
            current_status,
            persisted_status,
            execution_status,
            agent_id,
            snapshot_policy,
            oplog,
            initial_component,
            reconstructed_ephemeral,
        } = match Self::get_or_create_worker_metadata(
            deps,
            &owned_agent_id,
            component_revision,
            worker_env,
            worker_agent_config,
            parent,
            freshness_disposition,
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                crate::metrics::wasm::record_create_worker_failure(&err);
                return Err(err);
            }
        };
        let oplog = Ctx::wrap_oplog(owned_agent_id.clone(), oplog, deps.extra_deps());

        let current_status_snapshot = current_status.load_full();
        let metrics_status = Arc::new(WorkerStatusMetric::new(current_status_snapshot.status));
        let initial_invocation_results = current_status_snapshot.invocation_results.clone();
        let last_oplog_idx = current_status_snapshot.oplog_idx;
        drop(current_status_snapshot);

        // Invocations already pending when this worker is loaded were enqueued by
        // an earlier request, possibly in an earlier process, so there is no
        // in-process originator to relate them to. They start with no origin rather
        // than being attributed to whichever request happened to trigger the load,
        // which is neither their true origin nor a span that outlives them.
        let queue = Arc::new(RwLock::new(VecDeque::new()));
        let external_invocation_origins = Arc::new(RwLock::new(HashMap::new()));

        let invocation_results = Arc::new(RwLock::new(HashMap::from_iter(
            initial_invocation_results.iter().map(|(key, oplog_idx)| {
                (
                    key.clone(),
                    InvocationResult::Lazy {
                        oplog_idx: *oplog_idx,
                    },
                )
            }),
        )));

        let instance = Arc::new(Mutex::new(WorkerInstance::Unloaded {
            startup_failure: reconstructed_ephemeral.then(inactive_ephemeral_agent_error),
        }));

        // Fetch the account's resource entry and register it with the
        // concurrent-agents semaphore. This must happen before WaitingWorker
        // can acquire a concurrent-agent permit so that the real plan limit
        // is enforced from the very first agent startup for this account.
        // Registration is idempotent — subsequent calls for the same account
        // on the same executor are instant (OnceCell cache hit in ResourceLimitsGrpc).
        let owner_account_id = initial_worker_metadata.created_by;
        let resource_entry = deps
            .resource_limits()
            .initialize_account(owner_account_id)
            .await?;
        let registered_concurrent_account = deps
            .active_agents()
            .register_account_concurrency(owner_account_id, resource_entry.clone())
            .await;

        let read_only_cache_cfg = &deps.config().read_only_cache;
        let read_only_cache = golem_common::cache::Cache::new(
            Some(read_only_cache_cfg.cache_capacity),
            golem_common::cache::FullCacheEvictionMode::LeastRecentlyUsed(1),
            golem_common::cache::BackgroundEvictionMode::OlderThan {
                ttl: read_only_cache_cfg.max_entry_age,
                period: read_only_cache_cfg.cache_eviction_interval,
            },
            "worker_read_only_cache",
        );

        let current_component = Arc::new(arc_swap::ArcSwap::from(initial_component));

        let last_known_status_detached = Arc::new(AtomicBool::new(false));
        let status_flusher = status_flusher::AgentStatusFlusher::new(
            owned_agent_id.clone(),
            initial_worker_metadata.agent_mode == AgentMode::Ephemeral,
            deps.config().agent_status_flush.enabled,
            deps.worker_service(),
            deps.active_agents().status_flush_queue(),
            persisted_status,
            current_status.clone(),
            last_known_status_detached.clone(),
        );

        let status_checkpointer = status_checkpointer::StatusCheckpointer::new(
            owned_agent_id.clone(),
            initial_worker_metadata.agent_mode == AgentMode::Ephemeral,
            deps.config().agent_status_checkpoint.enabled,
            deps.config().agent_status_checkpoint.min_oplog_delta,
            deps.worker_service(),
        );

        let all_deps = All::from_other(deps);
        // Start stale so a newly restored store must reconcile status/oplog
        // authority before it can use lock-free host-call authorization.
        let published_authority_generation = Arc::new(AtomicU64::new(1));

        let state_actor = Arc::new(state_actor::WorkerStateActor::new(
            all_deps.clone(),
            owned_agent_id.clone(),
            initial_worker_metadata.agent_mode,
            initial_worker_metadata.created_by,
            oplog.clone(),
            current_status.clone(),
            last_known_status_detached.clone(),
            metrics_status.clone(),
            status_flusher.clone(),
            published_authority_generation.clone(),
            instance.clone(),
        ));
        let owner_execution = Arc::new(OwnerExecution::new(
            owned_agent_id.clone(),
            oplog.clone(),
            state_actor.owner_commit_controller(),
        ));
        let owner_runtime_resources = Arc::new(OwnerRuntimeResources::new(
            Arc::clone(&resource_entry),
            execution_status.clone(),
        ));

        let worker = Worker {
            owned_agent_id,
            parsed_agent_id: agent_id.clone(),
            oplog,
            worker_event_service: Arc::new(WorkerEventServiceDefault::new(
                deps.config().limits.event_broadcast_capacity,
                deps.config().limits.event_history_size,
            )),
            deps: all_deps,
            queue,
            external_invocation_origins,
            invocation_results,
            ephemeral_invocation: StdMutex::new(if reconstructed_ephemeral {
                EphemeralInvocationState::Accepted(None)
            } else {
                EphemeralInvocationState::Available
            }),
            instance,
            startup_attempt: StartupAttemptTracker::default(),
            linear_memory_grant: StdMutex::new(None),
            interrupt_signal: Arc::new(async_lock::Mutex::new(WorkerInterruptState::default())),
            execution_status,
            initial_worker_metadata,
            resource_entry,
            registered_concurrent_account,
            last_known_status: current_status,
            metrics_status,
            card_interest_index,
            card_event_boundary_lock: Arc::new(Mutex::new(())),
            published_authority_generation,
            oom_retry_config: deps.config().memory.oom_retry_config.clone(),
            snapshot_policy,
            state_actor,
            owner_execution,
            owner_runtime_resources,
            last_known_status_detached,
            status_flusher,
            status_checkpointer,
            last_resume_request: Mutex::new(Timestamp::now_utc()),
            snapshot_recovery_disabled: AtomicBool::new(false),
            startup_linear_memory_bytes: AtomicU64::new(0),
            memory_growth: StdMutex::new(Arc::new(PendingMemoryGrowth::default())),
            memory_limit_interrupt_queued: AtomicBool::new(false),
            current_component,
            read_only_cache,
            read_only_cache_epoch: Arc::new(AtomicU64::new(0)),
            durable_stream_producer: OnceCell::new(),
        };

        // Wire the worker event service into the forwarding oplog so plugin errors
        // can be emitted as live events without writing to the oplog.
        if let Some(forwarding_oplog) = downcast_oplog::<ForwardingOplog>(&worker.oplog) {
            forwarding_oplog
                .set_worker_event_service(worker.worker_event_service.clone())
                .await;
        }

        // just some sanity checking
        assert!(last_oplog_idx >= OplogIndex::INITIAL);

        // if the worker is an agent, we need to ensure the initialize invocation is the first enqueued action.
        // We might have crashed between creating the oplog and writing it, so just check here for it.
        if let Some(agent_id) = &agent_id
            && last_oplog_idx <= OplogIndex::from_u64(2)
            && !reconstructed_ephemeral
        {
            let init_idempotency_key = IdempotencyKey::new(format!("init-{}", worker.agent_id()));
            let init_input = agent_id.parameters.value().clone();
            worker
                .enqueue_worker_invocation(AgentInvocation::AgentInitialization {
                    idempotency_key: init_idempotency_key,
                    input: init_input,
                    invocation_context: invocation_context_stack.clone(),
                    principal,
                })
                .await
                .expect("Failed enqueuing initial agent invocations to worker");
        };
        if Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS
            && !worker
                .durable_stream_producer()
                .await?
                .deletion_started()
                .await
        {
            worker.reconcile_durable_stream_attachments().await?;
            worker.recover_durable_stream_topologies().await?;
            worker.recover_finished_durable_streaming_sessions().await?;
        }
        crate::metrics::wasm::record_create_worker(start.elapsed());

        Ok(worker)
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

    pub(crate) async fn create_entity_context(
        self: &Arc<Self>,
        runtime: OwnerRuntime,
        filesystem: FilesystemCapability,
        executable_component: golem_service_base::model::component::Component,
        activation: Option<Arc<golem_common::model::entity::EntityActivation>>,
        owner_component_metadata: Arc<golem_service_base::model::component::Component>,
    ) -> Result<Ctx, WorkerExecutorError> {
        if !matches!(runtime, OwnerRuntime::Entity(_)) {
            return Err(WorkerExecutorError::runtime(
                "Entity context construction requires an entity runtime",
            ));
        }
        let worker_metadata = self.get_latest_worker_metadata().await;
        let agent_effective_surface = match &self.parsed_agent_id {
            Some(agent_id) => agent_effective_surface_from_component_metadata(
                &owner_component_metadata,
                &self.owned_agent_id,
                agent_id,
            )?,
            None => golem_common::model::card::EffectiveSurface::default(),
        };
        let executable_revision = executable_component.revision;
        let initial_agent_config = match &self.parsed_agent_id {
            Some(agent_id) => {
                let component_config = owner_component_metadata
                    .metadata
                    .agent_type_config(&agent_id.agent_type)
                    .map(|config| config.to_vec())
                    .unwrap_or_default();
                effective_agent_config(worker_metadata.config, component_config)?
                    .into_iter()
                    .map(|(path, value)| TypedAgentConfigEntry { path, value })
                    .collect()
            }
            None => worker_metadata.config,
        };
        let filesystem_generation = self
            .owner_runtime_resources
            .filesystem_generation_handle()?;
        if let Some(files) = activation
            .as_ref()
            .map(|activation| &activation.policy().provision().files)
            .filter(|files| !files.is_empty())
        {
            provision_initial_files(
                &filesystem_generation,
                self.file_loader(),
                self.owned_agent_id.environment_id,
                files.clone(),
            )
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        }
        let filesystem_context = create_filesystem_context(filesystem_generation).await?;
        let initial_linear_memory = executable_component.metadata.initial_linear_memory_bytes();
        if initial_linear_memory > self.resource_entry.max_memory_limit() as u64 {
            return Err(WorkerExecutorError::worker_creation_failed(
                self.agent_id(),
                format!(
                    "Linear memories require {initial_linear_memory} bytes, exceeding the per-agent limit of {} bytes",
                    self.resource_entry.max_memory_limit()
                ),
            ));
        }
        let retained_memory_grant = Arc::new(StdMutex::new(
            self.active_agents()
                .acquire_memory(initial_linear_memory)
                .await,
        ));
        let admitted_startup_bytes = retained_memory_grant.lock().unwrap().bytes();
        let replaying = self.owner_execution.replay().await?.is_replay();
        let linear_memory = LinearMemoryTracker::new_with_metering(
            initial_linear_memory,
            admitted_startup_bytes,
            self.agent_mode(),
            replaying,
            Arc::clone(&self.resource_entry),
            retained_memory_grant,
            self.config().resource_usage_metering.memory,
        );
        if let Some(meter) = linear_memory.meter_if_enabled() {
            meter.resume(initial_linear_memory, std::time::Instant::now());
        }

        Ctx::create(
            worker_metadata.created_by,
            self.owned_agent_id.clone(),
            self.parsed_agent_id.clone(),
            self.promise_service(),
            self.worker_service(),
            self.worker_enumeration_service(),
            self.key_value_service(),
            self.blob_store_service(),
            self.rdbms_service(),
            self.quota_service(),
            self.worker_event_service.clone(),
            self.active_agents(),
            self.oplog_service(),
            self.oplog.clone(),
            Arc::downgrade(self),
            self.scheduler_service(),
            self.rpc(),
            self.worker_proxy(),
            self.card_service(),
            self.card_interest_index.clone(),
            self.component_service(),
            self.extra_deps(),
            self.config(),
            filesystem_context,
            linear_memory,
            AgentConfig::new(
                DeletedRegions::default(),
                0,
                executable_revision,
                worker_metadata.created_by,
                worker_metadata.created_by_email,
                initial_agent_config,
                None,
                agent_effective_surface,
                Some(owner_component_metadata),
            ),
            self.execution_status.clone(),
            self.file_loader(),
            self.worker_fork_service(),
            self.resource_limits(),
            self.agent_types(),
            self.environment_state_service(),
            self.agent_webhooks(),
            self.shard_service(),
            self.http_connection_pool(),
            self.websocket_connection_pool(),
            None,
            worker_metadata.original_phantom_id,
            runtime,
            self.owner_execution(),
            self.owner_runtime_resources(),
            filesystem,
            executable_component,
            activation,
        )
        .await
    }

    pub fn oom_retry_config(&self) -> &RetryConfig {
        &self.oom_retry_config
    }

    pub(crate) fn snapshot_policy(&self) -> &SnapshotPolicy {
        &self.snapshot_policy
    }

    pub async fn start_if_needed(
        this: Arc<Worker<Ctx>>,
    ) -> Result<Option<Uuid>, WorkerExecutorError> {
        Self::start_if_needed_internal(this, 0, None).await
    }

    async fn start_if_needed_internal(
        this: Arc<Worker<Ctx>>,
        oom_retry_count: u32,
        existing_start_attempt: Option<Uuid>,
    ) -> Result<Option<Uuid>, WorkerExecutorError> {
        {
            *this.last_resume_request.lock().await = Timestamp::now_utc();
        }

        let mut instance_guard = this.lock_non_stopping_worker().await;
        match &*instance_guard {
            WorkerInstance::Unloaded {
                startup_failure: Some(err),
            } if this.agent_mode() == AgentMode::Ephemeral => {
                crate::metrics::ephemeral::record_inactive_invocation_failure();
                Err(err.clone())
            }
            WorkerInstance::Unloaded { .. } => {
                let start_attempt =
                    existing_start_attempt.or_else(|| this.startup_attempt.pending());
                let memory_requirement = match this.memory_requirement().await {
                    Ok(memory_requirement) => memory_requirement,
                    Err(error) => {
                        *instance_guard = WorkerInstance::Unloaded {
                            startup_failure: Some(error.clone()),
                        };
                        this.fail_pending_invocations(error.clone()).await;
                        drop(instance_guard);
                        if let Some(start_attempt) = start_attempt {
                            this.complete_startup(start_attempt, Err(error.clone()));
                        }
                        return Err(error);
                    }
                };
                let start_attempt = this.startup_attempt.begin(start_attempt);
                this.mark_as_loading(start_attempt);
                crate::metrics::workers::inc_worker_waiting_for_memory();
                *instance_guard = WorkerInstance::WaitingForPermit(WaitingWorker::new(
                    this.clone(),
                    memory_requirement,
                    oom_retry_count,
                    start_attempt,
                ));
                Ok(Some(start_attempt))
            }
            WorkerInstance::CleanupFailed(error) => Err(error.clone()),
            WorkerInstance::WaitingForPermit(waiting) => Ok(Some(waiting.start_attempt)),
            WorkerInstance::Running(_) => this.startup_attempt.current(),
            WorkerInstance::Deleting => Err(WorkerExecutorError::invalid_request(
                "Worker is being deleted",
            )),
            WorkerInstance::Stopping(_) => panic!("impossible"),
        }
    }

    /// This method is supposed to be called on a worker for what `is_currently_idle_but_running`
    /// previously returned true.
    ///
    /// It is not guaranteed that the worker is still "running (loaded in memory) but idle" when
    /// this method is called, so it rechecks this condition and only stops the worker if it
    /// is still true. If it was not true, it returns false.
    ///
    /// There are two conditions to this:
    /// - the ExecutionStatus must be suspended; this means the worker is currently not running any invocations
    /// - there must be no more pending invocations in the invocation queue
    ///
    /// Here we first acquire the `instance` lock. This means the worker cannot be started/stopped while we
    /// are processing this method.
    /// If it was not running, then we don't have to stop it.
    /// If it was running, then we recheck the conditions and then stop the worker.
    ///
    /// We know that the conditions remain true because:
    /// - the invocation queue is empty, so it cannot get into `ExecutionStatus::Running`, as there is nothing to run
    /// - nothing can be added to the invocation queue because we are holding the `instance` lock
    ///
    /// By passing the running lock to `stop_internal_running` it is never released and the stop eventually
    /// drops the `RunningWorker` instance.
    ///
    /// The `stopping` flag is only used to prevent re-entrance of the stopping sequence in case the invocation loop
    /// triggers a stop (in case of a failure - by the way it should not happen here because the worker is idle).
    pub async fn stop_if_idle(&self) -> bool {
        let active_agent = self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await;
        let reopen_entity_generation = match active_agent.as_ref() {
            Some(active_agent) => match active_agent.try_fence_idle_entity_bodies() {
                Some(reopen_generation) => reopen_generation,
                None => return false,
            },
            None => None,
        };
        let mut instance_guard = self.lock_non_stopping_worker().await;
        let stop_result = match &*instance_guard {
            WorkerInstance::Running(running) => {
                if self.is_running_worker_idle(running).await {
                    let stop_result = self
                        .stop_internal_locked(
                            &mut instance_guard,
                            false,
                            None,
                            UnloadRequest::ordinary(UnloadReason::Idle),
                            FinalWorkerState::Unloaded {
                                startup_failure: None,
                            },
                            PendingLiveInvocationDisposition::Fail,
                        )
                        .await;

                    Some(stop_result)
                } else {
                    None
                }
            }
            WorkerInstance::WaitingForPermit(_) => None,
            WorkerInstance::Stopping(_) => None,
            WorkerInstance::Unloaded { .. } | WorkerInstance::CleanupFailed(_) => None,
            WorkerInstance::Deleting => None,
        };

        drop(instance_guard);

        if let Some(stop_result) = stop_result {
            self.handle_stop_result(stop_result).await;
            true
        } else {
            if let (Some(generation), Some(active_agent)) = (reopen_entity_generation, active_agent)
            {
                active_agent.reopen_entity_admission_if_generation(generation);
            }
            false
        }
    }

    /// Transition the worker into a deleting state.
    /// Rejects all new invocations and stops any running execution.
    async fn start_deleting_internal(&self) -> Result<(), WorkerExecutorError> {
        self.queue_interrupt(
            InterruptKind::Interrupt(Timestamp::now_utc()),
            false,
            UnloadReason::Deleting,
        )
        .await;
        if let Some(active_agent) = self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await
        {
            active_agent.fence_entity_bodies();
        }
        // Stop any future background flush or clean-checkpoint write from resurrecting the cached
        // status after the upcoming `WorkerService::remove`/`remove_cached_status` deletes it (the
        // latter clears both the live cache and the checkpoint). Each awaits any in-flight write so
        // none can land after the delete.
        self.status_flusher.begin_delete().await;
        self.status_checkpointer.begin_delete().await;
        let error = WorkerExecutorError::invalid_request("Worker is being deleted");
        self.stop_internal(
            false,
            Some(error),
            UnloadRequest::ordinary(UnloadReason::Deleting),
            FinalWorkerState::Deleting,
            PendingLiveInvocationDisposition::Fail,
        )
        .await;
        if let WorkerInstance::CleanupFailed(error) = &*self.instance.lock().await {
            return Err(error.clone());
        }
        self.finalize_durable_stream_consumer_dependencies().await?;
        self.reconcile_durable_stream_attachments().await?;
        let probe = DbDirectStreamAttachmentConsumerProbe::new_routed(
            self.worker_service(),
            self.oplog_service(),
            self.rpc(),
        );
        let producer = self.durable_stream_producer().await?;
        producer
            .cascade_deletion(Timestamp::now_utc().to_millis(), &probe)
            .await
            .map_err(|error| {
                if let Some(evidence) = error.deletion_blocked_evidence() {
                    WorkerExecutorError::invalid_request(format!(
                        "Worker deletion is blocked by durable stream dependents: {evidence}"
                    ))
                } else {
                    WorkerExecutorError::runtime(error.to_string())
                }
            })?;
        let diagnostics = producer.deletion_diagnostics().await;
        debug!(
            agent_id = %self.owned_agent_id,
            deleting = diagnostics.deleting,
            attachments = ?diagnostics.attachments,
            cascade_completed = ?diagnostics.cascade_completed,
            "Durable stream deletion cascade completed"
        );
        Ok(())
    }

    async fn finalize_durable_stream_consumer_dependencies(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        let mut deleting_recorded = false;
        let mut dependencies = HashMap::new();
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_exact(OplogIndex::INITIAL, current.as_u64())
                .await
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = self
                    .oplog
                    .download_payload(record)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                validate_stream_session_record(&record)?;
                match record {
                    StreamSessionRecordV1::ConsumerDeleting(record)
                        if record.consumer_environment_id == self.owned_agent_id.environment_id
                            && record.consumer == self.owned_agent_id.agent_id
                            && record.consumer_fingerprint
                                == self.initial_worker_metadata.fingerprint =>
                    {
                        deleting_recorded = true;
                    }
                    StreamSessionRecordV1::TopologyPrepared(record)
                        if record.attachment.consumer_environment_id
                            == self.owned_agent_id.environment_id
                            && record.attachment.consumer == self.owned_agent_id.agent_id
                            && record.attachment.expected_consumer_fingerprint
                                == self.initial_worker_metadata.fingerprint =>
                    {
                        let slot = (
                            record.attachment.producer_environment_id,
                            record.attachment.producer.clone(),
                            record.attachment.attachment_id,
                            record.attachment.stream_id,
                        );
                        if dependencies.get(&slot).is_none_or(
                            |(key, _): &(StreamAttachmentKeyV1, StreamSessionMappingRecordV1)| {
                                key.epoch <= record.attachment.epoch
                            },
                        ) {
                            dependencies.insert(slot, (record.attachment, record.mapping));
                        }
                    }
                    StreamSessionRecordV1::TopologyActivated(record)
                        if record.attachment.consumer_environment_id
                            == self.owned_agent_id.environment_id
                            && record.attachment.consumer == self.owned_agent_id.agent_id
                            && record.attachment.expected_consumer_fingerprint
                                == self.initial_worker_metadata.fingerprint =>
                    {
                        let slot = (
                            record.attachment.producer_environment_id,
                            record.attachment.producer.clone(),
                            record.attachment.attachment_id,
                            record.attachment.stream_id,
                        );
                        if dependencies.get(&slot).is_none_or(
                            |(key, _): &(StreamAttachmentKeyV1, StreamSessionMappingRecordV1)| {
                                key.epoch <= record.attachment.epoch
                            },
                        ) {
                            dependencies.insert(slot, (record.attachment, record.mapping));
                        }
                    }
                    _ => {}
                }
            }
        }
        let producer = self.durable_stream_producer().await?;
        if !deleting_recorded {
            producer
                .append_session_record(StreamSessionRecordV1::ConsumerDeleting(
                    StreamConsumerDeletingRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        consumer_environment_id: self.owned_agent_id.environment_id,
                        consumer: self.owned_agent_id.agent_id.clone(),
                        consumer_fingerprint: self.initial_worker_metadata.fingerprint,
                        deleting_at_millis: Timestamp::now_utc().to_millis(),
                    },
                ))
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        }
        if dependencies.is_empty() {
            return Ok(());
        }
        let auth_ctx = self.durable_stream_consumer_auth_ctx()?;
        for (key, mapping) in dependencies.into_values() {
            RoutedStreamAttachmentControl::new(self.rpc(), mapping, auth_ctx.clone())
                .finalize_attachment(
                    key,
                    StreamAttachmentFinalizationReasonV1::ConsumerDeleted,
                    Timestamp::now_utc().to_millis(),
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        }
        Ok(())
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

    fn mark_as_loading(&self, start_attempt: Uuid) {
        self.startup_attempt.begin(Some(start_attempt));
        let mut execution_status = self.execution_status.write().unwrap();
        *execution_status = ExecutionStatus::Loading {
            agent_mode: execution_status.agent_mode(),
            timestamp: Timestamp::now_utc(),
        };
    }

    fn publish_startup_result(&self, start_attempt: Uuid, result: Result<(), WorkerExecutorError>) {
        if !self.startup_attempt.complete(start_attempt, &result) {
            return;
        }
        self.publish_completed_startup_result(start_attempt, result);
    }

    fn publish_completed_startup_result(
        &self,
        start_attempt: Uuid,
        result: Result<(), WorkerExecutorError>,
    ) {
        self.events().publish(Event::WorkerLoaded {
            agent_id: self.agent_id(),
            start_attempt,
            result,
        });
    }

    pub(crate) fn complete_startup(
        &self,
        start_attempt: Uuid,
        result: Result<(), WorkerExecutorError>,
    ) {
        self.publish_startup_result(start_attempt, result);
    }

    pub(crate) async fn complete_startup_success(&self, start_attempt: Uuid) -> bool {
        let instance_guard = self.instance.lock().await;
        let active_attempt = match &*instance_guard {
            WorkerInstance::Running(running) => Some(running.start_attempt),
            _ => None,
        };
        let is_active = active_attempt == Some(start_attempt);
        let result = if is_active {
            Ok(())
        } else {
            Err(WorkerExecutorError::unknown(
                "Worker stopped before startup completed",
            ))
        };
        let completed = match &result {
            Ok(()) => self
                .startup_attempt
                .complete_success_if_active(start_attempt, active_attempt),
            Err(_) => self.startup_attempt.complete(start_attempt, &result),
        };
        drop(instance_guard);

        if completed {
            self.publish_completed_startup_result(start_attempt, result);
        }
        is_active
    }

    pub(crate) fn pending_startup_attempt(&self) -> Option<Uuid> {
        self.startup_attempt.pending()
    }

    pub fn get_initial_worker_metadata(&self) -> AgentMetadata {
        self.initial_worker_metadata.clone()
    }

    pub async fn get_latest_worker_metadata(&self) -> AgentMetadata {
        let updated_status = self.last_known_status.load_full().as_ref().clone();
        let result = self.get_initial_worker_metadata();
        AgentMetadata {
            last_known_status: updated_status,
            ..result
        }
    }

    pub async fn get_last_known_status(&self) -> AgentStatusRecord {
        self.last_known_status.load_full().as_ref().clone()
    }

    // Outside of reverts and updates, this will return the same status as get_latest_worker_metadata.
    // This just has an additional assert built in for when decisions need to be sure that they are fully up to date on the oplog.
    // _NEVER_ call this from outside the invocation loop, as that is the only place that can reason about whether the status is detached or not.
    pub async fn get_non_detached_last_known_status(&self) -> AgentStatusRecord {
        // Runs on the worker-state actor's status queue so the detached flag and the published
        // status are observed consistently with any in-flight commit/reattach transaction.
        self.state_actor.non_detached_status().await
    }

    pub(crate) fn owned_agent_id(&self) -> &OwnedAgentId {
        &self.owned_agent_id
    }

    /// Marks the worker as interrupting - this should eventually make the worker interrupted.
    /// There are several interruption modes but not all of them are supported by all worker
    /// executor implementations.
    ///
    /// - `Interrupt` means that the worker should be interrupted as soon as possible, and it should
    ///   remain interrupted.
    /// - `Restart` is a simulated crash, the worker gets automatically restarted after it got interrupted,
    ///   but only if the worker context supports recovering workers.
    /// - `Suspend` means that the worker should be moved out of memory and stay in suspended state,
    ///   automatically resumed when the worker is needed again. This only works if the worker context
    ///   supports recovering workers.
    pub async fn set_interrupting(&self, interrupt_kind: InterruptKind) -> Option<Receiver<()>> {
        self.set_interrupting_internal(
            interrupt_kind,
            false,
            UnloadReason::from_interrupt(interrupt_kind),
        )
        .await
    }

    async fn set_interrupting_internal(
        &self,
        interrupt_kind: InterruptKind,
        reacquire_permits: bool,
        unload_reason: UnloadReason,
    ) -> Option<Receiver<()>> {
        if !self
            .queue_interrupt(interrupt_kind, reacquire_permits, unload_reason)
            .await
        {
            return None;
        }
        if let Some(active_agent) = self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await
        {
            active_agent.fence_entity_bodies();
        }
        self.notify_queued_interrupt(interrupt_kind).await
    }

    async fn notify_queued_interrupt(&self, interrupt_kind: InterruptKind) -> Option<Receiver<()>> {
        let instance_guard = self.lock_non_stopping_worker().await;
        if let WorkerInstance::Running(running) = &*instance_guard {
            let _ = running.sender.send(WorkerCommand::WorkAvailable);
        }
        drop(instance_guard);

        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running {
                interrupt_signal, ..
            } => {
                let _ = interrupt_signal.send(interrupt_kind);
                let (sender, receiver) = tokio::sync::broadcast::channel(1);
                *execution_status = ExecutionStatus::Interrupting {
                    interrupt_kind,
                    await_interruption: Arc::new(sender),
                    agent_mode: execution_status.agent_mode(),
                    timestamp: Timestamp::now_utc(),
                };
                Some(receiver)
            }
            ExecutionStatus::Suspended { .. } => None,
            ExecutionStatus::Interrupting {
                interrupt_kind: current_kind,
                await_interruption,
                agent_mode,
                timestamp,
            } => {
                let receiver = await_interruption.subscribe();
                if matches!(current_kind, InterruptKind::Restart)
                    && !matches!(interrupt_kind, InterruptKind::Restart)
                {
                    *execution_status = ExecutionStatus::Interrupting {
                        interrupt_kind,
                        await_interruption,
                        agent_mode,
                        timestamp,
                    };
                }
                Some(receiver)
            }
            ExecutionStatus::Loading { .. } => None,
        }
    }

    async fn queue_interrupt(
        &self,
        interrupt_kind: InterruptKind,
        reacquire_permits: bool,
        unload_reason: UnloadReason,
    ) -> bool {
        let mut state = self.interrupt_signal.lock().await;
        state.queue(PendingWorkerInterrupt {
            kind: interrupt_kind,
            reacquire_permits,
            unload_request: UnloadRequest::ordinary(unload_reason),
        })
    }

    pub(crate) async fn set_interrupting_for(
        &self,
        interrupt_kind: InterruptKind,
        unload_reason: UnloadReason,
    ) -> Option<Receiver<()>> {
        self.set_interrupting_internal(interrupt_kind, false, unload_reason)
            .await
    }

    pub async fn resume_replay(&self) -> Result<(), WorkerExecutorError> {
        match &*self.lock_non_stopping_worker().await {
            WorkerInstance::Running(running) => {
                running.resume_replay_pending.store(true, Ordering::Release);
                running
                    .sender
                    .send(WorkerCommand::ResumeReplay)
                    .expect("Failed to send resume command");

                Ok(())
            }
            WorkerInstance::Unloaded { .. } | WorkerInstance::WaitingForPermit(_) => {
                Err(WorkerExecutorError::invalid_request(
                    "Explicit resume is not supported for uninitialized workers",
                ))
            }
            WorkerInstance::CleanupFailed(error) => Err(error.clone()),
            WorkerInstance::Deleting => Err(WorkerExecutorError::invalid_request(
                "Explicit resume is not supported for deleting workers",
            )),
            WorkerInstance::Stopping(_) => panic!("impossible"),
        }
    }

    /// Extracts the read-only context for `invocation` by looking up the
    /// method's `read_only` config on the currently-loaded component
    /// metadata. Returns `None` for non-`AgentMethod` invocations and for
    /// methods that are not declared `#[read_only]`.
    fn read_only_context_for(&self, invocation: &AgentInvocation) -> Option<ReadOnlyContext> {
        let AgentInvocation::AgentMethod {
            method_name,
            input,
            principal,
            scope_card,
            ..
        } = invocation
        else {
            return None;
        };

        let snapshot = self.current_component.load();
        let component_revision = snapshot.revision;
        let metadata = &snapshot.metadata;
        let agent_type_opt = self.parsed_agent_id.as_ref().map(|p| p.agent_type.clone());

        let agent_type = agent_type_opt.as_ref()?;
        let method = read_only_cache::resolve_read_only_method(metadata, agent_type, method_name)?;
        let cfg = method.read_only.as_ref()?;

        Some(ReadOnlyContext {
            method_name: method_name.clone(),
            input: input.clone(),
            principal: principal.clone(),
            cfg: cfg.clone(),
            component_revision,
            cacheable: scope_card.is_none(),
        })
    }

    /// Invocation entry point. Returns `Finished(...)` on read-only cache hit,
    /// otherwise `Pending(subscription)`. `Arc<Self>` is needed to spawn the
    /// detached observer that fills the read-only cache on completion.
    pub async fn invoke(
        self: Arc<Self>,
        invocation: AgentInvocation,
    ) -> Result<ResultOrSubscription, WorkerExecutorError> {
        let idempotency_key = Self::require_idempotency_key(&invocation)?;

        // Classification uses the in-memory component snapshot - no metadata
        // fetch on the hot path.
        let read_only_ctx = self.read_only_context_for(&invocation);

        let effect = if read_only_ctx.is_some() {
            read_only_cache::InvocationEffect::ReadOnly
        } else {
            read_only_cache::InvocationEffect::Mutating
        };

        // Cache HIT: still refuse on deleting / startup-failed worker.
        if let Some(ro) = &read_only_ctx {
            let no_cache = !ro.cacheable || is_no_cache(&ro.cfg.cache_policy);
            if !no_cache {
                let cur_epoch = self.read_only_cache_epoch.load(Ordering::SeqCst);
                let principal_ref = if ro.cfg.uses_principal {
                    Some(&ro.principal)
                } else {
                    None
                };
                let key = read_only_cache::build_read_only_cache_key(
                    &ro.method_name,
                    &ro.input,
                    principal_ref,
                    ro.component_revision,
                    cur_epoch,
                );
                if let Some(entry) = self.read_only_cache.try_get(&key).await {
                    if !entry.is_expired(tokio::time::Instant::now()) {
                        let instance_guard = self.lock_non_stopping_worker().await;
                        if instance_guard.is_deleting() {
                            return Err(WorkerExecutorError::invalid_request(
                                "Cannot enqueue invocation to a deleting worker",
                            ));
                        }
                        if let Some(err) = instance_guard.startup_failure() {
                            return Err(err.clone());
                        }
                        drop(instance_guard);
                        return Ok(ResultOrSubscription::Finished(Ok(entry.output.clone())));
                    } else {
                        // Only evict if the stored entry is still ours.
                        let me = entry.clone();
                        let _ = self
                            .read_only_cache
                            .remove_if_cached(&key, move |current| Arc::ptr_eq(current, &me))
                            .await;
                    }
                }
            }
        }

        // Subscribe before enqueue/lookup to avoid missing the completion event.
        let subscription = self.events().subscribe();
        let observer_sub = if let Some(ro) = &read_only_ctx {
            if !ro.cacheable || is_no_cache(&ro.cfg.cache_policy) {
                None
            } else {
                Some(self.events().subscribe())
            }
        } else {
            None
        };

        let output = async { self.lookup_invocation_result(&idempotency_key).await }
            .instrument(span!(
                Level::INFO,
                "lookup_invocation_result",
                agent_id = %self.owned_agent_id.agent_id,
                idempotency_key = %idempotency_key,
            ))
            .await;
        let (result, enqueue_epoch) = match output {
            LookupResult::Complete(output) => (ResultOrSubscription::Finished(output), None),
            LookupResult::Interrupted => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            LookupResult::Pending => (ResultOrSubscription::Pending(subscription), None),
            LookupResult::New => {
                if let AgentInvocation::AgentMethod {
                    scope_card: Some(scope_card),
                    ..
                } = &invocation
                {
                    crate::services::card::validate_scope_card(
                        self.card_service().as_ref(),
                        scope_card,
                    )
                    .await?;
                }
                // For ReadOnly the helper returns the epoch captured under the
                // enqueue lock; using any other epoch could store stale data.
                let captured = self
                    .enqueue_worker_invocation_with_effect(invocation, effect)
                    .await?;
                (ResultOrSubscription::Pending(subscription), captured)
            }
        };

        // Only populate the cache when this call owns the enqueue (only then
        // do we have a valid epoch).
        if let Some(ro) = read_only_ctx
            && let (Some(mut obs_sub), Some(epoch)) = (observer_sub, enqueue_epoch)
        {
            // Do not capture `Arc<Self>` - a never-completing invocation would
            // otherwise pin the worker.
            let cache = self.read_only_cache.clone();
            // The observer task does a final epoch recheck before insert
            // (`populate_read_only_cache`), so it needs the live atomic.
            let read_only_cache_epoch = self.read_only_cache_epoch.clone();
            let agent_id = self.owned_agent_id.agent_id.clone();
            let idem = idempotency_key.clone();
            tokio::spawn(async move {
                let wait_result = obs_sub
                    .wait_for(|event| match event {
                        Event::InvocationCompleted {
                            agent_id: ev_agent,
                            idempotency_key,
                            result,
                        } if *ev_agent == agent_id && *idempotency_key == idem => {
                            Some(result.clone())
                        }
                        _ => None,
                    })
                    .await;
                if let Ok(Ok(output)) = wait_result
                    && matches!(output.result, AgentInvocationResult::AgentMethod { .. })
                {
                    populate_read_only_cache(&cache, &read_only_cache_epoch, &ro, epoch, output)
                        .await;
                }
            });
        }

        Ok(result)
    }

    /// Invokes the worker and awaits for a result.
    ///
    /// For cacheable read-only `AgentMethod` invocations, concurrent Await
    /// misses for the same `ReadOnlyCacheKey` are *coalesced* via
    /// [`golem_common::cache::Cache::get_or_insert_simple`] — only the first
    /// caller runs the underlying invocation and populates the cache; later
    /// concurrent callers receive the same result without re-enqueueing.
    ///
    /// Coalescing is intentionally scoped to the Await path. Fire-and-forget
    /// (`invoke`) callers must return immediately, so they do not block on
    /// pending entries and continue to use the detached observer to populate
    /// the cache. The unified key shape means an Await coalesce and a
    /// fire-and-forget observer can race; both produce the same
    /// [`ReadOnlyCacheEntry`] from the same output, so the race is benign.
    pub async fn invoke_and_await(
        self: Arc<Self>,
        invocation: AgentInvocation,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        let idempotency_key = Self::require_idempotency_key(&invocation)?;

        // Fast path: read-only Await coalescing.
        //
        // Coalescing is only safe for genuinely new invocations.
        // Idempotency replay (`lookup_invocation_result` returns `Complete`)
        // must return the result that was recorded under whatever epoch the
        // original invocation ran in — so coalescing the call (and caching
        // its result under the current epoch's `ReadOnlyCacheKey`) would
        // poison the cache. `Pending` means another caller is responsible
        // for completing the invocation, so we just await the existing
        // result instead of enqueueing a duplicate or caching here.
        //
        // For non-`New` results we MUST NOT fall through to
        // `Worker::invoke`: that path checks the read-only cache HIT before
        // looking up the idempotency key, which would let a warm
        // current-epoch entry shadow the recorded idempotency result.
        // Instead we handle non-`New` results inline below.
        let lookup_for_coalesce = if let Some(ro) = self.read_only_context_for(&invocation)
            && ro.cacheable
            && !is_no_cache(&ro.cfg.cache_policy)
        {
            Some((
                ro,
                async { self.lookup_invocation_result(&idempotency_key).await }
                    .instrument(span!(
                        Level::INFO,
                        "lookup_invocation_result",
                        agent_id = %self.owned_agent_id.agent_id,
                        idempotency_key = %idempotency_key,
                    ))
                    .await,
            ))
        } else {
            None
        };

        match lookup_for_coalesce {
            Some((_, LookupResult::Complete(Ok(output)))) => return Ok(output),
            Some((_, LookupResult::Complete(Err(err)))) => return Err(err),
            Some((_, LookupResult::Interrupted)) => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            Some((_, LookupResult::Pending)) => {
                // Another caller already enqueued this idempotency key. Wait
                // for its result without going through `Worker::invoke` (so a
                // current-epoch read-only cache HIT cannot shadow the
                // recorded idempotency result), and do not populate the
                // read-only cache here.
                let subscription = self.events().subscribe();
                Worker::start_if_needed(self.clone()).await?;
                let result = self
                    .wait_for_invocation_result(&idempotency_key, subscription)
                    .await;
                return match result {
                    Ok(LookupResult::Complete(Ok(output))) => Ok(output),
                    Ok(LookupResult::Complete(Err(err))) => Err(err),
                    Ok(LookupResult::Interrupted) => {
                        Err(InterruptKind::Interrupt(Timestamp::now_utc()).into())
                    }
                    Ok(LookupResult::Pending) => Err(WorkerExecutorError::unknown(
                        "Unexpected pending result after invoke",
                    )),
                    Ok(LookupResult::New) => Err(WorkerExecutorError::unknown(
                        "Unexpected missing result after invoke",
                    )),
                    Err(recv_error) => Err(WorkerExecutorError::unknown(format!(
                        "Failed waiting for invocation result: {recv_error}"
                    ))),
                };
            }
            _ => {}
        }

        if let Some((ro, LookupResult::New)) = lookup_for_coalesce {
            // Use the same key shape as the `invoke` cache HIT path so a hit
            // there and a coalesced miss here see the same entry.
            let cur_epoch = self.read_only_cache_epoch.load(Ordering::SeqCst);
            let principal_ref = if ro.cfg.uses_principal {
                Some(&ro.principal)
            } else {
                None
            };
            let key = read_only_cache::build_read_only_cache_key(
                &ro.method_name,
                &ro.input,
                principal_ref,
                ro.component_revision,
                cur_epoch,
            );

            // Honor TTL up front: a stale entry must miss, not hit (mirrors
            // the `Worker::invoke` HIT path).
            if let Some(entry) = self.read_only_cache.try_get(&key).await {
                if !entry.is_expired(tokio::time::Instant::now()) {
                    // Apply the same `is_deleting` / `startup_failure` guard
                    // the HIT path in `invoke` applies, so we don't return a
                    // cached value for a worker that's about to disappear.
                    let instance_guard = self.lock_non_stopping_worker().await;
                    if instance_guard.is_deleting() {
                        return Err(WorkerExecutorError::invalid_request(
                            "Cannot enqueue invocation to a deleting worker",
                        ));
                    }
                    if let Some(err) = instance_guard.startup_failure() {
                        return Err(err.clone());
                    }
                    drop(instance_guard);
                    return Ok(entry.output.clone());
                } else {
                    let me = entry.clone();
                    let _ = self
                        .read_only_cache
                        .remove_if_cached(&key, move |current| Arc::ptr_eq(current, &me))
                        .await;
                }
            }

            // Coalesce concurrent first-time misses for this key. Only the
            // first caller spawns the underlying invocation; subsequent
            // concurrent callers wait on the same pending entry inside
            // `get_or_insert_simple_spawned` and receive the same
            // `ReadOnlyCacheEntry`.
            //
            // The spawned closure runs `invoke_and_await_uncoalesced` to
            // bypass this coalescing path. Returning Err removes the pending
            // entry so a later caller retries (failures must not poison the
            // cache).
            //
            // The closure is spawned via `tokio::task::spawn` (see
            // [`Cache::get_or_insert_spawned`]) so that cancellation of any
            // single Await caller does NOT leave the pending entry stuck
            // forever — the spawned owner future survives caller drop and
            // resolves the entry one way or the other.
            let ro_for_closure = ro.clone();
            let worker = self.clone();
            let invocation_for_closure = invocation;
            let idem_for_closure = idempotency_key.clone();

            // The owner future is spawned and deliberately survives this caller, so
            // it cannot run inside the caller's span - that would hold the gRPC span
            // open for the whole invocation. It links back instead, so the coalesced
            // execution is still reachable from the request that started it.
            //
            // Captured before the wait span rather than inside it, unlike
            // `enqueue_worker_invocation_with_effect`: N callers coalesce onto one
            // execution, so linking to any one
            // caller's wait would pick an arbitrary winner. The request that owns the
            // execution is the honest originator.
            let origin = TraceOrigin::capture_current();
            let owner_agent_id = self.owned_agent_id.agent_id.clone();
            let owner_idempotency_key = idempotency_key.clone();

            let entry_result = async {
                self.read_only_cache
                    .get_or_insert_simple_spawned(&key, move || {
                        let span = related_span!(
                            origin,
                            Level::INFO,
                            "read_only_invocation",
                            agent_id = %owner_agent_id,
                            idempotency_key = %owner_idempotency_key,
                        );
                        async move {
                            let output = Worker::invoke_and_await_uncoalesced(
                                worker,
                                invocation_for_closure,
                                idem_for_closure,
                            )
                            .await?;
                            if !matches!(output.result, AgentInvocationResult::AgentMethod { .. }) {
                                // Defensive: only `AgentMethod` outputs are cacheable.
                                return Err(WorkerExecutorError::unknown(
                                    "read-only invocation produced a non-AgentMethod result",
                                ));
                            }
                            Ok(build_read_only_cache_entry(&ro_for_closure, output))
                        }
                        .instrument(span)
                    })
                    .await
            }
            // The caller's own share: however long it waits for the coalesced
            // execution, whether or not it is the one that owns it.
            .instrument(span!(
                Level::INFO,
                "wait_for_read_only_invocation",
                agent_id = %self.owned_agent_id.agent_id,
                idempotency_key = %idempotency_key,
            ))
            .await;

            // Stale-populate guard: if the epoch bumped while the owner ran,
            // the entry we just inserted is keyed on the old epoch and is
            // already unreachable for any future lookup. We could leave it
            // for the LRU; explicitly removing it keeps the cache tidy.
            if self.read_only_cache_epoch.load(Ordering::SeqCst) != cur_epoch
                && let Ok(entry) = &entry_result
            {
                let me = entry.clone();
                let _ = self
                    .read_only_cache
                    .remove_if_cached(&key, move |current| Arc::ptr_eq(current, &me))
                    .await;
            }

            return entry_result.map(|entry| entry.output.clone());
        }

        // Non-cacheable path: `NoCache` read-only methods and all
        // non-read-only invocations skip coalescing entirely.
        Worker::invoke_and_await_uncoalesced(self, invocation, idempotency_key).await
    }

    /// Underlying `invoke_and_await` implementation without read-only
    /// coalescing. Used directly for non-cacheable invocations and as the
    /// per-key owner future inside the coalesced path above.
    async fn invoke_and_await_uncoalesced(
        self: Arc<Self>,
        invocation: AgentInvocation,
        idempotency_key: IdempotencyKey,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        let result = self.clone().invoke(invocation).await?;
        self.await_invocation_result(idempotency_key, result).await
    }

    pub(crate) async fn await_enqueued_invocation(
        self: Arc<Self>,
        idempotency_key: IdempotencyKey,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        let subscription = self.events().subscribe();
        let result = match self.lookup_invocation_result(&idempotency_key).await {
            LookupResult::Complete(result) => ResultOrSubscription::Finished(result),
            LookupResult::Pending => ResultOrSubscription::Pending(subscription),
            LookupResult::Interrupted => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            LookupResult::New => {
                return Err(WorkerExecutorError::runtime(
                    "durable streaming acceptance did not leave its invocation pending",
                ));
            }
        };
        self.await_invocation_result(idempotency_key, result).await
    }

    pub(crate) async fn await_invocation_result(
        self: Arc<Self>,
        idempotency_key: IdempotencyKey,
        result: ResultOrSubscription,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        match result {
            ResultOrSubscription::Finished(Ok(output)) => Ok(output),
            ResultOrSubscription::Finished(Err(err)) => Err(err),
            ResultOrSubscription::Pending(subscription) => {
                // Cache miss / non-read-only path: ensure the wasmtime instance is
                // running so the queued invocation can be processed. The
                // `ResultOrSubscription::Finished` arm above short-circuits before
                // this, which is exactly what makes a read-only cache hit avoid
                // any agent loading.
                Worker::start_if_needed(self.clone()).await?;

                debug!("Waiting for idempotency key to complete",);

                // The caller's honest share of the work: the execution runs in the
                // worker's own trace, linked from here, and only its outcome comes
                // back. This span measures the waiting, which is what the request
                // actually spent its time on.

                let result = async {
                    self.wait_for_invocation_result(&idempotency_key, subscription)
                        .await
                }
                .instrument(span!(
                    Level::INFO,
                    "wait_for_invocation_result",
                    agent_id = %self.owned_agent_id.agent_id,
                    idempotency_key = %idempotency_key,
                ))
                .await;

                match result {
                    Ok(LookupResult::Complete(Ok(output))) => Ok(output),
                    Ok(LookupResult::Complete(Err(err))) => Err(err),
                    Ok(LookupResult::Interrupted) => {
                        Err(InterruptKind::Interrupt(Timestamp::now_utc()).into())
                    }
                    Ok(LookupResult::Pending) => Err(WorkerExecutorError::unknown(
                        "Unexpected pending result after invoke",
                    )),
                    Ok(LookupResult::New) => Err(WorkerExecutorError::unknown(
                        "Unexpected missing result after invoke",
                    )),
                    Err(recv_error) => Err(WorkerExecutorError::unknown(format!(
                        "Failed waiting for invocation result: {recv_error}"
                    ))),
                }
            }
        }
    }

    fn require_idempotency_key(
        invocation: &AgentInvocation,
    ) -> Result<IdempotencyKey, WorkerExecutorError> {
        invocation.idempotency_key().cloned().ok_or_else(|| {
            WorkerExecutorError::invalid_request("Invocation has no idempotency key")
        })
    }

    /// Enqueue attempting an update.
    ///
    /// The update itself is not performed by the invocation queue's processing loop,
    /// it is going to affect how the worker is recovered next time.
    pub async fn enqueue_update(&self, update_description: UpdateDescription) {
        // Bump + commit under the same instance lock.
        let instance_guard = self.lock_non_stopping_worker().await;
        self.bump_read_only_cache_epoch();
        let entry = OplogEntry::pending_update(update_description.clone());
        self.add_and_commit_oplog_internal(
            &instance_guard,
            entry,
            Some(WorkerCommand::WorkAvailable),
        )
        .await;
        drop(instance_guard);
    }

    /// Enqueues a manual update.
    ///
    /// This enqueues a special function invocation that saves the component's state and
    /// triggers a restart immediately.
    pub async fn enqueue_manual_update(
        &self,
        target_revision: ComponentRevision,
    ) -> Result<(), WorkerExecutorError> {
        self.enqueue_worker_invocation(AgentInvocation::ManualUpdate { target_revision })
            .await
    }

    pub async fn pending_invocations(&self) -> Vec<PendingInvocationRef> {
        self.last_known_status.load().pending_invocations.clone()
    }

    /// Reads the `PendingAgentInvocation` oplog entry referenced by `pending` and reconstructs the
    /// full invocation, downloading its payload from external storage if needed. The status record
    /// only keeps a lightweight reference, so callers that need to execute the invocation hydrate
    /// it on demand.
    async fn hydrate_pending_invocation(
        &self,
        pending: &PendingInvocationRef,
    ) -> Result<TimestampedAgentInvocation, WorkerExecutorError> {
        let entry = self.oplog.read(pending.oplog_index).await;
        match entry {
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => {
                let agent_payload = self.oplog.download_payload(payload).await.map_err(|e| {
                    WorkerExecutorError::unknown(format!(
                        "Failed to download pending agent invocation payload at oplog index {}: {e}",
                        pending.oplog_index
                    ))
                })?;
                let invocation_context = InvocationContextStack::from_oplog_data(
                    trace_id,
                    trace_states,
                    invocation_context,
                );
                let invocation =
                    AgentInvocation::from_parts(idempotency_key, agent_payload, invocation_context);
                Ok(TimestampedAgentInvocation {
                    timestamp,
                    invocation,
                })
            }
            other => Err(WorkerExecutorError::unknown(format!(
                "Expected a PendingAgentInvocation oplog entry at index {}, but found {other:?}",
                pending.oplog_index
            ))),
        }
    }

    /// Reads the `PendingUpdate` oplog entry referenced by `pending` and reconstructs the full
    /// update description, including any snapshot payload reference. The status record only keeps
    /// a lightweight reference, so callers that apply the update hydrate it on demand.
    async fn hydrate_pending_update(
        &self,
        pending: &PendingUpdateRef,
    ) -> Result<TimestampedUpdateDescription, WorkerExecutorError> {
        let entry = self.oplog.read(pending.oplog_index).await;
        match entry {
            OplogEntry::PendingUpdate {
                timestamp,
                description,
                ..
            } => Ok(TimestampedUpdateDescription {
                timestamp,
                oplog_index: pending.oplog_index,
                description,
            }),
            other => Err(WorkerExecutorError::unknown(format!(
                "Expected a PendingUpdate oplog entry at index {}, but found {other:?}",
                pending.oplog_index
            ))),
        }
    }

    pub async fn invocation_results(&self) -> HashMap<IdempotencyKey, OplogIndex> {
        self.last_known_status.load().invocation_results.clone()
    }

    // should only be called from invocation loop
    pub async fn store_invocation_success(
        &self,
        key: &IdempotencyKey,
        output: AgentInvocationOutput,
    ) {
        let mut map = self.invocation_results.write().await;
        map.insert(
            key.clone(),
            InvocationResult::Cached {
                result: Ok(output.clone()),
            },
        );
        // `drop` before taking `origins`: `fail_pending_invocations` locks
        // origins -> invocation_results, so holding `map` here would invert that
        // order and can deadlock. Not a scope tidy-up.
        drop(map);
        self.external_invocation_origins.write().await.remove(key);
        debug!("Stored invocation success for {key}");
        self.publish_completion(key, Ok(output));
    }

    fn publish_completion(
        &self,
        key: &IdempotencyKey,
        result: Result<AgentInvocationOutput, WorkerExecutorError>,
    ) {
        self.events().publish(Event::InvocationCompleted {
            agent_id: self.owned_agent_id.agent_id(),
            idempotency_key: key.clone(),
            result,
        });
    }

    // should only be called from invocation loop
    pub async fn store_invocation_failure(&self, key: &IdempotencyKey, trap_type: &TrapType) {
        let status = self.last_known_status.load_full().as_ref().clone();
        let keys_to_fail =
            invocation_keys_to_fail(&status, Some(key), !trap_type.is_invocation_rejection());
        let stderr = self.worker_event_service.get_last_invocation_errors();
        let golem_error = trap_type.as_golem_error(&stderr);
        let mut map = self.invocation_results.write().await;
        for key in &keys_to_fail {
            map.insert(
                key.clone(),
                InvocationResult::Cached {
                    result: Err(FailedInvocationResult {
                        trap_type: trap_type.clone(),
                        stderr: stderr.clone(),
                    }),
                },
            );
            if let Some(golem_error) = &golem_error {
                self.publish_completion(key, Err(golem_error.clone()));
            }
        }
        // See `store_invocation_success`: origins must not be taken while
        // `invocation_results` is held.
        drop(map);
        let mut origins = self.external_invocation_origins.write().await;
        for key in &keys_to_fail {
            origins.remove(key);
        }
    }

    pub(super) async fn store_invocation_resuming(&self, key: &IdempotencyKey) {
        let mut map = self.invocation_results.write().await;
        map.remove(key);
    }

    pub fn agent_mode(&self) -> AgentMode {
        self.execution_status.read().unwrap().agent_mode()
    }

    /// Gets the estimated memory requirement of the worker.
    ///
    /// This covers only the per-worker linear memory. The compiled component
    /// module is shared by all workers of a component and is charged once per
    /// resident component via the component-charge registry, not per worker.
    pub async fn memory_requirement(&self) -> Result<u64, WorkerExecutorError> {
        let metadata = self.get_latest_worker_metadata().await;

        Ok(metadata.last_known_status.total_linear_memory_size)
    }

    /// Startup module-charge requirement for a worker about to be (re)started.
    ///
    /// Returns the component identity and compiled-module size to reserve with
    /// the gate, keyed to the revision [`Worker::create_instance`] will actually
    /// instantiate: when a pending update is queued, the worker loads the
    /// update's `target_revision`, not the last known one, so the charge must be
    /// keyed to — and sized from — the target. Keying it to the old revision
    /// would attach the held charge to the wrong resident module and, if the
    /// target module is larger, under-reserve memory.
    ///
    /// The invariant is: if `create_instance` can still successfully load the
    /// target revision, admission must charge the target revision. Resolving the
    /// target's module size is therefore handled by error class:
    ///
    /// - `ComponentNotFound`: the target genuinely does not exist, so
    ///   `create_instance` will write a `failed_update` and retry the *current*
    ///   revision. Charge the current revision/size to match — falling back here
    ///   keeps the worker startable instead of wedged, and `create_instance`
    ///   drives the recovery.
    /// - Any other (transient/runtime) error: `create_instance`'s later
    ///   `component_service().get(target)` may still succeed and load the target,
    ///   so we must not fall back to the current revision (that would under-reserve
    ///   and mis-key the charge). Back off and retry resolving the target, exactly
    ///   as the memory admission loop treats transient pressure, until it resolves
    ///   to a definite answer.
    async fn startup_component_charge_requirement(&self) -> StartupComponentChargeRequirement {
        let metadata = self.get_latest_worker_metadata().await;
        let component_id = self.owned_agent_id.component_id();
        let current_revision = metadata.last_known_status.component_revision;
        let current_size = metadata.last_known_status.component_size;

        // Mirror create_instance: a queued pending update is applied by loading
        // its target revision, so charge against that revision rather than the
        // last known one.
        let pending_target = metadata
            .last_known_status
            .pending_updates
            .front()
            .map(|update| update.target_revision);
        let component_revision = component_charge_revision(pending_target, current_revision);

        // The currently-loaded revision's module size is already recorded in the
        // status; a pending-update target's size must be resolved from its
        // metadata so the reservation matches the module create_instance loads.
        if component_revision == current_revision {
            let canonical_bytes = metadata.last_known_status.total_linear_memory_size;
            let current = self.current_component.load();
            return startup_component_requirement(
                component_id,
                current_revision,
                current_size,
                current.metadata.initial_linear_memory_bytes(),
                canonical_bytes,
            );
        }

        let retry_delay = self.config().memory.acquire_retry_delay;
        loop {
            let result = self
                .component_service()
                .get_metadata(component_id, Some(component_revision))
                .await
                .map(|target| {
                    let initial_linear_memory_bytes = target.metadata.initial_linear_memory_bytes();
                    ResolvedComponentCharge {
                        module_bytes: target.component_size,
                        initial_linear_memory_bytes,
                        reserved_linear_memory_bytes: initial_linear_memory_bytes,
                    }
                });
            match classify_target_charge(&result) {
                TargetChargeAction::ChargeTarget(charge) => {
                    return StartupComponentChargeRequirement {
                        component_id,
                        component_revision,
                        module_bytes: charge.module_bytes,
                        startup_linear_memory_bytes: charge.initial_linear_memory_bytes,
                        reserved_linear_memory_bytes: charge.reserved_linear_memory_bytes,
                    };
                }
                TargetChargeAction::FallBackToCurrent => {
                    // The target revision does not exist; create_instance will fail
                    // the update and load the current revision, so charge that.
                    debug!(
                        "Pending-update target revision {component_revision} does not exist; charging against current revision and letting create_instance fail the update and recover"
                    );
                    let canonical_bytes = metadata.last_known_status.total_linear_memory_size;
                    let current = self.current_component.load();
                    return startup_component_requirement(
                        component_id,
                        current_revision,
                        current_size,
                        current.metadata.initial_linear_memory_bytes(),
                        canonical_bytes,
                    );
                }
                TargetChargeAction::Retry => {
                    // Transient failure: create_instance may still load the target,
                    // so do not fall back to the current revision (that would
                    // under-reserve). Back off and retry resolving the target.
                    debug!(
                        "Transient failure resolving pending-update target revision {component_revision} for charge sizing, backing off and retrying"
                    );
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }

    /// Eviction module-charge accounting for an already-resident worker.
    ///
    /// Returns the component identity and compiled-module size of the module the
    /// worker is *currently* holding a charge for — the last known (loaded)
    /// revision, never a queued pending-update target. The pending update has not
    /// been applied yet, so the held charge is still keyed to the loaded
    /// revision; the eviction planner must use that same key and size, otherwise
    /// its refcount lookup and freed-bytes accounting would not match the charge
    /// that is actually released when the worker stops. Infallible: it reads only
    /// the persisted status, doing no metadata lookup.
    pub async fn resident_component_charge_requirement(
        &self,
    ) -> (ComponentId, ComponentRevision, u64) {
        let metadata = self.get_latest_worker_metadata().await;
        (
            self.owned_agent_id.component_id(),
            metadata.last_known_status.component_revision,
            metadata.last_known_status.component_size,
        )
    }

    /// Returns true if the worker is running, but it is not performing any invocations at the moment
    /// (ExecutionStatus::Suspended) and has no pending work that should keep the
    /// loaded worker resident while memory pressure is low.
    ///
    /// These workers can be stopped to free up available worker memory.
    pub async fn is_currently_idle_but_running(&self) -> bool {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => self.is_running_worker_idle(running).await,
            WorkerInstance::WaitingForPermit(_) => {
                debug!(
                    "Worker {} is waiting for permit, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            WorkerInstance::Unloaded { .. } => {
                debug!(
                    "Worker {} is unloaded, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            WorkerInstance::CleanupFailed(_) => {
                debug!(
                    "Worker {} has failed filesystem cleanup, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            // TODO: this probably wants to cooperate with memory free up
            WorkerInstance::Stopping(_) => {
                debug!(
                    "Worker {} is stopping, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
            // TODO: this probably wants to cooperate with memory free up
            WorkerInstance::Deleting => {
                debug!(
                    "Worker {} is deleting, cannot be used to free up memory",
                    self.owned_agent_id
                );
                false
            }
        }
    }

    async fn is_running_worker_idle(&self, running: &RunningWorker) -> bool {
        let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
        let has_pending_invocations = !self.pending_invocations().await.is_empty();
        let has_queued_internal_work = !running.queue.read().await.is_empty();
        let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
        let has_interrupt = running.interrupt_signal.lock().await.has_interrupt();
        let has_filesystem_effects = self.has_active_filesystem_effects(running);
        let has_concurrent_agent_permit =
            running.concurrent_agent_permit_held.load(Ordering::Acquire);

        debug!(
            "Worker {} idle check: waiting_for_command={waiting_for_command} has_pending_invocations={has_pending_invocations} has_queued_internal_work={has_queued_internal_work} has_resume_replay={has_resume_replay} has_interrupt={has_interrupt} has_filesystem_effects={has_filesystem_effects} has_concurrent_agent_permit={has_concurrent_agent_permit}",
            self.owned_agent_id
        );

        waiting_for_command
            && !has_pending_invocations
            && !has_queued_internal_work
            && !has_resume_replay
            && !has_interrupt
            && !has_filesystem_effects
            && !has_concurrent_agent_permit
    }

    fn has_active_filesystem_effects(&self, running: &RunningWorker) -> bool {
        running
            .filesystem_activity
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(ResidentFilesystemActivity::has_active_effects)
    }

    /// Returns `true` iff this worker currently has a loaded wasmtime instance
    /// (i.e. its [`WorkerInstance`] is in the `Running` state).
    ///
    /// `Worker` shells can outlive their wasmtime instance — for example after
    /// memory-pressure eviction unloads the instance but the shell stays alive
    /// in [`ActiveAgents`] so its caches (read-only cache, pending
    /// invocations, …) can keep serving. This accessor lets callers
    /// distinguish those two states.
    pub async fn is_loaded(&self) -> bool {
        matches!(&*self.instance.lock().await, WorkerInstance::Running(_))
    }

    /// Classifies the worker for eviction ordering under memory pressure.
    /// Returns `None` if the worker is not evictable.
    ///
    /// - `LoadedIdle`: resident in memory, not executing, no durable pending work.
    ///   Evicted first.
    /// - `WarmRunnable`: resident in memory, not executing, has durable pending
    ///   invocations. Evicted only when `LoadedIdle` workers are exhausted.
    /// - `None`: worker is actively executing, has non-durable in-memory work
    ///   pending, or is not loaded. Never evicted.
    pub async fn eviction_class(&self) -> Option<EvictionClass> {
        if self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await
            .is_some_and(|active_agent| {
                active_agent
                    .entity_slots()
                    .iter()
                    .any(|slot| slot.active_invocation_count() != 0)
            })
        {
            return None;
        }
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                let has_queued_internal_work = !running.queue.read().await.is_empty();
                let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
                let has_interrupt = running.interrupt_signal.lock().await.has_interrupt();
                let has_filesystem_effects = self.has_active_filesystem_effects(running);
                let has_concurrent_agent_permit =
                    running.concurrent_agent_permit_held.load(Ordering::Acquire);

                // Non-evictable if actively executing or has non-durable in-memory work
                if !running_worker_can_be_evicted(
                    waiting_for_command,
                    has_queued_internal_work,
                    has_resume_replay,
                    has_interrupt,
                    has_filesystem_effects,
                    has_concurrent_agent_permit,
                ) {
                    return None;
                }

                let has_pending_invocations = !self.pending_invocations().await.is_empty();
                if has_pending_invocations {
                    Some(EvictionClass::WarmRunnable)
                } else {
                    Some(EvictionClass::LoadedIdle)
                }
            }
            _ => None,
        }
    }

    /// Stop this worker if it matches the given eviction class.
    ///
    /// Re-checks the eviction classification under the instance lock to avoid
    /// races. Returns `true` if the worker was actually stopped.
    pub async fn stop_if_evictable(&self, target_class: EvictionClass) -> bool {
        self.stop_if_evictable_with_outcome(
            target_class,
            None,
            UnloadRequest::ordinary(UnloadReason::MemoryPressure),
        )
        .await
            != EvictionStopOutcome::Ineligible
    }

    pub(crate) async fn stop_if_evictable_with_outcome(
        &self,
        target_class: EvictionClass,
        expected_eligibility: Option<FilesystemPressureEligibility>,
        unload_request: UnloadRequest,
    ) -> EvictionStopOutcome {
        let active_agent = self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await;
        let reopen_entity_generation = match active_agent.as_ref() {
            Some(active_agent) => match active_agent.try_fence_idle_entity_bodies() {
                Some(reopen_generation) => reopen_generation,
                None => return EvictionStopOutcome::Ineligible,
            },
            None => None,
        };
        let mut instance_guard = self.lock_non_stopping_worker().await;
        let should_stop = match &*instance_guard {
            WorkerInstance::Running(running) => {
                let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                let has_queued_internal_work = !running.queue.read().await.is_empty();
                let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
                let has_interrupt = running.interrupt_signal.lock().await.has_interrupt();
                let has_filesystem_effects = self.has_active_filesystem_effects(running);
                let has_concurrent_agent_permit =
                    running.concurrent_agent_permit_held.load(Ordering::Acquire);

                if !running_worker_can_be_evicted(
                    waiting_for_command,
                    has_queued_internal_work,
                    has_resume_replay,
                    has_interrupt,
                    has_filesystem_effects,
                    has_concurrent_agent_permit,
                ) {
                    false
                } else {
                    let eligibility = self.current_filesystem_pressure_eligibility(running);
                    let has_pending_invocations = !self.pending_invocations().await.is_empty();
                    let current_class = if has_pending_invocations {
                        EvictionClass::WarmRunnable
                    } else {
                        EvictionClass::LoadedIdle
                    };
                    current_class.eviction_priority() <= target_class.eviction_priority()
                        && expected_eligibility.is_none_or(|expected| expected == eligibility)
                }
            }
            _ => false,
        };

        if should_stop {
            let stop_result = self
                .stop_internal_locked(
                    &mut instance_guard,
                    false,
                    None,
                    unload_request,
                    FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                    PendingLiveInvocationDisposition::Fail,
                )
                .await;
            drop(instance_guard);
            self.handle_stop_result(stop_result).await;
            match &*self.instance.lock().await {
                WorkerInstance::CleanupFailed(_) => EvictionStopOutcome::CleanupFailed,
                WorkerInstance::Unloaded { .. } => EvictionStopOutcome::Unloaded,
                _ => EvictionStopOutcome::CleanupFailed,
            }
        } else {
            drop(instance_guard);
            if let (Some(generation), Some(active_agent)) = (reopen_entity_generation, active_agent)
            {
                active_agent.reopen_entity_admission_if_generation(generation);
            }
            EvictionStopOutcome::Ineligible
        }
    }

    /// Gets the timestamp of the last time the execution status changed
    pub fn last_execution_state_change(&self) -> Timestamp {
        self.execution_status.read().unwrap().timestamp()
    }

    pub(crate) async fn filesystem_pressure_eligibility(
        &self,
    ) -> Option<FilesystemPressureEligibility> {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                Some(self.current_filesystem_pressure_eligibility(running))
            }
            _ => None,
        }
    }

    fn current_filesystem_pressure_eligibility(
        &self,
        running: &RunningWorker,
    ) -> FilesystemPressureEligibility {
        let idle_since = running.idle_since_millis.load(Ordering::Acquire);
        let last_effect_completion = running
            .filesystem_activity
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |runtime| runtime.last_effect_completion_millis());
        FilesystemPressureEligibility {
            idle_since,
            last_effect_completion,
        }
    }

    pub(crate) fn filesystem_pressure_eligible_since(
        eligibility: FilesystemPressureEligibility,
    ) -> u64 {
        eligibility
            .idle_since
            .max(eligibility.last_effect_completion)
    }

    /// Records a committed guest `memory.grow` without blocking the store callback.
    pub fn request_memory_grow(self: &Arc<Self>, delta: u64) {
        let growth = self.memory_growth.lock().unwrap();
        growth
            .delta
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                Some(pending.saturating_add(delta))
            })
            .ok();
        if !growth.job_queued.swap(true, Ordering::AcqRel) {
            self.state_actor.grow_memory(self.clone(), growth.clone());
        }
    }

    async fn persist_pending_memory_growth(self: &Arc<Self>, growth: Arc<PendingMemoryGrowth>) {
        loop {
            let delta = growth.delta.swap(0, Ordering::AcqRel);
            if delta > 0 {
                self.add_to_oplog(OplogEntry::grow_memory(delta)).await;
            }

            let current_growth = self.memory_growth.lock().unwrap();
            if Arc::ptr_eq(&current_growth, &growth) {
                growth.job_queued.store(false, Ordering::Release);
                if growth.delta.load(Ordering::Acquire) > 0
                    && !growth.job_queued.swap(true, Ordering::AcqRel)
                {
                    self.state_actor.grow_memory(self.clone(), growth.clone());
                }
                return;
            }

            // A successful update rotated this accumulator before queueing its
            // ordered entry. No producer can now add to it, so drain it fully
            // here to keep all preceding growth ahead of that entry.
            if growth.delta.load(Ordering::Acquire) == 0 {
                growth.job_queued.store(false, Ordering::Release);
                return;
            }
        }
    }

    pub(crate) async fn persist_successful_update(
        self: &Arc<Self>,
        linear_memory: &LinearMemoryTracker,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<EnvironmentPluginGrantId>,
    ) {
        let done = {
            let mut growth = self.memory_growth.lock().unwrap();
            let entry = OplogEntry::successful_update(
                target_revision,
                new_component_size,
                Some(linear_memory.current_bytes()),
                new_active_plugins,
            );
            *growth = Arc::new(PendingMemoryGrowth::default());
            self.state_actor
                .queue_ordered_oplog_entry(self.clone(), entry)
        };
        if done.await.is_err() {
            panic!(
                "Worker state actor for {} dropped an ordered oplog entry",
                self.owned_agent_id
            );
        }
    }

    pub(crate) fn request_memory_limit_interrupt(self: &Arc<Self>, memory: LinearMemoryTracker) {
        if !self
            .memory_limit_interrupt_queued
            .swap(true, Ordering::AcqRel)
        {
            self.state_actor.memory_limit_exceeded(self.clone(), memory);
        }
    }

    async fn request_agent_filesystem_limit_update(
        &self,
        allocated_bytes: u64,
    ) -> Result<(), WorkerExecutorError> {
        let receiver = {
            let instance = self.lock_non_stopping_worker().await;
            let WorkerInstance::Running(running) = &*instance else {
                return Ok(());
            };
            let (sender, receiver) = oneshot::channel();
            if running
                .sender
                .send(WorkerCommand::UpdateFilesystemLimit {
                    allocated_bytes,
                    sender,
                })
                .is_err()
            {
                return Ok(());
            }
            receiver
        };
        receiver.await.unwrap_or(Ok(()))
    }

    pub(crate) fn linear_memory_grant(&self) -> Arc<StdMutex<MemoryGrant>> {
        self.linear_memory_grant
            .lock()
            .unwrap()
            .clone()
            .expect("linear memory grant requested while worker is not running")
    }

    fn release_linear_memory_grant(&self) {
        if let Some(grant) = self.linear_memory_grant.lock().unwrap().take() {
            *grant.lock().unwrap() = MemoryGrant::inert(0);
        }
    }

    pub(crate) fn startup_linear_memory_bytes(&self) -> u64 {
        self.startup_linear_memory_bytes.load(Ordering::Acquire)
    }

    /// Bumps the read-only cache epoch, lazily invalidating all cached entries
    /// (the epoch is part of the cache key). Called from
    /// `DurableWorkerCtx::on_agent_invocation_success` immediately after a
    /// mutating invocation's `AgentInvocationFinished` is committed, so a
    /// cached read-only result keeps serving while the mutation is queued /
    /// running. Also called from
    /// `enqueue_update`/`revert` where the change is effectively in flight.
    pub(crate) fn bump_read_only_cache_epoch(&self) {
        self.read_only_cache_epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// Classifies a just-completed `AgentMethod` invocation by `method_name`
    /// against the worker's in-memory component snapshot.
    ///
    /// Returns `true` for any invocation that should invalidate cached
    /// read-only results: a non-read-only method, an unknown method (safe
    /// default), or an `AgentMethod` on a worker with no `parsed_agent_id`.
    /// Returns `false` only when the method is explicitly `read_only`.
    ///
    /// Used by `DurableWorkerCtx::on_agent_invocation_success` to decide
    /// whether to bump the read-only cache epoch on successful completion
    pub fn agent_method_invalidates_read_only_cache(&self, method_name: &str) -> bool {
        let snapshot = self.current_component.load();
        let metadata = &snapshot.metadata;
        let Some(parsed) = self.parsed_agent_id.as_ref() else {
            return true;
        };
        match read_only_cache::resolve_read_only_method(metadata, &parsed.agent_type, method_name) {
            Some(method) => method.read_only.is_none(),
            None => true,
        }
    }

    /// Enqueue invocation of an exported function. Uses
    /// `UnknownAssumeMutating` as a safe default for callers without
    /// classification; the epoch is no longer bumped at enqueue time.
    async fn enqueue_worker_invocation(
        &self,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        self.enqueue_worker_invocation_with_effect(
            invocation,
            read_only_cache::InvocationEffect::UnknownAssumeMutating,
        )
        .await
        .map(|_| ())
    }

    /// Enqueue invocation, classified by the caller. Passing `ReadOnly` for a
    /// mutating method would skip cache invalidation and produce stale reads.
    ///
    /// For `ReadOnly`, returns the epoch captured under the same instance lock
    /// that commits the pending entry. Populating the cache later must use
    /// this captured epoch, not the current one, to avoid storing a stale
    /// result under a post-mutation epoch.
    pub(crate) async fn enqueue_worker_invocation_with_effect(
        &self,
        invocation: AgentInvocation,
        read_only_cache_effect: read_only_cache::InvocationEffect,
    ) -> Result<Option<u64>, WorkerExecutorError> {
        // Carried on the span so the two sides of the hand-off share a searchable
        // key: the execution runs in its own trace, linked rather than nested, so
        // `idempotency_key` is what joins them. Left unset rather than empty when
        // there is none, so a search for one key cannot collide with every
        // keyless enqueue.
        let span = span!(
            Level::INFO,
            "enqueue_invocation",
            agent_id = %self.owned_agent_id.agent_id,
            idempotency_key = tracing::field::Empty,
            // Pairs with the `consumer` span the invocation loop creates when it
            // picks the work up.
            otel.kind = "producer"
        );
        if let Some(idempotency_key) = invocation.idempotency_key() {
            span.record("idempotency_key", tracing::field::display(idempotency_key));
        }

        async {
            self.accept_ephemeral_invocation(&invocation)?;
            let instance_guard = self.lock_non_stopping_worker().await;

            if instance_guard.is_deleting() {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot enqueue invocation to a deleting worker",
                ));
            };

            if let Some(err) = instance_guard.startup_failure() {
                if self.agent_mode() == AgentMode::Ephemeral {
                    crate::metrics::ephemeral::record_inactive_invocation_failure();
                }
                return Err(err.clone());
            }

            if let Some(idempotency_key) = invocation.idempotency_key() {
                let has_result = self
                    .invocation_results
                    .read()
                    .await
                    .contains_key(idempotency_key);
                let status = self.last_known_status.load();
                let is_pending = status
                    .pending_invocations
                    .iter()
                    .any(|entry| entry.has_idempotency_key(idempotency_key));
                let is_current = status.current_idempotency_key.as_ref() == Some(idempotency_key);
                if has_result || is_pending || is_current {
                    return Ok(None);
                }
            }

            let (idempotency_key, invocation_payload, invocation_context) = invocation.into_parts();
            let invocation_context = invocation_context
                .limit_depth(self.deps.config().limits.max_invocation_context_stack_depth);
            let invocation = AgentInvocation::from_parts(
                idempotency_key.clone(),
                invocation_payload.clone(),
                invocation_context.clone(),
            );
            let payload = self
                .oplog
                .upload_payload(&invocation_payload)
                .await
                .map_err(|e| {
                    WorkerExecutorError::invalid_request(format!(
                        "Failed to upload invocation payload: {e}"
                    ))
                })?;
            let invocation_context_spans = invocation_context.to_oplog_data();
            let entry = OplogEntry::pending_agent_invocation(
                idempotency_key,
                payload,
                invocation_context.trace_id,
                invocation_context.trace_states,
                invocation_context_spans,
            );
            let timestamped_invocation = TimestampedAgentInvocation {
                timestamp: entry.timestamp(),
                invocation,
            };

            // Snapshot the epoch under the instance lock that commits the
            // pending entry. Read-only captures the current epoch for later
            // cache fill. Mutating invocations no longer bump here — the bump
            // happens on *successful completion* in
            // `DurableWorkerCtx::on_agent_invocation_success`, so a cached
            // read-only result stays serviceable while the mutation is queued
            // / running. The populate-time recheck in
            // `populate_read_only_cache` covers the race where the mutation
            // completes before the read-only observer fills the cache.
            let read_only_epoch_snapshot = match read_only_cache_effect {
                read_only_cache::InvocationEffect::ReadOnly => {
                    Some(self.read_only_cache_epoch.load(Ordering::SeqCst))
                }
                read_only_cache::InvocationEffect::Mutating
                | read_only_cache::InvocationEffect::UnknownAssumeMutating => None,
            };

            self.add_and_commit_oplog_internal(&instance_guard, entry, None)
                .await;

            if let Some(idempotency_key) = timestamped_invocation.invocation.idempotency_key() {
                // Captured here, inside the producer span, because a consumer links
                // back to the *creation context* of the work rather than to wherever
                // the caller happened to call from.
                // Captured before taking the lock: `capture_current` reaches into the
                // tracing registry, which has no business happening inside this map's
                // critical section. Same-worker enqueues are already serialized by the
                // instance guard held above, so this keeps the critical section
                // minimal rather than fixing measurable contention.
                let origin = TraceOrigin::capture_current();
                self.external_invocation_origins
                    .write()
                    .await
                    .insert(idempotency_key.clone(), origin);
            }

            if let WorkerInstance::Running(running) = &*instance_guard {
                running.sender.send(WorkerCommand::WorkAvailable).unwrap();
            };

            drop(instance_guard);

            Ok(read_only_epoch_snapshot)
        }
        .instrument(span)
        .await
    }

    fn accept_ephemeral_invocation(
        &self,
        invocation: &AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let AgentInvocation::AgentMethod {
            idempotency_key, ..
        } = invocation
        else {
            return Ok(());
        };
        if self.agent_mode() != AgentMode::Ephemeral {
            return Ok(());
        }

        let mut state = self.ephemeral_invocation.lock().unwrap();
        let result = state.accept(idempotency_key);
        if result.is_err() {
            crate::metrics::ephemeral::record_inactive_invocation_failure();
        }
        result
    }

    pub async fn get_file_system_node(
        &self,
        path: CanonicalFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot access filesystem of a deleting worker",
            ));
        };

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        let (sender, receiver) = oneshot::channel();

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::GetFileSystemNode { path, sender });

        // Two cases here:
        // - Worker is running, we can send the invocation command, and the worker will look at the queue immediately
        // - Worker is starting, it will process the request when it is started

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        drop(instance_guard);

        receiver.await.unwrap()
    }

    pub async fn get_wallet_cards(&self) -> Result<Vec<StoredCard>, WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot access wallet of a deleting worker",
            ));
        };

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        let (sender, receiver) = oneshot::channel();

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::GetWalletCards { sender });

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        drop(instance_guard);

        let mut wallet = receiver.await.unwrap()?;
        let revoked_cards = self
            .get_last_known_status()
            .await
            .pending_card_events
            .into_iter()
            .filter_map(|pending_event| match pending_event.event {
                QueuedCardEvent::Revoke(event) => Some(event.card_id),
                QueuedCardEvent::Install(_)
                | QueuedCardEvent::TransferStarted(_)
                | QueuedCardEvent::TransferReceived(_) => None,
            })
            .collect::<HashSet<_>>();
        wallet.retain(|card| !revoked_cards.contains(&card.card_id()));
        Ok(wallet)
    }

    pub async fn read_file(
        &self,
        path: CanonicalFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot access filesystem of a deleting worker",
            ));
        };

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        let (sender, receiver) = oneshot::channel();

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::ReadFile { path, sender });

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        drop(instance_guard);

        receiver.await.unwrap()
    }

    pub async fn await_ready_to_process_commands(&self) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot await readiness of a deleting worker",
            ));
        };

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        // An unloaded worker has no invocation loop that could drain a queued readiness marker,
        // and this method must not start one: the worker already reached a stopped state (for
        // example a debugging worker that suspended itself after replaying to its target), which
        // is exactly the "not processing anything until the next explicit start" condition
        // callers wait for.
        if matches!(&*instance_guard, WorkerInstance::Unloaded { .. }) {
            return Ok(());
        }

        let (sender, receiver) = oneshot::channel();

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender });

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        drop(instance_guard);

        // The marker is resolved either by the running invocation loop once it becomes idle, or
        // by the stop transition when the worker stops without draining it (see
        // `resolve_pending_readiness_awaiters_on_stop`), so this wait cannot hang on a worker
        // that suspends mid-invocation.
        receiver.await.unwrap()
    }

    async fn pending_streaming_invocation_entry(
        &self,
        invocation: AgentInvocation,
    ) -> Result<OplogEntry, WorkerExecutorError> {
        let (idempotency_key, invocation_payload, invocation_context) = invocation.into_parts();
        let invocation_context = invocation_context
            .limit_depth(self.deps.config().limits.max_invocation_context_stack_depth);
        let payload = self
            .oplog
            .upload_payload(&invocation_payload)
            .await
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "Failed to upload invocation payload: {error}"
                ))
            })?;
        let invocation_context_spans = invocation_context.to_oplog_data();
        Ok(OplogEntry::pending_agent_invocation(
            idempotency_key,
            payload,
            invocation_context.trace_id,
            invocation_context.trace_states,
            invocation_context_spans,
        ))
    }

    pub(crate) async fn accept_durable_streaming_invocation(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
        let result = self
            .accept_durable_streaming_invocation_unmetered(request)
            .await;
        match &result {
            Ok(acceptance) => crate::metrics::durable_stream::record_attempt(
                "start",
                if acceptance.replayed {
                    "replayed"
                } else {
                    "accepted"
                },
                (!acceptance.replayed).then_some(1),
            ),
            Err(error) => crate::metrics::durable_stream::record_attempt(
                "start",
                durable_stream_attempt_error_outcome(error),
                None,
            ),
        }
        result
    }

    async fn accept_durable_streaming_invocation_unmetered(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;
        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot invoke a deleting worker",
            ));
        }
        if let Some(error) = instance_guard.startup_failure() {
            return Err(error.clone());
        }

        let session_key = request.attempt.session_key.clone();
        let records = self.stream_session_records(&session_key).await?;
        let mut prepared_records = records.iter().filter_map(|record| match record {
            StreamSessionRecordV1::Prepared(prepared) => Some(prepared.clone()),
            _ => None,
        });
        let existing_prepared = prepared_records.next();
        if existing_prepared.is_some() && prepared_records.next().is_some() {
            return Err(WorkerExecutorError::runtime(
                "durable Stream Session contains multiple Prepared records",
            ));
        }
        let producer = self.durable_stream_producer().await?;
        let mut invocation = Some(request.invocation);
        let mut acceptance_committed = Some(request.acceptance_committed);
        let mut attached_during_prepare = false;
        if !request.registrations.is_empty() && !request.foreign_mappings.is_empty() {
            return Err(WorkerExecutorError::invalid_request(
                "durable invocation inputs cannot mix inline and agent-hosted stream mappings",
            ));
        }
        let foreign_mappings = request.foreign_mappings.clone();

        let prepared = if let Some(prepared) = existing_prepared {
            let mut requested_attempt = request.attempt.clone();
            let requested_handles = if foreign_mappings.is_empty() {
                let mut handles = Vec::with_capacity(request.registrations.len());
                for (_, registration) in &request.registrations {
                    handles.push(
                        producer
                            .validate_registration(registration)
                            .await
                            .map_err(|_| {
                                WorkerExecutorError::invalid_request(
                                    "IdempotencyConflict: persisted durable stream registration does not match the invocation descriptor",
                                )
                            })?,
                    );
                }
                handles
            } else {
                foreign_mappings
                    .iter()
                    .map(|mapping| mapping.handle.clone())
                    .collect()
            };
            requested_attempt.invocation.stream_handles = requested_handles.clone();
            if !persisted_stream_descriptor_matches(
                &prepared.attempt.invocation,
                &requested_attempt.invocation,
            ) {
                return Err(WorkerExecutorError::invalid_request(
                    "IdempotencyConflict: the invocation key is already bound to a different durable stream descriptor",
                ));
            }
            if prepared.attempt.attempt_id != request.attempt.attempt_id
                || !stream_attempt_matches(&prepared.attempt, &requested_attempt)
            {
                return Err(WorkerExecutorError::invalid_request(
                    "AttemptConflict: the durable session start attempt does not exactly match the persisted attempt",
                ));
            }
            let requested_mappings = if foreign_mappings.is_empty() {
                request
                    .registrations
                    .iter()
                    .map(|(transport_stream_id, _)| *transport_stream_id)
                    .zip(requested_handles)
                    .map(
                        |(transport_stream_id, handle)| StreamSessionMappingRecordV1 {
                            transport_stream_id,
                            handle,
                            role: SessionStreamRoleV1::Input,
                        },
                    )
                    .collect()
            } else {
                foreign_mappings.clone()
            };
            if prepared.stream_mappings != requested_mappings {
                return Err(WorkerExecutorError::invalid_request(
                    "AttemptConflict: the durable session stream mappings do not exactly match the persisted attempt",
                ));
            }
            prepared
        } else if !foreign_mappings.is_empty() {
            let mut attempt = request.attempt.clone();
            attempt.invocation.stream_handles = foreign_mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect();
            let prepared = StreamSessionPreparedRecordV1 {
                format_version: 1,
                attempt,
                stream_mappings: foreign_mappings.clone(),
            };
            producer
                .append_session_record(StreamSessionRecordV1::Prepared(prepared.clone()))
                .await
                .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            prepared
        } else {
            let pending = self
                .pending_streaming_invocation_entry(
                    invocation
                        .take()
                        .expect("fresh durable session has an invocation"),
                )
                .await?;
            let attempt = request.attempt.clone();
            let prepared = producer
                .prepare_session(
                    request.registrations.clone(),
                    pending,
                    acceptance_committed
                        .take()
                        .expect("fresh durable session has a commit notification"),
                    move |bindings| {
                        let mut attempt = attempt;
                        attempt.invocation.stream_handles =
                            bindings.iter().map(|(_, handle)| handle.clone()).collect();
                        StreamSessionPreparedRecordV1 {
                            format_version: 1,
                            stream_mappings: bindings
                                .into_iter()
                                .map(
                                    |(transport_stream_id, handle)| StreamSessionMappingRecordV1 {
                                        transport_stream_id,
                                        handle,
                                        role: SessionStreamRoleV1::Input,
                                    },
                                )
                                .collect(),
                            attempt,
                        }
                    },
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            attached_during_prepare = true;
            prepared
        };

        let mut attached_records = records.iter().filter_map(|record| match record {
            StreamSessionRecordV1::Attached(attached) => Some(attached),
            _ => None,
        });
        let attached = attached_records.next();
        if attached.is_some() && attached_records.next().is_some() {
            return Err(WorkerExecutorError::runtime(
                "durable Stream Session contains multiple Attached records",
            ));
        }
        if let Some(attached) = attached
            && (attached.session_key != prepared.attempt.session_key
                || attached.attachment_id != prepared.attempt.attachment_id
                || attached.attempt_id != prepared.attempt.attempt_id
                || attached.epoch != 1
                || !matches!(
                    self.oplog.read(attached.pending_invocation_oplog_index).await,
                    OplogEntry::PendingAgentInvocation { idempotency_key, .. }
                        if idempotency_key == prepared.attempt.session_key.idempotency_key
                ))
        {
            return Err(WorkerExecutorError::runtime(
                "durable Attached record does not exactly identify its Prepared attempt and pending invocation",
            ));
        }
        let already_attached = attached.is_some();

        let streams = DurableSessionStreams::new(
            producer,
            self.oplog.clone(),
            session_key,
            prepared.stream_mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }),
        )
        .with_attachment(1, prepared.attempt.attempt_id)
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?)
        .with_input_schema(
            request.input_schema,
            prepared.attempt.invocation.target_component_revision,
            request.input_element_types,
        );
        for mapping in &foreign_mappings {
            streams
                .prepare_foreign_mapping(mapping.clone(), 1)
                .await
                .map_err(WorkerExecutorError::runtime)?;
        }
        if !already_attached && !attached_during_prepare {
            let pending = self
                .pending_streaming_invocation_entry(
                    invocation
                        .take()
                        .expect("unattached durable session has an invocation"),
                )
                .await?;
            let attached_attempt = prepared.attempt.clone();
            self.oplog
                .add_pair(
                    pending,
                    Box::new(move |pending_invocation_oplog_index| {
                        OplogEntry::stream_session(OplogPayload::Inline(Box::new(
                            StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
                                format_version: 1,
                                session_key: attached_attempt.session_key,
                                attachment_id: attached_attempt.attachment_id,
                                attempt_id: attached_attempt.attempt_id,
                                epoch: 1,
                                pending_invocation_oplog_index,
                            }),
                        )))
                    }),
                )
                .await;
            streams
                .commit_consumer_journal()
                .await
                .map_err(WorkerExecutorError::runtime)?;
        }
        for mapping in &foreign_mappings {
            let mut retry_delay = Duration::from_millis(10);
            loop {
                match streams.activate_foreign_mapping(mapping.clone(), 1).await {
                    Ok(()) => break,
                    Err(error) => {
                        warn!(
                            session = %prepared.attempt.session_key.idempotency_key,
                            stream = %mapping.handle.stream_id,
                            %error,
                            "retrying durable foreign topology activation after attachment commit"
                        );
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(1));
                    }
                }
            }
        }
        if already_attached {
            let _ = acceptance_committed
                .take()
                .expect("persisted durable session has a commit notification")
                .send(());
        } else if !attached_during_prepare {
            self.state_actor
                .commit_and_update_state_notifying(
                    CommitLevel::Always,
                    acceptance_committed
                        .take()
                        .expect("legacy durable session has a commit notification"),
                )
                .await;
            self.state_actor.notify_status_changed();
        }
        if !already_attached && let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        }
        drop(instance_guard);
        Ok(DurableStreamingInvocationAcceptance {
            prepared,
            streams,
            replayed: already_attached,
        })
    }

    pub(crate) async fn resume_durable_streaming_invocation(
        self: &Arc<Self>,
        attempt: ResumeAttemptDescriptorV1,
    ) -> Result<DurableStreamingResumeAcceptance, WorkerExecutorError> {
        let operation = match attempt.operation {
            golem_common::model::durable_stream::StreamResumeOperationV1::Resume => "resume",
            golem_common::model::durable_stream::StreamResumeOperationV1::Takeover => "takeover",
        };
        let result = self
            .resume_durable_streaming_invocation_unmetered(attempt)
            .await;
        match &result {
            Ok(acceptance) => crate::metrics::durable_stream::record_attempt(
                operation,
                if acceptance.replayed {
                    "replayed"
                } else {
                    "accepted"
                },
                (!acceptance.replayed).then_some(acceptance.epoch),
            ),
            Err(error) => crate::metrics::durable_stream::record_attempt(
                operation,
                durable_stream_attempt_error_outcome(error),
                None,
            ),
        }
        result
    }

    async fn resume_durable_streaming_invocation_unmetered(
        self: &Arc<Self>,
        attempt: ResumeAttemptDescriptorV1,
    ) -> Result<DurableStreamingResumeAcceptance, WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;
        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot resume a deleting worker",
            ));
        }
        if let Some(error) = instance_guard.startup_failure() {
            return Err(error.clone());
        }

        let records = self.stream_session_records(&attempt.session_key).await?;
        let prepared = records
            .iter()
            .find_map(|record| match record {
                StreamSessionRecordV1::Prepared(record) => Some(record.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "NotFound: durable Stream Session preparation was not found",
                )
            })?;
        if prepared.attempt.attempt_id == attempt.attempt_id {
            return Err(WorkerExecutorError::invalid_request(
                "AttemptConflict: a Start attempt ID cannot be reused for resume or takeover",
            ));
        }

        let mut mappings = prepared.stream_mappings.clone();
        for record in &records {
            match record {
                StreamSessionRecordV1::Mapping(record) => {
                    if !mappings.contains(&record.mapping) {
                        mappings.push(record.mapping.clone());
                    }
                }
                StreamSessionRecordV1::InvocationResult(record) => {
                    for mapping in &record.stream_mappings {
                        if !mappings.contains(mapping) {
                            mappings.push(mapping.clone());
                        }
                    }
                }
                StreamSessionRecordV1::TopologyPrepared(record) => {
                    if !mappings.contains(&record.mapping) {
                        mappings.push(record.mapping.clone());
                    }
                }
                StreamSessionRecordV1::TopologyActivated(record) => {
                    if !mappings.contains(&record.mapping) {
                        mappings.push(record.mapping.clone());
                    }
                }
                StreamSessionRecordV1::ConsumerItemValue(record) => {
                    for mapping in &record.recursive_mappings {
                        if !mappings.contains(mapping) {
                            mappings.push(mapping.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        let producer = self.durable_stream_producer().await?;
        let make_streams =
            |epoch, attempt_id| -> Result<DurableSessionStreams, WorkerExecutorError> {
                Ok(DurableSessionStreams::new(
                    producer.clone(),
                    self.oplog.clone(),
                    attempt.session_key.clone(),
                    mappings.iter().map(|mapping| {
                        (
                            mapping.transport_stream_id,
                            mapping.handle.clone(),
                            mapping.role,
                        )
                    }),
                )
                .with_attachment(epoch, attempt_id)
                .with_rpc(self.rpc())
                .with_consumer_journal(self.durable_stream_consumer_journal())
                .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?))
            };

        for record in &records {
            if let StreamSessionRecordV1::ResumeAttempt(existing) = record
                && existing.attempt.attempt_id == attempt.attempt_id
            {
                if existing.attempt != attempt {
                    return Err(WorkerExecutorError::invalid_request(
                        "AttemptConflict: the resume attempt does not exactly match its persisted descriptor",
                    ));
                }
                let streams = make_streams(existing.accepted_epoch, attempt.attempt_id)?;
                if streams.ensure_current_attachment().await.is_ok() {
                    for mapping in &mappings {
                        if !producer.owns_handle_identity(&mapping.handle) {
                            streams
                                .prepare_foreign_mapping(mapping.clone(), existing.accepted_epoch)
                                .await
                                .map_err(WorkerExecutorError::runtime)?;
                            streams
                                .activate_foreign_mapping(mapping.clone(), existing.accepted_epoch)
                                .await
                                .map_err(WorkerExecutorError::runtime)?;
                        }
                    }
                }
                drop(instance_guard);
                return Ok(DurableStreamingResumeAcceptance {
                    prepared,
                    mappings,
                    streams,
                    epoch: existing.accepted_epoch,
                    replayed: true,
                });
            }
        }

        if attempt.expected_callee_fingerprint != self.get_initial_worker_metadata().fingerprint
            || attempt.expected_callee_fingerprint != prepared.attempt.expected_callee_fingerprint
            || attempt.attachment_id != prepared.attempt.attachment_id
        {
            return Err(WorkerExecutorError::invalid_request(
                "IncarnationMismatch: resume does not identify the persisted callee attachment",
            ));
        }
        if attempt.effective_identity != prepared.attempt.effective_identity {
            return Err(WorkerExecutorError::invalid_request(
                "Unauthorized: resume identity differs from the pinned principal and grant",
            ));
        }

        let mut authority = None;
        for record in &records {
            match record {
                StreamSessionRecordV1::Attached(record) => {
                    if authority.is_some() {
                        return Err(WorkerExecutorError::runtime(
                            "durable Stream Session contains a repeated initial attachment",
                        ));
                    }
                    authority = Some((record.epoch, record.attempt_id, true));
                }
                StreamSessionRecordV1::ResumeAttempt(record) => {
                    let Some((epoch, _, _)) = authority else {
                        return Err(WorkerExecutorError::runtime(
                            "durable resume precedes initial attachment",
                        ));
                    };
                    if record.attempt.expected_epoch != epoch
                        || record.accepted_epoch
                            != epoch.checked_add(1).ok_or_else(|| {
                                WorkerExecutorError::runtime(
                                    "durable attachment epoch cannot advance past u64::MAX",
                                )
                            })?
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable resume contains an invalid epoch transition",
                        ));
                    }
                    authority = Some((record.accepted_epoch, record.attempt.attempt_id, true));
                }
                StreamSessionRecordV1::Detached(record) => {
                    let Some((epoch, owner_attempt, attached)) = authority else {
                        return Err(WorkerExecutorError::runtime(
                            "durable detach precedes initial attachment",
                        ));
                    };
                    if record.epoch != epoch || record.owner_attempt_id != owner_attempt {
                        return Err(WorkerExecutorError::runtime(
                            "durable detach does not match the current attachment",
                        ));
                    }
                    if attached {
                        authority = Some((epoch, owner_attempt, false));
                    }
                }
                _ => {}
            }
        }
        let (current_epoch, current_attempt_id, attached) = authority.ok_or_else(|| {
            WorkerExecutorError::runtime("durable Stream Session has no attachment authority")
        })?;
        if attempt.expected_epoch < current_epoch {
            return Err(WorkerExecutorError::invalid_request(format!(
                "StaleEpoch: current attachment epoch is {current_epoch}"
            )));
        }
        if attempt.expected_epoch > current_epoch {
            return Err(WorkerExecutorError::invalid_request(format!(
                "InvalidEpoch: current attachment epoch is {current_epoch}"
            )));
        }
        match (attempt.operation, attached) {
            (golem_common::model::durable_stream::StreamResumeOperationV1::Resume, false)
            | (golem_common::model::durable_stream::StreamResumeOperationV1::Takeover, true) => {}
            _ => {
                return Err(WorkerExecutorError::invalid_request(
                    "InvalidAttachmentState: resume requires Detached and takeover requires Attached",
                ));
            }
        }
        let current_streams = make_streams(current_epoch, current_attempt_id)?;
        current_streams
            .validate_resume_cursors(&attempt.cursors)
            .await
            .map_err(WorkerExecutorError::invalid_request)?;

        let accepted_epoch = current_epoch.checked_add(1).ok_or_else(|| {
            WorkerExecutorError::invalid_request(
                "ResourceExhausted: durable attachment epoch cannot wrap",
            )
        })?;
        current_streams
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: attempt.clone(),
                accepted_epoch,
            })
            .await
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;

        let streams = make_streams(accepted_epoch, attempt.attempt_id)?;
        for mapping in &mappings {
            if !producer.owns_handle_identity(&mapping.handle) {
                streams
                    .prepare_foreign_mapping(mapping.clone(), accepted_epoch)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                streams
                    .activate_foreign_mapping(mapping.clone(), accepted_epoch)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
            }
        }
        drop(instance_guard);
        Ok(DurableStreamingResumeAcceptance {
            prepared,
            mappings,
            streams,
            epoch: accepted_epoch,
            replayed: false,
        })
    }

    async fn stream_session_records(
        &self,
        session_key: &golem_common::base_model::durable_stream::StreamSessionKeyV1,
    ) -> Result<Vec<StreamSessionRecordV1>, WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(Vec::new());
        }
        let entries = self
            .oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await;
        let mut records = Vec::new();
        for (_, entry) in entries {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            validate_stream_session_record(&record)?;
            if stream_session_record_key(&record) == Some(session_key) {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub(crate) async fn rehydrate_durable_streaming_invocation(
        &self,
        invocation: AgentInvocation,
    ) -> Result<AgentInvocation, WorkerExecutorError> {
        let Some(idempotency_key) = invocation.idempotency_key().cloned() else {
            return Ok(invocation);
        };
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(invocation);
        }
        let entries = self
            .oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await;
        let mut prepared = None;
        for (_, entry) in entries {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            validate_stream_session_record(&record)?;
            if let StreamSessionRecordV1::Prepared(candidate) = record
                && candidate.attempt.session_key.idempotency_key == idempotency_key
            {
                prepared = Some(candidate);
                break;
            }
        }
        let Some(prepared) = prepared else {
            return Ok(invocation);
        };
        if prepared.attempt.invocation.stream_handles.is_empty() {
            return Ok(invocation);
        }
        let producer = self.durable_stream_producer().await?;
        let streams = DurableSessionStreams::new(
            producer,
            self.oplog.clone(),
            prepared.attempt.session_key.clone(),
            prepared.stream_mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }),
        )
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        streams
            .recover_nested_input_mappings()
            .await
            .map_err(WorkerExecutorError::runtime)?;
        streams
            .recover_session_mappings()
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let value = golem_api_grpc::proto::golem::schema::SchemaValue::decode(
            prepared.attempt.invocation.invocation_value.as_slice(),
        )
        .map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "failed to decode persisted durable invocation input: {error}"
            ))
        })?;
        let input = streams
            .decode_initial(
                value,
                &prepared.attempt.invocation.stream_handles,
                SessionStreamRoleV1::Input,
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;
        Ok(replace_agent_method_input(invocation, input))
    }

    pub(crate) async fn materialize_durable_streaming_result(
        &self,
        idempotency_key: &IdempotencyKey,
        value: golem_common::schema::SchemaValue,
        graph: &golem_common::schema::SchemaGraph,
        root: &golem_common::schema::SchemaType,
        component_revision: ComponentRevision,
    ) -> Result<golem_common::schema::SchemaValue, WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(value);
        }
        let entries = self
            .oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await;
        let mut prepared = None;
        for (_, entry) in entries {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            validate_stream_session_record(&record)?;
            if let StreamSessionRecordV1::Prepared(candidate) = record
                && candidate.attempt.session_key.idempotency_key == *idempotency_key
            {
                prepared = Some(candidate);
                break;
            }
        }
        let Some(prepared) = prepared else {
            if contains_stream(&value) {
                return Err(WorkerExecutorError::runtime(
                    "live stream at a materializing invocation boundary without a durable Stream Session",
                ));
            }
            return Ok(value);
        };
        let requires_attachment =
            stream_effective_identity_is_agent(&prepared.attempt.effective_identity);
        let mut streams = DurableSessionStreams::new(
            self.durable_stream_producer().await?,
            self.oplog.clone(),
            prepared.attempt.session_key,
            prepared.stream_mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }),
        )
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        if requires_attachment {
            streams = streams.require_attachment_before_production();
        }
        streams
            .materialize_result(value, graph, root, component_revision)
            .await
            .map_err(WorkerExecutorError::runtime)
    }

    pub(crate) async fn fail_durable_streaming_session(
        &self,
        idempotency_key: &IdempotencyKey,
        details: String,
    ) -> Result<(), WorkerExecutorError> {
        self.finish_durable_streaming_session(idempotency_key, Err(details))
            .await
    }

    pub(crate) async fn complete_durable_streaming_session(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<(), WorkerExecutorError> {
        self.finish_durable_streaming_session(idempotency_key, Ok(()))
            .await
    }

    async fn finish_durable_streaming_session(
        &self,
        idempotency_key: &IdempotencyKey,
        result: Result<(), String>,
    ) -> Result<(), WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(());
        }
        let mut prepared = None;
        let mut result_mappings = Vec::new();
        let mut finished = false;
        for (_, entry) in self
            .oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            validate_stream_session_record(&record)?;
            if stream_session_record_key(&record)
                .is_none_or(|key| key.idempotency_key != *idempotency_key)
            {
                continue;
            }
            match record {
                StreamSessionRecordV1::Prepared(record) => prepared = Some(record),
                StreamSessionRecordV1::InvocationResult(record) => {
                    result_mappings = record.stream_mappings
                }
                StreamSessionRecordV1::Finished(_) => finished = true,
                _ => {}
            }
        }
        let Some(prepared) = prepared else {
            return Ok(());
        };
        if finished {
            return Ok(());
        }
        let mappings = prepared
            .stream_mappings
            .iter()
            .chain(&result_mappings)
            .map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            });
        let streams = DurableSessionStreams::new(
            self.durable_stream_producer().await?,
            self.oplog.clone(),
            prepared.attempt.session_key,
            mappings,
        )
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        match result {
            Ok(()) => streams.complete().await,
            Err(details) => streams.fail(details).await,
        }
        .map_err(WorkerExecutorError::runtime)?;
        Ok(())
    }

    async fn recover_finished_durable_streaming_sessions(&self) -> Result<(), WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(());
        }
        let mut unfinished = HashSet::new();
        for (_, entry) in self
            .oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            validate_stream_session_record(&record)?;
            match record {
                StreamSessionRecordV1::Prepared(prepared) => {
                    unfinished.insert(prepared.attempt.session_key.idempotency_key);
                }
                StreamSessionRecordV1::Finished(finished) => {
                    unfinished.remove(&finished.session_key.idempotency_key);
                }
                _ => {}
            }
        }

        let status = self.get_last_known_status().await;
        for idempotency_key in unfinished {
            let mut invocation_result = {
                self.invocation_results
                    .read()
                    .await
                    .get(&idempotency_key)
                    .cloned()
            };
            let Some(invocation_result) = invocation_result.as_mut() else {
                continue;
            };
            invocation_result
                .cache(
                    &self.owned_agent_id,
                    self.agent_mode(),
                    self.initial_worker_metadata.fingerprint,
                    self,
                )
                .await;
            match lookup_result_from_cached_result(
                &status,
                &idempotency_key,
                invocation_result.clone(),
            ) {
                LookupResult::Complete(Ok(_)) => {
                    self.complete_durable_streaming_session(&idempotency_key)
                        .await?;
                }
                LookupResult::Complete(Err(error)) => {
                    self.fail_durable_streaming_session(&idempotency_key, error.to_string())
                        .await?;
                }
                LookupResult::New | LookupResult::Pending | LookupResult::Interrupted => {}
            }
        }
        Ok(())
    }

    pub(crate) async fn durable_stream_producer(
        &self,
    ) -> Result<Arc<DurableStreamProducer>, WorkerExecutorError> {
        self.durable_stream_producer
            .get_or_try_init(|| async {
                let state_actor = self.state_actor.clone();
                let commit: DurableStreamCommit = Arc::new(move |committed| {
                    let state_actor = state_actor.clone();
                    Box::pin(async move {
                        let (_, changed) = if let Some(committed) = committed {
                            state_actor
                                .commit_and_update_state_notifying(CommitLevel::Always, committed)
                                .await
                        } else {
                            state_actor
                                .commit_and_update_state(CommitLevel::Always)
                                .await
                        };
                        if changed {
                            state_actor.notify_status_changed();
                        }
                    })
                });
                DurableStreamProducer::load_with_commit(
                    self.oplog.clone(),
                    self.owned_agent_id.environment_id,
                    self.owned_agent_id.agent_id.clone(),
                    self.initial_worker_metadata.fingerprint,
                    Some(
                        self.deps
                            .config()
                            .limits
                            .live_stream_event_broadcast_capacity
                            .get(),
                    ),
                    commit,
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
            })
            .await
            .cloned()
    }

    fn durable_stream_consumer_auth_ctx(&self) -> Result<AuthCtx, WorkerExecutorError> {
        let parsed_agent_id = self.parsed_agent_id.as_ref().ok_or_else(|| {
            WorkerExecutorError::runtime(
                "durable stream consumer is not a registered agent instance",
            )
        })?;
        let surface = agent_effective_surface_from_component_metadata(
            self.current_component.load().as_ref(),
            &self.owned_agent_id,
            parsed_agent_id,
        )?;
        Ok(AuthCtx::agent_with_effective_surface(
            self.initial_worker_metadata.created_by,
            self.initial_worker_metadata.created_by_email.clone(),
            surface,
        ))
    }

    async fn reconcile_durable_stream_attachments(&self) -> Result<(), WorkerExecutorError> {
        let probe =
            DbDirectStreamAttachmentConsumerProbe::new(self.worker_service(), self.oplog_service());
        let config = &self.deps.config().durable_stream;
        self.durable_stream_producer()
            .await?
            .reconcile_attachments_configured(
                Timestamp::now_utc().to_millis(),
                u64::try_from(config.renewal_interval.as_millis()).unwrap_or(u64::MAX),
                config.reconciliation_batch_size,
                &probe,
            )
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(())
    }

    async fn recover_durable_stream_topologies(&self) -> Result<(), WorkerExecutorError> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(());
        }
        let mut topologies = HashMap::new();
        let mut prepared_attempts = HashMap::new();
        let mut attached_sessions = HashMap::new();
        let mut session_authorities = HashMap::new();
        let mut pending_invocations = HashMap::new();
        let mut finalized_attachments = HashSet::new();
        let mut finished_sessions = HashSet::new();
        let mut consumer_deleting = false;
        for (oplog_index, entry) in self
            .oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
            if let OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            } = &entry
            {
                pending_invocations.insert(oplog_index, idempotency_key.clone());
            }
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            validate_stream_session_record(&record)?;
            match record {
                StreamSessionRecordV1::Prepared(record) => {
                    if prepared_attempts
                        .insert(
                            record.attempt.session_key.clone(),
                            record.attempt.attempt_id,
                        )
                        .is_some()
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable Stream Session contains multiple Prepared records",
                        ));
                    }
                }
                StreamSessionRecordV1::Attached(record) => {
                    if attached_sessions
                        .insert(record.session_key.clone(), record.clone())
                        .is_some()
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable Stream Session contains multiple Attached records",
                        ));
                    }
                    if session_authorities
                        .insert(
                            record.session_key,
                            (record.attachment_id, record.epoch, record.attempt_id, true),
                        )
                        .is_some()
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable Stream Session contains multiple attachment authorities",
                        ));
                    }
                }
                StreamSessionRecordV1::ResumeAttempt(record) => {
                    let Some((attachment_id, epoch, attempt_id, attached)) =
                        session_authorities.get_mut(&record.attempt.session_key)
                    else {
                        return Err(WorkerExecutorError::runtime(
                            "durable resume precedes initial attachment",
                        ));
                    };
                    if record.attempt.attachment_id != *attachment_id
                        || record.attempt.expected_epoch != *epoch
                        || record.accepted_epoch != epoch.checked_add(1).unwrap_or_default()
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable resume contains an invalid attachment transition",
                        ));
                    }
                    *epoch = record.accepted_epoch;
                    *attempt_id = record.attempt.attempt_id;
                    *attached = true;
                }
                StreamSessionRecordV1::Detached(record) => {
                    let Some((attachment_id, epoch, attempt_id, attached)) =
                        session_authorities.get_mut(&record.session_key)
                    else {
                        return Err(WorkerExecutorError::runtime(
                            "durable detach precedes initial attachment",
                        ));
                    };
                    if record.attachment_id != *attachment_id
                        || record.epoch != *epoch
                        || record.owner_attempt_id != *attempt_id
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable detach does not match the current attachment",
                        ));
                    }
                    *attached = false;
                }
                StreamSessionRecordV1::ConsumerDeleting(record)
                    if record.consumer_environment_id == self.owned_agent_id.environment_id
                        && record.consumer == self.owned_agent_id.agent_id
                        && record.consumer_fingerprint
                            == self.initial_worker_metadata.fingerprint =>
                {
                    consumer_deleting = true;
                }
                StreamSessionRecordV1::AttachmentFinalized(record) => {
                    finalized_attachments.insert(record.key);
                }
                StreamSessionRecordV1::TopologyPrepared(record) => {
                    let slot = (
                        record.session_key.clone(),
                        record.attachment.attachment_id,
                        record.attachment.stream_id,
                        record.attachment.epoch,
                        record.mapping.transport_stream_id,
                        record.mapping.role,
                    );
                    match topologies.get(&slot) {
                        Some((attachment, mapping))
                            if attachment != &record.attachment || mapping != &record.mapping =>
                        {
                            return Err(WorkerExecutorError::runtime(
                                "conflicting durable topology preparation",
                            ));
                        }
                        Some(_) => {}
                        None => {
                            topologies.insert(slot, (record.attachment, record.mapping));
                        }
                    }
                }
                StreamSessionRecordV1::TopologyActivated(record) => {
                    let slot = (
                        record.session_key,
                        record.attachment.attachment_id,
                        record.attachment.stream_id,
                        record.attachment.epoch,
                        record.mapping.transport_stream_id,
                        record.mapping.role,
                    );
                    if topologies.get(&slot) != Some(&(record.attachment, record.mapping)) {
                        return Err(WorkerExecutorError::runtime(
                            "durable topology activation has no exact preparation",
                        ));
                    }
                }
                StreamSessionRecordV1::Finished(record) => {
                    finished_sessions.insert(record.session_key);
                }
                _ => {}
            }
        }
        topologies.retain(|(session_key, _, _, _, _, _), (attachment, _)| {
            let local_session_authority = session_key.callee_environment_id
                == self.owned_agent_id.environment_id
                && session_key.callee == self.owned_agent_id.agent_id
                && session_key.callee_fingerprint == self.initial_worker_metadata.fingerprint;
            !(local_session_authority && finished_sessions.contains(session_key))
                && !finalized_attachments.contains(attachment)
        });
        if consumer_deleting || topologies.is_empty() {
            return Ok(());
        }
        let producer = self.durable_stream_producer().await?;
        let auth_ctx = self.durable_stream_consumer_auth_ctx()?;
        let mut first_error = None;
        for ((session_key, _, _, _, _, _), (attachment, mapping)) in topologies {
            let local_session_authority = session_key.callee_environment_id
                == self.owned_agent_id.environment_id
                && session_key.callee == self.owned_agent_id.agent_id
                && session_key.callee_fingerprint == self.initial_worker_metadata.fingerprint;
            if local_session_authority {
                let Some(prepared_attempt) = prepared_attempts.get(&session_key) else {
                    return Err(WorkerExecutorError::runtime(
                        "local durable topology has no Prepared session authority",
                    ));
                };
                let Some((attachment_id, epoch, _, _)) = session_authorities.get(&session_key)
                else {
                    return Err(WorkerExecutorError::runtime(
                        "local durable topology has no attachment authority",
                    ));
                };
                if attachment.attachment_id != *attachment_id || attachment.epoch != *epoch {
                    continue;
                }
                let Some(attached) = attached_sessions.get(&session_key) else {
                    continue;
                };
                if attached.attempt_id != *prepared_attempt
                    || pending_invocations.get(&attached.pending_invocation_oplog_index)
                        != Some(&session_key.idempotency_key)
                {
                    return Err(WorkerExecutorError::runtime(
                        "local durable topology attachment does not exactly match its session authority",
                    ));
                }
            }
            let control =
                RoutedStreamAttachmentControl::new(self.rpc(), mapping.clone(), auth_ctx.clone());
            let streams = DurableSessionStreams::new(
                producer.clone(),
                self.oplog.clone(),
                session_key,
                std::iter::empty(),
            )
            .with_consumer_invocation(attachment.consumer_invocation.clone())
            .with_rpc(self.rpc())
            .with_consumer_journal(self.durable_stream_consumer_journal())
            .with_auth_ctx(auth_ctx.clone());
            if let Err(error) = streams
                .activate_forwarded_mapping(
                    attachment,
                    mapping,
                    &control,
                    Timestamp::now_utc().to_millis(),
                )
                .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            return Err(WorkerExecutorError::runtime(error));
        }
        Ok(())
    }

    pub(crate) async fn control_durable_stream_attachment(
        &self,
        request: StreamAttachmentControlRequestV1,
    ) -> Result<bool, WorkerExecutorError> {
        if !request.is_well_formed() {
            return Err(WorkerExecutorError::invalid_request(
                "malformed durable stream attachment control request",
            ));
        }
        let key = request.operation.key();
        if let StreamAttachmentControlOperationV1::SourceUnavailable {
            key,
            source_offset,
            consumer_read_ordinal,
        } = &request.operation
        {
            if key.consumer_environment_id != self.owned_agent_id.environment_id
                || key.consumer != self.owned_agent_id.agent_id
                || key.expected_consumer_fingerprint != self.initial_worker_metadata.fingerprint
                || request.mapping.is_some()
            {
                return Err(WorkerExecutorError::invalid_request(
                    "source-unavailable overlay does not match the consumer incarnation",
                ));
            }
            return self
                .durable_stream_producer()
                .await?
                .commit_source_unavailable_overlay(
                    key.clone(),
                    *source_offset,
                    *consumer_read_ordinal,
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()));
        }
        if key.producer_environment_id != self.owned_agent_id.environment_id
            || key.producer != self.owned_agent_id.agent_id
            || key.expected_producer_fingerprint != self.initial_worker_metadata.fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream attachment does not match the producer incarnation",
            ));
        }
        let mapping = request.mapping.as_ref().ok_or_else(|| {
            WorkerExecutorError::invalid_request(
                "routed durable stream attachment control requires the exact session mapping",
            )
        })?;
        if mapping.handle.stream_id != key.stream_id
            || mapping.handle.producer_environment_id != key.producer_environment_id
            || mapping.handle.producer != key.producer
            || mapping.handle.expected_producer_fingerprint != key.expected_producer_fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream attachment mapping does not match the producer key",
            ));
        }
        let producer = self.durable_stream_producer().await?;
        producer
            .validate_handle(&mapping.handle)
            .await
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        let probe =
            DbDirectStreamAttachmentConsumerProbe::new(self.worker_service(), self.oplog_service());
        let consumer_status = probe
            .status_exact(key, Some(mapping))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let authorized = match &request.operation {
            StreamAttachmentControlOperationV1::Prepare { .. } => matches!(
                consumer_status,
                ConsumerAttachmentStatus::Prepared | ConsumerAttachmentStatus::Active
            ),
            StreamAttachmentControlOperationV1::Activate { .. }
            | StreamAttachmentControlOperationV1::Detach { .. }
            | StreamAttachmentControlOperationV1::Renew { .. }
            | StreamAttachmentControlOperationV1::Cancel { .. } => {
                consumer_status == ConsumerAttachmentStatus::Active
            }
            StreamAttachmentControlOperationV1::Finalize { reason, .. } => {
                *reason == StreamAttachmentFinalizationReasonV1::ConsumerDeleted
                    && consumer_status == ConsumerAttachmentStatus::Deleting
            }
            StreamAttachmentControlOperationV1::SourceUnavailable { .. } => {
                unreachable!("source-unavailable controls return before producer authorization")
            }
        };
        if !authorized {
            return Err(WorkerExecutorError::invalid_request(format!(
                "consumer durable topology does not authorize this attachment transition: {consumer_status:?}"
            )));
        }
        let producer_now_millis = Timestamp::now_utc().to_millis();
        let replayed = match request.operation {
            StreamAttachmentControlOperationV1::Prepare { key, .. } => producer
                .prepare_attachment(key, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperationV1::Activate { key, .. } => producer
                .activate_attachment(key, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperationV1::Detach { key } => {
                producer.detach_attachment(&key).await.map(|_| false)
            }
            StreamAttachmentControlOperationV1::Renew { key, .. } => producer
                .renew_attachment(key, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperationV1::Cancel {
                key,
                role,
                reason,
                details,
            } => {
                let role_matches = match mapping.role {
                    SessionStreamRoleV1::Input => matches!(
                        role,
                        golem_common::model::durable_stream::StreamCancelRoleV1::InputProducer
                            | golem_common::model::durable_stream::StreamCancelRoleV1::InputConsumer
                    ),
                    SessionStreamRoleV1::Output => matches!(
                        role,
                        golem_common::model::durable_stream::StreamCancelRoleV1::OutputProducer
                            | golem_common::model::durable_stream::StreamCancelRoleV1::OutputConsumer
                    ),
                };
                if !role_matches {
                    return Err(WorkerExecutorError::invalid_request(
                        "durable stream cancellation role does not match its session mapping",
                    ));
                }
                producer
                    .cancel_open(key.stream_id, role, reason, details)
                    .await
                    .map(|_| false)
            }
            StreamAttachmentControlOperationV1::Finalize { key, reason, .. } => producer
                .finalize_attachment(key, reason, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperationV1::SourceUnavailable { .. } => unreachable!(
                "source-unavailable controls return before producer execution"
            ),
        }
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(replayed)
    }

    pub(crate) async fn read_durable_stream_segment(
        &self,
        request: AttachedStreamSegmentRequestV1,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, WorkerExecutorError> {
        if !request.is_well_formed() {
            return Err(WorkerExecutorError::invalid_request(
                "malformed durable stream segment request",
            ));
        }
        let key = &request.attachment;
        if key.producer_environment_id != self.owned_agent_id.environment_id
            || key.producer != self.owned_agent_id.agent_id
            || key.expected_producer_fingerprint != self.initial_worker_metadata.fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream segment does not match the producer incarnation",
            ));
        }
        let producer = self.durable_stream_producer().await?;
        producer
            .validate_handle(&request.mapping.handle)
            .await
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        let probe =
            DbDirectStreamAttachmentConsumerProbe::new(self.worker_service(), self.oplog_service());
        if probe
            .status_exact(key, Some(&request.mapping))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            != ConsumerAttachmentStatus::Active
        {
            return Err(WorkerExecutorError::invalid_request(
                "consumer durable topology does not authorize this stream read",
            ));
        }
        let events = if request.wait_for_events {
            producer
                .wait_for_attached_segment(
                    key,
                    &request.mapping.handle,
                    Timestamp::now_utc().to_millis(),
                    request.after,
                )
                .await
        } else {
            producer
                .read_attached_segment(
                    key,
                    &request.mapping.handle,
                    Timestamp::now_utc().to_millis(),
                    request.after,
                    request.through,
                )
                .await
        };
        events.map_err(|error| WorkerExecutorError::runtime(error.to_string()))
    }

    pub(crate) fn start_durable_stream_attachment_reconciler(this: &Arc<Self>) {
        let worker = Arc::downgrade(this);
        let interval_duration = this
            .deps
            .config()
            .durable_stream
            .renewal_interval
            .min(this.deps.config().durable_stream.reconciliation_interval);
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            let mut interval = tokio::time::interval(interval_duration);
            loop {
                interval.tick().await;
                let Some(worker) = worker.upgrade() else {
                    break;
                };
                if let Err(error) = worker.recover_durable_stream_topologies().await {
                    warn!(
                        agent_id = %worker.agent_id(),
                        error = %error,
                        "Failed to recover durable stream topology"
                    );
                }
                if let Err(error) = worker.reconcile_durable_stream_attachments().await {
                    warn!(
                        agent_id = %worker.agent_id(),
                        error = %error,
                        "Failed to reconcile durable stream attachments"
                    );
                }
            }
        });
    }

    /// Appends an oplog entry without forcing a durable commit. Callers that
    /// require ordering must await the append before exposing subsequent work.
    pub async fn add_to_oplog(&self, entry: OplogEntry) -> OplogIndex {
        self.oplog.add(entry).await
    }

    pub async fn commit_oplog_and_update_state(&self, commit_level: CommitLevel) -> OplogIndex {
        let (result, changed) = self.state_actor.commit_and_update_state(commit_level).await;
        if changed {
            // The notification goes through the worker-state actor's lifecycle queue so that
            // this method never waits on (or becomes a queued owner of) the instance lock. This
            // method runs inside durable-call host futures polled by wasmtime's store event loop
            // and on store-keeping wasm fibers, neither of which may block on locks shared with
            // the other (see the `state_actor` module docs).
            self.state_actor.notify_status_changed();
        }
        result
    }

    // Should only be called from invocation loop
    pub async fn add_and_commit_oplog(&self, entry: OplogEntry) -> OplogIndex {
        let result = self.add_to_oplog(entry).await;
        self.commit_oplog_and_update_state(CommitLevel::Always)
            .await;
        result
    }

    pub async fn queue_card_revocation(&self, card_id: CardId) -> Option<OplogIndex> {
        self.queue_card_revocations(&[card_id])
            .await
            .into_iter()
            .next()
    }

    pub async fn queue_card_revocations(&self, card_ids: &[CardId]) -> Vec<OplogIndex> {
        let boundary_lock = self.card_event_boundary_lock.clone();
        let _boundary_guard = boundary_lock.lock().await;
        self.queue_card_revocations_locked(card_ids).await
    }

    pub(crate) async fn queue_card_revocations_locked(
        &self,
        card_ids: &[CardId],
    ) -> Vec<OplogIndex> {
        let status = self.get_last_known_status().await;
        let pending_revocations = status
            .pending_card_events
            .iter()
            .filter_map(|pending_event| match &pending_event.event {
                QueuedCardEvent::Revoke(event) => Some(event.card_id),
                QueuedCardEvent::Install(_)
                | QueuedCardEvent::TransferStarted(_)
                | QueuedCardEvent::TransferReceived(_) => None,
            })
            .collect::<HashSet<_>>();
        let mut card_ids = card_ids
            .iter()
            .copied()
            .filter(|card_id| {
                !status.revoked_cards.contains(card_id) && !pending_revocations.contains(card_id)
            })
            .collect::<Vec<_>>();
        card_ids.sort_unstable();
        card_ids.dedup();

        let mut queued_event_indices = Vec::with_capacity(card_ids.len());
        for card_id in card_ids {
            queued_event_indices.push(
                self.add_to_oplog(OplogEntry::card_event_queued(QueuedCardEvent::revoke(
                    card_id,
                )))
                .await,
            );
        }
        if !queued_event_indices.is_empty() {
            self.commit_oplog_and_update_state(CommitLevel::Always)
                .await;
        }

        queued_event_indices
    }

    pub async fn receive_card_transfer(
        self: &Arc<Self>,
        transfer_id: Uuid,
        source_card_id: CardId,
        card: StoredCard,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker_owned().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot deliver a permission card to a deleting worker",
            ));
        }

        let status = self.state_actor.attached_status().await;
        if let Some(received) = status.received_card_transfers.get(&transfer_id) {
            return match received {
                golem_common::model::ReceivedCardTransferState::Received {
                    source_card_id: recorded_source_card_id,
                    card: recorded_card,
                } if recorded_source_card_id.is_none_or(|recorded_source_card_id| {
                    recorded_source_card_id == source_card_id
                }) && recorded_card == &card =>
                {
                    Ok(())
                }
                golem_common::model::ReceivedCardTransferState::Received { .. }
                | golem_common::model::ReceivedCardTransferState::Conflict => Err(
                    WorkerExecutorError::invalid_request(PERMISSION_CARD_TRANSFER_PAYLOAD_CONFLICT),
                ),
            };
        }

        let parsed_agent_id = self.parsed_agent_id.as_ref().ok_or_else(|| {
            WorkerExecutorError::invalid_request("permission cards can only be delivered to agents")
        })?;
        let component = self.current_component.load();
        let target_context =
            agent_monomorphization_context(&component, &self.owned_agent_id, parsed_agent_id);
        if !card_matches_agent_recipient(&card, &target_context) {
            return Err(WorkerExecutorError::invalid_request(
                PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH,
            ));
        }

        let boundary_guard = self.card_event_boundary_lock.clone().lock_owned().await;
        self.state_actor
            .append_and_commit_attached(
                OplogEntry::card_event_queued(QueuedCardEvent::transfer_received(
                    transfer_id,
                    source_card_id,
                    card,
                )),
                self.clone(),
                instance_guard,
                boundary_guard,
            )
            .await;

        Ok(())
    }

    pub(crate) fn card_event_boundary_lock(&self) -> Arc<Mutex<()>> {
        self.card_event_boundary_lock.clone()
    }

    pub(crate) fn published_authority_generation(&self) -> Arc<AtomicU64> {
        self.published_authority_generation.clone()
    }

    async fn add_and_commit_oplog_internal(
        &self,
        instance_guard: &MutexGuard<'_, WorkerInstance>,
        entry: OplogEntry,
        wakeup: Option<WorkerCommand>,
    ) -> OplogIndex {
        let result = self.add_to_oplog(entry).await;
        // The caller already holds the instance lock (and sends the wakeup itself below), so
        // this must not enqueue a `NotifyStatusChanged` lifecycle job: the commit job is safe to
        // await while holding the instance lock precisely because the status task never takes
        // that lock.
        let (_, changed) = self
            .state_actor
            .commit_and_update_state(CommitLevel::Always)
            .await;

        if changed
            && let Some(wakeup) = wakeup
            && let WorkerInstance::Running(running) = &**instance_guard
        {
            running.sender.send(wakeup).unwrap();
        };

        result
    }

    async fn activate_plugin_internal(
        &self,
        plugin_grant_id: EnvironmentPluginGrantId,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot activate plugin on a deleting worker",
            ));
        };

        // Plugin activation does not affect invocation results: do not bump
        // the read-only cache epoch.
        self.add_and_commit_oplog_internal(
            &instance_guard,
            OplogEntry::activate_plugin(plugin_grant_id),
            Some(WorkerCommand::WorkAvailable),
        )
        .await;

        drop(instance_guard);
        Ok(())
    }

    async fn deactivate_plugin_internal(
        &self,
        plugin_grant_id: EnvironmentPluginGrantId,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot deactivate plugin on a deleting worker",
            ));
        };

        // Plugin deactivation does not affect invocation results: do not bump
        // the read-only cache epoch.
        self.add_and_commit_oplog_internal(
            &instance_guard,
            OplogEntry::deactivate_plugin(plugin_grant_id),
            Some(WorkerCommand::WorkAvailable),
        )
        .await;

        drop(instance_guard);
        Ok(())
    }

    /// Reverts the worker to a previous state, selected by either the last oplog index to keep
    /// or the number of invocations to drop.
    ///
    /// The revert operations is implemented by inserting a special oplog entry that
    /// extends the worker's deleted oplog regions, skipping entries from the end of the oplog.
    async fn revert_internal(
        &self,
        target: RevertWorkerTarget,
        resolved_revert: Option<ResolvedRevert>,
    ) -> Result<(), WorkerExecutorError> {
        match target {
            RevertWorkerTarget::RevertToOplogIndex(target) => {
                if resolved_revert.is_some() {
                    return Err(WorkerExecutorError::invalid_request(
                        "Resolved revert must only be supplied for a count-based revert",
                    ));
                }
                self.revert_to_last_oplog_index(target.last_oplog_index, None)
                    .await
            }
            RevertWorkerTarget::RevertLastInvocations(_) => {
                let resolved_revert = resolved_revert.ok_or_else(|| {
                    WorkerExecutorError::invalid_request(
                        "Count-based revert requires a resolved cutoff",
                    )
                })?;
                self.revert_to_last_oplog_index(
                    resolved_revert.last_oplog_index,
                    Some(resolved_revert.observed_oplog_index),
                )
                .await
            }
        }
    }

    pub async fn cancel_invocation(
        &self,
        idempotency_key: IdempotencyKey,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.is_deleting() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot cancel invocation on a deleting worker",
            ));
        };

        self.add_and_commit_oplog_internal(
            &instance_guard,
            OplogEntry::cancel_pending_invocation(idempotency_key),
            Some(WorkerCommand::WorkAvailable),
        )
        .await;

        drop(instance_guard);
        Ok(())
    }

    async fn revert_to_last_oplog_index(
        &self,
        last_oplog_index: OplogIndex,
        expected_oplog_index: Option<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        if last_oplog_index == OplogIndex::NONE {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot revert a worker before the create oplog index".to_string(),
            ));
        }

        let instance_guard = self.lock_stopped_worker().await;
        match &*instance_guard {
            WorkerInstance::Unloaded { .. } => {}
            WorkerInstance::Deleting => {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot revert a deleting worker",
                ));
            }
            _ => panic!("impossible status after lock_stopped_worker"),
        };

        let region_end = self.oplog.current_oplog_index().await;
        if let Some(expected_oplog_index) = expected_oplog_index
            && region_end != expected_oplog_index
        {
            return Err(WorkerExecutorError::invalid_request(format!(
                "Stale count-based revert resolution: expected oplog index {expected_oplog_index}, found {region_end}"
            )));
        }
        let region_start = last_oplog_index.next();
        let last_known_status = self.get_latest_worker_metadata().await.last_known_status;

        if last_known_status
            .skipped_regions
            .is_in_deleted_region(region_start)
        {
            Err(WorkerExecutorError::invalid_request(format!(
                "Attempted to revert to a deleted region in oplog to index {last_oplog_index}"
            )))
        } else if let Some(stream_index) = cut_point::find_stream_history_in_range(
            |idx| self.oplog.read(idx),
            OplogIndex::INITIAL,
            region_end,
        )
        .await
        {
            Err(WorkerExecutorError::invalid_request(format!(
                "Cannot revert worker to oplog index {last_oplog_index}: durable stream history exists at oplog index {stream_index}"
            )))
        } else if let Some(spanning) = cut_point::find_construct_spanning_cut_point(
            |idx| self.oplog.read(idx),
            last_oplog_index,
            region_end,
            &last_known_status.skipped_regions,
        )
        .await
        {
            Err(WorkerExecutorError::invalid_request(format!(
                "Cannot revert worker to oplog index {last_oplog_index}: the cut point is inside {spanning}"
            )))
        } else {
            let region = OplogRegion {
                start: region_start,
                end: region_end,
            };

            // Revert changes observable state, invalidate cached results.
            self.bump_read_only_cache_epoch();

            // this commit will detach the worker status, immediately reattach it so we see the up to date status.
            self.add_and_commit_oplog_internal(&instance_guard, OplogEntry::revert(region), None)
                .await;
            self.reattach_worker_status().await;

            if let WorkerInstance::Running(running) = &*instance_guard {
                running.sender.send(WorkerCommand::WorkAvailable).unwrap();
            };
            drop(instance_guard);
            Ok(())
        }
    }

    async fn wait_for_invocation_result(
        &self,
        key: &IdempotencyKey,
        mut subscription: EventsSubscription,
    ) -> Result<LookupResult, RecvError> {
        loop {
            match self.lookup_invocation_result(key).await {
                LookupResult::Interrupted => break Ok(LookupResult::Interrupted),
                LookupResult::New | LookupResult::Pending => {
                    let wait_result = subscription
                        .wait_for(|event| match event {
                            Event::InvocationCompleted {
                                agent_id,
                                idempotency_key,
                                result,
                            } if *agent_id == self.owned_agent_id.agent_id
                                && idempotency_key == key =>
                            {
                                Some(LookupResult::Complete(result.clone()))
                            }
                            _ => None,
                        })
                        .await;
                    match wait_result {
                        Ok(result) => break Ok(result),
                        Err(RecvError::Lagged(_)) => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                        Err(RecvError::Closed) => break Err(RecvError::Closed),
                    }
                }
                LookupResult::Complete(result) => break Ok(LookupResult::Complete(result)),
            }
        }
    }

    pub async fn lookup_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
        let status = self.last_known_status.load_full().as_ref().clone();
        let maybe_result = self
            .invocation_results
            .read()
            .await
            .get(key)
            .cloned()
            .or_else(|| {
                status
                    .invocation_results
                    .get(key)
                    .map(|oplog_idx| InvocationResult::Lazy {
                        oplog_idx: *oplog_idx,
                    })
            });
        if let Some(mut result) = maybe_result {
            result
                .cache(
                    &self.owned_agent_id,
                    self.agent_mode(),
                    self.initial_worker_metadata.fingerprint,
                    self,
                )
                .await;
            lookup_result_from_cached_result(&status, key, result)
        } else {
            let is_pending = status
                .pending_invocations
                .iter()
                .any(|entry| entry.has_idempotency_key(key));
            let is_current = status.current_idempotency_key.as_ref() == Some(key);
            if is_pending || is_current {
                LookupResult::Pending
            } else {
                LookupResult::New
            }
        }
    }

    async fn stop_internal(
        &self,
        called_from_invocation_loop: bool,
        fail_pending_invocations: Option<WorkerExecutorError>,
        unload_request: UnloadRequest,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        let startup_error = fail_pending_invocations.clone().unwrap_or_else(|| {
            WorkerExecutorError::unknown("Worker stopped before startup completed")
        });
        let mut instance_guard = self.instance.lock().await;
        let startup_attempt = self.startup_attempt.pending();

        let stop_result = self
            .stop_internal_locked(
                &mut instance_guard,
                called_from_invocation_loop,
                fail_pending_invocations,
                unload_request,
                final_state,
                pending_live_invocations,
            )
            .await;

        // IMPORTANT: drop the lock here as the invocation loop might reenter this method after we drop a running worker.
        drop(instance_guard);

        self.handle_stop_result(stop_result).await;
        if !called_from_invocation_loop && let Some(startup_attempt) = startup_attempt {
            self.complete_startup(startup_attempt, Err(startup_error));
        }
    }

    async fn stop_internal_locked(
        &self,
        instance_guard: &mut MutexGuard<'_, WorkerInstance>,
        called_from_invocation_loop: bool,
        // Only respected when this is the call that triggered the stop
        fail_pending_invocations: Option<WorkerExecutorError>,
        unload_request: UnloadRequest,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) -> StopResult {
        // Temporarily set the instance to unloaded so we can work with the old value.
        // This is not visible to anyone as long as we are holding the lock.
        let previous_instance_state = std::mem::replace(
            &mut **instance_guard,
            WorkerInstance::Unloaded {
                startup_failure: None,
            },
        );

        match previous_instance_state {
            WorkerInstance::Unloaded { .. } => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations(error.clone()).await;
                }
                **instance_guard = final_state.into_instance();
                if let WorkerInstance::Unloaded { startup_failure } = &**instance_guard {
                    self.resolve_pending_queue_on_unload(
                        startup_failure.as_ref(),
                        pending_live_invocations,
                    )
                    .await;
                }
                StopResult::Stopped
            }
            WorkerInstance::CleanupFailed(error) => {
                if let Some(ref pending_error) = fail_pending_invocations {
                    self.fail_pending_invocations(pending_error.clone()).await;
                }
                **instance_guard = WorkerInstance::CleanupFailed(error);
                StopResult::Stopped
            }
            WorkerInstance::WaitingForPermit(_) => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations(error.clone()).await;
                }
                crate::metrics::workers::dec_worker_waiting_for_memory();
                **instance_guard = final_state.into_instance();
                match &**instance_guard {
                    WorkerInstance::Unloaded { startup_failure } => {
                        self.resolve_pending_queue_on_unload(
                            startup_failure.as_ref(),
                            pending_live_invocations,
                        )
                        .await;
                    }
                    WorkerInstance::CleanupFailed(error) => {
                        self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                            .await;
                    }
                    _ => {}
                }
                StopResult::Stopped
            }
            WorkerInstance::Deleting => {
                **instance_guard = previous_instance_state;
                // Should we return an error here?
                StopResult::Stopped
            }
            WorkerInstance::Stopping(stopping) if called_from_invocation_loop => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations(error.clone()).await;
                }
                let pending_live_invocations = stopping.pending_live_invocations;
                let (instance, notify) = complete_stopping_worker(stopping, final_state);
                **instance_guard = instance;
                match &**instance_guard {
                    WorkerInstance::Unloaded { startup_failure } => {
                        self.resolve_pending_queue_on_unload(
                            startup_failure.as_ref(),
                            pending_live_invocations,
                        )
                        .await;
                    }
                    WorkerInstance::CleanupFailed(error) => {
                        self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                            .await;
                    }
                    _ => {}
                }
                notify.set();
                StopResult::Stopped
            }
            WorkerInstance::Stopping(mut stopping) => {
                let deleting = matches!(&final_state, FinalWorkerState::Deleting);
                stopping.final_state = merge_final_worker_state(stopping.final_state, final_state);
                if pending_live_invocations == PendingLiveInvocationDisposition::Fail {
                    stopping.pending_live_invocations = PendingLiveInvocationDisposition::Fail;
                }
                if deleting && let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations(error.clone()).await;
                }
                let notify = stopping.notify.clone();
                **instance_guard = WorkerInstance::Stopping(stopping);
                StopResult::AlreadyStopping { notify }
            }
            WorkerInstance::Running(running) => {
                self.owner_runtime_resources.fence_filesystem_generation();
                debug!(
                    "Stopping running worker ({called_from_invocation_loop}) ({})",
                    fail_pending_invocations.is_some()
                );

                // TODO: fail pending invocations should be factored out of here and be guaranteed to run
                // even if there are multiple concurrent stop attempts.
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations(error.clone()).await;
                };

                // Make sure the oplog is committed
                self.oplog.commit(CommitLevel::Always).await;

                // Persist any pending cached-status changes synchronously before the worker leaves
                // memory, so a subsequent cold load does not have to re-fold oplog entries that were
                // only reflected in the (deferred) in-memory status. Best-effort: a failure is
                // logged/metered inside `flush` and re-queued; the blob is reconstructable from the
                // oplog, so it must not block the stop.
                if let Err(err) = self
                    .status_flusher
                    .flush(status_flusher::FlushReason::Forced)
                    .await
                {
                    debug!("Forced status flush on stop failed (will retry in background): {err}");
                }

                // when stopping via the invocation loop we can stop immediately, no need to go via the stopping status
                if called_from_invocation_loop {
                    crate::metrics::workers::dec_worker_memory_resident();
                    // The invocation-loop task retains the shared grant cell until it
                    // exits. Release its reservation now so permit reacquisition can
                    // register and admit the replacement generation without overlapping
                    // the old generation's grant.
                    self.release_linear_memory_grant();
                    **instance_guard = final_state.into_instance();
                    match &**instance_guard {
                        WorkerInstance::Unloaded { startup_failure } => {
                            self.resolve_pending_queue_on_unload(
                                startup_failure.as_ref(),
                                pending_live_invocations,
                            )
                            .await;
                        }
                        WorkerInstance::CleanupFailed(error) => {
                            self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                                .await;
                        }
                        _ => {}
                    }
                    StopResult::Stopped
                } else {
                    // drop the running worker, this signals to the invocation loop to start exiting.
                    // `stop()` consumes the RunningWorker and drops everything but
                    // its join handle, releasing its memory grant back to the gate.
                    let run_loop_handle = running.stop(unload_request);
                    let notify = OneShotEvent::new();
                    crate::metrics::workers::dec_worker_memory_resident();
                    **instance_guard = WorkerInstance::Stopping(StoppingWorker {
                        notify: notify.clone(),
                        final_state,
                        pending_live_invocations,
                    });
                    StopResult::NeedsWaitForLoopExit {
                        run_loop_handle,
                        notify,
                    }
                }
            }
        }
    }

    // IMPORTANT: must not be called within a held instance lock
    async fn handle_stop_result(&self, stop_result: StopResult) {
        match stop_result {
            StopResult::Stopped => {}
            StopResult::AlreadyStopping { notify } => notify.wait().await,
            StopResult::NeedsWaitForLoopExit {
                run_loop_handle,
                notify,
            } => {
                let run_loop_failure = run_loop_handle.await.err().map(|error| {
                    WorkerExecutorError::runtime(format!(
                        "invocation loop task stopped unexpectedly: {error}"
                    ))
                });

                let mut instance_guard = self.instance.lock().await;
                if let Some(error) = run_loop_failure.as_ref() {
                    merge_run_loop_failure(&mut instance_guard, error.clone());
                }
                let is_deleting = match &*instance_guard {
                    WorkerInstance::Stopping(stopping) => {
                        matches!(stopping.final_state, FinalWorkerState::Deleting)
                    }
                    WorkerInstance::Deleting => true,
                    _ => false,
                };

                // After the invocation loop has fully exited, fail any remaining
                // unresolved invocations (e.g. the currently running one that was
                // in progress when deletion was requested).
                if is_deleting {
                    drop(instance_guard);
                    self.fail_pending_invocations(WorkerExecutorError::invalid_request(
                        "Worker is being deleted",
                    ))
                    .await;
                    instance_guard = self.instance.lock().await;
                }

                if let Some(error) = run_loop_failure {
                    drop(instance_guard);
                    self.fail_pending_invocations(error).await;
                    instance_guard = self.instance.lock().await;
                }

                let pending_live_invocations =
                    if matches!(&*instance_guard, WorkerInstance::Stopping(_)) {
                        match std::mem::replace(
                            &mut *instance_guard,
                            WorkerInstance::Unloaded {
                                startup_failure: None,
                            },
                        ) {
                            WorkerInstance::Stopping(stopping) => {
                                let pending_live_invocations = stopping.pending_live_invocations;
                                *instance_guard = stopping.final_state.into_instance();
                                Some(pending_live_invocations)
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        None
                    };
                match &*instance_guard {
                    WorkerInstance::Unloaded { startup_failure } => {
                        self.resolve_pending_queue_on_unload(
                            startup_failure.as_ref(),
                            pending_live_invocations
                                .unwrap_or(PendingLiveInvocationDisposition::Fail),
                        )
                        .await;
                    }
                    WorkerInstance::CleanupFailed(error) => {
                        self.resolve_pending_readiness_awaiters_on_stop(Some(error))
                            .await;
                    }
                    _ => {}
                }
                drop(instance_guard);

                notify.set();
            }
        }
    }

    /// Resolves all queued `AwaitReadyToProcessCommands` markers when the worker reaches the
    /// `Unloaded` state without the invocation loop having drained them — for example when the
    /// worker suspends itself mid-invocation, as debugging workers do as soon as their replay
    /// goes live. Waiters observe the startup failure if there is one, otherwise a successful
    /// stop. All other queued items are kept for the next start.
    async fn resolve_pending_queue_on_unload(
        &self,
        startup_failure: Option<&WorkerExecutorError>,
        _pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        self.resolve_pending_readiness_awaiters_on_stop(startup_failure)
            .await;
    }

    async fn resolve_pending_readiness_awaiters_on_stop(
        &self,
        startup_failure: Option<&WorkerExecutorError>,
    ) {
        let mut queue = self.queue.write().await;
        let items = queue.drain(..).collect::<Vec<_>>();
        for item in items {
            match item {
                QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                    let _ = sender.send(match startup_failure {
                        Some(err) => Err(err.clone()),
                        None => Ok(()),
                    });
                }
                other => queue.push_back(other),
            }
        }
    }

    async fn fail_pending_invocations(&self, error: WorkerExecutorError) {
        let queued_items = self.queue.write().await.drain(..).collect::<VecDeque<_>>();
        let mut origins = self.external_invocation_origins.write().await;

        // Publishing the provided initialization error to all queued internal operations
        for item in queued_items {
            match item {
                QueuedWorkerInvocation::GetFileSystemNode { sender, .. } => {
                    let _ = sender.send(Err(error.clone()));
                }
                QueuedWorkerInvocation::GetWalletCards { sender } => {
                    let _ = sender.send(Err(error.clone()));
                }
                QueuedWorkerInvocation::ReadFile { sender, .. } => {
                    let _ = sender.send(Err(error.clone()));
                }
                QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                    let _ = sender.send(Err(error.clone()));
                }
                QueuedWorkerInvocation::SaveSnapshot => {}
            }
        }

        let status = self.last_known_status.load_full().as_ref().clone();
        let keys_to_fail = invocation_keys_to_fail(&status, None, true);

        let mut invocation_results = self.invocation_results.write().await;
        for idempotency_key in &keys_to_fail {
            if invocation_results.contains_key(idempotency_key) {
                continue;
            }
            invocation_results.insert(
                idempotency_key.clone(),
                InvocationResult::Cached {
                    result: Err(FailedInvocationResult {
                        trap_type: TrapType::Error {
                            error: golem_common::model::oplog::AgentError::Unknown(
                                error.to_string(),
                            ),
                            retry_from: OplogIndex::INITIAL,
                            in_atomic_region: false,
                            atomic_region_had_side_effects: false,
                            semantic_trap_retry_override: None,
                        },
                        stderr: String::new(),
                    }),
                },
            );
            self.publish_completion(idempotency_key, Err(error.clone()));
            origins.remove(idempotency_key);
        }
    }

    // Lock a worker not in stopping state.
    async fn lock_non_stopping_worker(&self) -> MutexGuard<'_, WorkerInstance> {
        loop {
            let instance_guard = self.instance.lock().await;

            match &*instance_guard {
                WorkerInstance::Stopping(stopping) => {
                    let notify = stopping.notify.clone();
                    drop(instance_guard);
                    notify.wait().await;
                }
                _ => return instance_guard,
            }
        }
    }

    async fn lock_non_stopping_worker_owned(&self) -> OwnedMutexGuard<WorkerInstance> {
        loop {
            let instance_guard = self.instance.clone().lock_owned().await;

            match &*instance_guard {
                WorkerInstance::Stopping(stopping) => {
                    let notify = stopping.notify.clone();
                    drop(instance_guard);
                    notify.wait().await;
                }
                _ => return instance_guard,
            }
        }
    }

    // Lock a worker in either Unloaded or Deleting state.
    async fn lock_stopped_worker(&self) -> MutexGuard<'_, WorkerInstance> {
        loop {
            self.stop_internal(
                false,
                None,
                UnloadRequest::ordinary(UnloadReason::ExplicitStop),
                FinalWorkerState::Unloaded {
                    startup_failure: None,
                },
                PendingLiveInvocationDisposition::Fail,
            )
            .await;
            let instance_guard = self.instance.lock().await;

            if let WorkerInstance::Deleting | WorkerInstance::Unloaded { .. } = &*instance_guard {
                return instance_guard;
            }
        }
    }

    async fn restart_on_oom(
        this: Arc<Worker<Ctx>>,
        called_from_invocation_loop: bool,
        delay: Option<Duration>,
        oom_retry_count: u32,
        start_attempt: Option<Uuid>,
    ) -> Result<Option<Uuid>, WorkerExecutorError> {
        this.stop_internal(
            called_from_invocation_loop,
            None,
            UnloadRequest::ordinary(UnloadReason::OutOfMemory),
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            PendingLiveInvocationDisposition::Preserve,
        )
        .await;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Self::start_if_needed_internal(this, oom_retry_count, start_attempt).await
    }

    async fn get_or_create_worker_metadata<
        T: HasWorkerService
            + HasComponentService
            + HasConfig
            + HasOplogService
            + HasEnvironmentStateService
            + Sync,
    >(
        this: &T,
        owned_agent_id: &OwnedAgentId,
        component_revision: Option<ComponentRevision>,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        parent: Option<AgentId>,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<GetOrCreateWorkerResult, WorkerExecutorError> {
        let component_id = owned_agent_id.component_id();

        // KnownFresh has already been validated against the ephemeral agent type, phantom ID, and
        // idempotency key at invocation ingress. All other paths retain the checked lookup.
        let existing_worker_metadata =
            if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                None
            } else {
                // Note: this also checks the oplog for the existence of the create entry.
                this.worker_service().get(owned_agent_id).await
            };

        match existing_worker_metadata {
            Some(GetWorkerMetadataResult {
                initial_worker_metadata,
                last_known_status,
            }) => {
                let persisted_status = last_known_status.clone();
                // make sure we are fully up to date on the oplog
                let agent_mode = initial_worker_metadata.agent_mode;
                let current_status = calculate_last_known_status_with_checkpoint(
                    this,
                    owned_agent_id,
                    agent_mode,
                    last_known_status,
                )
                .await
                .expect("Failed to calculate worker status for existing worker");

                // Use the CREATE-time revision: `agent_id` parsing and
                // `resolve_agent_properties` must stay tied to the metadata
                // the oplog was committed against. `current_component` is
                // refreshed to the live revision by `create_instance`.
                let initial_component = this
                    .component_service()
                    .get_metadata(
                        component_id,
                        Some(initial_worker_metadata.last_known_status.component_revision),
                    )
                    .await?;

                let current_status = Arc::new(arc_swap::ArcSwap::from_pointee(current_status));

                let agent_id = if initial_component.metadata.is_agent() {
                    let agent_id = ParsedAgentId::parse(
                        &owned_agent_id.agent_id.agent_id,
                        &initial_component.metadata,
                    )
                    .map_err(|err| {
                        WorkerExecutorError::invalid_request(format!("Invalid agent id: {}", err))
                    })?;
                    Some(agent_id)
                } else {
                    None
                };

                // For an existing worker, the authoritative `agent_mode` was decided at create
                // time and is persisted in the `Create` oplog entry; we do not re-resolve it
                // from the (possibly newer) component metadata to avoid silently routing the
                // worker to a different oplog namespace if the agent type's mode was changed
                // in a later component revision.
                let agent_mode = initial_worker_metadata.agent_mode;
                let ResolvedAgentProperties {
                    snapshot_policy, ..
                } = resolve_agent_properties(this, agent_id.as_ref(), &initial_component.metadata);

                let execution_status =
                    Arc::new(std::sync::RwLock::new(ExecutionStatus::Suspended {
                        agent_mode,
                        timestamp: Timestamp::now_utc(),
                    }));

                let oplog = this
                    .oplog_service()
                    .open(
                        owned_agent_id,
                        agent_mode,
                        None,
                        initial_worker_metadata.clone(),
                        read_only_lock::arc_swap::ReadOnlyView::new(current_status.clone()),
                        read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                    )
                    .await;

                Ok(GetOrCreateWorkerResult {
                    initial_worker_metadata,
                    current_status,
                    persisted_status,
                    execution_status,
                    agent_id,
                    snapshot_policy,
                    oplog,
                    initial_component: Arc::new(initial_component),
                    reconstructed_ephemeral: agent_mode == AgentMode::Ephemeral,
                })
            }
            None => {
                // Create and initialize a new worker.
                let component = this
                    .component_service()
                    .get_metadata(component_id, component_revision)
                    .await?;

                let agent_id = if component.metadata.is_agent() {
                    let agent_id = ParsedAgentId::parse(
                        &owned_agent_id.agent_id.agent_id,
                        &component.metadata,
                    )
                    .map_err(|err| {
                        WorkerExecutorError::invalid_request(format!("Invalid agent id: {}", err))
                    })?;
                    Some(agent_id)
                } else {
                    None
                };

                let ResolvedAgentProperties {
                    agent_mode,
                    snapshot_policy,
                } = resolve_agent_properties(this, agent_id.as_ref(), &component.metadata);

                let execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };

                {
                    // The actual checks are performed in the DurableWorkerCtx on secret access.
                    // This is just to fail early with a nicer error.
                    let agent_secrets = this
                        .environment_state_service()
                        .get_agent_secrets(component.environment_id)
                        .await?;
                    ensure_required_agent_secrets_are_configured(
                        &agent_secrets,
                        agent_id.as_ref(),
                        &component,
                    )?
                };

                let initial_agent_config = parse_worker_creation_agent_config(
                    worker_agent_config,
                    agent_id.as_ref(),
                    &component,
                )?;
                // Store only the per-worker env overrides. Agent-type defaults are applied
                // at runtime in get_environment
                let worker_env: Vec<(String, String)> = worker_env.unwrap_or_default();
                let created_at = Timestamp::now_utc();

                // Note: Keep this in sync with the logic in crate::services::worker::WorkerService::get
                let initial_status = AgentStatusRecord {
                    component_revision: component.revision,
                    component_revision_for_replay: component.revision,
                    component_size: component.component_size,
                    total_linear_memory_size: component.metadata.initial_linear_memory_bytes(),
                    active_plugins: agent_id
                        .as_ref()
                        .and_then(|agent_id| {
                            component.metadata.agent_type_plugins(&agent_id.agent_type)
                        })
                        .unwrap_or_default()
                        .iter()
                        .map(|i| i.environment_plugin_grant_id)
                        .collect(),
                    agent_mode,
                    ..Default::default()
                };

                // Use the component's authoritative account_id and environment_id
                // rather than the caller-provided values. During cross-account or
                // cross-environment RPC the caller may pass its own account/environment,
                // but the worker must belong to the component's owning account and
                // environment for correct metric attribution and quota enforcement.

                let instance_id = Uuid::now_v7();

                let initial_worker_metadata = AgentMetadata {
                    agent_id: owned_agent_id.agent_id(),
                    env: worker_env,
                    config: initial_agent_config,
                    environment_id: component.environment_id,
                    created_by: component.account_id,
                    created_by_email: component.account_email.clone(),
                    created_at,
                    parent,
                    last_known_status: initial_status.clone(),
                    original_phantom_id: agent_id.as_ref().and_then(|id| id.phantom_id),
                    fingerprint: AgentFingerprint(instance_id),
                    agent_mode,
                };

                // Alternatively, we could just write the oplog entry and recompute the initial_worker_metadata from it.
                // both options are equivalent here, this is just cheaper.

                // Strip the schema graph from the typed config entries to get
                // the raw (untyped) form persisted in the Create oplog entry.
                let local_agent_config: Vec<golem_common::model::worker::UntypedAgentConfigEntry> =
                    initial_worker_metadata
                        .config
                        .iter()
                        .cloned()
                        .map(golem_common::model::worker::UntypedAgentConfigEntry::try_from)
                        .collect::<Result<_, _>>()
                        .map_err(|err: String| WorkerExecutorError::runtime(err))?;

                let initial_oplog_entry = OplogEntry::create(
                    initial_worker_metadata.agent_id.clone(),
                    initial_worker_metadata.agent_mode,
                    initial_worker_metadata.last_known_status.component_revision,
                    initial_worker_metadata.env.clone(),
                    initial_worker_metadata.environment_id,
                    initial_worker_metadata.created_by,
                    initial_worker_metadata.parent.clone(),
                    initial_worker_metadata.last_known_status.component_size,
                    initial_worker_metadata
                        .last_known_status
                        .total_linear_memory_size,
                    initial_worker_metadata
                        .last_known_status
                        .active_plugins
                        .clone(),
                    local_agent_config,
                    initial_worker_metadata.original_phantom_id,
                    instance_id,
                );

                let initial_status = Arc::new(arc_swap::ArcSwap::from_pointee(initial_status));
                let execution_status = Arc::new(std::sync::RwLock::new(execution_status));

                let oplog_service = this.oplog_service();
                let oplog = if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                    oplog_service
                        .create_fresh(
                            owned_agent_id,
                            agent_mode,
                            initial_oplog_entry,
                            initial_worker_metadata.clone(),
                            read_only_lock::arc_swap::ReadOnlyView::new(initial_status.clone()),
                            read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                        )
                        .await
                } else {
                    oplog_service
                        .create(
                            owned_agent_id,
                            agent_mode,
                            initial_oplog_entry,
                            initial_worker_metadata.clone(),
                            read_only_lock::arc_swap::ReadOnlyView::new(initial_status.clone()),
                            read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                        )
                        .await
                };

                {
                    let mut status = initial_status.load_full().as_ref().clone();
                    status.oplog_idx = oplog.current_oplog_index().await;
                    initial_status.store(Arc::new(status));
                }

                // Cold path (worker creation): no previously cached status to diff against.
                let initial_status_value = initial_status.load_full().as_ref().clone();
                this.worker_service()
                    .update_cached_status(owned_agent_id, None, initial_status_value.clone())
                    .await;

                Ok(GetOrCreateWorkerResult {
                    initial_worker_metadata,
                    current_status: initial_status,
                    persisted_status: Some(initial_status_value),
                    execution_status,
                    agent_id,
                    snapshot_policy,
                    oplog,
                    initial_component: Arc::new(component),
                    reconstructed_ephemeral: false,
                })
            }
        }
    }

    // TODO: should be private, exposed for the invocation loop for now.
    pub async fn reattach_worker_status(&self) {
        self.state_actor.reattach_worker_status().await;
    }

    async fn start_waiting_worker(
        this: Arc<Worker<Ctx>>,
        memory_grant: MemoryGrant,
        component_charge: WorkerComponentCharge,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        oom_retry_count: u32,
        start_attempt: Uuid,
        worker_trace: WorkerTrace,
    ) {
        let mut instance_guard = this.instance.lock().await;
        match &*instance_guard {
            WorkerInstance::WaitingForPermit(waiting_worker)
                if waiting_worker.start_attempt == start_attempt =>
            {
                this.interrupt_signal
                    .lock()
                    .await
                    .reset_terminal_for_new_generation();
                let running = RunningWorker::new(
                    this.owned_agent_id.clone(),
                    this.queue.clone(),
                    this.clone(),
                    memory_grant,
                    component_charge,
                    concurrent_agent_permit,
                    oom_retry_count,
                    start_attempt,
                    worker_trace,
                )
                .await;
                crate::metrics::workers::dec_worker_waiting_for_memory();
                crate::metrics::workers::inc_worker_memory_resident();
                *instance_guard = WorkerInstance::Running(running);
            }
            _ => {
                debug!("worker was not waiting for permit anymore, not starting");
                // The worker is not becoming resident: dropping `memory_grant`
                // here returns its reservation to the gate.
            }
        }
    }

    /// Writes a *clean* status checkpoint from the current in-memory status if eligible (see
    /// [`status_checkpointer::StatusCheckpointer::maybe_checkpoint`]).
    ///
    /// Must only be called at structurally clean boundaries where no jumpable oplog region is open
    /// (snapshot save, idle suspend). Skipped while the status is detached, because then the
    /// in-memory status is not authoritative — checkpointing it could persist a baseline inside a
    /// region. Best-effort and bounded by the throttle; never blocks meaningfully.
    pub(crate) async fn checkpoint_status(&self, reason: status_checkpointer::CheckpointReason) {
        if self.last_known_status_detached.load(Ordering::Acquire) {
            return;
        }
        let status = self.last_known_status.load_full().as_ref().clone();
        self.status_checkpointer
            .maybe_checkpoint(&status, reason)
            .await;
    }

    /// Writes a *clean* status checkpoint *during* a long-running invocation, taken from the current
    /// committed in-memory status (the caller must only invoke this right after a durable commit, so
    /// `last_known_status` reflects the committed oplog tip).
    ///
    /// In addition to the [`Self::checkpoint_status`] guards, this respects the per-invocation
    /// `get_oplog_index` marker watermark: if the guest captured an oplog index `M` via
    /// `get_oplog_index`, a later `set_oplog_index(M)` deletes `(M.next()..tip]` but preserves `M`,
    /// so a checkpoint must not advance past `M` or it would be discarded after such a jump. When a
    /// marker is present and the committed tip is already beyond it, we skip the checkpoint (a cheap
    /// no-op) rather than write one that a later jump would invalidate.
    pub(crate) async fn checkpoint_status_mid_invocation(
        &self,
        min_exposed_marker: Option<OplogIndex>,
    ) {
        if self.last_known_status_detached.load(Ordering::Acquire) {
            return;
        }
        let status = self.last_known_status.load_full().as_ref().clone();
        if let Some(marker) = min_exposed_marker
            && status.oplog_idx > marker
        {
            return;
        }
        self.status_checkpointer
            .maybe_checkpoint(
                &status,
                status_checkpointer::CheckpointReason::MidInvocation,
            )
            .await;
    }

    /// Synchronously persists any pending cached-status changes for this worker. Used at lifecycle
    /// boundaries (e.g. suspend) so the cached blob is up to date when the worker goes idle, rather
    /// than waiting for the next background sweep.
    pub(crate) async fn force_flush_status(&self) {
        // Best-effort: a failure is logged/metered and re-queued inside `flush`; the blob is
        // reconstructable from the oplog so it must not block the caller (e.g. suspend).
        if let Err(err) = self
            .status_flusher
            .flush(status_flusher::FlushReason::Forced)
            .await
        {
            debug!("Forced status flush failed (will retry in background): {err}");
        }
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

pub fn merge_agent_env_with_default_env(
    agent_env: Option<Vec<(String, String)>>,
    default_agent_env: BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut seen_keys = HashSet::new();
    let mut result = Vec::new();

    if let Some(worker_env) = agent_env {
        for (key, value) in worker_env {
            seen_keys.insert(key.clone());
            result.push((key, value));
        }
    }

    for (key, value) in default_agent_env {
        // Prioritise per worker environment variables all the time
        if !seen_keys.contains(&key) {
            result.push((key, value));
        }
    }

    result
}

#[derive(Debug)]
enum WorkerInstance {
    Unloaded {
        startup_failure: Option<WorkerExecutorError>,
    },
    CleanupFailed(WorkerExecutorError),
    WaitingForPermit(WaitingWorker),
    Running(RunningWorker),
    Stopping(StoppingWorker),
    Deleting,
}

impl WorkerInstance {
    fn is_deleting(&self) -> bool {
        matches!(
            self,
            Self::Deleting
                | Self::Stopping(StoppingWorker {
                    final_state: FinalWorkerState::Deleting,
                    ..
                })
        )
    }

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

impl WaitingWorker {
    pub fn new<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
        memory_requirement: u64,
        oom_retry_count: u32,
        start_attempt: Uuid,
    ) -> Self {
        let worker_trace = parent.trace(TraceOrigin::capture_current());

        let handle = tokio::task::spawn(async move {
            let agent_id = parent.owned_agent_id.agent_id();
            let registered_concurrent_account = parent.registered_concurrent_account.clone();

            // Determine the component's compiled-module size before acquiring
            // the per-account concurrency slot (and before reserving memory),
            // so the worker's memory and its module are admitted together (the
            // module is reserved first, then the memory admission accounts for
            // it). The module is charged once per resident component and shared
            // by all its workers.
            //
            // Charges the pending-update target revision when one is queued
            // (matching what create_instance loads); only a non-existent
            // target falls back to the current revision, and transient
            // resolution failures are retried rather than wedging the worker
            // in WaitingForPermit or under-reserving against the old revision.
            //
            // This resolution is read-only and holds no permits, so it is done
            // before acquiring the concurrent-agent permit: its retry loop must
            // not hold one of the account's active-agent slots while the worker
            // is not yet running, otherwise a single worker whose target
            // metadata is transiently unavailable could block unrelated workers
            // of the same account from starting.
            // Not spanned: this retries on a 500ms delay and logs once per attempt,
            // so a span covering the whole call would accumulate two events a second
            // for as long as resolution keeps failing, and never close to export
            // them. The retry events stay in the logs, and how long the wait took
            // is recorded as a metric rather than a span.
            let phase_start = std::time::Instant::now();
            let requirement = parent.startup_component_charge_requirement().await;
            parent
                .startup_linear_memory_bytes
                .store(requirement.startup_linear_memory_bytes, Ordering::Release);
            let memory_requirement =
                memory_requirement.max(requirement.reserved_linear_memory_bytes);
            crate::metrics::workers::record_worker_admission_wait(
                AdmissionPhase::ResolveComponentCharge,
                phase_start.elapsed(),
            );

            let phase_start = std::time::Instant::now();
            let concurrent_agent_permit = registered_concurrent_account
                .acquire(agent_id.clone())
                .instrument(related_span!(
                    worker_trace.startup_origin,
                    Level::INFO,
                    "acquire_concurrent_agent_slot",
                    %agent_id,
                    agent_type = %worker_trace.agent_type
                ))
                .await;
            crate::metrics::workers::record_worker_admission_wait(
                AdmissionPhase::ConcurrencySlot,
                phase_start.elapsed(),
            );

            // `memory_grant` and `component_charge` own their reservations
            // from here on: held as locals until the worker becomes resident
            // (when they move into the RunningWorker) or this task ends/aborts
            // (when dropping them returns the reservations to the gate). This
            // is what makes a start cancelled mid-flight — e.g. the worker
            // being deleted while still waiting for its remaining permits —
            // release rather than leak its grant and module charge.
            //
            // Admission is not gated while waiting for a per-account
            // concurrency slot above; otherwise one account could exhaust the
            // memory headroom with workers that are not allowed to run yet.
            let phase_start = std::time::Instant::now();
            let (memory_grant, component_charge) = parent
                .active_agents()
                .acquire_with_component_charge(
                    memory_requirement,
                    requirement.component_id,
                    requirement.component_revision,
                    requirement.module_bytes,
                )
                // Not spanned, for the same reason as the charge resolution above:
                // `acquire_memory` retries on the same 500ms delay and logs once per
                // attempt. Its duration is recorded as a metric instead.
                .await;
            crate::metrics::workers::record_worker_admission_wait(
                AdmissionPhase::Memory,
                phase_start.elapsed(),
            );
            debug!("Attempting to start worker after acquiring enough permits");
            Worker::start_waiting_worker(
                parent,
                memory_grant,
                component_charge,
                concurrent_agent_permit,
                oom_retry_count,
                start_attempt,
                worker_trace,
            )
            .await;
            // If we do not start the worker here we will drop the permits here, which will release them to the host.
        });

        WaitingWorker {
            handle: Some(handle),
            start_attempt,
        }
    }
}

impl Drop for WaitingWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingWorkerInterrupt {
    kind: InterruptKind,
    reacquire_permits: bool,
    unload_request: UnloadRequest,
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
    Suspend,
}

impl UnloadReason {
    fn from_interrupt(kind: InterruptKind) -> Self {
        match kind {
            InterruptKind::Restart | InterruptKind::Jump => Self::Restart,
            InterruptKind::Suspend(_) => Self::Suspend,
            InterruptKind::Interrupt(_) => Self::Interrupt,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnloadRequest {
    pub(crate) reason: UnloadReason,
    pub(crate) deadline: Instant,
}

impl UnloadRequest {
    pub(crate) fn new(reason: UnloadReason, deadline: Instant) -> Self {
        Self { reason, deadline }
    }

    fn ordinary(reason: UnloadReason) -> Self {
        Self::new(reason, Instant::now() + Duration::from_secs(30))
    }
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

impl PendingWorkerInterrupt {
    fn is_terminal(&self) -> bool {
        !matches!(self.kind, InterruptKind::Restart | InterruptKind::Jump)
    }

    /// How the invocation loop should proceed after honoring this interrupt. A suspension retains
    /// its timestamp so a newer wakeup can supersede it, while an explicit interrupt remains
    /// terminal.
    fn retry_decision(&self) -> RetryDecision {
        if self.reacquire_permits {
            RetryDecision::ReacquirePermits
        } else {
            match self.kind {
                InterruptKind::Restart | InterruptKind::Jump => RetryDecision::Immediate,
                InterruptKind::Interrupt(_) => RetryDecision::None,
                InterruptKind::Suspend(timestamp) => RetryDecision::TryStop(timestamp),
            }
        }
    }
}

impl WorkerInterruptState {
    fn has_interrupt(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    fn queue(&mut self, mut interrupt: PendingWorkerInterrupt) -> bool {
        match self {
            Self::TerminalClaimed if interrupt.is_terminal() => {
                *self = Self::Pending(interrupt);
                true
            }
            Self::TerminalClaimed => false,
            Self::Pending(current) if current.is_terminal() => false,
            Self::Pending(current) => {
                if matches!(interrupt.kind, InterruptKind::Restart) {
                    interrupt.reacquire_permits |= current.reacquire_permits;
                }
                *self = Self::Pending(interrupt);
                true
            }
            Self::Idle => {
                *self = Self::Pending(interrupt);
                true
            }
        }
    }

    fn take(&mut self) -> Option<PendingWorkerInterrupt> {
        match std::mem::take(self) {
            Self::Pending(interrupt) if interrupt.is_terminal() => {
                *self = Self::TerminalClaimed;
                Some(interrupt)
            }
            Self::Pending(interrupt) => Some(interrupt),
            state => {
                *self = state;
                None
            }
        }
    }

    fn claim_pending_terminal(&mut self) -> Option<PendingWorkerInterrupt> {
        match self {
            Self::Pending(interrupt) if interrupt.is_terminal() => self.take(),
            _ => None,
        }
    }

    fn reset_terminal_for_new_generation(&mut self) {
        if matches!(self, Self::TerminalClaimed) {
            *self = Self::Idle;
        }
    }
}

#[derive(Debug)]
struct RunningWorker {
    handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<WorkerCommand>,
    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    waiting_for_command: Arc<AtomicBool>,
    concurrent_agent_permit_held: Arc<AtomicBool>,
    filesystem_activity: Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    unload_request: Arc<StdMutex<Option<UnloadRequest>>>,
    idle_since_millis: Arc<AtomicU64>,
    interrupt_signal: Arc<async_lock::Mutex<WorkerInterruptState>>,
    /// `ResumeReplay` is signalled directly through the command channel rather
    /// than the internal queue, so eviction must treat it as pending work.
    resume_replay_pending: Arc<AtomicBool>,
    start_attempt: Uuid,
}

struct RunningAgent<Runtime, Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    runtime: Runtime,
    filesystem: ResidentFilesystem<Adapter>,
}

struct RunningAgentRuntime<Ctx: WorkerCtx> {
    instance: Instance,
    store: async_lock::Mutex<Store<Ctx>>,
}

type WorkerRunningAgent<Ctx> = RunningAgent<RunningAgentRuntime<Ctx>>;

pub(crate) struct CreateWorkerInstanceError {
    pub(crate) error: WorkerExecutorError,
    pub(crate) filesystem_cleanup_failed: bool,
}

impl From<WorkerExecutorError> for CreateWorkerInstanceError {
    fn from(error: WorkerExecutorError) -> Self {
        Self {
            error,
            filesystem_cleanup_failed: false,
        }
    }
}

struct LinearMemoryGrantRegistration<Ctx: WorkerCtx> {
    worker: Arc<Worker<Ctx>>,
    grant: Arc<StdMutex<MemoryGrant>>,
}

impl<Ctx: WorkerCtx> LinearMemoryGrantRegistration<Ctx> {
    fn new(worker: Arc<Worker<Ctx>>, grant: Arc<StdMutex<MemoryGrant>>) -> Self {
        let previous = worker
            .linear_memory_grant
            .lock()
            .unwrap()
            .replace(grant.clone());
        assert!(
            previous.is_none(),
            "worker already has a linear memory grant"
        );
        Self { worker, grant }
    }
}

impl<Ctx: WorkerCtx> Drop for LinearMemoryGrantRegistration<Ctx> {
    fn drop(&mut self) {
        let mut registered = self.worker.linear_memory_grant.lock().unwrap();
        if registered
            .as_ref()
            .is_some_and(|grant| Arc::ptr_eq(grant, &self.grant))
        {
            registered.take();
        }
    }
}

impl RunningWorker {
    pub async fn new<Ctx: WorkerCtx>(
        owned_agent_id: OwnedAgentId,
        queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        parent: Arc<Worker<Ctx>>,
        memory_grant: MemoryGrant,
        component_charge: WorkerComponentCharge,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        oom_retry_count: u32,
        start_attempt: Uuid,
        worker_trace: WorkerTrace,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender.send(WorkerCommand::WorkAvailable).unwrap();

        let active_clone = queue.clone();
        let owned_agent_id_clone = owned_agent_id.clone();
        let waiting_for_command = Arc::new(AtomicBool::new(false));
        let waiting_for_command_clone = waiting_for_command.clone();
        let concurrent_agent_permit_held = Arc::new(AtomicBool::new(true));
        let concurrent_agent_permit_held_clone = Arc::clone(&concurrent_agent_permit_held);
        let filesystem_activity = Arc::new(StdMutex::new(None));
        let filesystem_activity_clone = Arc::clone(&filesystem_activity);
        let unload_request = Arc::new(StdMutex::new(None));
        let unload_request_clone = Arc::clone(&unload_request);
        let idle_since_millis = Arc::new(AtomicU64::new(0));
        let idle_since_millis_clone = Arc::clone(&idle_since_millis);
        let interrupt_signal = parent.interrupt_signal.clone();
        let interrupt_signal_clone = interrupt_signal.clone();
        let resume_replay_pending = Arc::new(AtomicBool::new(false));
        let resume_replay_pending_clone = resume_replay_pending.clone();
        let memory_grant = Arc::new(StdMutex::new(memory_grant));
        let memory_grant_registration =
            LinearMemoryGrantRegistration::new(parent.clone(), memory_grant);

        let panic_parent = Arc::clone(&parent);
        let invocation_loop_task = async move {
            RunningWorker::invocation_loop(
                receiver,
                active_clone,
                owned_agent_id_clone,
                parent,
                waiting_for_command_clone,
                interrupt_signal_clone,
                oom_retry_count,
                concurrent_agent_permit,
                concurrent_agent_permit_held_clone,
                filesystem_activity_clone,
                unload_request_clone,
                idle_since_millis_clone,
                resume_replay_pending_clone,
                start_attempt,
                worker_trace,
            )
            .await;
            drop((memory_grant_registration, component_charge));
        };
        let handle = tokio::task::spawn(async move {
            run_invocation_loop_task(
                invocation_loop_task,
                move |error: WorkerExecutorError| async move {
                    panic_parent.complete_startup(start_attempt, Err(error.clone()));
                    panic_parent
                        .stop_internal(
                            true,
                            Some(error.clone()),
                            UnloadRequest::ordinary(UnloadReason::Panic),
                            FinalWorkerState::CleanupFailed(error),
                            PendingLiveInvocationDisposition::Fail,
                        )
                        .await;
                },
            )
            .await;
        });

        RunningWorker {
            handle: Some(handle),
            sender,
            queue,
            waiting_for_command,
            concurrent_agent_permit_held,
            filesystem_activity,
            unload_request,
            idle_since_millis,
            interrupt_signal,
            resume_replay_pending,
            start_attempt,
        }
    }

    pub fn stop(mut self, unload_request: UnloadRequest) -> JoinHandle<()> {
        *self.unload_request.lock().unwrap() = Some(unload_request);
        self.handle.take().unwrap()
    }

    async fn create_instance<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
    ) -> Result<
        (
            WorkerRunningAgent<Ctx>,
            crate::services::resource_usage_metering::ResourceUsageMeteringWindow,
            Option<RetryDecision>,
        ),
        CreateWorkerInstanceError,
    > {
        let component_id = parent.owned_agent_id.component_id();

        // we might have detached the worker status during the last invocation loop. Make sure it's attached and we are fully up-to-date on the oplog
        parent.reattach_worker_status().await;

        let worker_metadata = parent.get_latest_worker_metadata().await;
        debug!("Creating instance with parent metadata {worker_metadata:?}");

        let (pending_update, component, component_metadata) = {
            let pending_update_ref = worker_metadata
                .last_known_status
                .pending_updates
                .front()
                .cloned();

            let component_revision = pending_update_ref.as_ref().map_or(
                worker_metadata.last_known_status.component_revision,
                |update| {
                    let target_revision = update.target_revision;
                    info!(
                        "Attempting {} update from {} to revision {target_revision}",
                        match update.kind {
                            PendingUpdateKind::Automatic => "automatic",
                            PendingUpdateKind::SnapshotBased => "snapshot based",
                        },
                        worker_metadata.last_known_status.component_revision
                    );
                    target_revision
                },
            );

            match parent
                .component_service()
                .get(&parent.engine(), component_id, component_revision)
                .await
            {
                Ok((component, component_metadata)) => {
                    // The status record only keeps a lightweight reference to the pending update;
                    // hydrate the full description (including any snapshot payload) from the oplog
                    // before handing it to the worker context.
                    let pending_update = match &pending_update_ref {
                        Some(pending_update_ref) => {
                            Some(parent.hydrate_pending_update(pending_update_ref).await?)
                        }
                        None => None,
                    };
                    Ok((pending_update, component, component_metadata))
                }
                Err(error) => {
                    if component_revision != worker_metadata.last_known_status.component_revision {
                        // An update was attempted but the targeted version does not exist
                        warn!(
                            "Attempting update to revision {component_revision} failed with {error}"
                        );

                        parent
                            .add_and_commit_oplog(OplogEntry::failed_update(
                                component_revision,
                                Some(error.to_string()),
                            ))
                            .await;

                        // The update is now marked failed in the parent, we can retry.
                        return Box::pin(Self::create_instance(parent, concurrent_agent_permit))
                            .await;
                    } else {
                        Err(error)
                    }
                }
            }?
        };

        if component_metadata.metadata.has_shared_linear_memory() {
            return Err(shared_linear_memory_error(&parent).into());
        }

        // Refresh the snapshot used by the read-only cache key. The component
        // metadata was already fetched above, so no extra fetch is incurred.
        parent
            .current_component
            .store(Arc::new(component_metadata.clone()));

        let component_version_for_replay = worker_metadata
            .last_known_status
            .pending_updates
            .front()
            .and_then(|update| match update.kind {
                PendingUpdateKind::SnapshotBased => Some(update.target_revision),
                PendingUpdateKind::Automatic => None,
            })
            .unwrap_or(
                worker_metadata
                    .last_known_status
                    .component_revision_for_replay,
            );

        let component_metadata_for_replay =
            if component_metadata.revision == component_version_for_replay {
                component_metadata.clone()
            } else {
                parent
                    .component_service()
                    .get_metadata(component_id, Some(component_version_for_replay))
                    .await?
            };

        let agent_effective_surface = match &parent.parsed_agent_id {
            Some(agent_id) => agent_effective_surface_from_component_metadata(
                &component_metadata_for_replay,
                &parent.owned_agent_id,
                agent_id,
            )?,
            None => golem_common::model::card::EffectiveSurface::default(),
        };

        let mut skipped_regions = worker_metadata.last_known_status.skipped_regions;
        let mut last_snapshot_index = worker_metadata
            .last_known_status
            .last_manual_update_snapshot_index;

        // automatic snapshots are only considered until the first failure.
        // additionally, if there are updates, the automatic snapshot is temporarily ignored to catch issues earlier
        if let Some(snapshot_idx) = worker_metadata
            .last_known_status
            .last_automatic_snapshot_index
            && pending_update.is_none()
            && !parent.snapshot_recovery_disabled.load(Ordering::Acquire)
        {
            let snapshot_skip =
                DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                    OplogIndex::INITIAL.next()..=snapshot_idx,
                )])
                .build();
            skipped_regions.set_override(snapshot_skip);

            last_snapshot_index = Some(snapshot_idx);
        }

        let filesystems = parent.active_agents().agent_filesystems();
        let initial_files = parent
            .parsed_agent_id
            .as_ref()
            .and_then(|agent_id| {
                component_metadata_for_replay
                    .metadata
                    .agent_type_provision_configs()
                    .get(&agent_id.agent_type)
            })
            .map(|config| config.files.clone())
            .unwrap_or_default();
        let limits = filesystems
            .resolved_limits(parent.resource_entry.max_disk_space_limit())
            .map_err(|error| CreateWorkerInstanceError {
                error: WorkerExecutorError::runtime(error.to_string()),
                filesystem_cleanup_failed: error.cleanup_failed(),
            })?;
        let pressure = filesystems.pressure_policy();
        let pressure_recovery =
            crate::filesystem_pressure::FilesystemWriteRecovery::for_active_agents(
                filesystems.volume().clone(),
                Arc::downgrade(&parent.active_agents()),
                crate::filesystem_pressure::FilesystemWritePressurePolicy::from_config(pressure),
            );
        let created = filesystems
            .create_fresh_with_pressure_recovery(
                parent.owned_agent_id.clone(),
                limits,
                pressure_recovery,
            )
            .await
            .map_err(|failure| CreateWorkerInstanceError {
                error: WorkerExecutorError::runtime(failure.source.to_string()),
                filesystem_cleanup_failed: failure.source.cleanup_failed(),
            })?;
        let retained_memory_grant = parent.linear_memory_grant();
        let admitted_startup_bytes = retained_memory_grant.lock().unwrap().bytes();
        let linear_memory = LinearMemoryTracker::new_with_metering(
            parent.startup_linear_memory_bytes(),
            admitted_startup_bytes,
            parent.agent_mode(),
            false,
            Arc::clone(&parent.resource_entry),
            retained_memory_grant,
            parent.config().resource_usage_metering.memory,
        );
        let reconstructing = match bind_configured_resource_usage_metering(
            created,
            ResourceUsageAccount::new(
                parent.agent_mode(),
                linear_memory.clone(),
                Arc::clone(&parent.resource_entry),
            ),
            parent.config().resource_usage_metering,
        ) {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                let startup_error = WorkerExecutorError::runtime(failure.source.to_string());
                return Err(match delete_created(failure.filesystem).await {
                    Ok(()) => startup_error.into(),
                    Err(cleanup_error) => CreateWorkerInstanceError {
                        error: WorkerExecutorError::runtime(format!(
                            "{startup_error}; additionally failed to clean up the created agent filesystem: {}",
                            cleanup_error.source
                        )),
                        filesystem_cleanup_failed: true,
                    },
                });
            }
        };
        let window =
            match open_resource_usage_window(&reconstructing, concurrent_agent_permit).await {
                Ok(window) => window,
                Err(error) => {
                    let sealed = abort_reconstruction(reconstructing);
                    return Err(cleanup_typed_agent_filesystem(
                        sealed,
                        WorkerExecutorError::runtime(format!(
                            "Failed to open worker resource usage window: {error}"
                        )),
                    )
                    .await);
                }
            };
        let prepared = match prepare_initial_files(
            &parent.file_loader(),
            parent.owned_agent_id.environment_id,
            &initial_files,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(cleanup_reconstructing_agent_filesystem(
                    reconstructing,
                    window,
                    WorkerExecutorError::runtime(error.to_string()),
                )
                .await);
            }
        };
        let reconstructing = match materialize_initial_files(reconstructing, prepared).await {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                let startup_error = reconstruction_startup_error(failure.source);
                return Err(cleanup_open_agent_filesystem(
                    failure.filesystem,
                    window,
                    startup_error,
                )
                .await);
            }
        };
        let reconstruction_generation_handle =
            match reconstruction_generation_handle(&reconstructing) {
                Ok(generation_handle) => generation_handle,
                Err(error) => {
                    return Err(cleanup_reconstructing_agent_filesystem(
                        reconstructing,
                        window,
                        WorkerExecutorError::runtime(error.to_string()),
                    )
                    .await);
                }
            };
        let filesystem_publication = FilesystemGenerationPublication::new(
            Arc::clone(&parent.owner_runtime_resources),
            reconstruction_generation_handle.clone(),
        );
        let filesystem_context = match create_filesystem_context(reconstruction_generation_handle)
            .await
        {
            Ok(context) => context,
            Err(error) => {
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        let context = match Ctx::create(
            worker_metadata.created_by,
            OwnedAgentId::new(worker_metadata.environment_id, &worker_metadata.agent_id),
            parent.parsed_agent_id.clone(),
            parent.promise_service(),
            parent.worker_service(),
            parent.worker_enumeration_service(),
            parent.key_value_service(),
            parent.blob_store_service(),
            parent.rdbms_service(),
            parent.quota_service(),
            parent.worker_event_service.clone(),
            parent.active_agents(),
            parent.oplog_service(),
            parent.oplog.clone(),
            Arc::downgrade(&parent),
            parent.scheduler_service(),
            parent.rpc(),
            parent.worker_proxy(),
            parent.card_service(),
            parent.card_interest_index.clone(),
            parent.component_service(),
            parent.extra_deps(),
            parent.config(),
            filesystem_context,
            linear_memory,
            AgentConfig::new(
                skipped_regions,
                worker_metadata.last_known_status.total_linear_memory_size,
                component_version_for_replay,
                worker_metadata.created_by,
                worker_metadata.created_by_email,
                worker_metadata.config,
                last_snapshot_index,
                agent_effective_surface,
                None,
            ),
            parent.execution_status.clone(),
            parent.file_loader(),
            parent.worker_fork_service(),
            parent.resource_limits(),
            parent.agent_types(),
            parent.environment_state_service(),
            parent.agent_webhooks(),
            parent.shard_service(),
            parent.http_connection_pool(),
            parent.websocket_connection_pool(),
            pending_update,
            worker_metadata.original_phantom_id,
            OwnerRuntime::Agent,
            parent.owner_execution(),
            parent.owner_runtime_resources(),
            FilesystemCapability::Capable,
            component_metadata_for_replay,
            None,
        )
        .await
        {
            Ok(context) => context,
            Err(error) => {
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };

        let instance_host = match instance::InstanceHost::new(
            &parent,
            OwnerRuntime::Agent,
            ExecutableTarget::new(component_id, component_metadata.revision),
        ) {
            Ok(instance_host) => instance_host,
            Err(error) => {
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        let mut hosted = match instance_host.instantiate(context, &component).await {
            Ok(hosted) => hosted,
            Err(error) => {
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        if let Err(error) = instance_host.reconcile_linear_memories(&mut hosted).await {
            drop(hosted);
            return Err(
                cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
            );
        }
        let (instance, mut store) = hosted.into_parts();
        let prepare_result =
            Ctx::prepare_instance(&parent.owned_agent_id.agent_id, &instance, &mut store).await;
        let decision = match prepare_result {
            Ok(decision) => decision,
            Err(error) => {
                drop(store);
                return Err(
                    cleanup_reconstructing_agent_filesystem(reconstructing, window, error).await,
                );
            }
        };
        let reconstructing = match finish_replay(reconstructing).await {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                drop(store);
                let startup_error = reconstruction_startup_error(failure.source);
                return Err(cleanup_open_agent_filesystem(
                    failure.filesystem,
                    window,
                    startup_error,
                )
                .await);
            }
        };
        let filesystem = match finish_reconstruction(reconstructing).await {
            Ok(filesystem) => filesystem,
            Err(failure) => {
                drop(store);
                let startup_error = reconstruction_startup_error(failure.source);
                return Err(cleanup_open_agent_filesystem(
                    failure.filesystem,
                    window,
                    startup_error,
                )
                .await);
            }
        };
        let filesystem_generation = resident_generation_handle(&filesystem);
        store
            .data_mut()
            .durable_ctx_mut()
            .activate_resident_generation_handle(filesystem_generation.clone());
        filesystem_publication.commit(filesystem_generation);
        Ok((
            RunningAgent {
                runtime: RunningAgentRuntime {
                    instance,
                    store: async_lock::Mutex::new(store),
                },
                filesystem,
            },
            window,
            decision,
        ))
    }

    async fn invocation_loop<Ctx: WorkerCtx>(
        receiver: UnboundedReceiver<WorkerCommand>,
        active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        owned_agent_id: OwnedAgentId,
        parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
        waiting_for_command: Arc<AtomicBool>,
        interrupt_signal: Arc<async_lock::Mutex<WorkerInterruptState>>,
        oom_retry_count: u32,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        concurrent_agent_permit_held: Arc<AtomicBool>,
        filesystem_activity: Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
        unload_request: Arc<StdMutex<Option<UnloadRequest>>>,
        idle_since_millis: Arc<AtomicU64>,
        resume_replay_pending: Arc<AtomicBool>,
        start_attempt: Uuid,
        worker_trace: WorkerTrace,
    ) {
        let mut invocation_loop = InvocationLoop {
            receiver,
            active,
            owned_agent_id,
            parent,
            waiting_for_command,
            interrupt_signal,
            oom_retry_count,
            permit_state: ConcurrentAgentPermitState::new(
                Some(concurrent_agent_permit.track_held(Arc::clone(&concurrent_agent_permit_held))),
                concurrent_agent_permit_held,
            ),
            filesystem_activity,
            unload_request,
            idle_since_millis,
            resume_replay_pending,
            start_attempt,
            worker_trace,
        };
        invocation_loop.run().await;
    }
}

struct FilesystemGenerationPublication {
    resources: Arc<OwnerRuntimeResources>,
    clear_on_drop: bool,
}

impl FilesystemGenerationPublication {
    fn new(
        resources: Arc<OwnerRuntimeResources>,
        generation_handle: FilesystemGenerationHandle,
    ) -> Self {
        resources.activate_filesystem_generation(generation_handle);
        Self {
            resources,
            clear_on_drop: true,
        }
    }

    fn commit(mut self, generation_handle: FilesystemGenerationHandle) {
        self.resources
            .activate_filesystem_generation(generation_handle);
        self.clear_on_drop = false;
    }
}

impl Drop for FilesystemGenerationPublication {
    fn drop(&mut self) {
        if self.clear_on_drop {
            self.resources.fence_filesystem_generation();
        }
    }
}

async fn create_filesystem_context(
    generation_handle: FilesystemGenerationHandle,
) -> Result<WorkerFilesystemContext, WorkerExecutorError> {
    let target = PathTarget::at_root(&generation_handle, "")
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let preopen = open_agent_filesystem(
        &generation_handle,
        target,
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::ReadWrite,
            follow: Follow::Yes,
        },
    )
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    .await
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    .node;
    Ok(WorkerFilesystemContext::new(generation_handle, preopen))
}

async fn cleanup_reconstructing_agent_filesystem(
    filesystem: ReconstructingFilesystem,
    window: crate::services::resource_usage_metering::ResourceUsageMeteringWindow,
    startup_error: WorkerExecutorError,
) -> CreateWorkerInstanceError {
    cleanup_open_agent_filesystem(abort_reconstruction(filesystem), window, startup_error).await
}

fn reconstruction_startup_error(
    error: crate::services::agent_filesystem::Error,
) -> WorkerExecutorError {
    match error {
        crate::services::agent_filesystem::Error::AgentQuota(_) => {
            WorkerExecutorError::Interrupted {
                kind: InterruptKind::Suspend(Timestamp::now_utc()),
            }
        }
        error => WorkerExecutorError::runtime(error.to_string()),
    }
}

async fn cleanup_open_agent_filesystem(
    filesystem: SealedFilesystem,
    window: crate::services::resource_usage_metering::ResourceUsageMeteringWindow,
    startup_error: WorkerExecutorError,
) -> CreateWorkerInstanceError {
    crate::services::agent_filesystem::drain_sealed_filesystem(&filesystem).await;
    let close_error = crate::services::resource_usage_metering::close_window(
        window,
        std::time::Instant::now() + Duration::from_secs(30),
    )
    .await
    .err();
    let startup_error = match close_error {
        Some(error) => WorkerExecutorError::runtime(format!("{startup_error}; {error}")),
        None => startup_error,
    };
    cleanup_typed_agent_filesystem(filesystem, startup_error).await
}

async fn cleanup_typed_agent_filesystem(
    filesystem: SealedFilesystem,
    startup_error: WorkerExecutorError,
) -> CreateWorkerInstanceError {
    match delete_agent_filesystem(filesystem).await {
        Ok(()) => startup_error.into(),
        Err(cleanup_error) => {
            warn!(error = %cleanup_error.source, "Failed to clean up filesystem after worker startup failure");
            CreateWorkerInstanceError {
                error: WorkerExecutorError::runtime(format!(
                    "{startup_error}; additionally failed to clean up the agent filesystem: {}",
                    cleanup_error.source
                )),
                filesystem_cleanup_failed: true,
            }
        }
    }
}

fn shared_linear_memory_error<Ctx: WorkerCtx>(parent: &Arc<Worker<Ctx>>) -> WorkerExecutorError {
    WorkerExecutorError::worker_creation_failed(
        parent.owned_agent_id.agent_id(),
        SHARED_LINEAR_MEMORY_ERROR,
    )
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

fn running_worker_can_be_evicted(
    waiting_for_command: bool,
    has_queued_internal_work: bool,
    has_resume_replay: bool,
    has_interrupt: bool,
    has_filesystem_effects: bool,
    has_concurrent_agent_permit: bool,
) -> bool {
    waiting_for_command
        && !has_queued_internal_work
        && !has_resume_replay
        && !has_interrupt
        && !has_filesystem_effects
        && !has_concurrent_agent_permit
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvictionStopOutcome {
    Ineligible,
    Unloaded,
    CleanupFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilesystemPressureEligibility {
    idle_since: u64,
    last_effect_completion: u64,
}

impl EvictionClass {
    /// Lower values are evicted first.
    pub fn eviction_priority(self) -> u8 {
        match self {
            EvictionClass::LoadedIdle => 0,
            EvictionClass::WarmRunnable => 1,
        }
    }
}

#[derive(Debug)]
pub(crate) enum FinalWorkerState {
    Unloaded {
        startup_failure: Option<WorkerExecutorError>,
    },
    CleanupFailed(WorkerExecutorError),
    Deleting,
}

impl FinalWorkerState {
    fn into_instance(self) -> WorkerInstance {
        match self {
            FinalWorkerState::Unloaded { startup_failure } => {
                WorkerInstance::Unloaded { startup_failure }
            }
            FinalWorkerState::CleanupFailed(error) => WorkerInstance::CleanupFailed(error),
            FinalWorkerState::Deleting => WorkerInstance::Deleting,
        }
    }
}

fn merge_final_worker_state(
    current: FinalWorkerState,
    requested: FinalWorkerState,
) -> FinalWorkerState {
    match (&current, &requested) {
        (FinalWorkerState::CleanupFailed(_), _) => current,
        (_, FinalWorkerState::CleanupFailed(_)) => requested,
        (FinalWorkerState::Deleting, _) => current,
        (_, FinalWorkerState::Deleting) => requested,
        (
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            FinalWorkerState::Unloaded {
                startup_failure: Some(_),
            },
        ) => requested,
        _ => current,
    }
}

fn merge_run_loop_failure(instance: &mut WorkerInstance, error: WorkerExecutorError) {
    match instance {
        WorkerInstance::Stopping(stopping) => {
            stopping.final_state = merge_final_worker_state(
                std::mem::replace(
                    &mut stopping.final_state,
                    FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                ),
                FinalWorkerState::CleanupFailed(error),
            );
        }
        WorkerInstance::CleanupFailed(_) => {}
        _ => *instance = WorkerInstance::CleanupFailed(error),
    }
}

fn complete_stopping_worker(
    mut stopping: StoppingWorker,
    final_state: FinalWorkerState,
) -> (WorkerInstance, OneShotEvent) {
    stopping.final_state = merge_final_worker_state(stopping.final_state, final_state);
    let notify = stopping.notify;
    (stopping.final_state.into_instance(), notify)
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

impl InvocationResult {
    pub async fn cache<T: HasOplog + HasOplogService + HasConfig + HasComponentService>(
        &mut self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        agent_fingerprint: AgentFingerprint,
        services: &T,
    ) {
        if let Self::Lazy { oplog_idx } = self {
            let oplog_idx = *oplog_idx;
            let entry = services.oplog().read(oplog_idx).await;

            let result = match entry {
                OplogEntry::AgentInvocationFinished {
                    result,
                    consumed_fuel,
                    component_revision,
                    ..
                } => {
                    let invocation_result: AgentInvocationResult = services
                        .oplog()
                        .download_payload(result)
                        .await
                        .expect("failed to deserialize function response payload");
                    Ok(AgentInvocationOutput {
                        result: invocation_result,
                        consumed_fuel: Some(consumed_fuel as u64),
                        invocation_status: None,
                        component_revision: Some(component_revision),
                        agent_id: None,
                        idempotency_key: None,
                        // `oplog_idx` is the index of the matched
                        // `AgentInvocationFinished` entry. The fingerprint is
                        // the current worker's per-instance fingerprint: the
                        // oplog is owned by a single worker instance, so any
                        // `AgentInvocationFinished` we read from it was
                        // necessarily produced by that instance.
                        oplog_index: Some(oplog_idx),
                        agent_fingerprint: Some(agent_fingerprint),
                    })
                }
                OplogEntry::Error {
                    error,
                    retry_from,
                    inside_atomic_region,
                    ..
                } => {
                    let stderr =
                        recover_stderr_logs(services, owned_agent_id, agent_mode, oplog_idx).await;
                    Err(FailedInvocationResult {
                        trap_type: TrapType::Error {
                            error,
                            retry_from,
                            // Membership is not persisted; only the side-effect flag is. This
                            // reconstructed trap is used for error reporting, not for re-deciding
                            // recovery, so the membership bit is irrelevant here.
                            in_atomic_region: inside_atomic_region,
                            atomic_region_had_side_effects: inside_atomic_region,
                            semantic_trap_retry_override: None,
                        },
                        stderr,
                    })
                }
                OplogEntry::Interrupted { .. } => Err(FailedInvocationResult {
                    trap_type: TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::now_utc())),
                    stderr: "".to_string(),
                }),
                OplogEntry::Exited { .. } => Err(FailedInvocationResult {
                    trap_type: TrapType::Exit,
                    stderr: "".to_string(),
                }),
                _ => panic!(
                    "Unexpected oplog entry pointed by invocation result at index {oplog_idx} for {owned_agent_id:?}"
                ),
            };

            *self = Self::Cached { result }
        }
    }
}

fn lookup_result_from_cached_result(
    status: &AgentStatusRecord,
    key: &IdempotencyKey,
    result: InvocationResult,
) -> LookupResult {
    match result {
        InvocationResult::Cached {
            result: Ok(values), ..
        } => LookupResult::Complete(Ok(values)),
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    // Retry marker error entries are persisted before the invocation has
                    // actually finished. While the same idempotency key is still current
                    // and the worker has not entered a terminal state, report it as
                    // pending so lookup callers can observe the eventual terminal result.
                    trap_type: TrapType::Error { .. },
                    ..
                }),
        } if status.current_idempotency_key.as_ref() == Some(key)
            && !matches!(status.status, AgentStatus::Failed | AgentStatus::Exited) =>
        {
            LookupResult::Pending
        }
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Interrupt(InterruptKind::Interrupt(_)),
                    ..
                }),
            ..
        } => LookupResult::Interrupted,
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Interrupt(_),
                    ..
                }),
            ..
        } => LookupResult::Pending,
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type:
                        TrapType::Error {
                            error: AgentError::PermissionDenied(details),
                            ..
                        },
                    ..
                }),
            ..
        } => LookupResult::Complete(Err(WorkerExecutorError::permission_denied(details))),
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Error { error, .. },
                    stderr,
                }),
            ..
        } => LookupResult::Complete(Err(WorkerExecutorError::InvocationFailed { error, stderr })),
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Exit,
                    ..
                }),
            ..
        } => LookupResult::Complete(Err(WorkerExecutorError::runtime("Process exited"))),
        InvocationResult::Lazy { .. } => {
            panic!("Unexpected lazy result after InvocationResult.cache")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::AgentError;
    use std::path::Path;
    use test_r::test;

    #[test]
    fn reconstruction_agent_quota_maps_to_startup_suspension() {
        let error =
            reconstruction_startup_error(crate::services::agent_filesystem::Error::AgentQuota(
                crate::services::agent_filesystem::FilesystemStorageError::verification(
                    "seed initial file",
                    Path::new("<scripted>"),
                ),
            ));

        assert!(matches!(
            error,
            WorkerExecutorError::Interrupted {
                kind: InterruptKind::Suspend(_)
            }
        ));
    }

    #[test]
    fn active_filesystem_effects_exclude_an_otherwise_idle_worker() {
        assert!(running_worker_can_be_evicted(
            true, false, false, false, false, false
        ));
        assert!(!running_worker_can_be_evicted(
            true, false, false, false, true, false
        ));
        assert!(!running_worker_can_be_evicted(
            true, false, false, false, false, true
        ));
    }

    #[test]
    fn merging_unloaded_states_preserves_startup_failure() {
        let state = merge_final_worker_state(
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            FinalWorkerState::Unloaded {
                startup_failure: Some(WorkerExecutorError::runtime("startup failed")),
            },
        );

        assert!(matches!(
            state,
            FinalWorkerState::Unloaded {
                startup_failure: Some(_)
            }
        ));
    }

    #[test]
    fn concurrent_startup_callers_share_one_attempt() {
        let tracker = Arc::new(StartupAttemptTracker::default());
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let handles = (0..16)
            .map(|_| {
                let tracker = tracker.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    tracker.begin(None)
                })
            })
            .collect::<Vec<_>>();

        let attempts = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert!(attempts.iter().all(|attempt| *attempt == attempts[0]));
        assert_eq!(tracker.pending(), Some(attempts[0]));
    }

    #[test]
    fn oom_retry_preserves_startup_attempt() {
        let tracker = StartupAttemptTracker::default();
        let initial = tracker.begin(None);

        let retry = tracker.begin(Some(initial));

        assert_eq!(retry, initial);
        assert_eq!(tracker.current().unwrap(), Some(initial));
        assert!(!tracker.complete(Uuid::new_v4(), &Ok(())));
        assert_eq!(tracker.pending(), Some(initial));
        assert!(tracker.complete(initial, &Ok(())));
        assert_eq!(tracker.current().unwrap(), None);
    }

    #[test]
    fn startup_success_requires_matching_active_attempt() {
        let tracker = StartupAttemptTracker::default();
        let attempt = tracker.begin(None);

        assert!(!tracker.complete_success_if_active(attempt, None));
        assert_eq!(tracker.pending(), Some(attempt));
        assert!(!tracker.complete_success_if_active(attempt, Some(Uuid::new_v4())));
        assert_eq!(tracker.pending(), Some(attempt));
        assert!(tracker.complete_success_if_active(attempt, Some(attempt)));
        assert_eq!(tracker.current().unwrap(), None);
    }

    #[test]
    fn stop_wins_atomic_startup_completion() {
        let tracker = Arc::new(StartupAttemptTracker::default());
        let attempt = tracker.begin(None);
        let active_attempt = Arc::new(StdMutex::new(Some(attempt)));
        let (stop_holds_instance, wait_for_stop) = std::sync::mpsc::channel();
        let (allow_stop_to_finish, finish_stop) = std::sync::mpsc::channel();

        let stop = {
            let active_attempt = active_attempt.clone();
            std::thread::spawn(move || {
                let mut active_attempt = active_attempt.lock().unwrap();
                *active_attempt = None;
                stop_holds_instance.send(()).unwrap();
                finish_stop.recv().unwrap();
            })
        };

        wait_for_stop.recv().unwrap();
        let (completion_started, wait_for_completion) = std::sync::mpsc::channel();
        let completion = {
            let tracker = tracker.clone();
            let active_attempt = active_attempt.clone();
            std::thread::spawn(move || {
                completion_started.send(()).unwrap();
                let active_attempt = *active_attempt.lock().unwrap();
                assert!(!tracker.complete_success_if_active(attempt, active_attempt));
                let error = WorkerExecutorError::unknown("Worker stopped before startup completed");
                assert!(tracker.complete(attempt, &Err(error.clone())));
                tracker.current()
            })
        };

        wait_for_completion.recv().unwrap();
        allow_stop_to_finish.send(()).unwrap();
        stop.join().unwrap();
        let error = WorkerExecutorError::unknown("Worker stopped before startup completed");
        assert!(
            matches!(completion.join().unwrap(), Err(actual) if actual.to_string() == error.to_string())
        );
    }

    #[test]
    fn allocated_memory_sums_unique_untouched_backings() -> anyhow::Result<()> {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
                (memory $aliased 2 3)
                (export "a" (memory $aliased))
                (export "b" (memory $aliased))
                (memory 4 5)
            )"#,
        )?;
        let mut store = Store::new(&engine, ());
        wasmtime::Instance::new(&mut store, &module, &[])?;

        assert_eq!(
            instance::allocated_linear_memory_bytes(&store),
            Ok(6 * 65_536)
        );
        Ok(())
    }

    #[test]
    fn reconstructed_ephemeral_agent_is_terminal() {
        let instance = WorkerInstance::Unloaded {
            startup_failure: Some(inactive_ephemeral_agent_error()),
        };

        assert!(matches!(
            instance.startup_failure(),
            Some(WorkerExecutorError::InvalidRequest { details })
                if details == "An ephemeral agent cannot accept another invocation or be resumed"
        ));
    }

    #[test]
    fn filesystem_cleanup_failure_overrides_pending_unload() {
        let cleanup_error = WorkerExecutorError::runtime("cleanup failed");
        let final_state = merge_final_worker_state(
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            FinalWorkerState::CleanupFailed(cleanup_error),
        );

        assert!(matches!(final_state, FinalWorkerState::CleanupFailed(_)));
    }

    #[test]
    fn filesystem_cleanup_failure_prevents_successful_deletion() {
        let final_state = merge_final_worker_state(
            FinalWorkerState::Deleting,
            FinalWorkerState::CleanupFailed(WorkerExecutorError::runtime("cleanup failed")),
        );

        assert!(matches!(final_state, FinalWorkerState::CleanupFailed(_)));
    }

    #[test]
    async fn panicked_run_loop_join_becomes_cleanup_failure_and_notifies_stopper() {
        let run_loop = tokio::spawn(async { panic!("injected run-loop panic") });
        let error = run_loop.await.expect_err("run loop must panic");
        let error = WorkerExecutorError::runtime(format!(
            "invocation loop task stopped unexpectedly: {error}"
        ));
        let notify = OneShotEvent::new();
        let waiter = notify.clone();
        let mut instance = WorkerInstance::Stopping(StoppingWorker {
            notify: notify.clone(),
            final_state: FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
        });

        merge_run_loop_failure(&mut instance, error);
        notify.set();

        tokio::time::timeout(Duration::from_secs(1), waiter.wait())
            .await
            .expect("stop notification must not be stranded");
        assert!(matches!(
            instance,
            WorkerInstance::Stopping(StoppingWorker {
                final_state: FinalWorkerState::CleanupFailed(_),
                ..
            })
        ));
    }

    #[test]
    fn ephemeral_agent_accepts_only_one_invocation_identity() {
        let first = IdempotencyKey::fresh();
        let second = IdempotencyKey::fresh();
        let mut state = EphemeralInvocationState::Available;

        assert!(state.accept(&first).is_ok());
        assert!(state.accept(&first).is_ok());
        assert!(state.accept(&second).is_err());

        let mut reconstructed = EphemeralInvocationState::Accepted(None);
        assert!(reconstructed.accept(&first).is_err());
    }

    #[test]
    fn ephemeral_invocation_requires_a_final_phantom_id_even_when_it_may_exist() {
        let result = validate_resolved_invocation_identity(
            AgentMode::Ephemeral,
            None,
            &IdempotencyKey::fresh(),
            InvocationFreshnessDisposition::MayExist,
        );

        assert!(matches!(
            result,
            Err(WorkerExecutorError::InvalidRequest { details })
                if details == "An ephemeral invocation requires a final phantom agent ID"
        ));
    }

    #[test]
    fn known_fresh_ephemeral_invocation_requires_key_derived_phantom_id() {
        let idempotency_key = IdempotencyKey::fresh();
        let expected_phantom_id = ephemeral_invocation_phantom_id(&idempotency_key);

        assert!(
            validate_resolved_invocation_identity(
                AgentMode::Ephemeral,
                Some(expected_phantom_id),
                &idempotency_key,
                InvocationFreshnessDisposition::KnownFresh,
            )
            .is_ok()
        );
        assert!(
            validate_resolved_invocation_identity(
                AgentMode::Ephemeral,
                Some(Uuid::new_v4()),
                &idempotency_key,
                InvocationFreshnessDisposition::KnownFresh,
            )
            .is_err()
        );
    }

    #[test]
    fn durable_may_exist_invocation_identity_is_unchanged() {
        assert!(
            validate_resolved_invocation_identity(
                AgentMode::Durable,
                None,
                &IdempotencyKey::fresh(),
                InvocationFreshnessDisposition::MayExist,
            )
            .is_ok()
        );
    }

    #[test]
    fn start_attempt_exact_duplicate_matches_and_mismatch_does_not() {
        let environment_id =
            golem_common::base_model::environment::EnvironmentId(Uuid::from_u128(1));
        let agent_id = AgentId {
            component_id: golem_common::base_model::component::ComponentId(Uuid::from_u128(2)),
            agent_id: "callee".to_string(),
        };
        let fingerprint = AgentFingerprint(Uuid::from_u128(3));
        let session_key = golem_common::base_model::durable_stream::StreamInvocationIdV1 {
            callee_environment_id: environment_id,
            callee: agent_id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("invocation".to_string()),
        };
        let attempt = StartAttemptDescriptorV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: session_key.clone(),
            attachment_id: golem_common::base_model::durable_stream::AttachmentId::primary(
                environment_id,
                &agent_id,
                &session_key.idempotency_key,
            )
            .unwrap(),
            expected_callee_fingerprint: fingerprint,
            attempt_id: golem_common::base_model::durable_stream::AttemptId::fresh(),
            invocation: PersistedStreamInvocationDescriptorV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key,
                target_component_revision: ComponentRevision::INITIAL,
                method_name: "consume".to_string(),
                invocation_value: vec![1],
                stream_handles: Vec::new(),
                execution_config: vec![2],
                effective_identity: vec![3],
            },
            effective_identity: vec![3],
            live_join_buffer_events: 8,
        };

        assert!(stream_attempt_matches(&attempt, &attempt.clone()));

        let mut mismatched = attempt.clone();
        mismatched.live_join_buffer_events += 1;
        assert!(!stream_attempt_matches(&attempt, &mismatched));

        let mut mismatched = attempt.clone();
        mismatched.invocation.method_name = "other".to_string();
        assert!(!stream_attempt_matches(&attempt, &mismatched));

        let mut mismatched = attempt.clone();
        mismatched.attempt_id = golem_common::base_model::durable_stream::AttemptId::fresh();
        assert!(!stream_attempt_matches(&attempt, &mismatched));
    }

    fn status_with_current_key(status: AgentStatus, key: &IdempotencyKey) -> AgentStatusRecord {
        AgentStatusRecord {
            status,
            current_idempotency_key: Some(key.clone()),
            ..AgentStatusRecord::default()
        }
    }

    #[test]
    fn lookup_keeps_retrying_error_pending() {
        let key = IdempotencyKey::fresh();
        let lookup = lookup_result_from_cached_result(
            &status_with_current_key(AgentStatus::Retrying, &key),
            &key,
            InvocationResult::Cached {
                result: Err(FailedInvocationResult {
                    trap_type: TrapType::Error {
                        error: AgentError::TransientError("in-function retry".to_string()),
                        retry_from: OplogIndex::from_u64(17),
                        in_atomic_region: false,
                        atomic_region_had_side_effects: false,
                        semantic_trap_retry_override: None,
                    },
                    stderr: String::new(),
                }),
            },
        );

        assert!(matches!(lookup, LookupResult::Pending));
    }

    #[test]
    fn lookup_reports_terminal_error_as_failure() {
        let key = IdempotencyKey::fresh();
        let lookup = lookup_result_from_cached_result(
            &status_with_current_key(AgentStatus::Failed, &key),
            &key,
            InvocationResult::Cached {
                result: Err(FailedInvocationResult {
                    trap_type: TrapType::Error {
                        error: AgentError::TransientError("in-function retry".to_string()),
                        retry_from: OplogIndex::from_u64(17),
                        in_atomic_region: false,
                        atomic_region_had_side_effects: false,
                        semantic_trap_retry_override: None,
                    },
                    stderr: String::new(),
                }),
            },
        );

        match lookup {
            LookupResult::Complete(Err(WorkerExecutorError::InvocationFailed {
                error: AgentError::TransientError(details),
                stderr,
            })) => {
                assert_eq!(details, "in-function retry");
                assert!(stderr.is_empty());
            }
            other => panic!("expected terminal lookup failure, got {other:?}"),
        }
    }

    #[test]
    fn reconstructed_permission_denial_keeps_its_executor_error_type() {
        let key = IdempotencyKey::fresh();
        let lookup = lookup_result_from_cached_result(
            &AgentStatusRecord::default(),
            &key,
            InvocationResult::Cached {
                result: Err(FailedInvocationResult {
                    trap_type: TrapType::Error {
                        error: AgentError::PermissionDenied("permission denied".to_string()),
                        retry_from: OplogIndex::from_u64(17),
                        in_atomic_region: false,
                        atomic_region_had_side_effects: false,
                        semantic_trap_retry_override: None,
                    },
                    stderr: String::new(),
                }),
            },
        );

        assert!(matches!(
            lookup,
            LookupResult::Complete(Err(WorkerExecutorError::PermissionDenied { details }))
                if details == "permission denied"
        ));
    }

    #[test]
    fn invocation_rejection_fails_only_the_rejected_pending_key() {
        let rejected = IdempotencyKey::fresh();
        let still_pending = IdempotencyKey::fresh();
        let status = AgentStatusRecord {
            pending_invocations: vec![PendingInvocationRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(2),
                idempotency_key: Some(still_pending.clone()),
                manual_update_target_revision: None,
            }],
            ..AgentStatusRecord::default()
        };

        assert_eq!(
            invocation_keys_to_fail(&status, Some(&rejected), false),
            vec![rejected]
        );
        assert_eq!(
            invocation_keys_to_fail(&status, None, true),
            vec![still_pending]
        );
    }

    #[test]
    fn startup_charge_revision_uses_last_known_without_pending_update() {
        let last_known = ComponentRevision::INITIAL.next().unwrap();
        assert_eq!(
            component_charge_revision(None, last_known),
            last_known,
            "with no pending update the worker instantiates the last known revision"
        );
    }

    #[test]
    fn startup_charge_revision_uses_pending_update_target() {
        let last_known = ComponentRevision::INITIAL;
        let target = ComponentRevision::INITIAL.next().unwrap();
        assert_eq!(
            component_charge_revision(Some(target), last_known),
            target,
            "at startup a queued pending update is applied by loading its target revision, so the charge must key to the target, not the last known revision"
        );
    }

    #[test]
    fn classify_target_charge_charges_resolved_target() {
        assert_eq!(
            classify_target_charge(&Ok(ResolvedComponentCharge {
                module_bytes: 4096,
                initial_linear_memory_bytes: 8192,
                reserved_linear_memory_bytes: 16384,
            })),
            TargetChargeAction::ChargeTarget(ResolvedComponentCharge {
                module_bytes: 4096,
                initial_linear_memory_bytes: 8192,
                reserved_linear_memory_bytes: 16384,
            }),
            "a resolved target is charged with its own module size"
        );
    }

    #[test]
    fn classify_target_charge_falls_back_only_for_component_not_found() {
        let not_found = Err(WorkerExecutorError::ComponentNotFound {
            component_id: ComponentId(uuid::Uuid::new_v4()),
        });
        assert_eq!(
            classify_target_charge(&not_found),
            TargetChargeAction::FallBackToCurrent,
            "a non-existent target falls back to the current revision (create_instance fails the update and recovers)"
        );
    }

    #[test]
    fn classify_target_charge_retries_on_transient_error() {
        let transient = Err(WorkerExecutorError::runtime("registry unavailable"));
        assert_eq!(
            classify_target_charge(&transient),
            TargetChargeAction::Retry,
            "a transient resolution failure must retry, not fall back: create_instance may still load the target, so charging the current revision would under-reserve and mis-key the charge"
        );
    }

    #[test]
    fn interrupt_retry_decision_matrix() {
        fn decision(kind: InterruptKind, reacquire_permits: bool) -> RetryDecision {
            PendingWorkerInterrupt {
                kind,
                reacquire_permits,
                unload_request: UnloadRequest::ordinary(UnloadReason::from_interrupt(kind)),
            }
            .retry_decision()
        }

        // Restart-like interrupts retry immediately.
        assert_eq!(
            decision(InterruptKind::Restart, false),
            RetryDecision::Immediate
        );
        assert_eq!(
            decision(InterruptKind::Jump, false),
            RetryDecision::Immediate
        );

        // Explicit interrupts remain terminal.
        assert_eq!(
            decision(InterruptKind::Interrupt(Timestamp::now_utc()), false),
            RetryDecision::None
        );
        // Suspensions stop unless a newer wakeup supersedes them.
        let suspend_timestamp = Timestamp::now_utc();
        assert_eq!(
            decision(InterruptKind::Suspend(suspend_timestamp), false),
            RetryDecision::TryStop(suspend_timestamp)
        );

        // Permit reacquisition overrides the kind-based decision for every kind.
        for kind in [
            InterruptKind::Restart,
            InterruptKind::Jump,
            InterruptKind::Interrupt(Timestamp::now_utc()),
            InterruptKind::Suspend(Timestamp::now_utc()),
        ] {
            assert_eq!(
                decision(kind, true),
                RetryDecision::ReacquirePermits,
                "reacquire_permits must override the kind-based decision"
            );
        }
    }

    #[test]
    fn interrupt_terminality_matrix() {
        fn terminal(kind: InterruptKind) -> bool {
            PendingWorkerInterrupt {
                kind,
                reacquire_permits: false,
                unload_request: UnloadRequest::ordinary(UnloadReason::from_interrupt(kind)),
            }
            .is_terminal()
        }

        assert!(!terminal(InterruptKind::Restart));
        assert!(!terminal(InterruptKind::Jump));
        assert!(terminal(InterruptKind::Interrupt(Timestamp::now_utc())));
        assert!(terminal(InterruptKind::Suspend(Timestamp::now_utc())));
    }

    #[test]
    fn terminal_interrupt_claim_is_released_after_a_worker_generation() {
        let mut state = WorkerInterruptState::Idle;
        assert!(state.queue(PendingWorkerInterrupt {
            kind: InterruptKind::Suspend(Timestamp::now_utc()),
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
        }));
        assert!(state.take().is_some());
        assert!(!state.queue(PendingWorkerInterrupt {
            kind: InterruptKind::Restart,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Restart),
        }));

        state.reset_terminal_for_new_generation();

        assert!(state.queue(PendingWorkerInterrupt {
            kind: InterruptKind::Restart,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Restart),
        }));
    }

    #[test]
    fn terminal_interrupt_can_be_queued_for_a_resuming_claimed_generation() {
        let mut state = WorkerInterruptState::Pending(PendingWorkerInterrupt {
            kind: InterruptKind::Interrupt(Timestamp::now_utc()),
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
        });
        assert!(state.take().is_some());
        assert!(matches!(state, WorkerInterruptState::TerminalClaimed));

        let delete_interrupt = PendingWorkerInterrupt {
            kind: InterruptKind::Interrupt(Timestamp::now_utc()),
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Deleting),
        };
        assert!(state.queue(delete_interrupt));
        assert!(matches!(state, WorkerInterruptState::Pending(_)));
        state.reset_terminal_for_new_generation();
        assert!(matches!(state, WorkerInterruptState::Pending(_)));
    }
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

fn resolve_agent_properties<T: HasConfig>(
    deps: &T,
    agent_id: Option<&ParsedAgentId>,
    metadata: &golem_common::model::component_metadata::ComponentMetadata,
) -> ResolvedAgentProperties {
    let resolved_agent_type =
        agent_id.and_then(|id| metadata.find_agent_type_by_name_ref(&id.agent_type));

    let agent_mode = resolved_agent_type.map_or(AgentMode::Durable, |at| at.mode);

    let snapshot_policy = if let Some(agent_type) = resolved_agent_type {
        // Agent with explicit metadata — use agent-level snapshotting config
        resolve_snapshot_policy(
            &deps.config().oplog.default_snapshotting,
            Some(&agent_type.snapshotting),
        )
    } else if is_snapshot_capable_oplog_processor(metadata) {
        // Oplog processor that exports save-snapshot/load-snapshot — use the
        // oplog-processor-specific global config
        deps.config().oplog.oplog_processor_snapshotting.clone()
    } else {
        // Non-agent, non-snapshot-capable-oplog-processor — use default
        resolve_snapshot_policy(&deps.config().oplog.default_snapshotting, None)
    };

    ResolvedAgentProperties {
        agent_mode,
        snapshot_policy,
    }
}

fn resolve_snapshot_policy(
    default_config: &SnapshotPolicy,
    agent_snapshotting: Option<&Snapshotting>,
) -> SnapshotPolicy {
    match agent_snapshotting {
        None | Some(Snapshotting::Enabled(SnapshottingConfig::Default(_))) => {
            default_config.clone()
        }
        Some(Snapshotting::Disabled(_)) => SnapshotPolicy::Disabled,
        Some(Snapshotting::Enabled(SnapshottingConfig::Periodic(p))) => {
            let period = Duration::from_nanos(p.duration_nanos);
            if period.is_zero() {
                warn!("Agent snapshot periodic duration is zero, disabling");
                SnapshotPolicy::Disabled
            } else {
                SnapshotPolicy::Periodic { period }
            }
        }
        Some(Snapshotting::Enabled(SnapshottingConfig::EveryNInvocation(n))) => {
            if n.count == 0 {
                warn!("Agent snapshot every-n-invocation count is zero, disabling");
                SnapshotPolicy::Disabled
            } else {
                SnapshotPolicy::EveryNInvocation { count: n.count }
            }
        }
    }
}

/// Returns true if the component is an oplog processor that also exports
/// save-snapshot and load-snapshot functions, making it eligible for
/// automatic snapshot-based recovery.
fn is_snapshot_capable_oplog_processor(
    metadata: &golem_common::model::component_metadata::ComponentMetadata,
) -> bool {
    metadata.has_oplog_processor() && metadata.has_save_snapshot() && metadata.has_load_snapshot()
}

fn invocation_keys_to_fail(
    status: &AgentStatusRecord,
    first_key: Option<&IdempotencyKey>,
    include_pending_and_current: bool,
) -> Vec<IdempotencyKey> {
    let mut keys = Vec::new();

    if let Some(key) = first_key {
        keys.push(key.clone());
    }

    if !include_pending_and_current {
        return keys;
    }

    for pending_key in status
        .pending_invocations
        .iter()
        .filter_map(|entry| entry.idempotency_key())
    {
        if !keys.contains(pending_key) {
            keys.push(pending_key.clone());
        }
    }

    if let Some(current_key) = &status.current_idempotency_key
        && !keys.contains(current_key)
    {
        keys.push(current_key.clone());
    }

    keys
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
    // The worker will suspend execution until the stream is dropped, so consume in a timely manner.
    ReadFile {
        path: CanonicalFilePath,
        sender: oneshot::Sender<Result<ReadFileResult, WorkerExecutorError>>,
    },
    // Waits for the invocation loop to pick up this message, ensuring that the worker is ready to process followup commands.
    // The sender will be called with Ok if the worker is in a running state.
    // If the worker initialization fails and will not recover without manual intervention, it will be called with Err.
    AwaitReadyToProcessCommands {
        sender: oneshot::Sender<Result<(), WorkerExecutorError>>,
    },
    SaveSnapshot,
}

fn durable_stream_attempt_error_outcome(error: &WorkerExecutorError) -> &'static str {
    let (details, fallback) = match error {
        WorkerExecutorError::InvalidRequest { details } => (details.as_str(), "rejected"),
        WorkerExecutorError::Runtime { details } => (details.as_str(), "error"),
        _ => return "error",
    };
    [
        ("AttemptConflict:", "attempt_conflict"),
        ("IdempotencyConflict:", "idempotency_conflict"),
        ("StaleEpoch:", "stale_epoch"),
        ("InvalidEpoch:", "future_epoch"),
        ("InvalidAttachmentState:", "invalid_attachment_state"),
        ("IncarnationMismatch:", "incarnation_mismatch"),
        ("Unauthorized:", "unauthorized"),
        ("NotFound:", "not_found"),
        ("ResourceExhausted:", "resource_exhausted"),
    ]
    .into_iter()
    .find_map(|(prefix, outcome)| details.starts_with(prefix).then_some(outcome))
    .unwrap_or(fallback)
}

fn stream_attempt_matches(
    persisted: &StartAttemptDescriptorV1,
    requested: &StartAttemptDescriptorV1,
) -> bool {
    persisted.format_version == requested.format_version
        && persisted.session_key == requested.session_key
        && persisted.attachment_id == requested.attachment_id
        && persisted.expected_callee_fingerprint == requested.expected_callee_fingerprint
        && persisted.attempt_id == requested.attempt_id
        && persisted.effective_identity == requested.effective_identity
        && persisted.live_join_buffer_events == requested.live_join_buffer_events
        && persisted_stream_descriptor_matches(&persisted.invocation, &requested.invocation)
}

fn persisted_stream_descriptor_matches(
    persisted: &PersistedStreamInvocationDescriptorV1,
    requested: &PersistedStreamInvocationDescriptorV1,
) -> bool {
    persisted.format_version == requested.format_version
        && persisted.session_key == requested.session_key
        && persisted.target_component_revision == requested.target_component_revision
        && persisted.method_name == requested.method_name
        && persisted.invocation_value == requested.invocation_value
        && persisted.stream_handles == requested.stream_handles
        && persisted.execution_config == requested.execution_config
        && persisted.effective_identity == requested.effective_identity
}

fn stream_effective_identity_is_agent(effective_identity: &[u8]) -> bool {
    let mut cursor = effective_identity;
    let mut last = None;
    while cursor.len() >= 8 {
        let (length, rest) = cursor.split_at(8);
        let length = u64::from_be_bytes(
            length
                .try_into()
                .expect("effective identity length prefix has fixed width"),
        );
        let Ok(length) = usize::try_from(length) else {
            return false;
        };
        if rest.len() < length {
            return false;
        }
        let (value, rest) = rest.split_at(length);
        last = Some(value);
        cursor = rest;
    }
    if !cursor.is_empty() {
        return false;
    }
    last.and_then(|value| {
        golem_api_grpc::proto::golem::component::Principal::decode(value)
            .ok()
            .and_then(|principal| Principal::try_from(principal).ok())
    })
    .is_some_and(|principal| matches!(principal, Principal::Agent(_)))
}

fn stream_session_record_key(
    record: &StreamSessionRecordV1,
) -> Option<&golem_common::base_model::durable_stream::StreamSessionKeyV1> {
    match record {
        StreamSessionRecordV1::CallerAttempt(record) => Some(&record.session_key),
        StreamSessionRecordV1::Prepared(record) => Some(&record.attempt.session_key),
        StreamSessionRecordV1::Attached(record) => Some(&record.session_key),
        StreamSessionRecordV1::ResumeAttempt(record) => Some(&record.attempt.session_key),
        StreamSessionRecordV1::Detached(record) => Some(&record.session_key),
        StreamSessionRecordV1::Mapping(record) => Some(&record.session_key),
        StreamSessionRecordV1::AttachmentPrepared(record) => Some(&record.key.session_key),
        StreamSessionRecordV1::AttachmentActivated(record) => Some(&record.key.session_key),
        StreamSessionRecordV1::AttachmentRenewed(record) => Some(&record.key.session_key),
        StreamSessionRecordV1::AttachmentFinalized(record) => Some(&record.key.session_key),
        StreamSessionRecordV1::CascadeOutbox(record) => Some(&record.key.session_key),
        StreamSessionRecordV1::SourceUnavailable(record) => Some(&record.key.session_key),
        StreamSessionRecordV1::TopologyPrepared(record) => Some(&record.session_key),
        StreamSessionRecordV1::TopologyActivated(record) => Some(&record.session_key),
        StreamSessionRecordV1::InputHighWater(record) => Some(&record.session_key),
        StreamSessionRecordV1::ConsumerItemValue(record) => Some(&record.session_key),
        StreamSessionRecordV1::ConsumerCancelIntent(record) => Some(&record.session_key),
        StreamSessionRecordV1::ConsumerTerminal(record) => Some(&record.session_key),
        StreamSessionRecordV1::InvocationResult(record) => Some(&record.session_key),
        StreamSessionRecordV1::Finished(record) => Some(&record.session_key),
        StreamSessionRecordV1::ProducerDeleting(_) | StreamSessionRecordV1::ConsumerDeleting(_) => {
            None
        }
    }
}

fn validate_stream_session_record(
    record: &StreamSessionRecordV1,
) -> Result<(), WorkerExecutorError> {
    if record.has_supported_format() {
        Ok(())
    } else {
        Err(WorkerExecutorError::runtime(
            "unsupported or malformed durable Stream Session record version",
        ))
    }
}

fn replace_agent_method_input(
    invocation: AgentInvocation,
    replacement: golem_common::schema::SchemaValue,
) -> AgentInvocation {
    match invocation {
        AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            invocation_context,
            principal,
            scope_card,
            ..
        } => AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input: replacement,
            invocation_context,
            principal,
            scope_card,
        },
        other => other,
    }
}

#[allow(clippy::large_enum_variant)]
pub enum ResultOrSubscription {
    Finished(Result<AgentInvocationOutput, WorkerExecutorError>),
    Pending(EventsSubscription),
}

struct GetOrCreateWorkerResult {
    initial_worker_metadata: AgentMetadata,
    current_status: Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
    /// The status value currently persisted in the live cache, used as the first delta baseline.
    persisted_status: Option<AgentStatusRecord>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    agent_id: Option<ParsedAgentId>,
    snapshot_policy: SnapshotPolicy,
    oplog: Arc<dyn Oplog>,
    /// Loaded during `get_or_create_worker_metadata` and stored on the
    /// [`Worker`] so the read-only cache can resolve metadata without a new
    /// `component_service` lookup.
    initial_component: Arc<golem_service_base::model::component::Component>,
    /// Ephemeral agents are fail-stop: reconstructing one from its lower oplog is allowed for
    /// observation and invocation-result lookup, but the instance must never be started again.
    reconstructed_ephemeral: bool,
}

pub(crate) const INACTIVE_EPHEMERAL_AGENT_ERROR: &str =
    "An ephemeral agent cannot accept another invocation or be resumed";

fn inactive_ephemeral_agent_error() -> WorkerExecutorError {
    WorkerExecutorError::invalid_request(INACTIVE_EPHEMERAL_AGENT_ERROR)
}

fn validate_resolved_invocation_identity(
    agent_mode: AgentMode,
    phantom_id: Option<Uuid>,
    idempotency_key: &IdempotencyKey,
    freshness_disposition: InvocationFreshnessDisposition,
) -> Result<(), WorkerExecutorError> {
    if agent_mode == AgentMode::Ephemeral && phantom_id.is_none() {
        return Err(WorkerExecutorError::invalid_request(
            "An ephemeral invocation requires a final phantom agent ID",
        ));
    }

    if freshness_disposition == InvocationFreshnessDisposition::MayExist {
        return Ok(());
    }

    if agent_mode != AgentMode::Ephemeral {
        return Err(WorkerExecutorError::invalid_request(
            "KnownFresh can only be used for an ephemeral agent invocation",
        ));
    }

    let expected_phantom_id = ephemeral_invocation_phantom_id(idempotency_key);
    if phantom_id != Some(expected_phantom_id) {
        return Err(WorkerExecutorError::invalid_request(
            "KnownFresh ephemeral agent id does not match the invocation idempotency key",
        ));
    }

    Ok(())
}

#[derive(Debug)]
enum EphemeralInvocationState {
    Available,
    Accepted(Option<IdempotencyKey>),
}

impl EphemeralInvocationState {
    fn accept(&mut self, idempotency_key: &IdempotencyKey) -> Result<(), WorkerExecutorError> {
        match self {
            Self::Available => {
                *self = Self::Accepted(Some(idempotency_key.clone()));
                Ok(())
            }
            Self::Accepted(Some(accepted_key)) if accepted_key == idempotency_key => Ok(()),
            Self::Accepted(_) => Err(inactive_ephemeral_agent_error()),
        }
    }
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
