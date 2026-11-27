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
use crate::services::oplog::*;
use crate::services::stream_session_index::StreamSessionIndexService;
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::model::ShardEpoch;
use golem_common::model::oplog::{PayloadId, RawOplogPayload};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// An observation-only oplog over a frozen seal record. It owns no writer or transfer task.
pub(crate) struct SealedOplog {
    id: OwnedAgentId,
    record: Box<SealRecord>,
    blobs: Arc<dyn BlobStorage + Send + Sync>,
    payloads: Arc<dyn OplogService>,
    stream_session_index: Arc<StreamSessionIndexService>,
    fence: OplogFence,
    retired: AtomicBool,
    close: Mutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

impl std::fmt::Debug for SealedOplog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedOplog").field("id", &self.id).finish()
    }
}

impl SealedOplog {
    pub(crate) fn new(
        id: OwnedAgentId,
        record: Box<SealRecord>,
        blobs: Arc<dyn BlobStorage + Send + Sync>,
        payloads: Arc<dyn OplogService>,
        epoch: Option<ShardEpoch>,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<Self> {
        let fence = OplogFence {
            agent_id: id.agent_id,
            expected_epoch: epoch.unwrap_or(record.writer_epoch),
            actual_epoch: None,
        };
        Arc::new_cyclic(|weak: &std::sync::Weak<Self>| {
            let open: std::sync::Weak<dyn Oplog> = weak.clone();
            let stream_session_index = Arc::new(StreamSessionIndexService::new_frozen(
                open,
                id.clone(),
                record.agent_mode,
                record.fingerprint,
                record.last_index,
            ));
            Self {
                id,
                record,
                blobs,
                payloads,
                stream_session_index,
                fence,
                retired: AtomicBool::new(false),
                close: Mutex::new(Some(close)),
            }
        })
    }

    pub(crate) fn stream_session_index(&self) -> Arc<StreamSessionIndexService> {
        self.stream_session_index.clone()
    }

    fn refusal(&self) -> OplogError {
        OplogError::Fenced(self.fence.clone())
    }
}

impl Drop for SealedOplog {
    fn drop(&mut self) {
        if let Some(close) = self.close.get_mut().unwrap().take() {
            close();
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl Oplog for SealedOplog {
    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }
    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }
    fn fence(&self) -> Option<OplogFence> {
        Some(self.fence.clone())
    }

    fn enqueue_add(&self, _entry: OplogEntry) -> OplogAddReceipt {
        futures::future::ready(Err(self.refusal())).boxed()
    }

    fn enqueue_add_pair(
        &self,
        _start: OplogEntry,
        _make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> OplogAddPairReceipt {
        futures::future::ready(Err(self.refusal())).boxed()
    }

    async fn add_durable_stream_batch(
        &self,
        _make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        Err(self.refusal())
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        _serialized_request: Vec<u8>,
        _build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        Err(self.refusal())
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        _build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        Err(self.refusal())
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        0
    }
    async fn try_drop_prefix(&self, _last_dropped_id: OplogIndex) -> Result<u64, String> {
        Err(self.refusal().to_string())
    }
    async fn commit(
        &self,
        _level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        Err(self.refusal())
    }
    async fn current_oplog_index(&self) -> OplogIndex {
        self.record.last_index
    }
    async fn raw_durable_stream_session_status(
        &self,
        session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> RawDurableStreamSessionStatus {
        let status = if session_key.callee_environment_id != self.id.environment_id
            || session_key.callee != self.id.agent_id
            || session_key.callee_fingerprint != self.record.fingerprint
        {
            Err("frozen stream session identity mismatch".into())
        } else {
            self.stream_session_index
                .lookup_persisted(
                    &self.id,
                    self.record.agent_mode,
                    self.record.fingerprint,
                    self.record.last_index,
                    &session_key.idempotency_key,
                )
                .await
                .map_err(|error| error.to_string())
        };
        RawDurableStreamSessionStatus {
            watermark: self.record.last_index,
            status,
        }
    }
    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }
    async fn wait_for_replicas(
        &self,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<bool, OplogError> {
        Err(self.refusal())
    }

    async fn read_exact(
        &self,
        start: OplogIndex,
        count: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, reader::UnreadableOplog> {
        read_sealed(self.blobs.as_ref(), &self.id, &self.record, start, count)
            .await
            .map_err(|error| reader::UnreadableOplog::new(&self.id.agent_id, error))
    }

    async fn read_source(
        &self,
        start: OplogIndex,
        count: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, reader::UnreadableOplog> {
        read_sealed_source(self.blobs.as_ref(), &self.id, &self.record, start, count)
            .await
            .map_err(|error| reader::UnreadableOplog::new(&self.id.agent_id, error))
    }

    async fn length(&self) -> Result<u64, reader::UnreadableOplog> {
        Ok(self
            .record
            .manifest
            .values()
            .flat_map(|manifest| manifest.values())
            .map(|chunk| chunk.count)
            .sum())
    }

    async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
        Err(self.refusal().to_string())
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.payloads
            .download_raw_payload(&self.id, self.record.agent_mode, payload_id, md5_hash)
            .await
    }

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.payloads
            .download_raw_payload_classified(&self.id, self.record.agent_mode, payload_id, md5_hash)
            .await
    }
}
