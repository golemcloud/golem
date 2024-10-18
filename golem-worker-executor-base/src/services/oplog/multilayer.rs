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

use std::cmp::min;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use nonempty_collections::{NEVec, NonEmptyIterator};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::Sender;
use tracing::{debug, error, info, warn, Instrument};

use crate::error::GolemError;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{AccountId, ComponentId, ComponentType, OwnedWorkerId, ScanCursor};

use crate::services::oplog::ephemeral::EphemeralOplog;
use crate::services::oplog::multilayer::BackgroundTransferMessage::{
    TransferFromLower, TransferFromPrimary,
};
use crate::services::oplog::{
    downcast_oplog, CommitLevel, OpenOplogs, Oplog, OplogConstructor, OplogService,
};

#[async_trait]
pub trait OplogArchiveService: Debug {
    /// Opens an oplog archive for writing
    async fn open(&self, owned_worker_id: &OwnedWorkerId) -> Arc<dyn OplogArchive + Send + Sync>;

    /// Deletes the oplog archive for a worker completely
    async fn delete(&self, owned_worker_id: &OwnedWorkerId);

    /// Read an arbitrary section of the oplog archive without opening it for writing
    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Checks if an oplog archive exists for a worker
    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool;

    async fn scan_for_component(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError>;

    /// Gets the last stored oplog entry's id in the archive
    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex;
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

    /// Gets the last index in this oplog archive
    async fn get_last_index(&self) -> OplogIndex;
}

#[derive(Debug)]
pub struct MultiLayerOplogService {
    pub primary: Arc<dyn OplogService + Send + Sync>,
    pub lower: NEVec<Arc<dyn OplogArchiveService + Send + Sync>>,

    oplogs: OpenOplogs,

    entry_count_limit: u64,
    max_operations_before_commit_ephemeral: u64,
}

impl MultiLayerOplogService {
    pub fn new(
        primary: Arc<dyn OplogService + Send + Sync>,
        lower: NEVec<Arc<dyn OplogArchiveService + Send + Sync>>,
        entry_count_limit: u64,
        max_operations_before_commit_ephemeral: u64,
    ) -> Self {
        Self {
            primary,
            lower,
            oplogs: OpenOplogs::new("multi-layer oplog"),
            entry_count_limit,
            max_operations_before_commit_ephemeral,
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
            max_operations_before_commit_ephemeral: self.max_operations_before_commit_ephemeral,
        }
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    owned_worker_id: OwnedWorkerId,
    initial_entry: Option<OplogEntry>,
    primary: Arc<dyn OplogService + Send + Sync>,
    service: MultiLayerOplogService,
    last_oplog_index: OplogIndex,
    component_type: ComponentType,
}

impl CreateOplogConstructor {
    fn new(
        owned_worker_id: OwnedWorkerId,
        initial_entry: Option<OplogEntry>,
        primary: Arc<dyn OplogService + Send + Sync>,
        service: MultiLayerOplogService,
        last_oplog_index: OplogIndex,
        component_type: ComponentType,
    ) -> Self {
        Self {
            owned_worker_id,
            initial_entry,
            primary,
            service,
            last_oplog_index,
            component_type,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(
        self,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog + Send + Sync> {
        match self.component_type {
            ComponentType::Durable => {
                let primary = if let Some(initial_entry) = self.initial_entry {
                    self.primary
                        .create(&self.owned_worker_id, initial_entry, self.component_type)
                        .await
                } else {
                    self.primary
                        .open(
                            &self.owned_worker_id,
                            self.last_oplog_index,
                            self.component_type,
                        )
                        .await
                };
                MultiLayerOplog::new(self.owned_worker_id, primary, self.service, close).await
            }
            ComponentType::Ephemeral => {
                let primary = self
                    .primary
                    .open(
                        &self.owned_worker_id,
                        self.last_oplog_index,
                        self.component_type,
                    )
                    .await;

                let target_layer = self.service.lower.last();
                let target = target_layer.open(&self.owned_worker_id).await;

                if let Some(initial_entry) = self.initial_entry {
                    target
                        .append(vec![(OplogIndex::INITIAL, initial_entry)])
                        .await;
                }

                Arc::new(
                    EphemeralOplog::new(
                        self.owned_worker_id,
                        self.last_oplog_index,
                        self.service.max_operations_before_commit_ephemeral,
                        primary,
                        target,
                        close,
                    )
                    .await,
                )
            }
        }
    }
}

#[async_trait]
impl OplogService for MultiLayerOplogService {
    async fn create(
        &self,
        owned_worker_id: &OwnedWorkerId,
        initial_entry: OplogEntry,
        component_type: ComponentType,
    ) -> Arc<dyn Oplog + Send + Sync> {
        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateOplogConstructor::new(
                    owned_worker_id.clone(),
                    Some(initial_entry),
                    self.primary.clone(),
                    self.clone(),
                    OplogIndex::INITIAL,
                    component_type,
                ),
            )
            .await
    }

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: OplogIndex,
        component_type: ComponentType,
    ) -> Arc<dyn Oplog + Send + Sync> {
        debug!("MultiLayerOplogService::open {owned_worker_id}");
        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateOplogConstructor::new(
                    owned_worker_id.clone(),
                    None,
                    self.primary.clone(),
                    self.clone(),
                    last_oplog_index,
                    component_type,
                ),
            )
            .await
    }

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        let mut result = self.primary.get_last_index(owned_worker_id).await;
        if result == OplogIndex::NONE {
            for layer in &self.lower {
                let idx = layer.get_last_index(owned_worker_id).await;
                if idx != OplogIndex::NONE {
                    result = idx;
                    break;
                }
            }
        }
        result
    }

    async fn delete(&self, owned_worker_id: &OwnedWorkerId) {
        self.primary.delete(owned_worker_id).await;
        for layer in &self.lower {
            layer.delete(owned_worker_id).await
        }
    }

    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        // TODO: could be optimized by caching what each layer's oldest oplog index is

        let mut result = BTreeMap::new();
        let mut n: i64 = n as i64;
        if n > 0 {
            let partial_result = self.primary.read(owned_worker_id, idx, n as u64).await;
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
                    let partial_result = layer.read(owned_worker_id, idx, n as u64).await;
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

    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool {
        if self.primary.exists(owned_worker_id).await {
            return true;
        }

        for layer in &self.lower {
            if layer.exists(owned_worker_id).await {
                return true;
            }
        }

        false
    }

    async fn scan_for_component(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError> {
        match cursor.layer {
            0 => {
                let (new_cursor, ids) = self
                    .primary
                    .scan_for_component(account_id, component_id, cursor, count)
                    .await?;
                if new_cursor.is_active_layer_finished() {
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
                    Ok((new_cursor, ids))
                }
            }
            layer if layer <= self.lower.len().get() => {
                let (new_cursor, ids) = self.lower[layer - 1]
                    .scan_for_component(account_id, component_id, cursor, count)
                    .await?;
                if new_cursor.is_active_layer_finished() && (layer + 1) <= self.lower.len().get() {
                    // Continuing with the next lower layer
                    Ok((
                        ScanCursor {
                            cursor: 0,
                            layer: layer + 1,
                        },
                        ids,
                    ))
                } else if new_cursor.is_active_layer_finished() {
                    // Finished scanning the last layer
                    Ok((
                        ScanCursor {
                            cursor: 0,
                            layer: 0,
                        },
                        ids,
                    ))
                } else {
                    // Still scanning the current layer
                    Ok((new_cursor, ids))
                }
            }
            layer => Err(GolemError::unknown(format!(
                "Invalid oplog layer in scan cursor: {layer}"
            ))),
        }
    }

    async fn upload_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: &[u8],
    ) -> Result<OplogPayload, String> {
        self.primary.upload_payload(owned_worker_id, data).await
    }

    async fn download_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload: &OplogPayload,
    ) -> Result<Bytes, String> {
        self.primary
            .download_payload(owned_worker_id, payload)
            .await
    }
}

pub struct MultiLayerOplog {
    owned_worker_id: OwnedWorkerId,
    primary: Arc<dyn Oplog + Send + Sync>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    multi_layer_oplog_service: MultiLayerOplogService,
    transfer_fiber: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    primary_length: AtomicU64,
    close_fn: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl MultiLayerOplog {
    #[allow(clippy::new_ret_no_self)]
    pub async fn new(
        owned_worker_id: OwnedWorkerId,
        primary: Arc<dyn Oplog + Send + Sync>,
        multi_layer_oplog_service: MultiLayerOplogService,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog + Send + Sync> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut lower: Vec<Arc<dyn OplogArchive + Send + Sync>> = Vec::new();
        for (i, layer) in multi_layer_oplog_service.lower.iter().enumerate() {
            if i != (multi_layer_oplog_service.lower.len().get() - 1) {
                // Wrapping the intermediate layers to they transfer entries to the next layer
                lower.push(Arc::new(
                    WrappedOplogArchive::new(
                        i,
                        layer.open(&owned_worker_id).await,
                        tx.clone(),
                        multi_layer_oplog_service.entry_count_limit,
                    )
                    .await,
                ));
            } else {
                // Not wrapping the last layer
                lower.push(layer.open(&owned_worker_id).await);
            }
        }
        let lower = NEVec::from_vec(lower).expect("At least one lower layer is required");

        let initial_primary_length = primary.length().await;
        let result = Arc::new(Self {
            owned_worker_id: owned_worker_id.clone(),
            primary: primary.clone(),
            lower: lower.clone(),
            multi_layer_oplog_service: multi_layer_oplog_service.clone(),
            transfer_fiber: Arc::new(Mutex::new(None)),
            transfer: tx,
            primary_length: AtomicU64::new(initial_primary_length),
            close_fn: Some(close),
        });
        let result_oplog: Arc<dyn Oplog + Send + Sync> = result.clone();

        result.set_background_transfer(tokio::spawn(
            Self::background_transfer(
                owned_worker_id,
                Arc::downgrade(&result_oplog),
                lower,
                multi_layer_oplog_service,
                rx,
            )
            .in_current_span(),
        ));

        result
    }

    fn set_background_transfer(&self, fiber: tokio::task::JoinHandle<()>) {
        *self.transfer_fiber.lock().unwrap() = Some(fiber);
    }

    async fn background_transfer(
        owned_worker_id: OwnedWorkerId,
        primary: Weak<dyn Oplog + Send + Sync>,
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
                    done,
                } => {
                    info!("Transferring oplog entries up to index {last_transferred_idx} of the primary oplog to the next layer");
                    debug!("Reading entries from the primary oplog");

                    if let Some(primary) = primary.upgrade() {
                        let transfer = BackgroundTransferFromPrimary::new(
                            owned_worker_id.clone(),
                            last_transferred_idx,
                            multi_layer_oplog_service.clone(),
                            primary.clone(),
                            lower.clone(),
                        );
                        let result = transfer.run().await;
                        if let Err(error) = result {
                            error!("Failed to transfer entries from the primary oplog: {error}");
                        }
                        let _ = keep_alive.take();

                        if let Some(done) = done {
                            done.send(()).unwrap()
                        }
                    }
                }
                TransferFromLower {
                    source,
                    last_transferred_idx,
                    mut keep_alive,
                    done,
                } => {
                    info!("Transferring oplog entries up to index {last_transferred_idx} of oplog layer {source} to the next layer");
                    debug!("Reading entries from oplog layer {source}");

                    let transfer = BackgroundTransferBetweenLowers::new(
                        source,
                        last_transferred_idx,
                        lower.clone(),
                    );
                    let result = transfer.run().await;

                    if let Err(error) = result {
                        error!("Failed to transfer entries from oplog layer {source}: {error}");
                    }
                    let _ = keep_alive.take();

                    if let Some(done) = done {
                        done.send(()).unwrap()
                    }
                }
            }
        }
    }

    pub async fn try_archive(this: &Arc<dyn Oplog + Send + Sync>) -> Option<bool> {
        let this = downcast_oplog::<MultiLayerOplog>(this)?;
        Some(Self::archive(this, false).await)
    }

    pub async fn try_archive_blocking(this: &Arc<dyn Oplog + Send + Sync>) -> Option<bool> {
        let this = downcast_oplog::<MultiLayerOplog>(this)?;
        Some(Self::archive(this, true).await)
    }

    async fn archive(this: Arc<Self>, blocking: bool) -> bool {
        let (done_tx, done_rx) = if blocking {
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            (Some(done_tx), Some(done_rx))
        } else {
            (None, None)
        };
        let result = if this.primary_length.load(Ordering::Acquire) > 0 {
            // transferring the whole primary oplog to the next layer
            this.transfer
                .send(TransferFromPrimary {
                    last_transferred_idx: this.primary.current_oplog_index().await,
                    keep_alive: Some(this.clone()),
                    done: done_tx,
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
                        done: done_tx,
                    })
                    .expect("Failed to enqueue transfer of primary oplog entries");

                // If there are more layers to transfer from, return true
                first_non_empty < this.lower.len().get() - 2
            } else {
                // Fully archived
                false
            }
        };

        if let Some(done_rx) = done_rx {
            done_rx
                .await
                .expect("Failed to wait for the archiving to finish");
        }

        result
    }
}

impl Drop for MultiLayerOplog {
    fn drop(&mut self) {
        if let Some(close_fn) = self.close_fn.take() {
            close_fn();
        }
        self.transfer_fiber.lock().unwrap().take().unwrap().abort();
    }
}

impl Debug for MultiLayerOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiLayerOplog")
            .field("worker_id", &self.owned_worker_id)
            .finish()
    }
}

#[async_trait]
impl Oplog for MultiLayerOplog {
    async fn add(&self, entry: OplogEntry) {
        self.primary.add(entry).await;
        self.primary_length.fetch_add(1, Ordering::AcqRel);
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.primary.drop_prefix(last_dropped_id).await;
        let new_length = self.primary.length().await;
        let old_length = self.primary_length.load(Ordering::Acquire);
        let new_length = min(new_length, old_length);
        self.primary_length.store(new_length, Ordering::Release);
    }

    async fn commit(&self, level: CommitLevel) {
        self.primary.commit(level).await;
        let count = self.primary_length.load(Ordering::Acquire);
        if count >= self.multi_layer_oplog_service.entry_count_limit {
            let current_idx = self.primary.current_oplog_index().await;
            debug!("Enqueuing transfer of {count} oplog entries from the primary oplog to the next layer up to {current_idx}");
            let _ = self.transfer.send(TransferFromPrimary {
                last_transferred_idx: current_idx,
                keep_alive: None,
                done: None,
            });
            // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
            self.primary_length.store(0, Ordering::Release);
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
            .read(&self.owned_worker_id, oplog_index, 1)
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
        done: Option<Sender<()>>,
    },
    TransferFromLower {
        source: usize,
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog + Send + Sync>>,
        done: Option<Sender<()>>,
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
                self.append_target(entries).await;
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
            let old_count = self.entry_count.fetch_add(1, Ordering::AcqRel);
            let count = old_count + 1;
            if count >= self.entry_count_limit {
                debug!("Enqueuing transfer of oplog entries from the oplog layer {} to the next layer up to {last_idx}", self.layer);
                let _ = self.transfer.send(TransferFromLower {
                    source: self.layer,
                    last_transferred_idx: last_idx,
                    keep_alive: None,
                    done: None,
                });
                // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
                self.entry_count.store(0, Ordering::Release);
            }
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.archive.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.archive.drop_prefix(last_dropped_id).await;
        let new_length = self.archive.length().await;
        let old_entry_count = self.entry_count.load(Ordering::Acquire);
        let new_entry_count = min(new_length, old_entry_count);
        self.entry_count.store(new_entry_count, Ordering::Release);
    }

    async fn length(&self) -> u64 {
        self.archive.length().await
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.archive.get_last_index().await
    }
}

struct BackgroundTransferFromPrimary {
    owned_worker_id: OwnedWorkerId,
    last_transferred_idx: OplogIndex,
    multi_layer_oplog_service: MultiLayerOplogService,
    primary: Arc<dyn Oplog + Send + Sync>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
}

impl BackgroundTransferFromPrimary {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        last_transferred_idx: OplogIndex,
        multi_layer_oplog_service: MultiLayerOplogService,
        primary: Arc<dyn Oplog + Send + Sync>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        Self {
            owned_worker_id,
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
            .read_prefix(&self.owned_worker_id, self.last_transferred_idx)
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
