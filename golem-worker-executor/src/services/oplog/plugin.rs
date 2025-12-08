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

use crate::model::public_oplog::PublicOplogEntryOps;
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
use golem_common::model::component::{ComponentId, ComponentRevision, InstalledPlugin};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, PublicOplogEntry, RawOplogPayload,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{
    IdempotencyKey, OwnedWorkerId, ScanCursor, ShardId, WorkerId, WorkerMetadata,
    WorkerStatusRecord,
};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::{IntoValue, Value};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;
use uuid::Uuid;

#[async_trait]
pub trait OplogProcessorPlugin: Send + Sync {
    async fn send(
        &self,
        worker_metadata: WorkerMetadata,
        plugin: &InstalledPlugin,
        initial_oplog_index: OplogIndex,
        entries: Vec<PublicOplogEntry>,
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
    pub component_version: ComponentRevision,
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

                        let worker_id = self.generate_worker_id_for(&plugin_component_id).await?;
                        let plugin_component = self
                            .component_service
                            .get_metadata(&plugin_component_id, Some(plugin_component_revision))
                            .await?;
                        let owned_worker_id = OwnedWorkerId {
                            environment_id,
                            worker_id: worker_id.clone(),
                        };
                        let running_plugin = RunningPlugin {
                            account_id: plugin_component.account_id,
                            owned_worker_id: owned_worker_id.clone(),
                            configuration: plugin.parameters.clone(),
                            component_version: plugin_component_revision,
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
        plugin_component_id: &ComponentId,
    ) -> Result<WorkerId, WorkerExecutorError> {
        let current_assignment = self.shard_service.current_assignment()?;
        let worker_id = Self::generate_local_worker_id(
            *plugin_component_id,
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
        entries: Vec<PublicOplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        let running_plugin = self
            .resolve_plugin_worker(worker_metadata.environment_id, plugin)
            .await?;

        let worker = self
            .worker_activator
            .get_or_create_running(
                &running_plugin.account_id,
                &running_plugin.owned_worker_id,
                None,
                None,
                Some(running_plugin.component_version),
                None,
                &InvocationContextStack::fresh(),
            )
            .await?;

        let idempotency_key = IdempotencyKey::fresh();

        let val_account_info = Value::Record(vec![worker_metadata.created_by.into_value()]);
        let val_component_id = worker_metadata.worker_id.component_id.into_value();
        let mut config_pairs = Vec::new();
        for (key, value) in running_plugin.configuration.iter() {
            config_pairs.push(Value::Tuple(vec![
                key.clone().into_value(),
                value.clone().into_value(),
            ]));
        }
        let val_config = Value::List(config_pairs);
        let function_name = "golem:api/oplog-processor@1.3.0.{process}".to_string();

        let val_worker_id = worker_metadata.worker_id.clone().into_value();
        let agent_metadata_for_guests: AgentMetadataForGuests = worker_metadata.into();
        let val_metadata = agent_metadata_for_guests.into_value();
        let val_first_entry_index = initial_oplog_index.into_value();
        let val_entries = Value::List(
            entries
                .into_iter()
                .map(|entry| entry.into_value())
                .collect(),
        );

        let function_input = vec![
            val_account_info,
            val_config,
            val_component_id,
            val_worker_id,
            val_metadata,
            val_first_entry_index,
            val_entries,
        ];

        worker
            .invoke(
                idempotency_key,
                function_name,
                function_input,
                InvocationContextStack::fresh(),
            )
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
    last_oplog_index: OplogIndex,
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
        last_oplog_index: OplogIndex,
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
                    self.last_oplog_index,
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        };

        Arc::new(ForwardingOplog::new(
            inner,
            self.oplog_plugins,
            self.inner,
            self.components,
            self.initial_worker_metadata,
            self.last_known_status,
            self.last_oplog_index,
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
                    OplogIndex::INITIAL,
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
        last_oplog_index: OplogIndex,
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
        oplog_service: Arc<dyn OplogService>,
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
            oplog_service,
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
    oplog_service: Arc<dyn OplogService>,
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
        let mut public_entries = Vec::new();

        for (delta, entry) in entries.iter().enumerate() {
            let idx = initial_oplog_index.range_end(delta as u64 + 1);
            let public_entry = PublicOplogEntry::from_oplog_entry(
                idx,
                entry.clone(),
                self.oplog_service.clone(),
                self.components.clone(),
                &metadata.owned_worker_id(),
                metadata.last_known_status.component_revision, // NOTE: this is only safe if the component version is not changing within one batch
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed to enrich oplog entry for oplog processors: {err}"
                ))
            })?;

            public_entries.push(public_entry);
        }

        if !metadata.last_known_status.active_plugins.is_empty() {
            let component_metadata = self
                .components
                .get_metadata(
                    &metadata.owned_worker_id().component_id(),
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
                        public_entries.clone(),
                    )
                    .await?;
            }
        }

        Ok(())
    }
}
