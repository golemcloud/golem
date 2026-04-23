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
use crate::services::oplog::multilayer::{
    BackgroundTransferMessage, InstrumentedOplogArchive, OplogArchive, WrappedOplogArchive,
};
use crate::services::oplog::{CommitLevel, Oplog, OplogService, downcast_oplog};
use async_lock::Mutex;
use async_trait::async_trait;
use golem_common::model::OwnedAgentId;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use nonempty_collections::NEVec;
use std::cmp::{max, min};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use tracing::{Instrument, Level, Span, debug, info, span, warn};

pub struct EphemeralOplog {
    owned_agent_id: OwnedAgentId,
    primary_service: Arc<dyn OplogService>,
    state: Arc<Mutex<EphemeralOplogState>>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    transfer_fiber: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>>,
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
        owned_agent_id: OwnedAgentId,
        last_oplog_idx: OplogIndex,
        max_operations_before_commit: u64,
        primary_service: Arc<dyn OplogService>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
        transfer: UnboundedSender<BackgroundTransferMessage>,
        transfer_fiber: tokio::task::JoinHandle<()>,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let target = lower.first().clone();
        Self {
            owned_agent_id,
            primary_service,
            state: Arc::new(Mutex::new(EphemeralOplogState {
                buffer: VecDeque::new(),
                last_oplog_idx,
                last_committed_idx: last_oplog_idx,
                max_operations_before_commit,
                target,
                last_added_non_hint_entry: None,
            })),
            lower,
            transfer,
            transfer_fiber: Arc::new(StdMutex::new(Some(transfer_fiber))),
            close_fn: Some(close),
        }
    }

    pub async fn try_archive(this: &Arc<dyn Oplog>) -> Option<bool> {
        let this = downcast_oplog::<EphemeralOplog>(this)?;
        Some(this.archive(false).await)
    }

    pub async fn try_archive_blocking(this: &Arc<dyn Oplog>) -> Option<bool> {
        let this = downcast_oplog::<EphemeralOplog>(this)?;
        Some(this.archive(true).await)
    }

    async fn archive(self: &Arc<Self>, blocking: bool) -> bool {
        // With only one lower layer there is nowhere to transfer to.
        if self.lower.len().get() <= 1 {
            return false;
        }

        let (done_tx, done_rx) = if blocking {
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            (Some(done_tx), Some(done_rx))
        } else {
            (None, None)
        };

        // Find the first non-empty lower layer that has a target below it.
        let last_movable = self.lower.len().get() - 1;
        let mut first_non_empty = None;
        for i in 0..last_movable {
            if self.lower[i].length().await > 0 {
                first_non_empty = Some(i);
                break;
            }
        }

        let result = if let Some(source) = first_non_empty {
            let last_idx = self.lower[source].current_oplog_index().await;
            info!(
                "Transferring oplog entries up to index {last_idx} of ephemeral oplog layer {source} to the next layer"
            );
            let keep_alive: Arc<dyn Oplog> = self.clone();
            self.transfer
                .send(BackgroundTransferMessage::TransferFromLower {
                    source,
                    last_transferred_idx: last_idx,
                    keep_alive: Some(keep_alive),
                    done: done_tx,
                })
                .expect("Failed to enqueue transfer of ephemeral oplog entries");
            // Return true if there are more movable layers that could still hold data
            source + 1 < last_movable
        } else {
            // Fully archived
            false
        };

        if let Some(done_rx) = done_rx {
            done_rx
                .await
                .expect("Failed to wait for the archiving to finish");
        }

        result
    }

    /// Spawns the background transfer fiber that processes `TransferFromLower`
    /// messages for ephemeral oplogs.
    pub fn spawn_background_transfer(
        owned_agent_id: OwnedAgentId,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
        rx: UnboundedReceiver<BackgroundTransferMessage>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(
            Self::background_transfer(owned_agent_id, lower, rx).instrument(
                span!(parent: None, Level::INFO, "Ephemeral oplog background transfer")
                    .follows_from(Span::current())
                    .clone(),
            ),
        )
    }

    async fn background_transfer(
        owned_agent_id: OwnedAgentId,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
        mut rx: UnboundedReceiver<BackgroundTransferMessage>,
    ) {
        while let Some(msg) = rx.recv().await {
            match msg {
                BackgroundTransferMessage::TransferFromLower {
                    source,
                    last_transferred_idx,
                    mut keep_alive,
                    done,
                } => {
                    if source + 1 >= lower.len().get() {
                        warn!(
                            "Invalid TransferFromLower source layer {source} — no target layer exists"
                        );
                        let _ = keep_alive.take();
                        if let Some(done) = done {
                            let _ = done.send(());
                        }
                        continue;
                    }

                    info!(
                        "Transferring oplog entries up to index {last_transferred_idx} of ephemeral oplog layer {source} to the next layer"
                    );
                    debug!("Reading entries from ephemeral oplog layer {source}");

                    let source_layer = lower[source].clone();
                    let target_layer = lower[source + 1].clone();

                    let entries: Vec<_> = source_layer
                        .read_prefix(last_transferred_idx)
                        .await
                        .into_iter()
                        .collect();

                    match entries.last() {
                        Some(last_entry) => {
                            let last_dropped_id = last_entry.0;
                            let _ = target_layer.append(entries).await;
                            source_layer.drop_prefix(last_dropped_id).await;
                        }
                        None => {
                            warn!("No entries to transfer from ephemeral oplog layer {source}");
                        }
                    }

                    let _ = keep_alive.take();
                    if let Some(done) = done {
                        let _ = done.send(());
                    }
                }
                BackgroundTransferMessage::TransferFromPrimary {
                    mut keep_alive,
                    done,
                    ..
                } => {
                    // Ephemeral oplogs do not use primary storage — ignore.
                    warn!(
                        "Unexpected TransferFromPrimary message in ephemeral oplog for {}",
                        owned_agent_id
                    );
                    let _ = keep_alive.take();
                    if let Some(done) = done {
                        let _ = done.send(());
                    }
                }
            }
        }
    }

    /// Builds the wrapped lower-layer stack for an ephemeral oplog. Layers except the
    /// last are wrapped with `WrappedOplogArchive` to trigger automatic overflow
    /// transfers.
    pub async fn build_lower_layers(
        lower_services: &NEVec<Arc<dyn super::multilayer::OplogArchiveService>>,
        owned_agent_id: &OwnedAgentId,
        account_id: golem_common::model::account::AccountId,
        entry_count_limit: u64,
        transfer_tx: &UnboundedSender<BackgroundTransferMessage>,
    ) -> NEVec<Arc<dyn OplogArchive + Send + Sync>> {
        let mut lower: Vec<Arc<dyn OplogArchive + Send + Sync>> = Vec::new();
        for (i, layer) in lower_services.iter().enumerate() {
            if i != (lower_services.len().get() - 1) {
                let raw = layer.open(owned_agent_id).await;
                let instrumented = Arc::new(InstrumentedOplogArchive::new(
                    raw,
                    account_id,
                    owned_agent_id.environment_id(),
                ));
                lower.push(Arc::new(
                    WrappedOplogArchive::new(
                        i,
                        instrumented,
                        transfer_tx.clone(),
                        entry_count_limit,
                    )
                    .await,
                ));
            } else {
                let raw = layer.open(owned_agent_id).await;
                lower.push(Arc::new(InstrumentedOplogArchive::new(
                    raw,
                    account_id,
                    owned_agent_id.environment_id(),
                )));
            }
        }
        NEVec::try_from_vec(lower).expect("At least one lower layer is required")
    }
}

impl Drop for EphemeralOplog {
    fn drop(&mut self) {
        if let Some(close_fn) = self.close_fn.take() {
            close_fn();
        }
        if let Some(fiber) = self.transfer_fiber.lock().unwrap().take() {
            fiber.abort();
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
        state.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        record_oplog_call("drop_prefix");
        let mut dropped = 0;
        for layer in &self.lower {
            dropped += layer.drop_prefix(last_dropped_id).await;
        }
        dropped
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
        self.read_many(oplog_index, 1)
            .await
            .remove(&oplog_index)
            .unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry {oplog_index} for ephemeral oplog of {:?}",
                    self.owned_agent_id
                )
            })
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read_many");
        if n == 0 {
            return BTreeMap::new();
        }

        let state = self.state.lock().await;

        let req_start: u64 = oplog_index.into();
        let req_end: u64 = oplog_index.range_end(n).into();

        let mut result = BTreeMap::new();

        // First, fill from the in-memory buffer (uncommitted entries)
        if !state.buffer.is_empty() {
            let first_uncommitted: u64 = state.last_committed_idx.next().into();
            let buffer_end: u64 = first_uncommitted + state.buffer.len() as u64 - 1;

            let overlap_start = max(req_start, first_uncommitted);
            let overlap_end = min(req_end, buffer_end);

            if overlap_start <= overlap_end {
                let offset = (overlap_start - first_uncommitted) as usize;
                let count = (overlap_end - overlap_start + 1) as usize;
                for i in 0..count {
                    let idx = OplogIndex::from_u64(overlap_start + i as u64);
                    let entry = state.buffer[offset + i].clone();
                    result.insert(idx, entry);
                }
            }
        }

        // Check if the buffer already satisfied the full request
        let full_match = match result.first_key_value() {
            Some((first_idx, _)) => *first_idx == oplog_index && result.len() as u64 >= n,
            None => false,
        };

        // Read remaining entries from committed lower layers, stopping as soon as
        // the requested range starting at oplog_index is fully covered.
        if !full_match {
            let committed_end: u64 = state.last_committed_idx.into();
            if committed_end >= req_start {
                let storage_end = min(req_end, committed_end);
                let mut remaining = storage_end - req_start + 1
                    - min(result.len() as u64, storage_end - req_start + 1);

                for layer in &self.lower {
                    if remaining == 0 {
                        break;
                    }
                    let partial = layer.read(oplog_index, remaining).await;
                    let layer_full_match = match partial.first_key_value() {
                        None => false,
                        Some((first_idx, _)) => {
                            remaining -= partial.len() as u64;
                            *first_idx == oplog_index
                        }
                    };
                    result.extend(partial);
                    if layer_full_match {
                        break;
                    }
                }
            }
        }

        result
    }

    async fn length(&self) -> u64 {
        record_oplog_call("length");
        let mut total = 0;
        for layer in &self.lower {
            total += layer.length().await;
        }
        total
    }

    async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.primary_service
            .upload_raw_payload(&self.owned_agent_id, data)
            .await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.primary_service
            .download_raw_payload(&self.owned_agent_id, payload_id, md5_hash)
            .await
    }
}
