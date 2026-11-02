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

impl ForwardingOplog {
    pub async fn new(
        inner: Arc<dyn Oplog>,
        oplog_plugins: Arc<dyn OplogProcessorPlugin>,
        components: Arc<dyn ComponentService>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        last_oplog_idx: OplogIndex,
        close_fn: Box<dyn FnOnce() + Send + Sync>,
        max_commit_count: usize,
        max_elapsed_time: Duration,
    ) -> Self {
        // Seed per-plugin live state from the current status snapshot (not the
        // potentially stale initial_worker_metadata.last_known_status)
        let plugin_state = {
            let seed_status = last_known_status.get();
            let mut state = HashMap::new();
            for (grant_id, cp) in &seed_status.oplog_processor_checkpoints {
                state.insert(
                    *grant_id,
                    LivePluginState {
                        target_agent_id: cp.target_agent_id.clone(),
                        confirmed_up_to: cp.confirmed_up_to,
                        sending_up_to: cp.sending_up_to,
                        send_in_progress: false,
                        last_batch_start: cp.last_batch_start,
                    },
                );
            }
            // Also seed entries for active plugins that have no checkpoint yet (active from Create)
            for grant_id in &seed_status.active_plugins {
                state.entry(*grant_id).or_insert(LivePluginState {
                    target_agent_id: None,
                    confirmed_up_to: OplogIndex::NONE,
                    sending_up_to: OplogIndex::NONE,
                    send_in_progress: false,
                    last_batch_start: OplogIndex::NONE,
                });
            }
            state
        };

        // Captured here, where the caller's context is still current: each flush
        // tick links back to it rather than running inside it.
        let flush_origin = TraceOrigin::capture_current();
        let agent_id = initial_worker_metadata.agent_id.clone();

        let forwarding_retired = CancellationToken::new();
        let forwarding_activity = ActivityGate::new();
        let mut state = ForwardingOplogState {
            forwarding_retired: forwarding_retired.clone(),
            forwarding_activity: forwarding_activity.clone(),
            pending_checkpoint_activity: None,
            buffer: None,
            buffer_start_idx: last_oplog_idx.next(),
            commit_count: 0,
            last_send: Instant::now(),
            oplog_plugins,
            initial_worker_metadata,
            last_known_status,
            last_oplog_idx,
            last_committed_idx: last_oplog_idx,
            components,
            inner: inner.clone(),
            plugin_state,
            pending_direct_commits: BTreeMap::new(),
            worker_event_service: None,
            monitor_tasks: Vec::new(),
        };

        let (jobs, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<ForwardingJob>();
        let actor = tokio::spawn(async move {
            let result = AssertUnwindSafe(async {
                while let Some(job) = job_rx.recv().await {
                    state.debug_assert_invariants();
                    match job {
                        ForwardingJob::Close => break,
                        ForwardingJob::Add { entry, done } => {
                            let cache_entries = state.cache_is_required();
                            let cached_entry = cache_entries.then(|| entry.clone());
                            let result = state.inner.add(entry).await;
                            // A refused write is recorded nowhere: the inner oplog appended
                            // nothing, so the index it would have taken has to stay free.
                            if let Ok(idx) = &result {
                                if let Some(entry) = cached_entry {
                                    state.record_cached([(*idx, entry)]);
                                } else {
                                    state.record_uncached([*idx]);
                                }
                            }
                            let _ = done.send(result);
                        }
                        ForwardingJob::AddDurableStreamBatch { make_batch, done } => {
                            let cache_entries = state.cache_is_required();
                            let result = state.inner.add_durable_stream_batch(make_batch).await;
                            if let Ok(entries) = &result {
                                if cache_entries {
                                    state.record_cached(entries.iter().cloned());
                                } else {
                                    state.record_uncached(entries.iter().map(|(idx, _)| *idx));
                                }
                            }
                            let _ = done.send(result);
                        }
                        ForwardingJob::AddPair {
                            start,
                            make_second,
                            done,
                        } => {
                            // The `Start` will be appended at the next index; this wrapper tracks
                            // `last_oplog_idx` in lockstep with the inner oplog, so the predicted
                            // index matches the one the inner oplog assigns.
                            let first_idx = state.last_oplog_idx.next();
                            let second = make_second(first_idx);
                            let cache_entries = state.cache_is_required();
                            let cached_entries =
                                cache_entries.then(|| (start.clone(), second.clone()));
                            let result = state
                                .inner
                                .enqueue_add_pair(start, Box::new(move |_| second))
                                .await;
                            if let Ok((start_idx, second_idx)) = &result {
                                if let Some((start, second)) = cached_entries {
                                    state.record_cached([
                                        (*start_idx, start),
                                        (*second_idx, second),
                                    ]);
                                } else {
                                    state.record_uncached([*start_idx, *second_idx]);
                                }
                            }
                            let _ = done.send(result);
                        }
                        ForwardingJob::AddStart {
                            serialized_request,
                            build_start,
                            done,
                        } => {
                            let cache_entries = state.cache_is_required();
                            // The `Start` entry is built deep in the leaf from the reserved payload
                            // reference, so — unlike `Add`/`AddPair` — it is mirrored into the
                            // buffer only after the delegation returns it.
                            let result = state
                                .inner
                                .add_start_with_reserved_raw_payload(
                                    serialized_request,
                                    build_start,
                                )
                                .await;
                            if let Ok(ordered) = &result {
                                if cache_entries {
                                    state.record_cached([(ordered.index, ordered.entry.clone())]);
                                } else {
                                    state.record_uncached([ordered.index]);
                                }
                            }
                            let _ = done.send(result);
                        }
                        ForwardingJob::AddIndexedStart {
                            build_request,
                            done,
                        } => {
                            let cache_entries = state.cache_is_required();
                            let result = state
                                .inner
                                .add_start_with_indexed_reserved_raw_payload(build_request)
                                .await;
                            if let Ok(ordered) = &result {
                                if cache_entries {
                                    state.record_cached([(ordered.index, ordered.entry.clone())]);
                                } else {
                                    state.record_uncached([ordered.index]);
                                }
                            }
                            let _ = done.send(result);
                        }
                        ForwardingJob::Commit { level, done } => {
                            match state.inner.commit(level).await {
                                Err(error) => {
                                    let _ = done.send(Err(error));
                                }
                                Ok(mut result) => {
                                    // Update last_committed_idx from committed entries
                                    if let Some(max_idx) = result.keys().max()
                                        && *max_idx > state.last_committed_idx
                                    {
                                        state.last_committed_idx = *max_idx;
                                    }
                                    // A report commits nothing, so it neither counts nor starts
                                    // the plugin flush, which commits the buffer of the worker.
                                    let reports_only = level == CommitLevel::ReportOnly;
                                    if !reports_only {
                                        state.commit_count += 1;
                                    }
                                    if !reports_only && state.commit_count >= max_commit_count {
                                        // Spanned inside the threshold check, not around the
                                        // commit: this arm runs per oplog commit, the flush only
                                        // every `max_commit_count` of them. The actor has no
                                        // ambient span, so without this the flush would be
                                        // untraceable.
                                        //
                                        // Named apart from the periodic
                                        // `oplog_forwarding_flush` so the two triggers stay
                                        // distinguishable in a trace backend. The link points at
                                        // the worker's startup rather than at the commit that
                                        // tripped the threshold: the actor receives commits over
                                        // a channel, so the committing invocation's context is
                                        // not available here.
                                        state
                                            .try_flush()
                                            .instrument(related_span!(
                                                flush_origin,
                                                tracing::Level::INFO,
                                                "oplog_forwarding_threshold_flush",
                                                agent_id = %agent_id
                                            ))
                                            .await;
                                    }
                                    // Merge entries committed directly to inner during flush
                                    // so the Worker folds them into AgentStatusRecord
                                    result.append(&mut state.pending_direct_commits);
                                    state.pending_checkpoint_activity.take();
                                    let _ = done.send(Ok(result));
                                }
                            }
                        }
                        ForwardingJob::SetWorkerEventService { service, done } => {
                            state.worker_event_service = Some(service);
                            let _ = done.send(());
                        }
                        ForwardingJob::Tick => {
                            async {
                                state.try_locality_recovery().await;
                                state.try_flush().await;
                            }
                            .instrument(related_span!(
                                flush_origin,
                                tracing::Level::INFO,
                                "oplog_forwarding_flush",
                                agent_id = %agent_id
                            ))
                            .await;
                        }
                        ForwardingJob::Drain { done } => {
                            let _ = done.send(());
                        }
                        #[cfg(test)]
                        ForwardingJob::Inspect { done } => {
                            let _ = done.send(ForwardingStateSnapshot {
                                buffer_len: state.buffer.as_ref().map(VecDeque::len),
                                buffer_start_idx: state.buffer_start_idx,
                                last_oplog_idx: state.last_oplog_idx,
                            });
                        }
                    }
                    state.debug_assert_invariants();
                }
            })
            .catch_unwind()
            .await;
            let mut result = result.map_err(|_| "Forwarding oplog actor panicked".to_string());
            for monitor in state.monitor_tasks {
                let joined = AssertUnwindSafe(monitor.cancel()).catch_unwind().await;
                result = result.and(
                    joined
                        .map(|_| ())
                        .map_err(|_| "Oplog monitor panicked".to_string()),
                );
            }
            result
        });

        let timer = tokio::spawn({
            let jobs = jobs.clone();
            let forwarding_retired = forwarding_retired.clone();
            async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = forwarding_retired.cancelled() => break,
                        _ = tokio::time::sleep(max_elapsed_time) => {}
                    }
                    if jobs.send(ForwardingJob::Tick).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            inner,
            jobs,
            closed: async move { actor.await.map_err(|error| error.to_string())? }
                .boxed()
                .shared(),
            retired: AtomicBool::new(false),
            timer: Some(timer),
            forwarding_retired,
            close_fn: Mutex::new(Some(close_fn)),
        }
    }

    pub(crate) fn fence_forwarding(&self) {
        self.forwarding_retired.cancel();
    }

    pub(crate) async fn drain_forwarding(&self) {
        self.run_job(|done| ForwardingJob::Drain { done }).await;
    }

    pub async fn set_worker_event_service(
        &self,
        worker_event_service: Arc<dyn WorkerEventService>,
    ) {
        self.run_job(|done| ForwardingJob::SetWorkerEventService {
            service: worker_event_service,
            done,
        })
        .await
    }

    /// Enqueues a job for the actor task and awaits its reply.
    ///
    /// A missing reply means the actor failed, or this handle was used after retirement or after
    /// `retire` stopped it - both cooperative shutdowns that run only once no
    /// caller can still be in flight, and both drain every job queued before `Close`, so a
    /// missing reply otherwise means the actor itself panicked and the oplog's state is no
    /// longer trustworthy.
    pub(super) async fn run_job<R>(
        &self,
        make_job: impl FnOnce(tokio::sync::oneshot::Sender<R>) -> ForwardingJob,
    ) -> R {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(make_job(done)).is_err() {
            panic!("Forwarding oplog actor terminated unexpectedly");
        }
        match done_rx.await {
            Ok(result) => result,
            Err(_) => panic!("Forwarding oplog actor dropped a request without replying"),
        }
    }
}

impl Debug for ForwardingOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardingOplog")
            .field("inner", &self.inner)
            .finish()
    }
}

impl Drop for ForwardingOplog {
    fn drop(&mut self) {
        self.fence_forwarding();
        if let Some(close_fn) = self.close_fn.get_mut().unwrap().take() {
            close_fn();
        }
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        // In-flight `Oplog` calls borrow `self`, so at this point no caller can be awaiting a
        // job reply anymore. Requesting a graceful stop (rather than aborting the actor outright)
        // lets it drain whatever is already queued ahead of `Close` - including a stray `Tick`
        // the timer enqueued in the instant before it was aborted - before its background monitor
        // tasks are joined and it exits.
        let _ = self.jobs.send(ForwardingJob::Close);
    }
}

#[async_trait]
impl Oplog for ForwardingOplog {
    fn executor_shutdown_handle(&self) -> super::super::OplogShutdownHandle {
        let forwarding_retired = self.forwarding_retired.clone();
        let inner = self.inner.executor_shutdown_handle();
        let close_forwarding_retired = forwarding_retired.clone();
        let close_inner = inner.clone();
        let jobs = self.jobs.clone();
        let closed = self.closed();
        super::super::OplogShutdownHandle::new(
            move || {
                inner.fence();
                forwarding_retired.cancel();
            },
            move || {
                let forwarding_retired = close_forwarding_retired.clone();
                let jobs = jobs.clone();
                let closed = closed.clone();
                let inner = close_inner.clone();
                async move {
                    forwarding_retired.cancel();
                    let _ = jobs.send(ForwardingJob::Close);
                    let local = closed.await;
                    local.and(inner.close_and_wait().await)
                }
                .boxed()
            },
        )
    }

    fn retire(&self) {
        self.fence_forwarding();
        if let Some(timer) = &self.timer {
            timer.abort();
        }
        if !self.retired.swap(true, Ordering::AcqRel) {
            let _ = self.jobs.send(ForwardingJob::Close);
        }
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire) || self.jobs.is_closed() || self.inner.is_retired()
    }

    fn closed(&self) -> OplogCloseCompletion {
        self.closed.clone()
    }

    fn task_owner(&self) -> Option<&super::super::WorkerTasks> {
        self.inner.task_owner()
    }

    async fn stop_and_wait(&self) -> Result<(), String> {
        let tasks_result = if let Some(tasks) = self.task_owner() {
            tasks.stop_and_wait().await
        } else {
            Ok(())
        };
        self.retire();
        let result = self.closed().await;
        let inner_result = self.inner.stop_and_wait().await;
        tasks_result.and(result).and(inner_result)
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(ForwardingJob::Add { entry, done }).is_err() {
            panic!("Forwarding oplog actor terminated unexpectedly");
        }
        Box::pin(async move {
            done_rx.await.unwrap_or_else(|_| {
                panic!("Forwarding oplog actor dropped an add request without replying")
            })
        })
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        self.run_job(|done| ForwardingJob::AddDurableStreamBatch { make_batch, done })
            .await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.inner.drop_prefix(last_dropped_id).await
    }

    async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, String> {
        self.inner.try_drop_prefix(last_dropped_id).await
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        self.run_job(|done| ForwardingJob::Commit { level, done })
            .await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn try_current_oplog_index(&self) -> Result<OplogIndex, String> {
        self.inner.try_current_oplog_index().await
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> super::super::RawDurableStreamSessionStatus {
        self.inner
            .raw_durable_stream_session_status(session_key)
            .await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.inner.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<bool, OplogError> {
        self.inner.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> Result<OplogEntry, UnreadableOplog> {
        self.inner.read(oplog_index).await
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.inner.read_exact(oplog_index, n).await
    }

    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.inner.read_source(oplog_index, n).await
    }

    async fn try_read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        self.inner.try_read_source(oplog_index, n).await
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        self.inner.length().await
    }

    async fn try_length(&self) -> Result<u64, String> {
        self.inner.try_length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner.download_raw_payload(payload_id, md5_hash).await
    }

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.inner
            .download_raw_payload_classified(payload_id, md5_hash)
            .await
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> super::super::OplogAddPairReceipt {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self
            .jobs
            .send(ForwardingJob::AddPair {
                start,
                make_second,
                done,
            })
            .is_err()
        {
            panic!("Forwarding oplog actor terminated unexpectedly");
        }
        Box::pin(async move {
            done_rx.await.unwrap_or_else(|_| {
                panic!("Forwarding oplog actor dropped an add-pair request without replying")
            })
        })
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.run_job(|done| ForwardingJob::AddStart {
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
        self.run_job(|done| ForwardingJob::AddIndexedStart {
            build_request,
            done,
        })
        .await
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}
