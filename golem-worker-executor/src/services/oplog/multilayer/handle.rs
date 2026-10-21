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

use super::*;

impl MultiLayerOplog {
    #[allow(clippy::new_ret_no_self)]
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        primary: Arc<dyn Oplog>,
        multi_layer_oplog_service: MultiLayerOplogService,
        close: Box<dyn FnOnce() + Send + Sync>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, UnreadableOplog> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // Every layer is opened, recording `shard_epoch` on it, before the archive watermark is
        // read below: an older owner's transfer either landed before this owner's record, and the
        // watermark covers it, or is refused after it.
        //
        // A primary that refused this owner's claim leaves the handle finished: its commits fail
        // and `archive` refuses, so nothing ever writes its layers. They are opened without an
        // epoch rather than paying a refused write, a warning, or a stale claim on each.
        let shard_epoch = shard_epoch.filter(|_| primary.fence().is_none());
        let mut lower: Vec<Arc<dyn OplogArchive + Send + Sync>> = Vec::new();
        for (i, layer) in multi_layer_oplog_service.lower.iter().enumerate() {
            // A layer that does not open fails the open of the oplog, and nothing is cached.
            let raw = match layer.open(&owned_agent_id, agent_mode, shard_epoch).await {
                Ok(raw) => raw,
                Err(error) => {
                    close();
                    return Err(unreadable_archive(&owned_agent_id, i, error));
                }
            };
            if i != (multi_layer_oplog_service.lower.len().get() - 1) {
                let instrumented = Arc::new(InstrumentedOplogArchive::new(
                    raw,
                    account_id,
                    owned_agent_id.environment_id(),
                ));
                lower.push(Arc::new(
                    WrappedOplogArchive::new(
                        i,
                        instrumented,
                        tx.clone(),
                        owned_agent_id.clone(),
                        multi_layer_oplog_service.clone(),
                        multi_layer_oplog_service.entry_count_limit,
                    )
                    .await,
                ));
            } else {
                lower.push(Arc::new(InstrumentedOplogArchive::new(
                    raw,
                    account_id,
                    owned_agent_id.environment_id(),
                )));
            }
        }
        let lower = NEVec::try_from_vec(lower).expect("At least one lower layer is required");

        let last_reported_commit_index =
            AtomicOplogIndex::from_oplog_index(primary.current_oplog_index().await);
        let mut archived_through = OplogIndex::NONE;
        for layer in &lower {
            match layer.get_last_index().await {
                Ok(index) => archived_through = archived_through.max(index),
                Err(error) => {
                    warn!(
                        agent_id = %owned_agent_id.agent_id,
                        error = %error,
                        "Failed to inspect archived oplog while opening; archive scheduling will retry"
                    );
                    archived_through = OplogIndex::NONE;
                    break;
                }
            }
        }
        let last_transfer_point = AtomicOplogIndex::from_oplog_index(archived_through);
        let result = Arc::new(Self {
            owned_agent_id: owned_agent_id.clone(),
            agent_mode,
            primary: primary.clone(),
            lower: lower.clone(),
            retired: Arc::new(AtomicBool::new(false)),
            multi_layer_oplog_service: multi_layer_oplog_service.clone(),
            transfer_fiber: new_transfer_fiber(),
            transfer: tx,
            last_reported_commit_index,
            last_transfer_point,
            close_fn: Mutex::new(Some(close)),
        });
        let result_oplog: Arc<dyn Oplog> = result.clone();
        multi_layer_oplog_service.register_transfer(
            result.owned_agent_id.agent_id.clone(),
            &result.transfer_fiber,
        );
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let transfer_service = multi_layer_oplog_service.clone();
        let transfer_fiber = tokio::spawn(async move {
            if start_rx.await.is_ok() {
                Self::background_transfer(
                    owned_agent_id,
                    Arc::downgrade(&result_oplog),
                    lower,
                    transfer_service,
                    rx,
                )
                .await;
            }
        });
        result
            .set_background_transfer(start_tx, transfer_fiber)
            .await;

        Ok(result)
    }

    async fn set_background_transfer(&self, start: Sender<()>, fiber: tokio::task::JoinHandle<()>) {
        MultiLayerOplogService::start_transfer(&self.transfer_fiber, start, fiber).await;
    }

    async fn background_transfer(
        owned_agent_id: OwnedAgentId,
        primary: Weak<dyn Oplog>,
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
                    transfer_origin,
                } => {
                    async {
                        multi_layer_oplog_service
                            .wait_until_archive_ready(
                                &owned_agent_id,
                                ArchiveSource::Primary,
                            )
                            .await;
                        info!(
                            "Transferring oplog entries up to index {last_transferred_idx} of the primary oplog to the next layer"
                        );
                        debug!("Reading entries from the primary oplog");

                        if let Some(primary) = primary.upgrade() {
                            let transfer = BackgroundTransferFromPrimary::new(
                                last_transferred_idx,
                                primary.clone(),
                                lower.clone(),
                            );
                            let result = transfer.run().await;
                            let _ = keep_alive.take();

                            match &result {
                                Ok(()) => multi_layer_oplog_service.record_archive_success(
                                    &owned_agent_id,
                                    ArchiveSource::Primary,
                                ),
                                // A newer owner holds the oplog: nothing here is retried.
                                Err(OplogError::Fenced(_)) => {
                                    info!("Oplog transfer stopped: the shard has a new owner")
                                }
                                Err(error) => {
                                    multi_layer_oplog_service.record_archive_failure(
                                        &owned_agent_id,
                                        ArchiveSource::Primary,
                                    );
                                    crate::metrics::oplog::record_archive_maintenance_failure(
                                        "primary_transfer",
                                    );
                                    warn!(error = %error, "Failed to archive oplog; the source entries remain available for retry");
                                }
                            }
                            if let Some(done) = done {
                                let _ = done.send(result);
                            }
                        }
                    }
                    .instrument(related_span!(
                        transfer_origin,
                        Level::INFO,
                        "oplog_background_transfer",
                        agent_id = %owned_agent_id.agent_id,
                        from = "primary",
                        last_transferred_idx = %last_transferred_idx,
                    ))
                    .await;
                }
                TransferFromLower {
                    source,
                    last_transferred_idx,
                    mut keep_alive,
                    done,
                    drain: _,
                    transfer_origin,
                } => {
                    async {
                        multi_layer_oplog_service
                            .wait_until_archive_ready(
                                &owned_agent_id,
                                ArchiveSource::Lower(source),
                            )
                            .await;
                        info!(
                            "Transferring oplog entries up to index {last_transferred_idx} of oplog layer {source} to the next layer"
                        );
                        debug!("Reading entries from oplog layer {source}");

                        let result = transfer_between_lower_layers(
                            source,
                            last_transferred_idx,
                            lower.clone(),
                        )
                        .await;
                        let _ = keep_alive.take();

                        match &result {
                            Ok(()) => multi_layer_oplog_service.record_archive_success(
                                &owned_agent_id,
                                ArchiveSource::Lower(source),
                            ),
                            // A newer owner holds the oplog: nothing here is retried.
                            Err(OplogError::Fenced(_)) => {
                                info!("Oplog transfer stopped: the shard has a new owner")
                            }
                            Err(error) => {
                                multi_layer_oplog_service.record_archive_failure(
                                    &owned_agent_id,
                                    ArchiveSource::Lower(source),
                                );
                                crate::metrics::oplog::record_archive_maintenance_failure(
                                    "lower_layer_transfer",
                                );
                                warn!(error = %error, "Failed to archive oplog; the source entries remain available for retry");
                            }
                        }
                        if let Some(done) = done {
                            let _ = done.send(result);
                        }
                    }
                    .instrument(related_span!(
                        transfer_origin,
                        Level::INFO,
                        "oplog_background_transfer",
                        agent_id = %owned_agent_id.agent_id,
                        // A string in both arms: the same field name typed
                        // differently per call site conflicts in a typed store.
                        from = %format!("layer-{source}"),
                        last_transferred_idx = %last_transferred_idx,
                    ))
                    .await;
                }
            }
        }
    }

    pub async fn try_archive(this: &Arc<dyn Oplog>) -> Result<Option<bool>, String> {
        let Some(this) = downcast_oplog::<MultiLayerOplog>(this) else {
            return Ok(None);
        };
        Self::archive(this, false).await.map(Some)
    }

    pub async fn try_archive_blocking(this: &Arc<dyn Oplog>) -> Result<Option<bool>, String> {
        let Some(this) = downcast_oplog::<MultiLayerOplog>(this) else {
            return Ok(None);
        };
        Self::archive(this, true).await.map(Some)
    }

    async fn archive(this: Arc<Self>, blocking: bool) -> Result<bool, String> {
        // A newer owner holds the oplog: its own archiving decides what moves, and nothing here
        // could be written anyway.
        if this.fence().is_some() {
            return Ok(false);
        }
        let (done_tx, done_rx) = if blocking {
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            (Some(done_tx), Some(done_rx))
        } else {
            (None, None)
        };
        let result = if this.primary.try_length().await? > 0 {
            // Unreported automatic commits must remain in primary storage until the next
            // explicit commit returns them to the status reducer.
            let last_transferred_idx = this.last_reported_commit_index.get();
            if last_transferred_idx == OplogIndex::NONE {
                return Ok(true);
            }
            if !this
                .multi_layer_oplog_service
                .begin_archive_attempt(&this.owned_agent_id, ArchiveSource::Primary)
            {
                return if blocking {
                    Err("Oplog archive retry is backed off after a previous failure".to_string())
                } else {
                    Ok(true)
                };
            }
            this.transfer
                .send(TransferFromPrimary {
                    last_transferred_idx,
                    keep_alive: Some(this.clone()),
                    done: done_tx,
                    transfer_origin: TraceOrigin::capture_current(),
                })
                .expect("Failed to enqueue transfer of primary oplog entries");

            // Retry if additions beyond the reported prefix still need archiving.
            if blocking {
                this.lower.len().get() > 1
                    || this.primary.try_current_oplog_index().await? > last_transferred_idx
            } else {
                // The scheduled follow-up rechecks the authoritative source after `next_after`;
                // failed transfers leave that source intact and are retried on that later pass.
                true
            }
        } else {
            let mut first_non_empty = None;
            for n in 0..this.lower.len().get().saturating_sub(1) {
                if this.lower[n]
                    .length()
                    .await
                    .map_err(|error| error.to_string())?
                    > 0
                {
                    first_non_empty = Some(n);
                    break;
                }
            }

            if let Some(first_non_empty) = first_non_empty {
                // transferring the whole non-empty lower layer to the next layer
                let last_transferred_idx = this.lower[first_non_empty]
                    .current_oplog_index()
                    .await
                    .map_err(|error| error.to_string())?;
                if !this.multi_layer_oplog_service.begin_archive_attempt(
                    &this.owned_agent_id,
                    ArchiveSource::Lower(first_non_empty),
                ) {
                    return if blocking {
                        Err("Oplog archive retry is backed off after a previous failure"
                            .to_string())
                    } else {
                        Ok(true)
                    };
                }
                this.transfer
                    .send(TransferFromLower {
                        source: first_non_empty,
                        last_transferred_idx,
                        keep_alive: Some(this.clone()),
                        done: done_tx,
                        drain: false,
                        transfer_origin: TraceOrigin::capture_current(),
                    })
                    .expect("Failed to enqueue transfer of primary oplog entries");

                !blocking || first_non_empty < this.lower.len().get() - 2
            } else {
                // Fully archived, and no transfer was enqueued to wait for
                return Ok(false);
            }
        };

        if let Some(done_rx) = done_rx {
            let transferred = done_rx.await.map_err(|_| {
                "Oplog archive transfer stopped before reporting completion".to_string()
            })?;
            match transferred {
                Ok(()) => {}
                Err(OplogError::Fenced(_)) => return Ok(false),
                Err(error) => return Err(error.to_string()),
            }
        }

        Ok(result)
    }
}

impl Drop for MultiLayerOplog {
    fn drop(&mut self) {
        self.multi_layer_oplog_service
            .unregister_transfer(&self.owned_agent_id.agent_id, &self.transfer_fiber);
        if let Some(close_fn) = self.close_fn.get_mut().unwrap().take() {
            close_fn();
        }
        self.multi_layer_oplog_service
            .abort_transfer_in_drop(&self.transfer_fiber);
    }
}

impl Debug for MultiLayerOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiLayerOplog")
            .field("agent_id", &self.owned_agent_id)
            .finish()
    }
}

#[async_trait]
impl Oplog for MultiLayerOplog {
    fn executor_shutdown_handle(&self) -> crate::services::oplog::OplogShutdownHandle {
        let retired = self.retired.clone();
        let service = self.multi_layer_oplog_service.clone();
        let agent_id = self.owned_agent_id.agent_id.clone();
        let transfer_fiber = self.transfer_fiber.clone();
        let inner = self.primary.executor_shutdown_handle();
        let close_retired = retired.clone();
        let close_service = service.clone();
        let close_agent_id = agent_id.clone();
        let close_transfer_fiber = transfer_fiber.clone();
        let close_inner = inner.clone();
        let closed = self.closed();
        crate::services::oplog::OplogShutdownHandle::new(
            move || {
                retired.store(true, Ordering::Release);
                service.unregister_transfer(&agent_id, &transfer_fiber);
                service.abort_transfer_in_drop(&transfer_fiber);
                inner.fence();
            },
            move || {
                let retired = close_retired.clone();
                let service = close_service.clone();
                let agent_id = close_agent_id.clone();
                let transfer_fiber = close_transfer_fiber.clone();
                let closed = closed.clone();
                let inner = close_inner.clone();
                async move {
                    retired.store(true, Ordering::Release);
                    service.unregister_transfer(&agent_id, &transfer_fiber);
                    service.abort_transfer_in_drop(&transfer_fiber);
                    let local = closed.await;
                    local.and(inner.close_and_wait().await)
                }
                .boxed()
            },
        )
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.multi_layer_oplog_service
            .unregister_transfer(&self.owned_agent_id.agent_id, &self.transfer_fiber);
        self.multi_layer_oplog_service
            .abort_transfer_in_drop(&self.transfer_fiber);
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire) || self.primary.is_retired()
    }

    fn closed(&self) -> OplogCloseCompletion {
        MultiLayerOplogService::transfer_closed(&self.transfer_fiber)
    }

    fn task_owner(&self) -> Option<&crate::services::oplog::WorkerTasks> {
        self.primary.task_owner()
    }

    async fn stop_and_wait(&self) -> Result<(), String> {
        let tasks_result = if let Some(tasks) = self.task_owner() {
            tasks.stop_and_wait().await
        } else {
            Ok(())
        };
        self.retire();
        let result = self.closed().await;
        let primary_result = self.primary.stop_and_wait().await;
        tasks_result.and(result).and(primary_result)
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        self.primary.enqueue_add(entry)
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        self.primary.add_durable_stream_batch(make_batch).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let dropped_entries = self.primary.drop_prefix(last_dropped_id).await;
        self.last_transfer_point.max(last_dropped_id);
        dropped_entries
    }

    async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, String> {
        let dropped_entries = self.primary.try_drop_prefix(last_dropped_id).await?;
        self.last_transfer_point.max(last_dropped_id);
        Ok(dropped_entries)
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        let result = self.primary.commit(level).await?;

        if let Some(index) = result.keys().next_back() {
            self.last_reported_commit_index.max(*index);
        }
        let last_committed_idx = self.last_reported_commit_index.get();
        let last_transferred_idx = self.last_transfer_point.get();
        let count = u64::from(last_committed_idx).saturating_sub(u64::from(last_transferred_idx));
        if count >= self.multi_layer_oplog_service.entry_count_limit
            || self
                .multi_layer_oplog_service
                .archive_retry_due(&self.owned_agent_id, ArchiveSource::Primary)
        {
            if !self
                .multi_layer_oplog_service
                .begin_archive_attempt(&self.owned_agent_id, ArchiveSource::Primary)
            {
                return Ok(result);
            }
            debug!(
                "Enqueuing transfer of {count} oplog entries from the primary oplog to the next layer up to {last_committed_idx}"
            );
            let _ = self.transfer.send(TransferFromPrimary {
                last_transferred_idx: last_committed_idx,
                keep_alive: None,
                done: None,
                transfer_origin: TraceOrigin::capture_current(),
            });
            self.last_transfer_point.max(last_committed_idx);
        }
        Ok(result)
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.primary.current_oplog_index().await
    }

    async fn try_current_oplog_index(&self) -> Result<OplogIndex, String> {
        self.primary.try_current_oplog_index().await
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> crate::services::oplog::RawDurableStreamSessionStatus {
        self.primary
            .raw_durable_stream_session_status(session_key)
            .await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.primary.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<bool, OplogError> {
        self.primary.wait_for_replicas(replicas, timeout).await
    }

    async fn read_exact(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        let unreadable = |error| UnreadableOplog::new(&self.owned_agent_id.agent_id, error);
        let mut read = OplogRead::new(idx, n).map_err(unreadable)?;

        if let Some((start, count)) = read.next_range() {
            let entries = self.primary.read_source(start, count).await?;
            read.add_source(OplogReadSource::Primary, entries)
                .map_err(unreadable)?;
        }

        for (level, layer) in self.lower.iter().enumerate() {
            if let Some((start, count)) = read.next_range() {
                let entries = layer.read_source(start, count).await.map_err(unreadable)?;
                read.add_source(OplogReadSource::Archive(level), entries)
                    .map_err(unreadable)?;
            } else {
                break;
            }
        }

        read.finish().map_err(unreadable)
    }

    async fn try_read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        self.primary.try_read_source(idx, n).await
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        let mut total_length = self.primary.length().await?;
        for (level, layer) in self.lower.iter().enumerate() {
            total_length += layer
                .length()
                .await
                .map_err(|error| unreadable_archive(&self.owned_agent_id, level, error))?;
        }
        Ok(total_length)
    }

    async fn try_length(&self) -> Result<u64, String> {
        let mut total_length = self.primary.try_length().await?;
        for layer in &self.lower {
            total_length += layer.length().await.map_err(|error| error.to_string())?;
        }
        Ok(total_length)
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

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.primary
            .download_raw_payload_classified(payload_id, md5_hash)
            .await
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> crate::services::oplog::OplogAddPairReceipt {
        self.primary.enqueue_add_pair(start, make_second)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.primary
            .add_start_with_reserved_raw_payload(serialized_request, build_start)
            .await
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.primary
            .add_start_with_indexed_reserved_raw_payload(build_request)
            .await
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.primary.clone())
    }

    /// The primary's refusal, or one an archive layer latched: a newer owner recorded its
    /// generation on both, so either one means the handle is finished.
    fn fence(&self) -> Option<OplogFence> {
        self.primary.fence().or_else(|| layers_fence(&self.lower))
    }
}
