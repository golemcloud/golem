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

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use nonempty_collections::NEVec;
use prometheus::core::{Atomic, AtomicU64};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, warn};

use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{AccountId, WorkerId};

use crate::services::oplog::multilayer::BackgroundTransferMessage::{
    TransferFromLower, TransferFromPrimary,
};
use crate::services::oplog::{Oplog, OplogService};

// TODO: need a "global" background thread that transfers things from closed old oplogs

#[derive(Debug, Clone)]
pub struct MultiLayerOplogService {
    primary: Arc<dyn OplogService + Send + Sync>,
    lower: NEVec<Arc<dyn OplogLayer + Send + Sync>>,

    entry_count_limit: u64,
}

impl MultiLayerOplogService {
    pub fn new(
        primary: Arc<dyn OplogService + Send + Sync>,
        lower: NEVec<Arc<dyn OplogLayer + Send + Sync>>,
        entry_count_limit: u64,
    ) -> Self {
        Self {
            primary,
            lower,
            entry_count_limit,
        }
    }
}

#[async_trait]
impl OplogService for MultiLayerOplogService {
    async fn create(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        Arc::new(
            MultiLayerOplog::new(
                worker_id.clone(),
                self.primary
                    .create(account_id, worker_id, initial_entry)
                    .await,
                self.clone(),
            )
            .await,
        )
    }

    async fn open(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
    ) -> Arc<dyn Oplog + Send + Sync> {
        Arc::new(
            MultiLayerOplog::new(
                worker_id.clone(),
                self.primary.open(account_id, worker_id).await,
                self.clone(),
            )
            .await,
        )
    }

    async fn get_first_index(&self, worker_id: &WorkerId) -> OplogIndex {
        self.primary.get_first_index(worker_id).await
    }

    async fn get_last_index(&self, worker_id: &WorkerId) -> OplogIndex {
        self.primary.get_last_index(worker_id).await
    }

    async fn delete(&self, worker_id: &WorkerId) {
        self.primary.delete(worker_id).await;
        for layer in &self.lower {
            layer.delete(worker_id).await
        }
    }

    async fn read(
        &self,
        worker_id: &WorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        // TODO: can be optimized by caching what each layer's oldest oplog index is

        let mut result = BTreeMap::new();
        let mut n: i64 = n as i64;
        if n > 0 {
            let partial_result = self.primary.read(worker_id, idx, n as u64).await;
            let full_match = match partial_result.first_key_value() {
                None => false,
                Some((first_idx, _)) => {
                    debug!("first_idx: {first_idx}, n: {n}");
                    // It is possible that n is bigger than the available number of entries
                    // so we cannot just decrease n by the number of entries read. Instead,
                    // we want to read from the next layer only up to the first index that was
                    // read from the primary oplog.s
                    n = (*first_idx as i64) - (idx as i64) - 1;
                    *first_idx == idx
                }
            };

            debug!(
                "Read {} entries from the primary oplog, full match: {}, n = {}",
                partial_result.len(),
                full_match,
                n
            );

            result.extend(partial_result.into_iter());

            if !full_match {
                for layer in &self.lower {
                    let partial_result = layer.read(worker_id, idx, n as u64).await;
                    let full_match = match partial_result.first_key_value() {
                        None => false,
                        Some((first_idx, _)) => {
                            debug!("first_idx: {first_idx}, n: {n}");
                            n = (*first_idx as i64) - (idx as i64) - 1;
                            *first_idx == idx
                        }
                    };

                    debug!(
                        "Read {} entries from the next oplog layer, full match: {}, n = {}",
                        partial_result.len(),
                        full_match,
                        n
                    );

                    result.extend(partial_result.into_iter());

                    if full_match {
                        break;
                    }
                }
            }
        }
        result
    }
}

#[derive(Debug)]
pub struct MultiLayerOplog {
    worker_id: WorkerId,
    oplog: Arc<dyn Oplog + Send + Sync>,
    multi_layer_oplog_service: MultiLayerOplogService,
    transfer_fiber: Option<tokio::task::JoinHandle<()>>,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    primary_length: AtomicU64,
}

impl MultiLayerOplog {
    pub async fn new(
        worker_id: WorkerId,
        oplog: Arc<dyn Oplog + Send + Sync>,
        multi_layer_oplog_service: MultiLayerOplogService,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let transfer_fiber = tokio::spawn(Self::background_transfer(
            worker_id.clone(),
            oplog.clone(),
            multi_layer_oplog_service.clone(),
            tx.clone(),
            rx,
        ));

        let initial_primary_length = oplog.length().await;

        Self {
            worker_id,
            oplog,
            multi_layer_oplog_service,
            transfer_fiber: Some(transfer_fiber),
            transfer: tx,
            primary_length: AtomicU64::new(initial_primary_length),
        }
    }

    async fn background_transfer(
        worker_id: WorkerId,
        oplog: Arc<dyn Oplog + Send + Sync>,
        multi_layer_oplog_service: MultiLayerOplogService,
        tx: UnboundedSender<BackgroundTransferMessage>,
        mut rx: UnboundedReceiver<BackgroundTransferMessage>,
    ) {
        while let Some(msg) = rx.recv().await {
            match msg {
                TransferFromPrimary { start_idx, count } => {
                    debug!("Transferring {count} oplog entries from index {start_idx} of the primary oplog of {worker_id} to the next layer");
                    debug!("Reading entries from the primary oplog of {worker_id}");
                    let entries: Vec<_> = multi_layer_oplog_service
                        .read(&worker_id, start_idx, count)
                        .await
                        .into_iter()
                        .collect();
                    match entries.last() {
                        Some(last_entry) => {
                            let last_dropped_id = last_entry.0;

                            debug!("Writing entries to the secondary oplog of {worker_id}");
                            multi_layer_oplog_service
                                .lower
                                .head
                                .append(&worker_id, entries)
                                .await;
                            let new_lower_length = multi_layer_oplog_service
                                .lower
                                .head
                                .length(&worker_id)
                                .await;
                            if new_lower_length >= multi_layer_oplog_service.entry_count_limit
                                && multi_layer_oplog_service.lower.len().get() > 1
                            {
                                debug!("Enqueuing transfer of {new_lower_length} oplog entries from the secondary oplog of {worker_id} to the next layer");
                                let first_idx = multi_layer_oplog_service
                                    .lower
                                    .head
                                    .get_first_index(&worker_id)
                                    .await;
                                let _ = tx
                                    .send(TransferFromLower {
                                        source: 0,
                                        start_idx: first_idx,
                                        count: last_dropped_id - first_idx,
                                    })
                                    .expect("Failed to enqueue transfer request");
                            }
                            debug!("Dropping transferred entries from the primary oplog of {worker_id}");
                            oplog.drop_prefix(last_dropped_id).await;
                        }
                        None => {
                            warn!("No entries to transfer from the primary oplog of {worker_id}");
                        }
                    }
                }
                TransferFromLower {
                    source,
                    start_idx,
                    count,
                } => {
                    debug!("Transferring {count} oplog entries from index {start_idx} of oplog layer {source} of {worker_id} to the next layer");
                    debug!("Reading entries from oplog layer {source} of {worker_id}");
                    let source_layer = multi_layer_oplog_service.lower[source].clone();
                    let target = source + 1;
                    let target_layer = multi_layer_oplog_service.lower[target].clone();

                    let entries: Vec<_> = source_layer
                        .read(&worker_id, start_idx, count)
                        .await
                        .into_iter()
                        .collect();
                    match entries.last() {
                        Some(last_entry) => {
                            let last_dropped_id = last_entry.0;

                            debug!("Writing entries to oplog layer {target} of {worker_id}");
                            target_layer.append(&worker_id, entries).await;
                            let new_target_length = target_layer.length(&worker_id).await;
                            if new_target_length >= multi_layer_oplog_service.entry_count_limit
                                && (target + 1) < multi_layer_oplog_service.lower.len().get()
                            {
                                debug!("Enqueuing transfer of {new_target_length} oplog entries from the oplog layer {target} of {worker_id} to the next layer");
                                let first_idx = target_layer.get_first_index(&worker_id).await;
                                let _ = tx
                                    .send(TransferFromLower {
                                        source: target,
                                        start_idx: first_idx,
                                        count: last_dropped_id - first_idx,
                                    })
                                    .expect("Failed to enqueue transfer request");
                            }
                            debug!("Dropping transferred entries from the oplog layer {source} of {worker_id}");
                            source_layer.drop_prefix(&worker_id, last_dropped_id).await;
                        }
                        None => {
                            warn!("No entries to transfer from the primary oplog of {worker_id}");
                        }
                    }
                }
            }
        }
    }
}

impl Drop for MultiLayerOplog {
    fn drop(&mut self) {
        self.transfer_fiber.take().unwrap().abort();
    }
}

#[async_trait]
impl Oplog for MultiLayerOplog {
    async fn add(&self, entry: OplogEntry) {
        self.oplog.add(entry).await;
        self.primary_length.inc_by(1);
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.oplog.drop_prefix(last_dropped_id).await;
        let new_length = self.oplog.length().await;
        self.primary_length.set(new_length);
    }

    async fn commit(&self) {
        self.oplog.commit().await;
        let count = self.primary_length.get();
        if count >= self.multi_layer_oplog_service.entry_count_limit {
            debug!("Enqueuing transfer of {count} oplog entries from the primary oplog of {} to the next layer", self.worker_id);
            let _ = self.transfer.send(TransferFromPrimary {
                start_idx: self.oplog.current_oplog_index().await - count,
                count,
            });
            // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
            self.primary_length.set(0);
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.oplog.current_oplog_index().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.oplog.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.multi_layer_oplog_service
            .read(&self.worker_id, oplog_index, 1)
            .await
            .into_values()
            .into_iter()
            .next()
            .expect("Missing oplog entry")
    }

    async fn length(&self) -> u64 {
        let mut total_length = self.oplog.length().await;
        for layer in &self.multi_layer_oplog_service.lower {
            total_length += layer.length(&self.worker_id).await;
        }
        total_length
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        self.oplog.upload_payload(data).await
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        self.oplog.download_payload(payload).await
    }
}

/// Interface for secondary oplog layers - requires less functionality than the primary layer
#[async_trait]
pub trait OplogLayer: Debug {
    /// Read an arbitrary section of the oplog
    async fn read(
        &self,
        worker_id: &WorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Append a new chunk of entries to the oplog
    async fn append(&self, worker_id: &WorkerId, chunk: Vec<(OplogIndex, OplogEntry)>);

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the layer below this one
    async fn drop_prefix(&self, worker_id: &WorkerId, last_dropped_id: OplogIndex);

    /// Deletes the oplog layer for a worker completely
    async fn delete(&self, worker_id: &WorkerId);

    /// Gets the total number of entries in this oplog layer
    async fn length(&self, worker_id: &WorkerId) -> u64;

    /// Gets the index of the first entry in this oplog layer
    async fn get_first_index(&self, worker_id: &WorkerId) -> OplogIndex;
}

enum BackgroundTransferMessage {
    TransferFromPrimary {
        start_idx: OplogIndex,
        count: u64,
    },
    TransferFromLower {
        source: usize,
        start_idx: OplogIndex,
        count: u64,
    },
}
