// Copyright 2024-2025 Golem Cloud
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
use crate::metrics::oplog::record_oplog_call;
use crate::model::ExecutionStatus;
use crate::services::oplog::{CommitLevel, OpenOplogs, Oplog, OplogConstructor, OplogService};
use crate::storage::indexed::{IndexedStorage, IndexedStorageLabelledApi, IndexedStorageNamespace};
use async_mutex::Mutex;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload, PayloadId};
use golem_common::model::{
    AccountId, ComponentId, OwnedWorkerId, ScanCursor, WorkerId, WorkerMetadata,
};
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::error;

/// The primary oplog service implementation, suitable for direct use (top level of a multi-layered setup).
///
/// Stores and retrieves individual oplog entries from the `IndexedStorage` implementation configured for
/// the executor.
#[derive(Clone, Debug)]
pub struct PrimaryOplogService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    oplogs: OpenOplogs,
}

impl PrimaryOplogService {
    pub async fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_operations_before_commit: u64,
        max_payload_size: usize,
    ) -> Self {
        let replicas = indexed_storage
            .with("oplog", "new")
            .number_of_replicas()
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get the number of replicas of the indexed storage: {err}")
            });
        Self {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_payload_size,
            oplogs: OpenOplogs::new("primary oplog"),
        }
    }

    fn oplog_key(worker_id: &WorkerId) -> String {
        worker_id.to_redis_key()
    }

    pub fn key_pattern(component_id: &ComponentId) -> String {
        format!("{}*", component_id.0)
    }

    pub fn get_worker_id_from_key(key: &str, component_id: &ComponentId) -> WorkerId {
        let redis_prefix = format!("{}:", component_id.0);
        if key.starts_with(&redis_prefix) {
            let worker_name = &key[redis_prefix.len()..];
            WorkerId {
                worker_name: worker_name.to_string(),
                component_id: component_id.clone(),
            }
        } else {
            panic!("Failed to get worker id from indexed storage key: {key}")
        }
    }

    async fn upload_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_payload_size: usize,
        owned_worker_id: &OwnedWorkerId,
        data: &[u8],
    ) -> Result<OplogPayload, String> {
        if data.len() > max_payload_size {
            let payload_id: PayloadId = PayloadId::new();
            let md5_hash = md5::compute(data).to_vec();

            blob_storage
                .put_raw(
                    "oplog",
                    "upload_payload",
                    BlobStorageNamespace::OplogPayload {
                        account_id: owned_worker_id.account_id(),
                        worker_id: owned_worker_id.worker_id(),
                    },
                    Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
                    data,
                )
                .await?;

            Ok(OplogPayload::External {
                payload_id,
                md5_hash,
            })
        } else {
            Ok(OplogPayload::Inline(data.to_vec()))
        }
    }

    async fn download_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        owned_worker_id: &OwnedWorkerId,
        payload: &OplogPayload,
    ) -> Result<Bytes, String> {
        match payload {
            OplogPayload::Inline(data) => Ok(Bytes::copy_from_slice(data)),
            OplogPayload::External {
                payload_id,
                md5_hash,
            } => {
                blob_storage
                    .get_raw(
                        "oplog",
                        "download_payload",
                        BlobStorageNamespace::OplogPayload {
                            account_id: owned_worker_id.account_id(),
                            worker_id: owned_worker_id.worker_id(),
                        },
                        Path::new(&format!("{}/{}", hex::encode(md5_hash), payload_id.0)),
                    )
                    .await?
                    .ok_or(format!("Payload not found (worker: {owned_worker_id}, payload_id: {payload_id}, md5 hash: {md5_hash:02X?})"))
            }
        }
    }
}

#[async_trait]
impl OplogService for PrimaryOplogService {
    async fn create(
        &self,
        owned_worker_id: &OwnedWorkerId,
        initial_entry: OplogEntry,
        initial_worker_metadata: WorkerMetadata,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    ) -> Arc<dyn Oplog + Send + Sync> {
        record_oplog_call("create");

        let key = Self::oplog_key(&owned_worker_id.worker_id);
        let already_exists: bool = self
            .indexed_storage
            .with("oplog", "create")
            .exists(IndexedStorageNamespace::OpLog, &key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to check if oplog exists for worker {owned_worker_id} in indexed storage: {err}")
            });

        if already_exists {
            panic!("oplog for worker {owned_worker_id} already exists in indexed storage")
        }

        self.indexed_storage
            .with_entity("oplog", "create", "entry")
            .append(IndexedStorageNamespace::OpLog, &key, 1, &initial_entry)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to append initial oplog entry for worker {owned_worker_id} in indexed storage: {err}"
                )
            });

        self.open(
            owned_worker_id,
            OplogIndex::INITIAL,
            initial_worker_metadata,
            execution_status,
        )
        .await
    }

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: OplogIndex,
        _initial_worker_metadata: WorkerMetadata,
        _execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    ) -> Arc<dyn Oplog + Send + Sync> {
        record_oplog_call("open");

        let key = Self::oplog_key(&owned_worker_id.worker_id);

        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateOplogConstructor::new(
                    self.indexed_storage.clone(),
                    self.blob_storage.clone(),
                    self.replicas,
                    self.max_operations_before_commit,
                    self.max_payload_size,
                    key,
                    last_oplog_index,
                    owned_worker_id.clone(),
                ),
            )
            .await
    }

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        record_oplog_call("get_last_index");

        OplogIndex::from_u64(
        self.indexed_storage
            .with_entity("oplog", "get_last_index", "entry")
            .last_id(IndexedStorageNamespace::OpLog, &Self::oplog_key(&owned_worker_id.worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to get last oplog index for worker {owned_worker_id} from indexed storage: {err}"
                )
            })
            .unwrap_or_default()
        )
    }

    async fn delete(&self, owned_worker_id: &OwnedWorkerId) {
        record_oplog_call("delete");

        self.indexed_storage
            .with("oplog", "delete")
            .delete(
                IndexedStorageNamespace::OpLog,
                &Self::oplog_key(&owned_worker_id.worker_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to drop oplog for worker {owned_worker_id} in indexed storage: {err}"
                )
            });
    }

    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read");

        self.indexed_storage
            .with_entity("oplog", "read", "entry")
            .read(
                IndexedStorageNamespace::OpLog,
                &Self::oplog_key(&owned_worker_id.worker_id),
                idx.into(),
                idx.range_end(n).into(),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to read oplog for worker {owned_worker_id} from indexed storage: {err}"
                )
            })
            .into_iter()
            .map(|(k, v): (u64, OplogEntry)| (OplogIndex::from_u64(k), v))
            .collect()
    }

    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool {
        record_oplog_call("exists");

        self.indexed_storage
            .with("oplog", "exists")
            .exists(IndexedStorageNamespace::OpLog, &Self::oplog_key(&owned_worker_id.worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to check if oplog exists for worker {owned_worker_id} in indexed storage: {err}")
            })
    }

    async fn scan_for_component(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError> {
        record_oplog_call("scan");

        let (cursor, keys) = self
            .indexed_storage
            .with("oplog", "scan")
            .scan(
                IndexedStorageNamespace::OpLog,
                &Self::key_pattern(component_id),
                cursor.cursor,
                count,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to scan for component {component_id} in indexed storage: {err}")
            });

        Ok((
            ScanCursor { cursor, layer: 0 },
            keys.into_iter()
                .map(|key| OwnedWorkerId {
                    worker_id: Self::get_worker_id_from_key(&key, component_id),
                    account_id: account_id.clone(),
                })
                .collect(),
        ))
    }

    async fn upload_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: &[u8],
    ) -> Result<OplogPayload, String> {
        Self::upload_payload(
            self.blob_storage.clone(),
            self.max_payload_size,
            owned_worker_id,
            data,
        )
        .await
    }

    async fn download_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload: &OplogPayload,
    ) -> Result<Bytes, String> {
        Self::download_payload(self.blob_storage.clone(), owned_worker_id, payload).await
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    key: String,
    last_oplog_idx: OplogIndex,
    owned_worker_id: OwnedWorkerId,
}

impl CreateOplogConstructor {
    fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        key: String,
        last_oplog_idx: OplogIndex,
        owned_worker_id: OwnedWorkerId,
    ) -> Self {
        Self {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_payload_size,
            key,
            last_oplog_idx,
            owned_worker_id,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(
        self,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog + Send + Sync> {
        Arc::new(PrimaryOplog::new(
            self.indexed_storage,
            self.blob_storage,
            self.replicas,
            self.max_operations_before_commit,
            self.max_payload_size,
            self.key,
            self.last_oplog_idx,
            self.owned_worker_id,
            close,
        ))
    }
}

struct PrimaryOplog {
    state: Arc<Mutex<PrimaryOplogState>>,
    key: String,
    close: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Drop for PrimaryOplog {
    fn drop(&mut self) {
        if let Some(close) = self.close.take() {
            close();
        }
    }
}

impl PrimaryOplog {
    fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        key: String,
        last_oplog_idx: OplogIndex,
        owned_worker_id: OwnedWorkerId,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(PrimaryOplogState {
                indexed_storage,
                blob_storage,
                replicas,
                max_operations_before_commit,
                max_payload_size,
                key: key.clone(),
                buffer: VecDeque::new(),
                last_committed_idx: last_oplog_idx,
                last_oplog_idx,
                owned_worker_id,
            })),
            key,
            close: Some(close),
        }
    }
}

struct PrimaryOplogState {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    owned_worker_id: OwnedWorkerId,
}

impl PrimaryOplogState {
    async fn append(&mut self, entries: &[OplogEntry]) {
        record_oplog_call("append");

        for entry in entries {
            let oplog_idx = self.last_committed_idx.next();
            self.indexed_storage
                .with_entity("oplog", "append", "entry")
                .append(
                    IndexedStorageNamespace::OpLog,
                    &self.key,
                    oplog_idx.into(),
                    entry,
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to append oplog entry for {} in indexed storage: {err}",
                        self.key
                    )
                });
            self.last_committed_idx = oplog_idx;
        }
    }

    async fn add(&mut self, entry: OplogEntry) {
        record_oplog_call("add");

        self.buffer.push_back(entry);
        if self.buffer.len() > self.max_operations_before_commit as usize {
            self.commit().await;
        }
        self.last_oplog_idx = self.last_oplog_idx.next();
    }

    async fn commit(&mut self) {
        record_oplog_call("commit");

        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();
        self.append(&entries).await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        record_oplog_call("wait_for_replicas");

        let replicas = replicas.min(self.replicas);
        match self
            .indexed_storage
            .with("oplog", "wait_for_replicas")
            .wait_for_replicas(replicas, timeout)
            .await
        {
            Ok(n) => n == replicas,
            Err(err) => {
                error!("Failed to wait for replicas to sync indexed storage: {err}");
                false
            }
        }
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        record_oplog_call("read");

        let entries: Vec<(u64, OplogEntry)> = self
            .indexed_storage
            .with_entity("oplog", "read", "entry")
            .read(
                IndexedStorageNamespace::OpLog,
                &self.key,
                oplog_index.into(),
                oplog_index.into(),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to read oplog entry {oplog_index} from {} from indexed storage: {err}",
                    self.key
                )
            });

        entries
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry {oplog_index} for {} in indexed storage",
                    self.key
                )
            })
            .1
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        record_oplog_call("drop_prefix");

        self.indexed_storage
            .with("oplog", "drop_prefix")
            .drop_prefix(
                IndexedStorageNamespace::OpLog,
                &self.key,
                last_dropped_id.into(),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to drop prefix for {} in indexed storage: {err}",
                    self.key
                )
            });
    }

    async fn length(&self) -> u64 {
        record_oplog_call("length");

        self.indexed_storage
            .with("oplog", "length")
            .length(IndexedStorageNamespace::OpLog, &self.key)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to get the length of oplog for {} from indexed storage: {err}",
                    self.key
                )
            })
    }

    async fn delete(&self) {
        record_oplog_call("delete");

        self.indexed_storage
            .with("oplog", "delete")
            .delete(IndexedStorageNamespace::OpLog, &self.key)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to delete oplog for {} from indexed storage: {err}",
                    self.key
                )
            });
    }
}

impl Debug for PrimaryOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

#[async_trait]
impl Oplog for PrimaryOplog {
    async fn add(&self, entry: OplogEntry) {
        let mut state = self.state.lock().await;
        state.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        let state = self.state.lock().await;
        state.drop_prefix(last_dropped_id).await;
        let remaining = state.length().await;
        if remaining == 0 {
            state.delete().await;
        }
    }

    async fn commit(&self, _level: CommitLevel) {
        let mut state = self.state.lock().await;
        state.commit().await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        let state = self.state.lock().await;
        state.last_oplog_idx
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        let mut state = self.state.lock().await;
        state.commit().await;
        state.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let state = self.state.lock().await;
        state.read(oplog_index).await
    }

    async fn length(&self) -> u64 {
        let state = self.state.lock().await;
        state.length().await
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        let (blob_storage, owned_worker_id, max_length) = {
            let state = self.state.lock().await;
            (
                state.blob_storage.clone(),
                state.owned_worker_id.clone(),
                state.max_payload_size,
            )
        };
        PrimaryOplogService::upload_payload(blob_storage, max_length, &owned_worker_id, data).await
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        let (blob_storage, owned_worker_id) = {
            let state = self.state.lock().await;
            (state.blob_storage.clone(), state.owned_worker_id.clone())
        };
        PrimaryOplogService::download_payload(blob_storage, &owned_worker_id, payload).await
    }
}
