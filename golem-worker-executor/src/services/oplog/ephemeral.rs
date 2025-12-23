// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::metrics::oplog::record_oplog_call;
use crate::services::oplog::multilayer::OplogArchive;
use crate::services::oplog::{CommitLevel, Oplog};
use async_lock::Mutex;
use async_trait::async_trait;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use golem_common::model::OwnedWorkerId;
use std::cmp::{max, min};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

pub struct EphemeralOplog {
    owned_worker_id: OwnedWorkerId,
    primary: Arc<dyn Oplog>,
    target: Arc<dyn OplogArchive + Send + Sync>,
    state: Arc<Mutex<EphemeralOplogState>>,
    close_fn: Option<Box<dyn FnOnce() + Send + Sync>>,
}

struct EphemeralOplogState {
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    max_operations_before_commit: u64,
    target: Arc<dyn OplogArchive + Send + Sync>,
    last_added_non_hint_entry: Option<OplogIndex>,
}

impl EphemeralOplogState {
    async fn add(&mut self, entry: OplogEntry) -> OplogIndex {
        let is_hint = entry.is_hint();
        self.buffer.push_back(entry);
        if self.buffer.len() > self.max_operations_before_commit as usize {
            self.commit().await;
        }
        self.last_oplog_idx = self.last_oplog_idx.next();
        if !is_hint {
            self.last_added_non_hint_entry = Some(self.last_oplog_idx);
        }
        self.last_oplog_idx
    }

    async fn commit(&mut self) -> BTreeMap<OplogIndex, OplogEntry> {
        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();

        let mut result = BTreeMap::new();
        let mut pairs = Vec::new();
        for entry in entries {
            let oplog_idx = self.last_committed_idx.next();
            result.insert(oplog_idx, entry.clone());
            pairs.push((oplog_idx, entry));
            self.last_committed_idx = oplog_idx;
        }

        self.target.append(pairs).await;
        result
    }
}

impl EphemeralOplog {
    pub async fn new(
        owned_worker_id: OwnedWorkerId,
        last_oplog_idx: OplogIndex,
        max_operations_before_commit: u64,
        primary: Arc<dyn Oplog>,
        target: Arc<dyn OplogArchive + Send + Sync>,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            owned_worker_id,
            primary,
            target: target.clone(),
            state: Arc::new(Mutex::new(EphemeralOplogState {
                buffer: VecDeque::new(),
                last_oplog_idx,
                last_committed_idx: last_oplog_idx,
                max_operations_before_commit,
                target,
                last_added_non_hint_entry: None,
            })),
            close_fn: Some(close),
        }
    }
}

impl Drop for EphemeralOplog {
    fn drop(&mut self) {
        if let Some(close_fn) = self.close_fn.take() {
            close_fn();
        }
    }
}

impl Debug for EphemeralOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EphemeralOplog")
            .field("worker_id", &self.owned_worker_id)
            .finish()
    }
}

#[async_trait]
impl Oplog for EphemeralOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        record_oplog_call("add");
        let mut state = self.state.lock().await;
        state.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        record_oplog_call("drop_prefix");
        self.target.drop_prefix(last_dropped_id).await
    }

    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("commit");
        match level {
            CommitLevel::Always => {
                let mut state = self.state.lock().await;
                state.commit().await
            }
            CommitLevel::DurableOnly => BTreeMap::new(),
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        record_oplog_call("current_oplog_index");
        let state = self.state.lock().await;
        state.last_oplog_idx
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        record_oplog_call("last_added_non_hint_entry");
        let state = self.state.lock().await;
        state.last_added_non_hint_entry
    }

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        record_oplog_call("wait_for_replicas");
        // Not supported
        false
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        record_oplog_call("read");
        let entries = self.target.read(oplog_index, 1).await;
        if let Some(entry) = entries.get(&oplog_index) {
            entry.clone()
        } else {
            panic!(
                "Missing oplog entry {oplog_index} in {:?} for ephemeral oplog",
                self.target
            );
        }
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read_many");
        let state = self.state.lock().await;

        let last_idx = oplog_index.range_end(n);

        if last_idx < state.last_committed_idx {
            // The whole range is already committed no further action needed
            self.target.read(oplog_index, n).await
        } else {
            // There can be some uncommitted entries in the buffer
            let mut result = self
                .target
                .read_range(oplog_index, state.last_committed_idx)
                .await;

            let uncommitted_count = last_idx.distance_from(state.last_committed_idx);
            let buffered_to_take =
                min(max(0, uncommitted_count), state.buffer.len() as i64) as usize;

            let mut current = state.last_committed_idx;
            for idx in 0..buffered_to_take {
                current = current.next();
                let entry = state.buffer[idx].clone();
                result.insert(current, entry);
            }

            result
        }
    }

    async fn length(&self) -> u64 {
        record_oplog_call("length");
        self.target.length().await
    }

    async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.primary.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.primary
            .download_raw_payload(payload_id, md5_hash)
            .await
    }
}
