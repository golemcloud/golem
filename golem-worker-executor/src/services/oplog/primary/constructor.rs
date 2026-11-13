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

impl CreateOplogConstructor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        shard_epoch: Option<ShardEpoch>,
        epoch_record: EpochRecord,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        max_retained_auto_commit_batches: usize,
        retry_config: RetryConfig,
        key: String,
        last_oplog_idx: Option<OplogIndex>,
        reconcile_last_index: bool,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        fingerprint: AgentFingerprint,
        stream_session_index: Option<Arc<crate::services::oplog::StreamSessionIndexService>>,
    ) -> Self {
        Self {
            shard_epoch,
            epoch_record,
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_payload_size,
            max_retained_auto_commit_batches,
            retry_config,
            key,
            last_oplog_idx,
            reconcile_last_index,
            owned_agent_id,
            agent_mode,
            account_id,
            fingerprint,
            stream_session_index,
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
        _lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        // Recorded before the oplog is usable, so an executor whose shard has moved is refused
        // at its very first write rather than after replaying the new owner's entries.
        let fence = match (self.epoch_record, self.shard_epoch) {
            (EpochRecord::Recorded(fence), _) => fence,
            (EpochRecord::Pending, Some(shard_epoch)) => {
                record_owning_epoch(
                    &*self.indexed_storage,
                    &self.retry_config,
                    PrimaryOplogService::namespace(&self.owned_agent_id.agent_id, self.agent_mode),
                    &self.owned_agent_id.agent_id,
                    &self.key,
                    shard_epoch,
                )
                .await
            }
            (EpochRecord::Pending, None) => None,
        };

        // Read after the claim: once it returns, every writer at an older epoch is refused, so the
        // read sees every entry that will ever precede this handle's first write. An index a
        // caller read before the claim can be behind by whatever a losing executor committed in
        // between, and this handle's first append would collide with it. That index is merged
        // rather than replaced, because it may count entries this layer no longer holds, such as
        // ones already moved to an archive.
        let stored_last_index = || {
            PrimaryOplogService::get_last_index_from_storage(
                &*self.indexed_storage,
                &self.owned_agent_id,
                self.agent_mode,
                &self.retry_config,
            )
        };
        let last_oplog_idx = match self.last_oplog_idx {
            None => stored_last_index().await,
            Some(idx)
                if self.reconcile_last_index && self.shard_epoch.is_some() && fence.is_none() =>
            {
                OplogIndex::from_u64(idx.as_u64().max(stored_last_index().await.as_u64()))
            }
            Some(idx) => idx,
        };

        Ok(Arc::new(PrimaryOplog::new(
            self.shard_epoch,
            fence,
            self.indexed_storage,
            self.blob_storage,
            self.replicas,
            self.max_operations_before_commit,
            self.max_payload_size,
            self.max_retained_auto_commit_batches,
            self.retry_config,
            PrimaryOplogService::namespace(&self.owned_agent_id.agent_id, self.agent_mode),
            self.key,
            last_oplog_idx,
            self.owned_agent_id,
            self.agent_mode,
            self.account_id,
            self.fingerprint,
            self.stream_session_index,
            close,
        )))
    }
}
