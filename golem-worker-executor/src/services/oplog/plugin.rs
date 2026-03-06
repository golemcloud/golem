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
use crate::services::component::ComponentService;
use crate::services::oplog::{CommitLevel, OpenOplogs, Oplog, OplogConstructor, OplogService};
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
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
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{
    AgentInvocation, IdempotencyKey, OwnedWorkerId, ScanCursor, ShardId, WorkerId, WorkerMetadata,
    WorkerStatusRecord,
};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;
use uuid::{uuid, Uuid};

#[async_trait]
pub trait OplogProcessorPlugin: Send + Sync {
    async fn send(
        &self,
        worker_metadata: WorkerMetadata,
        plugin: &InstalledPlugin,
        initial_oplog_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), WorkerExecutorError>;

    async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError>;
}

/// An implementation of the `OplogProcessorPlugin` trait that runs a single instance of each
/// used plugin on each worker executor node.
pub struct PerExecutorOplogProcessorPlugin<Ctx: WorkerCtx> {
    workers: Arc<RwLock<HashMap<WorkerKey, RunningPlugin>>>,
    component_service: Arc<dyn ComponentService>,
    shard_service: Arc<dyn ShardService>,
    worker_activator: Arc<dyn WorkerActivator<Ctx>>,
}

type WorkerKey = (EnvironmentId, PluginRegistrationId);

#[derive(Debug, Clone)]
struct RunningPlugin {
    pub account_id: AccountId,
    pub owned_worker_id: OwnedWorkerId,
    pub configuration: BTreeMap<String, String>,
    pub component_revision: ComponentRevision,
}

impl<Ctx: WorkerCtx> PerExecutorOplogProcessorPlugin<Ctx> {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        shard_service: Arc<dyn ShardService>,
        worker_activator: Arc<dyn WorkerActivator<Ctx>>,
    ) -> Self {
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            component_service,
            shard_service,
            worker_activator,
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
                    Some(worker_id) => Ok(worker_id.clone()),
                    None => {
                        let plugin_component_id = plugin
                            .oplog_processor_component_id
                            .ok_or(anyhow!("missing oplog processor plugin component id"))?;
                        let plugin_component_revision =
                            plugin.oplog_processor_component_revision.ok_or(anyhow!(
                                "missing oplog processor plugin component revision"
                            ))?;

                        let worker_id = self.generate_worker_id_for(plugin_component_id).await?;
                        let plugin_component = self
                            .component_service
                            .get_metadata(plugin_component_id, Some(plugin_component_revision))
                            .await?;
                        let owned_worker_id = OwnedWorkerId {
                            environment_id,
                            worker_id: worker_id.clone(),
                        };
                        let running_plugin = RunningPlugin {
                            account_id: plugin_component.account_id,
                            owned_worker_id: owned_worker_id.clone(),
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

    async fn generate_worker_id_for(
        &self,
        plugin_component_id: ComponentId,
    ) -> Result<WorkerId, WorkerExecutorError> {
        let current_assignment = self.shard_service.current_assignment()?;
        let worker_id = Self::generate_local_worker_id(
            plugin_component_id,
            &current_assignment.shard_ids,
            current_assignment.number_of_shards,
        );

        Ok(worker_id)
    }

    /// Converts a `TargetWorkerId` to a `WorkerId`. If the worker name was not specified,
    /// it generates a new unique one, and if the `force_in_shard` set is not empty, it guarantees
    /// that the generated worker ID will belong to one of the provided shards.
    ///
    /// If the worker name was specified, `force_in_shard` is ignored.
    fn generate_local_worker_id(
        component_id: ComponentId,
        force_in_shard: &HashSet<ShardId>,
        number_of_shards: usize,
    ) -> WorkerId {
        if force_in_shard.is_empty() || number_of_shards == 0 {
            let worker_name = Uuid::new_v4().to_string();
            WorkerId {
                component_id,
                worker_name,
            }
        } else {
            let mut current = Uuid::new_v4().to_u128_le();
            loop {
                let uuid = Uuid::from_u128_le(current);
                let worker_name = uuid.to_string();
                let worker_id = WorkerId {
                    component_id,
                    worker_name,
                };
                let shard_id = ShardId::from_worker_id(&worker_id, number_of_shards);
                if force_in_shard.contains(&shard_id) {
                    return worker_id;
                }
                current += 1;
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> OplogProcessorPlugin for PerExecutorOplogProcessorPlugin<Ctx> {
    async fn send(
        &self,
        worker_metadata: WorkerMetadata,
        plugin: &InstalledPlugin,
        initial_oplog_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        let running_plugin = self
            .resolve_plugin_worker(worker_metadata.environment_id, plugin)
            .await?;

        let worker = self
            .worker_activator
            .get_or_create_running(
                running_plugin.account_id,
                &running_plugin.owned_worker_id,
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
            &worker_metadata.owned_worker_id().worker_id,
            &plugin.environment_plugin_grant_id,
            initial_oplog_index,
            batch_last_index,
        );

        let account_id = worker_metadata.created_by;
        worker
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

    async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError> {
        let new_assignment = self.shard_service.current_assignment()?;

        let mut workers = self.workers.write().await;
        let keys = workers.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let entry = workers.entry(key);
            match entry {
                Entry::Occupied(entry) => {
                    let shard_id = ShardId::from_worker_id(
                        &entry.get().owned_worker_id.worker_id,
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
}

impl<Ctx: WorkerCtx> Clone for PerExecutorOplogProcessorPlugin<Ctx> {
    fn clone(&self) -> Self {
        Self {
            workers: self.workers.clone(),
            component_service: self.component_service.clone(),
            shard_service: self.shard_service.clone(),
            worker_activator: self.worker_activator.clone(),
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
    owned_worker_id: OwnedWorkerId,
    initial_entry: Option<OplogEntry>,
    inner: Arc<dyn OplogService>,
    last_oplog_index: Option<OplogIndex>,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    components: Arc<dyn ComponentService>,
    initial_worker_metadata: WorkerMetadata,
    last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
    execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
}

impl CreateOplogConstructor {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_entry: Option<OplogEntry>,
        inner: Arc<dyn OplogService>,
        last_oplog_index: Option<OplogIndex>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Self {
        Self {
            owned_worker_id,
            initial_entry,
            inner,
            last_oplog_index,
            oplog_plugins,
            components,
            initial_worker_metadata,
            last_known_status,
            execution_status,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(self, close: Box<dyn FnOnce() + Send + Sync>) -> Arc<dyn Oplog> {
        let last_oplog_index = match self.last_oplog_index {
            Some(idx) => idx,
            None => self.inner.get_last_index(&self.owned_worker_id).await,
        };
        let inner = if let Some(initial_entry) = self.initial_entry {
            self.inner
                .create(
                    &self.owned_worker_id,
                    initial_entry,
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        } else {
            self.inner
                .open(
                    &self.owned_worker_id,
                    Some(last_oplog_index),
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        };

        Arc::new(ForwardingOplog::new(
            inner,
            self.oplog_plugins,
            self.components,
            self.initial_worker_metadata,
            self.last_known_status,
            last_oplog_index,
            close,
        ))
    }
}

pub struct ForwardingOplogService {
    pub inner: Arc<dyn OplogService>,
    oplogs: OpenOplogs,

    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    components: Arc<dyn ComponentService>,
}

impl ForwardingOplogService {
    pub fn new(
        inner: Arc<dyn OplogService>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
    ) -> Self {
        Self {
            inner,
            oplogs: OpenOplogs::new("forwarding_oplog_service"),
            oplog_plugins,
            components,
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
        owned_worker_id: &OwnedWorkerId,
        initial_entry: OplogEntry,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateOplogConstructor::new(
                    owned_worker_id.clone(),
                    Some(initial_entry),
                    self.inner.clone(),
                    Some(OplogIndex::INITIAL),
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                ),
            )
            .await
    }

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateOplogConstructor::new(
                    owned_worker_id.clone(),
                    None,
                    self.inner.clone(),
                    last_oplog_index,
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                ),
            )
            .await
    }

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        self.inner.get_last_index(owned_worker_id).await
    }

    async fn delete(&self, owned_worker_id: &OwnedWorkerId) {
        self.inner.delete(owned_worker_id).await
    }

    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read(owned_worker_id, idx, n).await
    }

    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool {
        self.inner.exists(owned_worker_id).await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, cursor, count)
            .await
    }

    async fn upload_raw_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(owned_worker_id, data).await
    }

    async fn download_raw_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .download_raw_payload(owned_worker_id, payload_id, md5_hash)
            .await
    }
}

/// A wrapper for `Oplog` that periodically sends buffered oplog entries to oplog processor plugins
pub struct ForwardingOplog {
    inner: Arc<dyn Oplog>,
    state: Arc<Mutex<ForwardingOplogState>>,
    timer: Option<JoinHandle<()>>,
    close_fn: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl ForwardingOplog {
    const MAX_COMMIT_COUNT: usize = 3;

    pub fn new(
        inner: Arc<dyn Oplog>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        last_oplog_idx: OplogIndex,
        close_fn: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let state = Arc::new(Mutex::new(ForwardingOplogState {
            buffer: VecDeque::new(),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins,
            initial_worker_metadata,
            last_known_status,
            last_oplog_idx,
            components,
        }));

        let timer = tokio::spawn({
            let state = state.clone();
            async move {
                const MAX_ELAPSED_TIME: Duration = Duration::from_secs(5);
                loop {
                    tokio::time::sleep(MAX_ELAPSED_TIME).await;
                    let mut state = state.lock().await;
                    if !state.buffer.is_empty() && state.last_send.elapsed() > MAX_ELAPSED_TIME {
                        state.send_buffer().await;
                    }
                }
            }
            .in_current_span()
        });
        Self {
            inner,
            state,
            timer: Some(timer),
            close_fn: Some(close_fn),
        }
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
        let result = self.inner.commit(level).await;
        state.commit_count += 1;
        if state.commit_count > Self::MAX_COMMIT_COUNT {
            state.send_buffer().await;
        }
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
}

struct ForwardingOplogState {
    buffer: VecDeque<OplogEntry>,
    commit_count: usize,
    last_send: Instant,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    initial_worker_metadata: WorkerMetadata,
    last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
    last_oplog_idx: OplogIndex,
    components: Arc<dyn ComponentService>,
}

impl ForwardingOplogState {
    pub async fn send_buffer(&mut self) {
        let metadata = {
            let status = self.last_known_status.read().await.clone();
            WorkerMetadata {
                last_known_status: status,
                ..self.initial_worker_metadata.clone()
            }
        };

        if !metadata.last_known_status.active_plugins.is_empty() {
            let entries: Vec<_> = self.buffer.drain(..).collect();
            let initial_oplog_index =
                OplogIndex::from_u64(Into::<u64>::into(self.last_oplog_idx) - entries.len() as u64);

            if let Err(err) = self
                .try_send_entries(metadata, initial_oplog_index, &entries)
                .await
            {
                log::error!("Failed to send oplog entries: {err}");
                // In case of an error we keep the unsent entries in the buffer.
                // This does not guarantee that we don't double-send entries (in case the error happened
                // only for one of the `send` calls, for example) - this is going to be handled
                // better in future versions where the last known oplog index will be tracked for
                // each active plugin.
                self.buffer.extend(entries);
            } else {
                self.last_send = Instant::now();
                self.commit_count = 0;
            }
        } else {
            // If there are no active plugins we just reset the state
            self.last_send = Instant::now();
            self.commit_count = 0;
        }
    }

    async fn try_send_entries(
        &self,
        metadata: WorkerMetadata,
        initial_oplog_index: OplogIndex,
        entries: &[OplogEntry],
    ) -> Result<(), WorkerExecutorError> {
        if !metadata.last_known_status.active_plugins.is_empty() {
            let component_metadata = self
                .components
                .get_metadata(
                    metadata.owned_worker_id().component_id(),
                    Some(metadata.last_known_status.component_revision),
                )
                .await?;

            let plugins_to_send_to = component_metadata
                .installed_plugins
                .into_iter()
                .filter(|p| {
                    metadata
                        .last_known_status
                        .active_plugins
                        .contains(&p.priority)
                });

            for plugin in plugins_to_send_to {
                self.oplog_plugins
                    .send(
                        metadata.clone(),
                        &plugin,
                        initial_oplog_index,
                        entries.to_vec(),
                    )
                    .await?;
            }
        }

        Ok(())
    }
}

const OPLOG_PROC_NS: Uuid = uuid!("A7E3F1B2-8C4D-5E6F-9A0B-1C2D3E4F5A6B");

fn oplog_processor_idempotency_key(
    source_worker_id: &WorkerId,
    plugin_installation_id: &EnvironmentPluginGrantId,
    batch_first_index: OplogIndex,
    batch_last_index: OplogIndex,
) -> IdempotencyKey {
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(b"oplog-proc-v1\0");
    buf.extend_from_slice(source_worker_id.component_id.0.as_bytes());
    let worker_name = source_worker_id.worker_name.as_bytes();
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
    use golem_common::model::account::AccountId;
    use golem_common::model::application::ApplicationId;
    use golem_common::model::component::{
        ComponentId, ComponentName, ComponentRevision, InstalledPlugin, PluginPriority,
    };
    use golem_common::model::diff;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::model::oplog::PersistenceLevel;
    use golem_common::model::plugin_registration::PluginRegistrationId;
    use golem_common::model::{Timestamp, WorkerMetadata, WorkerStatusRecord};
    use golem_common::read_only_lock;
    use golem_service_base::model::component::Component;
    use golem_common::model::component_metadata::ComponentMetadata;
    use test_r::test;

    // --------------------------------------------------------------------------
    // U1: Deterministic idempotency key stability
    // --------------------------------------------------------------------------

    #[test]
    fn deterministic_idempotency_key_same_inputs_produce_same_key() {
        let worker_id = WorkerId {
            component_id: ComponentId::new(),
            worker_name: "test-worker".to_string(),
        };
        let plugin_id = EnvironmentPluginGrantId::new();
        let first = OplogIndex::from_u64(10);
        let last = OplogIndex::from_u64(20);

        let key1 = oplog_processor_idempotency_key(&worker_id, &plugin_id, first, last);
        let key2 = oplog_processor_idempotency_key(&worker_id, &plugin_id, first, last);

        assert_eq!(key1, key2, "Same inputs must produce the same idempotency key");
    }

    #[test]
    fn deterministic_idempotency_key_different_batch_range_produces_different_key() {
        let worker_id = WorkerId {
            component_id: ComponentId::new(),
            worker_name: "test-worker".to_string(),
        };
        let plugin_id = EnvironmentPluginGrantId::new();

        let key1 = oplog_processor_idempotency_key(
            &worker_id,
            &plugin_id,
            OplogIndex::from_u64(10),
            OplogIndex::from_u64(20),
        );
        let key2 = oplog_processor_idempotency_key(
            &worker_id,
            &plugin_id,
            OplogIndex::from_u64(21),
            OplogIndex::from_u64(30),
        );

        assert_ne!(key1, key2, "Different batch ranges must produce different keys");
    }

    #[test]
    fn deterministic_idempotency_key_different_worker_produces_different_key() {
        let component_id = ComponentId::new();
        let worker_id1 = WorkerId {
            component_id,
            worker_name: "worker-a".to_string(),
        };
        let worker_id2 = WorkerId {
            component_id,
            worker_name: "worker-b".to_string(),
        };
        let plugin_id = EnvironmentPluginGrantId::new();
        let first = OplogIndex::from_u64(1);
        let last = OplogIndex::from_u64(5);

        let key1 = oplog_processor_idempotency_key(&worker_id1, &plugin_id, first, last);
        let key2 = oplog_processor_idempotency_key(&worker_id2, &plugin_id, first, last);

        assert_ne!(key1, key2, "Different workers must produce different keys");
    }

    #[test]
    fn deterministic_idempotency_key_different_plugin_produces_different_key() {
        let worker_id = WorkerId {
            component_id: ComponentId::new(),
            worker_name: "test-worker".to_string(),
        };
        let plugin_id1 = EnvironmentPluginGrantId::new();
        let plugin_id2 = EnvironmentPluginGrantId::new();
        let first = OplogIndex::from_u64(1);
        let last = OplogIndex::from_u64(5);

        let key1 = oplog_processor_idempotency_key(&worker_id, &plugin_id1, first, last);
        let key2 = oplog_processor_idempotency_key(&worker_id, &plugin_id2, first, last);

        assert_ne!(key1, key2, "Different plugins must produce different keys");
    }

    // --------------------------------------------------------------------------
    // Helpers: recording mock for OplogProcessorPlugin and fake ComponentService
    // --------------------------------------------------------------------------

    /// Records all `send()` calls for verification
    struct RecordingOplogProcessorPlugin {
        sends: async_lock::Mutex<Vec<RecordedSend>>,
    }

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct RecordedSend {
        initial_oplog_index: OplogIndex,
        entry_count: usize,
    }

    impl RecordingOplogProcessorPlugin {
        fn new() -> Self {
            Self {
                sends: async_lock::Mutex::new(Vec::new()),
            }
        }

        async fn send_count(&self) -> usize {
            self.sends.lock().await.len()
        }

        async fn sends(&self) -> Vec<RecordedSend> {
            self.sends.lock().await.clone()
        }
    }

    #[async_trait]
    impl OplogProcessorPlugin for RecordingOplogProcessorPlugin {
        async fn send(
            &self,
            _worker_metadata: WorkerMetadata,
            _plugin: &InstalledPlugin,
            initial_oplog_index: OplogIndex,
            entries: Vec<OplogEntry>,
        ) -> Result<(), WorkerExecutorError> {
            self.sends.lock().await.push(RecordedSend {
                initial_oplog_index,
                entry_count: entries.len(),
            });
            Ok(())
        }

        async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError> {
            Ok(())
        }
    }

    /// A fake ComponentService that returns a component with one installed oplog processor plugin
    struct FakeComponentService {
        installed_plugins: Vec<InstalledPlugin>,
    }

    impl FakeComponentService {
        fn with_one_oplog_processor_plugin(priority: PluginPriority) -> Self {
            Self {
                installed_plugins: vec![InstalledPlugin {
                    environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
                    priority,
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
                local_agent_config: Vec::new(),
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
        active_plugins: HashSet<PluginPriority>,
    ) -> (WorkerMetadata, read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>) {
        let worker_id = WorkerId {
            component_id: ComponentId::new(),
            worker_name: "test-worker".to_string(),
        };
        let environment_id = EnvironmentId::new();
        let account_id = AccountId::new();
        let status = WorkerStatusRecord {
            active_plugins,
            ..Default::default()
        };

        let metadata = WorkerMetadata {
            worker_id,
            env: vec![],
            environment_id,
            created_by: account_id,
            config_vars: BTreeMap::new(),
            local_agent_config: Vec::new(),
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
    async fn empty_buffer_no_send() {
        let plugin_priority = PluginPriority(0);
        let (metadata, status_lock) =
            test_worker_metadata(HashSet::from([plugin_priority]));
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> =
            Arc::new(FakeComponentService::with_one_oplog_processor_plugin(plugin_priority));

        let mut state = ForwardingOplogState {
            buffer: VecDeque::new(),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::INITIAL,
            components,
        };

        // Buffer is empty — send_buffer should be a no-op
        state.send_buffer().await;

        assert_eq!(
            recording_plugin.send_count().await,
            0,
            "Empty buffer should not trigger any plugin sends"
        );
    }

    // --------------------------------------------------------------------------
    // U5 (partial): No active plugins → no send even with entries in buffer
    // --------------------------------------------------------------------------

    #[test]
    async fn no_active_plugins_no_send() {
        let (metadata, status_lock) = test_worker_metadata(HashSet::new());
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> =
            Arc::new(FakeComponentService::empty());

        let mut state = ForwardingOplogState {
            buffer: VecDeque::from([OplogEntry::GrowMemory {
                timestamp: Timestamp::now_utc(),
                delta: 100,
            }]),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::from_u64(1),
            components,
        };

        state.send_buffer().await;

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
        let plugin_priority = PluginPriority(0);
        let (metadata, status_lock) =
            test_worker_metadata(HashSet::from([plugin_priority]));
        let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let components: Arc<dyn ComponentService> =
            Arc::new(FakeComponentService::with_one_oplog_processor_plugin(plugin_priority));

        let mut state = ForwardingOplogState {
            buffer: VecDeque::from([
                OplogEntry::GrowMemory {
                    timestamp: Timestamp::now_utc(),
                    delta: 100,
                },
                OplogEntry::GrowMemory {
                    timestamp: Timestamp::now_utc(),
                    delta: 200,
                },
            ]),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins: recording_plugin.clone(),
            initial_worker_metadata: metadata,
            last_known_status: status_lock,
            last_oplog_idx: OplogIndex::from_u64(2),
            components,
        };

        state.send_buffer().await;

        let sends = recording_plugin.sends().await;
        assert_eq!(sends.len(), 1, "Should have sent exactly one batch");
        assert_eq!(sends[0].entry_count, 2, "Batch should contain 2 entries");
    }
}
