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
pub mod invocation;
mod invocation_loop;
pub mod read_only_cache;
pub mod status;
pub mod status_checkpointer;
pub mod status_flusher;

use self::agent_config::{
    ensure_required_agent_secrets_are_configured, parse_worker_creation_agent_config,
};
use self::status::update_status_with_new_entries;
use crate::durable_host::recover_stderr_logs;
use crate::metrics::storage::record_filesystem_pool_released;
use crate::model::{AgentConfig, ExecutionStatus, LookupResult, ReadFileResult, TrapType};
use crate::services::active_workers::{
    FilesystemStoragePermit, RegisteredConcurrentAccount, WorkerMemoryPermit,
};
use crate::services::events::{Event, EventsSubscription};
use crate::services::golem_config::SnapshotPolicy;
use crate::services::oplog::plugin::ForwardingOplog;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, downcast_oplog};
use crate::services::worker::GetWorkerMetadataResult;
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{
    All, HasActiveWorkers, HasAgentTypesService, HasAgentWebhooksService, HasAll,
    HasBlobStoreService, HasComponentService, HasConfig, HasEnvironmentStateService, HasEvents,
    HasExtraDeps, HasFileLoader, HasHttpConnectionPool, HasKeyValueService, HasOplog,
    HasOplogService, HasPromiseService, HasQuotaService, HasRdbmsService, HasResourceLimits,
    HasRpc, HasSchedulerService, HasShardService, HasWasmtimeEngine, HasWebSocketConnectionPool,
    HasWorkerEnumerationService, HasWorkerForkService, HasWorkerProxy, HasWorkerService,
    UsesAllDeps,
};
use crate::worker::invocation_loop::InvocationLoop;
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::Utc;
use futures::FutureExt;
use futures::channel::oneshot;
use golem_common::base_model::agent::CachePolicy;
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::cache::SimpleCache;
use golem_common::model::AgentStatus;
use golem_common::model::RetryConfig;
use golem_common::model::agent::{
    AgentMode, ParsedAgentId, Principal, Snapshotting, SnapshottingConfig,
};
use golem_common::model::component::CanonicalFilePath;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegionsBuilder, OplogRegion};
use golem_common::model::worker::{AgentConfigEntryDto, RevertWorkerTarget};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult,
    AgentMetadata, AgentStatusRecord, IdempotencyKey, OwnedAgentId, PendingInvocationRef,
    PendingUpdateKind, PendingUpdateRef, ScheduledAction, Timestamp, TimestampedAgentInvocation,
};
use golem_common::one_shot::OneShotEvent;
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::GetFileSystemNodeResult;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, MutexGuard, RwLock};
use tokio::task::JoinHandle;
use tracing::{Instrument, Level, Span, debug, info, span, warn};
use uuid::Uuid;
use wasmtime::component::Instance;
use wasmtime::{Store, UpdateDeadline};

/// Resolved read-only `AgentMethod` invocation data needed to build the
/// cache key and entry.
#[derive(Clone)]
struct ReadOnlyContext {
    method_name: String,
    input: golem_common::schema::SchemaValue,
    principal: Principal,
    cfg: golem_common::base_model::agent::ReadOnlyConfig,
    component_revision: ComponentRevision,
}

/// `Ttl(0)` is folded in as it is equivalent to `NoCache`.
fn is_no_cache(policy: &CachePolicy) -> bool {
    match policy {
        CachePolicy::NoCache(_) => true,
        CachePolicy::Ttl(ttl) => ttl.duration_nanos == 0,
        CachePolicy::UntilWrite(_) => false,
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
    external_invocation_spans: Arc<RwLock<HashMap<IdempotencyKey, Span>>>,

    invocation_results: Arc<RwLock<HashMap<IdempotencyKey, InvocationResult>>>,
    initial_worker_metadata: AgentMetadata,
    registered_concurrent_account: RegisteredConcurrentAccount,
    last_known_status: Arc<RwLock<AgentStatusRecord>>,
    metrics_status: WorkerStatusMetric,
    last_known_status_detached: Arc<AtomicBool>,
    status_flusher: Arc<status_flusher::AgentStatusFlusher>,
    status_checkpointer: status_checkpointer::StatusCheckpointer,
    // Note: std lock for wasmtime reasons
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    update_state_lock: Mutex<()>,
    worker_estimate_coefficient: f64,

    // IMPORTANT: Every external operation must acquire the instance lock, even briefly, to confirm the worker isn’t deleting.
    instance: Arc<Mutex<WorkerInstance>>,
    oom_retry_config: RetryConfig,
    snapshot_policy: SnapshotPolicy,

    last_resume_request: Mutex<Timestamp>,
    pub(crate) snapshot_recovery_disabled: AtomicBool,
    /// Bytes that triggered the last `NodeOutOfFilesystemStorage` trap. Set by
    /// `acquire_filesystem_space` on failure so `WaitingWorker::new` can request
    /// at least that many bytes from the blocking eviction path, ensuring
    /// enough idle workers are evicted to satisfy the pending write.
    desired_extra_filesystem_storage: AtomicU64,

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
        deps.active_workers()
            .get_or_add(
                deps,
                owned_agent_id,
                worker_env,
                worker_agent_config,
                component_revision,
                parent,
                invocation_context_stack,
                principal,
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
        let worker = Self::get_or_create_suspended(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
        )
        .await?;
        Self::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    pub async fn get_latest_metadata<T: HasAll<Ctx>>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentMetadata> {
        if let Some(worker) = deps.active_workers().try_get(owned_agent_id).await {
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
        owned_agent_id: OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Self, WorkerExecutorError> {
        let start = std::time::Instant::now();
        let GetOrCreateWorkerResult {
            initial_worker_metadata,
            current_status,
            execution_status,
            agent_id,
            snapshot_policy,
            oplog,
            initial_component,
        } = match Self::get_or_create_worker_metadata(
            deps,
            &owned_agent_id,
            component_revision,
            worker_env,
            worker_agent_config,
            parent,
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                crate::metrics::wasm::record_create_worker_failure(&err);
                return Err(err);
            }
        };

        let current_status_guard = current_status.read().await;
        let metrics_status = WorkerStatusMetric::new(current_status_guard.status);
        let initial_pending_invocations = current_status_guard.pending_invocations.clone();
        let initial_invocation_results = current_status_guard.invocation_results.clone();
        let last_oplog_idx = current_status_guard.oplog_idx;
        drop(current_status_guard);

        let mut spans_map = HashMap::new();
        for inv in initial_pending_invocations {
            if let Some(idempotency_key) = inv.idempotency_key() {
                spans_map.insert(idempotency_key.clone(), Span::current());
            }
        }

        let queue = Arc::new(RwLock::new(VecDeque::new()));
        let external_invocation_spans = Arc::new(RwLock::new(spans_map));

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
            startup_failure: None,
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
            .active_workers()
            .register_account_concurrency(owner_account_id, resource_entry)
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
            deps.active_workers().status_flush_queue(),
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

        let worker = Worker {
            owned_agent_id,
            parsed_agent_id: agent_id.clone(),
            oplog,
            worker_event_service: Arc::new(WorkerEventServiceDefault::new(
                deps.config().limits.event_broadcast_capacity,
                deps.config().limits.event_history_size,
            )),
            deps: All::from_other(deps),
            queue,
            external_invocation_spans,
            invocation_results,
            instance,
            execution_status,
            initial_worker_metadata,
            registered_concurrent_account,
            last_known_status: current_status,
            metrics_status,
            worker_estimate_coefficient: deps.config().memory.worker_estimate_coefficient,
            oom_retry_config: deps.config().memory.oom_retry_config.clone(),
            snapshot_policy,
            update_state_lock: Mutex::new(()),
            last_known_status_detached,
            status_flusher,
            status_checkpointer,
            last_resume_request: Mutex::new(Timestamp::now_utc()),
            snapshot_recovery_disabled: AtomicBool::new(false),
            desired_extra_filesystem_storage: AtomicU64::new(0),
            current_component,
            read_only_cache,
            read_only_cache_epoch: Arc::new(AtomicU64::new(0)),
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
        {
            let init_idempotency_key = IdempotencyKey::new(format!("init-{}", worker.agent_id()));
            let init_input = agent_id.parameters.clone().into_parts().1;
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
        crate::metrics::wasm::record_create_worker(start.elapsed());

        Ok(worker)
    }

    pub fn agent_id(&self) -> AgentId {
        self.owned_agent_id.agent_id()
    }

    pub fn oom_retry_config(&self) -> &RetryConfig {
        &self.oom_retry_config
    }

    pub(crate) fn snapshot_policy(&self) -> &SnapshotPolicy {
        &self.snapshot_policy
    }

    pub async fn start_if_needed(this: Arc<Worker<Ctx>>) -> Result<bool, WorkerExecutorError> {
        Self::start_if_needed_internal(this, 0).await
    }

    async fn start_if_needed_internal(
        this: Arc<Worker<Ctx>>,
        oom_retry_count: u32,
    ) -> Result<bool, WorkerExecutorError> {
        {
            *this.last_resume_request.lock().await = Timestamp::now_utc();
        }

        let mut instance_guard = this.lock_non_stopping_worker().await;
        match &*instance_guard {
            WorkerInstance::Unloaded { .. } => {
                this.mark_as_loading();
                crate::metrics::workers::inc_worker_waiting_for_memory();
                *instance_guard = WorkerInstance::WaitingForPermit(WaitingWorker::new(
                    this.clone(),
                    this.memory_requirement().await?,
                    this.filesystem_storage_requirement().await?,
                    oom_retry_count,
                ));
                Ok(true)
            }
            WorkerInstance::WaitingForPermit(_) | WorkerInstance::Running(_) => Ok(false),
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
        let mut instance_guard = self.lock_non_stopping_worker().await;
        let stop_result = match &*instance_guard {
            WorkerInstance::Running(running) => {
                if self.is_running_worker_idle(running).await {
                    let stop_result = self
                        .stop_internal_locked(
                            &mut instance_guard,
                            false,
                            None,
                            FinalWorkerState::Unloaded {
                                startup_failure: None,
                            },
                        )
                        .await;

                    Some(stop_result)
                } else {
                    None
                }
            }
            WorkerInstance::WaitingForPermit(_) => None,
            WorkerInstance::Stopping(_) => None,
            WorkerInstance::Unloaded { .. } => None,
            WorkerInstance::Deleting => None,
        };

        drop(instance_guard);

        if let Some(stop_result) = stop_result {
            self.handle_stop_result(stop_result).await;
            true
        } else {
            false
        }
    }

    /// Transition the worker into a deleting state.
    /// Rejects all new invocations and stops any running execution.
    pub async fn start_deleting(&self) -> Result<(), WorkerExecutorError> {
        // Stop any future background flush or clean-checkpoint write from resurrecting the cached
        // status after the upcoming `WorkerService::remove`/`remove_cached_status` deletes it (the
        // latter clears both the live cache and the checkpoint). Each awaits any in-flight write so
        // none can land after the delete.
        self.status_flusher.begin_delete().await;
        self.status_checkpointer.begin_delete().await;
        let error = WorkerExecutorError::invalid_request("Worker is being deleted");
        self.stop_internal(false, Some(error), FinalWorkerState::Deleting)
            .await;
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

    fn mark_as_loading(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        *execution_status = ExecutionStatus::Loading {
            agent_mode: execution_status.agent_mode(),
            timestamp: Timestamp::now_utc(),
        };
    }

    pub fn get_initial_worker_metadata(&self) -> AgentMetadata {
        self.initial_worker_metadata.clone()
    }

    pub async fn get_latest_worker_metadata(&self) -> AgentMetadata {
        let updated_status = self.last_known_status.read().await.clone();
        let result = self.get_initial_worker_metadata();
        AgentMetadata {
            last_known_status: updated_status,
            ..result
        }
    }

    pub async fn get_last_known_status(&self) -> AgentStatusRecord {
        self.last_known_status.read().await.clone()
    }

    // Outside of reverts and updates, this will return the same status as get_latest_worker_metadata.
    // This just has an additional assert built in for when decisions need to be sure that they are fully up to date on the oplog.
    // _NEVER_ call this from outside the invocation loop, as that is the only place that can reason about whether the status is detached or not.
    pub async fn get_non_detached_last_known_status(&self) -> AgentStatusRecord {
        // hold the update lock so we know that the atomic bool and state are consistent
        let update_state_lock_guard = self.update_state_lock.lock().await;

        let is_detached = self.last_known_status_detached.load(Ordering::Relaxed);
        assert!(!is_detached);
        let result = self.last_known_status.read().await.clone();

        // ensure we hold mutex for the full duration
        drop(update_state_lock_guard);
        result
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
        if let WorkerInstance::Running(running) = &*self.lock_non_stopping_worker().await {
            running.interrupt(interrupt_kind).await;
        }

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
                await_interruption, ..
            } => {
                let receiver = await_interruption.subscribe();
                Some(receiver)
            }
            ExecutionStatus::Loading { .. } => None,
        }
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
        })
    }

    /// Invocation entry point. Returns `Finished(...)` on read-only cache hit,
    /// otherwise `Pending(subscription)`. `Arc<Self>` is needed to spawn the
    /// detached observer that fills the read-only cache on completion.
    pub async fn invoke(
        self: Arc<Self>,
        invocation: AgentInvocation,
    ) -> Result<ResultOrSubscription, WorkerExecutorError> {
        let idempotency_key = invocation
            .idempotency_key()
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request("Invocation has no idempotency key")
            })?
            .clone();

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
            let no_cache = is_no_cache(&ro.cfg.cache_policy);
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
            if is_no_cache(&ro.cfg.cache_policy) {
                None
            } else {
                Some(self.events().subscribe())
            }
        } else {
            None
        };

        let output = async { self.lookup_invocation_result(&idempotency_key).await }
            .instrument(span!(Level::INFO, "lookup_invocation_result"))
            .await;
        let (result, enqueue_epoch) = match output {
            LookupResult::Complete(output) => (ResultOrSubscription::Finished(output), None),
            LookupResult::Interrupted => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            LookupResult::Pending => (ResultOrSubscription::Pending(subscription), None),
            LookupResult::New => {
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
        let idempotency_key = invocation
            .idempotency_key()
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request("Invocation has no idempotency key")
            })?
            .clone();

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
            && !is_no_cache(&ro.cfg.cache_policy)
        {
            Some((ro, self.lookup_invocation_result(&idempotency_key).await))
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
            let entry_result = self
                .read_only_cache
                .get_or_insert_simple_spawned(&key, move || async move {
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
                })
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
        match self.clone().invoke(invocation).await? {
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

                let result = async {
                    self.wait_for_invocation_result(&idempotency_key, subscription)
                        .await
                }
                .instrument(span!(Level::INFO, "wait_for_invocation_result"))
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

    /// Enqueue attempting an update.
    ///
    /// The update itself is not performed by the invocation queue's processing loop,
    /// it is going to affect how the worker is recovered next time.
    pub async fn enqueue_update(&self, update_description: UpdateDescription) {
        // Bump + commit under the same instance lock.
        let instance_guard = self.lock_non_stopping_worker().await;
        self.bump_read_only_cache_epoch();
        let entry = OplogEntry::pending_update(update_description.clone());
        self.add_and_commit_oplog_internal(&instance_guard, entry)
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
        self.last_known_status
            .read()
            .await
            .pending_invocations
            .clone()
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
        self.last_known_status
            .read()
            .await
            .invocation_results
            .clone()
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
        debug!("Stored invocation success for {key}");
        self.events().publish(Event::InvocationCompleted {
            agent_id: self.owned_agent_id.agent_id(),
            idempotency_key: key.clone(),
            result: Ok(output),
        });
    }

    // should only be called from invocation loop
    pub async fn store_invocation_failure(&self, key: &IdempotencyKey, trap_type: &TrapType) {
        let pending = self.pending_invocations().await;
        let keys_to_fail = [
            vec![key],
            pending
                .iter()
                .filter_map(|entry| entry.idempotency_key())
                .collect(),
        ]
        .concat();
        let mut map = self.invocation_results.write().await;
        for key in keys_to_fail {
            let stderr = self.worker_event_service.get_last_invocation_errors();
            map.insert(
                key.clone(),
                InvocationResult::Cached {
                    result: Err(FailedInvocationResult {
                        trap_type: trap_type.clone(),
                        stderr: stderr.clone(),
                    }),
                },
            );
            let golem_error = trap_type.as_golem_error(&stderr);
            if let Some(golem_error) = golem_error {
                self.events().publish(Event::InvocationCompleted {
                    agent_id: self.owned_agent_id.agent_id(),
                    idempotency_key: key.clone(),
                    result: Err(golem_error),
                });
            }
        }
    }

    pub(super) async fn store_invocation_resuming(&self, key: &IdempotencyKey) {
        let mut map = self.invocation_results.write().await;
        map.remove(key);
    }

    pub fn agent_mode(&self) -> AgentMode {
        self.execution_status.read().unwrap().agent_mode()
    }

    /// Gets the estimated memory requirement of the worker
    pub async fn memory_requirement(&self) -> Result<u64, WorkerExecutorError> {
        let metadata = self.get_latest_worker_metadata().await;

        let ml = metadata.last_known_status.total_linear_memory_size as f64;
        let sw = metadata.last_known_status.component_size as f64;
        let c = 2.0;
        let x = self.worker_estimate_coefficient;
        Ok((x * (ml + c * sw)) as u64)
    }

    /// Gets the storage requirement of the worker based on the last known status.
    /// Used by `WaitingWorker::new` to pre-acquire storage semaphore permits.
    pub async fn filesystem_storage_requirement(&self) -> Result<u64, WorkerExecutorError> {
        let metadata = self.get_latest_worker_metadata().await;
        Ok(metadata.last_known_status.current_filesystem_storage_usage)
    }

    /// Returns true if the worker is running, but it is not performing any invocations at the moment
    /// (ExecutionStatus::Suspended) and has no pending work that should keep the
    /// loaded worker resident while memory and filesystem pressure is low.
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
        let has_interrupt = running.interrupt_signal.lock().await.is_some();

        debug!(
            "Worker {} idle check: waiting_for_command={waiting_for_command} has_pending_invocations={has_pending_invocations} has_queued_internal_work={has_queued_internal_work} has_resume_replay={has_resume_replay} has_interrupt={has_interrupt}",
            self.owned_agent_id
        );

        waiting_for_command
            && !has_pending_invocations
            && !has_queued_internal_work
            && !has_resume_replay
            && !has_interrupt
    }

    /// Returns `true` iff this worker currently has a loaded wasmtime instance
    /// (i.e. its [`WorkerInstance`] is in the `Running` state).
    ///
    /// `Worker` shells can outlive their wasmtime instance — for example after
    /// memory-pressure eviction unloads the instance but the shell stays alive
    /// in [`ActiveWorkers`] so its caches (read-only cache, pending
    /// invocations, …) can keep serving. This accessor lets callers
    /// distinguish those two states.
    pub async fn is_loaded(&self) -> bool {
        matches!(&*self.instance.lock().await, WorkerInstance::Running(_))
    }

    /// Classifies the worker for eviction ordering under memory/filesystem
    /// pressure. Returns `None` if the worker is not evictable.
    ///
    /// - `LoadedIdle`: resident in memory, not executing, no durable pending work.
    ///   Evicted first.
    /// - `WarmRunnable`: resident in memory, not executing, has durable pending
    ///   invocations. Evicted only when `LoadedIdle` workers are exhausted.
    /// - `None`: worker is actively executing, has non-durable in-memory work
    ///   pending, or is not loaded. Never evicted.
    pub async fn eviction_class(&self) -> Option<EvictionClass> {
        match &*self.instance.lock().await {
            WorkerInstance::Running(running) => {
                let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                let has_queued_internal_work = !running.queue.read().await.is_empty();
                let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
                let has_interrupt = running.interrupt_signal.lock().await.is_some();

                // Non-evictable if actively executing or has non-durable in-memory work
                if !waiting_for_command
                    || has_queued_internal_work
                    || has_resume_replay
                    || has_interrupt
                {
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
        let mut instance_guard = self.lock_non_stopping_worker().await;
        let should_stop = match &*instance_guard {
            WorkerInstance::Running(running) => {
                let waiting_for_command = running.waiting_for_command.load(Ordering::Acquire);
                let has_queued_internal_work = !running.queue.read().await.is_empty();
                let has_resume_replay = running.resume_replay_pending.load(Ordering::Acquire);
                let has_interrupt = running.interrupt_signal.lock().await.is_some();

                if !waiting_for_command
                    || has_queued_internal_work
                    || has_resume_replay
                    || has_interrupt
                {
                    false
                } else {
                    let has_pending_invocations = !self.pending_invocations().await.is_empty();
                    let current_class = if has_pending_invocations {
                        EvictionClass::WarmRunnable
                    } else {
                        EvictionClass::LoadedIdle
                    };
                    current_class.eviction_priority() <= target_class.eviction_priority()
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
                    FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                )
                .await;
            drop(instance_guard);
            self.handle_stop_result(stop_result).await;
            true
        } else {
            drop(instance_guard);
            false
        }
    }

    /// Gets the timestamp of the last time the execution status changed
    pub fn last_execution_state_change(&self) -> Timestamp {
        self.execution_status.read().unwrap().timestamp()
    }

    // Should only be called from invocation loop
    pub async fn increase_memory(&self, delta: u64) -> anyhow::Result<()> {
        match &mut *self.instance.lock().await {
            WorkerInstance::Running(running) => {
                if let Some(new_permits) = self.active_workers().try_acquire(delta).await {
                    running.merge_extra_permits(new_permits);
                    Ok(())
                } else {
                    Err(anyhow!(GolemSpecificWasmTrap::WorkerOutOfMemory))
                }
            }
            WorkerInstance::Stopping(_) => Ok(()),
            WorkerInstance::WaitingForPermit(_) => Ok(()),
            WorkerInstance::Unloaded { .. } => Ok(()),
            WorkerInstance::Deleting => Ok(()),
        }
    }

    /// Return `freed_bytes` to the storage semaphore pool.
    /// Called from `DurableWorkerCtx::release_filesystem_space` when a file is
    /// deleted or truncated. Should only be called from the invocation loop.
    ///
    /// The permits are returned by splitting them off `RunningWorker.filesystem_storage_permit`
    /// and dropping the split portion. This correctly reduces the permit count held
    /// by the `RunningWorker`, preventing double-return when it later drops.
    pub async fn release_filesystem_storage_space(&self, freed_bytes: u64) {
        let permits_to_release =
            crate::services::active_workers::bytes_to_filesystem_storage_permits(freed_bytes);
        if permits_to_release == 0 {
            return;
        }
        if let WorkerInstance::Running(running) = &mut *self.instance.lock().await
            && let Some(ref mut permit) = running.filesystem_storage_permit
        {
            // Split off `permits_to_release` permits and drop them.
            // Dropping the split permit returns its permits to the semaphore
            // automatically — no separate add_permits needed.
            let n = permits_to_release as usize;
            let actual_n = n.min(permit.num_permits());
            let to_drop = permit.split(actual_n);
            let released_bytes =
                crate::services::active_workers::filesystem_storage_permits_to_bytes(
                    actual_n as u32,
                );
            record_filesystem_pool_released(released_bytes);
            drop(to_drop); // returns permits to the semaphore
        }
    }

    /// Acquire storage semaphore permits for a write operation.
    /// Called from `DurableWorkerCtx::acquire_filesystem_space` in live mode only.
    /// Returns `NodeOutOfFilesystemStorage` if the executor pool is exhausted.
    ///
    /// Should only be called from the invocation loop.
    pub async fn acquire_filesystem_storage_space(&self, new_bytes: u64) -> anyhow::Result<()> {
        match &mut *self.instance.lock().await {
            WorkerInstance::Running(running) => {
                if let Some(permit) = self
                    .active_workers()
                    .try_acquire_filesystem_storage(new_bytes)
                    .await
                {
                    running.merge_extra_filesystem_storage_permits(permit);
                    // Success — clear any pending desired_extra_filesystem_storage.
                    self.desired_extra_filesystem_storage
                        .store(0, Ordering::Relaxed);
                    Ok(())
                } else {
                    // Record the requested size so WaitingWorker can evict enough
                    // idle workers to satisfy this write on the next restart.
                    self.desired_extra_filesystem_storage
                        .store(new_bytes, Ordering::Relaxed);
                    Err(anyhow!(GolemSpecificWasmTrap::NodeOutOfFilesystemStorage))
                }
            }
            // Worker is stopping/unloaded — no-op; the current invocation will
            // fail anyway and permits will be re-acquired on restart.
            _ => Ok(()),
        }
    }

    /// Acquire storage semaphore permits for the total size of all initial
    /// component files. Called once from `DurableWorkerCtx::create` after
    /// `prepare_filesystem` has loaded the files. Merges the acquired permits
    /// into the running worker's `filesystem_storage_permit` so they are released
    /// automatically when the worker stops.
    ///
    /// Uses the non-blocking priority path (`try_acquire_storage`). If the
    /// semaphore pool is full, idle workers are evicted by the semaphore's own
    /// logic; the permit is returned as `None` and the caller should propagate
    /// a retriable `NodeOutOfFilesystemStorage` error.
    ///
    /// Should only be called from the invocation loop.
    pub async fn acquire_initial_filesystem_storage(
        &self,
        total_bytes: u64,
    ) -> Result<(), GolemSpecificWasmTrap> {
        if total_bytes == 0 {
            return Ok(());
        }
        match &mut *self.instance.lock().await {
            WorkerInstance::Running(running) => {
                if let Some(permit) = self
                    .active_workers()
                    .try_acquire_filesystem_storage(total_bytes)
                    .await
                {
                    running.merge_extra_filesystem_storage_permits(permit);
                    Ok(())
                } else {
                    Err(GolemSpecificWasmTrap::NodeOutOfFilesystemStorage)
                }
            }
            // Worker stopped between create and acquire — no-op, permits will be
            // re-acquired on next startup from AgentStatusRecord.
            _ => Ok(()),
        }
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
        async {
            let instance_guard = self.lock_non_stopping_worker().await;

            if instance_guard.is_deleting() {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot enqueue invocation to a deleting worker",
                ));
            };

            if let Some(err) = instance_guard.startup_failure() {
                return Err(err.clone());
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

            self.add_and_commit_oplog_internal(&instance_guard, entry)
                .await;

            if let Some(idempotency_key) = timestamped_invocation.invocation.idempotency_key() {
                self.external_invocation_spans
                    .write()
                    .await
                    .insert(idempotency_key.clone(), Span::current());
            }

            if let WorkerInstance::Running(running) = &*instance_guard {
                running.sender.send(WorkerCommand::Unblock).unwrap();
            };

            drop(instance_guard);

            Ok(read_only_epoch_snapshot)
        }
        .instrument(span!(Level::INFO, "enqueue_invocation"))
        .await
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
            running.sender.send(WorkerCommand::Unblock).unwrap();
        };

        drop(instance_guard);

        receiver.await.unwrap()
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
            running.sender.send(WorkerCommand::Unblock).unwrap();
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

        let (sender, receiver) = oneshot::channel();

        self.queue
            .write()
            .await
            .push_back(QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender });

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::Unblock).unwrap();
        };

        drop(instance_guard);

        receiver.await.unwrap()
    }

    // Should only be called from invocation loop
    pub async fn add_to_oplog(&self, entry: OplogEntry) -> OplogIndex {
        self.oplog.add(entry).await
    }

    pub async fn commit_oplog_and_update_state(&self, commit_level: CommitLevel) -> OplogIndex {
        let (result, changed) = self
            .commit_oplog_and_update_state_internal(commit_level)
            .await;
        if changed {
            let instance_guard = self.instance.lock().await;
            if let WorkerInstance::Running(running) = &*instance_guard {
                running.sender.send(WorkerCommand::Unblock).unwrap();
            };
        }
        result
    }

    // Should only be called from invocation loop
    async fn commit_oplog_and_update_state_internal(
        &self,
        commit_level: CommitLevel,
    ) -> (OplogIndex, bool) {
        let update_state_lock_guard = self.update_state_lock.lock().await;

        let changed = self
            .commit_and_update_state_inner(&update_state_lock_guard, commit_level)
            .await;
        let new_index = self.oplog.current_oplog_index().await;

        // ensure we hold mutex for the full duration
        drop(update_state_lock_guard);
        (new_index, changed)
    }

    // Should only be called from invocation loop
    pub async fn add_and_commit_oplog(&self, entry: OplogEntry) -> OplogIndex {
        let result = self.add_to_oplog(entry).await;
        self.commit_oplog_and_update_state(CommitLevel::Always)
            .await;
        result
    }

    async fn add_and_commit_oplog_internal(
        &self,
        instance_guard: &MutexGuard<'_, WorkerInstance>,
        entry: OplogEntry,
    ) -> OplogIndex {
        let result = self.add_to_oplog(entry).await;
        let (_, changed) = self
            .commit_oplog_and_update_state_internal(CommitLevel::Always)
            .await;

        if changed && let WorkerInstance::Running(running) = &**instance_guard {
            running.sender.send(WorkerCommand::Unblock).unwrap();
        };

        result
    }

    pub async fn activate_plugin(
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
        )
        .await;

        drop(instance_guard);
        Ok(())
    }

    pub async fn deactivate_plugin(
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
    pub async fn revert(&self, target: RevertWorkerTarget) -> Result<(), WorkerExecutorError> {
        match target {
            RevertWorkerTarget::RevertToOplogIndex(target) => {
                self.revert_to_last_oplog_index(target.last_oplog_index)
                    .await
            }
            RevertWorkerTarget::RevertLastInvocations(target) => {
                if let Some(last_oplog_index) = self
                    .find_nth_invocation_from_end(target.number_of_invocations as usize)
                    .await
                {
                    self.revert_to_last_oplog_index(last_oplog_index.previous())
                        .await
                } else {
                    Err(WorkerExecutorError::invalid_request(format!(
                        "Could not find {} invocations to revert",
                        target.number_of_invocations
                    )))
                }
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
        )
        .await;

        drop(instance_guard);
        Ok(())
    }

    /// Starting from the end of the oplog, find the Nth AgentInvocationStarted entry's index.
    async fn find_nth_invocation_from_end(&self, n: usize) -> Option<OplogIndex> {
        let mut current = self.oplog.current_oplog_index().await;
        let mut found = 0;
        loop {
            let entry = self.oplog.read(current).await;

            if matches!(entry, OplogEntry::AgentInvocationStarted { .. }) {
                found += 1;
                if found == n {
                    return Some(current);
                }
            }

            if current == OplogIndex::INITIAL {
                return None;
            } else {
                current = current.previous();
            }
        }
    }

    async fn revert_to_last_oplog_index(
        &self,
        last_oplog_index: OplogIndex,
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
        let region_start = last_oplog_index.next();
        let last_known_status = self.get_latest_worker_metadata().await.last_known_status;

        if last_known_status
            .skipped_regions
            .is_in_deleted_region(region_start)
        {
            Err(WorkerExecutorError::invalid_request(format!(
                "Attempted to revert to a deleted region in oplog to index {last_oplog_index}"
            )))
        } else {
            let region = OplogRegion {
                start: region_start,
                end: region_end,
            };

            // Revert changes observable state, invalidate cached results.
            self.bump_read_only_cache_epoch();

            // this commit will detach the worker status, immediately reattach it so we see the up to date status.
            self.add_and_commit_oplog_internal(&instance_guard, OplogEntry::revert(region))
                .await;
            self.reattach_worker_status().await;

            if let WorkerInstance::Running(running) = &*instance_guard {
                running.sender.send(WorkerCommand::Unblock).unwrap();
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
        let status = self.last_known_status.read().await.clone();
        let maybe_result = self.invocation_results.read().await.get(key).cloned();
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
        final_state: FinalWorkerState,
    ) {
        let mut instance_guard = self.instance.lock().await;

        let stop_result = self
            .stop_internal_locked(
                &mut instance_guard,
                called_from_invocation_loop,
                fail_pending_invocations,
                final_state,
            )
            .await;

        // IMPORTANT: drop the lock here as the invocation loop might reenter this method after we drop a running worker.
        drop(instance_guard);

        self.handle_stop_result(stop_result).await;
    }

    async fn stop_internal_locked(
        &self,
        instance_guard: &mut MutexGuard<'_, WorkerInstance>,
        called_from_invocation_loop: bool,
        // Only respected when this is the call that triggered the stop
        fail_pending_invocations: Option<WorkerExecutorError>,
        final_state: FinalWorkerState,
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
                StopResult::Stopped
            }
            WorkerInstance::WaitingForPermit(_) => {
                if let Some(ref error) = fail_pending_invocations {
                    self.fail_pending_invocations(error.clone()).await;
                }
                crate::metrics::workers::dec_worker_waiting_for_memory();
                **instance_guard = final_state.into_instance();
                StopResult::Stopped
            }
            WorkerInstance::Deleting => {
                **instance_guard = previous_instance_state;
                // Should we return an error here?
                StopResult::Stopped
            }
            WorkerInstance::Stopping(_) if called_from_invocation_loop => {
                **instance_guard = previous_instance_state;
                StopResult::Stopped
            }
            WorkerInstance::Stopping(mut stopping) => {
                // If we're stopping for deletion, upgrade the final state
                if matches!(final_state, FinalWorkerState::Deleting) {
                    stopping.final_state = FinalWorkerState::Deleting;
                    if let Some(ref error) = fail_pending_invocations {
                        self.fail_pending_invocations(error.clone()).await;
                    }
                }
                let notify = stopping.notify.clone();
                **instance_guard = WorkerInstance::Stopping(stopping);
                StopResult::AlreadyStopping { notify }
            }
            WorkerInstance::Running(running) => {
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
                    **instance_guard = final_state.into_instance();
                    StopResult::Stopped
                } else {
                    // drop the running worker, this signals to the invocation loop to start exiting.
                    // RunningWorker::drop releases the memory permit, so dec resident here.
                    let run_loop_handle = running.stop();
                    let notify = OneShotEvent::new();
                    crate::metrics::workers::dec_worker_memory_resident();
                    **instance_guard = WorkerInstance::Stopping(StoppingWorker {
                        notify: notify.clone(),
                        final_state,
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
                run_loop_handle.await.expect("Failed to join run loop");

                let mut instance_guard = self.instance.lock().await;
                let is_deleting = match &*instance_guard {
                    WorkerInstance::Stopping(stopping) => {
                        matches!(stopping.final_state, FinalWorkerState::Deleting)
                    }
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

                match std::mem::replace(
                    &mut *instance_guard,
                    WorkerInstance::Unloaded {
                        startup_failure: None,
                    },
                ) {
                    WorkerInstance::Stopping(stopping) => {
                        *instance_guard = stopping.final_state.into_instance();
                    }
                    other => panic!("expected Stopping, got {other:?}"),
                }
                drop(instance_guard);

                notify.set();
            }
        }
    }

    async fn fail_pending_invocations(&self, error: WorkerExecutorError) {
        let queued_items = self.queue.write().await.drain(..).collect::<VecDeque<_>>();
        let mut spans_map = self.external_invocation_spans.write().await;

        // Publishing the provided initialization error to all queued internal operations
        for item in queued_items {
            match item {
                QueuedWorkerInvocation::GetFileSystemNode { sender, .. } => {
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

        // Collect all idempotency keys to fail: pending invocations + currently running invocation
        let status = self.last_known_status.read().await.clone();
        let mut keys_to_fail: Vec<IdempotencyKey> = status
            .pending_invocations
            .iter()
            .filter_map(|inv| inv.idempotency_key().cloned())
            .collect();
        if let Some(current_key) = &status.current_idempotency_key
            && !keys_to_fail.contains(current_key)
        {
            keys_to_fail.push(current_key.clone());
        }

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
                            semantic_trap_retry_override: None,
                        },
                        stderr: String::new(),
                    }),
                },
            );
            self.events().publish(Event::InvocationCompleted {
                agent_id: self.owned_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                result: Err(error.clone()),
            });
            // Clean up the span entry
            spans_map.remove(idempotency_key);
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

    // Lock a worker in either Unloaded or Deleting state.
    async fn lock_stopped_worker(&self) -> MutexGuard<'_, WorkerInstance> {
        loop {
            self.stop_internal(
                false,
                None,
                FinalWorkerState::Unloaded {
                    startup_failure: None,
                },
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
    ) -> Result<bool, WorkerExecutorError> {
        this.stop_internal(
            called_from_invocation_loop,
            None,
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
        )
        .await;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Self::start_if_needed_internal(this, oom_retry_count).await
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
    ) -> Result<GetOrCreateWorkerResult, WorkerExecutorError> {
        let component_id = owned_agent_id.component_id();

        // Note: this also checks the oplog for the existence of the create entry, which is the main thing we are interested in here.
        let existing_worker_metadata = this.worker_service().get(owned_agent_id).await;

        match existing_worker_metadata {
            Some(GetWorkerMetadataResult {
                initial_worker_metadata,
                last_known_status,
            }) => {
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

                let current_status = Arc::new(RwLock::new(current_status));

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
                        read_only_lock::tokio::ReadOnlyLock::new(current_status.clone()),
                        read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                    )
                    .await;

                Ok(GetOrCreateWorkerResult {
                    initial_worker_metadata,
                    current_status,
                    execution_status,
                    agent_id,
                    snapshot_policy,
                    oplog,
                    initial_component: Arc::new(initial_component),
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
                    total_linear_memory_size: component
                        .metadata
                        .memories()
                        .iter()
                        .map(|m| m.initial)
                        .sum(),
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
                    initial_worker_metadata
                        .config
                        .iter()
                        .cloned()
                        .map(Into::into)
                        .collect(),
                    initial_worker_metadata.original_phantom_id,
                    instance_id,
                );

                let initial_status = Arc::new(tokio::sync::RwLock::new(initial_status));
                let execution_status = Arc::new(std::sync::RwLock::new(execution_status));

                let oplog = this
                    .oplog_service()
                    .create(
                        owned_agent_id,
                        agent_mode,
                        initial_oplog_entry,
                        initial_worker_metadata.clone(),
                        read_only_lock::tokio::ReadOnlyLock::new(initial_status.clone()),
                        read_only_lock::std::ReadOnlyLock::new(execution_status.clone()),
                    )
                    .await;

                initial_status.write().await.oplog_idx = oplog.current_oplog_index().await;

                // Cold path (worker creation): no previously cached status to diff against.
                let initial_status_value = initial_status.read().await.clone();
                this.worker_service()
                    .update_cached_status(owned_agent_id, None, initial_status_value)
                    .await;

                Ok(GetOrCreateWorkerResult {
                    initial_worker_metadata,
                    current_status: initial_status,
                    execution_status,
                    agent_id,
                    snapshot_policy,
                    oplog,
                    initial_component: Arc::new(component),
                })
            }
        }
    }

    // TODO: should be private, exposed for the invocation loop for now.
    pub async fn reattach_worker_status(&self) {
        let update_state_lock_guard = self.update_state_lock.lock().await;

        self.commit_and_update_state_inner(&update_state_lock_guard, CommitLevel::Always)
            .await;
        if self.last_known_status_detached.load(Ordering::Relaxed) {
            debug!("Worker status was detached from oplog, recomputing it");

            // The in-memory status is no longer foldable (a jump deleted its index, or a revert
            // moved the oplog behind it), so we recompute. Prefer folding forward from the clean
            // checkpoint (which predates any jump region) over a full re-read of the oplog.
            let agent_mode = self.agent_mode();
            let owned_agent_id = &self.owned_agent_id;
            let worker_status =
                calculate_last_known_status_with_checkpoint(self, owned_agent_id, agent_mode, None)
                    .await
                    .expect("Failed to recompute worker status for existing worker");

            // Install the recomputed status while still detached, so a concurrent background sweep
            // keeps skipping (the in-memory status is not authoritative until it is installed).
            self.update_last_known_status(worker_status.clone()).await;

            // Now the in-memory status is authoritative again; clear the flag and force a flush.
            self.last_known_status_detached
                .store(false, Ordering::Relaxed);

            // The status was just recomputed from scratch; persist it synchronously (a full
            // reconcile write, since the baseline was invalidated on detach) so the cache is
            // immediately consistent rather than waiting for the next background sweep. Best-effort:
            // a failure is logged/metered and re-queued inside `flush`.
            if let Err(err) = self
                .status_flusher
                .flush(status_flusher::FlushReason::Forced)
                .await
            {
                debug!("Forced status flush on reattach failed (will retry in background): {err}");
            }

            // ensure we hold mutex for the full duration
            drop(update_state_lock_guard);
        };
    }

    // must be called within a held update_state_lock lock.
    async fn commit_and_update_state_inner(
        &self,
        _update_state_lock_guard: &MutexGuard<'_, ()>,
        commit_level: CommitLevel,
    ) -> bool {
        let new_entries = self.oplog.commit(commit_level).await;

        if !self.last_known_status_detached.load(Ordering::Acquire) {
            let old_status = self.last_known_status.read().await.clone();

            let updated_status = update_status_with_new_entries(
                self.agent_mode(),
                old_status.clone(),
                new_entries,
                &self.config().retry,
            );

            if let Some(updated_status) = updated_status {
                if updated_status != old_status {
                    self.update_last_known_status(updated_status.clone()).await;

                    self.schedule_oplog_archive_if_needed(&old_status, &updated_status)
                        .await;

                    true
                } else {
                    false
                }
            } else {
                // The status can no longer be incrementally computed by adding the new oplog entries, instead a full reload needs to be performed.
                // This can happen during a revert or a snapshot update for example.
                tracing::debug!("Detaching worker_status from oplog");
                self.last_known_status_detached
                    .store(true, Ordering::Release);
                // The in-memory status is no longer authoritative, and after reattach it will be
                // recomputed from scratch, so the persisted baseline can no longer be trusted: the
                // next flush must be a full reconcile write.
                self.status_flusher.invalidate_baseline().await;
                true
            }
        } else {
            false
        }
    }

    async fn schedule_oplog_archive_if_needed(
        &self,
        old_status: &AgentStatusRecord,
        new_status: &AgentStatusRecord,
    ) {
        if old_status.status != new_status.status
            && matches!(
                new_status.status,
                AgentStatus::Idle | AgentStatus::Failed | AgentStatus::Exited
            )
        {
            let archive_interval = self.config().oplog.archive_interval;
            let last_oplog_index = new_status.oplog_idx;
            let account_id = self.initial_worker_metadata.created_by;

            debug!(
                worker_id = %self.owned_agent_id,
                new_status = ?new_status.status,
                "Scheduling ArchiveOplog after status transition"
            );

            self.scheduler_service()
                .schedule(
                    Utc::now() + archive_interval,
                    ScheduledAction::ArchiveOplog {
                        account_id,
                        owned_agent_id: self.owned_agent_id.clone(),
                        agent_mode: self.agent_mode(),
                        last_oplog_index,
                        next_after: archive_interval,
                    },
                )
                .await;
        }
    }

    async fn start_waiting_worker(
        this: Arc<Worker<Ctx>>,
        permit: WorkerMemoryPermit,
        filesystem_storage_permit: Option<FilesystemStoragePermit>,
        concurrent_agent_permit: crate::services::active_workers::ConcurrentAgentPermit,
        oom_retry_count: u32,
        start_attempt: Uuid,
    ) {
        let mut instance_guard = this.instance.lock().await;
        match &*instance_guard {
            WorkerInstance::WaitingForPermit(waiting_worker)
                if waiting_worker.start_attempt == start_attempt =>
            {
                let mut running = RunningWorker::new(
                    this.owned_agent_id.clone(),
                    this.queue.clone(),
                    this.clone(),
                    permit,
                    concurrent_agent_permit,
                    oom_retry_count,
                )
                .await;
                if let Some(sp) = filesystem_storage_permit {
                    running.merge_extra_filesystem_storage_permits(sp);
                }
                crate::metrics::workers::dec_worker_waiting_for_memory();
                crate::metrics::workers::inc_worker_memory_resident();
                *instance_guard = WorkerInstance::Running(running);
            }
            _ => {
                debug!("worker was not waiting for permit anymore, not starting");
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
        let status = self.last_known_status.read().await.clone();
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
        let status = self.last_known_status.read().await.clone();
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

    async fn update_last_known_status(&self, new_status: AgentStatusRecord) {
        let previous_metrics_status = self.metrics_status.status();
        // The in-memory `last_known_status` is the authoritative live status; the flusher reads it
        // when it persists the cached blob. We replace it here and hand the (previous, new) pair to
        // the flusher, which updates the `RunningWorkers` recovery index synchronously and either
        // marks the worker dirty for the background sweeper or writes the blob inline (when
        // background flushing is disabled).
        let previous_status = {
            let mut guard = self.last_known_status.write().await;
            std::mem::replace(&mut *guard, new_status.clone())
        };
        self.metrics_status
            .update(previous_metrics_status, new_status.status);
        self.status_flusher
            .on_status_changed(&previous_status, &new_status)
            .await;
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
            } => Some(err),
            _ => None,
        }
    }
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
        filesystem_storage_requirement: u64,
        oom_retry_count: u32,
    ) -> Self {
        let span = span!(
            parent: None,
            Level::INFO,
            "waiting-for-permits",
            agent_id = parent.owned_agent_id.agent_id.to_string(),
            agent_type = parent
                .parsed_agent_id
                .as_ref()
                .map(|id| id.agent_type.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );
        span.follows_from(Span::current());

        let start_attempt = Uuid::new_v4();

        let handle = tokio::task::spawn(
            async move {
                let agent_id = parent.owned_agent_id.agent_id();
                let registered_concurrent_account = parent.registered_concurrent_account.clone();
                let concurrent_agent_permit = registered_concurrent_account.acquire(agent_id).await;
                // Do not reserve executor memory while waiting for a per-account
                // concurrency slot. Otherwise one account could fill the memory
                // pool with workers that are not allowed to run yet.
                let permit = parent.active_workers().acquire(memory_requirement).await;
                // Pre-acquire storage permits for this restart.
                //
                // We need to acquire `filesystem_storage_requirement + desired_extra` total:
                // - `filesystem_storage_requirement`: bytes to hold as the pre-acquired permit
                //   for replay (mirrors what the worker held before being evicted).
                //   The old RunningWorker already returned these bytes to the pool
                //   when it dropped, so the pool likely already has them — the
                //   blocking acquire will find them without needing to evict anyone.
                // - `desired_extra`: bytes for the write that triggered NodeOutOfFilesystemStorage.
                //   The pool may not have these yet, so the blocking acquire will
                //   evict idle workers only for the missing portion.
                //
                // After acquiring, we release `desired_extra` back to the pool so
                // it is available for the pending write to re-acquire at runtime.
                //
                // Example: prior writes = 3 KB, failing write needs 1 KB extra.
                //   Old RunningWorker drops → 3 KB returned to pool.
                //   acquire_bytes = 4 KB. Pool has 3 KB → 1 KB gap → evict 1 KB.
                //   Hold 3 KB as filesystem_storage_permit, release 1 KB → pool has 1 KB free.
                //   Pending write re-acquires 1 KB → succeeds.
                let desired_extra = parent
                    .desired_extra_filesystem_storage
                    .load(Ordering::Relaxed);
                let acquire_bytes = filesystem_storage_requirement + desired_extra;
                let filesystem_storage_permit = if acquire_bytes > 0 {
                    let mut permit = parent
                        .active_workers()
                        .acquire_filesystem_storage(acquire_bytes)
                        .await;
                    // Release the `desired_extra` portion back to the pool.
                    if desired_extra > 0 {
                        let extra_permits =
                            crate::services::active_workers::bytes_to_filesystem_storage_permits(
                                desired_extra,
                            ) as usize;
                        if let Some(extra) = permit.split(extra_permits) {
                            drop(extra); // returns to semaphore
                        }
                    }
                    if permit.num_permits() > 0 {
                        Some(permit)
                    } else {
                        None
                    }
                } else {
                    None
                };
                debug!("Attempting to start worker after acquiring enough permits");
                Worker::start_waiting_worker(
                    parent,
                    permit,
                    filesystem_storage_permit,
                    concurrent_agent_permit,
                    oom_retry_count,
                    start_attempt,
                )
                .await;
                // If we do not start the worker here we will drop the permits here, which will release them to the host.
            }
            .instrument(span),
        );

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

#[derive(Debug)]
struct RunningWorker {
    handle: Option<JoinHandle<()>>,
    sender: UnboundedSender<WorkerCommand>,
    queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    permit: WorkerMemoryPermit,
    /// Storage semaphore permits held by this worker. `None` until storage
    /// space is first acquired (at startup or on first write). Dropped
    /// automatically when `RunningWorker` is dropped, returning storage
    /// permits to the pool.
    filesystem_storage_permit: Option<FilesystemStoragePermit>,
    waiting_for_command: Arc<AtomicBool>,
    interrupt_signal: Arc<async_lock::Mutex<Option<InterruptKind>>>,
    /// `ResumeReplay` is signalled directly through the command channel rather
    /// than the internal queue, so eviction must treat it as pending work.
    resume_replay_pending: Arc<AtomicBool>,
}

impl Drop for RunningWorker {
    fn drop(&mut self) {
        if let Some(ref permit) = self.filesystem_storage_permit {
            let bytes = crate::services::active_workers::filesystem_storage_permits_to_bytes(
                permit.num_permits() as u32,
            );
            if bytes > 0 {
                record_filesystem_pool_released(bytes);
            }
        }
    }
}

impl RunningWorker {
    pub async fn new<Ctx: WorkerCtx>(
        owned_agent_id: OwnedAgentId,
        queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        parent: Arc<Worker<Ctx>>,
        permit: WorkerMemoryPermit,
        concurrent_agent_permit: crate::services::active_workers::ConcurrentAgentPermit,
        oom_retry_count: u32,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender.send(WorkerCommand::Unblock).unwrap();

        let active_clone = queue.clone();
        let owned_agent_id_clone = owned_agent_id.clone();
        let waiting_for_command = Arc::new(AtomicBool::new(false));
        let waiting_for_command_clone = waiting_for_command.clone();
        let interrupt_signal = Arc::new(async_lock::Mutex::new(None));
        let interrupt_signal_clone = interrupt_signal.clone();
        let resume_replay_pending = Arc::new(AtomicBool::new(false));
        let resume_replay_pending_clone = resume_replay_pending.clone();

        let span = span!(
            parent: None,
            Level::INFO,
            "invocation-loop",
            agent_id = parent.owned_agent_id.agent_id.to_string(),
            agent_type = parent
                .parsed_agent_id
                .as_ref()
                .map(|id| id.agent_type.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );
        let handle = tokio::task::spawn(
            async move {
                RunningWorker::invocation_loop(
                    receiver,
                    active_clone,
                    owned_agent_id_clone,
                    parent,
                    waiting_for_command_clone,
                    interrupt_signal_clone,
                    oom_retry_count,
                    concurrent_agent_permit,
                    resume_replay_pending_clone,
                )
                .instrument(span)
                .await;
            }
            .in_current_span(),
        );

        RunningWorker {
            handle: Some(handle),
            sender,
            queue,
            permit,
            filesystem_storage_permit: None,
            waiting_for_command,
            interrupt_signal,
            resume_replay_pending,
        }
    }

    pub fn merge_extra_permits(&mut self, extra_permit: WorkerMemoryPermit) {
        self.permit.merge(extra_permit);
    }

    /// Merge additional storage permits into this worker's storage permit. If
    /// the worker does not yet hold a storage permit, the given permit becomes
    /// the initial one. Additional calls merge into that initial permit.
    pub fn merge_extra_filesystem_storage_permits(
        &mut self,
        extra_permit: FilesystemStoragePermit,
    ) {
        match &mut self.filesystem_storage_permit {
            Some(existing) => existing.merge(extra_permit),
            None => self.filesystem_storage_permit = Some(extra_permit),
        }
    }

    pub fn stop(mut self) -> JoinHandle<()> {
        self.handle.take().unwrap()
    }

    async fn interrupt(&self, kind: InterruptKind) {
        *self.interrupt_signal.lock().await = Some(kind);
        let _ = self.sender.send(WorkerCommand::Unblock);
    }

    async fn create_instance<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
    ) -> Result<(Instance, async_lock::Mutex<Store<Ctx>>), WorkerExecutorError> {
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
                        return Box::pin(Self::create_instance(parent)).await;
                    } else {
                        Err(error)
                    }
                }
            }?
        };

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

        let context = Ctx::create(
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
            parent.active_workers(),
            parent.oplog_service(),
            parent.oplog.clone(),
            Arc::downgrade(&parent),
            parent.scheduler_service(),
            parent.rpc(),
            parent.worker_proxy(),
            parent.component_service(),
            parent.extra_deps(),
            parent.config(),
            AgentConfig::new(
                skipped_regions,
                worker_metadata.last_known_status.total_linear_memory_size,
                worker_metadata
                    .last_known_status
                    .current_filesystem_storage_usage,
                component_version_for_replay,
                worker_metadata.created_by,
                worker_metadata.created_by_email,
                worker_metadata.config,
                last_snapshot_index,
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
        )
        .await?;

        let engine = parent.engine();
        let mut store = Store::new(&engine, context);

        // Set initial epoch deadline to 0 so the callback fires immediately on the
        // first epoch check point in WASM code, ensuring fuel is checked even for
        // very fast invocations that complete within a single epoch tick interval.
        store.set_epoch_deadline(0);
        store.epoch_deadline_callback(move |mut store| {
            let current_level = store.get_fuel().unwrap_or(0);
            let data_mut = store.data_mut();
            if let Err(error) = data_mut.ensure_fuel(current_level) {
                if data_mut.agent_mode() == AgentMode::Ephemeral {
                    warn!(error = ?error, "Could not borrow more fuel for ephemeral agent");
                    return Err(WorkerExecutorError::InvocationFailed {
                        error,
                        stderr: String::new(),
                    }
                    .into());
                } else {
                    warn!("Could not borrow more fuel, suspending");
                    return Err(InterruptKind::Suspend(Timestamp::now_utc()).into());
                }
            }

            match data_mut.check_interrupt() {
                Some(kind) => Err(kind.into()),
                None => Ok(UpdateDeadline::YieldCustom(
                    1,
                    tokio::task::yield_now().boxed(),
                )),
            }
        });
        store
            .set_fuel(u64::MAX)
            .map_err(|e| WorkerExecutorError::runtime(e.to_string()))?;

        store.limiter_async(|ctx| ctx.resource_limiter());

        let linker = (*parent.linker()).clone(); // fresh linker

        let instance_pre = linker.instantiate_pre(&component).map_err(|e| {
            WorkerExecutorError::worker_creation_failed(
                parent.owned_agent_id.agent_id(),
                format!(
                    "Failed to pre-instantiate worker {}: {e}",
                    parent.owned_agent_id
                ),
            )
        })?;

        let instance = instance_pre
            .instantiate_async(&mut store)
            .await
            .map_err(|e| {
                WorkerExecutorError::worker_creation_failed(
                    parent.owned_agent_id.agent_id(),
                    format!(
                        "Failed to instantiate worker {}: {e}",
                        parent.owned_agent_id
                    ),
                )
            })?;
        let store = async_lock::Mutex::new(store);
        Ok((instance, store))
    }

    async fn invocation_loop<Ctx: WorkerCtx>(
        receiver: UnboundedReceiver<WorkerCommand>,
        active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        owned_agent_id: OwnedAgentId,
        parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
        waiting_for_command: Arc<AtomicBool>,
        interrupt_signal: Arc<async_lock::Mutex<Option<InterruptKind>>>,
        oom_retry_count: u32,
        concurrent_agent_permit: crate::services::active_workers::ConcurrentAgentPermit,
        resume_replay_pending: Arc<AtomicBool>,
    ) {
        let mut invocation_loop = InvocationLoop {
            receiver,
            active,
            owned_agent_id,
            parent,
            waiting_for_command,
            interrupt_signal,
            oom_retry_count,
            concurrent_agent_permit: Some(concurrent_agent_permit),
            resume_replay_pending,
        };
        invocation_loop.run().await;
    }
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
    Deleting,
}

impl FinalWorkerState {
    fn into_instance(self) -> WorkerInstance {
        match self {
            FinalWorkerState::Unloaded { startup_failure } => {
                WorkerInstance::Unloaded { startup_failure }
            }
            FinalWorkerState::Deleting => WorkerInstance::Deleting,
        }
    }
}

#[derive(Debug)]
struct StoppingWorker {
    notify: OneShotEvent,
    final_state: FinalWorkerState,
}

#[derive(Debug, Clone)]
struct FailedInvocationResult {
    pub trap_type: TrapType,
    pub stderr: String,
}

#[derive(Debug, Clone)]
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
                    error, retry_from, ..
                } => {
                    let stderr =
                        recover_stderr_logs(services, owned_agent_id, agent_mode, oplog_idx).await;
                    Err(FailedInvocationResult {
                        trap_type: TrapType::Error {
                            error,
                            retry_from,
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
    use test_r::test;

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
        agent_id.and_then(|id| metadata.find_agent_type_by_name(&id.agent_type));

    let agent_mode = resolved_agent_type
        .as_ref()
        .map_or(AgentMode::Durable, |at| at.mode);

    let snapshot_policy = if let Some(agent_type) = resolved_agent_type.as_ref() {
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

#[derive(Debug)]
enum WorkerCommand {
    Unblock,
    ResumeReplay,
}

#[derive(Debug)]
pub enum QueuedWorkerInvocation {
    GetFileSystemNode {
        path: CanonicalFilePath,
        sender: oneshot::Sender<Result<GetFileSystemNodeResult, WorkerExecutorError>>,
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

pub enum ResultOrSubscription {
    Finished(Result<AgentInvocationOutput, WorkerExecutorError>),
    Pending(EventsSubscription),
}

struct GetOrCreateWorkerResult {
    initial_worker_metadata: AgentMetadata,
    current_status: Arc<RwLock<AgentStatusRecord>>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    agent_id: Option<ParsedAgentId>,
    snapshot_policy: SnapshotPolicy,
    oplog: Arc<dyn Oplog>,
    /// Loaded during `get_or_create_worker_metadata` and stored on the
    /// [`Worker`] so the read-only cache can resolve metadata without a new
    /// `component_service` lookup.
    initial_component: Arc<golem_service_base::model::component::Component>,
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
