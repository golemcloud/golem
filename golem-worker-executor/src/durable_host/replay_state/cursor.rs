use super::claims::{StartClaim, recorded_request_payload_matches};
use super::*;

impl ReplayCursor {
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
    pub(super) async fn tx(&self) -> CursorTx<'_> {
        CursorTx {
            cursor: self,
            st: self.state.lock().await,
            notify_progress: false,
        }
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
        self.oplog.read_many(idx, n).await.into_iter().collect()
    }

    pub(super) fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
        let mut hasher = MetroHash128::new();
        hasher.write_u8(level as u8);
        hasher.write(context.as_bytes());
        hasher.write(message.as_bytes());
        hasher.finish128()
    }

    /// Forward-scans the oplog from `start` up to `replay_target`, skipping entries inside deleted
    /// regions, running `end_check`/`for_all_intermediate` (and `update_state`) over the rest. This
    /// is the shared core of the public [`ReplayState::lookup_oplog_entry_with_condition_and_state`]
    /// and the persist-nothing-zone scan in [`CursorTx::should_skip_to`].
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
        replay_target: OplogIndex,
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

        while start < replay_target {
            let entries = self.read_oplog(start, CHUNK_SIZE).await;
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
            start = start.range_end(entries.len() as u64).next();
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
    cursor: &'a ReplayCursor,
    st: MutexGuard<'a, CursorState>,
    notify_progress: bool,
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

            // Snapshot/cache churn can make a cross-layer batch start after the requested index.
            if !self
                .st
                .replay_buffer
                .front()
                .is_some_and(|(idx, _)| *idx == read_idx)
            {
                self.st.replay_buffer = self
                    .cursor
                    .read_oplog(read_idx, 1)
                    .await
                    .into_iter()
                    .collect();
            }
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
        self.try_get_oplog_entry_inner(None, condition).await
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
        self.try_get_oplog_entry_inner(Some(abandoned), condition)
            .await
    }

    pub(super) async fn try_get_oplog_entry_inner(
        &mut self,
        mut abandoned: Option<&mut AbandonedStarts>,
        mut condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        loop {
            if self.cursor.is_live() {
                // No further entries to read: nothing to drain, condition cannot match.
                return Ok(None);
            }

            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;

            if self.is_awaited_terminal(&entry) {
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
    pub(super) fn is_awaited_terminal(&self, entry: &OplogEntry) -> bool {
        terminal_start_index(entry)
            .is_some_and(|start_index| self.st.concurrent_resolver.is_pending(start_index))
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
        self.skip_forward().await?;
        self.cursor
            .position
            .last_replayed_non_hint_index
            .set(read_idx);
        // Committed-consume hook: this entry is now permanently consumed (speculative reads never
        // reach here — they return before committing), so it is safe to feed the concurrent replay
        // resolver.
        self.on_committed_replay_entry(read_idx, entry);
        self.notify_progress = true;
        Ok(())
    }

    /// Skips trailing hint entries (and persist-nothing zones) following the just-committed entry,
    /// recording any log hints, then leaves the cursor on the next non-hint entry without consuming
    /// it.
    pub(super) async fn skip_forward(&mut self) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs: HashMap<(u64, u64), usize> = HashMap::new();
        while self.cursor.is_replay() {
            // Speculative peek: does not advance the published cursor. The cursor is advanced (via
            // `move_replay_idx`) only when a hint / persist-nothing-zone entry is actually skipped
            // past below; the first non-hint entry leaves the cursor untouched, so no speculative
            // position is ever globally observable.
            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;
            match self.should_skip_to(read_idx, &entry).await {
                Some(skip_to) => {
                    // This hint / persist-nothing-zone entry is being permanently consumed, so its
                    // commit-only side effects fire here (they must NOT fire on the rolled-back
                    // probe in the `None` branch below).
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
    /// For hint entries, the next tried oplog index is the next one. When reaching
    /// persist-nothing zones, it points to the end of the zone.
    ///
    /// If the entry is a hint entry, the result is `Some` and contains the current last
    /// read index, so the next read will get the next one.
    /// If the entry is the beginning of a persist-nothing zone, the result will be `Some`
    /// containing the _end_ of the zone so the next read will get the first entry outside
    /// the zone.
    /// If the entry is not a hint entry the result is `None`.
    pub(super) async fn should_skip_to(
        &self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Option<OplogIndex> {
        if entry.is_hint() {
            // Advance to the hint entry itself; the caller publishes this (via `move_replay_idx`) so
            // the next read gets `read_idx.next()`.
            Some(read_idx)
        } else if let OplogEntry::ChangePersistenceLevel {
            persistence_level, ..
        } = &entry
        {
            if persistence_level == &PersistenceLevel::PersistNothing {
                let begin_index = read_idx;
                let cursor = self.cursor;
                // Scan with the transaction's own skip state (no re-lock); see `scan_oplog`.
                let end_index = match cursor
                    .scan_oplog(
                        cursor.last_replayed_index().next(),
                        cursor.replay_target(),
                        &self.st.skipped_regions,
                        self.st.next_skipped_region.clone(),
                        begin_index,
                        |entry, _idx, _state: &()| match entry {
                            OplogEntry::ChangePersistenceLevel {
                                persistence_level, ..
                            } => persistence_level != &PersistenceLevel::PersistNothing,
                            OplogEntry::AgentInvocationFinished { .. } => true,
                            _ => false,
                        },
                        |_, _, _state: &()| true,
                        (),
                        |_, _, _state: &mut ()| {},
                    )
                    .await
                {
                    OplogEntryLookupResult::Found { index, .. } => Some(index),
                    OplogEntryLookupResult::NotFound { .. } => None,
                };

                if let Some(end_index) = end_index {
                    Some(end_index)
                } else {
                    // The zone has not been closed
                    Some(cursor.replay_target())
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Applies the replay side effects of an entry that is being **permanently consumed** at
    /// `read_idx`. Split out of the raw read so it fires only on commit, never on a rolled-back
    /// speculative read. Called for the entry returned to a caller, and for each hint /
    /// persist-nothing-zone entry skipped past in [`Self::skip_forward`].
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
            OplogEntry::CardInstalled { card, .. } => {
                self.record_replay_event(ReplayEvent::CardInstalled { card: card.clone() });
            }
            OplogEntry::CardRevoked { card_id, .. } => {
                self.record_replay_event(ReplayEvent::CardRevoked { card_id: *card_id });
            }
            OplogEntry::CardExpired { card_id, .. } => {
                self.record_replay_event(ReplayEvent::CardExpired { card_id: *card_id });
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
    /// a single [`ReplayEvent::ReplayFinished`] if this advance is the one that crosses the cursor
    /// into live mode.
    ///
    /// This is the single chokepoint for every replay-mode position advance — direct consumption of
    /// the target entry, skipping past trailing hint entries, jumping over a persist-nothing zone,
    /// and jumping over a skipped region (via [`Self::get_out_of_skipped_region`]) all funnel through
    /// here. Detecting the transition here (rather than only when the *consumed* entry index equals
    /// `replay_target`) guarantees `ReplayFinished` is queued on every transition to live, including
    /// when the cursor reaches the target via a skip/jump that never consumes the target entry. The
    /// forced transition in [`Self::switch_to_live`] is the only other path to live and emits its
    /// own `ReplayFinished`.
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
        // Loop: after jumping a region, the freshly looked-up next region may start immediately
        // after the jump target (adjacent regions recorded separately), requiring another jump.
        while self.cursor.is_replay() {
            match &self.st.next_skipped_region {
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
                let resolution = match self.discarded_completion_marker(*start_index) {
                    Some(marker_idx) => Resolution::CompletedButDiscarded {
                        end_idx: idx,
                        marker_idx,
                        response: response.clone(),
                    },
                    None => Resolution::Completed {
                        end_idx: idx,
                        response: response.clone(),
                        forced_commit: *forced_commit,
                    },
                };
                self.st
                    .concurrent_resolver
                    .resolve_if_pending(*start_index, resolution);
            }
            OplogEntry::Cancelled {
                start_index,
                partial,
                ..
            } => {
                self.st.concurrent_resolver.resolve_if_pending(
                    *start_index,
                    Resolution::Cancelled {
                        cancelled_idx: idx,
                        partial: partial.clone(),
                    },
                );
            }
            _ => {}
        }
    }

    /// Returns the index of the `CompletionDiscarded` marker for the durable call starting at
    /// `start_index`, if one exists and lies outside any deleted region (a marker inside a
    /// reverted/jumped-away region belongs to an abandoned timeline, so its `End` — if still
    /// visible — is delivered normally).
    ///
    /// The `discarded_completions` map is populated only from entries at or before the replay
    /// target (the construction scan is bounded by the initial target and target growth rescans
    /// exactly the newly visible range, see [`ReplayState::set_replay_target`]), so a returned
    /// marker never encodes knowledge of oplog entries beyond the target. A target that falls
    /// *between* an `End` and its marker is an invalid replay configuration — the delivery
    /// status of that `End` is not decidable from the visible prefix — and is rejected at
    /// delivery time ([`ReplayState::await_resolution_outcome`]) as well as up front by debug
    /// target validation and cut-point (fork/revert) validation.
    pub(super) fn discarded_completion_marker(
        &self,
        start_index: OplogIndex,
    ) -> Option<OplogIndex> {
        let marker_idx = *self
            .cursor
            .discarded_completions
            .lock()
            .unwrap()
            .get(&start_index)?;
        if !self.st.skipped_regions.is_in_deleted_region(marker_idx) {
            Some(marker_idx)
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

    /// Claims the first not-yet-claimed `Start` entry matching `matches_identity`, registering a
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
    /// divergence (no matching `Start` recorded at all) surfaces as a `NotFound` claim error
    /// instead of an immediate head mismatch.
    pub(super) async fn claim_start_matching(
        &mut self,
        matches_identity: impl Fn(&OplogEntry) -> bool,
        expected: impl FnOnce() -> String,
    ) -> Result<(ReplayCallHandle, Box<OplogEntry>), WorkerExecutorError> {
        // Head fast path: auto-drains awaited terminals and already-claimed `Start`s, then
        // consumes the head iff it matches this claim's identity.
        if let Some((start_idx, entry)) = self.try_get_oplog_entry(&matches_identity).await? {
            let receiver = self.st.concurrent_resolver.register(start_idx);
            // A newly-registered awaiter means an `End`/`Cancelled` already sitting at (or arriving
            // at) the cursor head may now be a drainable awaited terminal: have `finish_tx` wake
            // suspended awaiters so they re-drive the cursor.
            self.notify_progress = true;
            return Ok((ReplayCallHandle::new(start_idx, receiver), Box::new(entry)));
        }

        // The head belongs to someone else: scan ahead for the first not-yet-claimed matching
        // `Start`, skipping deleted regions exactly like the cursor itself would.
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
                        && matches_identity(entry)
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
                self.st.claimed_starts.insert(index);
                let receiver = self.st.concurrent_resolver.register(index);
                self.notify_progress = true;
                Ok((ReplayCallHandle::new(index, receiver), entry))
            }
            OplogEntryLookupResult::NotFound { .. } => {
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    expected(),
                    "no matching Start between the replay cursor and the replay target".to_string(),
                ))
            }
        }
    }

    /// Request-matching counterpart of [`Self::claim_start_matching`]. It scans identity-matching
    /// candidates in oplog order and resolves each recorded payload to a value before claiming it.
    /// Payload resolution is deliberately outside the synchronous scan predicate because an
    /// external payload may require blob I/O.
    pub(super) async fn claim_start_matching_request(
        &mut self,
        matches_identity: impl Fn(&OplogEntry) -> bool,
        expected_request: &HostRequest,
        expected: impl FnOnce() -> String,
    ) -> Result<(ReplayCallHandle, Box<OplogEntry>), WorkerExecutorError> {
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
                            && matches_identity(entry)
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
                let receiver = self.st.concurrent_resolver.register(index);
                self.notify_progress = true;
                return Ok((ReplayCallHandle::new(index, receiver), entry));
            }

            scan_start = index.next();
        }

        Err(WorkerExecutorError::unexpected_oplog_entry(
            expected(),
            "no matching Start between the replay cursor and the replay target".to_string(),
        ))
    }

    /// Claims the `Start` entry described by `claim`: builds the identity predicate from the
    /// typed descriptor and drives the shared claim core ([`Self::claim_start_matching`], or its
    /// request-matching counterpart [`Self::claim_start_matching_request`] when the descriptor
    /// pins the recorded request payload). Returns the registered replay handle together with the
    /// claimed `Start` entry.
    pub(super) async fn claim_start(
        &mut self,
        claim: &StartClaim,
    ) -> Result<(ReplayCallHandle, Box<OplogEntry>), WorkerExecutorError> {
        let matches_identity = |entry: &OplogEntry| {
            matches!(entry, OplogEntry::Start {
                function_name,
                request,
                durable_function_type,
                parent_start_index,
                ..
            } if claim
                .expected_function_name()
                .is_none_or(|expected| function_name == expected)
                && claim
                    .expected_function_type()
                    .is_none_or(|expected| durable_function_type == expected)
                && request.is_some() == claim.carries_request()
                && *parent_start_index == claim.expected_parent_start_index())
        };
        let expected = || claim.expected_description();
        let (handle, entry) = match claim.matching_request() {
            Some(expected_request) => {
                self.claim_start_matching_request(matches_identity, expected_request, expected)
                    .await?
            }
            None => {
                self.claim_start_matching(matches_identity, expected)
                    .await?
            }
        };
        // Every `Start` claim registers a resolver awaiter atomically with the consume/claim, so
        // its terminal is always a resolver-routed *awaited terminal* — never an orphan a parked
        // awaiter behind it could sleep on until `switch_to_live`. The only un-drained terminals
        // the cursor may leave at its head are the dedicated-positional-consumer pairs (manual
        // durability, `GolemApiFork`).
        debug_assert!(
            self.st.concurrent_resolver.is_pending(handle.start_idx()),
            "Start claim at {} must leave a registered awaiter",
            handle.start_idx()
        );
        Ok((handle, entry))
    }

    /// Switches the cursor to live mode: records `ReplayFinished` if replay was still in progress,
    /// clamps the cursor head to the replay target, and wakes every still-suspended awaiter with
    /// `Incomplete` (any durable call whose `Start` was committed but whose terminal never was).
    pub(super) fn switch_to_live(&mut self) {
        if !self.cursor.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished);
        }
        self.cursor
            .position
            .last_replayed_index
            .set(self.cursor.replay_target());
        // Replay is over: any durable call whose `Start` was committed but whose terminal never was
        // is incomplete. Wake every still-suspended awaiter so it returns `Incomplete` instead of
        // sleeping forever waiting for a cursor that will not advance again.
        self.st.concurrent_resolver.fail_all_pending_incomplete();
        // Scan-ahead-claimed `Start`s the cursor never reached are moot now: their awaiters were
        // just failed with `Incomplete`, and the cursor will not read again.
        self.st.claimed_starts.clear();
        self.st.replay_buffer.clear();
        self.notify_progress = true;
    }

    /// Resets the cursor to the start of replay after dropping a manual-update override.
    pub(super) async fn drop_override_and_restart(&mut self) -> Result<(), WorkerExecutorError> {
        self.st.skipped_regions.drop_override();
        let next = self
            .st
            .skipped_regions
            .find_next_deleted_region(OplogIndex::NONE);
        self.st.next_skipped_region = next;
        self.cursor.set_log_hashes(HashMap::new());
        self.cursor.pending_replay_events.lock().unwrap().clear();
        self.st.claimed_starts.clear();
        self.st.replay_buffer.clear();
        self.cursor
            .position
            .last_replayed_index
            .set(OplogIndex::NONE);
        self.cursor
            .position
            .last_replayed_non_hint_index
            .set(OplogIndex::NONE);
        self.move_replay_idx(OplogIndex::INITIAL).await;
        self.skip_forward().await
    }
}

impl ReplayState {
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        skipped_regions: DeletedRegions,
    ) -> Result<Self, WorkerExecutorError> {
        let next_skipped_region = skipped_regions.find_next_deleted_region(OplogIndex::NONE);
        let last_oplog_index = oplog.current_oplog_index().await;
        let discarded_completions =
            Self::scan_discarded_completions(&oplog, OplogIndex::INITIAL, last_oplog_index).await?;
        let cursor = ReplayCursor {
            owned_agent_id,
            oplog,
            position: PublishedPosition {
                last_replayed_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
                last_replayed_non_hint_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
                has_seen_logs: AtomicBool::new(false),
            },
            replay_target: AtomicOplogIndex::from_oplog_index(last_oplog_index),
            state: Mutex::new(CursorState {
                skipped_regions,
                next_skipped_region,
                replay_buffer: VecDeque::new(),
                pending_fork_starts: HashSet::new(),
                concurrent_resolver: ConcurrentReplayResolver::default(),
                claimed_starts: HashSet::new(),
            }),
            discarded_completions: std::sync::Mutex::new(discarded_completions),
            log_hashes: std::sync::Mutex::new(HashMap::new()),
            pending_replay_events: std::sync::Mutex::new(Vec::new()),
            progress: Notify::new(),
        };
        {
            // No concurrency during construction: the replay state is not shared yet, so driving the
            // cursor without anyone to notify is sound.
            let mut tx = cursor.tx().await;
            tx.move_replay_idx(OplogIndex::INITIAL).await; // By this we handle initial skipped regions applied by manual updates correctly
            tx.skip_forward().await?;
        }
        Ok(Self {
            cursor: Arc::new(cursor),
        })
    }

    /// Scans the oplog range `[from, to]` for `CompletionDiscarded` marker entries, building the
    /// `Start`-index → marker-index map consulted when an `End` is resolved during replay. Used
    /// with `[INITIAL, initial replay target]` at construction and with exactly the newly visible
    /// range when the replay target grows ([`ReplayState::set_replay_target`]). Two markers
    /// referencing the same `Start` within the scanned range is oplog corruption and fails the
    /// scan.
    pub(super) async fn scan_discarded_completions(
        oplog: &Arc<dyn Oplog>,
        from: OplogIndex,
        to: OplogIndex,
    ) -> Result<HashMap<OplogIndex, OplogIndex>, WorkerExecutorError> {
        const CHUNK_SIZE: u64 = 1024;
        let mut discarded = HashMap::new();
        let mut next = from;
        while next <= to {
            let available = u64::from(to) - u64::from(next) + 1;
            let entries = oplog.read_many(next, CHUNK_SIZE.min(available)).await;
            let Some(last_read) = entries.keys().next_back().copied() else {
                break;
            };
            for (marker_idx, entry) in entries {
                if marker_idx > to {
                    break;
                }
                if let OplogEntry::CompletionDiscarded { start_index, .. } = entry
                    && discarded.insert(start_index, marker_idx).is_some()
                {
                    return Err(WorkerExecutorError::runtime(format!(
                        "corrupt oplog: multiple CompletionDiscarded markers reference the durable call Start at {start_index} (second marker at {marker_idx})"
                    )));
                }
            }
            next = last_read.next();
        }
        Ok(discarded)
    }

    /// Records a live-appended `CompletionDiscarded` marker: the durable call starting at
    /// `start_index` persisted a successful `End`, but the guest dropped the completion future
    /// before the response was delivered, and the marker was appended at `marker_index`. If this
    /// instance later re-enters replay over these entries (e.g. a manual-update restart), the
    /// recorded `End` must park instead of delivering the response.
    pub fn record_discarded_completion(&self, start_index: OplogIndex, marker_index: OplogIndex) {
        let previous = self
            .cursor
            .discarded_completions
            .lock()
            .unwrap()
            .insert(start_index, marker_index);
        if let Some(previous) = previous {
            tracing::warn!(
                "duplicate CompletionDiscarded marker recorded for durable call Start {start_index}: previous at {previous}, new at {marker_index}"
            );
        }
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
    pub(super) async fn with_tx<R>(&self, op: impl AsyncFnOnce(&mut CursorTx<'_>) -> R) -> R {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
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

    pub async fn switch_to_live(&self) {
        let result = self
            .run_owned_cursor_op(|state| async move {
                state.with_tx(async |tx| tx.switch_to_live()).await;
                // `CursorTx::switch_to_live` publishes the cursor position directly (not via
                // `move_replay_idx`), so replay-progress observers are notified here.
                state
                    .cursor
                    .oplog
                    .on_replay_progress(state.cursor.last_replayed_index())
                    .await;
                Ok(())
            })
            .await;
        if let Err(err) = result {
            warn!("switch_to_live cursor operation did not complete: {err}");
        }
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

    /// Sets the replay target. This is a phase-boundary operation (e.g. refreshing the target
    /// before replay resumes); it must not race with concurrent cursor advances.
    ///
    /// The discarded-completion map is kept in sync with the visible prefix `[.., target]`:
    ///
    /// - Growing the target makes a previously invisible oplog range visible, so the newly
    ///   visible range `(old_target, new_target]` is scanned for `CompletionDiscarded` markers
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
                        .discarded_completions
                        .lock()
                        .unwrap()
                        .retain(|_, marker_idx| *marker_idx <= new_target);
                }
                std::cmp::Ordering::Greater => {
                    let additions = Self::scan_discarded_completions(
                        &cursor.oplog,
                        old_target.next(),
                        new_target,
                    )
                    .await?;
                    if !additions.is_empty() {
                        let mut discarded = cursor.discarded_completions.lock().unwrap();
                        for (start_index, marker_idx) in &additions {
                            // Rediscovering the exact marker already in the map (recorded live by
                            // this instance via `record_discarded_completion` before the target
                            // grew over it) is idempotent; only a *different* marker for the same
                            // `Start` is oplog corruption.
                            if let Some(previous) = discarded.get(start_index)
                                && previous != marker_idx
                            {
                                return Err(WorkerExecutorError::runtime(format!(
                                    "corrupt oplog: multiple CompletionDiscarded markers reference the durable call Start at {start_index} (previous at {previous}, second marker at {marker_idx})"
                                )));
                            }
                        }
                        discarded.extend(additions);
                    }
                }
            }
            cursor.replay_target.set(new_target);
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

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        self.cursor.is_replay()
    }

    pub fn take_new_replay_events(&self) -> Vec<ReplayEvent> {
        std::mem::take(&mut *self.cursor.pending_replay_events.lock().unwrap())
    }

    /// Whether some task currently holds an open cursor transaction ([`ReplayCursor::tx`]).
    ///
    /// The invocation event loop can exit while a store-spawned durable task is suspended
    /// mid-transaction (a transaction awaits oplog reads); such a task is not polled again until
    /// the next event loop runs, so the fair cursor lock it holds would block every cursor read
    /// issued from outside the event loop. The invocation completion path polls the event loop
    /// until this reports `false` before any such read.
    pub fn has_open_cursor_transaction(&self) -> bool {
        self.cursor.state.try_lock().is_err()
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    ///
    /// Returns an error if the underlying read fails (e.g. missing oplog entry,
    /// corrupted GolemApiFork payload) so the worker can fail the agent with a
    /// non-retriable trap rather than panicking the executor.
    pub async fn get_oplog_entry(&self) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        // The closure always returns true, so the only `None` case is end-of-replay (a positional
        // reader expecting an entry that the oplog does not contain).
        self.with_tx(async |tx| tx.try_get_oplog_entry(|_| true).await)
            .await?
            .ok_or_else(|| self.end_of_replay_error())
    }

    /// Reads the next oplog entry, and if it matches the given condition, skips
    /// every hint entry following it and returns the oplog index of the entry read.
    /// If the condition is not met, returns `None` and the candidate entry is left unconsumed with
    /// the cursor, skipped-region state, and side effects untouched. (Any *awaited terminals* sitting
    /// ahead of the candidate are drained to their awaiters first — see
    /// [`CursorTx::try_get_oplog_entry`] — and those drains stay committed.)
    ///
    /// The auto-skipped hint entries can be of two kind:
    /// - A set of oplog entry cases are always hint entries. They manipulate the worker status
    ///   but are non-deterministic from the replay's point of view.
    /// - Every oplog entry recorded in persist-nothing zones. These are there for observability,
    ///   but they never participate in the replay. A persist-nothing zone is bounded by two
    ///   ChangePersistenceLevel entries, or if the closing one is missing, it is up to the end of the
    ///   oplog.
    pub async fn try_get_oplog_entry(
        &self,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.with_tx(async |tx| tx.try_get_oplog_entry(condition).await)
            .await
    }

    /// [`Self::get_oplog_entry`] variant for callers running inside Wasmtime accessor futures:
    /// the cursor transaction runs on an owned task (see [`Self::run_owned_cursor_op`]), so the
    /// store-polled caller never queues on the cursor mutex directly. Direct invocation-loop /
    /// p2 host-call readers keep using [`Self::get_oplog_entry`].
    pub async fn get_oplog_entry_owned(
        &self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            state
                .with_tx(async |tx| tx.try_get_oplog_entry(|_| true).await)
                .await?
                .ok_or_else(|| state.end_of_replay_error())
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
        // must never queue on the cursor mutex directly. On task cancellation (runtime shutdown)
        // the conservative `NotFound { violates_for_all: true }` answer is returned: callers
        // treat it as "cannot prove the scope completed cleanly" and fail the operation rather
        // than fabricating success.
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
                cursor.replay_target(),
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
                let (_, oplog_entry) = self.get_oplog_entry().await?;
                match oplog_entry {
                    OplogEntry::AgentInvocationStarted {
                        idempotency_key,
                        payload,
                        trace_id,
                        trace_states,
                        invocation_context: spans,
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
                            idempotency_key,
                            invocation_payload,
                            invocation_context,
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
        self.with_tx(async |tx| {
            tx.try_get_oplog_entry_at_invocation_boundary(abandoned, |_| true)
                .await
        })
        .await?
        .ok_or_else(|| self.end_of_replay_error())
    }
}

/// The `start_index` of the durable call `entry` terminates, when `entry` is a durable-call
/// terminal (`End` / `Cancelled`); `None` for every other entry kind.
pub(super) fn terminal_start_index(entry: &OplogEntry) -> Option<OplogIndex> {
    match entry {
        OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
            Some(*start_index)
        }
        _ => None,
    }
}
