// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::error::GolemError;
use crate::model::public_oplog::PublicOplogEntryOps;
use crate::model::ExecutionStatus;
use crate::preview2::golem;
use crate::services::component::ComponentService;
use crate::services::events::Events;
use crate::services::file_loader::FileLoader;
use crate::services::oplog::{CommitLevel, Oplog, OplogService};
use crate::services::plugins::Plugins;
use crate::services::rpc::Rpc;
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{
    active_workers, blob_store, component, golem_config, key_value, oplog, promise, rpc, scheduler,
    shard, shard_manager, worker, worker_activator, worker_enumeration, worker_proxy, All,
    HasActiveWorkers, HasBlobStoreService, HasComponentService, HasConfig, HasEvents, HasExtraDeps,
    HasFileLoader, HasKeyValueService, HasOplogProcessorPlugin, HasOplogService, HasPlugins,
    HasPromiseService, HasRpc, HasRunningWorkerEnumerationService, HasSchedulerService,
    HasShardManagerService, HasShardService, HasWasmtimeEngine, HasWorkerActivator,
    HasWorkerEnumerationService, HasWorkerProxy, HasWorkerService,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_lock::{RwLock, RwLockUpgradableReadGuard};
use async_mutex::Mutex;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::component::ComponentOwner;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::plugin::{
    OplogProcessorDefinition, PluginDefinition, PluginOwner, PluginScope,
    PluginTypeSpecificDefinition,
};
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::{
    AccountId, ComponentId, ComponentVersion, IdempotencyKey, OwnedWorkerId, PluginInstallationId,
    ShardId, TargetWorkerId, WorkerId, WorkerMetadata,
};
use golem_wasm_rpc::{IntoValue, Value};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use wasmtime::component::Lower;

#[async_trait]
pub trait OplogProcessorPlugin {
    async fn send(
        &self,
        worker_metadata: WorkerMetadata,
        plugin_installation_id: &PluginInstallationId,
        initial_oplog_index: OplogIndex,
        entries: Vec<PublicOplogEntry>,
    ) -> Result<(), GolemError>;

    async fn on_shard_assignment_changed(&self) -> Result<(), GolemError>;
}

/// An implementation of the `OplogProcessorPlugin` trait that runs a single instance of each
/// used plugin on each worker executor node.
struct PerExecutorOplogProcessorPlugin<Ctx: WorkerCtx> {
    workers: Arc<RwLock<HashMap<WorkerKey, RunningPlugin>>>,

    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    component_service: Arc<dyn ComponentService + Send + Sync>,
    shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    shard_service: Arc<dyn ShardService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    rpc: Arc<dyn Rpc + Send + Sync>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
    worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
    events: Arc<Events>,
    file_loader: Arc<FileLoader>,
    plugins: Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    >,
    extra_deps: Ctx::ExtraDeps,
}

type WorkerKey = (AccountId, String, String);

#[derive(Debug, Clone)]
struct RunningPlugin {
    pub owned_worker_id: OwnedWorkerId,
    pub configuration: HashMap<String, String>,
    pub component_version: ComponentVersion,
}

impl<Ctx: WorkerCtx> PerExecutorOplogProcessorPlugin<Ctx> {
    pub fn new(
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<
            dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
                + Send
                + Sync,
        >,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            shard_manager_service,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            rpc,
            scheduler_service,
            worker_activator,
            worker_proxy,
            events,
            file_loader,
            plugins,
            extra_deps,
        }
    }

    async fn resolve_plugin_worker(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation_id: &PluginInstallationId,
    ) -> Result<RunningPlugin, GolemError> {
        let (installation, definition) = self
            .plugins
            .get(
                account_id,
                component_id,
                component_version,
                plugin_installation_id,
            )
            .await?;

        let workers = self.workers.upgradable_read().await;
        let key = (
            account_id.clone(),
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
                        let plugin_component_id =
                            Self::get_oplog_processor_component_id(&definition)?;
                        let worker_id = self.generate_worker_id_for(&plugin_component_id).await?;
                        let owned_worker_id = OwnedWorkerId {
                            account_id: account_id.clone(),
                            worker_id: worker_id.clone(),
                        };
                        let running_plugin = RunningPlugin {
                            owned_worker_id: owned_worker_id.clone(),
                            configuration: installation.parameters.clone(),
                            component_version: component_version.clone(),
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
    ) -> Result<WorkerId, GolemError> {
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
        definition: &PluginDefinition<
            <Ctx::ComponentOwner as ComponentOwner>::PluginOwner,
            Ctx::PluginScope,
        >,
    ) -> Result<ComponentId, GolemError> {
        match &definition.specs {
            PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
                component_id,
                ..
            }) => Ok(component_id.clone()),
            _ => Err(GolemError::runtime("Plugin is not an oplog processor")),
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
    ) -> Result<(), GolemError> {
        let running_plugin = self
            .resolve_plugin_worker(
                &worker_metadata.account_id,
                &worker_metadata.worker_id.component_id,
                worker_metadata.last_known_status.component_version,
                plugin_installation_id,
            )
            .await?;

        let worker = Worker::get_or_create_running(
            self,
            &running_plugin.owned_worker_id,
            None,
            None,
            Some(running_plugin.component_version),
            None,
        )
        .await?;

        let idempotency_key = IdempotencyKey::fresh();

        let (component_id_hi, component_id_lo) =
            worker_metadata.worker_id.component_id.0.as_u64_pair();
        let wave_account_info =
            format!("{{ account-id: \"{}\" }}", worker_metadata.account_id.value);
        let wave_component_id = format!(
            "{{ high-bits: {}, low-bits: {} }}",
            component_id_hi, component_id_lo
        );
        let mut wave_config = "[".to_string();
        for (idx, (key, value)) in running_plugin.configuration.iter().enumerate() {
            wave_config.push_str(&format!("( \"{}\", \"{}\")", key, value));
            if idx != running_plugin.configuration.len() - 1 {
                wave_config.push_str(", ");
            }
        }
        wave_config.push_str("]");
        let function_name = format!("golem:api/oplog-processor@1.1.0-rc1.{{processor({wave_account_info}, {wave_component_id}, {wave_config}).process}}");

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
            .invoke(idempotency_key, function_name, function_input)
            .await?;

        Ok(())
    }

    async fn on_shard_assignment_changed(&self) -> Result<(), GolemError> {
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
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            oplog_service: self.oplog_service.clone(),
            rpc: self.rpc.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            worker_proxy: self.worker_proxy.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            plugins: self.plugins.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> HasEvents for PerExecutorOplogProcessorPlugin<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
    }
}

impl<Ctx: WorkerCtx> HasActiveWorkers<Ctx> for PerExecutorOplogProcessorPlugin<Ctx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.active_workers.clone()
    }
}

impl<Ctx: WorkerCtx> HasComponentService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for PerExecutorOplogProcessorPlugin<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService + Send + Sync> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync> {
        self.running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService + Send + Sync> {
        self.promise_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWasmtimeEngine<Ctx> for PerExecutorOplogProcessorPlugin<Ctx> {
    fn engine(&self) -> Arc<wasmtime::Engine> {
        self.engine.clone()
    }

    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>> {
        self.linker.clone()
    }

    fn runtime(&self) -> Handle {
        self.runtime.clone()
    }
}

impl<Ctx: WorkerCtx> HasKeyValueService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService + Send + Sync> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService + Send + Sync> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService + Send + Sync> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRpc for PerExecutorOplogProcessorPlugin<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.rpc.clone()
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for PerExecutorOplogProcessorPlugin<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn shard_service(&self) -> Arc<dyn ShardService + Send + Sync> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardManagerService for PerExecutorOplogProcessorPlugin<Ctx> {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService + Send + Sync> {
        self.shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator for PerExecutorOplogProcessorPlugin<Ctx> {
    fn worker_activator(&self) -> Arc<dyn WorkerActivator + Send + Sync> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for PerExecutorOplogProcessorPlugin<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx> HasFileLoader for PerExecutorOplogProcessorPlugin<Ctx> {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.file_loader.clone()
    }
}

impl<Ctx: WorkerCtx>
    HasPlugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
    for PerExecutorOplogProcessorPlugin<Ctx>
{
    fn plugins(
        &self,
    ) -> Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    > {
        self.plugins.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for PerExecutorOplogProcessorPlugin<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin + Send + Sync> {
        Arc::new(self.clone())
    }
}

/// A wrapper for `Oplog` that periodically sends buffered oplog entries to oplog processor plugins
#[derive(Debug)]
pub struct ForwardingOplog<
    Inner: Oplog + Send + Sync + 'static,
    Owner: PluginOwner,
    Scope: PluginScope,
> {
    inner: Inner,
    state: Arc<Mutex<ForwardingOplogState<Owner, Scope>>>,
    timer: Option<JoinHandle<()>>,
}

impl<Inner: Oplog + Send + Sync + 'static, Owner: PluginOwner, Scope: PluginScope>
    ForwardingOplog<Inner, Owner, Scope>
{
    const MAX_COMMIT_COUNT: usize = 3;

    pub fn new(
        inner: Inner,
        oplog_plugins: Arc<dyn OplogProcessorPlugin + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        components: Arc<dyn ComponentService + Send + Sync>,
        plugins: Arc<dyn Plugins<Owner, Scope> + Send + Sync>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        initial_worker_metadata: WorkerMetadata,
        last_oplog_idx: OplogIndex,
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
        }));

        let timer = tokio::spawn({
            let state = state.clone();
            async move {
                const MAX_ELAPSED_TIME: Duration = Duration::from_secs(5);
                loop {
                    tokio::time::sleep(MAX_ELAPSED_TIME).await;
                    let mut state = state.lock().await;
                    if state.buffer.len() > 0 && state.last_send.elapsed() > MAX_ELAPSED_TIME {
                        state.send_buffer().await;
                    }
                }
            }
        });
        Self {
            inner,
            state,
            timer: Some(timer),
        }
    }
}

impl<Inner: Oplog + Send + Sync + 'static, Owner: PluginOwner, Scope: PluginScope> Drop
    for ForwardingOplog<Inner, Owner, Scope>
{
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }
}

#[async_trait]
impl<Inner: Oplog + Send + Sync + 'static, Owner: PluginOwner, Scope: PluginScope> Oplog
    for ForwardingOplog<Inner, Owner, Scope>
{
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

struct ForwardingOplogState<Owner: PluginOwner, Scope: PluginScope> {
    buffer: VecDeque<OplogEntry>,
    commit_count: usize,
    last_send: Instant,
    oplog_plugins: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
    last_oplog_idx: OplogIndex,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    components: Arc<dyn ComponentService + Send + Sync>,
    plugins: Arc<dyn Plugins<Owner, Scope> + Send + Sync>,
}

impl<Owner: PluginOwner, Scope: PluginScope> Debug for ForwardingOplogState<Owner, Scope> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardingOplogState").finish()
    }
}

impl<Owner: PluginOwner, Scope: PluginScope> ForwardingOplogState<Owner, Scope> {
    pub async fn send_buffer(&mut self) {
        let metadata = {
            let execution_status = self.execution_status.read().unwrap();
            let status = execution_status.last_known_status().clone();
            WorkerMetadata {
                last_known_status: status,
                ..self.initial_worker_metadata.clone()
            }
        };

        let active_plugins = metadata.last_known_status.active_plugins();
        if !active_plugins.is_empty() {
            let entries: Vec<_> = self.buffer.drain(..).collect();
            let initial_oplog_index =
                OplogIndex::from_u64(Into::<u64>::into(self.last_oplog_idx) - entries.len() as u64);

            if let Err(err) = self
                .try_send_entries(metadata, initial_oplog_index, &entries)
                .await
            {
                log::error!("Failed to send oplog entries: {}", err);
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
    ) -> Result<(), GolemError> {
        let mut public_entries = Vec::new();

        for entry in entries {
            let public_entry = PublicOplogEntry::from_oplog_entry(
                entry.clone(),
                self.oplog_service.clone(),
                self.components.clone(),
                self.plugins.clone(),
                &metadata.owned_worker_id(),
                metadata.last_known_status.component_version, // NOTE: this is only safe if the component version is not changing within one batch
            )
            .await
            .map_err(|err| {
                GolemError::runtime(format!(
                    "Failed to enrich oplog entry for oplog processors: {err}"
                ))
            })?;

            public_entries.push(public_entry);
        }

        for installation_id in metadata.last_known_status.active_plugins() {
            self.oplog_plugins
                .send(
                    metadata.clone(),
                    &installation_id,
                    initial_oplog_index,
                    public_entries.clone(),
                )
                .await?;
        }

        Ok(())
    }
}
