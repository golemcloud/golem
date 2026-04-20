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

use crate::metrics::oplog::record_oplog_call;
use crate::services::oplog::multilayer::OplogArchive;
use crate::services::oplog::{CommitLevel, Oplog};
use async_lock::Mutex;
use async_trait::async_trait;
use golem_common::model::OwnedAgentId;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use std::cmp::{max, min};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub struct EphemeralOplog {
    owned_agent_id: OwnedAgentId,
    primary: Arc<dyn Oplog>,
    target: Arc<dyn OplogArchive + Send + Sync>,
    state: Arc<Mutex<EphemeralOplogState>>,
    /// Shared with the state so tests can read it without acquiring the lock.
    last_written_idx: Arc<AtomicU64>,
    close_fn: Option<Box<dyn FnOnce() + Send + Sync>>,
}

struct EphemeralOplogState {
    buffer: VecDeque<OplogEntry>,
    /// Entries that have been logically committed (assigned oplog indices and
    /// removed from `buffer`) but whose blob/S3 write is still in-flight as a
    /// background task.  Reads consult this map before going to S3 so that the
    /// invocation loop is never blocked waiting for a storage round-trip.
    ///
    /// Entries are lazily evicted on the next `commit()` call once the
    /// background write has confirmed completion via `last_written_idx`.
    pending_background: BTreeMap<OplogIndex, OplogEntry>,
    /// The highest oplog index that has been successfully written to the blob
    /// archive by a background task.  Updated atomically by background tasks
    /// (no lock needed); read inside `commit()` which already holds the state
    /// lock, so no extra synchronisation is required at the eviction site.
    last_written_idx: Arc<AtomicU64>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    max_operations_before_commit: u64,
    target: Arc<dyn OplogArchive + Send + Sync>,
    last_added_non_hint_entry: Option<OplogIndex>,
}

impl EphemeralOplogState {
    fn add(&mut self, entry: OplogEntry) -> OplogIndex {
        let is_hint = entry.is_hint();
        self.buffer.push_back(entry);
        if self.buffer.len() > self.max_operations_before_commit as usize {
            self.commit();
        }
        self.last_oplog_idx = self.last_oplog_idx.next();
        if !is_hint {
            self.last_added_non_hint_entry = Some(self.last_oplog_idx);
        }
        self.last_oplog_idx
    }

    /// Flush the in-memory buffer: assigns oplog indices, caches entries in
    /// `pending_background` for immediate reads, and spawns a background task
    /// to write them to the blob archive.  Returns immediately — no S3 wait.
    ///
    /// Also lazily evicts entries from `pending_background` whose background
    /// write has already completed (signalled via `last_written_idx`).  This
    /// piggybacks on the existing lock acquisition so no extra locking is needed.
    fn commit(&mut self) -> BTreeMap<OplogIndex, OplogEntry> {
        // Lazily evict entries confirmed written by previous background tasks.
        // `last_written_idx` is written by background tasks with Release ordering
        // and read here (already inside the state lock) with Acquire ordering.
        let confirmed_up_to = OplogIndex::from_u64(self.last_written_idx.load(Ordering::Acquire));
        if confirmed_up_to > OplogIndex::NONE {
            self.pending_background
                .retain(|idx, _| *idx > confirmed_up_to);
        }

        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();

        let mut result = BTreeMap::new();
        let mut pairs = Vec::new();
        for entry in entries {
            let oplog_idx = self.last_committed_idx.next();
            result.insert(oplog_idx, entry.clone());
            self.pending_background.insert(oplog_idx, entry.clone());
            pairs.push((oplog_idx, entry));
            self.last_committed_idx = oplog_idx;
        }

        if !pairs.is_empty() {
            let target = self.target.clone();
            // The highest index in this batch — after the write completes the
            // background task records this so the next commit can evict.
            let max_idx: u64 = pairs.last().unwrap().0.into();
            let last_written_idx = self.last_written_idx.clone();
            tokio::spawn(async move {
                target.append(pairs).await;
                // Update with the max index this batch wrote.  Use fetch_max so
                // concurrent or out-of-order batches never regress the value.
                last_written_idx.fetch_max(max_idx, Ordering::Release);
            });
        }

        result
    }
}

impl EphemeralOplog {
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        last_oplog_idx: OplogIndex,
        max_operations_before_commit: u64,
        primary: Arc<dyn Oplog>,
        target: Arc<dyn OplogArchive + Send + Sync>,
        initial_pending: BTreeMap<OplogIndex, OplogEntry>,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let last_written_idx = Arc::new(AtomicU64::new(0));
        Self {
            owned_agent_id,
            primary,
            target: target.clone(),
            last_written_idx: last_written_idx.clone(),
            state: Arc::new(Mutex::new(EphemeralOplogState {
                buffer: VecDeque::new(),
                pending_background: initial_pending,
                last_written_idx,
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

#[cfg(test)]
impl EphemeralOplog {
    /// Returns the oplog indices currently held in the pending-background cache.
    /// Used in tests to assert exactly which entries are still awaiting S3
    /// confirmation and which have been evicted after a successful write.
    pub async fn pending_background_keys(&self) -> Vec<OplogIndex> {
        self.state
            .lock()
            .await
            .pending_background
            .keys()
            .copied()
            .collect()
    }

    /// Returns the highest oplog index confirmed written by a completed
    /// background task.  Reads the atomic directly — no lock needed.
    /// Use this in tests to wait until a specific batch has been flushed
    /// (including the `fetch_max` call that follows `append`), avoiding
    /// any timing-dependent sleeps or yields.
    pub fn confirmed_written_up_to(&self) -> u64 {
        self.last_written_idx.load(Ordering::Acquire)
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
            .field("agent_id", &self.owned_agent_id)
            .finish()
    }
}

#[async_trait]
impl Oplog for EphemeralOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        record_oplog_call("add");
        let mut state = self.state.lock().await;
        state.add(entry)
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
                state.commit()
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
        // Check in-memory caches first to avoid blocking on a background S3 write.
        {
            let state = self.state.lock().await;
            if let Some(entry) = state.pending_background.get(&oplog_index) {
                return entry.clone();
            }
            // Also check the uncommitted buffer.
            let committed_base: u64 = state.last_committed_idx.into();
            let target_idx: u64 = oplog_index.into();
            if target_idx > committed_base {
                let offset = (target_idx - committed_base - 1) as usize;
                if let Some(entry) = state.buffer.get(offset) {
                    return entry.clone();
                }
            }
        }
        // Fall back to S3 for older entries already persisted and evicted from the
        // in-memory window.
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

        // Collect from pending_background (committed but S3 write still in-flight).
        let mut result: BTreeMap<OplogIndex, OplogEntry> = state
            .pending_background
            .range(oplog_index..=last_idx)
            .map(|(k, v)| (*k, v.clone()))
            .collect();

        // Collect uncommitted buffer entries.
        let uncommitted_count = last_idx.distance_from(state.last_committed_idx);
        let buffered_to_take = min(max(0, uncommitted_count), state.buffer.len() as i64) as usize;
        let mut current = state.last_committed_idx;
        for idx in 0..buffered_to_take {
            current = current.next();
            if current >= oplog_index && current <= last_idx {
                result
                    .entry(current)
                    .or_insert_with(|| state.buffer[idx].clone());
            }
        }

        // For indices that predate the in-memory window, fall back to S3.
        let first_in_memory: u64 = state
            .pending_background
            .keys()
            .next()
            .copied()
            .map(Into::into)
            .unwrap_or_else(|| Into::<u64>::into(state.last_committed_idx) + 1);
        let oplog_index_u64: u64 = oplog_index.into();
        if oplog_index_u64 < first_in_memory {
            let fetch_end = OplogIndex::from_u64(first_in_memory.saturating_sub(1));
            let s3_entries = self.target.read_range(oplog_index, fetch_end).await;
            for (k, v) in s3_entries {
                result.entry(k).or_insert(v);
            }
        }

        result
    }

    async fn length(&self) -> u64 {
        record_oplog_call("length");
        self.target.length().await
    }

    async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.primary.clone())
    }

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
