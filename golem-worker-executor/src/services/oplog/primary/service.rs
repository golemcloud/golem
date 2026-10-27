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

impl Debug for PrimaryOplogService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrimaryOplogService")
            .field("indexed_storage", &self.indexed_storage)
            .field("blob_storage", &self.blob_storage)
            .field("replicas", &self.replicas)
            .field(
                "max_operations_before_commit",
                &self.max_operations_before_commit,
            )
            .field(
                "max_operations_before_commit_ephemeral",
                &self.max_operations_before_commit_ephemeral,
            )
            .field("max_payload_size", &self.max_payload_size)
            .field(
                "max_retained_auto_commit_batches",
                &self.max_retained_auto_commit_batches,
            )
            .field("retry_config", &self.retry_config)
            .field("oplogs", &self.oplogs)
            .field("stream_session_index", &self.stream_session_index)
            .finish()
    }
}

impl PrimaryOplogService {
    pub async fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_operations_before_commit: u64,
        max_operations_before_commit_ephemeral: u64,
        max_payload_size: usize,
        max_retained_auto_commit_batches: usize,
        retry_config: RetryConfig,
    ) -> Self {
        let replicas = retry_storage_op(&retry_config, "number_of_replicas", "global", || {
            let is = indexed_storage.clone();
            async move { is.with("oplog", "new").number_of_replicas().await }
        })
        .await;
        Self {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_operations_before_commit_ephemeral,
            max_payload_size,
            max_retained_auto_commit_batches,
            retry_config,
            oplogs: OpenOplogs::new("primary oplog"),
            stream_session_index: Arc::new(std::sync::OnceLock::new()),
        }
    }

    fn oplog_key(agent_id: &AgentId) -> String {
        agent_id.to_redis_key()
    }

    fn staged_oplog_key(agent_id: &AgentId, stage_id: uuid::Uuid) -> String {
        format!("{}#{stage_id}", Self::oplog_key(agent_id))
    }

    pub(super) fn namespace(agent_id: &AgentId, agent_mode: AgentMode) -> IndexedStorageNamespace {
        IndexedStorageNamespace::OpLog {
            agent_id: agent_id.clone(),
            agent_mode,
        }
    }

    fn staged_namespace(agent_id: &AgentId, agent_mode: AgentMode) -> IndexedStorageNamespace {
        IndexedStorageNamespace::StagedOpLog {
            agent_id: agent_id.clone(),
            agent_mode,
        }
    }

    pub fn key_prefix(component_id: &ComponentId) -> String {
        component_id.0.to_string()
    }

    async fn append_initial_entry(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        op_name: &str,
        api_name: &'static str,
        entry: &OplogEntry,
        shard_epoch: Option<ShardEpoch>,
    ) {
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let namespace = IndexedStorageNamespace::OpLog {
            agent_id: owned_agent_id.agent_id(),
            agent_mode,
        };
        let value = Bytes::from(
            serialize(entry)
                .unwrap_or_else(|err| panic!("Failed to serialize initial oplog entry: {err}")),
        );

        retry_oplog_append(
            &self.retry_config,
            self.indexed_storage.as_ref(),
            &namespace,
            op_name,
            api_name,
            &key,
            SerializedOplogAppend::Entry((1, value)),
            shard_epoch,
        )
        .await
        .unwrap_or_else(|err| {
            // Only a fence reaches here - every other permanent failure already panicked inside
            // `retry_oplog_append`. Nothing more is written: `open` records the epoch again and,
            // being refused there too, hands back an oplog that refuses every write.
            warn!(
                agent_id = %owned_agent_id,
                error = %err,
                "Initial oplog entry fenced: the shard has a new owner"
            );
        });
    }

    async fn open_with(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        shard_epoch: Option<ShardEpoch>,
        epoch_record: EpochRecord,
        reconcile_last_index: bool,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        record_oplog_call("open");

        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let max_operations_before_commit = match agent_mode {
            AgentMode::Durable => self.max_operations_before_commit,
            AgentMode::Ephemeral => self.max_operations_before_commit_ephemeral,
        };

        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    shard_epoch,
                    epoch_record,
                    self.indexed_storage.clone(),
                    self.blob_storage.clone(),
                    self.replicas,
                    max_operations_before_commit,
                    self.max_payload_size,
                    self.max_retained_auto_commit_batches,
                    self.retry_config.clone(),
                    key,
                    last_oplog_index,
                    reconcile_last_index,
                    owned_agent_id.clone(),
                    agent_mode,
                    initial_worker_metadata.created_by,
                    initial_worker_metadata.fingerprint,
                    self.stream_session_index(),
                ),
            )
            .await
    }

    pub(super) async fn get_last_index_from_storage(
        indexed_storage: &(dyn IndexedStorage + Send + Sync),
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        retry_config: &RetryConfig,
    ) -> OplogIndex {
        Self::try_get_last_index_from_storage(
            indexed_storage,
            owned_agent_id,
            agent_mode,
            retry_config,
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"))
    }

    async fn try_get_last_index_from_storage(
        indexed_storage: &(dyn IndexedStorage + Send + Sync),
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        retry_config: &RetryConfig,
    ) -> Result<OplogIndex, String> {
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let agent_id = owned_agent_id.agent_id();
        Ok(OplogIndex::from_u64(
            retry_storage_op_result(retry_config, "get_last_index", &key, || {
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    indexed_storage
                        .with_entity("oplog", "get_last_index", "entry")
                        .last_id(ns, &key)
                        .await
                }
            })
            .await?
            .unwrap_or_default(),
        ))
    }

    pub fn get_agent_id_from_key(key: &str, component_id: &ComponentId) -> AgentId {
        let redis_prefix = format!("{}:", component_id.0);
        if key.starts_with(&redis_prefix) {
            let agent_name = &key[redis_prefix.len()..];
            AgentId {
                agent_id: agent_name.to_string(),
                component_id: *component_id,
            }
        } else {
            panic!("Failed to get worker id from indexed storage key: {key}")
        }
    }

    pub(super) async fn upload_raw_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_payload_size: usize,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        if data.len() > max_payload_size {
            let payload_id: PayloadId = PayloadId::new();
            let md5_hash = md5::compute(&data).to_vec();

            blob_storage
                .with("oplog", "upload_payload")
                .put_raw(
                    BlobStorageNamespace::OplogPayload {
                        environment_id: owned_agent_id.environment_id(),
                        agent_id: owned_agent_id.agent_id(),
                        agent_mode,
                    },
                    Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
                    &data,
                )
                .await
                .map_err(|e| format!("Failed uploading oplog data to the blob store {e}"))?;

            Ok(RawOplogPayload::External {
                payload_id,
                md5_hash,
            })
        } else {
            Ok(RawOplogPayload::SerializedInline(data))
        }
    }

    pub(super) async fn download_raw_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        Self::download_raw_payload_classified(
            blob_storage,
            owned_agent_id,
            agent_mode,
            payload_id,
            md5_hash,
        )
        .await
        .map_err(|error| error.to_string())
    }

    pub(super) async fn download_raw_payload_classified(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        let result = blob_storage
            .with("oplog", "download_payload")
            .get_raw(
                BlobStorageNamespace::OplogPayload {
                    environment_id: owned_agent_id.environment_id(),
                    agent_id: owned_agent_id.agent_id(),
                    agent_mode,
                },
                Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
            )
            .await
            .map_err(|error| {
                RawOplogPayloadDownloadError::Backend(
                    error.context("failed downloading oplog data from the blob store"),
                )
            })?;
        result.ok_or(RawOplogPayloadDownloadError::Missing(payload_id))
    }
}

#[async_trait]
impl OplogService for PrimaryOplogService {
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        self.oplogs.lock_lifecycle(agent_id).await
    }

    fn set_stream_session_index(
        &self,
        index: Arc<crate::services::oplog::StreamSessionIndexService>,
    ) {
        assert!(
            self.stream_session_index.set(index).is_ok(),
            "stream session index is already installed"
        );
    }

    fn stream_session_index(
        &self,
    ) -> Option<Arc<crate::services::oplog::StreamSessionIndexService>> {
        self.stream_session_index.get().cloned()
    }

    async fn create_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
        initial_worker_metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        record_oplog_call("create_staged");
        if agent_mode != AgentMode::Durable {
            return Err("Only durable agents can have staged oplogs".into());
        }
        let key = Self::staged_oplog_key(&owned_agent_id.agent_id, stage_id);
        let namespace = Self::staged_namespace(&owned_agent_id.agent_id, agent_mode);
        Ok(Arc::new(PrimaryOplog::new(
            // A stage is hidden and has exactly one writer, so it asserts no epoch and can never
            // be fenced: it is published into the target only once it is fully committed.
            None,
            None,
            self.indexed_storage.clone(),
            self.blob_storage.clone(),
            self.replicas,
            self.max_operations_before_commit,
            self.max_payload_size,
            self.max_retained_auto_commit_batches,
            self.retry_config.clone(),
            namespace,
            key,
            OplogIndex::NONE,
            owned_agent_id.clone(),
            agent_mode,
            initial_worker_metadata.created_by,
            initial_worker_metadata.fingerprint,
            None,
            Box::new(|| {}),
        )))
    }

    async fn staged_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        record_oplog_call("staged_exists");
        let key = Self::staged_oplog_key(&owned_agent_id.agent_id, stage_id);
        self.indexed_storage
            .with("oplog", "staged_exists")
            .exists(
                Self::staged_namespace(&owned_agent_id.agent_id, agent_mode),
                &key,
            )
            .await
            .map_err(|error| format!("Failed checking staged oplog {key}: {error}"))
    }

    async fn publish_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        publication: StagePublication,
        expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        record_oplog_call("publish_staged");
        let stage_id = publication.stage_id();
        if agent_mode != AgentMode::Durable {
            return Err("Only durable agents can publish staged oplogs".into());
        }
        let stage_key = Self::staged_oplog_key(&owned_agent_id.agent_id, stage_id);
        let target_key = Self::oplog_key(&owned_agent_id.agent_id);
        self.indexed_storage
            .with("oplog", "publish_staged")
            .move_if_absent(
                IndexedStorageNamespace::StagedOpLog {
                    agent_id: owned_agent_id.agent_id.clone(),
                    agent_mode,
                },
                &stage_key,
                IndexedStorageNamespace::OpLog {
                    agent_id: owned_agent_id.agent_id.clone(),
                    agent_mode,
                },
                &target_key,
                expected_last_index.into(),
            )
            .await
            .map_err(|error| format!("Failed publishing staged oplog {stage_key}: {error}"))
    }

    async fn discard_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        record_oplog_call("discard_staged");
        let key = Self::staged_oplog_key(&owned_agent_id.agent_id, stage_id);
        self.indexed_storage
            .with("oplog", "discard_staged")
            .delete(
                Self::staged_namespace(&owned_agent_id.agent_id, agent_mode),
                &key,
            )
            .await
            .map_err(|error| format!("Failed discarding staged oplog {key}: {error}"))
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        record_oplog_call("create");
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        let key = Self::oplog_key(&owned_agent_id.agent_id);

        // The record goes in before the existence probe and the first entry. A probe taken before
        // the claim can miss a `Create` that an executor at an older epoch lands in between, and
        // the initial append would then collide with it. If the claim is refused, this executor
        // has already lost the shard: it writes nothing, so whether the owner created the oplog
        // first is not its question, and `open` below hands back an oplog that refuses every write.
        let fence_at_create = match shard_epoch {
            Some(epoch) => {
                record_owning_epoch(
                    &*self.indexed_storage,
                    &self.retry_config,
                    Self::namespace(&owned_agent_id.agent_id, agent_mode),
                    &owned_agent_id.agent_id,
                    &key,
                    epoch,
                )
                .await
            }
            None => None,
        };

        if fence_at_create.is_none() {
            let already_exists: bool = {
                let is = self.indexed_storage.clone();
                let agent_id = owned_agent_id.agent_id();
                let key = key.clone();
                retry_storage_op(&self.retry_config, "create_exists", &key, || {
                    let is = is.clone();
                    let ns = IndexedStorageNamespace::OpLog {
                        agent_id: agent_id.clone(),
                        agent_mode,
                    };
                    let key = key.clone();
                    async move { is.with("oplog", "create").exists(ns, &key).await }
                })
                .await
            };

            if already_exists {
                panic!("oplog for worker {owned_agent_id} already exists in indexed storage")
            }

            self.append_initial_entry(
                owned_agent_id,
                agent_mode,
                "create_append",
                "create",
                &initial_entry,
                shard_epoch,
            )
            .await;
        }

        // The claim came before the initial entry, so `INITIAL` is exact and needs no re-read.
        self.open_with(
            lifecycle,
            owned_agent_id,
            agent_mode,
            Some(OplogIndex::INITIAL),
            initial_worker_metadata,
            shard_epoch,
            EpochRecord::Recorded(fence_at_create),
            false,
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
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        record_oplog_call("create_fresh");
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        // The caller guarantees the agent id is freshly derived and unused, so
        // the existence probe performed by `create` is skipped: the initial
        // entry is appended directly without any prior read. The epoch record still goes in
        // first - a fresh agent id does not mean a fresh shard.
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let fence_at_create = match shard_epoch {
            Some(epoch) => {
                record_owning_epoch(
                    &*self.indexed_storage,
                    &self.retry_config,
                    Self::namespace(&owned_agent_id.agent_id, agent_mode),
                    &owned_agent_id.agent_id,
                    &key,
                    epoch,
                )
                .await
            }
            None => None,
        };

        if fence_at_create.is_none() {
            self.append_initial_entry(
                owned_agent_id,
                agent_mode,
                "create_fresh_append",
                "create_fresh",
                &initial_entry,
                shard_epoch,
            )
            .await;
        }

        // Claimed before the initial entry, so `INITIAL` is exact; not re-reading it keeps a fresh
        // create free of storage reads.
        self.open_with(
            lifecycle,
            owned_agent_id,
            agent_mode,
            Some(OplogIndex::INITIAL),
            initial_worker_metadata,
            shard_epoch,
            EpochRecord::Recorded(fence_at_create),
            false,
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
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        // An index handed in by a caller was read before this open claims the epoch.
        let reconcile_last_index = last_oplog_index.is_some();
        self.open_with(
            lifecycle,
            owned_agent_id,
            agent_mode,
            last_oplog_index,
            initial_worker_metadata,
            shard_epoch,
            EpochRecord::Pending,
            reconcile_last_index,
        )
        .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        record_oplog_call("get_last_index");
        Ok(Self::get_last_index_from_storage(
            &*self.indexed_storage,
            owned_agent_id,
            agent_mode,
            &self.retry_config,
        )
        .await)
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        record_oplog_call("get_last_index");
        Self::try_get_last_index_from_storage(
            &*self.indexed_storage,
            owned_agent_id,
            agent_mode,
            &self.retry_config,
        )
        .await
    }

    async fn assert_owning_epoch(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        record_oplog_call("assert_owning_epoch");
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        match record_owning_epoch(
            self.indexed_storage.as_ref(),
            &self.retry_config,
            Self::namespace(&owned_agent_id.agent_id, agent_mode),
            &owned_agent_id.agent_id,
            &key,
            expected_epoch,
        )
        .await
        {
            None => Ok(()),
            Some(fence) => Err(OplogError::Fenced(fence)),
        }
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError> {
        record_oplog_call("delete");
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        let is = self.indexed_storage.clone();
        let agent_id = owned_agent_id.agent_id();
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        // The entries and the epoch record go in one step, and only while the record is still
        // this executor's: a delete that outlived the agent's shard is refused like a write, and
        // leaves the new owner's oplog alone.
        let outcome = retry_maintenance_storage_op(&self.retry_config, "delete", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::OpLog {
                agent_id: agent_id.clone(),
                agent_mode,
            };
            let key = key.clone();
            async move {
                is.delete_with_epoch("oplog", "delete", ns, &key, expected_epoch)
                    .await
            }
        })
        .await;
        match outcome {
            Ok(()) => Ok(()),
            Err(error @ IndexedStorageError::Fenced { .. }) => {
                let fence = OplogFence::refused(agent_id, error);
                warn!(
                    agent_id = %owned_agent_id,
                    expected_epoch = fence.expected_epoch.0,
                    actual_epoch = ?fence.actual_epoch.map(|epoch| epoch.0),
                    "Oplog delete refused: the shard has a new owner"
                );
                Err(OplogError::Fenced(fence))
            }
            Err(other) => Err(OplogError::Maintenance(format!(
                "Failed to delete primary oplog for {agent_id}: {other}"
            ))),
        }
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        if n == 0 {
            return Ok(BTreeMap::new());
        }
        let entries = self.read_source(owned_agent_id, agent_mode, idx, n).await?;
        exact_from_source(OplogReadSource::Primary, idx, n, entries)
            .map_err(|error| UnreadableOplog::new(&owned_agent_id.agent_id, error))
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        record_oplog_call("read");
        let Some(end) = checked_range_end(idx, n)
            .map_err(|error| UnreadableOplog::new(&owned_agent_id.agent_id, error))?
        else {
            return Ok(BTreeMap::new());
        };

        Ok({
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            let start: u64 = idx.into();
            let end: u64 = end.into();
            retry_storage_op(&self.retry_config, "read", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { read_persisted_oplog_entries(is, ns, key, start, end).await }
            })
            .await
            .into_iter()
            .map(|(k, v): (u64, OplogEntry)| (OplogIndex::from_u64(k), v))
            .collect()
        })
    }

    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        Ok(self
            .try_exists(owned_agent_id, agent_mode)
            .await
            .unwrap_or_else(|error| panic!("{error}")))
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, String> {
        record_oplog_call("exists");

        Ok({
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            retry_storage_op_result(&self.retry_config, "exists", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "exists").exists(ns, &key).await }
            })
            .await?
        })
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        record_oplog_call("scan");

        let state = decode_scan_cursor(&cursor, modes)?;
        if state.layer != 0 {
            return Err(WorkerExecutorError::invalid_request(
                "Primary oplog scan cursor must name layer 0",
            ));
        }
        let active_mode = state.mode;

        let (next_resume, keys) = {
            let is = self.indexed_storage.clone();
            let prefix = Self::key_prefix(component_id);
            let resume = state.resume.clone();
            retry_scan_storage_op(&self.retry_config, "scan", &prefix, || {
                let is = is.clone();
                let prefix = prefix.clone();
                let resume = resume.clone();
                async move {
                    is.with("oplog", "scan")
                        .scan_stable(
                            IndexedStorageMetaNamespace::Oplog {
                                agent_mode: active_mode,
                            },
                            Some(&prefix),
                            resume,
                            count,
                        )
                        .await
                }
            })
            .await?
        };

        let next_cursor = next_scan_cursor(state, modes, next_resume)?;
        let owned_agent_ids = keys
            .into_iter()
            .map(|key| OwnedAgentId {
                agent_id: Self::get_agent_id_from_key(&key, component_id),
                environment_id: *environment_id,
            })
            .collect();

        Ok((next_cursor, owned_agent_ids))
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        Self::upload_raw_payload(
            self.blob_storage.clone(),
            self.max_payload_size,
            owned_agent_id,
            agent_mode,
            data,
        )
        .await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        Self::download_raw_payload(
            self.blob_storage.clone(),
            owned_agent_id,
            agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }

    async fn download_raw_payload_classified(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        Self::download_raw_payload_classified(
            self.blob_storage.clone(),
            owned_agent_id,
            agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }
}
