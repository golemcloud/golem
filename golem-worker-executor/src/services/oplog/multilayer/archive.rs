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

impl InstrumentedOplogArchive {
    pub fn new(
        inner: Arc<dyn OplogArchive + Send + Sync>,
        account_id: AccountId,
        environment_id: EnvironmentId,
    ) -> Self {
        Self {
            inner,
            account_id,
            environment_id,
        }
    }
}

#[async_trait]
impl OplogArchive for InstrumentedOplogArchive {
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError> {
        self.inner.read_source(idx, n).await
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> Result<u64, OplogError> {
        if chunk.is_empty() {
            return Ok(0);
        }
        let entry_count = chunk.len() as u64;
        let account_id = self.account_id.to_string();
        let environment_id = self.environment_id.to_string();

        let bytes = self.inner.append(chunk).await?;

        record_storage_bytes_written(
            STORAGE_TYPE_OPLOG_ARCHIVE,
            &account_id,
            &environment_id,
            bytes,
        );
        record_storage_objects_written(
            STORAGE_TYPE_OPLOG_ARCHIVE,
            &account_id,
            &environment_id,
            entry_count,
        );

        Ok(bytes)
    }

    async fn verify_persisted(
        &self,
        entries: &[(OplogIndex, OplogEntry)],
    ) -> OplogArchiveResult<()> {
        self.inner.verify_persisted(entries).await
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.inner.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError> {
        let dropped = self.inner.drop_prefix(last_dropped_id).await?;
        if dropped > 0 {
            let account_id = self.account_id.to_string();
            let environment_id = self.environment_id.to_string();
            record_storage_objects_deleted(
                STORAGE_TYPE_OPLOG_ARCHIVE,
                &account_id,
                &environment_id,
                dropped,
            );
        }
        Ok(dropped)
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        self.inner.length().await
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.inner.get_last_index().await
    }

    fn fence(&self) -> Option<OplogFence> {
        self.inner.fence()
    }
}

impl WrappedOplogArchive {
    pub async fn new(
        layer: usize,
        archive: Arc<dyn OplogArchive + Send + Sync>,
        transfer: UnboundedSender<BackgroundTransferMessage>,
        owned_agent_id: OwnedAgentId,
        multi_layer_oplog_service: MultiLayerOplogService,
        entry_count_limit: u64,
    ) -> Self {
        let initial_entry_count = archive.length().await.unwrap_or_else(|error| {
            warn!(
                layer,
                error = %error,
                "Failed to read oplog archive length; archive operations will retry later"
            );
            0
        });
        Self {
            layer,
            archive,
            entry_count: AtomicU64::new(initial_entry_count),
            transfer,
            owned_agent_id,
            multi_layer_oplog_service,
            entry_count_limit,
        }
    }

    pub fn new_fresh(
        layer: usize,
        archive: Arc<dyn OplogArchive + Send + Sync>,
        transfer: UnboundedSender<BackgroundTransferMessage>,
        owned_agent_id: OwnedAgentId,
        multi_layer_oplog_service: MultiLayerOplogService,
        entry_count_limit: u64,
    ) -> Self {
        Self {
            layer,
            archive,
            entry_count: AtomicU64::new(0),
            transfer,
            owned_agent_id,
            multi_layer_oplog_service,
            entry_count_limit,
        }
    }
}

#[async_trait]
impl OplogArchive for WrappedOplogArchive {
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError> {
        self.archive.read_source(idx, n).await
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> Result<u64, OplogError> {
        if !chunk.is_empty() {
            let last_idx = chunk.last().unwrap().0;
            let bytes = self.archive.append(chunk).await?;
            let old_count = self.entry_count.fetch_add(1, Ordering::AcqRel); // Note: the whole chunk is stored as one entry, so incrementing only by one
            let count = old_count + 1;
            if count >= self.entry_count_limit
                && self
                    .multi_layer_oplog_service
                    .archive_attempt_ready(&self.owned_agent_id, ArchiveSource::Lower(self.layer))
            {
                debug!(
                    "Enqueuing transfer of oplog entries from the oplog layer {} to the next layer up to {last_idx}",
                    self.layer
                );
                let _ = self.transfer.send(TransferFromLower {
                    source: self.layer,
                    last_transferred_idx: last_idx,
                    keep_alive: None,
                    done: None,
                    drain: false,
                    transfer_origin: TraceOrigin::capture_current(),
                });
                // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
                self.entry_count.store(0, Ordering::Release);
            }
            Ok(bytes)
        } else {
            Ok(0)
        }
    }

    async fn verify_persisted(
        &self,
        entries: &[(OplogIndex, OplogEntry)],
    ) -> OplogArchiveResult<()> {
        self.archive.verify_persisted(entries).await
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.archive.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError> {
        let dropped_entries = self.archive.drop_prefix(last_dropped_id).await?;
        let new_length = self
            .archive
            .length()
            .await
            .map_err(|error| OplogError::Maintenance(error.to_string()))?;
        let old_entry_count = self.entry_count.load(Ordering::Acquire);
        let new_entry_count = min(new_length, old_entry_count);
        self.entry_count.store(new_entry_count, Ordering::Release);
        Ok(dropped_entries)
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        self.archive.length().await
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.archive.get_last_index().await
    }

    fn fence(&self) -> Option<OplogFence> {
        self.archive.fence()
    }
}
