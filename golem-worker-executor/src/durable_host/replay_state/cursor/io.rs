use super::*;

impl ReplayCursor {
    /// Replaces the seen-log multiset and updates the `has_seen_logs` fast-path flag.
    pub(in crate::durable_host::replay_state) fn set_log_hashes(
        &self,
        logs: HashMap<(u64, u64), usize>,
    ) {
        let has_logs = !logs.is_empty();
        *self.log_hashes.lock().unwrap() = logs;
        self.position
            .has_seen_logs
            .store(has_logs, Ordering::Relaxed);
    }

    /// Begins a cursor-advance transaction by acquiring the advance gate and [`Self::state`].
    /// The returned [`CursorTx`] is the sole gateway to advance the physical cursor.
    pub(in crate::durable_host::replay_state) async fn tx(
        &self,
    ) -> Result<CursorTx<'_>, WorkerExecutorError> {
        let advance_gate = self.advance_gate.clone().lock_owned().await;
        self.tx_with_gate(advance_gate).await
    }

    pub(in crate::durable_host::replay_state) async fn tx_with_gate(
        &self,
        advance_gate: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<CursorTx<'_>, WorkerExecutorError> {
        if let Some(failure) = self.delivery_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        Ok(CursorTx {
            cursor: self,
            st: self.state.lock().await,
            advance_gate: Some(advance_gate),
            blocked_on_completion_delivery: false,
            resolved_retained_claim: None,
            retaining_for: None,
            notify_progress: false,
        })
    }

    /// Releases a finished transaction and, if it made progress (advanced the cursor, registered an
    /// awaiter, or switched to live), wakes awaiters parked on cursor progress. The wakeup happens
    /// *after* the lock is released, so a woken awaiter does not immediately contend on the lock it
    /// is about to take.
    pub(in crate::durable_host::replay_state) fn finish_tx(&self, tx: CursorTx<'_>) {
        let notify = tx.notify_progress;
        drop(tx);
        if notify {
            self.progress.notify_waiters();
        }
    }

    pub(in crate::durable_host::replay_state) fn last_replayed_index(&self) -> OplogIndex {
        self.position.last_replayed_index.get()
    }

    pub(in crate::durable_host::replay_state) fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.position.last_replayed_non_hint_index.get()
    }

    pub(in crate::durable_host::replay_state) fn replay_target(&self) -> OplogIndex {
        self.replay_target.get()
    }

    pub(in crate::durable_host::replay_state) fn is_live(&self) -> bool {
        self.last_replayed_index() == self.replay_target()
    }

    pub(in crate::durable_host::replay_state) fn is_replay(&self) -> bool {
        !self.is_live()
    }

    pub(in crate::durable_host::replay_state) async fn read_oplog(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, UnreadableOplog> {
        Ok(self.oplog.read_exact(idx, n).await?.into_iter().collect())
    }

    pub(in crate::durable_host::replay_state) fn hash_log_entry(
        level: LogLevel,
        context: &str,
        message: &str,
    ) -> (u64, u64) {
        let mut hasher = MetroHash128::new();
        hasher.write_u8(level as u8);
        hasher.write(context.as_bytes());
        hasher.write(message.as_bytes());
        hasher.finish128()
    }

    /// Forward-scans the oplog from `start` up to, but not including, `end`, skipping entries
    /// inside deleted regions and running `end_check`/`for_all_intermediate` (and `update_state`)
    /// over the rest. This is the shared core for replay scans that need to inspect entries without
    /// advancing the cursor.
    ///
    /// It only reads the oplog (via [`Self::read_oplog`]); it never touches [`Self::state`], so it is
    /// safe to call both from inside a held [`CursorTx`] (passing a borrow of the transaction's skip
    /// state) and from outside it (passing a snapshot taken under a brief lock). This split is what
    /// removes the old self-deadlock hazard of a scan that needed the cursor lock while the cursor
    /// lock was already held.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::durable_host::replay_state) async fn scan_oplog(
        &self,
        mut start: OplogIndex,
        end: OplogIndex,
        skipped_regions: &DeletedRegions,
        mut current_next_skip_region: Option<OplogRegion>,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex, OplogIndex) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex, OplogIndex) -> bool,
    ) -> Result<OplogEntryLookupResult, UnreadableOplog> {
        const CHUNK_SIZE: u64 = 1024;

        let mut violation = false;

        while start < end {
            let available = end.as_u64() - start.as_u64();
            let entries = self.read_oplog(start, CHUNK_SIZE.min(available)).await?;
            for (idx, entry) in &entries {
                if current_next_skip_region
                    .as_ref()
                    .map(|r| r.contains(*idx))
                    .unwrap_or(false)
                {
                    // If we are in the current skip region, ignore the entry; when this is the last
                    // entry of the region, look up the next region so later deleted regions are
                    // skipped too.
                    if current_next_skip_region
                        .as_ref()
                        .map(|r| &r.end == idx)
                        .unwrap_or(false)
                    {
                        current_next_skip_region =
                            skipped_regions.find_next_deleted_region(idx.next());
                    }
                    continue;
                }

                if end_check(entry, begin_idx, *idx) {
                    return Ok(OplogEntryLookupResult::Found {
                        index: *idx,
                        entry: Box::new(entry.clone()),
                        violates_for_all: violation,
                    });
                }

                if !for_all_intermediate(entry, begin_idx, *idx) {
                    violation = true;
                }
            }
            start = entries.last().unwrap().0.next();
        }

        Ok(OplogEntryLookupResult::NotFound {
            violates_for_all: violation,
        })
    }
}

impl CursorTx<'_> {
    /// Reads the next oplog entry (the one right after the committed cursor) **without** advancing
    /// the published cursor and **without** applying any replay side effects. This is the
    /// *speculative* read: the caller either commits it (via [`Self::commit_consumed_entry`] / the
    /// skip path, which publish the advance and apply side effects) or discards it. Because nothing
    /// is published, a discarded read leaves no globally observable state behind — other tasks never
    /// see a transient cursor position or a half-applied side effect. This is what the concurrent
    /// cursor relies on, since a speculative read whose predicate fails (parking) is a normal path.
    ///
    /// Returns the index it read and the entry. Returns an error (rather than panicking) if the
    /// expected entry is missing, so the caller propagates a non-retriable trap instead of crashing
    /// the executor process.
    pub(in crate::durable_host::replay_state) async fn raw_read_next_oplog_entry(
        &mut self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        let read_idx = self.cursor.last_replayed_index().next();

        while self
            .st
            .replay_buffer
            .front()
            .is_some_and(|(idx, _)| *idx < read_idx)
        {
            self.st.replay_buffer.pop_front();
        }
        if self
            .st
            .replay_buffer
            .front()
            .is_some_and(|(idx, _)| *idx > read_idx)
        {
            self.st.replay_buffer.clear();
        }
        if self.st.replay_buffer.is_empty() {
            let remaining = u64::from(self.cursor.replay_target())
                .saturating_sub(u64::from(read_idx))
                .saturating_add(1);
            self.st.replay_buffer = self
                .cursor
                .read_oplog(read_idx, remaining.min(CHUNK_SIZE))
                .await?
                .into_iter()
                .collect();
        }

        let oplog_entry = if let Some((idx, oplog_entry)) = self.st.replay_buffer.pop_front()
            && idx == read_idx
        {
            oplog_entry
        } else {
            // Use `unexpected_oplog_entry` so the typing survives the wasmtime
            // round-trip and `TrapType::from_error` classifies it as a
            // non-retriable internal error rather than a policy-retriable
            // `Runtime`/`Unknown` failure (retrying replay against the same
            // truncated oplog would just fail again).
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "next oplog entry to replay",
                format!(
                    "missing oplog entry for {} at index {}; replay target = {}, last replayed non-hint index = {}",
                    self.cursor.owned_agent_id,
                    read_idx,
                    self.cursor.replay_target(),
                    self.cursor.last_replayed_non_hint_index()
                ),
            ));
        };

        Ok((read_idx, oplog_entry))
    }
}

impl ReplayState {
    /// Runs `op` inside a cursor transaction: acquires the cursor lock via [`ReplayCursor::tx`],
    /// awaits the operation, and always finishes the transaction via [`ReplayCursor::finish_tx`]
    /// (publishing the cursor position and waking parked awaiters) before returning the
    /// operation's result — including when the operation returns an error, since a failed
    /// operation may still have made cursor progress (e.g. auto-drained awaited terminals) that
    /// parked awaiters must observe.
    ///
    /// This wraps only the transaction lifecycle. It is *not* accessor-safe by itself: callers
    /// running inside Wasmtime accessor futures must reach it through
    /// [`Self::run_owned_cursor_op`] so they never queue on the fair cursor mutex directly.
    pub(in crate::durable_host::replay_state) async fn with_tx<R>(
        &self,
        op: impl AsyncFnOnce(&mut CursorTx<'_>) -> Result<R, WorkerExecutorError>,
    ) -> Result<R, WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await?;
        let result = op(&mut tx).await;
        cursor.finish_tx(tx);
        result
    }

    /// Enqueue exactly one advance-gate waiter before inspecting retained history. A retained
    /// miss keeps that queue position while progress wakes the state-only recheck; it is never
    /// evidence of divergence or permission to execute live.
    pub(in crate::durable_host::replay_state) async fn with_claim_tx_admission<R>(
        &self,
        claim: &StartClaim,
        admission: Option<&AdmissionRefusal>,
        on_retained: impl FnOnce(ReplayCallHandle, Box<OplogEntry>) -> R,
        gated: impl AsyncFnOnce(&mut CursorTx<'_>) -> Result<R, WorkerExecutorError>,
    ) -> Result<R, WorkerExecutorError> {
        let mut on_retained = Some(on_retained);
        let gate = self.cursor.advance_gate.clone().lock_owned();
        tokio::pin!(gate);
        let gate = loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if let std::task::Poll::Ready(gate) = futures::poll!(gate.as_mut()) {
                break gate;
            }
            let mut st = self.cursor.state.lock().await;
            if let Some(failure) = self.cursor.delivery_failure.lock().unwrap().clone() {
                return Err(failure);
            }
            if let Some((operation, _)) = admission
                && let Some(refusal) = st
                    .disposal_admission_refusal(&self.cursor, operation)
                    .await?
            {
                drop(st);
                let error = operation.reproduce_admission_refusal(refusal).await;
                self.cursor.progress.notify_waiters();
                return Err(error);
            }
            if let Some((refusal, terminal_index)) = matching_admission_refusal(admission, claim)?
                && !st
                    .has_unclaimed_start_before(&self.cursor, claim, terminal_index)
                    .await?
            {
                drop(st);
                let error = admission
                    .unwrap()
                    .0
                    .reproduce_admission_refusal(refusal)
                    .await;
                self.cursor.progress.notify_waiters();
                return Err(error);
            }
            let mut notify_progress = false;
            let mut resolved_retained_claim = None;
            let mut retained = RetainedClaims {
                cursor: &self.cursor,
                st: &mut st,
                resolved_retained_claim: &mut resolved_retained_claim,
                notify_progress: &mut notify_progress,
            };
            let result = retained.claim_start(claim).await;
            let result = result.map(|result| {
                result.map(|(handle, entry)| on_retained.take().unwrap()(handle, entry))
            });
            drop(st);
            if notify_progress {
                self.cursor.progress.notify_waiters();
            }
            if let Some(result) = result? {
                return Ok(result);
            }
            #[cfg(test)]
            if let Some(missed) = self.cursor.retained_claim_miss.lock().unwrap().take() {
                let _ = missed.send(());
            }
            tokio::select! {
                biased;
                gate = gate.as_mut() => break gate,
                _ = progress.as_mut() => {}
            }
        };
        let mut tx = self.cursor.tx_with_gate(gate).await?;
        if let Some((operation, _)) = admission
            && let Some(refusal) = tx
                .st
                .disposal_admission_refusal(&self.cursor, operation)
                .await?
        {
            self.cursor.finish_tx(tx);
            let error = operation.reproduce_admission_refusal(refusal).await;
            self.cursor.progress.notify_waiters();
            return Err(error);
        }
        if let Some((refusal, terminal_index)) = matching_admission_refusal(admission, claim)?
            && !tx
                .st
                .has_unclaimed_start_before(&self.cursor, claim, terminal_index)
                .await?
        {
            self.cursor.finish_tx(tx);
            let error = admission
                .unwrap()
                .0
                .reproduce_admission_refusal(refusal)
                .await;
            self.cursor.progress.notify_waiters();
            return Err(error);
        }
        let result = gated(&mut tx).await;
        self.cursor.finish_tx(tx);
        result
    }

    /// Runs a finite cursor operation on an independently-scheduled owned task and awaits its
    /// completion.
    ///
    /// Wasmtime accessor futures are polled by the component event loop, which a concurrent p2
    /// `&mut self` host call blocks for its whole duration (it holds exclusive store access). The
    /// cursor mutex is fair: releasing it hands ownership to the *queued* waiter at the front, so
    /// if a store-polled accessor future is queued on it — not just holding it — the lock can be
    /// granted to a future that will not be polled again until the event loop resumes, while the
    /// p2 host call blocking the event loop waits behind it on the same mutex: mutual starvation.
    /// Every cursor-lock interaction reachable from an accessor future therefore runs through this
    /// helper: the spawned task owns a `ReplayState` clone and all operation inputs, acquires and
    /// releases the cursor lock internally on the runtime's own scheduler, and always runs to
    /// completion — the `JoinHandle` is awaited but never aborted, so cancelling the awaiting
    /// accessor future cannot abandon a lock-owning transaction mid-flight.
    ///
    /// Task panics are resumed on the awaiting task (same observable behavior as running the
    /// operation inline); a join error without a panic payload (runtime shutdown) is reported as
    /// a runtime error.
    pub(in crate::durable_host::replay_state) async fn run_owned_cursor_op<R, Fut>(
        &self,
        op: impl FnOnce(ReplayState) -> Fut,
    ) -> Result<R, WorkerExecutorError>
    where
        Fut: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
        R: Send + 'static,
    {
        match tokio::spawn(op(self.clone())).await {
            Ok(result) => result,
            Err(join_error) => match join_error.try_into_panic() {
                Ok(panic_payload) => std::panic::resume_unwind(panic_payload),
                Err(join_error) => Err(WorkerExecutorError::runtime(format!(
                    "owned cursor operation task for {} was cancelled: {join_error}",
                    self.cursor.owned_agent_id
                ))),
            },
        }
    }

    /// Waits for every owned cursor operation queued before this call to release the cursor.
    ///
    /// An accessor future may be cancelled after spawning an owned operation but before awaiting
    /// its result. A store-owned cleanup task can use this fence to remain pending until that
    /// operation finishes, so Wasmtime does not observe an externally-held cursor with no host
    /// future left to drive.
    pub(in crate::durable_host) async fn fence_owned_cursor_ops(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move { state.with_tx(async |_| Ok(())).await })
            .await
    }

    pub fn last_replayed_index(&self) -> OplogIndex {
        self.cursor.last_replayed_index()
    }

    pub fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.cursor.last_replayed_non_hint_index()
    }

    pub fn replay_target(&self) -> OplogIndex {
        self.cursor.replay_target()
    }
}
