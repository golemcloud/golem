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

impl Drop for PrimaryOplog {
    fn drop(&mut self) {
        let _ = self.jobs.send(OplogJob::Close);
        if let Some(close) = self.close.get_mut().unwrap().take() {
            close();
        }
    }
}

impl PrimaryOplog {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        shard_epoch: Option<ShardEpoch>,
        fence: Option<OplogFence>,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
        namespace: IndexedStorageNamespace,
        key: String,
        last_oplog_idx: OplogIndex,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        fingerprint: AgentFingerprint,
        stream_session_index: Option<Arc<crate::services::oplog::StreamSessionIndexService>>,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let account_id_label = account_id.to_string();
        let environment_id_label = owned_agent_id.environment_id().to_string();
        let fence = Arc::new(match fence {
            Some(fence) => std::sync::OnceLock::from(fence),
            None => std::sync::OnceLock::new(),
        });
        let auto_commit_listener = AutoCommitListener::default();
        let mut state = PrimaryOplogState {
            shard_epoch,
            fence: fence.clone(),
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_payload_size,
            retry_config,
            namespace,
            key: key.clone(),
            buffer: VecDeque::new(),
            last_committed_idx: last_oplog_idx,
            last_reported_commit_idx: last_oplog_idx,
            retained_auto_commits: VecDeque::new(),
            auto_commit_listener: auto_commit_listener.clone(),
            last_oplog_idx,
            owned_agent_id,
            agent_mode,
            account_id,
            fingerprint,
            account_id_label,
            environment_id_label,
            last_added_non_hint_entry: None,
            pending_uploads: Vec::new(),
            durable_stream_sessions: crate::services::oplog::raw_session::RawSessionCache::default(
            ),
        };
        let owned_agent_id = state.owned_agent_id.clone();
        let agent_mode = state.agent_mode;
        let fingerprint = state.fingerprint;

        let (jobs, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<OplogJob>();
        let actor = tokio::spawn(async move {
            while let Some(job) = job_rx.recv().await {
                match job {
                    OplogJob::Close => break,
                    OplogJob::Add { entry, done } => {
                        record_oplog_call("add");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let idx = state.push(entry);
                        // A threshold commit failing must fail the `add` that triggered it: the
                        // caller would otherwise be told its entry landed when the batch it was
                        // folded into was refused.
                        let result = match state.maybe_commit().await {
                            Ok(()) => Ok(idx),
                            Err(error) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::AddDurableStreamBatch { make_batch, done } => {
                        record_oplog_call("add_durable_stream_batch");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let first_index = state.last_oplog_idx.next();
                        let records = make_batch(first_index);
                        let serialized = records
                            .into_iter()
                            .map(|record| record.serialize().map(|bytes| (record, bytes)))
                            .collect::<Result<Vec<_>, _>>();
                        let result = serialized.and_then(|serialized| {
                            let mut prepared = Vec::with_capacity(serialized.len());
                            for (record, bytes) in serialized {
                                let ReservedPayload {
                                    raw,
                                    pending: _,
                                    guard,
                                } = state.reserve_raw_payload(bytes);
                                prepared.push((record.into_entry(raw)?, guard));
                            }
                            let mut result = Vec::with_capacity(prepared.len());
                            for (entry, guard) in prepared {
                                let index = state.push(entry.clone());
                                drop(guard);
                                result.push((index, entry));
                            }
                            Ok(result)
                        });
                        let result = match (result, state.maybe_commit().await) {
                            (Ok(value), Ok(())) => Ok(value),
                            (Err(error), _) => Err(error.into()),
                            (Ok(_), Err(error)) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::AddPair {
                        start,
                        make_second,
                        done,
                    } => {
                        record_oplog_call("add_pair");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let first_idx = state.push(start);
                        let second = make_second(first_idx);
                        let second_idx = state.push(second);
                        // Both halves of the pair share the threshold commit, so a refused
                        // commit fails the pair rather than reporting a write that was rolled
                        // back.
                        let result = state.maybe_commit().await.map(|()| (first_idx, second_idx));
                        let _ = done.send(result);
                    }
                    OplogJob::AddStart {
                        serialized_request,
                        build_start,
                        done,
                    } => {
                        record_oplog_call("add_start_with_reserved_raw_payload");
                        // ORDERING (Start determinism) — CRITICAL SECTION: reserving the payload,
                        // building the `Start`, and assigning its index happen as one non-yielding
                        // step here on the actor, so a concurrently enqueued writer cannot
                        // interleave its own `Start` and reorder the deterministic replay
                        // sequence. The reservation only *starts* the (possibly large) blob
                        // upload; it is not awaited here. Durability of the blob before any
                        // referencing entry is committed is enforced by the commit barrier in
                        // `append`.
                        //
                        // The no-`.await` window is enforced at compile time by the `!Send`
                        // `guard`: this actor future must stay `Send` for `tokio::spawn`, so a
                        // refactor holding the guard across an `.await` is rejected rather than
                        // silently breaking ordering. Do not move `drop(guard)` before `push`.
                        //
                        // A fenced oplog refuses before reserving, so no upload is started for a
                        // `Start` that can never be written.
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let result = {
                            let ReservedPayload {
                                raw,
                                pending,
                                guard,
                            } = state.reserve_raw_payload(serialized_request);
                            match build_start(raw) {
                                Ok(entry) => {
                                    let index = state.push(entry.clone());
                                    drop(guard);
                                    Ok(OrderedOplogStart {
                                        index,
                                        entry,
                                        pending_upload: pending,
                                    })
                                }
                                Err(err) => Err(err),
                            }
                        };
                        let result = match (result, state.maybe_commit().await) {
                            (Ok(value), Ok(())) => Ok(value),
                            (Err(error), _) => Err(error.into()),
                            (Ok(_), Err(error)) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::AddIndexedStart {
                        build_request,
                        done,
                    } => {
                        record_oplog_call("add_start_with_indexed_reserved_raw_payload");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let result = build_request(state.last_oplog_idx.next()).and_then(
                            |(serialized_request, build_start)| {
                                let ReservedPayload {
                                    raw,
                                    pending,
                                    guard,
                                } = state.reserve_raw_payload(serialized_request);
                                let entry = build_start(raw)?;
                                let index = state.push(entry.clone());
                                drop(guard);
                                Ok(OrderedOplogStart {
                                    index,
                                    entry,
                                    pending_upload: pending,
                                })
                            },
                        );
                        let result = match (result, state.maybe_commit().await) {
                            (Ok(value), Ok(())) => Ok(value),
                            (Err(error), _) => Err(error.into()),
                            (Ok(_), Err(error)) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::Commit { level, done } => {
                        let previously_committed_through = state.last_committed_idx;
                        let committed = match level {
                            CommitLevel::ReportOnly => Ok(BTreeMap::new()),
                            level => state.commit(level).await,
                        };
                        let result = match committed {
                            Ok(committed) => Ok(state
                                .committed_since_last_report(
                                    previously_committed_through,
                                    committed,
                                )
                                .await),
                            Err(error) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::Flush { done } => {
                        // The job has no error to reply with. A fence is latched on the state,
                        // where `wait_for_replicas` reads it after this job, so a fenced flush is
                        // reported as not durable rather than lost; every subsequent write fails on
                        // it without asking the storage again. A transient storage failure is fatal
                        // here as everywhere else.
                        match state.auto_commit().await {
                            Ok(()) | Err(OplogError::Fenced(_)) => {}
                            Err(error) => panic!("oplog write: {error}"),
                        }
                        let _ = done.send(());
                    }
                    OplogJob::DropPrefix {
                        last_dropped_id,
                        done,
                    } => {
                        let result = async {
                            let before = state.reader().try_length().await?;
                            state.try_drop_prefix(last_dropped_id).await?;
                            let remaining = state.reader().try_length().await?;
                            let dropped = before - remaining;
                            if dropped > 0 {
                                let account_id = state.account_id.to_string();
                                let environment_id =
                                    state.owned_agent_id.environment_id().to_string();
                                record_storage_objects_deleted(
                                    STORAGE_TYPE_OPLOG,
                                    &account_id,
                                    &environment_id,
                                    dropped,
                                );
                            }
                            Ok(dropped)
                        }
                        .await;
                        let _ = done.send(result);
                    }
                    OplogJob::CurrentIndex { done } => {
                        let _ = done.send(state.last_oplog_idx);
                    }
                    OplogJob::RawDurableStreamSessionStatus { session_key, done } => {
                        let cached = state
                            .durable_stream_sessions
                            .cached(&session_key)
                            .transpose();
                        let _ = done.send(RawSessionLookup {
                            watermark: state.last_oplog_idx,
                            committed: state.last_committed_idx,
                            buffer: if cached.is_none() {
                                state.buffer.clone()
                            } else {
                                VecDeque::new()
                            },
                            cached,
                        });
                    }
                    OplogJob::CompleteRawDurableStreamSessionStatus {
                        session_key,
                        expected_watermark,
                        expected_committed,
                        status,
                        done,
                    } => {
                        let result = if state.last_oplog_idx == expected_watermark
                            && state.last_committed_idx == expected_committed
                        {
                            if let Ok(value) = status.as_ref() {
                                state
                                    .durable_stream_sessions
                                    .insert(session_key, value.clone());
                            }
                            Some(crate::services::oplog::RawDurableStreamSessionStatus {
                                watermark: expected_watermark,
                                status: *status,
                            })
                        } else {
                            None
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::LastAddedNonHintEntry { done } => {
                        let _ = done.send(state.last_added_non_hint_entry);
                    }
                    OplogJob::Reader { done } => {
                        let _ = done.send(state.reader());
                    }
                    OplogJob::BlobContext { done } => {
                        let _ = done.send(OplogBlobContext {
                            blob_storage: state.blob_storage.clone(),
                            owned_agent_id: state.owned_agent_id.clone(),
                            agent_mode: state.agent_mode,
                            account_id: state.account_id,
                            max_payload_size: state.max_payload_size,
                        });
                    }
                }
            }
            let mut upload_result = Ok(());
            for upload in state.pending_uploads {
                upload_result = upload_result.and(upload.wait().await);
            }
            upload_result
        });

        Self {
            jobs,
            closed: async move { actor.await.map_err(|error| error.to_string())? }
                .boxed()
                .shared(),
            tasks: crate::services::oplog::WorkerTasks::default(),
            retired: AtomicBool::new(false),
            key,
            owned_agent_id,
            agent_mode,
            fingerprint,
            shard_epoch,
            max_payload_size,
            fence,
            stream_session_index,
            auto_commit_listener,
            close: Mutex::new(Some(close)),
        }
    }

    /// Sends a job to the actor and waits for its reply.
    ///
    /// A missing reply means the actor failed or this handle was used after retirement.
    /// Orderly shutdown drains jobs queued before Close.
    async fn run_job<R>(
        &self,
        make_job: impl FnOnce(tokio::sync::oneshot::Sender<R>) -> OplogJob,
    ) -> R {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(make_job(done)).is_err() {
            panic!("Oplog actor for {} terminated unexpectedly", self.key);
        }
        match done_rx.await {
            Ok(result) => result,
            Err(_) => panic!(
                "Oplog actor for {} dropped a request without replying",
                self.key
            ),
        }
    }
}
impl Debug for PrimaryOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

#[async_trait]
impl Oplog for PrimaryOplog {
    fn executor_shutdown_handle(&self) -> crate::services::oplog::OplogShutdownHandle {
        let jobs = self.jobs.clone();
        let closed = self.closed();
        crate::services::oplog::OplogShutdownHandle::new(
            || {},
            move || {
                let jobs = jobs.clone();
                let closed = closed.clone();
                async move {
                    let _ = jobs.send(OplogJob::Close);
                    closed.await
                }
                .boxed()
            },
        )
    }

    fn retire(&self) {
        if !self.retired.swap(true, Ordering::AcqRel) {
            let _ = self.jobs.send(OplogJob::Close);
        }
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire) || self.jobs.is_closed()
    }

    fn closed(&self) -> OplogCloseCompletion {
        self.closed.clone()
    }

    fn task_owner(&self) -> Option<&crate::services::oplog::WorkerTasks> {
        Some(&self.tasks)
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(OplogJob::Add { entry, done }).is_err() {
            panic!("Oplog actor for {} terminated unexpectedly", self.key);
        }
        let key = self.key.clone();
        Box::pin(async move {
            done_rx.await.unwrap_or_else(|_| {
                panic!("Oplog actor for {key} dropped an add request without replying")
            })
        })
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        self.run_job(|done| OplogJob::AddDurableStreamBatch { make_batch, done })
            .await
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> crate::services::oplog::OplogAddPairReceipt {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self
            .jobs
            .send(OplogJob::AddPair {
                start,
                make_second,
                done,
            })
            .is_err()
        {
            panic!("Oplog actor for {} terminated unexpectedly", self.key);
        }
        let key = self.key.clone();
        Box::pin(async move {
            done_rx.await.unwrap_or_else(|_| {
                panic!("Oplog actor for {key} dropped an add-pair request without replying")
            })
        })
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.try_drop_prefix(last_dropped_id)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, String> {
        self.run_job(|done| OplogJob::DropPrefix {
            last_dropped_id,
            done,
        })
        .await
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        self.run_job(|done| OplogJob::Commit { level, done }).await
    }

    fn set_auto_commit_listener(&self, listener: Weak<crate::services::oplog::AutoCommitCallback>) {
        self.auto_commit_listener.set(listener);
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.run_job(|done| OplogJob::CurrentIndex { done }).await
    }

    async fn try_current_oplog_index(&self) -> Result<OplogIndex, String> {
        Ok(self.current_oplog_index().await)
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> crate::services::oplog::RawDurableStreamSessionStatus {
        loop {
            let snapshot = self
                .run_job(|done| OplogJob::RawDurableStreamSessionStatus {
                    session_key: session_key.clone(),
                    done,
                })
                .await;
            if let Some(status) = snapshot.cached {
                return crate::services::oplog::RawDurableStreamSessionStatus {
                    watermark: snapshot.watermark,
                    status,
                };
            }
            let status = crate::services::oplog::raw_session::RawSessionCache::reconstruct(
                self.stream_session_index.as_ref(),
                &self.owned_agent_id,
                self.agent_mode,
                self.fingerprint,
                snapshot.committed,
                &snapshot.buffer,
                session_key,
            )
            .await;
            if let Some(result) = self
                .run_job(|done| OplogJob::CompleteRawDurableStreamSessionStatus {
                    session_key: session_key.clone(),
                    expected_watermark: snapshot.watermark,
                    expected_committed: snapshot.committed,
                    status: Box::new(status),
                    done,
                })
                .await
            {
                return result;
            }
        }
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.run_job(|done| OplogJob::LastAddedNonHintEntry { done })
            .await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<bool, OplogError> {
        record_oplog_call("wait_for_replicas");

        self.run_job(|done| OplogJob::Flush { done }).await;
        // A refused flush reached no replica. The storage would still answer with its replica
        // count, and passing that on would tell the caller that entries it turned away are durable.
        if let Some(fence) = self.fence.get() {
            return Err(OplogError::Fenced(fence.clone()));
        }
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        let replicas = replicas.min(reader.replicas);
        match reader
            .indexed_storage
            .with("oplog", "wait_for_replicas")
            .wait_for_replicas(replicas, timeout)
            .await
        {
            Ok(n) => Ok(n == replicas),
            Err(err) => {
                error!("Failed to wait for replicas to sync indexed storage: {err}");
                Ok(false)
            }
        }
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        let unreadable = |error| UnreadableOplog::new(&self.owned_agent_id.agent_id, error);
        checked_range_end(oplog_index, n).map_err(unreadable)?;
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        let entries = reader.read_source(oplog_index, n).await;
        exact_from_source(OplogReadSource::Primary, oplog_index, n, entries).map_err(unreadable)
    }

    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        Ok(reader.read_source(oplog_index, n).await)
    }

    async fn try_read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.try_read_source(oplog_index, n).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> Result<OplogEntry, UnreadableOplog> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        Ok(reader.read(oplog_index).await)
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        Ok(reader.length().await)
    }

    async fn try_length(&self) -> Result<u64, String> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.try_length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        // A retired oplog still takes the actor path, which reports the terminated actor.
        if data.len() <= self.max_payload_size && !self.is_retired() {
            return Ok(RawOplogPayload::SerializedInline(data));
        }
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        let data_len = data.len() as u64;
        let result = PrimaryOplogService::upload_raw_payload(
            ctx.blob_storage,
            ctx.max_payload_size,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            data,
        )
        .await;
        if let Ok(RawOplogPayload::External { .. }) = &result {
            // Only count bytes that were actually uploaded externally
            record_storage_bytes_written(
                STORAGE_TYPE_OPLOG,
                &ctx.account_id.to_string(),
                &ctx.owned_agent_id.environment_id().to_string(),
                data_len,
            );
        }
        result
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        PrimaryOplogService::download_raw_payload(
            ctx.blob_storage,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        PrimaryOplogService::download_raw_payload_classified(
            ctx.blob_storage,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        // ORDERING (Start determinism): the job is enqueued synchronously here — there is no
        // `.await` between a subtask initiating its durable operation and this send — and the
        // actor assigns `Start` indices strictly in job order, so initiation order becomes
        // `Start`-index order exactly as the ordering contract requires. See the ordering note on
        // [`PrimaryOplog`].
        self.run_job(|done| OplogJob::AddStart {
            serialized_request,
            build_start,
            done,
        })
        .await
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.run_job(|done| OplogJob::AddIndexedStart {
            build_request,
            done,
        })
        .await
    }

    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.shard_epoch
    }

    fn fence(&self) -> Option<OplogFence> {
        self.fence.get().cloned()
    }
}
