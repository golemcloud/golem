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

use super::*;

/// An implementation of the `OplogProcessorPlugin` trait that runs a single instance of each
/// used plugin on each worker executor node.
pub struct PerExecutorOplogProcessorPlugin<Ctx: WorkerCtx> {
    workers: Arc<RwLock<HashMap<WorkerKey, RunningPlugin>>>,
    component_service: Arc<dyn ComponentService>,
    shard_service: Arc<dyn ShardService>,
    worker_activator: Arc<dyn WorkerActivator<Ctx>>,
    worker_proxy: Arc<dyn WorkerProxy>,
}

// TODO: Once cross-environment RPC is enabled via host.wit, the cache key should
// use the component's canonical environment_id rather than the caller-provided one.
// Currently all plugin workers live in the same environment as their callers so
// the inconsistency is harmless — a cache miss just triggers a component lookup.
type WorkerKey = (EnvironmentId, PluginRegistrationId);

#[derive(Debug, Clone)]
struct RunningPlugin {
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
                        // Resolve the component's authoritative environment_id.
                        let plugin_component = self
                            .component_service
                            .get_metadata(plugin_component_id, Some(plugin_component_revision))
                            .await?;
                        let owned_agent_id = OwnedAgentId {
                            environment_id: plugin_component.environment_id,
                            agent_id: agent_id.clone(),
                        };
                        let running_plugin = RunningPlugin {
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
            &current_assignment.shard_id_set(),
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

        let is_local = self.is_local(target_agent_id).await?;

        if is_local {
            // Local path: activate and invoke directly
            let target_owned = OwnedAgentId::new(worker_metadata.environment_id, target_agent_id);

            let worker = self
                .worker_activator
                .get_or_create_running(
                    &target_owned,
                    None,
                    Vec::new(),
                    Some(running_plugin.component_revision),
                    None,
                    &InvocationContextStack::fresh(),
                    Principal::anonymous(),
                )
                .await?;

            let account_id = worker_metadata.created_by;
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
        } else {
            // Remote path: route through worker service to the executor
            // that currently owns the recorded target worker
            let proto_metadata = {
                use std::collections::HashMap;
                let latest_status = &worker_metadata.last_known_status;
                golem_api_grpc::proto::golem::worker::AgentMetadata {
                    agent_id: Some(worker_metadata.agent_id.clone().into()),
                    owner_kind: worker_metadata.owner_kind.into(),
                    environment_id: Some(worker_metadata.environment_id.into()),
                    env: HashMap::from_iter(worker_metadata.env.iter().cloned()),
                    config: worker_metadata
                        .config
                        .clone()
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, String>>()
                        .map_err(|error| {
                            WorkerExecutorError::unknown(format!(
                                "failed converting agent configuration: {error}"
                            ))
                        })?,
                    created_by: Some(worker_metadata.created_by.into()),
                    component_revision: latest_status.component_revision.into(),
                    status: Into::<golem_api_grpc::proto::golem::worker::AgentStatus>::into(
                        latest_status.status,
                    )
                    .into(),
                    retry_count: latest_status
                        .current_retry_state
                        .iter()
                        .max_by_key(|(idx, _)| **idx)
                        .map(|(_, state)| state.retry_count())
                        .unwrap_or_default(),
                    pending_invocation_count: latest_status.pending_invocations.len() as u64,
                    updates: Vec::new(),
                    created_at: Some(worker_metadata.created_at.into()),
                    last_error: None,
                    last_error_kind: latest_status.last_error_kind.map(|kind| match kind {
                        golem_common::model::oplog::OplogErrorKind::Invocation => {
                            golem_api_grpc::proto::golem::worker::OplogErrorKind::Invocation as i32
                        }
                        golem_common::model::oplog::OplogErrorKind::Recovery => {
                            golem_api_grpc::proto::golem::worker::OplogErrorKind::Recovery as i32
                        }
                    }),
                    component_size: latest_status.component_size,
                    total_linear_memory_size: latest_status.total_linear_memory_size,
                    owned_resources: Vec::new(),
                    active_plugins: latest_status
                        .active_plugins
                        .iter()
                        .map(|id| (*id).into())
                        .collect(),
                    skipped_regions: latest_status
                        .skipped_regions
                        .regions()
                        .map(|region| region.clone().into())
                        .collect(),
                    deleted_regions: latest_status
                        .deleted_regions
                        .regions()
                        .map(|region| region.clone().into())
                        .collect(),
                    oplog_idx: u64::from(latest_status.oplog_idx),
                    fingerprint: Some(worker_metadata.fingerprint.0.into()),
                }
            };
            let proto_entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry> = entries
                .into_iter()
                .map(golem_api_grpc::proto::golem::worker::RawOplogEntry::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    WorkerExecutorError::unknown(format!(
                        "Failed to convert oplog entries to proto: {e}"
                    ))
                })?;

            self.worker_proxy
                .process_oplog_entries(
                    target_agent_id,
                    running_plugin.owned_agent_id.environment_id,
                    running_plugin.component_revision,
                    idempotency_key,
                    worker_metadata.created_by,
                    running_plugin
                        .configuration
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    proto_metadata,
                    initial_oplog_index,
                    proto_entries,
                    &AuthCtx::System,
                )
                .await
                .map_err(|e| {
                    WorkerExecutorError::unknown(format!(
                        "Remote oplog processor delivery failed: {e}"
                    ))
                })?;
        }

        Ok(())
    }

    async fn invalidate_target(&self, environment_id: EnvironmentId, plugin: &InstalledPlugin) {
        let mut workers = self.workers.write().await;
        workers.remove(&(environment_id, plugin.plugin_registration_id));
    }

    async fn is_local(&self, agent_id: &AgentId) -> Result<bool, WorkerExecutorError> {
        let assignment = self.shard_service.current_assignment()?;
        let shard_id = ShardId::from_agent_id(agent_id, assignment.number_of_shards);
        Ok(assignment.contains(&shard_id))
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
                    if new_assignment.contains(&shard_id) {
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
        target_agent_id: &AgentId,
        _caller_account_id: AccountId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationStatus, WorkerExecutorError> {
        self.worker_proxy
            .lookup_invocation_status(
                target_agent_id,
                idempotency_key.clone(),
                Some(environment_id),
                &AuthCtx::System,
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
