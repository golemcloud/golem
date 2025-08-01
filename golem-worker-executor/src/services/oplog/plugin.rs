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
use crate::services::plugins::Plugins;
use crate::services::projects::ProjectService;
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
use crate::services::{
    HasComponentService, HasOplogProcessorPlugin, HasPlugins, HasShardService, HasWorkerActivator,
};
use crate::workerctx::WorkerCtx;
use async_lock::{RwLock, RwLockUpgradableReadGuard};
use async_mutex::Mutex;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::plugin::{
    OplogProcessorDefinition, PluginDefinition, PluginTypeSpecificDefinition,
};
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::{
    AccountId, ComponentId, ComponentVersion, IdempotencyKey, OwnedWorkerId, PluginInstallationId,
    ProjectId, ScanCursor, ShardId, TargetWorkerId, WorkerId, WorkerMetadata,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm_rpc::{IntoValue, Value};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;

#[async_trait]
pub trait OplogProcessorPlugin: Send + Sync {
    async fn send(
        &self,
        worker_metadata: WorkerMetadata,
        plugin_installation_id: &PluginInstallationId,
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
    plugins: Arc<dyn Plugins>,
    project_service: Arc<dyn ProjectService>,
}

type WorkerKey = (ProjectId, String, String);

#[derive(Debug, Clone)]
struct RunningPlugin {
    pub account_id: AccountId,
    pub owned_worker_id: OwnedWorkerId,
    pub configuration: HashMap<String, String>,
    pub component_version: ComponentVersion,
}

impl<Ctx: WorkerCtx> PerExecutorOplogProcessorPlugin<Ctx> {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        shard_service: Arc<dyn ShardService>,
        worker_activator: Arc<dyn WorkerActivator<Ctx>>,
        plugins: Arc<dyn Plugins>,
        project_service: Arc<dyn ProjectService>,
    ) -> Self {
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            component_service,
            shard_service,
            worker_activator,
            plugins,
            project_service,
        }
    }

    async fn resolve_plugin_worker(
        &self,
        project_id: &ProjectId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation_id: &PluginInstallationId,
    ) -> Result<RunningPlugin, WorkerExecutorError> {
        let project_owner = self.project_service.get_project_owner(project_id).await?;
        let (installation, definition) = self
            .plugins
            .get(
                &project_owner,
                component_id,
                component_version,
                plugin_installation_id,
            )
            .await?;

        let workers = self.workers.upgradable_read().await;
        let key = (
            project_id.clone(),
            definition.name.to_string(),
            definition.version.to_string(),
        );
        match workers.get(&key) {
            Some(running_plugin) => Ok(running_plugin.clone()),
            None => {
                let mut workers = RwLockUpgradableReadGuard::upgrade(workers).await;
                match workers.get(&key) {
                    Some(worker_id) => Ok(worker_id.clone()),
                    None => {
                        let (plugin_component_id, plugin_component_version) =
                            Self::get_oplog_processor_component_id(&definition)?;
                        let worker_id = self.generate_worker_id_for(&plugin_component_id).await?;
                        let owned_worker_id = OwnedWorkerId {
                            project_id: project_id.clone(),
                            worker_id: worker_id.clone(),
                        };
                        let running_plugin = RunningPlugin {
                            account_id: project_owner,
                            owned_worker_id: owned_worker_id.clone(),
                            configuration: installation.parameters.clone(),
                            component_version: plugin_component_version,
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
        let target_worker_id = TargetWorkerId {
            component_id: plugin_component_id.clone(),
            worker_name: None,
        };

        let current_assignment = self.shard_service.current_assignment()?;
        let worker_id = target_worker_id.into_worker_id(
            &current_assignment.shard_ids,
            current_assignment.number_of_shards,
        );

        Ok(worker_id)
    }

    fn get_oplog_processor_component_id(
        definition: &PluginDefinition,
    ) -> Result<(ComponentId, ComponentVersion), WorkerExecutorError> {
        match &definition.specs {
            PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
                component_id,
                component_version,
            }) => Ok((component_id.clone(), *component_version)),
            _ => Err(WorkerExecutorError::runtime(
                "Plugin is not an oplog processor",
            )),
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> OplogProcessorPlugin for PerExecutorOplogProcessorPlugin<Ctx> {
    async fn send(
        &self,
        worker_metadata: WorkerMetadata,
        plugin_installation_id: &PluginInstallationId,
        initial_oplog_index: OplogIndex,
        entries: Vec<PublicOplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        let running_plugin = self
            .resolve_plugin_worker(
                &worker_metadata.project_id,
                &worker_metadata.worker_id.component_id,
                worker_metadata.last_known_status.component_version,
                plugin_installation_id,
            )
            .await?;

        let worker = self
            .worker_activator
            .get_or_create_running(
                &running_plugin.account_id,
                &running_plugin.owned_worker_id,
                None,
                None,
                None,
                Some(running_plugin.component_version),
                None,
            )
            .await?;

        let idempotency_key = IdempotencyKey::fresh();

        let (component_id_hi, component_id_lo) =
            worker_metadata.worker_id.component_id.0.as_u64_pair();
        let wave_account_info = format!(
            "{{ account-id: {{ value: \"{}\" }} }}",
            worker_metadata.created_by.value
        );
        let wave_component_id =
            format!("{{ uuid: {{ high-bits: {component_id_hi}, low-bits: {component_id_lo} }} }}");
        let mut wave_config = "[".to_string();
        for (idx, (key, value)) in running_plugin.configuration.iter().enumerate() {
            wave_config.push_str(&format!("( \"{key}\", \"{value}\")"));
            if idx != running_plugin.configuration.len() - 1 {
                wave_config.push_str(", ");
            }
        }
        wave_config.push(']');
        let function_name = format!("golem:api/oplog-processor@1.1.7.{{processor({wave_account_info}, {wave_component_id}, {wave_config}).process}}");

        let val_worker_id = worker_metadata.worker_id.clone().into_value();
        let val_metadata = worker_metadata.into_value();
        let val_first_entry_index = initial_oplog_index.into_value();
        let val_entries = Value::List(
            entries
                .into_iter()
                .map(|entry| entry.into_value())
                .collect(),
        );

        let function_input = vec![
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
            plugins: self.plugins.clone(),
            project_service: self.project_service.clone(),
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

impl<Ctx: WorkerCtx> HasPlugins for PerExecutorOplogProcessorPlugin<Ctx> {
    fn plugins(&self) -> Arc<dyn Plugins> {
        self.plugins.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for PerExecutorOplogProcessorPlugin<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin> {
        Arc::new(self.clone())
    }
}

struct CreateOplogConstructor {
    owned_worker_id: OwnedWorkerId,
    initial_entry: Option<OplogEntry>,
    inner: Arc<dyn OplogService>,
    last_oplog_index: OplogIndex,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    components: Arc<dyn ComponentService>,
    plugins: Arc<dyn Plugins>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
    project_service: Arc<dyn ProjectService>,
}

// We can have clone here independently of whether T is clone due to the Arcs, so deriving
// does the wrong thing here
impl Clone for CreateOplogConstructor {
    fn clone(&self) -> Self {
        Self {
            owned_worker_id: self.owned_worker_id.clone(),
            initial_entry: self.initial_entry.clone(),
            inner: self.inner.clone(),
            last_oplog_index: self.last_oplog_index,
            oplog_plugins: self.oplog_plugins.clone(),
            components: self.components.clone(),
            plugins: self.plugins.clone(),
            execution_status: self.execution_status.clone(),
            initial_worker_metadata: self.initial_worker_metadata.clone(),
            project_service: self.project_service.clone(),
        }
    }
}

impl CreateOplogConstructor {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_entry: Option<OplogEntry>,
        inner: Arc<dyn OplogService>,
        last_oplog_index: OplogIndex,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        plugins: Arc<dyn Plugins>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        initial_worker_metadata: WorkerMetadata,
        project_service: Arc<dyn ProjectService>,
    ) -> Self {
        Self {
            owned_worker_id,
            initial_entry,
            inner,
            last_oplog_index,
            oplog_plugins,
            components,
            plugins,
            execution_status,
            initial_worker_metadata,
            project_service,
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
                    self.execution_status.clone(),
                )
                .await
        } else {
            self.inner
                .open(
                    &self.owned_worker_id,
                    self.last_oplog_index,
                    self.initial_worker_metadata.clone(),
                    self.execution_status.clone(),
                )
                .await
        };

        Arc::new(ForwardingOplog::new(
            inner,
            self.oplog_plugins,
            self.inner,
            self.components,
            self.plugins,
            self.project_service,
            self.execution_status,
            self.initial_worker_metadata,
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
    plugins: Arc<dyn Plugins>,
    project_service: Arc<dyn ProjectService>,
}

impl ForwardingOplogService {
    pub fn new(
        inner: Arc<dyn OplogService>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        plugins: Arc<dyn Plugins>,
        project_service: Arc<dyn ProjectService>,
    ) -> Self {
        Self {
            inner,
            oplogs: OpenOplogs::new("forwarding_oplog_service"),
            oplog_plugins,
            components,
            plugins,
            project_service,
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
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
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
                    self.plugins.clone(),
                    execution_status,
                    initial_worker_metadata,
                    self.project_service.clone(),
                ),
            )
            .await
    }

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: OplogIndex,
        initial_worker_metadata: WorkerMetadata,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
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
                    self.plugins.clone(),
                    execution_status,
                    initial_worker_metadata,
                    self.project_service.clone(),
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
        account_id: &ProjectId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(account_id, component_id, cursor, count)
            .await
    }

    async fn upload_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: &[u8],
    ) -> Result<OplogPayload, String> {
        self.inner.upload_payload(owned_worker_id, data).await
    }

    async fn download_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload: &OplogPayload,
    ) -> Result<Bytes, String> {
        self.inner.download_payload(owned_worker_id, payload).await
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
        plugins: Arc<dyn Plugins>,
        project_service: Arc<dyn ProjectService>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        initial_worker_metadata: WorkerMetadata,
        last_oplog_idx: OplogIndex,
        close_fn: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let state = Arc::new(Mutex::new(ForwardingOplogState {
            buffer: VecDeque::new(),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins,
            execution_status,
            initial_worker_metadata,
            last_oplog_idx,
            oplog_service,
            components,
            plugins,
            project_service,
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
    async fn add(&self, entry: OplogEntry) {
        let mut state = self.state.lock().await;
        state.buffer.push_back(entry.clone());
        state.last_oplog_idx = state.last_oplog_idx.next();
        self.inner.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.inner.drop_prefix(last_dropped_id).await
    }

    async fn commit(&self, level: CommitLevel) {
        let mut state = self.state.lock().await;
        self.inner.commit(level).await;
        state.commit_count += 1;
        if state.commit_count > Self::MAX_COMMIT_COUNT {
            state.send_buffer().await;
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.inner.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.inner.read(oplog_index).await
    }

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        self.inner.upload_payload(data).await
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        self.inner.download_payload(payload).await
    }
}

struct ForwardingOplogState {
    buffer: VecDeque<OplogEntry>,
    commit_count: usize,
    last_send: Instant,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
    last_oplog_idx: OplogIndex,
    oplog_service: Arc<dyn OplogService>,
    components: Arc<dyn ComponentService>,
    plugins: Arc<dyn Plugins>,
    project_service: Arc<dyn ProjectService>,
}

impl ForwardingOplogState {
    pub async fn send_buffer(&mut self) {
        let metadata = {
            let execution_status = self.execution_status.read().unwrap();
            let status = execution_status.last_known_status().clone();
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

        for entry in entries {
            let public_entry = PublicOplogEntry::from_oplog_entry(
                entry.clone(),
                self.oplog_service.clone(),
                self.components.clone(),
                self.plugins.clone(),
                self.project_service.clone(),
                &metadata.owned_worker_id(),
                metadata.last_known_status.component_version, // NOTE: this is only safe if the component version is not changing within one batch
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed to enrich oplog entry for oplog processors: {err}"
                ))
            })?;

            public_entries.push(public_entry);
        }

        for installation_id in metadata.last_known_status.active_plugins.iter() {
            self.oplog_plugins
                .send(
                    metadata.clone(),
                    installation_id,
                    initial_oplog_index,
                    public_entries.clone(),
                )
                .await?;
        }

        Ok(())
    }
}
