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

#[derive(Clone)]
struct CreateOplogConstructor {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    initial_entry: Option<OplogEntry>,
    inner: Arc<dyn OplogService>,
    last_oplog_index: Option<OplogIndex>,
    fresh: bool,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    components: Arc<dyn ComponentService>,
    initial_worker_metadata: AgentMetadata,
    last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
    execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    plugin_max_commit_count: usize,
    plugin_max_elapsed_time: Duration,
    shard_epoch: Option<ShardEpoch>,
}

impl CreateOplogConstructor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: Option<OplogEntry>,
        inner: Arc<dyn OplogService>,
        last_oplog_index: Option<OplogIndex>,
        fresh: bool,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        plugin_max_commit_count: usize,
        plugin_max_elapsed_time: Duration,
        shard_epoch: Option<ShardEpoch>,
    ) -> Self {
        Self {
            owned_agent_id,
            agent_mode,
            initial_entry,
            inner,
            last_oplog_index,
            fresh,
            oplog_plugins,
            components,
            initial_worker_metadata,
            last_known_status,
            execution_status,
            plugin_max_commit_count,
            plugin_max_elapsed_time,
            shard_epoch,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.shard_epoch
    }

    async fn create_oplog(
        self,
        lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let inner = if let Some(initial_entry) = self.initial_entry {
            if self.fresh {
                self.inner
                    .create_fresh(
                        lifecycle,
                        &self.owned_agent_id,
                        self.agent_mode,
                        initial_entry,
                        self.initial_worker_metadata.clone(),
                        self.last_known_status.clone(),
                        self.execution_status.clone(),
                        self.shard_epoch,
                    )
                    .await
            } else {
                self.inner
                    .create(
                        lifecycle,
                        &self.owned_agent_id,
                        self.agent_mode,
                        initial_entry,
                        self.initial_worker_metadata.clone(),
                        self.last_known_status.clone(),
                        self.execution_status.clone(),
                        self.shard_epoch,
                    )
                    .await
            }
        } else {
            self.inner
                .open(
                    lifecycle,
                    &self.owned_agent_id,
                    self.agent_mode,
                    self.last_oplog_index,
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                    self.shard_epoch,
                )
                .await
        };
        // A failed open caches nothing, so its slot is released at once.
        let inner = match inner {
            Ok(inner) => inner,
            Err(error) => {
                close();
                return Err(error);
            }
        };
        // Taken from the opened inner oplog, not read up front: the inner open claims the shard
        // epoch, so an index read before it could be behind a losing executor's last commit, and
        // the forwarding buffer would then label its entries with indexes already in use.
        let last_oplog_index = match self.last_oplog_index {
            Some(idx) => idx,
            None => inner.current_oplog_index().await,
        };

        Ok(Arc::new(
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
        ))
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
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        self.inner.lock_lifecycle(agent_id).await
    }

    fn set_stream_session_index(&self, index: Arc<super::super::StreamSessionIndexService>) {
        self.inner.set_stream_session_index(index);
    }

    fn stream_session_index(&self) -> Option<Arc<super::super::StreamSessionIndexService>> {
        self.inner.stream_session_index()
    }

    async fn create_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
        initial_worker_metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        self.inner
            .create_staged(
                owned_agent_id,
                agent_mode,
                stage_id,
                initial_worker_metadata,
            )
            .await
    }

    async fn staged_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        self.inner
            .staged_exists(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn publish_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        publication: StagePublication,
        expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        self.inner
            .publish_staged(owned_agent_id, agent_mode, publication, expected_last_index)
            .await
    }

    async fn discard_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        self.inner
            .discard_staged(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog + 'static>, WorkerExecutorError> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    Some(initial_entry),
                    self.inner.clone(),
                    Some(OplogIndex::INITIAL),
                    false,
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    self.plugin_max_commit_count,
                    self.plugin_max_elapsed_time,
                    shard_epoch,
                ),
            )
            .await
    }

    async fn create_fresh(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog + 'static>, WorkerExecutorError> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    Some(initial_entry),
                    self.inner.clone(),
                    Some(OplogIndex::INITIAL),
                    true,
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    self.plugin_max_commit_count,
                    self.plugin_max_elapsed_time,
                    shard_epoch,
                ),
            )
            .await
    }

    async fn open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog + 'static>, WorkerExecutorError> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    None,
                    self.inner.clone(),
                    last_oplog_index,
                    false,
                    self.oplog_plugins.clone(),
                    self.components.clone(),
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    self.plugin_max_commit_count,
                    self.plugin_max_elapsed_time,
                    shard_epoch,
                ),
            )
            .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        self.inner.get_last_index(owned_agent_id, agent_mode).await
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        self.inner
            .try_get_last_index(owned_agent_id, agent_mode)
            .await
    }

    async fn assert_owning_epoch(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        self.inner
            .assert_owning_epoch(owned_agent_id, agent_mode, expected_epoch)
            .await
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError> {
        self.inner
            .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
            .await
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.inner
            .read_exact(owned_agent_id, agent_mode, idx, n)
            .await
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.inner
            .read_source(owned_agent_id, agent_mode, idx, n)
            .await
    }

    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        self.inner.exists(owned_agent_id, agent_mode).await
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, String> {
        self.inner.try_exists(owned_agent_id, agent_mode).await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, modes, cursor, count)
            .await
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.inner
            .upload_raw_payload(owned_agent_id, agent_mode, data)
            .await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .download_raw_payload(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }

    async fn download_raw_payload_classified(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.inner
            .download_raw_payload_classified(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }
}
