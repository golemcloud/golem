use super::claims::{RequestClaimIdentity, StartClaim, recorded_request_payload_matches};
use super::*;
#[cfg(feature = "test-utils")]
use std::pin::Pin;

impl ReplayCursor {
    fn begin_settling(&self) {
        let _ = self.transition_phase.compare_exchange(
            ReplayTransitionPhase::Replaying as u8,
            ReplayTransitionPhase::Settling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn publish_live(&self) {
        self.transition_phase
            .store(ReplayTransitionPhase::Live as u8, Ordering::Release);
        self.progress.notify_waiters();
    }

    fn is_live_published(&self) -> bool {
        self.transition_phase.load(Ordering::Acquire) == ReplayTransitionPhase::Live as u8
    }

    /// Replaces the seen-log multiset and updates the `has_seen_logs` fast-path flag.
    pub(super) fn set_log_hashes(&self, logs: HashMap<(u64, u64), usize>) {
        let has_logs = !logs.is_empty();
        *self.log_hashes.lock().unwrap() = logs;
        self.position
            .has_seen_logs
            .store(has_logs, Ordering::Relaxed);
    }

    /// Begins a cursor-advance transaction by acquiring [`Self::state`]. The returned [`CursorTx`]
    /// is the sole gateway to advance the cursor or mutate the guarded state.
    pub(super) async fn tx(&self) -> Result<CursorTx<'_>, WorkerExecutorError> {
        let advance_gate = self.advance_gate.clone().lock_owned().await;
        if let Some(failure) = self.delivery_failure.lock().unwrap().clone() {
            return Err(WorkerExecutorError::runtime(failure));
        }
        Ok(CursorTx {
            cursor: self,
            st: self.state.lock().await,
            advance_gate: Some(advance_gate),
            blocked_on_completion_delivery: false,
            notify_progress: false,
        })
    }

    /// Releases a finished transaction and, if it made progress (advanced the cursor, registered an
    /// awaiter, or switched to live), wakes awaiters parked on cursor progress. The wakeup happens
    /// *after* the lock is released, so a woken awaiter does not immediately contend on the lock it
    /// is about to take.
    pub(super) fn finish_tx(&self, tx: CursorTx<'_>) {
        let notify = tx.notify_progress;
        drop(tx);
        if notify {
            self.progress.notify_waiters();
        }
    }

    pub(super) fn last_replayed_index(&self) -> OplogIndex {
        self.position.last_replayed_index.get()
    }

    pub(super) fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.position.last_replayed_non_hint_index.get()
    }

    pub(super) fn replay_target(&self) -> OplogIndex {
        self.replay_target.get()
    }

    pub(super) fn is_live(&self) -> bool {
        self.last_replayed_index() == self.replay_target()
    }

    pub(super) fn is_replay(&self) -> bool {
        !self.is_live()
    }

    pub(super) async fn read_oplog(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> Vec<(OplogIndex, OplogEntry)> {
        self.oplog.read_exact(idx, n).await.into_iter().collect()
    }

    pub(super) fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
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
    pub(super) async fn scan_oplog<State>(
        &self,
        mut start: OplogIndex,
        end: OplogIndex,
        skipped_regions: &DeletedRegions,
        mut current_next_skip_region: Option<OplogRegion>,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        mut state: State,
        mut update_state: impl FnMut(&OplogEntry, OplogIndex, &mut State),
    ) -> OplogEntryLookupResult {
        const CHUNK_SIZE: u64 = 1024;

        let mut violation = false;

        while start < end {
            let available = end.as_u64() - start.as_u64();
            let entries = self.read_oplog(start, CHUNK_SIZE.min(available)).await;
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

                update_state(entry, *idx, &mut state);

                if end_check(entry, begin_idx, &state) {
                    return OplogEntryLookupResult::Found {
                        index: *idx,
                        entry: Box::new(entry.clone()),
                        violates_for_all: violation,
                    };
                }

                if !for_all_intermediate(entry, begin_idx, &state) {
                    violation = true;
                }
            }
            start = entries.last().unwrap().0.next();
        }

        OplogEntryLookupResult::NotFound {
            violates_for_all: violation,
        }
    }
}

/// An in-progress cursor-advance transaction. Holds [`ReplayCursor::state`] for its whole lifetime
/// and is the only type permitted to publish the cursor position. Its methods may `await` oplog
/// reads / payload downloads while the lock is held (exactly as the old marker lock did), but they
/// never `await` a resolver receiver and never call a `ReplayState` method that re-acquires the
/// lock. It accumulates whether cursor progress should be signalled; the public entry point notifies
/// (via [`ReplayCursor::finish_tx`]) after the guard is dropped.
pub(super) struct CursorTx<'a> {
    pub(super) cursor: &'a ReplayCursor,
    pub(super) st: MutexGuard<'a, CursorState>,
    /// Normally released with the transaction. Consuming a `CompletionDelivered` marker transfers
    /// it to the matching [`ReplayDeliveryBarrier`] so no later transaction can advance first.
    advance_gate: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Set when this transaction's positional read parked at a `CompletionDelivered` marker.
    /// Optional readers use it to distinguish that global barrier from an ordinary predicate
    /// mismatch, which may be returned to their caller immediately.
    pub(super) blocked_on_completion_delivery: bool,
    notify_progress: bool,
}

impl CursorTx<'_> {
    pub(super) fn register_custom_subtree_root(&mut self, root: OplogIndex) {
        self.st.custom_subtrees.insert(root, HashSet::from([root]));
    }

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
    pub(super) async fn raw_read_next_oplog_entry(
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
                .await
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

    /// The single cursor transaction body.
    ///
    /// Before evaluating the caller's `condition`, it **auto-drains** any *awaited terminals* at the
    /// cursor head: `End`/`Cancelled` entries whose `start_index` currently has a registered
    /// resolver awaiter. Each is committed and routed back to its awaiter (via
    /// [`Self::on_committed_replay_entry`]), then the loop continues. This is what makes concurrent
    /// replay correct: a positional reader (a scope/marker consumer, or another call's claim) never
    /// steals a host call's terminal that belongs to a different, concurrently-replaying call — it
    /// drains those to their owners first and only then looks at the next non-terminal entry.
    /// *Orphan terminals* — `End`/`Cancelled` whose `Start` lies inside a skipped/deleted region —
    /// are likewise auto-drained (consumed without an awaiter), see [`Self::is_orphan_terminal`].
    ///
    /// On the first non-drainable entry (a non-terminal, or an `End`/`Cancelled` nobody awaits):
    /// - if `condition` matches, it is committed and returned;
    /// - otherwise `None` is returned. The speculative read advanced nothing observable (the cursor
    ///   is published only on commit), so there is nothing to roll back. The auto-drained terminals
    ///   stay committed — that is the correct contract under concurrent replay: draining another
    ///   call's terminal is real progress even when this caller's own predicate then fails.
    pub(super) async fn try_get_oplog_entry(
        &mut self,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.try_get_oplog_entry_inner(None, None, condition).await
    }

    /// Consumes exactly the `CompletionDelivered` marker owned by `start_index`. Ordinary cursor
    /// readers always park at these markers; only the matching replay delivery token may commit
    /// one and take ownership of this transaction's global advance gate.
    pub(super) async fn consume_completion_delivered(
        &mut self,
        start_index: OplogIndex,
        marker_index: OplogIndex,
    ) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, WorkerExecutorError> {
        let consumed = self
            .try_get_oplog_entry_inner(Some((start_index, marker_index)), None, |_| false)
            .await?;
        if consumed.is_some() {
            Ok(Some(
                self.advance_gate
                    .take()
                    .expect("cursor transactions always own the advance gate"),
            ))
        } else {
            Ok(None)
        }
    }

    /// [`Self::try_get_oplog_entry`] with the invocation-boundary tolerance for live-only
    /// abandoned durable-call records enabled: never-claimed `Start`s (and the `End`/`Cancelled`
    /// terminals closing them) are drained into `abandoned` instead of being handed to the
    /// positional reader. Only the agent-invocation-finished reader uses this — see
    /// [`AbandonedStarts`] for why the tolerance is sound there and nowhere else.
    pub(super) async fn try_get_oplog_entry_at_invocation_boundary(
        &mut self,
        abandoned: &mut AbandonedStarts,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.try_get_oplog_entry_inner(None, Some(abandoned), condition)
            .await
    }

    pub(super) async fn try_get_oplog_entry_inner(
        &mut self,
        expected_delivery: Option<(OplogIndex, OplogIndex)>,
        mut abandoned: Option<&mut AbandonedStarts>,
        mut condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.blocked_on_completion_delivery = false;
        if self.st.skip_hints_after_delivery {
            self.skip_forward().await?;
            self.st.skip_hints_after_delivery = false;
        }
        loop {
            if self.cursor.is_live() {
                // No further entries to read: nothing to drain, condition cannot match.
                return Ok(None);
            }

            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;

            if let OplogEntry::CompletionDelivered { start_index, .. } = &entry {
                if expected_delivery == Some((*start_index, read_idx)) {
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    return Ok(Some((read_idx, entry)));
                }
                if self.is_custom_subtree_descendant(*start_index) {
                    // A completed custom invocation replays as one logical result, while an
                    // incomplete one re-executes its whole body. Either way, no replay delivery
                    // token exists for a physical call in its observational subtree, so its
                    // marker is replay-inert rather than a guest-delivery boundary.
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    self.st.skip_hints_after_delivery = false;
                    self.skip_forward().await?;
                    continue;
                }
                if self.st.skipped_regions.is_in_deleted_region(*start_index) {
                    // The call belongs to an abandoned timeline, so no replay delivery token can
                    // exist for its surviving marker. Consume it like an orphan terminal and keep
                    // normal hint skipping enabled: there is no guest boundary to hold here.
                    debug!(
                        "Skipping orphan CompletionDelivered at {read_idx} whose Start {start_index} lies in a skipped region"
                    );
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    self.st.skip_hints_after_delivery = false;
                    self.skip_forward().await?;
                    continue;
                }
                if abandoned
                    .as_deref()
                    .is_some_and(|abandoned| abandoned.contains(*start_index))
                {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "AgentInvocationFinished",
                        format!(
                            "CompletionDelivered at {read_idx} references unclaimed durable call Start at {start_index} — the recorded guest received this completion but replay reached the invocation boundary without claiming it"
                        ),
                    ));
                }

                // This is a reserved guest-delivery boundary. Leave it at the cursor head for the
                // matching completion token; even an unconditional positional reader may not
                // steal it or advance beyond it.
                self.st.replay_buffer.push_front((read_idx, entry));
                self.blocked_on_completion_delivery = true;
                return Ok(None);
            }

            if self.is_awaited_terminal(read_idx, &entry) {
                // An `End`/`Cancelled` owned by a concurrently-replaying call: commit it and hand it
                // back to its awaiter, then keep draining. Never returned to this caller.
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if self.is_orphan_terminal(&entry) {
                // An `End`/`Cancelled` whose `Start` lies inside a skipped/deleted region (a
                // jump/revert/fork/snapshot cut between a `Start` and its terminal): nobody can
                // ever claim or await it, so consume it here and keep draining instead of handing
                // it to a positional reader as an unexpected entry.
                debug!(
                    "Skipping orphan terminal at {read_idx} whose Start lies in a skipped region"
                );
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if self.st.claimed_starts.contains(&read_idx) {
                // A `Start` already claimed out-of-position by an identity-keyed scan-ahead claim
                // (`claim_owned_start`): its owner registered a resolver awaiter at claim time, so
                // just consume it here and keep draining — it must never be handed to a positional
                // reader.
                self.st.claimed_starts.remove(&read_idx);
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if let OplogEntry::Start {
                observational_owner: Some(owner),
                ..
            } = &entry
                && let Some(root) = self.custom_subtree_root(*owner)
            {
                // The owning custom invocation replays as one logical operation, so its physical
                // calls are observational records only: consume their Starts without registering
                // resolver awaiters or waiting for terminals.
                self.commit_consumed_entry(read_idx, &entry).await?;
                self.st
                    .custom_subtrees
                    .get_mut(&root)
                    .expect("custom subtree root disappeared while consuming an observational call")
                    .insert(read_idx);
                continue;
            }

            if let OplogEntry::Start {
                parent_start_index: Some(parent_start_index),
                ..
            } = &entry
                && let Some(root) = self.custom_subtree_root(*parent_start_index)
            {
                // A custom invocation returns its persisted root result without running its body.
                // Nested calls, scopes, and custom invocations therefore belong to the same replay-
                // inert tree through their ordinary parent identity, irrespective of interleaving.
                self.commit_consumed_entry(read_idx, &entry).await?;
                self.st
                    .custom_subtrees
                    .get_mut(&root)
                    .expect("custom subtree root disappeared while consuming a descendant")
                    .insert(read_idx);
                continue;
            }

            if terminal_start_index(&entry)
                .is_some_and(|start_index| self.is_custom_subtree_descendant(start_index))
            {
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if let Some(abandoned) = abandoned.as_deref_mut() {
                // Invocation-boundary tolerance: any `Start` still unconsumed here can never be
                // claimed anymore (the replayed guest already produced its invocation result), so
                // it is live-only abandoned progress — drain it and its terminal instead of
                // failing the positional reader. Terminals of starts *not* tracked as abandoned
                // stay fatal below.
                match &entry {
                    OplogEntry::Start {
                        function_name,
                        parent_start_index,
                        ..
                    } => {
                        // Reject before committing: a replay-side-effecting Start must not fire
                        // its commit effects from the drain (see `AbandonedStarts::can_drain`).
                        if !AbandonedStarts::can_drain(function_name) {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                "AgentInvocationFinished",
                                format!(
                                    "unclaimed {function_name:?} Start at {read_idx} — a \
                                     replay-side-effecting record cannot be tolerated as \
                                     abandoned at the invocation boundary"
                                ),
                            ));
                        }
                        abandoned.record_start(
                            read_idx,
                            function_name.clone(),
                            *parent_start_index,
                        );
                        self.commit_consumed_entry(read_idx, &entry).await?;
                        continue;
                    }
                    OplogEntry::End { start_index, .. } if abandoned.contains(*start_index) => {
                        abandoned.record_terminal(*start_index, read_idx, "End")?;
                        self.commit_consumed_entry(read_idx, &entry).await?;
                        continue;
                    }
                    OplogEntry::Cancelled { start_index, .. }
                        if abandoned.contains(*start_index) =>
                    {
                        abandoned.record_terminal(*start_index, read_idx, "Cancelled")?;
                        self.commit_consumed_entry(read_idx, &entry).await?;
                        continue;
                    }
                    _ => {}
                }
            }

            if condition(&entry) {
                self.commit_consumed_entry(read_idx, &entry).await?;
                return Ok(Some((read_idx, entry)));
            } else {
                // Predicate failed: the speculative read published nothing, so the cursor,
                // skipped-region state, and side effects are already untouched.
                self.st.replay_buffer.push_front((read_idx, entry));
                return Ok(None);
            }
        }
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` currently has a registered
    /// resolver awaiter (and is therefore an *awaited terminal* the cursor auto-drains to its owner
    /// rather than handing to a positional reader).
    pub(super) fn is_awaited_terminal(&self, terminal_idx: OplogIndex, entry: &OplogEntry) -> bool {
        terminal_start_index(entry).is_some_and(|start_index| {
            self.st
                .concurrent_resolver
                .owns_terminal(start_index, terminal_idx)
        })
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` lies inside a skipped/deleted
    /// region. Such an *orphan terminal* is left behind when a jump/revert/fork/snapshot deletes
    /// the region containing a call's `Start` but not its terminal. Its `Start` can never be
    /// claimed (both the positional head consume and the scan-ahead claim jump over deleted
    /// regions), so no awaiter can ever exist for it; the cursor consumes it like a no-op instead
    /// of surfacing it to a positional reader as an unexpected entry.
    pub(super) fn is_orphan_terminal(&self, entry: &OplogEntry) -> bool {
        terminal_start_index(entry)
            .is_some_and(|start_index| self.st.skipped_regions.is_in_deleted_region(start_index))
    }

    fn custom_subtree_root(&self, member: OplogIndex) -> Option<OplogIndex> {
        self.st
            .custom_subtrees
            .iter()
            .find_map(|(root, members)| members.contains(&member).then_some(*root))
    }

    fn is_custom_subtree_descendant(&self, member: OplogIndex) -> bool {
        self.custom_subtree_root(member)
            .is_some_and(|root| root != member)
    }

    /// Commits a just-read entry: apply its commit-only side effects, publish the cursor advance,
    /// skip any trailing hint entries, advance the non-hint marker, route it to the concurrent
    /// resolver, and mark that cursor progress should be signalled once the lock is released.
    pub(super) async fn commit_consumed_entry(
        &mut self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        // Apply the fallible commit-only side effects *before* publishing the cursor advance, so a
        // failure (e.g. a corrupt `GolemApiFork` payload) cannot leave the cursor advanced while
        // resolver routing / progress signalling below never run — a partial-publish on the error
        // path. None of these effects depend on the cursor position.
        self.apply_commit_effects(read_idx, entry).await?;
        // Publish the cursor advance now (and only now): committing is the single point where the
        // speculative read of `read_idx` becomes globally observable. This also performs the
        // skipped-region jump for the next read via `get_out_of_skipped_region`, and must precede
        // `skip_forward` (which reads forward from the advanced cursor).
        self.move_replay_idx(read_idx).await;
        if matches!(entry, OplogEntry::CompletionDelivered { .. }) {
            self.st.skip_hints_after_delivery = true;
        } else {
            self.skip_forward().await?;
        }
        if !entry.is_hint() {
            self.cursor
                .position
                .last_replayed_non_hint_index
                .set(read_idx);
        }
        // Committed-consume hook: this entry is now permanently consumed (speculative reads never
        // reach here — they return before committing), so it is safe to feed the concurrent replay
        // resolver.
        self.on_committed_replay_entry(read_idx, entry);
        self.notify_progress = true;
        Ok(())
    }

    /// Skips trailing hint entries following the just-committed entry, recording any log hints,
    /// then leaves the cursor on the next non-hint entry without consuming it.
    pub(super) async fn skip_forward(&mut self) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs: HashMap<(u64, u64), usize> = HashMap::new();
        while self.cursor.is_replay() {
            // Speculative peek: does not advance the published cursor. The cursor is advanced (via
            // `move_replay_idx`) only when a hint entry is actually skipped past below; the first
            // non-hint entry leaves the cursor untouched, so no speculative position is ever
            // globally observable.
            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;
            match self.should_skip_to(read_idx, &entry).await {
                Some(skip_to) => {
                    // This hint entry is being permanently consumed, so its commit-only side
                    // effects fire here (they must NOT fire on the rolled-back probe in the `None`
                    // branch below).
                    self.apply_commit_effects(read_idx, &entry).await?;

                    // Recording seen log entries
                    if let OplogEntry::Log {
                        level,
                        context,
                        message,
                        ..
                    } = &entry
                    {
                        let hash = ReplayCursor::hash_log_entry(*level, context, message);
                        *logs.entry(hash).or_insert(0) += 1;
                    }

                    // Publish the advance past this hint (also performs the skipped-region jump for
                    // the next read). Leaving last_replayed_non_hint_index unchanged, because this is
                    // a hint entry.
                    self.move_replay_idx(skip_to).await;
                }
                None => {
                    // We've found the first non-hint entry; the speculative peek advanced nothing, so
                    // the cursor and skipped-region state already point just before it.
                    break;
                }
            }
        }

        self.cursor.set_log_hashes(logs);
        Ok(())
    }

    /// Checks whether the currently read `entry` is a hint entry valid for replay, or
    /// if a new oplog index should be tried instead.
    ///
    /// For hint entries, the next tried oplog index is the next one.
    ///
    /// If the entry is a hint entry, the result is `Some` and contains the current last
    /// read index, so the next read will get the next one.
    /// If the entry is not a hint entry the result is `None`.
    pub(super) async fn should_skip_to(
        &self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Option<OplogIndex> {
        if entry.is_hint() && !matches!(entry, OplogEntry::CompletionDelivered { .. }) {
            // Advance to the hint entry itself; the caller publishes this (via `move_replay_idx`) so
            // the next read gets `read_idx.next()`.
            Some(read_idx)
        } else {
            None
        }
    }

    /// Applies the replay side effects of an entry that is being **permanently consumed** at
    /// `read_idx`. Split out of the raw read so it fires only on commit, never on a rolled-back
    /// speculative read. Called for the entry returned to a caller, and for each hint /
    /// hint entry skipped past in [`Self::skip_forward`].
    pub(super) async fn apply_commit_effects(
        &mut self,
        read_idx: OplogIndex,
        oplog_entry: &OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        // record side effects that need to be applied at the next opportunity
        if let OplogEntry::SuccessfulUpdate {
            target_revision, ..
        } = oplog_entry
        {
            self.record_replay_event(ReplayEvent::UpdateReplayed {
                new_revision: *target_revision,
            });
        }
        // The sequential adapter persists GolemApiFork as a matched
        // `Start { function_name: GolemApiFork, .. }` + `End { response: Some(..), .. }`
        // pair. On Start we remember the `Start`'s `OplogIndex`, on the matching
        // End (via `start_index`) we decode the response and emit `ForkReplayed`
        // if necessary.
        match oplog_entry {
            OplogEntry::AgentInvocationStarted {
                wallet_pin: Some(wallet_pin),
                ..
            } => {
                self.record_replay_event(ReplayEvent::InvocationWalletPinned {
                    wallet_pin: wallet_pin.clone(),
                });
            }
            OplogEntry::CardInstalled {
                card,
                wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardInstalled {
                    card: card.clone(),
                    wallet_generation: *wallet_generation,
                });
            }
            OplogEntry::CardDerived {
                card,
                wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardDerived {
                    card: card.clone(),
                    wallet_generation: *wallet_generation,
                });
            }
            OplogEntry::CardTransferStarted {
                transfer_id,
                card_id,
                source_holder,
                target_holder,
                source_wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardTransferStarted {
                    transfer_id: *transfer_id,
                    card_id: *card_id,
                    source_holder: source_holder.clone(),
                    target_holder: target_holder.clone(),
                    source_wallet_generation: *source_wallet_generation,
                });
            }
            OplogEntry::CardTransferred {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                card,
                target_wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardTransferred {
                    transfer_id: *transfer_id,
                    source_card_id: *source_card_id,
                    installed_card_id: *installed_card_id,
                    target_holder: target_holder.clone(),
                    card: card.clone(),
                    target_wallet_generation: *target_wallet_generation,
                });
            }
            OplogEntry::CardTransferConfirmed {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardTransferConfirmed {
                    transfer_id: *transfer_id,
                    source_card_id: *source_card_id,
                    installed_card_id: *installed_card_id,
                    target_holder: target_holder.clone(),
                });
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids,
                local_wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardRevokedCascade {
                    card_ids: revoked_card_ids.clone(),
                    local_wallet_generation: *local_wallet_generation,
                });
            }
            OplogEntry::CardRevoked {
                card_id,
                wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardRevoked {
                    card_id: *card_id,
                    wallet_generation: *wallet_generation,
                });
            }
            OplogEntry::CardExpired {
                card_id,
                wallet_generation,
                ..
            } => {
                self.record_replay_event(ReplayEvent::CardExpired {
                    card_id: *card_id,
                    wallet_generation: *wallet_generation,
                });
            }
            OplogEntry::Start { function_name, .. }
                if function_name == &HostFunctionName::GolemApiFork =>
            {
                self.st.pending_fork_starts.insert(read_idx);
            }
            OplogEntry::End {
                start_index,
                response: Some(response_payload),
                ..
            } => {
                let is_pending = self.st.pending_fork_starts.remove(start_index);
                if is_pending {
                    let response = self
                        .cursor
                        .oplog
                        .download_payload(response_payload.clone())
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to download GolemApiFork oplog payload at index {read_idx}: {err}"
                            ))
                        })?;
                    let result: HostResponseGolemApiFork =
                        if let HostResponse::GolemApiFork(result) = response {
                            result
                        } else {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                "HostResponse::GolemApiFork",
                                format!("{response:?}"),
                            ));
                        };
                    if result.result == Ok(ForkResult::Forked) {
                        self.record_replay_event(ReplayEvent::ForkReplayed {
                            new_phantom_id: result.forked_phantom_id,
                        });
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Advances the published cursor to `new_idx`, applying any skipped-region jump, and synthesizes
    /// a single [`ReplayEvent::ReplayFinished`] if this advance is the one that exhausts replay.
    ///
    /// This is the single chokepoint for every replay-mode position advance — direct consumption of
    /// the target entry, skipping past trailing hint entries, and jumping over a skipped region (via
    /// [`Self::get_out_of_skipped_region`]) all funnel through here. Detecting the transition here
    /// (rather than only when the *consumed* entry index equals `replay_target`) guarantees
    /// `ReplayFinished` is queued whenever the cursor reaches the target, including when it gets
    /// there via a skip/jump that never consumes the target entry. Consumers withhold the event
    /// until the primary publishes live admission after reconstruction settlement. The forced
    /// transition in [`Self::switch_to_live`] is the only other path to live and emits its own
    /// `ReplayFinished`.
    ///
    /// Exactly-once holds because the `was_replay && is_live` edge is true only on the single advance
    /// that crosses into live: once live, the replay-driving loops stop and no further
    /// `move_replay_idx` runs until the replay target is grown (`set_replay_target`) or the cursor is
    /// reset (`new` / `drop_override_and_restart`), each of which starts a fresh replay epoch that
    /// emits its own `ReplayFinished` on completion.
    pub(super) async fn move_replay_idx(&mut self, new_idx: OplogIndex) {
        let was_replay = self.cursor.is_replay();
        self.cursor.position.last_replayed_index.set(new_idx);
        self.get_out_of_skipped_region().await;
        if was_replay && self.cursor.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished);
        }
        // Publish the committed cursor position to replay-progress observers (see
        // `Oplog::on_replay_progress`). This chokepoint is only reached by committed advances —
        // speculative reads return before calling it — so observers never see a position that is
        // later rolled back.
        self.cursor
            .oplog
            .on_replay_progress(self.cursor.last_replayed_index())
            .await;
    }

    pub(super) async fn get_out_of_skipped_region(&mut self) {
        let initial_snapshot_skip_end = self.st.initial_snapshot_skip_end.take();
        // Loop: after jumping a region, the freshly looked-up next region may start immediately
        // after the jump target (adjacent regions recorded separately), requiring another jump.
        while self.cursor.is_replay() {
            match self.st.next_skipped_region.clone() {
                Some(region) if region.start == (self.cursor.last_replayed_index().next()) => {
                    let target = region.end.next(); // we want to continue reading _after_ the region
                    debug!(
                        "Worker reached skipped region at {}, jumping to {} (oplog size: {})",
                        region.start,
                        target,
                        self.cursor.replay_target()
                    );
                    self.cursor
                        .position
                        .last_replayed_index
                        .set(target.previous()); // so we set the last replayed index to the end of the region

                    let events_region = match initial_snapshot_skip_end {
                        Some(snapshot_end) if region.end <= snapshot_end => None,
                        Some(snapshot_end) => Some(OplogRegion {
                            start: region.start.max(snapshot_end.next()),
                            end: region.end,
                        }),
                        None => Some(region),
                    };
                    if let Some(events_region) = events_region {
                        self.record_card_events_in_region(&events_region).await;
                    }

                    // The lookup must start *after* the just-jumped region: `find_next_deleted_region`
                    // matches regions starting at-or-after the given index, so looking up from the
                    // region's own end would re-find a single-entry region (start == end) and leave
                    // the genuinely next region untracked.
                    let next = self
                        .st
                        .skipped_regions
                        .find_next_deleted_region(self.cursor.last_replayed_index().next());
                    self.st.next_skipped_region = next;
                }
                _ => break,
            }
        }
    }

    async fn record_card_events_in_region(&mut self, region: &OplogRegion) {
        let mut next = region.start;
        while next <= region.end {
            let remaining = region.end.as_u64() - next.as_u64() + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(remaining))
                .await;
            let last_read = *entries.last_key_value().unwrap().0;
            for entry in entries.into_values() {
                match entry {
                    OplogEntry::CardInstalled {
                        card,
                        wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardInstalled {
                        card,
                        wallet_generation,
                    }),
                    OplogEntry::CardDerived {
                        card,
                        wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardDerived {
                        card,
                        wallet_generation,
                    }),
                    OplogEntry::CardTransferStarted {
                        transfer_id,
                        card_id,
                        source_holder,
                        target_holder,
                        source_wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardTransferStarted {
                        transfer_id,
                        card_id,
                        source_holder,
                        target_holder,
                        source_wallet_generation,
                    }),
                    OplogEntry::CardTransferred {
                        transfer_id,
                        source_card_id,
                        installed_card_id,
                        target_holder,
                        card,
                        target_wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardTransferred {
                        transfer_id,
                        source_card_id,
                        installed_card_id,
                        target_holder,
                        card,
                        target_wallet_generation,
                    }),
                    OplogEntry::CardTransferConfirmed {
                        transfer_id,
                        source_card_id,
                        installed_card_id,
                        target_holder,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardTransferConfirmed {
                        transfer_id,
                        source_card_id,
                        installed_card_id,
                        target_holder,
                    }),
                    OplogEntry::CardRevokedCascade {
                        revoked_card_ids,
                        local_wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardRevokedCascade {
                        card_ids: revoked_card_ids,
                        local_wallet_generation,
                    }),
                    OplogEntry::CardRevoked {
                        card_id,
                        wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardRevoked {
                        card_id,
                        wallet_generation,
                    }),
                    OplogEntry::CardExpired {
                        card_id,
                        wallet_generation,
                        ..
                    } => self.record_replay_event(ReplayEvent::CardExpired {
                        card_id,
                        wallet_generation,
                    }),
                    _ => {}
                }
            }
            next = last_read.next();
        }
    }

    /// Feeds the concurrent replay resolver when an `End`/`Cancelled` entry is *committed*
    /// (permanently consumed). Resolves only calls that are actually being awaited
    /// (`resolve_if_pending`), so the `End`/`Cancelled` of any call not tracked by the resolver —
    /// e.g. the guest-facing manual durability pair, consumed through this same cursor but never
    /// registered — is ignored instead of leaking.
    pub(super) fn on_committed_replay_entry(&mut self, idx: OplogIndex, entry: &OplogEntry) {
        match entry {
            OplogEntry::End {
                start_index,
                response,
                forced_commit,
                ..
            } => {
                let marker = self.completion_marker(*start_index);
                let resolution = match marker {
                    Some(CompletionMarker::Discarded(marker_idx)) => {
                        Resolution::CompletedButDiscarded {
                            end_idx: idx,
                            marker_idx,
                            response: response.clone(),
                        }
                    }
                    Some(CompletionMarker::Delivered(marker_idx)) => Resolution::Completed {
                        end_idx: idx,
                        response: response.clone(),
                        forced_commit: *forced_commit,
                        delivery_marker: Some(marker_idx),
                    },
                    None => Resolution::Completed {
                        end_idx: idx,
                        response: response.clone(),
                        forced_commit: *forced_commit,
                        delivery_marker: None,
                    },
                };
                self.st
                    .concurrent_resolver
                    .resolve_if_pending(*start_index, idx, resolution);
            }
            OplogEntry::Cancelled {
                start_index,
                partial,
                ..
            } => {
                self.st.concurrent_resolver.resolve_if_pending(
                    *start_index,
                    idx,
                    Resolution::Cancelled {
                        cancelled_idx: idx,
                        partial: partial.clone(),
                    },
                );
            }
            _ => {}
        }
    }

    /// Returns the guest-delivery marker for the durable call starting at `start_index`, if one
    /// exists and lies outside any deleted region. A marker in a reverted/jumped-away region
    /// belongs to an abandoned timeline, so a still-visible `End` uses the legacy immediate
    /// delivery behavior.
    ///
    /// The `discarded_completions` map is populated only from entries at or before the replay
    /// target (the construction scan is bounded by the initial target and target growth rescans
    /// exactly the newly visible range, see [`ReplayState::set_replay_target`]), so a returned
    /// marker never encodes knowledge of oplog entries beyond the target. A target that falls
    /// *between* an `End` and its marker is an invalid replay configuration — the delivery
    /// status of that `End` is not decidable from the visible prefix — and is rejected at
    /// delivery time ([`ReplayState::await_resolution_outcome`]) as well as up front by debug
    /// target validation and cut-point (fork/revert) validation.
    pub(super) fn completion_marker(&self, start_index: OplogIndex) -> Option<CompletionMarker> {
        let marker = *self
            .cursor
            .completion_markers
            .lock()
            .unwrap()
            .get(&start_index)?;
        if !self.st.skipped_regions.is_in_deleted_region(marker.index()) {
            Some(marker)
        } else {
            None
        }
    }

    pub(super) fn record_replay_event(&mut self, event: ReplayEvent) {
        self.cursor
            .pending_replay_events
            .lock()
            .unwrap()
            .push(event);
    }

    /// Registers a resolver for a claimed `Start`. Successful calls carrying a delivery marker are
    /// resolved from a non-consuming lookahead to their `End`: this lets the host continuation run
    /// while the positional cursor remains available for durable operations recorded before the
    /// completion became guest-visible. The matching terminal remains resolver-owned and is
    /// auto-drained when those intervening operations advance the cursor to it.
    async fn register_claimed_start(
        &mut self,
        start_idx: OplogIndex,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let prefetched = if let Some(marker) = self.completion_marker(start_idx) {
            let marker_idx = marker.index();
            let scan = self
                .cursor
                .scan_oplog(
                    start_idx.next(),
                    marker_idx,
                    &self.st.skipped_regions,
                    self.st
                        .skipped_regions
                        .find_next_deleted_region(start_idx.next()),
                    OplogIndex::NONE,
                    |entry, _, index: &Option<OplogIndex>| {
                        terminal_start_index(entry) == Some(start_idx)
                            && index.is_some_and(|index| index < marker_idx)
                    },
                    |_, _, _| true,
                    None,
                    |_, index, current: &mut Option<OplogIndex>| *current = Some(index),
                )
                .await;
            match scan {
                OplogEntryLookupResult::Found { index, entry, .. } => match *entry {
                    OplogEntry::End {
                        response,
                        forced_commit,
                        ..
                    } => Some((
                        index,
                        match marker {
                            CompletionMarker::Delivered(marker_idx) => Resolution::Completed {
                                end_idx: index,
                                response,
                                delivery_marker: Some(marker_idx),
                                forced_commit,
                            },
                            CompletionMarker::Discarded(marker_idx) => {
                                Resolution::CompletedButDiscarded {
                                    end_idx: index,
                                    marker_idx,
                                    response,
                                }
                            }
                        },
                    )),
                    OplogEntry::Cancelled { .. } => {
                        return Err(WorkerExecutorError::runtime(format!(
                            "corrupt oplog: successful-completion marker at {marker_idx} references cancelled durable call Start at {start_idx}"
                        )));
                    }
                    _ => unreachable!("the prefetch scan accepts only matching terminals"),
                },
                OplogEntryLookupResult::NotFound { .. } => {
                    return Err(WorkerExecutorError::runtime(format!(
                        "corrupt oplog: successful-completion marker at {marker_idx} references durable call Start at {start_idx} without a matching End before the marker"
                    )));
                }
            }
        } else {
            None
        };

        let receiver = self.st.concurrent_resolver.register(start_idx);
        if let Some((terminal_idx, resolution)) = prefetched {
            self.st
                .concurrent_resolver
                .resolve_prefetched(start_idx, terminal_idx, resolution);
        }
        self.notify_progress = true;
        Ok(ReplayCallHandle::new(start_idx, receiver))
    }

    /// Looks for the first not-yet-claimed `Start` entry matching `matches_identity`, registering a
    /// resolver receiver keyed by the `Start`'s index and returning the registered handle together
    /// with the claimed entry. Shared core of every concurrent-replay `Start` claim.
    ///
    /// Claiming by identity rather than strict position is required because accessor host calls
    /// run concurrently: `Start` entries appended by concurrently running host tasks (sibling
    /// sends' scopes, per-chunk children of overlapping consume-body scopes, top-level calls
    /// racing with them) land in the oplog in network/scheduling order, which is not reproduced by
    /// replay — only the initiation order *within one guest task / parent chain* is. The head is
    /// consumed positionally when it already matches (the serial fast path costs nothing);
    /// otherwise the **first not-yet-claimed matching `Start`** between the cursor and the replay
    /// target is scan-ahead-claimed: its index is recorded in [`CursorState::claimed_starts`] (so
    /// the cursor auto-consumes the entry when it reaches it, like an awaited terminal, and never
    /// hands it to another reader) and the resolver awaiter is registered immediately.
    ///
    /// The `Start` consume/claim and the resolver registration happen **atomically** within this
    /// transaction (under the cursor lock). This is required for concurrent replay: if the cursor
    /// advanced past the `Start` before the awaiter was registered, this call's `End` arriving at
    /// the head in that window would not be recognised as an awaited terminal and could be wrongly
    /// consumed by a positional reader.
    ///
    /// Because a terminal always follows its `Start`, a scan-ahead-claimed call's
    /// `End`/`Cancelled` is reached only after the cursor has consumed the claimed `Start`, so
    /// terminal routing is unaffected. Matching `Start`s that share the same identity are claimed
    /// in oplog order, preserving the deterministic per-task/per-parent chain order. A replay
    /// divergence (no matching `Start` recorded at all) is reported to the caller instead of as an
    /// immediate head mismatch.
    pub(super) async fn claim_start_matching(
        &mut self,
        matches_identity: impl Fn(&OplogEntry) -> bool,
    ) -> Result<StartClaimAttempt, WorkerExecutorError> {
        // Head fast path: auto-drains awaited terminals and already-claimed `Start`s, then
        // consumes the head iff it matches this claim's identity.
        if let Some((start_idx, entry)) = self.try_get_oplog_entry(&matches_identity).await? {
            let handle = self.register_claimed_start(start_idx).await?;
            return Ok(StartClaimAttempt::Claimed(handle, Box::new(entry)));
        }
        if self.blocked_on_completion_delivery {
            return Ok(StartClaimAttempt::Blocked);
        }

        // The head belongs to someone else: scan ahead for the first not-yet-claimed matching
        // `Start`, skipping deleted regions exactly like the cursor itself would. A delivery
        // marker bounds the scan: a later `Start` must not be claimed before the recorded guest
        // handoff at that marker.
        let already_claimed = self.st.claimed_starts.clone();
        let replay_target = self.cursor.replay_target();
        let scan_result = self
            .cursor
            .scan_oplog(
                self.cursor.last_replayed_index().next(),
                replay_target.next(),
                &self.st.skipped_regions,
                self.st.next_skipped_region.clone(),
                OplogIndex::NONE,
                |entry, _begin_idx, state: &Option<OplogIndex>| {
                    state
                        .map(|idx| idx <= replay_target && !already_claimed.contains(&idx))
                        .unwrap_or(false)
                        && (matches_identity(entry)
                            || matches!(entry, OplogEntry::CompletionDelivered { .. }))
                },
                |_, _, _| true,
                None,
                |_, idx, state: &mut Option<OplogIndex>| {
                    *state = Some(idx);
                },
            )
            .await;

        match scan_result {
            OplogEntryLookupResult::Found { index, entry, .. } => {
                if matches!(entry.as_ref(), OplogEntry::CompletionDelivered { .. }) {
                    self.blocked_on_completion_delivery = true;
                    return Ok(StartClaimAttempt::Blocked);
                }
                self.st.claimed_starts.insert(index);
                let handle = self.register_claimed_start(index).await?;
                Ok(StartClaimAttempt::Claimed(handle, entry))
            }
            OplogEntryLookupResult::NotFound { .. } => Ok(StartClaimAttempt::Missing),
        }
    }

    /// Request-matching counterpart of [`Self::claim_start_matching`]. It scans identity-matching
    /// candidates in oplog order and resolves each recorded payload to a value before claiming it.
    /// Payload resolution is deliberately outside the synchronous scan predicate because an
    /// external payload may require blob I/O. A completed scan returns `Missing`; payload loading
    /// or decoding failure remains an error.
    pub(super) async fn claim_start_matching_request(
        &mut self,
        matches_identity: impl Fn(&OplogEntry) -> bool,
        expected_request: &RequestClaimIdentity,
    ) -> Result<StartClaimAttempt, WorkerExecutorError> {
        // Drain any awaited terminals at the head and detect a delivery marker before the
        // request-payload scan. The false predicate leaves an ordinary candidate untouched.
        self.try_get_oplog_entry(|_| false).await?;
        if self.blocked_on_completion_delivery {
            return Ok(StartClaimAttempt::Blocked);
        }

        let already_claimed = self.st.claimed_starts.clone();
        let mut scan_start = self.cursor.last_replayed_index().next();
        let replay_target = self.cursor.replay_target();

        while scan_start <= replay_target {
            let scan_result = self
                .cursor
                .scan_oplog(
                    scan_start,
                    replay_target.next(),
                    &self.st.skipped_regions,
                    self.st.skipped_regions.find_next_deleted_region(scan_start),
                    OplogIndex::NONE,
                    |entry, _begin_idx, state: &Option<OplogIndex>| {
                        state
                            .map(|idx| idx <= replay_target && !already_claimed.contains(&idx))
                            .unwrap_or(false)
                            && (matches_identity(entry)
                                || matches!(entry, OplogEntry::CompletionDelivered { .. }))
                    },
                    |_, _, _| true,
                    None,
                    |_, idx, state: &mut Option<OplogIndex>| {
                        *state = Some(idx);
                    },
                )
                .await;

            let OplogEntryLookupResult::Found { index, entry, .. } = scan_result else {
                break;
            };
            if matches!(entry.as_ref(), OplogEntry::CompletionDelivered { .. }) {
                self.blocked_on_completion_delivery = true;
                return Ok(StartClaimAttempt::Blocked);
            }
            let OplogEntry::Start {
                request: Some(recorded_request),
                ..
            } = entry.as_ref()
            else {
                unreachable!("the request-matching claim predicate only accepts Start entries")
            };

            let payload_matches = recorded_request_payload_matches(
                self.cursor.oplog.as_ref(),
                recorded_request,
                expected_request,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to load durable call request payload at Start {index}: {err}"
                ))
            })?;
            if payload_matches {
                self.st.claimed_starts.insert(index);
                let handle = self.register_claimed_start(index).await?;
                return Ok(StartClaimAttempt::Claimed(handle, entry));
            }

            scan_start = index.next();
        }

        Ok(StartClaimAttempt::Missing)
    }

    /// Checks whether a `Start` matching this claim belongs to a jump-deleted region. An incomplete
    /// entity Store uses this to continue live locally while sibling Stores finish replaying the
    /// surviving owner-oplog tail.
    pub(super) async fn deleted_region_contains_start(
        &self,
        claim: &StartClaim,
    ) -> Result<bool, WorkerExecutorError> {
        let replay_target = self.cursor.replay_target();
        let regions = self
            .st
            .skipped_regions
            .regions()
            .cloned()
            .collect::<Vec<_>>();
        for region in regions {
            if region.start > replay_target {
                break;
            }
            let end = region.end.min(replay_target);
            let mut next = region.start;
            while next <= end {
                let available = u64::from(end) - u64::from(next) + 1;
                let entries = self
                    .cursor
                    .oplog
                    .read_exact(next, CHUNK_SIZE.min(available))
                    .await;
                let last_read = *entries.last_key_value().unwrap().0;
                for (_, entry) in entries {
                    if !claim.matches_start_identity(&entry) {
                        continue;
                    }
                    let request_matches = match claim.matching_request() {
                        Some(expected_request) => {
                            let OplogEntry::Start {
                                request: Some(recorded_request),
                                ..
                            } = entry
                            else {
                                unreachable!(
                                    "request-matching claim only accepts Start entries with requests"
                                );
                            };
                            recorded_request_payload_matches(
                                self.cursor.oplog.as_ref(),
                                &recorded_request,
                                expected_request,
                            )
                            .await
                            .map_err(|error| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to load deleted durable call request payload: {error}"
                                ))
                            })?
                        }
                        None => true,
                    };
                    if request_matches {
                        return Ok(true);
                    }
                }
                next = last_read.next();
            }
        }
        Ok(false)
    }

    /// Claims the `Start` entry described by `claim`: builds the identity predicate from the
    /// typed descriptor and drives the shared claim core ([`Self::claim_start_matching`], or its
    /// request-matching counterpart [`Self::claim_start_matching_request`] when the descriptor
    /// pins the recorded request payload). Returns the typed claim attempt so callers can handle a
    /// genuine missing match separately from storage or payload failures.
    pub(super) async fn claim_start(
        &mut self,
        claim: &StartClaim,
    ) -> Result<StartClaimAttempt, WorkerExecutorError> {
        let matches_identity = |entry: &OplogEntry| claim.matches_start_identity(entry);
        let attempt = match claim.matching_request() {
            Some(expected_request) => {
                self.claim_start_matching_request(matches_identity, expected_request)
                    .await?
            }
            None => self.claim_start_matching(matches_identity).await?,
        };
        let (mut handle, entry) = match attempt {
            StartClaimAttempt::Claimed(handle, entry) => (handle, entry),
            other => return Ok(other),
        };
        if claim.is_reconstruction_claim() {
            let reconstruction = self
                .st
                .concurrent_resolver
                .register_reconstruction(handle.start_idx());
            handle.attach_historical_reconstruction(reconstruction);
        }
        // Every `Start` claim registers a resolver awaiter atomically with the consume/claim, so
        // its terminal is always a resolver-routed *awaited terminal* — never an orphan a parked
        // awaiter behind it could sleep on until `switch_to_live`. The only un-drained terminals
        // the cursor may leave at its head are the dedicated-positional-consumer pairs (manual
        // durability, `GolemApiFork`).
        debug_assert!(
            self.st.concurrent_resolver.has_claim(handle.start_idx()),
            "Start claim at {} must leave a registered awaiter",
            handle.start_idx()
        );
        Ok(StartClaimAttempt::Claimed(handle, entry))
    }

    /// Claims an exact scope `Start`. When `recover_missing` is set, a missing scope enters replay
    /// settlement only after proving that doing so cannot abandon another concurrent operation.
    pub(super) async fn claim_scope_start_with_missing_recovery(
        &mut self,
        claim: &StartClaim,
        recover_missing: bool,
    ) -> Result<StartClaimAttempt, WorkerExecutorError> {
        debug_assert!(!claim.carries_request());
        let outcome = self
            .claim_start_matching(|entry| claim.matches_start_identity(entry))
            .await?;
        if !matches!(outcome, StartClaimAttempt::Missing) {
            return Ok(outcome);
        }
        if !recover_missing {
            return Ok(StartClaimAttempt::Missing);
        }

        if self.st.concurrent_resolver.has_any_claims()
            || !self.st.claimed_starts.is_empty()
            || !self.st.claimed_custom_invocation_ids.is_empty()
            || !self.st.custom_subtrees.is_empty()
        {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                claim.expected_description(),
                "the scope Start is missing while another concurrent replay claim is active"
                    .to_string(),
            ));
        }

        let expected_name = claim
            .expected_function_name()
            .expect("a recoverable scope claim always has an exact function name");
        let replay_target = self.cursor.replay_target();
        let name_collision = self
            .cursor
            .scan_oplog(
                OplogIndex::INITIAL,
                replay_target.next(),
                &self.st.skipped_regions,
                self.st
                    .skipped_regions
                    .find_next_deleted_region(OplogIndex::INITIAL),
                OplogIndex::NONE,
                |entry, _, _| {
                    matches!(entry, OplogEntry::Start { function_name, .. }
                        if function_name == expected_name)
                },
                |_, _, _| true,
                (),
                |_, _, _| {},
            )
            .await;
        if matches!(name_collision, OplogEntryLookupResult::Found { .. }) {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                claim.expected_description(),
                "a scope Start with the same discriminator exists but cannot be claimed"
                    .to_string(),
            ));
        }

        let suffix_start = self.cursor.last_replayed_index().next();
        let unsafe_suffix = self
            .cursor
            .scan_oplog(
                suffix_start,
                replay_target.next(),
                &self.st.skipped_regions,
                self.st
                    .skipped_regions
                    .find_next_deleted_region(suffix_start),
                OplogIndex::NONE,
                |entry, begin_idx, state| {
                    matches!(entry, OplogEntry::CompletionDelivered { .. })
                        || !entry.no_concurrent_side_effect(begin_idx, state)
                },
                |_, _, _| true,
                ScopeScanState {
                    root: OplogIndex::NONE,
                    descendants: HashSet::new(),
                    current_is_descendant_scope: false,
                },
                |entry, idx, state| entry.track_scope_membership(idx, state),
            )
            .await;
        if matches!(unsafe_suffix, OplogEntryLookupResult::Found { .. }) {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                claim.expected_description(),
                "the scope Start is missing before an unsafe concurrent side effect or delivery boundary"
                    .to_string(),
            ));
        }

        self.cursor.begin_settling();
        Ok(StartClaimAttempt::MissingSettling { replay_target })
    }

    /// Switches the cursor to live mode: records `ReplayFinished` if replay was still in progress,
    /// clamps the cursor head to the replay target, and wakes every still-suspended awaiter with
    /// `Incomplete` (any durable call whose `Start` was committed but whose terminal never was).
    pub(super) fn switch_to_live(&mut self) -> OplogIndex {
        let replay_target = self.cursor.replay_target();
        if self.cursor.last_replayed_index() != replay_target {
            self.record_replay_event(ReplayEvent::ReplayFinished);
        }
        self.cursor.begin_settling();
        self.cursor.position.last_replayed_index.set(replay_target);
        // Replay is over: any durable call whose `Start` was committed but whose terminal never was
        // is incomplete. Wake every still-suspended awaiter so it returns `Incomplete` instead of
        // sleeping forever waiting for a cursor that will not advance again.
        self.st.concurrent_resolver.fail_all_pending_incomplete();
        // Scan-ahead-claimed `Start`s the cursor never reached are moot now: their awaiters were
        // just failed with `Incomplete`, and the cursor will not read again.
        self.st.claimed_starts.clear();
        self.st.claimed_custom_invocation_ids.clear();
        self.st.custom_subtrees.clear();
        self.st.replay_buffer.clear();
        self.notify_progress = true;
        replay_target
    }

    pub(super) async fn finish_primary_settling(
        &mut self,
        expected_target: OplogIndex,
        linear_memory: &crate::services::linear_memory::LinearMemoryTracker,
    ) -> LivePublicationOutcome {
        let phase = self.cursor.transition_phase.load(Ordering::Acquire);
        let replay_target = self.cursor.replay_target();
        let last_replayed_index = self.cursor.last_replayed_index();
        if replay_target != expected_target {
            return LivePublicationOutcome::ReplayResumed;
        }
        if phase == ReplayTransitionPhase::Live as u8 {
            return if last_replayed_index == expected_target {
                LivePublicationOutcome::AlreadyLiveAtSameTarget
            } else {
                LivePublicationOutcome::ReplayResumed
            };
        }
        if phase != ReplayTransitionPhase::Settling as u8 {
            return LivePublicationOutcome::ReplayResumed;
        }

        let mut incomplete_reconstructions =
            self.st.concurrent_resolver.pending_reconstruction_starts();
        if let Some(first_start) = incomplete_reconstructions.iter().min().copied() {
            let mut next = first_start.next();
            while next <= replay_target && !incomplete_reconstructions.is_empty() {
                let available = u64::from(replay_target) - u64::from(next) + 1;
                let entries = self
                    .cursor
                    .read_oplog(next, CHUNK_SIZE.min(available))
                    .await;
                let last_read = entries
                    .last()
                    .expect("the fixed replay target must remain readable")
                    .0;
                for (index, entry) in entries {
                    if index > replay_target {
                        break;
                    }
                    if !self.st.skipped_regions.is_in_deleted_region(index)
                        && let Some(start_index) = terminal_start_index(&entry)
                    {
                        incomplete_reconstructions.remove(&start_index);
                    }
                }
                next = last_read.next();
            }
        }

        // Reconstruction registration and terminal routing use this same cursor transaction. An
        // unresolved claim may be classified as incomplete only while the target is still the one
        // scanned above. Every claim with a visible terminal remains a publication fence until its
        // body has validated and dropped the reconstruction guard.
        if !self
            .st
            .concurrent_resolver
            .only_pending_reconstruction_fences_remain(&incomplete_reconstructions)
        {
            return LivePublicationOutcome::ReconstructionClaimsActive;
        }

        #[cfg(test)]
        {
            let publication_gate = self.cursor.primary_publication_gate.lock().unwrap().take();
            if let Some((entered, release)) = publication_gate {
                entered.wait().await;
                release.wait().await;
            }
        }

        let owner_tool_operations = self.cursor.owner_tool_operations.clone();
        if owner_tool_operations.commit_if_owner_open(|| {
            self.switch_to_live();
            linear_memory.switch_to_live();
            self.cursor.publish_live();
        }) {
            LivePublicationOutcome::Published
        } else {
            LivePublicationOutcome::OwnerFailed
        }
    }

    pub(super) fn finish_non_primary_settling(
        &mut self,
        expected_target: OplogIndex,
    ) -> LivePublicationOutcome {
        let phase = self.cursor.transition_phase.load(Ordering::Acquire);
        let replay_target = self.cursor.replay_target();
        let last_replayed_index = self.cursor.last_replayed_index();
        if replay_target != expected_target {
            return LivePublicationOutcome::ReplayResumed;
        }
        if phase == ReplayTransitionPhase::Live as u8 && last_replayed_index == expected_target {
            return LivePublicationOutcome::AlreadyLiveAtSameTarget;
        }
        if phase != ReplayTransitionPhase::Settling as u8 {
            return LivePublicationOutcome::ReplayResumed;
        }

        self.switch_to_live();
        LivePublicationOutcome::Published
    }

    /// Resets the cursor to the start of replay after dropping a manual-update override.
    pub(super) async fn drop_override_and_restart(&mut self) -> Result<(), WorkerExecutorError> {
        self.st.skipped_regions.drop_override();
        self.st.initial_snapshot_skip_end = None;
        let next = self
            .st
            .skipped_regions
            .find_next_deleted_region(OplogIndex::NONE);
        self.st.next_skipped_region = next;
        self.cursor.set_log_hashes(HashMap::new());
        self.cursor.pending_replay_events.lock().unwrap().clear();
        self.st.claimed_starts.clear();
        self.st.claimed_custom_invocation_ids.clear();
        self.st.custom_subtrees.clear();
        self.st.replay_buffer.clear();
        self.cursor
            .position
            .last_replayed_index
            .set(OplogIndex::NONE);
        self.cursor
            .position
            .last_replayed_non_hint_index
            .set(OplogIndex::NONE);
        self.cursor
            .transition_phase
            .store(ReplayTransitionPhase::Replaying as u8, Ordering::Release);
        self.move_replay_idx(OplogIndex::INITIAL).await;
        self.skip_forward().await?;
        if self.cursor.is_live() {
            self.cursor.publish_live();
        }
        Ok(())
    }
}

impl ReplayState {
    pub(crate) async fn new_for_owner(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        skipped_regions: DeletedRegions,
        initial_snapshot_skip_end: Option<OplogIndex>,
        owner_tool_operations: Arc<crate::durable_host::tool::operation::OwnerToolOperations>,
    ) -> Result<Self, WorkerExecutorError> {
        let next_skipped_region = skipped_regions.find_next_deleted_region(OplogIndex::NONE);
        let last_oplog_index = oplog.current_oplog_index().await;
        let completion_markers =
            Self::scan_completion_markers(&oplog, OplogIndex::INITIAL, last_oplog_index).await?;
        let concurrent_resolver = ConcurrentReplayResolver::default();
        let reconstruction_claims = concurrent_resolver.reconstruction_claims();
        let cursor = ReplayCursor {
            owned_agent_id,
            oplog,
            owner_tool_operations,
            advance_gate: Arc::new(tokio::sync::Mutex::new(())),
            delivery_failure: std::sync::Mutex::new(None),
            position: PublishedPosition {
                last_replayed_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
                last_replayed_non_hint_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
                has_seen_logs: AtomicBool::new(false),
            },
            replay_target: AtomicOplogIndex::from_oplog_index(last_oplog_index),
            transition_phase: AtomicU8::new(ReplayTransitionPhase::Replaying as u8),
            state: Mutex::new(CursorState {
                skipped_regions,
                next_skipped_region,
                initial_snapshot_skip_end,
                skip_hints_after_delivery: false,
                replay_buffer: VecDeque::new(),
                pending_fork_starts: HashSet::new(),
                concurrent_resolver,
                claimed_starts: HashSet::new(),
                claimed_custom_invocation_ids: HashSet::new(),
                custom_subtrees: HashMap::new(),
            }),
            reconstruction_claims,
            completion_markers: std::sync::Mutex::new(completion_markers),
            log_hashes: std::sync::Mutex::new(HashMap::new()),
            pending_replay_events: std::sync::Mutex::new(Vec::new()),
            progress: Notify::new(),
            #[cfg(test)]
            primary_publication_gate: std::sync::Mutex::new(None),
        };
        {
            // No concurrency during construction: the replay state is not shared yet, so driving the
            // cursor without anyone to notify is sound.
            let mut tx = cursor.tx().await?;
            tx.move_replay_idx(OplogIndex::INITIAL).await; // By this we handle initial skipped regions applied by manual updates correctly
            tx.skip_forward().await?;
        }
        if cursor.is_live() {
            cursor.publish_live();
        }
        Ok(Self {
            cursor: Arc::new(cursor),
        })
    }

    /// Scans `[from, to]` for successful-completion delivery markers. Exactly one of
    /// `CompletionDelivered` or `CompletionDiscarded` may reference a `Start`; duplicates or a
    /// conflicting pair are oplog corruption.
    pub(super) async fn scan_completion_markers(
        oplog: &Arc<dyn Oplog>,
        from: OplogIndex,
        to: OplogIndex,
    ) -> Result<HashMap<OplogIndex, CompletionMarker>, WorkerExecutorError> {
        const CHUNK_SIZE: u64 = 1024;
        let mut markers = HashMap::new();
        let mut next = from;
        while next <= to {
            let available = u64::from(to) - u64::from(next) + 1;
            let entries = oplog.read_exact(next, CHUNK_SIZE.min(available)).await;
            let last_read = *entries.last_key_value().unwrap().0;
            for (marker_idx, entry) in entries {
                if marker_idx > to {
                    break;
                }
                let marker = match entry {
                    OplogEntry::CompletionDelivered { start_index, .. } => {
                        Some((start_index, CompletionMarker::Delivered(marker_idx)))
                    }
                    OplogEntry::CompletionDiscarded { start_index, .. } => {
                        Some((start_index, CompletionMarker::Discarded(marker_idx)))
                    }
                    _ => None,
                };
                if let Some((start_index, marker)) = marker
                    && let Some(previous) = markers.insert(start_index, marker)
                {
                    return Err(WorkerExecutorError::runtime(format!(
                        "corrupt oplog: multiple completion-delivery markers reference the durable call Start at {start_index} ({} at {}, {} at {})",
                        previous.entry_name(),
                        previous.index(),
                        marker.entry_name(),
                        marker.index(),
                    )));
                }
            }
            next = last_read.next();
        }
        Ok(markers)
    }

    /// Records a live-appended successful-completion delivery marker in the same map populated by
    /// the replay scan.
    pub(super) fn record_completion_marker(
        &self,
        start_index: OplogIndex,
        marker: CompletionMarker,
    ) {
        let previous = self
            .cursor
            .completion_markers
            .lock()
            .unwrap()
            .insert(start_index, marker);
        if let Some(previous) = previous {
            tracing::warn!(
                "duplicate completion-delivery marker recorded for durable call Start {start_index}: {} at {}, {} at {}",
                previous.entry_name(),
                previous.index(),
                marker.entry_name(),
                marker.index(),
            );
        }
    }

    pub fn record_discarded_completion(&self, start_index: OplogIndex, marker_index: OplogIndex) {
        self.record_completion_marker(start_index, CompletionMarker::Discarded(marker_index));
    }

    pub fn record_delivered_completion(&self, start_index: OplogIndex, marker_index: OplogIndex) {
        self.record_completion_marker(start_index, CompletionMarker::Delivered(marker_index));
    }

    pub(super) fn record_delivery_failure(&self, failure: String) {
        let mut current = self.cursor.delivery_failure.lock().unwrap();
        if current.is_none() {
            tracing::error!("{failure}");
            *current = Some(failure);
        }
    }

    pub(in crate::durable_host) fn fail_completion_delivery(
        &self,
        start_index: OplogIndex,
        marker_index: OplogIndex,
        reason: impl Into<String>,
    ) {
        self.record_delivery_failure(format!(
            "replay could not reproduce CompletionDelivered at {marker_index} for durable call Start at {start_index}: {}",
            reason.into()
        ));
        self.cursor.progress.notify_waiters();
    }

    /// Poisons replay because a markerless completed durable call (its recorded run crashed after
    /// the `End` became durable but before the completion crossed to the guest) hit a delivery
    /// boundary while it was still tail-gated: its delivery token must first wait for the replay
    /// tail via `CompletionDelivery::prepare_delivery`.
    pub(in crate::durable_host) fn fail_tail_delivery(
        &self,
        start_index: OplogIndex,
        reason: impl Into<String>,
    ) {
        self.record_delivery_failure(format!(
            "replay could not withhold the markerless completion of durable call Start at {start_index} until the end of the replay tail: {}",
            reason.into()
        ));
        self.cursor.progress.notify_waiters();
    }

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
    pub(super) async fn with_tx<R>(
        &self,
        op: impl AsyncFnOnce(&mut CursorTx<'_>) -> Result<R, WorkerExecutorError>,
    ) -> Result<R, WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await?;
        let result = op(&mut tx).await;
        cursor.finish_tx(tx);
        result
    }

    /// The error returned when a positional oplog reader expects a next entry but the cursor is
    /// at end-of-replay.
    fn end_of_replay_error(&self) -> WorkerExecutorError {
        WorkerExecutorError::unexpected_oplog_entry(
            "next oplog entry to replay",
            format!(
                "end of replay for {} at index {}; replay target = {}",
                self.cursor.owned_agent_id,
                self.cursor.last_replayed_index(),
                self.cursor.replay_target(),
            ),
        )
    }

    pub async fn drop_override_and_restart(&self) -> Result<(), WorkerExecutorError> {
        self.with_tx(async |tx| tx.drop_override_and_restart().await)
            .await
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
    pub(super) async fn run_owned_cursor_op<R, Fut>(
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

    pub(super) async fn switch_cursor_to_live(&self) -> Result<OplogIndex, WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            let replay_target = state
                .with_tx(async |tx| {
                    let replay_target = tx.switch_to_live();
                    Ok(replay_target)
                })
                .await?;
            // `CursorTx::switch_to_live` publishes the cursor position directly (not via
            // `move_replay_idx`), so replay-progress observers are notified here.
            state
                .cursor
                .oplog
                .on_replay_progress(state.cursor.last_replayed_index())
                .await;
            Ok(replay_target)
        })
        .await
    }

    async fn begin_primary_settling(&self) -> Result<OplogIndex, WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            state
                .with_tx(async |tx| {
                    tx.cursor.begin_settling();
                    Ok(tx.cursor.replay_target())
                })
                .await
        })
        .await
    }

    async fn wait_for_reconstruction_fences(&self) -> Result<(), WorkerExecutorError> {
        tokio::select! {
            biased;
            failure = self.cursor.owner_tool_operations.wait_for_owner_failure() => {
                Err(historical_reconstruction_owner_failure(failure))
            }
            _ = self.cursor.reconstruction_claims.wait_for_fences() => Ok(()),
        }
    }

    #[cfg(test)]
    pub(super) async fn test_wait_for_reconstruction_fences(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        self.wait_for_reconstruction_fences().await
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn test_is_settling(&self) -> bool {
        self.cursor.transition_phase.load(Ordering::Acquire)
            == ReplayTransitionPhase::Settling as u8
    }

    pub(crate) async fn switch_to_live(
        &self,
        linear_memory: &crate::services::linear_memory::LinearMemoryTracker,
        role: ReplayToLiveRole,
    ) -> Result<ReplayToLiveOutcome, WorkerExecutorError> {
        if role == ReplayToLiveRole::PrimaryAgent {
            let replay_target = self.begin_primary_settling().await?;
            self.finish_settling_to_live(linear_memory, role, replay_target)
                .await
        } else {
            let replay_target = self.switch_cursor_to_live().await?;
            linear_memory.switch_to_live();
            Ok(ReplayToLiveOutcome::Live { replay_target })
        }
    }

    pub(crate) async fn finish_settling_to_live(
        &self,
        linear_memory: &crate::services::linear_memory::LinearMemoryTracker,
        role: ReplayToLiveRole,
        replay_target: OplogIndex,
    ) -> Result<ReplayToLiveOutcome, WorkerExecutorError> {
        if role == ReplayToLiveRole::PrimaryAgent {
            loop {
                if let Some(failure) = self.cursor.owner_tool_operations.selected_owner_failure() {
                    return Err(historical_reconstruction_owner_failure(failure));
                }
                let mut reconstruction_fences =
                    self.cursor.reconstruction_claims.subscribe_fences();
                let progress = self.cursor.progress.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                let publication_linear_memory = linear_memory.clone();
                let publication = self
                    .run_owned_cursor_op(move |state| async move {
                        state
                            .with_tx(async |tx| {
                                Ok(tx
                                    .finish_primary_settling(
                                        replay_target,
                                        &publication_linear_memory,
                                    )
                                    .await)
                            })
                            .await
                    })
                    .await?;
                match publication {
                    LivePublicationOutcome::Published => break,
                    LivePublicationOutcome::AlreadyLiveAtSameTarget => {
                        linear_memory.switch_to_live();
                        break;
                    }
                    LivePublicationOutcome::ReconstructionClaimsActive => {
                        tokio::select! {
                            biased;
                            failure = self.cursor.owner_tool_operations.wait_for_owner_failure() => {
                                return Err(historical_reconstruction_owner_failure(failure));
                            }
                            changed = reconstruction_fences.changed() => {
                                changed.expect("replay cursor retains the reconstruction claim state");
                            }
                            _ = progress.as_mut() => {}
                        }
                    }
                    LivePublicationOutcome::OwnerFailed => {
                        let failure = self
                            .cursor
                            .owner_tool_operations
                            .selected_owner_failure()
                            .expect("failed live publication must retain the owner winner");
                        return Err(historical_reconstruction_owner_failure(failure));
                    }
                    LivePublicationOutcome::ReplayResumed => {
                        return Ok(ReplayToLiveOutcome::ReplayResumed);
                    }
                }
            }
            Ok(ReplayToLiveOutcome::Live { replay_target })
        } else {
            let publication = self
                .run_owned_cursor_op(move |state| async move {
                    state
                        .with_tx(async |tx| Ok(tx.finish_non_primary_settling(replay_target)))
                        .await
                })
                .await?;
            match publication {
                LivePublicationOutcome::Published
                | LivePublicationOutcome::AlreadyLiveAtSameTarget => {
                    linear_memory.switch_to_live();
                    Ok(ReplayToLiveOutcome::Live { replay_target })
                }
                LivePublicationOutcome::ReconstructionClaimsActive => {
                    unreachable!("non-primary settlement does not inspect reconstruction claims")
                }
                LivePublicationOutcome::OwnerFailed => {
                    unreachable!("non-primary settlement does not arbitrate owner publication")
                }
                LivePublicationOutcome::ReplayResumed => Ok(ReplayToLiveOutcome::ReplayResumed),
            }
        }
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn test_drain_reconstruction_terminal(
        &self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        let (terminal_index, terminal) = self
            .visible_terminal_record(start_index)
            .await
            .ok_or_else(|| {
                WorkerExecutorError::runtime(format!(
                    "test replay driver found no visible terminal for reconstruction Start {start_index}"
                ))
            })?;
        let marker = self
            .cursor
            .completion_markers
            .lock()
            .unwrap()
            .get(&start_index)
            .copied();
        let resolution = match terminal {
            OplogEntry::End {
                response,
                forced_commit,
                ..
            } => match marker {
                Some(CompletionMarker::Discarded(marker_idx)) => {
                    Resolution::CompletedButDiscarded {
                        end_idx: terminal_index,
                        marker_idx,
                        response,
                    }
                }
                Some(CompletionMarker::Delivered(marker_idx)) => Resolution::Completed {
                    end_idx: terminal_index,
                    response,
                    delivery_marker: Some(marker_idx),
                    forced_commit,
                },
                None => Resolution::Completed {
                    end_idx: terminal_index,
                    response,
                    delivery_marker: None,
                    forced_commit,
                },
            },
            OplogEntry::Cancelled { partial, .. } => Resolution::Cancelled {
                cancelled_idx: terminal_index,
                partial,
            },
            _ => unreachable!("visible_terminal_record returns only terminal entries"),
        };
        self.run_owned_cursor_op(move |state| async move {
            state
                .with_tx(async |tx| {
                    tx.st.concurrent_resolver.resolve_prefetched_for_test(
                        start_index,
                        terminal_index,
                        resolution,
                    );
                    Ok(())
                })
                .await
        })
        .await
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn test_drain_terminal_clamp_then_reconstruction_barrier(
        &self,
        start_index: OplogIndex,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>, WorkerExecutorError> {
        self.test_drain_reconstruction_terminal(start_index).await?;
        self.begin_primary_settling().await?;
        let replay = self.clone();
        Ok(Box::pin(async move {
            replay
                .wait_for_reconstruction_fences()
                .await
                .expect("test reconstruction barrier observed owner failure");
            replay
                .switch_cursor_to_live()
                .await
                .expect("test reconstruction barrier failed to clamp replay");
        }))
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn test_clamp_after_claim(
        &self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            let claimed = self
                .run_owned_cursor_op(move |state| async move {
                    let st = state.cursor.state.lock().await;
                    Ok(st.concurrent_resolver.has_claim(start_index))
                })
                .await?;
            if claimed {
                self.switch_cursor_to_live().await?;
                return Ok(());
            }
            if self.is_live() {
                return Err(WorkerExecutorError::runtime(format!(
                    "test replay driver reached live mode before Start {start_index} was claimed"
                )));
            }

            progress.await;
        }
    }

    pub(crate) fn historical_reconstruction_bodies(
        &self,
    ) -> tokio::sync::watch::Receiver<HashSet<OplogIndex>> {
        self.cursor.reconstruction_claims.subscribe_bodies()
    }

    pub(crate) fn ensure_reconstruction_claims_empty(&self) -> Result<(), WorkerExecutorError> {
        self.cursor.reconstruction_claims.ensure_empty()
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

    /// Reports whether the replay-visible owner oplog contains a terminal for `start_index`.
    /// Entity reconstruction uses this before starting its fresh Store so the invocation context
    /// distinguishes reconstruction of a completed body from repair of an incomplete Start. The
    /// scan is read-only and respects fork/revert deleted regions.
    pub(crate) async fn has_visible_terminal(&self, start_index: OplogIndex) -> bool {
        self.visible_terminal_entry(start_index).await.is_some()
    }

    /// Reports whether the replay-visible oplog contains any recorded work in the durable scope
    /// rooted at `start_index`. An incomplete filesystem-capable tool with no such work can release
    /// its historical body-reconstruction fence while it continues staging fresh input; a tool
    /// whose prior body started must retain the fence and reconstruct those descendants first.
    pub(crate) async fn has_visible_scope_descendant(&self, start_index: OplogIndex) -> bool {
        let replay_target = self.replay_target();
        if start_index >= replay_target {
            return false;
        }
        let skipped_regions = {
            let state = self.cursor.state.lock().await;
            state.skipped_regions.clone()
        };
        let mut projection = OplogScopeProjection::new(start_index);
        let mut next = start_index.next();
        while next <= replay_target {
            let available = u64::from(replay_target) - u64::from(next) + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(available))
                .await;
            let last_read = *entries.last_key_value().unwrap().0;
            for (index, entry) in entries {
                if index > replay_target {
                    break;
                }
                if !skipped_regions.is_in_deleted_region(index)
                    && projection.includes(index, &entry)
                {
                    return true;
                }
            }
            next = last_read.next();
        }
        false
    }

    /// Returns the replay-visible terminal for `start_index` without advancing the positional
    /// cursor. Tool replay uses the terminal's body-execution decision before allocating a
    /// transient Store; the ordinary reconstruction path subsequently consumes the same terminal.
    pub(crate) async fn visible_terminal_entry(
        &self,
        start_index: OplogIndex,
    ) -> Option<OplogEntry> {
        self.visible_terminal_record(start_index)
            .await
            .map(|(_, entry)| entry)
    }

    async fn visible_terminal_record(
        &self,
        start_index: OplogIndex,
    ) -> Option<(OplogIndex, OplogEntry)> {
        let replay_target = self.replay_target();
        if start_index >= replay_target {
            return None;
        }
        let skipped_regions = {
            let state = self.cursor.state.lock().await;
            state.skipped_regions.clone()
        };
        let mut next = start_index.next();
        while next <= replay_target {
            let available = u64::from(replay_target) - u64::from(next) + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(available))
                .await;
            let last_read = *entries.last_key_value().unwrap().0;
            if let Some((index, entry)) = entries.into_iter().find(|(index, entry)| {
                *index <= replay_target
                    && !skipped_regions.is_in_deleted_region(*index)
                    && terminal_start_index(entry) == Some(start_index)
            }) {
                return Some((index, entry));
            }
            next = last_read.next();
        }
        None
    }

    /// Waits until the replay cursor is blocked on a record in `root`'s call tree that no still
    /// running reconstructed entity body or live resolver awaiter can consume. This turns a body
    /// that returned before replaying its recorded subtree into permanent structural divergence
    /// instead of leaving the outer terminal awaiter parked forever.
    pub(crate) async fn await_unconsumed_scope_entry(
        &self,
        root: OplogIndex,
        mut active_entity_bodies: tokio::sync::watch::Receiver<HashSet<OplogIndex>>,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        loop {
            let active_bodies = active_entity_bodies.borrow_and_update().clone();
            let bodies_changed = active_entity_bodies.changed();
            tokio::pin!(bodies_changed);
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            if let Some(index) = self.unconsumed_scope_head(root, active_bodies).await? {
                return Ok(index);
            }

            tokio::select! {
                _ = progress.as_mut() => {}
                changed = &mut bodies_changed => {
                    if changed.is_err() {
                        return Err(WorkerExecutorError::runtime(
                            "owner reconstruction body tracker closed during replay",
                        ));
                    }
                }
            }
        }
    }

    pub(super) async fn unconsumed_scope_head(
        &self,
        root: OplogIndex,
        active_entity_bodies: HashSet<OplogIndex>,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let st = cursor.state.lock().await;
            let head = cursor.last_replayed_index().next();
            if head > cursor.replay_target() {
                return Ok(None);
            }

            let mut projection = OplogScopeProjection::new(root);
            let mut parents = HashMap::new();
            let mut previous_index = None;
            let mut previous_included_start = None;
            let mut next = root;
            while next <= head {
                let available = u64::from(head) - u64::from(next) + 1;
                let entries = cursor
                    .oplog
                    .read_exact(next, CHUNK_SIZE.min(available))
                    .await;
                let last_read = *entries.last_key_value().unwrap().0;

                for (index, entry) in entries {
                    if index > head {
                        break;
                    }
                    if st.skipped_regions.is_in_deleted_region(index) {
                        previous_index = Some(index);
                        previous_included_start = None;
                        continue;
                    }
                    let included = projection.includes(index, &entry);
                    let included_start = if included
                        && let OplogEntry::Start {
                            parent_start_index, ..
                        } = &entry
                    {
                        if let Some(parent) = parent_start_index {
                            parents.insert(index, *parent);
                        }
                        Some(index)
                    } else {
                        None
                    };

                    if index == head {
                        if !included {
                            return Ok(None);
                        }
                        if terminal_start_index(&entry).is_some_and(|start_index| {
                            st.concurrent_resolver.owns_terminal(start_index, index)
                        }) || custom_subtree_entry_is_drainable(&st, &entry)
                        {
                            return Ok(None);
                        }
                        let owner = scope_entry_owner(
                            index,
                            &entry,
                            previous_index,
                            previous_included_start,
                        );
                        if owner.is_some_and(|mut owner| {
                            while owner != root {
                                if active_entity_bodies.contains(&owner) {
                                    return true;
                                }
                                let Some(parent) = parents.get(&owner) else {
                                    break;
                                };
                                owner = *parent;
                            }
                            false
                        }) {
                            return Ok(None);
                        }
                        if matches!(entry, OplogEntry::Start { .. })
                            && st.claimed_starts.contains(&index)
                        {
                            return Ok(None);
                        }
                        if owner.is_some_and(|owner| {
                            owner != root && st.concurrent_resolver.is_awaited(owner)
                        }) {
                            return Ok(None);
                        }
                        if terminal_start_index(&entry) == Some(root)
                            && st.concurrent_resolver.is_awaited(root)
                        {
                            return Ok(None);
                        }
                        return Ok(Some(index));
                    }

                    previous_index = Some(index);
                    previous_included_start = included_start;
                }
                next = last_read.next();
            }
            Ok(None)
        })
        .await
    }

    /// Sets the replay target. This is a phase-boundary operation (e.g. refreshing the target
    /// before replay resumes); it must not race with concurrent cursor advances.
    ///
    /// The completion-marker map is kept in sync with the visible prefix `[.., target]`:
    ///
    /// - Growing the target makes a previously invisible oplog range visible, so the newly
    ///   visible range `(old_target, new_target]` is scanned for completion-delivery markers
    ///   *before* the new target is published — a debug session constructed with a target before
    ///   a marker and later grown past it must park the marked `End` instead of delivering it.
    ///   The merged additions are validated (duplicate markers for the same `Start` are oplog
    ///   corruption) before anything is mutated.
    /// - Shrinking the target hides part of the oplog, so markers beyond the new target are
    ///   removed *before* the smaller target is published — a later regrowth rescans the exposed
    ///   range and rediscovers them (without false duplicate-marker errors), and delivery-time
    ///   validation ([`Self::await_resolution_outcome`]) never sees a marker outside the visible
    ///   prefix.
    ///
    /// Both directions run under the cursor transaction lock, so replay cannot advance while the
    /// map and the target are being updated.
    pub async fn set_replay_target(
        &self,
        new_target: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        let cursor = &*self.cursor;
        self.with_tx(async |tx| {
            let old_target = cursor.replay_target();
            match new_target.cmp(&old_target) {
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Less => {
                    tx.st.replay_buffer.clear();
                    cursor
                        .completion_markers
                        .lock()
                        .unwrap()
                        .retain(|_, marker| marker.index() <= new_target);
                }
                std::cmp::Ordering::Greater => {
                    let additions = Self::scan_completion_markers(
                        &cursor.oplog,
                        old_target.next(),
                        new_target,
                    )
                    .await?;
                    if !additions.is_empty() {
                        let mut markers = cursor.completion_markers.lock().unwrap();
                        for (start_index, marker) in &additions {
                            // Rediscovering the exact marker already in the map (recorded live by
                            // this instance before the target
                            // grew over it) is idempotent; only a *different* marker for the same
                            // `Start` is oplog corruption.
                            if let Some(previous) = markers.get(start_index)
                                && previous != marker
                            {
                                return Err(WorkerExecutorError::runtime(format!(
                                    "corrupt oplog: multiple completion-delivery markers reference the durable call Start at {start_index} ({} at {}, {} at {})",
                                    previous.entry_name(),
                                    previous.index(),
                                    marker.entry_name(),
                                    marker.index(),
                                )));
                            }
                        }
                        markers.extend(additions);
                    }
                }
            }
            if new_target > cursor.last_replayed_index() {
                cursor
                    .transition_phase
                    .store(ReplayTransitionPhase::Replaying as u8, Ordering::Release);
                cursor
                    .pending_replay_events
                    .lock()
                    .unwrap()
                    .retain(|event| !matches!(event, ReplayEvent::ReplayFinished));
            }
            cursor.replay_target.set(new_target);
            if new_target != old_target {
                tx.notify_progress = true;
            }
            Ok(())
        })
        .await
    }

    /// Whether `oplog_index` lies in a deleted (skipped) oplog region. Used as a validity guard
    /// (e.g. rejecting jumps into deleted regions), so a failed cursor read propagates as an error
    /// rather than defaulting to an answer.
    pub async fn is_in_skipped_region(
        &self,
        oplog_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            let st = state.cursor.state.lock().await;
            Ok(st.skipped_regions.is_in_deleted_region(oplog_index))
        })
        .await
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.cursor.is_live()
    }

    /// Returns whether the primary owner has published live admission after reconstruction
    /// settlement. Cursor exhaustion alone is intentionally not sufficient.
    pub(crate) fn is_live_published(&self) -> bool {
        self.cursor.is_live_published()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        self.cursor.is_replay()
    }

    pub fn take_new_replay_events(&self) -> Vec<ReplayEvent> {
        let mut pending = self.cursor.pending_replay_events.lock().unwrap();
        if self.is_live_published() {
            std::mem::take(&mut *pending)
        } else {
            let events = std::mem::take(&mut *pending);
            let mut ready = Vec::with_capacity(events.len());
            for event in events {
                if matches!(event, ReplayEvent::ReplayFinished) {
                    pending.push(event);
                } else {
                    ready.push(event);
                }
            }
            ready
        }
    }

    pub async fn pending_card_derivation(
        &self,
        card_id: CardId,
    ) -> Option<(StoredCard, Option<u64>)> {
        self.cursor
            .pending_replay_events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|event| match event {
                ReplayEvent::CardDerived {
                    card,
                    wallet_generation,
                } if card.card_id() == card_id => Some((card.clone(), *wallet_generation)),
                _ => None,
            })
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    ///
    /// Returns an error if the underlying read fails (e.g. missing oplog entry,
    /// corrupted GolemApiFork payload) so the worker can fail the agent with a
    /// non-retriable trap rather than panicking the executor.
    pub async fn get_oplog_entry(&self) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if let Some(entry) = self
                .with_tx(async |tx| tx.try_get_oplog_entry(|_| true).await)
                .await?
            {
                return Ok(entry);
            }
            if self.is_live() {
                return Err(self.end_of_replay_error());
            }
            // An unconditional reader returns `None` during replay only at a reserved
            // `CompletionDelivered` marker. Wait for its owner to consume and acknowledge it.
            progress.await;
        }
    }

    /// Reads the next oplog entry, and if it matches the given condition, skips
    /// every hint entry following it and returns the oplog index of the entry read.
    /// If the condition is not met, returns `None` and the candidate entry is left unconsumed with
    /// the cursor, skipped-region state, and side effects untouched. (Any *awaited terminals* sitting
    /// ahead of the candidate are drained to their awaiters first — see
    /// [`CursorTx::try_get_oplog_entry`] — and those drains stay committed.)
    ///
    /// Auto-skipped hint entries manipulate worker status but are non-deterministic from the
    /// replay's point of view.
    pub async fn try_get_oplog_entry(
        &self,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let mut condition = condition;
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let (entry, blocked_on_completion_delivery) = self
                .with_tx(async |tx| {
                    let entry = tx.try_get_oplog_entry(&mut condition).await?;
                    Ok((entry, tx.blocked_on_completion_delivery))
                })
                .await?;
            if entry.is_some() || !blocked_on_completion_delivery {
                return Ok(entry);
            }
            progress.await;
        }
    }

    /// [`Self::get_oplog_entry`] variant for callers running inside Wasmtime accessor futures:
    /// the cursor transaction runs on an owned task (see [`Self::run_owned_cursor_op`]), so the
    /// store-polled caller never queues on the cursor mutex directly. Direct invocation-loop /
    /// p2 host-call readers keep using [`Self::get_oplog_entry`].
    pub async fn get_oplog_entry_owned(
        &self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            loop {
                let progress = state.cursor.progress.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                if let Some(entry) = state
                    .with_tx(async |tx| tx.try_get_oplog_entry(|_| true).await)
                    .await?
                {
                    return Ok(entry);
                }
                if state.is_live() {
                    return Err(state.end_of_replay_error());
                }
                progress.await;
            }
        })
        .await
    }

    /// Returns true if the given log entry has unmatched persisted occurrences since the last
    /// non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        if self.cursor.position.has_seen_logs.load(Ordering::Relaxed) {
            let hash = ReplayCursor::hash_log_entry(level, context, message);
            self.cursor.log_hashes.lock().unwrap().contains_key(&hash)
        } else {
            false
        }
    }

    /// Removes one occurrence of a seen log from the multiset (identical log entries may be
    /// persisted multiple times and each must be matched by exactly one re-emitted entry). If the
    /// multiset becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&self, level: LogLevel, context: &str, message: &str) {
        let hash = ReplayCursor::hash_log_entry(level, context, message);
        let log_hashes = &mut *self.cursor.log_hashes.lock().unwrap();
        if let Some(count) = log_hashes.get_mut(&hash) {
            *count -= 1;
            if *count == 0 {
                log_hashes.remove(&hash);
            }
        }
        self.cursor
            .position
            .has_seen_logs
            .store(!log_hashes.is_empty(), Ordering::Relaxed);
    }

    pub async fn lookup_oplog_entry(
        &self,
        begin_idx: OplogIndex,
        check: impl Fn(&OplogEntry, OplogIndex) -> bool,
    ) -> Option<OplogIndex> {
        match self
            .lookup_oplog_entry_with_condition(begin_idx, check, |_, _| true)
            .await
        {
            OplogEntryLookupResult::Found { index, .. } => Some(index),
            OplogEntryLookupResult::NotFound { .. } => None,
        }
    }

    pub async fn lookup_oplog_entry_with_condition(
        &self,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex) -> bool,
    ) -> OplogEntryLookupResult {
        self.lookup_oplog_entry_with_condition_and_state(
            begin_idx,
            |entry, idx, ()| end_check(entry, idx),
            |entry, idx, ()| for_all_intermediate(entry, idx),
            (),
            |_, _, ()| {},
        )
        .await
    }

    /// Forward-scans the oplog from the current cursor head for a matching entry. The scan start and
    /// the skip-region state are snapshotted under a brief cursor-lock acquisition, then the scan
    /// itself runs lock-free (see [`ReplayCursor::scan_oplog`]). Holding the lock only for the
    /// snapshot — rather than across the whole (potentially full-oplog) scan — keeps the snapshot
    /// internally consistent without blocking concurrent cursor advances for the scan's duration.
    pub async fn lookup_oplog_entry_with_condition_and_state<State>(
        &self,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        state: State,
        update_state: impl FnMut(&OplogEntry, OplogIndex, &mut State),
    ) -> OplogEntryLookupResult {
        let cursor = &*self.cursor;
        // The snapshot is taken on an owned task (see `run_owned_cursor_op`): this lookup is
        // called from accessor futures (e.g. the replay-side remote-write scope checks), which
        // must never queue on the cursor mutex directly.
        let snapshot = self
            .run_owned_cursor_op(|state| async move {
                let cursor = &*state.cursor;
                let st = cursor.state.lock().await;
                Ok((
                    cursor.last_replayed_index().next(),
                    st.skipped_regions.clone(),
                    st.next_skipped_region.clone(),
                ))
            })
            .await;
        let (start, skipped_regions, next_skipped_region) = match snapshot {
            Ok(snapshot) => snapshot,
            Err(err) => {
                warn!("oplog lookup cursor snapshot did not complete: {err}");
                return OplogEntryLookupResult::NotFound {
                    violates_for_all: true,
                };
            }
        };
        cursor
            .scan_oplog(
                start,
                cursor.replay_target().next(),
                &skipped_regions,
                next_skipped_region,
                begin_idx,
                end_check,
                for_all_intermediate,
                state,
                update_state,
            )
            .await
    }

    pub async fn get_oplog_entry_agent_invocation_started(
        &self,
    ) -> Result<Option<AgentInvocationStartedEntry>, WorkerExecutorError> {
        loop {
            if self.is_replay() {
                let (oplog_index, oplog_entry) = self.get_oplog_entry().await?;
                match oplog_entry {
                    OplogEntry::AgentInvocationStarted {
                        idempotency_key,
                        payload,
                        trace_id,
                        trace_states,
                        invocation_context: spans,
                        wallet_pin,
                        ..
                    } => {
                        let invocation_payload = self
                            .cursor
                            .oplog
                            .download_payload(payload)
                            .await
                            .map_err(|err| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to deserialize agent invocation payload: {err}"
                                ))
                            })?;

                        let invocation_context =
                            InvocationContextStack::from_oplog_data(trace_id, trace_states, spans);

                        break Ok(Some(AgentInvocationStartedEntry {
                            oplog_index,
                            idempotency_key,
                            invocation_payload,
                            invocation_context,
                            wallet_pin,
                        }));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(WorkerExecutorError::unexpected_oplog_entry(
                            "AgentInvocationStarted",
                            format!("{oplog_entry:?}"),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    pub async fn get_oplog_entry_agent_invocation_finished(
        &self,
    ) -> Result<Option<AgentInvocationResult>, WorkerExecutorError> {
        // The walk to the finished marker tolerates live-only abandoned durable-call records
        // (see `AbandonedStarts`): the replayed guest has already produced its invocation
        // result, so any still-unclaimed `Start` (and its terminal) can never be claimed and is
        // dead partial progress of a branch the guest abandoned at a point replay did not
        // reproduce.
        let mut abandoned = AbandonedStarts::default();
        loop {
            if self.is_replay() {
                let (_, oplog_entry) = self
                    .get_oplog_entry_at_invocation_boundary(&mut abandoned)
                    .await?;
                match oplog_entry {
                    OplogEntry::AgentInvocationFinished { result, .. } => {
                        std::mem::take(&mut abandoned).finish(&self.cursor.owned_agent_id)?;

                        let result: AgentInvocationResult = self
                            .cursor
                            .oplog
                            .download_payload(result)
                            .await
                            .map_err(|err| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to deserialize agent invocation result payload: {err}"
                                ))
                            })?;

                        break Ok(Some(result));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(WorkerExecutorError::unexpected_oplog_entry(
                            "AgentInvocationFinished",
                            format!("{oplog_entry:?}"),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    /// [`Self::get_oplog_entry`] for the agent-invocation-finished reader: drains live-only
    /// abandoned durable-call records into `abandoned` instead of handing them to the positional
    /// reader (see [`AbandonedStarts`]).
    pub(super) async fn get_oplog_entry_at_invocation_boundary(
        &self,
        abandoned: &mut AbandonedStarts,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if let Some(entry) = self
                .with_tx(async |tx| {
                    tx.try_get_oplog_entry_at_invocation_boundary(abandoned, |_| true)
                        .await
                })
                .await?
            {
                return Ok(entry);
            }
            if self.is_live() {
                return Err(self.end_of_replay_error());
            }
            progress.await;
        }
    }
}

fn custom_subtree_entry_is_drainable(state: &CursorState, entry: &OplogEntry) -> bool {
    let custom_root = |member| {
        state
            .custom_subtrees
            .iter()
            .find_map(|(root, members)| members.contains(&member).then_some(*root))
    };
    match entry {
        OplogEntry::Start {
            observational_owner,
            parent_start_index,
            ..
        } => observational_owner
            .and_then(custom_root)
            .or_else(|| parent_start_index.and_then(custom_root))
            .is_some(),
        entry => terminal_start_index(entry)
            .and_then(custom_root)
            .is_some_and(|root| terminal_start_index(entry) != Some(root)),
    }
}

fn scope_entry_owner(
    index: OplogIndex,
    entry: &OplogEntry,
    previous_index: Option<OplogIndex>,
    previous_included_start: Option<OplogIndex>,
) -> Option<OplogIndex> {
    match entry {
        OplogEntry::Start { .. } => Some(index),
        OplogEntry::End { start_index, .. }
        | OplogEntry::Cancelled { start_index, .. }
        | OplogEntry::CompletionDiscarded { start_index, .. }
        | OplogEntry::CompletionDelivered { start_index, .. } => Some(*start_index),
        OplogEntry::HostStreamFrame {
            parent_start_index, ..
        }
        | OplogEntry::Log {
            parent_start_index: Some(parent_start_index),
            ..
        }
        | OplogEntry::StartSpan {
            parent_start_index: Some(parent_start_index),
            ..
        }
        | OplogEntry::FinishSpan {
            parent_start_index: Some(parent_start_index),
            ..
        }
        | OplogEntry::SetSpanAttribute {
            parent_start_index: Some(parent_start_index),
            ..
        } => Some(*parent_start_index),
        OplogEntry::Error { retry_from, .. } => Some(*retry_from),
        OplogEntry::BeginRemoteTransaction {
            original_begin_index: Some(begin),
            ..
        } => Some(*begin),
        OplogEntry::BeginRemoteTransaction {
            original_begin_index: None,
            ..
        } => previous_index
            .zip(previous_included_start)
            .and_then(|(previous, start)| {
                (previous == start && previous.next() == index).then_some(start)
            }),
        OplogEntry::PreCommitRemoteTransaction { begin_index, .. }
        | OplogEntry::PreRollbackRemoteTransaction { begin_index, .. }
        | OplogEntry::CommittedRemoteTransaction { begin_index, .. }
        | OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => Some(*begin_index),
        OplogEntry::Create { .. }
        | OplogEntry::AgentInvocationStarted { .. }
        | OplogEntry::AgentInvocationFinished { .. }
        | OplogEntry::Suspend { .. }
        | OplogEntry::NoOp { .. }
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::BeginAtomicRegion { .. }
        | OplogEntry::EndAtomicRegion { .. }
        | OplogEntry::PendingAgentInvocation { .. }
        | OplogEntry::PendingUpdate { .. }
        | OplogEntry::SuccessfulUpdate { .. }
        | OplogEntry::FailedUpdate { .. }
        | OplogEntry::GrowMemory { .. }
        | OplogEntry::FilesystemStorageUsageUpdate { .. }
        | OplogEntry::CreateResource { .. }
        | OplogEntry::DropResource { .. }
        | OplogEntry::Log {
            parent_start_index: None,
            ..
        }
        | OplogEntry::Restart { .. }
        | OplogEntry::ActivatePlugin { .. }
        | OplogEntry::DeactivatePlugin { .. }
        | OplogEntry::Revert { .. }
        | OplogEntry::CancelPendingInvocation { .. }
        | OplogEntry::StartSpan {
            parent_start_index: None,
            ..
        }
        | OplogEntry::FinishSpan {
            parent_start_index: None,
            ..
        }
        | OplogEntry::SetSpanAttribute {
            parent_start_index: None,
            ..
        }
        | OplogEntry::Snapshot { .. }
        | OplogEntry::OplogProcessorCheckpoint { .. }
        | OplogEntry::SetRetryPolicy { .. }
        | OplogEntry::RemoveRetryPolicy { .. }
        | OplogEntry::CardEventQueued { .. }
        | OplogEntry::CardInstalled { .. }
        | OplogEntry::CardInstallFailed { .. }
        | OplogEntry::CardRevoked { .. }
        | OplogEntry::CardExpired { .. }
        | OplogEntry::CardDerived { .. }
        | OplogEntry::CardTransferStarted { .. }
        | OplogEntry::CardTransferred { .. }
        | OplogEntry::CardRevokedCascade { .. }
        | OplogEntry::CardTransferConfirmed { .. }
        | OplogEntry::StreamRegistered { .. }
        | OplogEntry::StreamItems { .. }
        | OplogEntry::StreamEnd { .. }
        | OplogEntry::StreamCancel { .. }
        | OplogEntry::StreamSession { .. } => None,
    }
}

fn historical_reconstruction_owner_failure(
    failure: crate::durable_host::tool::operation::OwnerFailureWinner,
) -> WorkerExecutorError {
    match failure {
        crate::durable_host::tool::operation::OwnerFailureWinner::Infrastructure(error) => error,
        crate::durable_host::tool::operation::OwnerFailureWinner::Trap(_) => {
            WorkerExecutorError::runtime(
                "owner failed while waiting for historical entity reconstruction",
            )
        }
        crate::durable_host::tool::operation::OwnerFailureWinner::Lifecycle(kind) => {
            WorkerExecutorError::runtime(format!(
                "owner lifecycle changed while waiting for historical entity reconstruction: {kind:?}"
            ))
        }
    }
}

/// The `start_index` of the durable call `entry` terminates, when `entry` is a durable-call
/// terminal (`End` / `Cancelled`); `None` for every other entry kind.
pub(super) fn terminal_start_index(entry: &OplogEntry) -> Option<OplogIndex> {
    match entry {
        OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
            Some(*start_index)
        }
        OplogEntry::Create { .. }
        | OplogEntry::Start { .. }
        | OplogEntry::CompletionDiscarded { .. }
        | OplogEntry::CompletionDelivered { .. }
        | OplogEntry::AgentInvocationStarted { .. }
        | OplogEntry::AgentInvocationFinished { .. }
        | OplogEntry::Suspend { .. }
        | OplogEntry::Error { .. }
        | OplogEntry::NoOp { .. }
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::BeginAtomicRegion { .. }
        | OplogEntry::EndAtomicRegion { .. }
        | OplogEntry::PendingAgentInvocation { .. }
        | OplogEntry::PendingUpdate { .. }
        | OplogEntry::SuccessfulUpdate { .. }
        | OplogEntry::FailedUpdate { .. }
        | OplogEntry::GrowMemory { .. }
        | OplogEntry::FilesystemStorageUsageUpdate { .. }
        | OplogEntry::CreateResource { .. }
        | OplogEntry::DropResource { .. }
        | OplogEntry::Log { .. }
        | OplogEntry::Restart { .. }
        | OplogEntry::ActivatePlugin { .. }
        | OplogEntry::DeactivatePlugin { .. }
        | OplogEntry::Revert { .. }
        | OplogEntry::CancelPendingInvocation { .. }
        | OplogEntry::StartSpan { .. }
        | OplogEntry::FinishSpan { .. }
        | OplogEntry::SetSpanAttribute { .. }
        | OplogEntry::BeginRemoteTransaction { .. }
        | OplogEntry::PreCommitRemoteTransaction { .. }
        | OplogEntry::PreRollbackRemoteTransaction { .. }
        | OplogEntry::CommittedRemoteTransaction { .. }
        | OplogEntry::RolledBackRemoteTransaction { .. }
        | OplogEntry::Snapshot { .. }
        | OplogEntry::OplogProcessorCheckpoint { .. }
        | OplogEntry::SetRetryPolicy { .. }
        | OplogEntry::RemoveRetryPolicy { .. }
        | OplogEntry::CardEventQueued { .. }
        | OplogEntry::CardInstalled { .. }
        | OplogEntry::CardInstallFailed { .. }
        | OplogEntry::CardRevoked { .. }
        | OplogEntry::CardExpired { .. }
        | OplogEntry::CardDerived { .. }
        | OplogEntry::CardTransferStarted { .. }
        | OplogEntry::CardTransferred { .. }
        | OplogEntry::CardRevokedCascade { .. }
        | OplogEntry::CardTransferConfirmed { .. }
        | OplogEntry::HostStreamFrame { .. }
        | OplogEntry::StreamRegistered { .. }
        | OplogEntry::StreamItems { .. }
        | OplogEntry::StreamEnd { .. }
        | OplogEntry::StreamCancel { .. }
        | OplogEntry::StreamSession { .. } => None,
    }
}
