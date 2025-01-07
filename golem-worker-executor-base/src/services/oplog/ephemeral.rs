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

use crate::metrics::oplog::record_oplog_call;
use crate::services::oplog::multilayer::OplogArchive;
use crate::services::oplog::{CommitLevel, Oplog};
use async_mutex::Mutex;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::OwnedWorkerId;
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

pub struct EphemeralOplog {
    owned_worker_id: OwnedWorkerId,
    primary: Arc<dyn Oplog + Send + Sync>,
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
}

impl EphemeralOplogState {
    async fn add(&mut self, entry: OplogEntry) {
        self.buffer.push_back(entry);
        if self.buffer.len() > self.max_operations_before_commit as usize {
            self.commit().await;
        }
        self.last_oplog_idx = self.last_oplog_idx.next();
    }

    async fn commit(&mut self) {
        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();

        let mut pairs = Vec::new();
        for entry in entries {
            let oplog_idx = self.last_committed_idx.next();
            pairs.push((oplog_idx, entry));
            self.last_committed_idx = oplog_idx;
        }

        self.target.append(pairs).await
    }
}

impl EphemeralOplog {
    pub async fn new(
        owned_worker_id: OwnedWorkerId,
        last_oplog_idx: OplogIndex,
        max_operations_before_commit: u64,
        primary: Arc<dyn Oplog + Send + Sync>,
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
    async fn add(&self, entry: OplogEntry) {
        record_oplog_call("add");
        let mut state = self.state.lock().await;
        state.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        record_oplog_call("drop_prefix");
        self.target.drop_prefix(last_dropped_id).await;
    }

    async fn commit(&self, level: CommitLevel) {
        record_oplog_call("commit");
        match level {
            CommitLevel::Immediate => {
                let mut state = self.state.lock().await;
                state.commit().await
            }
            CommitLevel::Always => {
                let clone = self.state.clone();
                tokio::spawn(
                    async move {
                        let mut state = clone.lock().await;
                        state.commit().await
                    }
                    .in_current_span(),
                );
            }
            CommitLevel::DurableOnly => {}
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        record_oplog_call("current_oplog_index");
        let state = self.state.lock().await;
        state.last_oplog_idx
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

    async fn length(&self) -> u64 {
        record_oplog_call("length");
        self.target.length().await
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        // Storing oplog payloads through the primary layer
        self.primary.upload_payload(data).await
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        // Downloading oplog payloads through the primary layer
        self.primary.download_payload(payload).await
    }
}
