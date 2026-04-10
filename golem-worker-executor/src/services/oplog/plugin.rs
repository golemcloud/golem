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

use crate::model::ExecutionStatus;
use crate::model::event::InternalWorkerEvent;
use crate::services::component::ComponentService;
use crate::services::oplog::{CommitLevel, OpenOplogs, Oplog, OplogConstructor, OplogService};
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{
    HasComponentService, HasOplogProcessorPlugin, HasShardService, HasWorkerActivator,
};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_lock::Mutex;
use async_lock::{RwLock, RwLockUpgradableReadGuard};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::Principal;
use golem_common::model::component::{ComponentId, ComponentRevision, InstalledPlugin};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{
    AgentId, AgentInvocation, AgentMetadata, AgentStatusRecord, IdempotencyKey, InvocationStatus,
    OwnedAgentId, ScanCursor, ShardId,
};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::component::Component;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;
use uuid::{Uuid, uuid};

/// Per-plugin live state tracked by `ForwardingOplogState` for exactly-once delivery.
/// Seeded from `AgentStatusRecord.oplog_processor_checkpoints` on construction,
/// updated synchronously after each checkpoint write.
#[derive(Clone, Debug)]
struct LivePluginState {
    target_agent_id: Option<AgentId>,
    confirmed_up_to: OplogIndex,
    sending_up_to: OplogIndex,
    send_in_progress: bool,
    last_batch_start: OplogIndex,
}

#[async_trait]
pub trait OplogProcessorPlugin: Send + Sync {
    /// Resolves or creates a target plugin worker for the given plugin, returning its AgentId.
    async fn resolve_target(
        &self,
        environment_id: EnvironmentId,
        plugin: &InstalledPlugin,
    ) -> Result<AgentId, WorkerExecutorError>;

    /// Enqueues oplog entries to the specified target plugin worker.
    /// Any `Ok(())` means durable delivery — the batch has been accepted.
    async fn send(
        &self,
        worker_metadata: AgentMetadata,
        plugin: &InstalledPlugin,
        target_agent_id: &AgentId,
        initial_oplog_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), WorkerExecutorError>;

    /// Evicts the cached plugin worker for the given plugin, forcing a fresh
    /// instance to be created on the next `resolve_target` call.
    async fn invalidate_target(&self, environment_id: EnvironmentId, plugin: &InstalledPlugin);

    async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError>;

    async fn is_local(&self, agent_id: &AgentId) -> Result<bool, WorkerExecutorError>;

    async fn lookup_invocation_status(
        &self,
        environment_id: EnvironmentId,
        plugin: &InstalledPlugin,
        target_agent_id: &AgentId,
        caller_account_id: AccountId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationStatus, WorkerExecutorError>;
}

/// An implementation of the `OplogProcessorPlugin` trait that runs a single instance of each
/// used plugin on each worker executor node.
pub struct PerExecutorOplogProcessorPlugin<Ctx: WorkerCtx> {
    workers: Arc<RwLock<HashMap<WorkerKey, RunningPlugin>>>,
    component_service: Arc<dyn ComponentService>,
    shard_service: Arc<dyn ShardService>,
    worker_activator: Arc<dyn WorkerActivator<Ctx>>,
    worker_proxy: Arc<dyn WorkerProxy>,
}

type WorkerKey = (EnvironmentId, PluginRegistrationId);

#[derive(Debug, Clone)]
struct RunningPlugin {
    pub account_id: AccountId,
    pub owned_agent_id: OwnedAgentId,
    pub configuration: BTreeMap<String, String>,
    pub component_revision: ComponentRevision,
}

impl<Ctx: WorkerCtx> PerExecutorOplogProcessorPlugin<Ctx> {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        shard_service: Arc<dyn ShardService>,
        worker_activator: Arc<dyn WorkerActivator<Ctx>>,
        worker_proxy: Arc<dyn WorkerProxy>,
    ) -> Self {
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            component_service,
            shard_service,
            worker_activator,
            worker_proxy,
        }
    }

    async fn resolve_plugin_worker(
        &self,
        environment_id: EnvironmentId,
        plugin: &InstalledPlugin,
    ) -> Result<RunningPlugin, WorkerExecutorError> {
        let workers = self.workers.upgradable_read().await;
        let key = (environment_id, plugin.plugin_registration_id);
        match workers.get(&key) {
            Some(running_plugin) => Ok(running_plugin.clone()),
            None => {
                let mut workers = RwLockUpgradableReadGuard::upgrade(workers).await;
                match workers.get(&key) {
                    Some(agent_id) => Ok(agent_id.clone()),
                    None => {
                        let plugin_component_id = plugin
                            .oplog_processor_component_id
                            .ok_or(anyhow!("missing oplog processor plugin component id"))?;
                        let plugin_component_revision =
                            plugin.oplog_processor_component_revision.ok_or(anyhow!(
                                "missing oplog processor plugin component revision"
                            ))?;

                        let agent_id = self.generate_agent_id_for(plugin_component_id).await?;
                        let plugin_component = self
                            .component_service
                            .get_metadata(plugin_component_id, Some(plugin_component_revision))
                            .await?;
                        let owned_agent_id = OwnedAgentId {
                            environment_id,
                            agent_id: agent_id.clone(),
                        };
                        let running_plugin = RunningPlugin {
                            account_id: plugin_component.account_id,
                            owned_agent_id: owned_agent_id.clone(),
                            configuration: plugin.parameters.clone(),
                            component_revision: plugin_component_revision,
                        };
                        workers.insert(key, running_plugin.clone());
                        Ok(running_plugin)
                    }
                }
            }
        }
    }

    async fn generate_agent_id_for(
        &self,
        plugin_component_id: ComponentId,
    ) -> Result<AgentId, WorkerExecutorError> {
        let current_assignment = self.shard_service.current_assignment()?;
        let agent_id = Self::generate_local_agent_id(
            plugin_component_id,
            &current_assignment.shard_ids,
            current_assignment.number_of_shards,
        );

        Ok(agent_id)
    }

    /// Converts a `TargetAgentId` to an `AgentId`. If the worker name was not specified,
    /// it generates a new unique one, and if the `force_in_shard` set is not empty, it guarantees
    /// that the generated worker ID will belong to one of the provided shards.
    ///
    /// If the worker name was specified, `force_in_shard` is ignored.
    fn generate_local_agent_id(
        component_id: ComponentId,
        force_in_shard: &HashSet<ShardId>,
        number_of_shards: usize,
    ) -> AgentId {
        if force_in_shard.is_empty() || number_of_shards == 0 {
            let agent_name = Uuid::new_v4().to_string();
            AgentId {
                component_id,
                agent_id: agent_name,
            }
        } else {
            let mut current = Uuid::new_v4().to_u128_le();
            loop {
                let uuid = Uuid::from_u128_le(current);
                let agent_name = uuid.to_string();
                let agent_id = AgentId {
                    component_id,
                    agent_id: agent_name,
                };
                let shard_id = ShardId::from_agent_id(&agent_id, number_of_shards);
                if force_in_shard.contains(&shard_id) {
                    return agent_id;
                }
                current += 1;
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> OplogProcessorPlugin for PerExecutorOplogProcessorPlugin<Ctx> {
    async fn resolve_target(
        &self,
        environment_id: EnvironmentId,
        plugin: &InstalledPlugin,
    ) -> Result<AgentId, WorkerExecutorError> {
        let running_plugin = self.resolve_plugin_worker(environment_id, plugin).await?;
        Ok(running_plugin.owned_agent_id.agent_id)
    }

    async fn send(
        &self,
        worker_metadata: AgentMetadata,
        plugin: &InstalledPlugin,
        target_agent_id: &AgentId,
        initial_oplog_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        let running_plugin = self
            .resolve_plugin_worker(worker_metadata.environment_id, plugin)
            .await?;

        // Use the explicitly provided target_agent_id for invocation
        let target_owned = OwnedAgentId::new(worker_metadata.environment_id, target_agent_id);

        let worker = self
            .worker_activator
            .get_or_create_running(
                running_plugin.account_id,
                &target_owned,
                None,
                None,
                Vec::new(),
                Some(running_plugin.component_revision),
                None,
                &InvocationContextStack::fresh(),
                Principal::anonymous(),
            )
            .await?;

        let batch_last_index = if entries.is_empty() {
            initial_oplog_index
        } else {
            initial_oplog_index.range_end(entries.len() as u64)
        };
        let idempotency_key = oplog_processor_idempotency_key(
            &worker_metadata.owned_agent_id().agent_id,
            &plugin.environment_plugin_grant_id,
            initial_oplog_index,
            batch_last_index,
        );

        let account_id = worker_metadata.created_by;
        // Enqueue only — any Ok (Pending or Finished) counts as durable delivery
        let _result = worker
            .invoke(AgentInvocation::ProcessOplogEntries {
                idempotency_key,
                account_id,
                config: running_plugin
                    .configuration
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                metadata: worker_metadata.into(),
                first_entry_index: initial_oplog_index,
                entries,
            })
            .await?;

        Ok(())
    }

    async fn invalidate_target(&self, environment_id: EnvironmentId, plugin: &InstalledPlugin) {
        let mut workers = self.workers.write().await;
        workers.remove(&(environment_id, plugin.plugin_registration_id));
    }

    async fn is_local(&self, agent_id: &AgentId) -> Result<bool, WorkerExecutorError> {
        let assignment = self.shard_service.current_assignment()?;
        let shard_id = ShardId::from_agent_id(agent_id, assignment.number_of_shards);
        Ok(assignment.shard_ids.contains(&shard_id))
    }

    async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError> {
        let new_assignment = self.shard_service.current_assignment()?;

        let mut workers = self.workers.write().await;
        let keys = workers.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let entry = workers.entry(key);
            match entry {
                Entry::Occupied(entry) => {
                    let shard_id = ShardId::from_agent_id(
                        &entry.get().owned_agent_id.agent_id,
                        new_assignment.number_of_shards,
                    );
                    if new_assignment.shard_ids.contains(&shard_id) {
                        continue;
                    } else {
                        // The worker is removed from the in-memory map, but we leave it running to finish any pending invocations.
                        // As there is no other reference to this worker id after dropping it here, there won't be any new invocations
                        // sent to it, and it will eventually suspend and got archived.
                        entry.remove();
                    }
                }
                Entry::Vacant(_) => {}
            }
        }

        Ok(())
    }

    async fn lookup_invocation_status(
        &self,
        environment_id: EnvironmentId,
        _plugin: &InstalledPlugin,
        target_agent_id: &AgentId,
        caller_account_id: AccountId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationStatus, WorkerExecutorError> {
        self.worker_proxy
            .lookup_invocation_status(
                target_agent_id,
                idempotency_key.clone(),
                caller_account_id,
                Some(environment_id),
            )
            .await
            .map_err(|e| {
                WorkerExecutorError::unknown(format!("Lookup invocation status failed: {e}"))
            })
    }
}

impl<Ctx: WorkerCtx> Clone for PerExecutorOplogProcessorPlugin<Ctx> {
    fn clone(&self) -> Self {
        Self {
            workers: self.workers.clone(),
            component_service: self.component_service.clone(),
            shard_service: self.shard_service.clone(),
            worker_activator: self.worker_activator.clone(),
            worker_proxy: self.worker_proxy.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> HasComponentService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn shard_service(&self) -> Arc<dyn ShardService> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator<Ctx> for PerExecutorOplogProcessorPlugin<Ctx> {
    fn worker_activator(&self) -> Arc<dyn WorkerActivator<Ctx>> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for PerExecutorOplogProcessorPlugin<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin> {
        Arc::new(self.clone())
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    owned_agent_id: OwnedAgentId,
    initial_entry: Option<OplogEntry>,
    inner: Arc<dyn OplogService>,
    last_oplog_index: Option<OplogIndex>,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    components: Arc<dyn ComponentService>,
    initial_worker_metadata: AgentMetadata,
    last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
    execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    plugin_max_commit_count: usize,
    plugin_max_elapsed_time: Duration,
}

impl CreateOplogConstructor {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        initial_entry: Option<OplogEntry>,
        inner: Arc<dyn OplogService>,
        last_oplog_index: Option<OplogIndex>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        plugin_max_commit_count: usize,
        plugin_max_elapsed_time: Duration,
    ) -> Self {
        Self {
            owned_agent_id,
            initial_entry,
            inner,
            last_oplog_index,
            oplog_plugins,
            components,
            initial_worker_metadata,
            last_known_status,
            execution_status,
            plugin_max_commit_count,
            plugin_max_elapsed_time,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(self, close: Box<dyn FnOnce() + Send + Sync>) -> Arc<dyn Oplog> {
        let last_oplog_index = match self.last_oplog_index {
            Some(idx) => idx,
            None => self.inner.get_last_index(&self.owned_agent_id).await,
        };
        let inner = if let Some(initial_entry) = self.initial_entry {
            self.inner
                .create(
                    &self.owned_agent_id,
                    initial_entry,
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        } else {
            self.inner
                .open(
                    &self.owned_agent_id,
                    Some(last_oplog_index),
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        };

        Arc::new(
            ForwardingOplog::new(
                inner,
                self.oplog_plugins,
                self.components,
                self.initial_worker_metadata,
                self.last_known_status,
                last_oplog_index,
                close,
                self.plugin_max_commit_count,
                self.plugin_max_elapsed_time,
            )
            .await,
        )
    }
}

pub struct ForwardingOplogService {
    pub inner: Arc<dyn OplogService>,
    oplogs: OpenOplogs,

    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    components: Arc<dyn ComponentService>,
    plugin_max_commit_count: usize,
    plugin_max_elapsed_time: Duration,
}

impl ForwardingOplogService {
    pub fn new(
        inner: Arc<dyn OplogService>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        plugin_max_commit_count: usize,
        plugin_max_elapsed_time: Duration,
    ) -> Self {
        Self {
            inner,
            oplogs: OpenOplogs::new("forwarding_oplog_service"),
            oplog_plugins,
            components,
            plugin_max_commit_count,
            plugin_max_elapsed_time,
        }
    }
}

impl Debug for ForwardingOplogService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardingOplogService").finish()
    }
}

#[async_trait]
impl OplogService for ForwardingOplogService {
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        self.oplogs
            .get_or_open(
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    Some(initial_entry),
                    self.inner.clone(),
                    Some(OplogIndex::INITIAL),
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    self.plugin_max_commit_count,
                    self.plugin_max_elapsed_time,
                ),
            )
            .await
    }

    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        self.oplogs
            .get_or_open(
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    None,
                    self.inner.clone(),
                    last_oplog_index,
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    self.plugin_max_commit_count,
                    self.plugin_max_elapsed_time,
                ),
            )
            .await
    }

    async fn get_last_index(&self, owned_agent_id: &OwnedAgentId) -> OplogIndex {
        self.inner.get_last_index(owned_agent_id).await
    }

    async fn delete(&self, owned_agent_id: &OwnedAgentId) {
        self.inner.delete(owned_agent_id).await
    }

    async fn read(
        &self,
        owned_agent_id: &OwnedAgentId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read(owned_agent_id, idx, n).await
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.inner.exists(owned_agent_id).await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, cursor, count)
            .await
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(owned_agent_id, data).await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .download_raw_payload(owned_agent_id, payload_id, md5_hash)
            .await
    }
}

/// A wrapper for `Oplog` that periodically sends buffered oplog entries to oplog processor plugins
pub struct ForwardingOplog {
    inner: Arc<dyn Oplog>,
    state: Arc<Mutex<ForwardingOplogState>>,
    max_commit_count: usize,
    timer: Option<JoinHandle<()>>,
    close_fn: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl ForwardingOplog {
    pub async fn new(
        inner: Arc<dyn Oplog>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        last_oplog_idx: OplogIndex,
        close_fn: Box<dyn FnOnce() + Send + Sync>,
        max_commit_count: usize,
        max_elapsed_time: Duration,
    ) -> Self {
        // Seed per-plugin live state from the current status snapshot (not the
        // potentially stale initial_worker_metadata.last_known_status)
        let plugin_state = {
            let seed_status = last_known_status.read().await;
            let mut state = HashMap::new();
            for (grant_id, cp) in &seed_status.oplog_processor_checkpoints {
                state.insert(
                    *grant_id,
                    LivePluginState {
                        target_agent_id: cp.target_agent_id.clone(),
                        confirmed_up_to: cp.confirmed_up_to,
                        sending_up_to: cp.sending_up_to,
                        send_in_progress: false,
                        last_batch_start: cp.last_batch_start,
                    },
                );
            }
            // Also seed entries for active plugins that have no checkpoint yet (active from Create)
            for grant_id in &seed_status.active_plugins {
                state.entry(*grant_id).or_insert(LivePluginState {
                    target_agent_id: None,
                    confirmed_up_to: OplogIndex::NONE,
                    sending_up_to: OplogIndex::NONE,
                    send_in_progress: false,
                    last_batch_start: OplogIndex::NONE,
                });
            }
            state
        };

        let state = Arc::new(Mutex::new(ForwardingOplogState {
            buffer: VecDeque::new(),
            buffer_start_idx: last_oplog_idx.next(),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins,
            initial_worker_metadata,
            last_known_status,
            last_oplog_idx,
            last_committed_idx: last_oplog_idx,
            components,
            inner: inner.clone(),
            plugin_state,
            pending_direct_commits: BTreeMap::new(),
            worker_event_service: None,
            monitor_tasks: Vec::new(),
        }));

        let timer = tokio::spawn({
            let state = state.clone();
            async move {
                loop {
                    tokio::time::sleep(max_elapsed_time).await;
                    let mut state = state.lock().await;
                    state.try_locality_recovery().await;
                    state.try_flush().await;
                }
            }
            .in_current_span()
        });
        Self {
            inner,
            state,
            max_commit_count,
            timer: Some(timer),
            close_fn: Some(close_fn),
        }
    }

    pub async fn set_worker_event_service(
        &self,
        worker_event_service: Arc<dyn WorkerEventService>,
    ) {
        let mut state = self.state.lock().await;
        state.worker_event_service = Some(worker_event_service);
    }
}

impl Debug for ForwardingOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardingOplog").finish()
    }
}

impl Drop for ForwardingOplog {
    fn drop(&mut self) {
        if let Some(close_fn) = self.close_fn.take() {
            close_fn();
        }
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        // Abort all background monitor tasks to prevent them from
        // outliving this oplog and causing resource contention.
        if let Some(mut state) = self.state.try_lock() {
            for task in state.monitor_tasks.drain(..) {
                task.abort();
            }
        }
    }
}

#[async_trait]
impl Oplog for ForwardingOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        let mut state = self.state.lock().await;
        state.buffer.push_back(entry.clone());
        state.last_oplog_idx = state.last_oplog_idx.next();
        self.inner.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.inner.drop_prefix(last_dropped_id).await
    }

    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut state = self.state.lock().await;
        let mut result = self.inner.commit(level).await;
        // Update last_committed_idx from committed entries
        if let Some(max_idx) = result.keys().max()
            && *max_idx > state.last_committed_idx
        {
            state.last_committed_idx = *max_idx;
        }
        state.commit_count += 1;
        if state.commit_count >= self.max_commit_count {
            state.try_flush().await;
        }
        // Merge entries committed directly to inner during flush
        // so the Worker folds them into AgentStatusRecord
        result.append(&mut state.pending_direct_commits);
        result
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.inner.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.inner.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.inner.read(oplog_index).await
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read_many(oplog_index, n).await
    }

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner.download_raw_payload(payload_id, md5_hash).await
    }

    async fn switch_persistence_level(&self, mode: PersistenceLevel) {
        self.inner.switch_persistence_level(mode).await;
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}

struct ForwardingOplogState {
    buffer: VecDeque<OplogEntry>,
    /// Oplog index corresponding to the first entry in `buffer`.
    buffer_start_idx: OplogIndex,
    commit_count: usize,
    last_send: Instant,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    initial_worker_metadata: AgentMetadata,
    last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    components: Arc<dyn ComponentService>,
    inner: Arc<dyn Oplog>,
    plugin_state: HashMap<EnvironmentPluginGrantId, LivePluginState>,
    /// Entries committed directly to inner (bypassing `ForwardingOplog::commit()`),
    /// to be surfaced in the next `ForwardingOplog::commit()` return value
    /// so the Worker folds them into `AgentStatusRecord`.
    pending_direct_commits: BTreeMap<OplogIndex, OplogEntry>,
    worker_event_service: Option<Arc<dyn WorkerEventService>>,
    monitor_tasks: Vec<JoinHandle<()>>,
}

impl ForwardingOplogState {
    /// Cursor-driven flush: for each plugin with unsent entries, send a batch.
    /// Handles retries of in-flight batches, first-send target resolution,
    /// pre-send and confirmation checkpoint writes.
    ///
    /// Entries are always read from the persisted oplog (canonical source) to avoid
    /// buffer/index drift caused by checkpoint entries injected during flush.
    pub async fn try_flush(&mut self) {
        let status = self.last_known_status.read().await.clone();
        let flush_set = self.reconcile_plugin_state(&status);

        if flush_set.is_empty() {
            self.finish_empty_flush();
        } else if let Some((metadata, component_metadata)) =
            self.prepare_flush_context(&status).await
        {
            let committed_tail = self.last_committed_idx;

            for grant_id in flush_set {
                self.flush_one_plugin(grant_id, committed_tail, &metadata, &component_metadata)
                    .await;
            }

            self.prune_buffer();
            self.finish_flush_cycle();
        }
    }

    /// Sync plugin_state with the current status: add newly active plugins,
    /// keep existing local state for already-tracked plugins, remove stale
    /// entries that are neither active nor in-flight.
    /// Returns the flush set (active ∪ in-flight plugin grant IDs).
    fn reconcile_plugin_state(
        &mut self,
        status: &AgentStatusRecord,
    ) -> Vec<EnvironmentPluginGrantId> {
        for grant_id in &status.active_plugins {
            if let Entry::Vacant(e) = self.plugin_state.entry(*grant_id) {
                let live = if let Some(cp) = status.oplog_processor_checkpoints.get(grant_id) {
                    LivePluginState {
                        target_agent_id: cp.target_agent_id.clone(),
                        confirmed_up_to: cp.confirmed_up_to,
                        sending_up_to: cp.sending_up_to,
                        send_in_progress: false,
                        last_batch_start: cp.last_batch_start,
                    }
                } else {
                    LivePluginState {
                        target_agent_id: None,
                        confirmed_up_to: OplogIndex::NONE,
                        sending_up_to: OplogIndex::NONE,
                        send_in_progress: false,
                        last_batch_start: OplogIndex::NONE,
                    }
                };
                e.insert(live);
            }
        }

        self.plugin_state.retain(|grant_id, state| {
            status.active_plugins.contains(grant_id) || state.sending_up_to > state.confirmed_up_to
        });

        self.plugin_state
            .iter()
            .filter(|(id, state)| {
                status.active_plugins.contains(id) || state.sending_up_to > state.confirmed_up_to
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Build the AgentMetadata and fetch component metadata needed for the flush.
    /// Returns `None` if component metadata cannot be retrieved.
    async fn prepare_flush_context(
        &self,
        status: &AgentStatusRecord,
    ) -> Option<(AgentMetadata, Component)> {
        let metadata = AgentMetadata {
            last_known_status: status.clone(),
            ..self.initial_worker_metadata.clone()
        };

        match self
            .components
            .get_metadata(
                metadata.owned_agent_id().component_id(),
                Some(status.component_revision),
            )
            .await
        {
            Ok(component_metadata) => Some((metadata, component_metadata)),
            Err(err) => {
                tracing::error!(
                    "Failed to get component metadata for oplog processor flush: {err}"
                );
                None
            }
        }
    }

    /// Flush a single plugin: resolve target, read entries, write checkpoints, send.
    ///
    /// Uses enqueue-and-confirm: after a successful `send()` (which only enqueues
    /// the invocation), we immediately write the confirmation checkpoint. A background
    /// monitoring task is spawned to observe errors and emit plugin_error events.
    async fn flush_one_plugin(
        &mut self,
        grant_id: EnvironmentPluginGrantId,
        committed_tail: OplogIndex,
        metadata: &AgentMetadata,
        component_metadata: &Component,
    ) {
        let live = match self.plugin_state.get(&grant_id) {
            Some(s) if !s.send_in_progress => s.clone(),
            _ => return,
        };

        let plugin = match component_metadata
            .installed_plugins
            .iter()
            .find(|p| p.environment_plugin_grant_id == grant_id)
        {
            Some(p) => p.clone(),
            None => return,
        };

        let batch = if live.sending_up_to > live.confirmed_up_to {
            // RETRY: exact same range — never widen
            Some((live.confirmed_up_to.next(), live.sending_up_to, true))
        } else if live.confirmed_up_to < committed_tail {
            // NEW BATCH: from confirmed+1 to committed tail
            Some((live.confirmed_up_to.next(), committed_tail, false))
        } else {
            None
        };

        let (batch_start, batch_end, is_retry) = match batch {
            Some(b) => b,
            None => return,
        };

        let target_agent_id = if let Some(id) = &live.target_agent_id {
            id.clone()
        } else {
            match self
                .oplog_plugins
                .resolve_target(metadata.environment_id, &plugin)
                .await
            {
                Ok(id) => {
                    self.write_checkpoint(
                        grant_id,
                        &id,
                        live.confirmed_up_to,
                        live.confirmed_up_to,
                        live.last_batch_start,
                    )
                    .await;
                    if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                        s.target_agent_id = Some(id.clone());
                    }
                    id
                }
                Err(err) => {
                    tracing::error!("Failed to resolve target for plugin {grant_id}: {err}");
                    return;
                }
            }
        };

        if let Some(s) = self.plugin_state.get_mut(&grant_id) {
            s.send_in_progress = true;
        }

        let batch_count = (batch_end.as_u64() - batch_start.as_u64() + 1) as usize;
        let entries = self.read_batch(batch_start, batch_count).await;

        if entries.len() != batch_count {
            tracing::error!(
                "Expected {batch_count} entries for plugin {grant_id} [{batch_start}..{batch_end}], got {}",
                entries.len()
            );
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.send_in_progress = false;
            }
            return;
        }

        // If ALL entries are checkpoint entries, skip — nothing meaningful to deliver.
        // Advance the cursor past them so we don't re-read the same range forever.
        if entries
            .iter()
            .all(|e| matches!(e, OplogEntry::OplogProcessorCheckpoint { .. }))
        {
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.confirmed_up_to = batch_end;
                s.sending_up_to = batch_end;
                s.send_in_progress = false;
            }
            return;
        }

        if !is_retry {
            self.write_checkpoint(
                grant_id,
                &target_agent_id,
                live.confirmed_up_to,
                batch_end,
                batch_start,
            )
            .await;
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.sending_up_to = batch_end;
            }
        }

        match self
            .oplog_plugins
            .send(
                metadata.clone(),
                &plugin,
                &target_agent_id,
                batch_start,
                entries,
            )
            .await
        {
            Ok(()) => {
                // Enqueue succeeded — immediately confirm
                self.write_checkpoint(
                    grant_id,
                    &target_agent_id,
                    batch_end,
                    batch_end,
                    batch_start,
                )
                .await;
                if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                    s.confirmed_up_to = batch_end;
                    s.sending_up_to = batch_end;
                    s.send_in_progress = false;
                    s.last_batch_start = batch_start;
                }

                // Spawn background monitoring task to observe errors.
                // Compute the idempotency key here so only lightweight data
                // needs to be moved into the task.
                let idempotency_key = oplog_processor_idempotency_key(
                    &metadata.agent_id,
                    &plugin.environment_plugin_grant_id,
                    batch_start,
                    batch_end,
                );
                let worker_event_service = self.worker_event_service.clone();
                let oplog_plugins = self.oplog_plugins.clone();
                let environment_id = metadata.environment_id;
                let caller_account_id = metadata.created_by;
                let plugin_clone = plugin.clone();
                let target_clone = target_agent_id.clone();
                let monitor = tokio::spawn(
                    async move {
                        // Poll until the invocation completes, with a timeout
                        let deadline = Instant::now() + Duration::from_secs(300);
                        loop {
                            if Instant::now() >= deadline {
                                tracing::warn!(
                                    "Plugin {grant_id}: monitoring timed out for batch [{batch_start}..{batch_end}]"
                                );
                                break;
                            }

                            tokio::time::sleep(Duration::from_secs(1)).await;

                            match oplog_plugins
                                .lookup_invocation_status(
                                    environment_id,
                                    &plugin_clone,
                                    &target_clone,
                                    caller_account_id,
                                    &idempotency_key,
                                )
                                .await
                            {
                                Ok(InvocationStatus::Complete) => {
                                    // Completed — nothing to do
                                    break;
                                }
                                Ok(InvocationStatus::Unknown) => {
                                    tracing::warn!(
                                        "Plugin {grant_id}: invocation status unknown for batch [{batch_start}..{batch_end}]"
                                    );
                                    break;
                                }
                                Ok(_) => {
                                    // Still pending — continue polling
                                }
                                Err(err) => {
                                    tracing::error!(
                                        "Plugin {grant_id} error monitoring batch [{batch_start}..{batch_end}]: {err}"
                                    );
                                    if let Some(event_service) = &worker_event_service {
                                        event_service.emit_event(
                                            InternalWorkerEvent::plugin_error(
                                                &format!("{grant_id}"),
                                                &format!("Error monitoring batch [{batch_start}..{batch_end}]: {err}"),
                                            ),
                                            true,
                                        );
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    .in_current_span(),
                );
                self.monitor_tasks.push(monitor);
            }
            Err(err) => {
                // Pre-enqueue failure — permanent for this target instance
                tracing::error!("Failed to enqueue oplog entries to plugin {grant_id}: {err}");
                if let Some(event_service) = &self.worker_event_service {
                    event_service.emit_event(
                        InternalWorkerEvent::plugin_error(
                            &format!("{grant_id}"),
                            &format!("Failed to enqueue batch [{batch_start}..{batch_end}]: {err}"),
                        ),
                        true,
                    );
                }
                // Invalidate target — next flush will create a fresh instance
                self.oplog_plugins
                    .invalidate_target(metadata.environment_id, &plugin)
                    .await;
                if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                    s.target_agent_id = None;
                    s.send_in_progress = false;
                    // Leave cursors unchanged — same batch retried on new target
                }
            }
        }
    }

    /// Read `count` entries starting at `start` from the buffer if available,
    /// otherwise fall back to the persisted oplog.
    async fn read_batch(&self, start: OplogIndex, count: usize) -> Vec<OplogEntry> {
        if count == 0 {
            return Vec::new();
        }

        if !self.buffer.is_empty() && start >= self.buffer_start_idx {
            let buffer_end_idx = self.buffer_start_idx.range_end(self.buffer.len() as u64);
            let request_end_idx = start.range_end(count as u64);

            if request_end_idx <= buffer_end_idx {
                let offset = (start.as_u64() - self.buffer_start_idx.as_u64()) as usize;
                return self
                    .buffer
                    .iter()
                    .skip(offset)
                    .take(count)
                    .cloned()
                    .collect();
            }
        }

        self.inner
            .read_many(start, count as u64)
            .await
            .into_values()
            .collect()
    }

    /// Write an OplogProcessorCheckpoint entry, commit it, and update index tracking.
    async fn write_checkpoint(
        &mut self,
        grant_id: EnvironmentPluginGrantId,
        target_agent_id: &AgentId,
        confirmed_up_to: OplogIndex,
        sending_up_to: OplogIndex,
        last_batch_start: OplogIndex,
    ) {
        let checkpoint = OplogEntry::OplogProcessorCheckpoint {
            timestamp: golem_common::model::Timestamp::now_utc(),
            plugin_grant_id: grant_id,
            target_agent_id: target_agent_id.clone(),
            confirmed_up_to,
            sending_up_to,
            last_batch_start,
        };
        self.buffer.push_back(checkpoint.clone());
        let idx = self.inner.add(checkpoint).await;
        self.last_oplog_idx = idx;
        let committed = self.inner.commit(CommitLevel::Always).await;
        if let Some(max_idx) = committed.keys().max().copied() {
            self.last_committed_idx = self.last_committed_idx.max(max_idx);
        }
        // Track all directly committed entries so ForwardingOplog::commit()
        // can surface them to the Worker for status folding
        self.pending_direct_commits.extend(committed);
    }

    /// Prune buffer: drain entries that ALL active/in-flight plugins have confirmed past.
    fn prune_buffer(&mut self) {
        if self.plugin_state.is_empty() || self.buffer.is_empty() {
            return;
        }

        let min_confirmed = self
            .plugin_state
            .values()
            .map(|s| s.confirmed_up_to)
            .min()
            .unwrap_or(OplogIndex::NONE);

        if min_confirmed >= self.buffer_start_idx {
            let drain_up_to = min_confirmed.as_u64() - self.buffer_start_idx.as_u64() + 1;
            let drain_count = (drain_up_to as usize).min(self.buffer.len());
            self.buffer.drain(..drain_count);
            self.buffer_start_idx =
                OplogIndex::from_u64(self.buffer_start_idx.as_u64() + drain_count as u64);
        }
    }

    fn finish_empty_flush(&mut self) {
        self.buffer.clear();
        self.buffer_start_idx = self.last_oplog_idx.next();
        self.last_send = Instant::now();
        self.commit_count = 0;
    }

    fn finish_flush_cycle(&mut self) {
        self.last_send = Instant::now();
        self.commit_count = 0;
        // Prune completed monitor tasks
        self.monitor_tasks.retain(|h| !h.is_finished());
    }

    /// Periodic locality recovery: for each plugin whose target worker is on a
    /// remote executor (after shard reassignment), check if the old target has
    /// caught up and, if so, migrate to a new local plugin worker.
    ///
    /// This is an optimization — correctness is preserved regardless because
    /// delivery always goes to the recorded `target_agent_id` with deterministic
    /// idempotency keys.
    async fn try_locality_recovery(&mut self) {
        let status = self.last_known_status.read().await.clone();
        // Ensure plugin_state is reconciled with current status
        self.reconcile_plugin_state(&status);
        let environment_id = self.initial_worker_metadata.environment_id;

        // Collect candidates: plugins with a target that might be non-local
        let candidates: Vec<(EnvironmentPluginGrantId, AgentId)> = self
            .plugin_state
            .iter()
            .filter(|(_, state)| {
                state.target_agent_id.is_some()
                    && !state.send_in_progress
                    && state.sending_up_to <= state.confirmed_up_to
            })
            .map(|(grant_id, state)| (*grant_id, state.target_agent_id.clone().unwrap()))
            .collect();

        if candidates.is_empty() {
            return;
        }

        let component_metadata = match self.prepare_flush_context(&status).await {
            Some((_, cm)) => cm,
            None => return,
        };

        for (grant_id, old_target) in candidates {
            // Check if the target is already local
            match self.oplog_plugins.is_local(&old_target).await {
                Ok(true) => continue, // already local
                Ok(false) => {}       // non-local, try recovery
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        error = %err,
                        "Locality recovery: failed to check locality"
                    );
                    continue;
                }
            }

            let plugin = match component_metadata
                .installed_plugins
                .iter()
                .find(|p| p.environment_plugin_grant_id == grant_id)
            {
                Some(p) => p.clone(),
                None => continue,
            };

            let (confirmed, last_batch_start) = match self.plugin_state.get(&grant_id) {
                Some(s) => (s.confirmed_up_to, s.last_batch_start),
                None => continue,
            };

            // Recompute the idempotency key for the last confirmed batch
            if last_batch_start == confirmed {
                // No batch has been confirmed yet (sentinel value), skip
                tracing::debug!(
                    plugin = %grant_id,
                    "Locality recovery: no confirmed batch yet, skipping"
                );
                continue;
            }

            let last_key = oplog_processor_idempotency_key(
                &self.initial_worker_metadata.agent_id,
                &grant_id,
                last_batch_start,
                confirmed,
            );

            match self
                .oplog_plugins
                .lookup_invocation_status(
                    environment_id,
                    &plugin,
                    &old_target,
                    self.initial_worker_metadata.created_by,
                    &last_key,
                )
                .await
            {
                Ok(InvocationStatus::Complete | InvocationStatus::Pending) => {
                    // Old target has received the batch (complete or queued), proceed with migration
                }
                Ok(status) => {
                    tracing::debug!(
                        plugin = %grant_id,
                        old_target = %old_target,
                        ?status,
                        "Locality recovery: old target has not received last batch yet, skipping"
                    );
                    continue;
                }
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        old_target = %old_target,
                        error = %err,
                        "Locality recovery: failed to query old target status"
                    );
                    continue;
                }
            }

            // Migrate to a new local worker
            let new_target = match self
                .oplog_plugins
                .resolve_target(environment_id, &plugin)
                .await
            {
                Ok(t) => t,
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        error = %err,
                        "Locality recovery: failed to resolve new local target"
                    );
                    continue;
                }
            };

            // Don't persist a no-op migration or migrate to another non-local target
            if new_target == old_target {
                tracing::debug!(
                    plugin = %grant_id,
                    target = %old_target,
                    "Locality recovery: resolved target is unchanged, skipping"
                );
                continue;
            }
            match self.oplog_plugins.is_local(&new_target).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!(
                        plugin = %grant_id,
                        new_target = %new_target,
                        "Locality recovery: resolved target is still non-local, skipping"
                    );
                    continue;
                }
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        new_target = %new_target,
                        error = %err,
                        "Locality recovery: failed to verify locality of new target"
                    );
                    continue;
                }
            }

            self.write_checkpoint(
                grant_id,
                &new_target,
                confirmed,
                confirmed,
                last_batch_start,
            )
            .await;
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.target_agent_id = Some(new_target.clone());
            }
            tracing::info!(
                plugin = %grant_id,
                old_target = %old_target,
                new_target = %new_target,
                "Locality recovery: migrated plugin to new local target"
            );
        }
    }
}

const OPLOG_PROC_NS: Uuid = uuid!("A7E3F1B2-8C4D-5E6F-9A0B-1C2D3E4F5A6B");

fn oplog_processor_idempotency_key(
    source_agent_id: &AgentId,
    plugin_installation_id: &EnvironmentPluginGrantId,
    batch_first_index: OplogIndex,
    batch_last_index: OplogIndex,
) -> IdempotencyKey {
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(b"oplog-proc-v1\0");
    buf.extend_from_slice(source_agent_id.component_id.0.as_bytes());
    let worker_name = source_agent_id.agent_id.as_bytes();
    buf.extend_from_slice(&(worker_name.len() as u32).to_be_bytes());
    buf.extend_from_slice(worker_name);
    buf.extend_from_slice(plugin_installation_id.0.as_bytes());
    buf.extend_from_slice(&batch_first_index.as_u64().to_be_bytes());
    buf.extend_from_slice(&batch_last_index.as_u64().to_be_bytes());
    IdempotencyKey::from_uuid(Uuid::new_v5(&OPLOG_PROC_NS, &buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use golem_common::model::account::AccountId;
    use golem_common::model::application::ApplicationId;
    use golem_common::model::component::{
        ComponentId, ComponentName, ComponentRevision, InstalledPlugin,
    };
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::diff;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::model::oplog::PersistenceLevel;
    use golem_common::model::plugin_registration::PluginRegistrationId;
    use golem_common::read_only_lock;
    use golem_service_base::model::component::Component;
    use test_r::test;

    // --------------------------------------------------------------------------
    // U1: Deterministic idempotency key stability
    // --------------------------------------------------------------------------

    #[test]
    fn deterministic_idempotency_key_same_inputs_produce_same_key() {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "test-worker".to_string(),
        };
        let plugin_id = EnvironmentPluginGrantId::new();
        let first = OplogIndex::from_u64(10);
        let last = OplogIndex::from_u64(20);

        let key1 = oplog_processor_idempotency_key(&agent_id, &plugin_id, first, last);
        let key2 = oplog_processor_idempotency_key(&agent_id, &plugin_id, first, last);

        assert_eq!(
            key1, key2,
            "Same inputs must produce the same idempotency key"
        );
    }

    #[test]
    fn deterministic_idempotency_key_different_batch_range_produces_different_key() {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "test-worker".to_string(),
        };
        let plugin_id = EnvironmentPluginGrantId::new();

        let key1 = oplog_processor_idempotency_key(
            &agent_id,
            &plugin_id,
            OplogIndex::from_u64(10),
            OplogIndex::from_u64(20),
        );
        let key2 = oplog_processor_idempotency_key(
            &agent_id,
            &plugin_id,
            OplogIndex::from_u64(21),
            OplogIndex::from_u64(30),
        );

        assert_ne!(
            key1, key2,
            "Different batch ranges must produce different keys"
        );
    }

    #[test]
    fn deterministic_idempotency_key_different_worker_produces_different_key() {
        let component_id = ComponentId::new();
        let agent_id1 = AgentId {
            component_id,
            agent_id: "worker-a".to_string(),
        };
        let agent_id2 = AgentId {
            component_id,
            agent_id: "worker-b".to_string(),
        };
        let plugin_id = EnvironmentPluginGrantId::new();
        let first = OplogIndex::from_u64(1);
        let last = OplogIndex::from_u64(5);

        let key1 = oplog_processor_idempotency_key(&agent_id1, &plugin_id, first, last);
        let key2 = oplog_processor_idempotency_key(&agent_id2, &plugin_id, first, last);

        assert_ne!(key1, key2, "Different workers must produce different keys");
    }

    #[test]
    fn deterministic_idempotency_key_different_plugin_produces_different_key() {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "test-worker".to_string(),
        };
        let plugin_id1 = EnvironmentPluginGrantId::new();
        let plugin_id2 = EnvironmentPluginGrantId::new();
        let first = OplogIndex::from_u64(1);
        let last = OplogIndex::from_u64(5);

        let key1 = oplog_processor_idempotency_key(&agent_id, &plugin_id1, first, last);
        let key2 = oplog_processor_idempotency_key(&agent_id, &plugin_id2, first, last);

        assert_ne!(key1, key2, "Different plugins must produce different keys");
    }

    // --------------------------------------------------------------------------
    // Helpers: recording mock for OplogProcessorPlugin and fake ComponentService
    // --------------------------------------------------------------------------

    /// Records all `send()` calls for verification
    struct RecordingOplogProcessorPlugin {
        sends: async_lock::Mutex<Vec<RecordedSend>>,
        lookups: async_lock::Mutex<Vec<RecordedLookup>>,
    }

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct RecordedSend {
        initial_oplog_index: OplogIndex,
        entry_count: usize,
    }

    #[derive(Debug, Clone)]
    struct RecordedLookup {
        caller_account_id: AccountId,
    }

    impl RecordingOplogProcessorPlugin {
        fn new() -> Self {
            Self {
                sends: async_lock::Mutex::new(Vec::new()),
                lookups: async_lock::Mutex::new(Vec::new()),
            }
        }

        async fn send_count(&self) -> usize {
            self.sends.lock().await.len()
        }

        async fn sends(&self) -> Vec<RecordedSend> {
            self.sends.lock().await.clone()
        }

        async fn lookups(&self) -> Vec<RecordedLookup> {
            self.lookups.lock().await.clone()
        }
    }

    #[async_trait]
    impl OplogProcessorPlugin for RecordingOplogProcessorPlugin {
        async fn resolve_target(
            &self,
            _environment_id: EnvironmentId,
            _plugin: &InstalledPlugin,
        ) -> Result<AgentId, WorkerExecutorError> {
            Ok(AgentId {
                component_id: ComponentId::new(),
                agent_id: "mock-target".to_string(),
            })
        }

        async fn send(
            &self,
            _worker_metadata: AgentMetadata,
            _plugin: &InstalledPlugin,
            _target_agent_id: &AgentId,
            initial_oplog_index: OplogIndex,
            entries: Vec<OplogEntry>,
        ) -> Result<(), WorkerExecutorError> {
            self.sends.lock().await.push(RecordedSend {
                initial_oplog_index,
                entry_count: entries.len(),
            });
            Ok(())
        }

        async fn invalidate_target(
            &self,
            _environment_id: EnvironmentId,
            _plugin: &InstalledPlugin,
        ) {
            // no-op in tests
        }

        async fn is_local(&self, _agent_id: &AgentId) -> Result<bool, WorkerExecutorError> {
            Ok(true)
        }

        async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError> {
            Ok(())
        }

        async fn lookup_invocation_status(
            &self,
            _environment_id: EnvironmentId,
            _plugin: &InstalledPlugin,
            _target_agent_id: &AgentId,
            caller_account_id: AccountId,
            _idempotency_key: &IdempotencyKey,
        ) -> Result<InvocationStatus, WorkerExecutorError> {
            self.lookups
                .lock()
                .await
                .push(RecordedLookup { caller_account_id });
            Ok(InvocationStatus::Unknown)
        }
    }

    /// A fake ComponentService that returns a component with one installed oplog processor plugin
    struct FakeComponentService {
        installed_plugins: Vec<InstalledPlugin>,
    }

    impl FakeComponentService {
        fn with_one_oplog_processor_plugin(grant_id: EnvironmentPluginGrantId) -> Self {
            Self {
                installed_plugins: vec![InstalledPlugin {
                    environment_plugin_grant_id: grant_id,
                    priority: golem_common::model::component::PluginPriority(0),
                    parameters: BTreeMap::new(),
                    plugin_registration_id: PluginRegistrationId::new(),
                    plugin_name: "test-oplog-plugin".to_string(),
                    plugin_version: "1.0.0".to_string(),
                    oplog_processor_component_id: Some(ComponentId::new()),
                    oplog_processor_component_revision: Some(ComponentRevision::INITIAL),
                }],
            }
        }

        fn empty() -> Self {
            Self {
                installed_plugins: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl ComponentService for FakeComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            unimplemented!("not needed for forwarding oplog tests")
        }

        async fn get_metadata(
            &self,
            component_id: ComponentId,
            _forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            Ok(Component {
                id: component_id,
                revision: ComponentRevision::INITIAL,
                environment_id: EnvironmentId::new(),
                component_name: ComponentName("test-component".to_string()),
                hash: diff::Hash::empty(),
                application_id: ApplicationId::new(),
                account_id: AccountId::new(),
                component_size: 100,
                metadata: ComponentMetadata::default(),
                created_at: chrono::Utc::now(),
                files: Vec::new(),
                installed_plugins: self.installed_plugins.clone(),
                env: BTreeMap::new(),
                config_vars: BTreeMap::new(),
                agent_config: Vec::new(),
                wasm_hash: diff::Hash::empty(),
                object_store_key: String::new(),
            })
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            Ok(None)
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            Vec::new()
        }
    }

    /// A minimal in-memory oplog for testing ForwardingOplog behavior
    #[allow(dead_code)]
    struct InMemoryOplog {
        entries: async_lock::Mutex<Vec<OplogEntry>>,
        current_idx: async_lock::Mutex<OplogIndex>,
    }

    #[allow(dead_code)]
    impl InMemoryOplog {
        fn new() -> Self {
            Self {
                entries: async_lock::Mutex::new(Vec::new()),
                current_idx: async_lock::Mutex::new(OplogIndex::INITIAL),
            }
        }
    }

    impl Debug for InMemoryOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("InMemoryOplog").finish()
        }
    }

    #[async_trait]
    impl Oplog for InMemoryOplog {
        async fn add(&self, entry: OplogEntry) -> OplogIndex {
            let mut entries = self.entries.lock().await;
            let mut idx = self.current_idx.lock().await;
            *idx = idx.next();
            entries.push(entry);
            *idx
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            0
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            BTreeMap::new()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            *self.current_idx.lock().await
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            None
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            true
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            let entries = self.entries.lock().await;
            let idx: u64 = oplog_index.into();
            entries[(idx - 1) as usize].clone()
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let entries = self.entries.lock().await;
            let start: u64 = oplog_index.into();
            let mut result = BTreeMap::new();
            for i in start..(start + n) {
                if let Some(entry) = entries.get((i - 1) as usize) {
                    result.insert(OplogIndex::from_u64(i), entry.clone());
                }
            }
            result
        }

        async fn length(&self) -> u64 {
            self.entries.lock().await.len() as u64
        }

        async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
            unimplemented!()
        }

        async fn download_raw_payload(
            &self,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unimplemented!()
        }

        async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
    }

    fn test_worker_metadata(
        active_plugins: HashSet<EnvironmentPluginGrantId>,
    ) -> (
        AgentMetadata,
        read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
    ) {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "test-worker".to_string(),
        };
        let environment_id = EnvironmentId::new();
        let account_id = AccountId::new();
        let status = AgentStatusRecord {
            active_plugins,
            ..Default::default()
        };

        let metadata = AgentMetadata {
            agent_id,
            env: vec![],
            environment_id,
            created_by: account_id,
            config_vars: BTreeMap::new(),
            agent_config: Vec::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: status.clone(),
            original_phantom_id: None,
        };

        let status_lock =
            read_only_lock::tokio::ReadOnlyLock::new(Arc::new(tokio::sync::RwLock::new(status)));

        (metadata, status_lock)
    }

    // --------------------------------------------------------------------------
    // U6: Empty buffer → no checkpoint written, no invoke called
    // --------------------------------------------------------------------------

    #[test]
    async fn no_committed_entries_no_send() {
        let grant_id = EnvironmentPluginGrantId::new();
        let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> = Arc::new(
            FakeComponentService::with_one_oplog_processor_plugin(grant_id),
        );
        let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

        let mut state = ForwardingOplogState {
            buffer: VecDeque::new(),
            buffer_start_idx: OplogIndex::INITIAL,
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::NONE,
            last_committed_idx: OplogIndex::NONE,
            components,
            inner,
            plugin_state: HashMap::from([(
                grant_id,
                LivePluginState {
                    target_agent_id: None,
                    confirmed_up_to: OplogIndex::NONE,
                    sending_up_to: OplogIndex::NONE,
                    send_in_progress: false,
                    last_batch_start: OplogIndex::NONE,
                },
            )]),
            pending_direct_commits: BTreeMap::new(),
            worker_event_service: None,
            monitor_tasks: Vec::new(),
        };

        // No committed entries (last_committed_idx = NONE) — try_flush should be a no-op
        state.try_flush().await;

        assert_eq!(
            recording_plugin.send_count().await,
            0,
            "No committed entries should not trigger any plugin sends"
        );
    }

    // --------------------------------------------------------------------------
    // U5 (partial): No active plugins → no send even with entries in buffer
    // --------------------------------------------------------------------------

    #[test]
    async fn no_active_plugins_no_send() {
        let (metadata, status_lock) = test_worker_metadata(HashSet::new());
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> = Arc::new(FakeComponentService::empty());
        let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

        let mut state = ForwardingOplogState {
            buffer: VecDeque::from([OplogEntry::GrowMemory {
                timestamp: Timestamp::now_utc(),
                delta: 100,
            }]),
            buffer_start_idx: OplogIndex::INITIAL,
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::from_u64(1),
            last_committed_idx: OplogIndex::from_u64(1),
            components,
            inner,
            plugin_state: HashMap::new(),
            pending_direct_commits: BTreeMap::new(),
            worker_event_service: None,
            monitor_tasks: Vec::new(),
        };

        state.try_flush().await;

        assert_eq!(
            recording_plugin.send_count().await,
            0,
            "No active plugins should not trigger any plugin sends"
        );
    }

    // --------------------------------------------------------------------------
    // Basic send verification: active plugin + entries → plugin receives them
    // --------------------------------------------------------------------------

    #[test]
    async fn active_plugin_receives_buffered_entries() {
        let grant_id = EnvironmentPluginGrantId::new();
        let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> = Arc::new(
            FakeComponentService::with_one_oplog_processor_plugin(grant_id),
        );
        let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

        // Pre-populate the inner oplog so read_many can return entries
        let entry1 = OplogEntry::GrowMemory {
            timestamp: Timestamp::now_utc(),
            delta: 100,
        };
        let entry2 = OplogEntry::GrowMemory {
            timestamp: Timestamp::now_utc(),
            delta: 200,
        };
        inner.add(entry1.clone()).await;
        inner.add(entry2.clone()).await;

        let mut state = ForwardingOplogState {
            buffer: VecDeque::from([entry1, entry2]),
            buffer_start_idx: OplogIndex::INITIAL,
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::from_u64(2),
            last_committed_idx: OplogIndex::from_u64(2),
            components,
            inner,
            plugin_state: HashMap::from([(
                grant_id,
                LivePluginState {
                    target_agent_id: None,
                    confirmed_up_to: OplogIndex::NONE,
                    sending_up_to: OplogIndex::NONE,
                    send_in_progress: false,
                    last_batch_start: OplogIndex::NONE,
                },
            )]),
            pending_direct_commits: BTreeMap::new(),
            worker_event_service: None,
            monitor_tasks: Vec::new(),
        };

        state.try_flush().await;

        let sends = recording_plugin.sends().await;
        assert_eq!(sends.len(), 1, "Should have sent exactly one batch");
        assert_eq!(sends[0].entry_count, 2, "Batch should contain 2 entries");
    }

    #[test]
    async fn plugin_monitor_looks_up_status_as_original_worker_owner() {
        let grant_id = EnvironmentPluginGrantId::new();
        let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
        let worker_owner = metadata.created_by;
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> = Arc::new(
            FakeComponentService::with_one_oplog_processor_plugin(grant_id),
        );
        let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

        let entry = OplogEntry::GrowMemory {
            timestamp: Timestamp::now_utc(),
            delta: 100,
        };
        inner.add(entry.clone()).await;

        let mut state = ForwardingOplogState {
            buffer: VecDeque::from([entry]),
            buffer_start_idx: OplogIndex::INITIAL,
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::from_u64(1),
            last_committed_idx: OplogIndex::from_u64(1),
            components,
            inner,
            plugin_state: HashMap::from([(
                grant_id,
                LivePluginState {
                    target_agent_id: None,
                    confirmed_up_to: OplogIndex::NONE,
                    sending_up_to: OplogIndex::NONE,
                    send_in_progress: false,
                    last_batch_start: OplogIndex::NONE,
                },
            )]),
            pending_direct_commits: BTreeMap::new(),
            worker_event_service: None,
            monitor_tasks: Vec::new(),
        };

        state.try_flush().await;

        assert_eq!(state.monitor_tasks.len(), 1, "Expected a monitoring task");
        for task in state.monitor_tasks.drain(..) {
            let _ = task.await;
        }

        let lookups = recording_plugin.lookups().await;
        assert_eq!(lookups.len(), 1, "Expected exactly one status lookup");
        assert_eq!(lookups[0].caller_account_id, worker_owner);
    }
}
