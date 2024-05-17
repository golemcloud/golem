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

use async_mutex::Mutex;
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, OplogPayload, PayloadId, UpdateDescription, WrappedFunctionType,
};
use golem_common::model::{
    AccountId, CallingConvention, ComponentVersion, IdempotencyKey, Timestamp, WorkerId,
};
use golem_common::serialization::{serialize, try_deserialize};
use tracing::error;

use crate::metrics::oplog::record_oplog_call;
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};
use crate::storage::indexed::{IndexedStorage, IndexedStorageLabelledApi, IndexedStorageNamespace};

#[async_trait]
pub trait OplogService {
    async fn create(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync>;
    async fn open(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
    ) -> Arc<dyn Oplog + Send + Sync>;

    async fn get_last_index(&self, worker_id: &WorkerId) -> u64;

    async fn delete(&self, worker_id: &WorkerId);

    async fn read(&self, worker_id: &WorkerId, idx: u64, n: u64) -> Vec<OplogEntry>;
}

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Debug {
    async fn add(&self, entry: OplogEntry);
    async fn commit(&self);

    async fn current_oplog_index(&self) -> u64;

    /// Waits until indexed store writes all changes into at least `replicas` replicas (or the maximum
    /// available).
    /// Returns true if the maximum possible number of replicas is reached within the timeout,
    /// otherwise false.
    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool;

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry;

    async fn add_and_commit(&self, entry: OplogEntry) -> OplogIndex {
        let idx = self.current_oplog_index().await;
        self.add(entry).await;
        self.commit().await;
        idx
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String>;
    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String>;
}

#[async_trait]
pub trait OplogOps: Oplog {
    async fn add_imported_function_invoked<R: Encode + Sync>(
        &self,
        function_name: String,
        response: &R,
        wrapped_function_type: WrappedFunctionType,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();

        let payload = self.upload_payload(&serialized_response).await?;
        let entry = OplogEntry::ImportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            response: payload,
            wrapped_function_type,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn add_exported_function_invoked<R: Encode + Sync>(
        &self,
        function_name: String,
        request: &R,
        idempotency_key: IdempotencyKey,
        calling_convention: Option<CallingConvention>,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();

        let payload = self.upload_payload(&serialized_request).await?;
        let entry = OplogEntry::ExportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            request: payload,
            idempotency_key,
            calling_convention,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn add_exported_function_completed<R: Encode + Sync>(
        &self,
        response: &R,
        consumed_fuel: i64,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();

        let payload = self.upload_payload(&serialized_response).await?;
        let entry = OplogEntry::ExportedFunctionCompleted {
            timestamp: Timestamp::now_utc(),
            response: payload,
            consumed_fuel,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn create_snapshot_based_update_description(
        &self,
        target_version: ComponentVersion,
        payload: &[u8],
    ) -> Result<UpdateDescription, String> {
        let payload = self.upload_payload(payload).await?;
        Ok(UpdateDescription::SnapshotBased {
            target_version,
            payload,
        })
    }

    async fn get_payload_of_entry<T: Decode>(
        &self,
        entry: &OplogEntry,
    ) -> Result<Option<T>, String> {
        match entry {
            OplogEntry::ImportedFunctionInvoked { response, .. } => {
                let response_bytes: Bytes = self.download_payload(response).await?;
                try_deserialize(&response_bytes)
            }
            OplogEntry::ExportedFunctionInvoked { request, .. } => {
                let response_bytes: Bytes = self.download_payload(request).await?;
                try_deserialize(&response_bytes)
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = self.download_payload(response).await?;
                try_deserialize(&response_bytes)
            }
            _ => Ok(None),
        }
    }

    async fn get_upload_description_payload(
        &self,
        description: &UpdateDescription,
    ) -> Result<Option<Bytes>, String> {
        match description {
            UpdateDescription::SnapshotBased { payload, .. } => {
                let bytes: Bytes = self.download_payload(payload).await?;
                Ok(Some(bytes))
            }
            UpdateDescription::Automatic { .. } => Ok(None),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DefaultOplogService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
}

impl DefaultOplogService {
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
        }
    }

    fn oplog_key(worker_id: &WorkerId) -> String {
        format!("worker:oplog:{}", worker_id.to_redis_key())
    }
}

#[async_trait]
impl OplogService for DefaultOplogService {
    async fn create(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        record_oplog_call("create");

        let key = Self::oplog_key(worker_id);
        let already_exists: bool = self
            .indexed_storage
            .with("oplog", "create")
            .exists(IndexedStorageNamespace::OpLog, &key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to check if oplog exists for worker {worker_id} in indexed storage: {err}")
            });

        if already_exists {
            panic!("oplog for worker {worker_id} already exists in indexed storage")
        }

        self.indexed_storage
            .with_entity("oplog", "create", "entry")
            .append(IndexedStorageNamespace::OpLog, &key, 1, &initial_entry)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to append initial oplog entry for worker {worker_id} in indexed storage: {err}"
                )
            });

        self.open(account_id, worker_id).await
    }

    async fn open(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
    ) -> Arc<dyn Oplog + Send + Sync> {
        let key = Self::oplog_key(worker_id);
        let last_oplog_index: u64 = self.get_last_index(worker_id).await;

        Arc::new(DefaultOplog::new(
            self.indexed_storage.clone(),
            self.blob_storage.clone(),
            self.replicas,
            self.max_operations_before_commit,
            self.max_payload_size,
            key,
            last_oplog_index,
            worker_id.clone(),
            account_id.clone(),
        ))
    }

    async fn get_last_index(&self, worker_id: &WorkerId) -> u64 {
        record_oplog_call("get_size");

        self.indexed_storage
            .with_entity("oplog", "get_size", "entry")
            .last_id(IndexedStorageNamespace::OpLog, &Self::oplog_key(worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to get oplog size for worker {worker_id} from indexed storage: {err}"
                )
            })
            .unwrap_or_default()
    }

    async fn delete(&self, worker_id: &WorkerId) {
        record_oplog_call("drop");

        self.indexed_storage
            .with("oplog", "drop")
            .delete(IndexedStorageNamespace::OpLog, &Self::oplog_key(worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to drop oplog for worker {worker_id} in indexed storage: {err}")
            });
    }

    async fn read(&self, worker_id: &WorkerId, idx: u64, n: u64) -> Vec<OplogEntry> {
        record_oplog_call("read");

        self.indexed_storage
            .with_entity("oplog", "read", "entry")
            .read(
                IndexedStorageNamespace::OpLog,
                &Self::oplog_key(worker_id),
                idx + 1,
                idx + n,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to read oplog for worker {worker_id} from indexed storage: {err}")
            })
    }
}

struct DefaultOplog {
    state: Arc<Mutex<DefaultOplogState>>,
    key: String,
}

impl DefaultOplog {
    fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        key: String,
        last_oplog_idx: u64,
        worker_id: WorkerId,
        account_id: AccountId,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(DefaultOplogState {
                indexed_storage,
                blob_storage,
                replicas,
                max_operations_before_commit,
                max_payload_size,
                key: key.clone(),
                buffer: VecDeque::new(),
                last_committed_idx: last_oplog_idx,
                last_oplog_idx,
                worker_id,
                account_id,
            })),
            key,
        }
    }
}

struct DefaultOplogState {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: u64,
    last_committed_idx: u64,
    worker_id: WorkerId,
    account_id: AccountId,
}

impl DefaultOplogState {
    async fn append(&mut self, arrays: &[OplogEntry]) {
        record_oplog_call("append");

        for entry in arrays {
            let id = self.last_committed_idx + 1;

            self.indexed_storage
                .with_entity("oplog", "append", "entry")
                .append(IndexedStorageNamespace::OpLog, &self.key, id, entry)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to append oplog entry for {} in indexed storage: {err}",
                        self.key
                    )
                });
            self.last_committed_idx += 1;
        }
    }

    async fn add(&mut self, entry: OplogEntry) {
        self.buffer.push_back(entry);
        if self.buffer.len() > self.max_operations_before_commit as usize {
            self.commit().await;
        }
        self.last_oplog_idx += 1;
    }

    async fn commit(&mut self) {
        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();
        self.append(&entries).await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
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
        let entries: Vec<OplogEntry> = self
            .indexed_storage
            .with_entity("oplog", "read", "entry")
            .read(
                IndexedStorageNamespace::OpLog,
                &self.key,
                oplog_index + 1,
                oplog_index + 1,
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to read oplog entry {oplog_index} from {} from indexed storage: {err}",
                    self.key
                )
            });

        entries.into_iter().next().unwrap_or_else(|| {
            panic!(
                "Missing oplog entry {oplog_index} for {} in indexed storage",
                self.key
            )
        })
    }
}

impl Debug for DefaultOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

#[async_trait]
impl Oplog for DefaultOplog {
    async fn add(&self, entry: OplogEntry) {
        let mut state = self.state.lock().await;
        state.add(entry).await
    }

    async fn commit(&self) {
        let mut state = self.state.lock().await;
        state.commit().await
    }

    async fn current_oplog_index(&self) -> u64 {
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

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        let (blob_storage, worker_id, account_id, max_length) = {
            let state = self.state.lock().await;
            (
                state.blob_storage.clone(),
                state.worker_id.clone(),
                state.account_id.clone(),
                state.max_payload_size,
            )
        };
        if data.len() > max_length {
            let payload_id: PayloadId = PayloadId::new();
            let md5_hash = md5::compute(data).to_vec();

            blob_storage
                .put(
                    "oplog",
                    "upload_payload",
                    BlobStorageNamespace::OplogPayload {
                        account_id: account_id.clone(),
                        worker_id: worker_id.clone(),
                    },
                    Path::new(&format!("{:02X?}/{}", md5_hash, payload_id.0)),
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

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        match payload {
            OplogPayload::Inline(data) => Ok(Bytes::copy_from_slice(data)),
            OplogPayload::External {
                payload_id,
                md5_hash,
            } => {
                let (blob_storage, worker_id, account_id) = {
                    let state = self.state.lock().await;
                    (
                        state.blob_storage.clone(),
                        state.worker_id.clone(),
                        state.account_id.clone(),
                    )
                };
                blob_storage
                    .get(
                        "oplog",
                        "download_payload",
                        BlobStorageNamespace::OplogPayload {
                            account_id: account_id.clone(),
                            worker_id: worker_id.clone(),
                        },
                        Path::new(&format!("{:02X?}/{}", md5_hash, payload_id.0)),
                    )
                    .await?
                    .ok_or(format!("Payload not found (account_id: {account_id}, worker_id: {worker_id}, payload_id: {payload_id}, md5 hash: {md5_hash:02X?})"))
            }
        }
    }
}

#[async_trait]
impl<O: Oplog + ?Sized> OplogOps for O {}

#[cfg(any(feature = "mocks", test))]
pub struct OplogServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for OplogServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl OplogServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl OplogService for OplogServiceMock {
    async fn create(
        &self,
        _account_id: &AccountId,
        _worker_id: &WorkerId,
        _initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn open(
        &self,
        _account_id: &AccountId,
        _worker_id: &WorkerId,
    ) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn get_last_index(&self, _worker_id: &WorkerId) -> u64 {
        unimplemented!()
    }

    async fn delete(&self, _worker_id: &WorkerId) {
        unimplemented!()
    }

    async fn read(&self, _worker_id: &WorkerId, _idx: u64, _n: u64) -> Vec<OplogEntry> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::memory::InMemoryBlobStorage;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::ComponentId;
    use uuid::Uuid;

    fn rounded_ts(ts: Timestamp) -> Timestamp {
        Timestamp::from(ts.to_millis())
    }

    fn rounded(entry: OplogEntry) -> OplogEntry {
        match entry {
            OplogEntry::Create {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                account_id,
            } => OplogEntry::Create {
                timestamp: rounded_ts(timestamp),
                worker_id,
                component_version,
                args,
                env,
                account_id,
            },
            OplogEntry::ImportedFunctionInvoked {
                timestamp,
                function_name,
                response,
                wrapped_function_type,
            } => OplogEntry::ImportedFunctionInvoked {
                timestamp: rounded_ts(timestamp),
                function_name,
                response,
                wrapped_function_type,
            },
            OplogEntry::ExportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                idempotency_key,
                calling_convention,
            } => OplogEntry::ExportedFunctionInvoked {
                timestamp: rounded_ts(timestamp),
                function_name,
                request,
                idempotency_key,
                calling_convention,
            },
            OplogEntry::ExportedFunctionCompleted {
                timestamp,
                response,
                consumed_fuel,
            } => OplogEntry::ExportedFunctionCompleted {
                timestamp: rounded_ts(timestamp),
                response,
                consumed_fuel,
            },
            OplogEntry::Suspend { timestamp } => OplogEntry::Suspend {
                timestamp: rounded_ts(timestamp),
            },
            OplogEntry::NoOp { timestamp } => OplogEntry::NoOp {
                timestamp: rounded_ts(timestamp),
            },
            OplogEntry::Jump { timestamp, jump } => OplogEntry::Jump {
                timestamp: rounded_ts(timestamp),
                jump,
            },
            OplogEntry::Interrupted { timestamp } => OplogEntry::Interrupted {
                timestamp: rounded_ts(timestamp),
            },
            OplogEntry::Exited { timestamp } => OplogEntry::Exited {
                timestamp: rounded_ts(timestamp),
            },
            OplogEntry::ChangeRetryPolicy {
                timestamp,
                new_policy,
            } => OplogEntry::ChangeRetryPolicy {
                timestamp: rounded_ts(timestamp),
                new_policy,
            },
            OplogEntry::BeginAtomicRegion { timestamp } => OplogEntry::BeginAtomicRegion {
                timestamp: rounded_ts(timestamp),
            },
            OplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
            } => OplogEntry::EndAtomicRegion {
                timestamp: rounded_ts(timestamp),
                begin_index,
            },
            OplogEntry::BeginRemoteWrite { timestamp } => OplogEntry::BeginRemoteWrite {
                timestamp: rounded_ts(timestamp),
            },
            OplogEntry::EndRemoteWrite {
                timestamp,
                begin_index,
            } => OplogEntry::EndRemoteWrite {
                timestamp: rounded_ts(timestamp),
                begin_index,
            },
            OplogEntry::PendingUpdate {
                timestamp,
                description,
            } => OplogEntry::PendingUpdate {
                timestamp: rounded_ts(timestamp),
                description,
            },
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
            } => OplogEntry::SuccessfulUpdate {
                timestamp: rounded_ts(timestamp),
                target_version,
            },
            OplogEntry::FailedUpdate {
                timestamp,
                target_version,
                details,
            } => OplogEntry::FailedUpdate {
                timestamp: rounded_ts(timestamp),
                target_version,
                details,
            },
            OplogEntry::Error { timestamp, error } => OplogEntry::Error {
                timestamp: rounded_ts(timestamp),
                error,
            },
            OplogEntry::PendingWorkerInvocation {
                timestamp,
                invocation,
            } => OplogEntry::PendingWorkerInvocation {
                timestamp: rounded_ts(timestamp),
                invocation,
            },
        }
    }

    #[tokio::test]
    async fn open_add_and_read_back() {
        let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let oplog_service = DefaultOplogService::new(indexed_storage, blob_storage, 1, 100).await;
        let account_id = AccountId {
            value: "user1".to_string(),
        };
        let worker_id = WorkerId {
            component_id: ComponentId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        };
        let oplog = oplog_service.open(&account_id, &worker_id).await;

        let entry1 = rounded(OplogEntry::jump(OplogRegion { start: 5, end: 12 }));
        let entry2 = rounded(OplogEntry::suspend());
        let entry3 = rounded(OplogEntry::exited());

        let start_idx = oplog.current_oplog_index().await;
        oplog.add(entry1.clone()).await;
        oplog.add(entry2.clone()).await;
        oplog.add(entry3.clone()).await;
        oplog.commit().await;

        let r1 = oplog.read(start_idx).await;
        let r2 = oplog.read(start_idx + 1).await;
        let r3 = oplog.read(start_idx + 2).await;

        assert_eq!(r1, entry1);
        assert_eq!(r2, entry2);
        assert_eq!(r3, entry3);

        let entries = oplog_service.read(&worker_id, start_idx, 3).await;
        assert_eq!(entries, vec![entry1, entry2, entry3]);
    }

    #[tokio::test]
    async fn entries_with_small_payload() {
        let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let oplog_service = DefaultOplogService::new(indexed_storage, blob_storage, 1, 100).await;
        let account_id = AccountId {
            value: "user1".to_string(),
        };
        let worker_id = WorkerId {
            component_id: ComponentId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        };
        let oplog = oplog_service.open(&account_id, &worker_id).await;

        let start_idx = oplog.current_oplog_index().await;
        let entry1 = rounded(
            oplog
                .add_imported_function_invoked(
                    "f1".to_string(),
                    &"response".to_string(),
                    WrappedFunctionType::ReadRemote,
                )
                .await
                .unwrap(),
        );
        let entry2 = rounded(
            oplog
                .add_exported_function_invoked(
                    "f2".to_string(),
                    &"request".to_string(),
                    IdempotencyKey::fresh(),
                    None,
                )
                .await
                .unwrap(),
        );
        let entry3 = rounded(
            oplog
                .add_exported_function_completed(&"response".to_string(), 42)
                .await
                .unwrap(),
        );

        let desc = oplog
            .create_snapshot_based_update_description(11, &[1, 2, 3])
            .await
            .unwrap();
        let entry4 = rounded(OplogEntry::PendingUpdate {
            timestamp: Timestamp::now_utc(),
            description: desc.clone(),
        });
        oplog.add(entry4.clone()).await;

        oplog.commit().await;

        let r1 = oplog.read(start_idx).await;
        let r2 = oplog.read(start_idx + 1).await;
        let r3 = oplog.read(start_idx + 2).await;
        let r4 = oplog.read(start_idx + 3).await;

        assert_eq!(r1, entry1);
        assert_eq!(r2, entry2);
        assert_eq!(r3, entry3);
        assert_eq!(r4, entry4);

        let entries = oplog_service.read(&worker_id, start_idx, 4).await;
        assert_eq!(
            entries,
            vec![
                entry1.clone(),
                entry2.clone(),
                entry3.clone(),
                entry4.clone()
            ]
        );

        let p1 = oplog
            .get_payload_of_entry::<String>(&entry1)
            .await
            .unwrap()
            .unwrap();
        let p2 = oplog
            .get_payload_of_entry::<String>(&entry2)
            .await
            .unwrap()
            .unwrap();
        let p3 = oplog
            .get_payload_of_entry::<String>(&entry3)
            .await
            .unwrap()
            .unwrap();
        let p4 = oplog
            .get_upload_description_payload(&desc)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(p1, "response");
        assert_eq!(p2, "request");
        assert_eq!(p3, "response");
        assert_eq!(p4, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn entries_with_large_payload() {
        let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let oplog_service = DefaultOplogService::new(indexed_storage, blob_storage, 1, 100).await;
        let account_id = AccountId {
            value: "user1".to_string(),
        };
        let worker_id = WorkerId {
            component_id: ComponentId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        };
        let oplog = oplog_service.open(&account_id, &worker_id).await;

        let large_payload1 = vec![0u8; 1024 * 1024];
        let large_payload2 = vec![1u8; 1024 * 1024];
        let large_payload3 = vec![2u8; 1024 * 1024];
        let large_payload4 = vec![3u8; 1024 * 1024];

        let start_idx = oplog.current_oplog_index().await;
        let entry1 = rounded(
            oplog
                .add_imported_function_invoked(
                    "f1".to_string(),
                    &large_payload1,
                    WrappedFunctionType::ReadRemote,
                )
                .await
                .unwrap(),
        );
        let entry2 = rounded(
            oplog
                .add_exported_function_invoked(
                    "f2".to_string(),
                    &large_payload2,
                    IdempotencyKey::fresh(),
                    None,
                )
                .await
                .unwrap(),
        );
        let entry3 = rounded(
            oplog
                .add_exported_function_completed(&large_payload3, 42)
                .await
                .unwrap(),
        );

        let desc = oplog
            .create_snapshot_based_update_description(11, &large_payload4)
            .await
            .unwrap();
        let entry4 = rounded(OplogEntry::PendingUpdate {
            timestamp: Timestamp::now_utc(),
            description: desc.clone(),
        });
        oplog.add(entry4.clone()).await;

        oplog.commit().await;

        let r1 = oplog.read(start_idx).await;
        let r2 = oplog.read(start_idx + 1).await;
        let r3 = oplog.read(start_idx + 2).await;
        let r4 = oplog.read(start_idx + 3).await;

        assert_eq!(r1, entry1);
        assert_eq!(r2, entry2);
        assert_eq!(r3, entry3);
        assert_eq!(r4, entry4);

        let entries = oplog_service.read(&worker_id, start_idx, 4).await;
        assert_eq!(
            entries,
            vec![
                entry1.clone(),
                entry2.clone(),
                entry3.clone(),
                entry4.clone()
            ]
        );

        let p1 = oplog
            .get_payload_of_entry::<Vec<u8>>(&entry1)
            .await
            .unwrap()
            .unwrap();
        let p2 = oplog
            .get_payload_of_entry::<Vec<u8>>(&entry2)
            .await
            .unwrap()
            .unwrap();
        let p3 = oplog
            .get_payload_of_entry::<Vec<u8>>(&entry3)
            .await
            .unwrap()
            .unwrap();
        let p4 = oplog
            .get_upload_description_payload(&desc)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(p1, large_payload1);
        assert_eq!(p2, large_payload2);
        assert_eq!(p3, large_payload3);
        assert_eq!(p4, large_payload4);
    }
}
