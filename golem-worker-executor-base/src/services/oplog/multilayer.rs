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
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use nonempty_collections::{NEVec, NonEmptyIterator};
use prometheus::core::{Atomic, AtomicU64};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, warn, Instrument};

use crate::error::GolemError;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{AccountId, ComponentId, ScanCursor, WorkerId};

use crate::services::oplog::multilayer::BackgroundTransferMessage::{
    TransferFromLower, TransferFromPrimary,
};
use crate::services::oplog::{downcast_oplog, OpenOplogs, Oplog, OplogConstructor, OplogService};

#[async_trait]
pub trait OplogArchiveService: Debug {
    /// Opens an oplog archive for writing
    async fn open(&self, worker_id: &WorkerId) -> Arc<dyn OplogArchive + Send + Sync>;

    /// Deletes the oplog archive for a worker completely
    async fn delete(&self, worker_id: &WorkerId);

    /// Read an arbitrary section of the oplog archive without opening it for writing
    async fn read(
        &self,
        worker_id: &WorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Checks if an oplog archive exists for a worker
    async fn exists(&self, worker_id: &WorkerId) -> bool;

    async fn scan_for_component(
        &self,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<WorkerId>), GolemError>;
}

/// Interface for secondary oplog archives - requires less functionality than the primary archive
#[async_trait]
pub trait OplogArchive: Debug {
    /// Read an arbitrary section of the oplog archive
    async fn read(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Reads an inclusive range of entries from the oplog archive
    async fn read_range(
        &self,
        start_idx: OplogIndex,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read(
            start_idx,
            Into::<u64>::into(last_idx) - Into::<u64>::into(start_idx) + 1,
        )
        .await
    }

    /// Reads a prefix up to and including the given index
    async fn read_prefix(&self, last_idx: OplogIndex) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_range(OplogIndex::INITIAL, last_idx).await
    }

    /// Append a new chunk of entries to the oplog
    async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>);

    /// Gets the last appended chunk's last index
    async fn current_oplog_index(&self) -> OplogIndex;

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the archive below this one
    async fn drop_prefix(&self, last_dropped_id: OplogIndex);

    /// Gets the total number of entries in this oplog archive
    async fn length(&self) -> u64;
}

#[derive(Debug)]
pub struct MultiLayerOplogService {
    primary: Arc<dyn OplogService + Send + Sync>,
    lower: NEVec<Arc<dyn OplogArchiveService + Send + Sync>>,

    oplogs: OpenOplogs,

    entry_count_limit: u64,
}

impl MultiLayerOplogService {
    pub fn new(
        primary: Arc<dyn OplogService + Send + Sync>,
        lower: NEVec<Arc<dyn OplogArchiveService + Send + Sync>>,
        entry_count_limit: u64,
    ) -> Self {
        Self {
            primary,
            lower,
            oplogs: OpenOplogs::new("multi-layer oplog"),
            entry_count_limit,
        }
    }
}

impl Clone for MultiLayerOplogService {
    fn clone(&self) -> Self {
        Self {
            primary: self.primary.clone(),
            lower: self.lower.clone(),
            oplogs: self.oplogs.clone(),
            entry_count_limit: self.entry_count_limit,
        }
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    worker_id: WorkerId,
    account_id: AccountId,
    initial_entry: Option<OplogEntry>,
    primary: Arc<dyn OplogService + Send + Sync>,
    service: MultiLayerOplogService,
}

impl CreateOplogConstructor {
    fn new(
        worker_id: WorkerId,
        account_id: AccountId,
        initial_entry: Option<OplogEntry>,
        primary: Arc<dyn OplogService + Send + Sync>,
        service: MultiLayerOplogService,
    ) -> Self {
        Self {
            worker_id,
            account_id,
            initial_entry,
            primary,
            service,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(
        self,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog + Send + Sync> {
        let primary = if let Some(initial_entry) = self.initial_entry {
            self.primary
                .create(&self.account_id, &self.worker_id, initial_entry)
                .await
        } else {
            self.primary.open(&self.account_id, &self.worker_id).await
        };
        Arc::new(MultiLayerOplog::new(self.worker_id, primary, self.service, close).await)
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
        self.oplogs
            .get_or_open(
                worker_id,
                CreateOplogConstructor::new(
                    worker_id.clone(),
                    account_id.clone(),
                    Some(initial_entry),
                    self.primary.clone(),
                    self.clone(),
                ),
            )
            .await
    }

    async fn open(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
    ) -> Arc<dyn Oplog + Send + Sync> {
        self.oplogs
            .get_or_open(
                worker_id,
                CreateOplogConstructor::new(
                    worker_id.clone(),
                    account_id.clone(),
                    None,
                    self.primary.clone(),
                    self.clone(),
                ),
            )
            .await
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
        // TODO: could be optimized by caching what each layer's oldest oplog index is

        let mut result = BTreeMap::new();
        let mut n: i64 = n as i64;
        if n > 0 {
            let partial_result = self.primary.read(worker_id, idx, n as u64).await;
            let full_match = match partial_result.first_key_value() {
                None => false,
                Some((first_idx, _)) => {
                    // It is possible that n is bigger than the available number of entries,
                    // so we cannot just decrease n by the number of entries read. Instead,
                    // we want to read from the next layer only up to the first index that was
                    // read from the primary oplog.s
                    n = (Into::<u64>::into(*first_idx) as i64) - (Into::<u64>::into(idx) as i64);
                    *first_idx == idx
                }
            };

            result.extend(partial_result.into_iter());

            if !full_match {
                for layer in &self.lower {
                    let partial_result = layer.read(worker_id, idx, n as u64).await;
                    let full_match = match partial_result.first_key_value() {
                        None => false,
                        Some((first_idx, _)) => {
                            n = (Into::<u64>::into(*first_idx) as i64)
                                - (Into::<u64>::into(idx) as i64);
                            *first_idx == idx
                        }
                    };

                    result.extend(partial_result.into_iter());

                    if full_match {
                        break;
                    }
                }
            }
        }
        result
    }

    async fn exists(&self, worker_id: &WorkerId) -> bool {
        if self.primary.exists(worker_id).await {
            return true;
        }

        for layer in &self.lower {
            if layer.exists(worker_id).await {
                return true;
            }
        }

        false
    }

    async fn scan_for_component(
        &self,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<WorkerId>), GolemError> {
        match cursor.layer {
            0 => {
                let (new_cursor, ids) = self
                    .primary
                    .scan_for_component(component_id, cursor, count)
                    .await?;
                if new_cursor.is_finished() {
                    // Continuing with the first lower layer
                    Ok((
                        ScanCursor {
                            cursor: 0,
                            layer: 1,
                        },
                        ids,
                    ))
                } else {
                    // Still scanning the primary layer
                    return Ok((new_cursor, ids));
                }
            }
            layer if layer < self.lower.len().get() => {
                let (new_cursor, ids) = self.lower[layer]
                    .scan_for_component(component_id, cursor, count)
                    .await?;
                if new_cursor.is_finished() && (layer + 1) < self.lower.len().get() {
                    // Continuing with the next lower layer
                    Ok((
                        ScanCursor {
                            cursor: 0,
                            layer: layer + 1,
                        },
                        ids,
                    ))
                } else {
                    // Still scanning the current layer
                    return Ok((new_cursor, ids));
                }
            }
            layer => {
                return Err(GolemError::unknown(format!(
                    "Invalid oplog layer in scan cursor: {layer}"
                )));
            }
        }
    }
}

pub struct MultiLayerOplog {
    worker_id: WorkerId,
    primary: Arc<dyn Oplog + Send + Sync>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    multi_layer_oplog_service: MultiLayerOplogService,
    transfer_fiber: Option<tokio::task::JoinHandle<()>>,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    primary_length: AtomicU64,
    close_fn: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl MultiLayerOplog {
    pub async fn new(
        worker_id: WorkerId,
        primary: Arc<dyn Oplog + Send + Sync>,
        multi_layer_oplog_service: MultiLayerOplogService,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut lower: Vec<Arc<dyn OplogArchive + Send + Sync>> = Vec::new();
        for (i, layer) in multi_layer_oplog_service.lower.iter().enumerate() {
            if i != (multi_layer_oplog_service.lower.len().get() - 1) {
                // Wrapping the intermediate layers to they transfer entries to the next layer
                lower.push(Arc::new(
                    WrappedOplogArchive::new(
                        i,
                        layer.open(&worker_id).await,
                        tx.clone(),
                        multi_layer_oplog_service.entry_count_limit,
                    )
                    .await,
                ));
            } else {
                // Not wrapping the last layer
                lower.push(layer.open(&worker_id).await);
            }
        }
        let lower = NEVec::from_vec(lower).expect("At least one lower layer is required");

        let transfer_fiber = tokio::spawn(
            Self::background_transfer(
                worker_id.clone(),
                primary.clone(),
                lower.clone(),
                multi_layer_oplog_service.clone(),
                rx,
            )
            .in_current_span(),
        );

        let initial_primary_length = primary.length().await;

        Self {
            worker_id,
            primary,
            lower,
            multi_layer_oplog_service,
            transfer_fiber: Some(transfer_fiber),
            transfer: tx,
            primary_length: AtomicU64::new(initial_primary_length),
            close_fn: Some(close),
        }
    }

    async fn background_transfer(
        worker_id: WorkerId,
        primary: Arc<dyn Oplog + Send + Sync>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
        multi_layer_oplog_service: MultiLayerOplogService,
        mut rx: UnboundedReceiver<BackgroundTransferMessage>,
    ) {
        // TODO: monitor queue length

        while let Some(msg) = rx.recv().await {
            match msg {
                TransferFromPrimary {
                    last_transferred_idx,
                    mut keep_alive,
                } => {
                    debug!("Transferring oplog entries up to index {last_transferred_idx} of the primary oplog of {worker_id} to the next layer");
                    debug!("Reading entries from the primary oplog of {worker_id}");

                    let transfer = BackgroundTransferFromPrimary::new(
                        worker_id.clone(),
                        last_transferred_idx,
                        multi_layer_oplog_service.clone(),
                        primary.clone(),
                        lower.clone(),
                    );
                    let result = transfer.run().await;
                    if let Err(error) = result {
                        error!("Failed to transfer entries from the primary oplog of {worker_id}: {error}");
                    }
                    let _ = keep_alive.take();
                }
                TransferFromLower {
                    source,
                    last_transferred_idx,
                    mut keep_alive,
                } => {
                    debug!("Transferring oplog entries up to index {last_transferred_idx} of oplog layer {source} of {worker_id} to the next layer");
                    debug!("Reading entries from oplog layer {source} of {worker_id}");

                    let transfer = BackgroundTransferBetweenLowers::new(
                        source,
                        last_transferred_idx,
                        lower.clone(),
                    );
                    let result = transfer.run().await;

                    if let Err(error) = result {
                        error!("Failed to transfer entries from oplog layer {source} of {worker_id}: {error}");
                    }
                    let _ = keep_alive.take();
                }
            }
        }
    }

    pub async fn try_archive(this: &Arc<dyn Oplog + Send + Sync>) -> Option<bool> {
        let this = downcast_oplog::<MultiLayerOplog>(this)?;
        Some(Self::archive(this).await)
    }

    async fn archive(this: Arc<Self>) -> bool {
        if this.primary_length.get() > 0 {
            // transferring the whole primary oplog to the next layer
            this.transfer
                .send(TransferFromPrimary {
                    last_transferred_idx: this.primary.current_oplog_index().await,
                    keep_alive: Some(this.clone()),
                })
                .expect("Failed to enqueue transfer of primary oplog entries");

            // If there are more layers to transfer from, return true
            this.lower.len().get() > 1
        } else {
            let mut n = 0;
            let first_non_empty = loop {
                let length = this.lower[n].length().await;
                if length > 0 {
                    break Some(n);
                } else if n < this.lower.len().get() - 2 {
                    // skipping the last layer as there is nowhere to transfer to from there
                    n += 1;
                } else {
                    break None;
                }
            };

            if let Some(first_non_empty) = first_non_empty {
                // transferring the whole non-empty lower layer to the next layer
                this.transfer
                    .send(TransferFromLower {
                        source: first_non_empty,
                        last_transferred_idx: this.lower[first_non_empty]
                            .current_oplog_index()
                            .await,
                        keep_alive: Some(this.clone()),
                    })
                    .expect("Failed to enqueue transfer of primary oplog entries");

                // If there are more layers to transfer from, return true
                first_non_empty < this.lower.len().get() - 2
            } else {
                // Fully archived
                false
            }
        }
    }
}

impl Drop for MultiLayerOplog {
    fn drop(&mut self) {
        if let Some(close_fn) = self.close_fn.take() {
            close_fn();
        }
        self.transfer_fiber.take().unwrap().abort();
    }
}

impl Debug for MultiLayerOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiLayerOplog")
            .field("worker_id", &self.worker_id)
            .finish()
    }
}

#[async_trait]
impl Oplog for MultiLayerOplog {
    async fn add(&self, entry: OplogEntry) {
        self.primary.add(entry).await;
        self.primary_length.inc_by(1);
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.primary.drop_prefix(last_dropped_id).await;
        let new_length = self.primary.length().await;
        self.primary_length.set(new_length);
    }

    async fn commit(&self) {
        self.primary.commit().await;
        let count = self.primary_length.get();
        if count >= self.multi_layer_oplog_service.entry_count_limit {
            let current_idx = self.primary.current_oplog_index().await;
            debug!("Enqueuing transfer of {count} oplog entries from the primary oplog of {} to the next layer up to {current_idx}", self.worker_id);
            let _ = self.transfer.send(TransferFromPrimary {
                last_transferred_idx: current_idx,
                keep_alive: None,
            });
            // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
            self.primary_length.set(0);
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.primary.current_oplog_index().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.primary.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.multi_layer_oplog_service
            .read(&self.worker_id, oplog_index, 1)
            .await
            .into_values()
            .next()
            .expect("Missing oplog entry")
    }

    async fn length(&self) -> u64 {
        let mut total_length = self.primary.length().await;
        for layer in &self.lower {
            total_length += layer.length().await;
        }
        total_length
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        self.primary.upload_payload(data).await
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        self.primary.download_payload(payload).await
    }
}

#[derive(Debug)]
enum BackgroundTransferMessage {
    TransferFromPrimary {
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog + Send + Sync>>,
    },
    TransferFromLower {
        source: usize,
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog + Send + Sync>>,
    },
}

#[async_trait]
trait BackgroundTransfer {
    async fn read_source(&self) -> Vec<(OplogIndex, OplogEntry)>;
    async fn append_target(&self, entries: Vec<(OplogIndex, OplogEntry)>);
    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex);

    async fn run(&self) -> Result<(), String> {
        let entries: Vec<_> = self.read_source().await;
        match entries.last() {
            Some(last_entry) => {
                let last_dropped_id = last_entry.0;

                debug!("Writing entries to the secondary oplog");
                self.append_target(entries).await;

                debug!("Dropping transferred entries from the primary oplog");
                self.drop_source_prefix(last_dropped_id).await;
            }
            None => {
                warn!("No entries to transfer from the primary oplog");
            }
        }
        Ok(())
    }
}

/// Wraps an open oplog archive to track the number of items written and automatically
/// scheduling transfers to lower levels when the limit is reached
#[derive(Debug)]
struct WrappedOplogArchive {
    layer: usize,
    archive: Arc<dyn OplogArchive + Send + Sync>,
    entry_count: AtomicU64,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    entry_count_limit: u64,
}

impl WrappedOplogArchive {
    pub async fn new(
        layer: usize,
        archive: Arc<dyn OplogArchive + Send + Sync>,
        transfer: UnboundedSender<BackgroundTransferMessage>,
        entry_count_limit: u64,
    ) -> Self {
        let initial_entry_count = archive.length().await;
        Self {
            layer,
            archive,
            entry_count: AtomicU64::new(initial_entry_count),
            transfer,
            entry_count_limit,
        }
    }
}

#[async_trait]
impl OplogArchive for WrappedOplogArchive {
    async fn read(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.archive.read(idx, n).await
    }

    async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>) {
        if !chunk.is_empty() {
            let last_idx = chunk.last().unwrap().0;
            self.archive.append(chunk).await;
            self.entry_count.inc_by(1);
            let count = self.entry_count.get();
            if count >= self.entry_count_limit {
                debug!("Enqueuing transfer of oplog entries from the oplog layer {} to the next layer up to {last_idx}", self.layer);
                let _ = self.transfer.send(TransferFromLower {
                    source: self.layer,
                    last_transferred_idx: last_idx,
                    keep_alive: None,
                });
                // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
                self.entry_count.set(0);
            }
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.archive.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.archive.drop_prefix(last_dropped_id).await;
        let new_length = self.archive.length().await;
        self.entry_count.set(new_length);
    }

    async fn length(&self) -> u64 {
        self.entry_count.get()
    }
}

struct BackgroundTransferFromPrimary {
    worker_id: WorkerId,
    last_transferred_idx: OplogIndex,
    multi_layer_oplog_service: MultiLayerOplogService,
    primary: Arc<dyn Oplog + Send + Sync>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
}

impl BackgroundTransferFromPrimary {
    pub fn new(
        worker_id: WorkerId,
        last_transferred_idx: OplogIndex,
        multi_layer_oplog_service: MultiLayerOplogService,
        primary: Arc<dyn Oplog + Send + Sync>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        Self {
            worker_id,
            last_transferred_idx,
            multi_layer_oplog_service,
            primary,
            lower,
        }
    }
}

#[async_trait]
impl BackgroundTransfer for BackgroundTransferFromPrimary {
    async fn read_source(&self) -> Vec<(OplogIndex, OplogEntry)> {
        self.multi_layer_oplog_service
            .primary
            .read_prefix(&self.worker_id, self.last_transferred_idx)
            .await
            .into_iter()
            .collect()
    }

    async fn append_target(&self, entries: Vec<(OplogIndex, OplogEntry)>) {
        self.lower.head.append(entries).await
    }

    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) {
        self.primary.drop_prefix(last_dropped_id).await
    }
}

struct BackgroundTransferBetweenLowers {
    last_transferred_idx: OplogIndex,
    source_layer: Arc<dyn OplogArchive + Send + Sync>,
    target_layer: Arc<dyn OplogArchive + Send + Sync>,
}

impl BackgroundTransferBetweenLowers {
    pub fn new(
        source: usize,
        last_transferred_idx: OplogIndex,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        let source_layer = lower[source].clone();
        let target_layer = lower[source + 1].clone();

        Self {
            last_transferred_idx,
            source_layer,
            target_layer,
        }
    }
}

#[async_trait]
impl BackgroundTransfer for BackgroundTransferBetweenLowers {
    async fn read_source(&self) -> Vec<(OplogIndex, OplogEntry)> {
        self.source_layer
            .read_prefix(self.last_transferred_idx)
            .await
            .into_iter()
            .collect()
    }

    async fn append_target(&self, entries: Vec<(OplogIndex, OplogEntry)>) {
        self.target_layer.append(entries).await
    }

    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) {
        self.source_layer.drop_prefix(last_dropped_id).await
    }
}
