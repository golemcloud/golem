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
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::WorkerId;
use tracing::error;

use crate::metrics::oplog::record_oplog_call;
use crate::storage::indexed::{IndexedStorage, IndexedStorageLabelledApi, IndexedStorageNamespace};

#[async_trait]
pub trait OplogService {
    async fn create(
        &self,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync>;
    async fn open(&self, worker_id: &WorkerId) -> Arc<dyn Oplog + Send + Sync>;

    async fn get_size(&self, worker_id: &WorkerId) -> u64;

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
}

#[derive(Clone, Debug)]
pub struct DefaultOplogService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
}

impl DefaultOplogService {
    pub async fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        max_operations_before_commit: u64,
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
            replicas,
            max_operations_before_commit,
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

        self.open(worker_id).await
    }

    async fn open(&self, worker_id: &WorkerId) -> Arc<dyn Oplog + Send + Sync> {
        let key = Self::oplog_key(worker_id);
        let oplog_size: u64 = self
            .indexed_storage
            .with("oplog", "open")
            .length(IndexedStorageNamespace::OpLog, &key)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to get oplog size for worker {worker_id} from indexed storage: {err}"
                )
            });
        Arc::new(DefaultOplog::new(
            self.indexed_storage.clone(),
            self.replicas,
            self.max_operations_before_commit,
            key,
            oplog_size,
        ))
    }

    async fn get_size(&self, worker_id: &WorkerId) -> u64 {
        record_oplog_call("get_size");

        self.indexed_storage
            .with("oplog", "get_size")
            .length(IndexedStorageNamespace::OpLog, &Self::oplog_key(worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to get oplog size for worker {worker_id} from indexed storage: {err}"
                )
            })
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
        replicas: u8,
        max_operations_before_commit: u64,
        key: String,
        oplog_size: u64,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(DefaultOplogState {
                indexed_storage,
                replicas,
                max_operations_before_commit,
                key: key.clone(),
                buffer: VecDeque::new(),
                last_committed_idx: oplog_size,
                last_oplog_idx: oplog_size,
            })),
            key,
        }
    }
}

struct DefaultOplogState {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: u64,
    last_committed_idx: u64,
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
}

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
        _worker_id: &WorkerId,
        _initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn open(&self, _worker_id: &WorkerId) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn get_size(&self, _worker_id: &WorkerId) -> u64 {
        unimplemented!()
    }

    async fn delete(&self, _worker_id: &WorkerId) {
        unimplemented!()
    }

    async fn read(&self, _worker_id: &WorkerId, _idx: u64, _n: u64) -> Vec<OplogEntry> {
        unimplemented!()
    }
}
