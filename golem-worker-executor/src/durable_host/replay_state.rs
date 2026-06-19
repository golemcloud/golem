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

use crate::durable_host::concurrent::{
    ConcurrentReplayResolver, ReplayCallHandle, Resolution, ResolutionOutcome,
};
use crate::services::oplog::{Oplog, OplogOps};
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    AtomicOplogIndex, DurableFunctionType, HostResponse, HostResponseGolemApiFork, LogLevel,
    OplogEntry, OplogIndex, PersistenceLevel,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentInvocationPayload, AgentInvocationResult, ForkResult, IdempotencyKey, OwnedAgentId,
    Timestamp,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use metrohash::MetroHash128;
use std::collections::HashSet;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;
use tokio::sync::{Mutex, MutexGuard, Notify};
use tracing::debug;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ReplayEvent {
    ReplayFinished,
    UpdateReplayed { new_revision: ComponentRevision },
    ForkReplayed { new_phantom_id: Uuid },
}

#[derive(Debug, Clone)]
pub struct AgentInvocationStartedEntry {
    pub idempotency_key: IdempotencyKey,
    pub invocation_payload: AgentInvocationPayload,
    pub invocation_context: InvocationContextStack,
}

/// The outcome of [`ReplayState::claim_any_concurrent_start`]: the replay handle for the claimed
/// call plus the identity (`function_name`, `durable_function_type`, `timestamp`) read from its
/// `Start` entry. Callers that knew the identity up front use [`ReplayState::claim_concurrent_start`]
/// and discard this; the dynamic guest-durability read uses these fields to reconstruct the
/// persisted invocation it returns to the guest.
pub struct ClaimedConcurrentStart {
    pub handle: ReplayCallHandle,
    pub function_name: HostFunctionName,
    pub durable_function_type: DurableFunctionType,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone)]
pub struct ReplayState {
    cursor: Arc<ReplayCursor>,
}

/// The shared replay cursor: the position reached in the oplog, the target it replays up to, and
/// every piece of state mutated while advancing it.
///
/// Advancing the cursor is a multi-`await` read-modify-commit transaction (oplog reads, payload
/// downloads), not a single CAS, so it is serialized by a single lock: [`Self::state`]. That lock
/// is the *only* gateway to mutate the cursor — there is no separate marker mutex. Acquire it via
/// [`Self::tx`], which returns a [`CursorTx`]; all advance/mutation logic lives on `CursorTx`, so
/// "what the cursor lock protects" is exactly "everything reachable through a `CursorTx`".
///
/// Hot, read-only position queries (`is_live`, `last_replayed_index`, …) read [`Self::position`]
/// and [`Self::replay_target`] without taking the lock. Those atomics are written *exclusively*
/// from inside a `CursorTx` (i.e. while `state` is held), so a lock-free read always observes a
/// committed advance, never a half-applied one. `replay_target` is the one exception: it is set at
/// phase boundaries (`set_replay_target`, before replay resumes) rather than during an advance.
#[derive(Debug)]
struct ReplayCursor {
    owned_agent_id: OwnedAgentId,
    oplog: Arc<dyn Oplog>,
    /// Published cursor position. Lock-free reads; writes happen only through a held [`CursorTx`].
    position: PublishedPosition,
    /// The oplog index replay runs up to. Read lock-free everywhere; set at phase boundaries via
    /// [`ReplayState::set_replay_target`] (and clamped to the current head when switching to live),
    /// not as part of a cursor-advance transaction.
    replay_target: AtomicOplogIndex,
    /// The cursor lock. Guards every piece of state a cursor-advance transaction touches, including
    /// the concurrent-replay resolver. Held across the transaction's internal `await`s (oplog reads,
    /// payload downloads), but never while a call is parked on its resolution / cursor progress — an
    /// awaiter releases it before sleeping (see [`ReplayState::await_resolution_outcome`]) — and no
    /// operation performed while it is held re-acquires it.
    state: Mutex<CursorState>,
    /// Fired (via `notify_waiters`) after a transaction that advanced the cursor, registered a
    /// resolver awaiter, or switched to live commits and releases [`Self::state`]. A durable call
    /// suspended in [`ReplayState::await_resolution_outcome`] — because its `End`/`Cancelled` is not
    /// yet at the cursor head while a concurrently-replaying sibling owns it — wakes on this to
    /// re-drive the cursor. Resolver delivery is the primary wakeup; this covers the "another
    /// consumer advanced the cursor past my blocker" case that a oneshot alone cannot.
    progress: Notify,
}

/// The published, lock-free-readable cursor position. Every field is written only while
/// [`ReplayCursor::state`] is held (through a [`CursorTx`], or — for `has_seen_logs` — while the
/// same lock is held in [`ReplayState::remove_seen_log`]), so a lock-free reader never observes a
/// partially-applied advance.
#[derive(Debug)]
struct PublishedPosition {
    /// The oplog index of the last replayed entry.
    last_replayed_index: AtomicOplogIndex,
    /// The oplog index of the last non-hint entry read.
    last_replayed_non_hint_index: AtomicOplogIndex,
    /// Fast-path flag for [`ReplayState::seen_log`]: whether any log hint was recorded since the
    /// last non-hint entry, so the common "no logs" case avoids locking.
    has_seen_logs: AtomicBool,
}

/// The mutable state a cursor-advance transaction owns. Reachable only by locking
/// [`ReplayCursor::state`]; this is the single thing the "cursor lock" protects.
#[derive(Debug)]
struct CursorState {
    skipped_regions: DeletedRegions,
    next_skipped_region: Option<OplogRegion>,
    /// Hashes of log entries persisted since the last read non-hint oplog entry.
    log_hashes: HashSet<(u64, u64)>,
    /// Updates that were encountered while reading the oplog.
    pending_replay_events: Vec<ReplayEvent>,
    /// `Start` entries for `GolemApiFork` whose matching `End` has not yet been replayed. When the
    /// matching `End` is read, the response is decoded and a `ForkReplayed` event is emitted. The
    /// legacy adapter only ever has at most one in flight at a time (it writes the matched `End`
    /// immediately after the `Start`), but we use a set so that future concurrent recorders cannot
    /// trip us up.
    pending_fork_starts: HashSet<OplogIndex>,
    /// Matches replayed `End`/`Cancelled` entries to the concurrent
    /// [`crate::durable_host::concurrent::CallHandle`]s awaiting them, keyed by their `Start` index.
    /// Fed only from the committed-consume hook. Lives under the cursor lock because awaited-terminal
    /// detection, terminal resolution, and `Start`-claim registration are all part of the cursor
    /// transaction; the rare slow-path `unregister` re-acquires the lock from outside a transaction.
    concurrent_resolver: ConcurrentReplayResolver,
}

impl ReplayCursor {
    /// Begins a cursor-advance transaction by acquiring [`Self::state`]. The returned [`CursorTx`]
    /// is the sole gateway to advance the cursor or mutate the guarded state.
    async fn tx(&self) -> CursorTx<'_> {
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
    fn finish_tx(&self, tx: CursorTx<'_>) {
        let notify = tx.notify_progress;
        drop(tx);
        if notify {
            self.progress.notify_waiters();
        }
    }

    fn last_replayed_index(&self) -> OplogIndex {
        self.position.last_replayed_index.get()
    }

    fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.position.last_replayed_non_hint_index.get()
    }

    fn replay_target(&self) -> OplogIndex {
        self.replay_target.get()
    }

    fn is_live(&self) -> bool {
        self.last_replayed_index() == self.replay_target()
    }

    fn is_replay(&self) -> bool {
        !self.is_live()
    }

    async fn read_oplog(&self, idx: OplogIndex, n: u64) -> Vec<(OplogIndex, OplogEntry)> {
        self.oplog.read_many(idx, n).await.into_iter().collect()
    }

    fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
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
    async fn scan_oplog<State>(
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
                    // If we are in the current skip region, ignore the entry
                    continue;
                }
                if current_next_skip_region
                    .as_ref()
                    .map(|r| &r.end == idx)
                    .unwrap_or(false)
                {
                    // if we are at the end of the current skip region, find the next one
                    current_next_skip_region = skipped_regions.find_next_deleted_region(idx.next());
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
struct CursorTx<'a> {
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
    async fn raw_read_next_oplog_entry(
        &self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        let read_idx = self.cursor.last_replayed_index().next();

        let oplog_entries = self.cursor.read_oplog(read_idx, 1).await;
        let oplog_entry = if let Some((_, oplog_entry)) = oplog_entries.into_iter().next() {
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
    ///
    /// On the first non-drainable entry (a non-terminal, or an `End`/`Cancelled` nobody awaits):
    /// - if `condition` matches, it is committed and returned;
    /// - otherwise `None` is returned. The speculative read advanced nothing observable (the cursor
    ///   is published only on commit), so there is nothing to roll back. The auto-drained terminals
    ///   stay committed — that is the correct contract under concurrent replay: draining another
    ///   call's terminal is real progress even when this caller's own predicate then fails.
    async fn try_get_oplog_entry(
        &mut self,
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

            if condition(&entry) {
                self.commit_consumed_entry(read_idx, &entry).await?;
                return Ok(Some((read_idx, entry)));
            } else {
                // Predicate failed: the speculative read published nothing, so the cursor,
                // skipped-region state, and side effects are already untouched.
                return Ok(None);
            }
        }
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` currently has a registered
    /// resolver awaiter (and is therefore an *awaited terminal* the cursor auto-drains to its owner
    /// rather than handing to a positional reader).
    fn is_awaited_terminal(&self, entry: &OplogEntry) -> bool {
        let start_index = match entry {
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                *start_index
            }
            _ => return false,
        };
        self.st.concurrent_resolver.is_pending(start_index)
    }

    /// Commits a just-read entry: apply its commit-only side effects, publish the cursor advance,
    /// skip any trailing hint entries, advance the non-hint marker, route it to the concurrent
    /// resolver, and mark that cursor progress should be signalled once the lock is released.
    async fn commit_consumed_entry(
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
    async fn skip_forward(&mut self) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs = HashSet::new();
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
                        logs.insert(hash);
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

        self.cursor
            .position
            .has_seen_logs
            .store(!logs.is_empty(), Ordering::Relaxed);
        self.st.log_hashes = logs;
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
    async fn should_skip_to(&self, read_idx: OplogIndex, entry: &OplogEntry) -> Option<OplogIndex> {
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
    async fn apply_commit_effects(
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
        // The legacy adapter persists GolemApiFork as a matched
        // `Start { function_name: GolemApiFork, .. }` + `End { response: Some(..), .. }`
        // pair. On Start we remember the `Start`'s `OplogIndex`, on the matching
        // End (via `start_index`) we decode the response and emit `ForkReplayed`
        // if necessary.
        match oplog_entry {
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
    async fn move_replay_idx(&mut self, new_idx: OplogIndex) {
        let was_replay = self.cursor.is_replay();
        self.cursor.position.last_replayed_index.set(new_idx);
        self.get_out_of_skipped_region().await;
        if was_replay && self.cursor.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished);
        }
    }

    async fn get_out_of_skipped_region(&mut self) {
        if self.cursor.is_replay() {
            let update_next_skipped_region = match &self.st.next_skipped_region {
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

                    true
                }
                _ => false,
            };

            if update_next_skipped_region {
                let next = self
                    .st
                    .skipped_regions
                    .find_next_deleted_region(self.cursor.last_replayed_index());
                self.st.next_skipped_region = next;
            }
        }
    }

    /// Feeds the concurrent replay resolver when an `End`/`Cancelled` entry is *committed*
    /// (permanently consumed). Resolves only calls that are actually being awaited
    /// (`resolve_if_pending`), so the `End`/`Cancelled` of any call not tracked by the resolver —
    /// e.g. the guest-facing manual durability pair, consumed through this same cursor but never
    /// registered — is ignored instead of leaking.
    fn on_committed_replay_entry(&mut self, idx: OplogIndex, entry: &OplogEntry) {
        match entry {
            OplogEntry::End {
                start_index,
                response,
                forced_commit,
                ..
            } => {
                self.st.concurrent_resolver.resolve_if_pending(
                    *start_index,
                    Resolution::Completed {
                        end_idx: idx,
                        response: response.clone(),
                        forced_commit: *forced_commit,
                    },
                );
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

    fn record_replay_event(&mut self, event: ReplayEvent) {
        self.st.pending_replay_events.push(event);
    }

    /// Positionally claims the next `Start` entry for a durable call **without** validating its
    /// function name or durable function type, registering a resolver receiver keyed by the
    /// `Start`'s index and returning the claimed entry's identity.
    ///
    /// The `Start` consume and the resolver registration happen **atomically** within this
    /// transaction (under the cursor lock). This is required for concurrent replay: if the cursor
    /// advanced past the `Start` before the awaiter was registered, this call's `End` arriving at
    /// the head in that window would not be recognised as an awaited terminal and could be wrongly
    /// consumed by a positional reader.
    async fn claim_any_concurrent_start(
        &mut self,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let read = self
            .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::Start { .. }))
            .await?;
        let (start_idx, entry) = read.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "Start",
                "a non-Start entry (end of replay, or concurrent interleaving)".to_string(),
            )
        })?;
        let OplogEntry::Start {
            timestamp,
            function_name,
            request,
            durable_function_type,
            ..
        } = entry
        else {
            unreachable!("try_get_oplog_entry condition guarantees a Start entry");
        };
        if request.is_none() {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "Start { request: Some(..) }",
                "Start { request: None }".to_string(),
            ));
        }
        let receiver = self.st.concurrent_resolver.register(start_idx);
        // A newly-registered awaiter means an `End`/`Cancelled` already sitting at (or arriving at)
        // the cursor head may now be a drainable awaited terminal: have `finish_tx` wake suspended
        // awaiters so they re-drive the cursor.
        self.notify_progress = true;
        Ok(ClaimedConcurrentStart {
            handle: ReplayCallHandle::new(start_idx, receiver),
            function_name,
            durable_function_type,
            timestamp,
        })
    }

    /// Switches the cursor to live mode: records `ReplayFinished` if replay was still in progress,
    /// clamps the cursor head to the replay target, and wakes every still-suspended awaiter with
    /// `Incomplete` (any durable call whose `Start` was committed but whose terminal never was).
    fn switch_to_live(&mut self) {
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
        self.notify_progress = true;
    }

    /// Resets the cursor to the start of replay after dropping a manual-update override.
    async fn drop_override_and_restart(&mut self) -> Result<(), WorkerExecutorError> {
        self.st.skipped_regions.drop_override();
        let next = self
            .st
            .skipped_regions
            .find_next_deleted_region(OplogIndex::NONE);
        self.st.next_skipped_region = next;
        self.st.log_hashes.clear();
        self.st.pending_replay_events.clear();
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
                log_hashes: HashSet::new(),
                pending_replay_events: Vec::new(),
                pending_fork_starts: HashSet::new(),
                concurrent_resolver: ConcurrentReplayResolver::default(),
            }),
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

    pub async fn drop_override_and_restart(&self) -> Result<(), WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        let result = tx.drop_override_and_restart().await;
        cursor.finish_tx(tx);
        result
    }

    pub async fn switch_to_live(&self) {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        tx.switch_to_live();
        cursor.finish_tx(tx);
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

    /// Sets the replay target. This is a phase-boundary operation (e.g. refreshing the target before
    /// replay resumes), not part of a cursor-advance transaction, so it is a lock-free atomic store;
    /// it must not race with concurrent cursor advances.
    pub fn set_replay_target(&self, new_target: OplogIndex) {
        self.cursor.replay_target.set(new_target)
    }

    pub async fn is_in_skipped_region(&self, oplog_index: OplogIndex) -> bool {
        let st = self.cursor.state.lock().await;
        st.skipped_regions.is_in_deleted_region(oplog_index)
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.cursor.is_live()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        self.cursor.is_replay()
    }

    pub async fn take_new_replay_events(&self) -> Vec<ReplayEvent> {
        std::mem::take(&mut self.cursor.state.lock().await.pending_replay_events)
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    ///
    /// Returns an error if the underlying read fails (e.g. missing oplog entry,
    /// corrupted GolemApiFork payload) so the worker can fail the agent with a
    /// non-retriable trap rather than panicking the executor.
    pub async fn get_oplog_entry(&self) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        // The closure always returns true, so the only `None` case is end-of-replay (a positional
        // reader expecting an entry that the oplog does not contain).
        let result = tx.try_get_oplog_entry(|_| true).await;
        cursor.finish_tx(tx);
        result?.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "next oplog entry to replay",
                format!(
                    "end of replay for {} at index {}; replay target = {}",
                    cursor.owned_agent_id,
                    cursor.last_replayed_index(),
                    cursor.replay_target(),
                ),
            )
        })
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
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        let result = tx.try_get_oplog_entry(condition).await;
        cursor.finish_tx(tx);
        result
    }

    /// Returns true if the given log entry has been seen since the last non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        if self.cursor.position.has_seen_logs.load(Ordering::Relaxed) {
            let hash = ReplayCursor::hash_log_entry(level, context, message);
            let st = self.cursor.state.lock().await;
            st.log_hashes.contains(&hash)
        } else {
            false
        }
    }

    /// Removes a seen log from the set. If the set becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&self, level: LogLevel, context: &str, message: &str) {
        let hash = ReplayCursor::hash_log_entry(level, context, message);
        let mut st = self.cursor.state.lock().await;
        st.log_hashes.remove(&hash);
        // Written while the cursor lock is held, preserving the invariant that `has_seen_logs` is
        // only ever stored under `state`.
        self.cursor
            .position
            .has_seen_logs
            .store(!st.log_hashes.is_empty(), Ordering::Relaxed);
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
        let (start, skipped_regions, next_skipped_region) = {
            let st = cursor.state.lock().await;
            (
                cursor.last_replayed_index().next(),
                st.skipped_regions.clone(),
                st.next_skipped_region.clone(),
            )
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
        loop {
            if self.is_replay() {
                let (_, oplog_entry) = self.get_oplog_entry().await?;
                match oplog_entry {
                    OplogEntry::AgentInvocationFinished { result, .. } => {
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

    /// Positionally claims the next `Start` entry for a durable call, validates its identity
    /// (function name, durable function type, request presence) and registers a resolver receiver
    /// keyed by the `Start`'s index.
    ///
    /// Claiming by position is sound because `Start` order is deterministic, even though
    /// `End`/`Cancelled` order is not. A `Start` is appended eagerly when the guest *initiates* a
    /// call, so the order of `Start` entries is the order in which the guest issued calls — and the
    /// guest's control flow is itself made deterministic by replay (every host result is delivered
    /// in the recorded order). So during replay the guest re-issues calls in the same order, and
    /// the n-th `claim_concurrent_start` always lands on the n-th `Start`. By contrast
    /// `End`/`Cancelled` are appended when a call *completes*, whose order reflects I/O and async
    /// scheduling and is therefore not reproducible — which is exactly why those are matched back
    /// to their awaiter by `start_index` (the resolver) instead of by position.
    ///
    /// `End` entries carry no function identity, so validation must happen here, at claim time.
    /// The request payload is not decoded: `function_name` already pins the request type (and the
    /// `Req` associated type has no `TryFrom<HostRequest>` to decode it generically); the response
    /// is fully type-checked on the `End` side during replay.
    pub async fn claim_concurrent_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let claimed = self.claim_any_concurrent_start().await?;
        let validation_error = if &claimed.function_name != expected_function_name {
            Some(WorkerExecutorError::unexpected_oplog_entry(
                format!("Start {{ function_name: {expected_function_name} }}"),
                format!("Start {{ function_name: {} }}", claimed.function_name),
            ))
        } else if &claimed.durable_function_type != expected_function_type {
            Some(WorkerExecutorError::unexpected_oplog_entry(
                format!("Start {{ durable_function_type: {expected_function_type:?} }}"),
                format!(
                    "Start {{ durable_function_type: {:?} }}",
                    claimed.durable_function_type
                ),
            ))
        } else {
            None
        };
        if let Some(err) = validation_error {
            // `claim_any_concurrent_start` already registered a resolver receiver for this `Start`;
            // drop it on validation failure so it cannot be matched by a later resolution.
            self.unregister_awaiter(claimed.handle.start_idx()).await;
            return Err(err);
        }
        Ok(claimed.handle)
    }

    /// Positionally claims the next `Start` entry for a durable call **without** validating its
    /// function name or durable function type, registering a resolver receiver keyed by the
    /// `Start`'s index and returning the claimed entry's identity for the caller to inspect.
    ///
    /// This is the dynamic counterpart of [`Self::claim_concurrent_start`]: it is used by callers
    /// that learn the call identity from the claimed entry itself rather than knowing it up front —
    /// notably the guest-facing `golem::durability` read, which returns the persisted invocation's
    /// function name to the guest and therefore has no expected name to validate against.
    ///
    /// The `Start` consume and the resolver registration happen atomically under the cursor lock;
    /// see [`CursorTx::claim_any_concurrent_start`].
    pub async fn claim_any_concurrent_start(
        &self,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        let result = tx.claim_any_concurrent_start().await;
        cursor.finish_tx(tx);
        result
    }

    /// Drops a resolver awaiter from outside a cursor transaction. Acquires the cursor lock briefly;
    /// callers must not hold it (the await loop releases it before parking).
    async fn unregister_awaiter(&self, start_idx: OplogIndex) {
        let mut st = self.cursor.state.lock().await;
        st.concurrent_resolver.unregister(start_idx);
    }

    /// Drains every *awaited terminal* (`End`/`Cancelled` whose `start_index` has a registered
    /// awaiter) currently at the cursor head, routing each to its awaiter, then stops at the first
    /// non-terminal entry without consuming it. This is the cursor-driving half of
    /// [`Self::await_resolution_outcome`]; it never blocks (it parks by returning, not suspending).
    async fn drain_awaited_terminals(&self) -> Result<(), WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        // `|_| false` never matches a non-terminal, so the transaction only auto-drains the awaited
        // terminals at the head and then returns `None` on the first non-terminal entry (or at
        // end-of-replay) without consuming it.
        let result = tx.try_get_oplog_entry(|_| false).await;
        cursor.finish_tx(tx);
        result.map(|_| ())
    }

    /// Awaits the resolution of the call identified by `handle`, treating end-of-replay as a hard
    /// error (the caller requires the call to have completed in the oplog).
    pub async fn await_resolution(
        &self,
        handle: ReplayCallHandle,
    ) -> Result<Resolution, WorkerExecutorError> {
        let start_idx = handle.start_idx();
        match self.await_resolution_outcome(handle).await? {
            ResolutionOutcome::Resolved(resolution) => Ok(resolution),
            ResolutionOutcome::Incomplete => Err(WorkerExecutorError::unexpected_oplog_entry(
                "End or Cancelled",
                format!(
                    "end of replay: durable call Start at {start_idx} has no matching End/Cancelled"
                ),
            )),
        }
    }

    /// Awaits the resolution of the call identified by `handle`, reporting a lone committed `Start`
    /// (replay reached the end of the oplog without the matching `End`/`Cancelled`) as
    /// [`ResolutionOutcome::Incomplete`] rather than a hard error, so the caller can decide whether
    /// to re-execute the call.
    ///
    /// This is the genuine concurrent-replay suspend/resume path. The awaiter does not drive the
    /// cursor toward its own `End` directly; instead it repeatedly:
    /// 1. drains the awaited terminals at the cursor head ([`Self::drain_awaited_terminals`]) —
    ///    resolving this call (when its `End` is at the head) and, in the interleaved case, routing
    ///    earlier-completing siblings' terminals to their own awaiters;
    /// 2. checks its receiver;
    /// 3. if still unresolved and replay is not over, **suspends** until either its resolution
    ///    arrives (a concurrently-replaying sibling drove the cursor to this call's `End`) or the
    ///    cursor advances (a positional consumer or a sibling claim made progress past the blocker),
    ///    then loops.
    ///
    /// Cursor progress is registered (`Notified::enable`) *before* the cursor is inspected, so a
    /// progress signal racing the inspection is never lost. The cursor lock is released before the
    /// suspension (the drain takes and drops it), so other in-flight calls can drive the cursor
    /// while this one sleeps — which is what lets overlapping calls' `End`s, recorded in a
    /// non-deterministic completion order, replay out of claim order.
    pub async fn await_resolution_outcome(
        &self,
        handle: ReplayCallHandle,
    ) -> Result<ResolutionOutcome, WorkerExecutorError> {
        let (start_idx, mut receiver) = handle.into_parts();

        loop {
            // Register interest in cursor progress before inspecting the cursor, so a signal that
            // races the inspection below is delivered to the suspension at the end of the loop.
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            // Drain the terminals at the head: resolves this call in the serial case, and any
            // already-claimed, earlier-completing sibling in the interleaved case.
            self.drain_awaited_terminals().await?;

            match receiver.try_recv() {
                Ok(outcome) => return Ok(outcome),
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    // Sender dropped without resolving (anomalous). Drop any lingering registration.
                    self.unregister_awaiter(start_idx).await;
                    return Err(WorkerExecutorError::runtime(format!(
                        "concurrent replay resolver channel closed for Start at {start_idx}"
                    )));
                }
            }

            if self.is_live() {
                // The lock-free `is_live()` snapshot may have observed a sibling transaction that
                // already published `last_replayed_index == replay_target` while committing *this*
                // call's terminal but had not yet routed it to the resolver (delivery in
                // `on_committed_replay_entry` happens after the position is published, see
                // `commit_consumed_entry`). Acquire the cursor lock — serializing with any such
                // in-flight transaction — and re-check the receiver before concluding the call is
                // incomplete, so a just-resolved final terminal is never misreported.
                let mut st = self.cursor.state.lock().await;
                match receiver.try_recv() {
                    Ok(outcome) => return Ok(outcome),
                    Err(oneshot::error::TryRecvError::Empty) => {
                        // Genuinely reached the end of the oplog without the matching
                        // `End`/`Cancelled`: a committed lone `Start` (a forced commit flushed it
                        // before its `End`, or a crash happened in between). Drop the stale
                        // registration and report Incomplete so the caller can re-execute the side
                        // effect and complete the existing `Start`.
                        st.concurrent_resolver.unregister(start_idx);
                        return Ok(ResolutionOutcome::Incomplete);
                    }
                    Err(oneshot::error::TryRecvError::Closed) => {
                        st.concurrent_resolver.unregister(start_idx);
                        return Err(WorkerExecutorError::runtime(format!(
                            "concurrent replay resolver channel closed for Start at {start_idx}"
                        )));
                    }
                }
            }

            // This call's terminal is not at the cursor head and replay is not over: a
            // concurrently-replaying sibling owns the cursor head. Suspend until our resolution
            // arrives or the cursor advances, then re-drive.
            tokio::select! {
                biased;
                resolved = &mut receiver => {
                    return match resolved {
                        Ok(outcome) => Ok(outcome),
                        Err(_closed) => {
                            self.unregister_awaiter(start_idx).await;
                            Err(WorkerExecutorError::runtime(format!(
                                "concurrent replay resolver channel closed for Start at {start_idx}"
                            )))
                        }
                    };
                }
                _ = progress.as_mut() => {
                    // The cursor advanced; loop to re-drain and re-check.
                }
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum OplogEntryLookupResult {
    Found {
        index: OplogIndex,
        entry: Box<OplogEntry>,
        violates_for_all: bool,
    },
    NotFound {
        violates_for_all: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::oplog::CommitLevel;
    use async_trait::async_trait;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::{
        AgentError, DurableFunctionType, HostRequest, HostRequestNoInput,
        HostResponseMonotonicClockTimestamp, OplogPayload, PayloadId, RawOplogPayload,
    };
    use golem_common::model::{AgentId, Timestamp};
    use std::collections::BTreeMap;
    use std::time::Duration;
    use test_r::test;

    /// Minimal in-memory `Oplog` used to drive a [`ReplayState`] over hand-built entries.
    #[derive(Debug)]
    struct InMemoryOplog {
        entries: tokio::sync::Mutex<Vec<OplogEntry>>,
    }

    impl InMemoryOplog {
        fn new() -> Self {
            Self {
                entries: tokio::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Oplog for InMemoryOplog {
        async fn add(&self, entry: OplogEntry) -> OplogIndex {
            let mut entries = self.entries.lock().await;
            entries.push(entry);
            OplogIndex::from_u64(entries.len() as u64)
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            0
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            BTreeMap::new()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            OplogIndex::from_u64(self.entries.lock().await.len() as u64)
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            None
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            true
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            let entries = self.entries.lock().await;
            let idx: u64 = oplog_index.into();
            entries[(idx - 1) as usize].clone()
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let entries = self.entries.lock().await;
            let start: u64 = oplog_index.into();
            let mut result = BTreeMap::new();
            for i in start..(start + n) {
                if let Some(entry) = entries.get((i - 1) as usize) {
                    result.insert(OplogIndex::from_u64(i), entry.clone());
                }
            }
            result
        }

        async fn length(&self) -> u64 {
            self.entries.lock().await.len() as u64
        }

        async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
            unimplemented!()
        }

        async fn download_raw_payload(
            &self,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unimplemented!()
        }

        async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
    }

    fn test_agent_id() -> OwnedAgentId {
        OwnedAgentId {
            environment_id: EnvironmentId::new(),
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "replay-state-test".to_string(),
            },
        }
    }

    fn noop() -> OplogEntry {
        OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
        }
    }

    fn start_now() -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
        }
    }

    fn begin_atomic_region() -> OplogEntry {
        OplogEntry::BeginAtomicRegion {
            timestamp: Timestamp::now_utc(),
        }
    }

    fn end_for(start_index: u64, nanos: u64) -> OplogEntry {
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp {
                    nanos,
                }),
            ))),
            forced_commit: false,
        }
    }

    /// A `Start` for the legacy `golem::api` fork pair. Its only special replay behaviour is the
    /// commit-only side effect in [`ReplayState::apply_commit_effects`] (recording its index in
    /// `pending_fork_starts`), which the FU8 speculative-rollback test exercises.
    fn fork_start() -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::GolemApiFork,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemote,
        }
    }

    async fn replay_state_over(entries: Vec<OplogEntry>) -> ReplayState {
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in entries {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        ReplayState::new(test_agent_id(), oplog, DeletedRegions::default())
            .await
            .expect("failed to build replay state")
    }

    #[test]
    async fn claim_and_await_resolves_completed() {
        // [NoOp, Start, End]
        let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed {
                end_idx, response, ..
            } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
                assert!(response.is_some());
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn claim_any_returns_claimed_identity() {
        // The dynamic claim does not validate name/type; it returns the claimed Start's identity.
        let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
        let claimed = rs.claim_any_concurrent_start().await.unwrap();
        assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(2));
        assert_eq!(claimed.function_name, HostFunctionName::MonotonicClockNow);
        assert_eq!(
            claimed.durable_function_type,
            DurableFunctionType::ReadLocal
        );

        match rs.await_resolution(claimed.handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn typed_claim_mismatch_does_not_leak_pending() {
        // A typed claim whose expected type does not match the recorded Start must fail AND drop the
        // resolver receiver that `claim_any_concurrent_start` registered, so no stale awaiter leaks.
        let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
        let err = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::WriteRemote, // recorded is ReadLocal
            )
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("durable_function_type"),
            "unexpected error: {err}"
        );
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2)),
            "failed typed claim must not leave a pending awaiter"
        );
    }

    #[test]
    async fn speculative_rollback_leaves_cursor_and_pending_unchanged() {
        // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — after claiming A, the cursor head
        // is the still-unclaimed, non-terminal Start(B). A speculative read whose predicate fails
        // must roll the cursor back fully (it must not steal Start(B)) and must not resolve A.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_now(),
            end_for(2, 42),
            end_for(3, 43),
        ])
        .await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let start_idx = handle.start_idx();

        let speculative = rs.try_get_oplog_entry(|_| false).await.unwrap();
        assert!(speculative.is_none());
        assert_eq!(
            rs.last_replayed_index(),
            OplogIndex::from_u64(2),
            "speculative rollback must not advance the cursor past Start(B)"
        );
        {
            let internal = rs.cursor.state.lock().await;
            assert!(
                internal.concurrent_resolver.is_pending(start_idx),
                "speculative rollback must not resolve the handle"
            );
            assert!(
                !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(3)),
                "speculative rollback must not claim Start(B)"
            );
        }
    }

    #[test]
    async fn speculative_rollback_does_not_apply_side_effects() {
        // FU8: a speculative read whose predicate fails rolls the cursor back AND applies none of the
        // entry's commit-only side effects. A GolemApiFork `Start` records its index in
        // `pending_fork_starts` only when permanently consumed; a rolled-back read must not.
        let rs = replay_state_over(vec![noop(), fork_start()]).await;

        let probe = rs.try_get_oplog_entry(|_| false).await.unwrap();
        assert!(probe.is_none());
        {
            let internal = rs.cursor.state.lock().await;
            assert!(
                internal.pending_fork_starts.is_empty(),
                "rolled-back speculative read must not apply the fork Start side effect"
            );
        }

        // The committed consume does apply the side effect.
        let (idx, _) = rs.try_get_oplog_entry(|_| true).await.unwrap().unwrap();
        assert_eq!(idx, OplogIndex::from_u64(2));
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal
                .pending_fork_starts
                .contains(&OplogIndex::from_u64(2)),
            "committed read must apply the fork Start side effect"
        );
    }

    #[test]
    async fn error_hint_between_start_and_end_resolves() {
        // [NoOp, Start, Error{retry_from: Start}, End] — Error is a hint, skipped transparently.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            OplogEntry::error(
                AgentError::TransientError("boom".to_string()),
                OplogIndex::from_u64(2),
                false,
                None,
            ),
            end_for(2, 42),
        ])
        .await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn dangling_start_without_end_errors() {
        // [NoOp, Start] — eager Start with no matching End/Cancelled (crash window).
        let rs = replay_state_over(vec![noop(), start_now()]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        let err = rs.await_resolution(handle).await.unwrap_err();
        let message = format!("{err}");
        assert!(
            message.contains("no matching End/Cancelled"),
            "unexpected error: {message}"
        );
    }

    #[test]
    async fn lone_start_reports_incomplete_outcome_and_unregisters() {
        // [NoOp, Start] — same crash window as above, but via the outcome-returning API: the lone
        // committed Start (no End) must be reported as Incomplete (not an error), and the stale
        // resolver registration must be dropped so it cannot leak.
        let rs = replay_state_over(vec![noop(), start_now()]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let start_idx = handle.start_idx();

        match rs.await_resolution_outcome(handle).await.unwrap() {
            ResolutionOutcome::Incomplete => {}
            other => panic!("expected Incomplete, got {other:?}"),
        }
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal.concurrent_resolver.is_pending(start_idx),
            "incomplete outcome must unregister the awaiter"
        );
    }

    #[test]
    async fn drain_parks_on_unclaimed_start() {
        // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — draining the awaited terminals
        // while only A is claimed must stop on the still-unclaimed Start(B): A stays pending and the
        // cursor does not advance past A's own Start. The cursor never steals a non-terminal entry a
        // positional consumer / sibling claim owns.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_now(),
            end_for(2, 42),
            end_for(3, 43),
        ])
        .await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

        rs.drain_awaited_terminals().await.unwrap();
        {
            let internal = rs.cursor.state.lock().await;
            assert!(
                internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(2)),
                "drain must not resolve A across the unclaimed Start(B)"
            );
        }
        assert_eq!(
            rs.last_replayed_index(),
            OplogIndex::from_u64(2),
            "drain must not advance past the unclaimed Start(B)"
        );

        // Once B is claimed (guest re-execution reaches its call), awaiting drains both Ends.
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

        match rs.await_resolution(handle_a).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
        match rs.await_resolution(handle_b).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn drain_parks_on_positional_marker() {
        // [NoOp, Start(A=2), BeginAtomicRegion(3), End(A=2→4)] — draining parks on the scope marker a
        // positional reader owns; A resolves once that marker has been consumed.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            begin_atomic_region(),
            end_for(2, 42),
        ])
        .await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        rs.drain_awaited_terminals().await.unwrap();
        {
            let internal = rs.cursor.state.lock().await;
            assert!(
                internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(2))
            );
        }
        assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(2));

        // The positional reader consumes the marker, after which awaiting A drains its End.
        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(3));
        assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn drain_parks_on_unawaited_end() {
        // [NoOp, Start(A=2), End(scope=99→3), End(A=2→4)] — End(99) is a scope End nobody awaits (its
        // Start was consumed positionally). Draining must park on it and leave it for the positional
        // reader instead of consuming it on A's behalf.
        let rs = replay_state_over(vec![noop(), start_now(), end_for(99, 7), end_for(2, 42)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        rs.drain_awaited_terminals().await.unwrap();
        {
            let internal = rs.cursor.state.lock().await;
            assert!(
                internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(2)),
                "drain must not resolve A across an unawaited End"
            );
        }
        assert_eq!(
            rs.last_replayed_index(),
            OplogIndex::from_u64(2),
            "drain must not consume the unawaited scope End"
        );

        // The positional scope reader consumes its own End, after which A's End resolves.
        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(3));
        assert!(matches!(entry, OplogEntry::End { .. }));

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn interleaved_calls_resolve_out_of_order() {
        // [NoOp, Start(A), Start(B), End(B=3), End(A=2)] — completion order (B then A) differs from
        // claim order (A then B). Each call resolves by its own start index, not by position.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_now(),
            end_for(3, 43),
            end_for(2, 42),
        ])
        .await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

        // End(B) at index 4 resolves B; End(A) at index 5 resolves A.
        match rs.await_resolution(handle_b).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
        match rs.await_resolution(handle_a).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn await_suspends_until_sibling_claim() {
        // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — A is claimed and awaited *before*
        // B is claimed. With real overlap the awaiter must SUSPEND on the still-unclaimed Start(B)
        // at the cursor head (neither erroring nor resolving), and then resume once a sibling claims
        // B and advances the cursor — at which point A's End becomes a drainable awaited terminal.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_now(),
            end_for(2, 42),
            end_for(3, 43),
        ])
        .await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

        // Awaiting A parks: its End sits behind the unclaimed Start(B), so the first poll is Pending.
        let a_fut = rs.await_resolution(handle_a);
        tokio::pin!(a_fut);
        assert!(
            futures::poll!(a_fut.as_mut()).is_pending(),
            "awaiting A must suspend while Start(B) is unclaimed"
        );
        assert_eq!(
            rs.last_replayed_index(),
            OplogIndex::from_u64(2),
            "a parked awaiter must not advance the cursor past the unclaimed Start(B)"
        );

        // A sibling claims B (guest re-execution reaches B's call): this advances the cursor and
        // signals progress, waking A so its End at index 4 can be drained on the next poll.
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

        match a_fut.await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
        match rs.await_resolution(handle_b).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn speculative_read_does_not_publish_live_cursor() {
        // [NoOp, Start(A=2)] — replay_target = 2. A speculative read of the last entry must NOT make
        // the cursor observably reach the replay target while the read is still rollbackable: the
        // predicate (run after the read) must still see `is_live() == false`. This is the regression
        // guard for "publish only committed cursor state" — a transient live cursor would let a
        // concurrent awaiter falsely conclude end-of-replay.
        let rs = replay_state_over(vec![noop(), start_now()]).await;
        assert!(!rs.is_live());

        let observed_live = std::cell::Cell::new(None);
        let probe = rs
            .try_get_oplog_entry(|_entry| {
                observed_live.set(Some(rs.is_live()));
                false
            })
            .await
            .unwrap();

        assert!(probe.is_none());
        assert_eq!(
            observed_live.get(),
            Some(false),
            "cursor must not be observably advanced to live while the read is still speculative"
        );
        assert_eq!(
            rs.last_replayed_index(),
            OplogIndex::from_u64(1),
            "a rolled-back probe must leave the committed cursor unchanged"
        );
        assert!(!rs.is_live());
    }

    #[test]
    async fn positional_reader_drains_awaited_terminal_before_marker() {
        // [NoOp, Start(A=2), Start(B=3), End(B=3→4), BeginAtomicRegion(5), End(A=2→6)] — both A and B
        // are claimed; a positional read for the atomic-region marker must first auto-drain B's End
        // (idx 4) to B's awaiter and only then return the marker (idx 5). It must never steal/return
        // End(B) positionally.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_now(),
            end_for(3, 43),
            begin_atomic_region(),
            end_for(2, 42),
        ])
        .await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(5));
        assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

        match rs.await_resolution(handle_b).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
            other => panic!("expected Completed, got {other:?}"),
        }
        match rs.await_resolution(handle_a).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn overlap_layout_with_scope_end_behind_awaited_sibling() {
        // The headline overlap layout:
        //   [NoOp, Start(A=2), Start(scope S=3), Start(B=4), End(B=4→5), End(scope S=3→6), End(A=2→7)]
        // A is claimed and awaited first, but its End sits last; in between are a positional scope
        // (S, consumed by a positional reader) and a fully overlapping sibling call B. This proves:
        // A suspends through the scope Start and B's Start; B's End is auto-drained to B; the scope's
        // End (nobody awaits it) is left for the positional reader; A resolves only once everything
        // ahead of its End has been consumed.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_now(),
            start_now(),
            end_for(4, 44),
            end_for(3, 43),
            end_for(2, 42),
        ])
        .await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

        // A awaits first; it parks on the scope Start (idx 3).
        let a_fut = rs.await_resolution(handle_a);
        tokio::pin!(a_fut);
        assert!(
            futures::poll!(a_fut.as_mut()).is_pending(),
            "A must park on the unclaimed scope Start"
        );

        // The positional scope reader consumes the scope Start (idx 3); A now parks on Start(B).
        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(3));
        assert!(matches!(entry, OplogEntry::Start { .. }));
        assert!(
            futures::poll!(a_fut.as_mut()).is_pending(),
            "A must park on the unclaimed Start(B)"
        );

        // The sibling call B is claimed and resolved; its End (idx 5) is auto-drained to B.
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(4));
        match rs.await_resolution(handle_b).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
            other => panic!("expected Completed, got {other:?}"),
        }

        // The scope End (idx 6) has no awaiter, so it is left for the positional scope reader.
        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(6));
        assert!(matches!(entry, OplogEntry::End { .. }));

        // Only now is A's End (idx 7) at the head; A resolves.
        match a_fut.await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(7)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn switch_to_live_wakes_parked_awaiter_as_incomplete() {
        // [NoOp, Start(A=2), Start(B=3)] — A is claimed and awaited but B is never claimed, so
        // awaiting A parks on the unclaimed Start(B). switch_to_live (end of replay) must wake the
        // parked awaiter as Incomplete instead of leaving it asleep forever, and must drop its
        // registration.
        let rs = replay_state_over(vec![noop(), start_now(), start_now()]).await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let start_idx = handle_a.start_idx();

        let a_fut = rs.await_resolution_outcome(handle_a);
        tokio::pin!(a_fut);
        assert!(
            futures::poll!(a_fut.as_mut()).is_pending(),
            "A must park on the unclaimed Start(B)"
        );

        rs.switch_to_live().await;

        match a_fut.await.unwrap() {
            ResolutionOutcome::Incomplete => {}
            other => panic!("expected Incomplete, got {other:?}"),
        }
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal.concurrent_resolver.is_pending(start_idx),
            "switch_to_live must unregister the parked awaiter"
        );
    }

    fn change_persistence_nothing() -> OplogEntry {
        OplogEntry::ChangePersistenceLevel {
            timestamp: Timestamp::now_utc(),
            persistence_level: PersistenceLevel::PersistNothing,
        }
    }

    fn log_entry() -> OplogEntry {
        OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            level: LogLevel::Info,
            context: "ctx".to_string(),
            message: "msg".to_string(),
        }
    }

    /// When replay reaches the target via a persist-nothing-zone jump (the zone extends to the end of
    /// the oplog and is never closed) rather than by consuming the target entry, the transition to
    /// live must still synthesize `ReplayFinished` — otherwise a pending automatic update would never
    /// be finalized. (Regression: the synthesis used to be gated on the *consumed* entry index
    /// equalling `replay_target`, which this jump never satisfies.)
    #[test]
    async fn replay_finished_emitted_when_persist_nothing_zone_reaches_target() {
        // [NoOp(1), ChangePersistenceLevel(PersistNothing)(2), Log(3), Log(4)] — the persist-nothing
        // zone opened at 2 is never closed, so replay jumps straight to the target (4) without
        // consuming it.
        let rs = replay_state_over(vec![
            noop(),
            change_persistence_nothing(),
            log_entry(),
            log_entry(),
        ])
        .await;
        assert!(rs.is_live(), "replay should be complete after construction");
        let events = rs.take_new_replay_events().await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ReplayEvent::ReplayFinished)),
            "a persist-nothing-zone jump to the target must still emit ReplayFinished, got {events:?}"
        );
    }

    /// When replay reaches the target via a skipped-region jump (`get_out_of_skipped_region` jumps
    /// the cursor to the region end, which is the target) rather than by consuming the target entry,
    /// the transition to live must still synthesize `ReplayFinished`.
    #[test]
    async fn replay_finished_emitted_when_skipped_region_reaches_target() {
        // [NoOp(1), Start(2), Log(3), Log(4)] with deleted region [3, 4]: consuming the Start at 2
        // jumps the cursor over the deleted tail straight to the target (4).
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [noop(), start_now(), log_entry(), log_entry()] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(4),
        }]);
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");

        assert!(!rs.is_live(), "Start at 2 is not yet consumed");
        let (idx, _) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(2));

        assert!(
            rs.is_live(),
            "consuming the Start must jump over the deleted tail to the target"
        );
        let events = rs.take_new_replay_events().await;
        let finished = events
            .iter()
            .filter(|e| matches!(e, ReplayEvent::ReplayFinished))
            .count();
        assert_eq!(
            finished, 1,
            "a skipped-region jump to the target must emit exactly one ReplayFinished, got {events:?}"
        );
    }

    /// Regression guard for the moved transition detection: consuming the target entry directly
    /// (the common path) still emits exactly one `ReplayFinished`.
    #[test]
    async fn replay_finished_emitted_when_target_entry_consumed() {
        // [NoOp(1), Start(2), End(3)] — replay becomes live by consuming the End at the target (3).
        let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
        // Nothing has crossed into live yet (the Start is still pending a claim).
        assert!(rs.take_new_replay_events().await.is_empty());

        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        rs.await_resolution(handle).await.unwrap();

        assert!(rs.is_live());
        let events = rs.take_new_replay_events().await;
        let finished = events
            .iter()
            .filter(|e| matches!(e, ReplayEvent::ReplayFinished))
            .count();
        assert_eq!(
            finished, 1,
            "consuming the target entry must emit exactly one ReplayFinished, got {events:?}"
        );
    }

    /// How a generated concurrent call terminates in the fabricated oplog.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CallKind {
        /// Recorded an `End` (successful completion).
        Completed,
        /// Recorded a `Cancelled` (dropped before completion).
        Cancelled,
        /// No terminal at all: a committed `Start` whose `End`/`Cancelled` never made it to disk
        /// (the forced-commit / crash window). Replay must report this as `Incomplete`.
        Incomplete,
    }

    fn cancelled_for(start_index: u64) -> OplogEntry {
        OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
            partial: None,
        }
    }

    fn end_atomic_region(begin_index: u64) -> OplogEntry {
        OplogEntry::EndAtomicRegion {
            timestamp: Timestamp::now_utc(),
            begin_index: OplogIndex::from_u64(begin_index),
        }
    }

    fn change_persistence_smart() -> OplogEntry {
        OplogEntry::ChangePersistenceLevel {
            timestamp: Timestamp::now_utc(),
            persistence_level: PersistenceLevel::Smart,
        }
    }

    fn pre_commit_remote_transaction(begin_index: u64) -> OplogEntry {
        OplogEntry::PreCommitRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index: OplogIndex::from_u64(begin_index),
        }
    }

    fn committed_remote_transaction(begin_index: u64) -> OplogEntry {
        OplogEntry::CommittedRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index: OplogIndex::from_u64(begin_index),
        }
    }

    /// A non-hint *positional* marker entry (atomic-region boundary, persistence-level switch, or an
    /// rdbms-transaction internal marker). These are never claimed and never auto-drained; a
    /// positional reader must consume them, and an overlapping awaiter parks on them until then.
    fn random_positional_marker(rng: &mut rand::rngs::StdRng) -> OplogEntry {
        use rand::Rng;
        match rng.random_range(0..6) {
            0 => begin_atomic_region(),
            1 => end_atomic_region(1),
            2 => change_persistence_smart(),
            3 => pre_commit_remote_transaction(1),
            4 => committed_remote_transaction(1),
            // `NoOp` is non-hint, so it too must be consumed by a positional reader (unlike the
            // `Log` hint entries, which are skipped transparently).
            _ => noop(),
        }
    }

    /// A generated item in a fabricated overlap layout: either a concurrent call (claimed +
    /// awaited) or a positional scope (a `Start`/`End` pair consumed by positional reads, standing
    /// in for a durable scope / rdbms transaction span).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ItemKind {
        Call(CallKind),
        Scope,
    }

    /// The role of a single fabricated oplog entry, aligned by index, so the replay driver knows how
    /// to consume each entry as the cursor reaches it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Role {
        Placeholder,
        CallStart(usize),
        CallTerminal(usize),
        ScopeStart,
        ScopeEnd,
        Marker,
        Hint,
    }

    /// Seam 1 of the concurrent-durability validation plan: a randomized generator over
    /// host-call-only oplog layouts. Each case builds
    /// `[<placeholder>, Start_1 .. Start_n, <terminals in a random completion order>]` with `Log`
    /// hint entries optionally interleaved everywhere, where each call independently completes (`End`), is
    /// cancelled (`Cancelled`), or is left incomplete (a committed `Start` with no terminal). It then
    /// claims every `Start` and awaits each call's resolution in a random order, asserting that:
    ///
    /// - the k-th positional claim returns the k-th `Start`;
    /// - every call resolves to exactly its recorded terminal *by oplog index*, independent of the
    ///   completion order recorded in the oplog and the order the calls are awaited in (a single
    ///   await drains all awaited terminals at the cursor head, buffering siblings' outcomes);
    /// - an incomplete `Start` reports `Incomplete` rather than erroring or stealing a sibling's
    ///   terminal;
    /// - interleaved hint entries (`Log`) are skipped transparently, whether they land between
    ///   `Start`s, between a sibling's `Start` and `End`, or among the terminals;
    /// - once all calls resolve, replay is live with no awaiter left registered.
    ///
    /// This generalizes the hand-written `n = 2/3` overlap tests above to the full
    /// call/completion/await permutation space. Seeds are deterministic so any failure reproduces.
    #[test]
    async fn concurrent_replay_call_permutation_fuzz() {
        use rand::rngs::StdRng;
        use rand::seq::SliceRandom;
        use rand::{Rng, SeedableRng};

        const CASES: u64 = 2000;

        for seed in 0..CASES {
            let mut rng = StdRng::seed_from_u64(seed);
            let n = rng.random_range(1..=5usize);

            let kinds: Vec<CallKind> = (0..n)
                .map(|_| match rng.random_range(0..3) {
                    0 => CallKind::Completed,
                    1 => CallKind::Cancelled,
                    _ => CallKind::Incomplete,
                })
                .collect();

            // Index 1 is the mandatory placeholder consumed unconditionally at construction (it
            // stands in for the `Create` worker entry), so the first Start is at index 2.
            let mut entries = vec![noop()];
            let mut start_idx = Vec::with_capacity(n);
            for _ in 0..n {
                if rng.random_bool(0.3) {
                    entries.push(log_entry());
                }
                entries.push(start_now());
                start_idx.push(entries.len() as u64);
            }

            // Terminals for the non-incomplete calls, recorded in a random completion order.
            let mut terminal_calls: Vec<usize> = (0..n)
                .filter(|&i| kinds[i] != CallKind::Incomplete)
                .collect();
            terminal_calls.shuffle(&mut rng);

            let mut terminal_oplog_idx: Vec<Option<u64>> = vec![None; n];
            let mut nanos = 0u64;
            for &i in &terminal_calls {
                if rng.random_bool(0.3) {
                    entries.push(log_entry());
                }
                let entry = match kinds[i] {
                    CallKind::Completed => {
                        nanos += 1;
                        end_for(start_idx[i], nanos)
                    }
                    CallKind::Cancelled => cancelled_for(start_idx[i]),
                    CallKind::Incomplete => unreachable!("incomplete calls have no terminal"),
                };
                entries.push(entry);
                terminal_oplog_idx[i] = Some(entries.len() as u64);
            }
            if rng.random_bool(0.3) {
                entries.push(log_entry());
            }

            let rs = replay_state_over(entries).await;

            // Claim every Start positionally; the k-th claim returns the k-th Start.
            let mut handles: Vec<Option<ReplayCallHandle>> = Vec::with_capacity(n);
            for (i, expected) in start_idx.iter().enumerate() {
                let handle = rs
                    .claim_concurrent_start(
                        &HostFunctionName::MonotonicClockNow,
                        &DurableFunctionType::ReadLocal,
                    )
                    .await
                    .unwrap_or_else(|e| panic!("seed {seed}: claim {i} failed: {e}"));
                assert_eq!(
                    handle.start_idx(),
                    OplogIndex::from_u64(*expected),
                    "seed {seed}: claim {i} returned the wrong Start"
                );
                handles.push(Some(handle));
            }

            // Await resolutions in a random order; out-of-order awaiting must still resolve each call
            // to its own recorded terminal.
            let mut await_order: Vec<usize> = (0..n).collect();
            await_order.shuffle(&mut rng);
            for i in await_order {
                let handle = handles[i]
                    .take()
                    .expect("each handle is awaited exactly once");
                let outcome = rs
                    .await_resolution_outcome(handle)
                    .await
                    .unwrap_or_else(|e| panic!("seed {seed}: await {i} failed: {e}"));
                match (kinds[i], outcome) {
                    (
                        CallKind::Completed,
                        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }),
                    ) => {
                        assert_eq!(
                            end_idx,
                            OplogIndex::from_u64(terminal_oplog_idx[i].unwrap()),
                            "seed {seed}: call {i} resolved to the wrong End index"
                        );
                    }
                    (
                        CallKind::Cancelled,
                        ResolutionOutcome::Resolved(Resolution::Cancelled {
                            cancelled_idx, ..
                        }),
                    ) => {
                        assert_eq!(
                            cancelled_idx,
                            OplogIndex::from_u64(terminal_oplog_idx[i].unwrap()),
                            "seed {seed}: call {i} resolved to the wrong Cancelled index"
                        );
                    }
                    (CallKind::Incomplete, ResolutionOutcome::Incomplete) => {}
                    (kind, other) => panic!(
                        "seed {seed}: call {i} (kind {kind:?}) resolved unexpectedly: {other:?}"
                    ),
                }
            }

            assert!(
                rs.is_live(),
                "seed {seed}: replay did not reach live after all calls resolved"
            );
            let internal = rs.cursor.state.lock().await;
            for (i, &si) in start_idx.iter().enumerate() {
                assert!(
                    !internal
                        .concurrent_resolver
                        .is_pending(OplogIndex::from_u64(si)),
                    "seed {seed}: call {i} left a registered awaiter"
                );
            }
        }
    }

    /// Seam 1, full layout space: a randomized generator over fabricated overlap layouts that mix
    /// concurrent calls (completed / cancelled / incomplete) with **positional** scopes (`Start`/`End`
    /// pairs consumed by positional reads) and non-hint positional **markers** (atomic-region
    /// boundaries, persistence-level switches, rdbms-transaction internal markers), all freely
    /// interleaved with `Log` hints, so that a sibling's scope `End` or a marker can land between
    /// another call's `Start` and `End` — the headline overlap layout generalized.
    ///
    /// Each call's resolution is awaited on its **own concurrently-suspended task** (`tokio::spawn`),
    /// mirroring the production model where the worker drives the replay cursor (claims + positional
    /// reads) while several call futures are suspended; this is what exercises the genuine
    /// suspend/resume path (`await_resolution_outcome` parking on a positional blocker and resuming on
    /// cursor progress), not just the auto-drain-at-head path. A single driver walks the oplog
    /// left-to-right, claiming call `Start`s, positionally reading scope `Start`/`End`s and markers,
    /// and leaving call terminals to be auto-drained. It asserts that:
    ///
    /// - each positional claim / read returns exactly the entry at the expected oplog index,
    ///   independent of how the suspended awaiter tasks are scheduled (auto-drains only ever consume
    ///   awaited call terminals, never a positional entry a reader owns);
    /// - every call resolves to exactly its recorded terminal (`End`/`Cancelled` by index) or, for a
    ///   committed `Start` with no terminal, `Incomplete`;
    /// - replay ends live with no awaiter left registered.
    ///
    /// Only final per-call outcomes and positional indices are asserted, both of which are
    /// timing-independent, so the test is deterministic despite the concurrent tasks. Seeds are
    /// fixed, so any failure reproduces.
    #[test]
    async fn concurrent_replay_overlap_with_scopes_and_markers_fuzz() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const CASES: u64 = 1000;

        for seed in 0..CASES {
            let mut rng = StdRng::seed_from_u64(seed);
            let num_items = rng.random_range(1..=5usize);

            let items: Vec<ItemKind> = (0..num_items)
                .map(|_| match rng.random_range(0..4) {
                    0 => ItemKind::Call(CallKind::Completed),
                    1 => ItemKind::Call(CallKind::Cancelled),
                    2 => ItemKind::Call(CallKind::Incomplete),
                    _ => ItemKind::Scope,
                })
                .collect();

            let is_incomplete = |i: usize| matches!(items[i], ItemKind::Call(CallKind::Incomplete));

            // Build a valid random interleaving: each item's Start precedes its End; incomplete
            // calls have no End; scopes and completed/cancelled calls do. Markers and hints are
            // sprinkled in from a budget so they can land between any sibling's Start and End.
            let mut entries = vec![noop()];
            let mut roles = vec![Role::Placeholder];
            let mut start_idx = vec![0u64; num_items];
            let mut terminal_idx = vec![None; num_items];
            let mut opened = vec![false; num_items];
            let mut closed = vec![false; num_items];
            let mut markers_left = rng.random_range(0..=4u32);
            let mut hints_left = rng.random_range(0..=3u32);
            let mut nanos = 0u64;

            loop {
                let can_open: Vec<usize> = (0..num_items).filter(|&i| !opened[i]).collect();
                let can_close: Vec<usize> = (0..num_items)
                    .filter(|&i| opened[i] && !closed[i] && !is_incomplete(i))
                    .collect();

                #[derive(Clone, Copy)]
                enum Cat {
                    Open,
                    Close,
                    Marker,
                    Hint,
                }
                let mut cats = Vec::new();
                if !can_open.is_empty() {
                    cats.push(Cat::Open);
                }
                if !can_close.is_empty() {
                    cats.push(Cat::Close);
                }
                if markers_left > 0 {
                    cats.push(Cat::Marker);
                }
                if hints_left > 0 {
                    cats.push(Cat::Hint);
                }
                if cats.is_empty() {
                    break;
                }

                match cats[rng.random_range(0..cats.len())] {
                    Cat::Open => {
                        let item = can_open[rng.random_range(0..can_open.len())];
                        entries.push(start_now());
                        start_idx[item] = entries.len() as u64;
                        opened[item] = true;
                        roles.push(match items[item] {
                            ItemKind::Call(_) => Role::CallStart(item),
                            ItemKind::Scope => Role::ScopeStart,
                        });
                    }
                    Cat::Close => {
                        let item = can_close[rng.random_range(0..can_close.len())];
                        let si = start_idx[item];
                        let (entry, role) = match items[item] {
                            ItemKind::Call(CallKind::Completed) => {
                                nanos += 1;
                                (end_for(si, nanos), Role::CallTerminal(item))
                            }
                            ItemKind::Call(CallKind::Cancelled) => {
                                (cancelled_for(si), Role::CallTerminal(item))
                            }
                            ItemKind::Scope => {
                                nanos += 1;
                                (end_for(si, nanos), Role::ScopeEnd)
                            }
                            ItemKind::Call(CallKind::Incomplete) => {
                                unreachable!("incomplete calls are never closed")
                            }
                        };
                        entries.push(entry);
                        terminal_idx[item] = Some(entries.len() as u64);
                        closed[item] = true;
                        roles.push(role);
                    }
                    Cat::Marker => {
                        entries.push(random_positional_marker(&mut rng));
                        roles.push(Role::Marker);
                        markers_left -= 1;
                    }
                    Cat::Hint => {
                        entries.push(log_entry());
                        roles.push(Role::Hint);
                        hints_left -= 1;
                    }
                }
            }

            let rs = Arc::new(replay_state_over(entries).await);

            // Walk the oplog left-to-right, consuming each entry by its role. Each claimed call's
            // resolution is awaited on its own suspended task.
            let mut tasks: Vec<(usize, tokio::task::JoinHandle<_>)> = Vec::new();
            for (zero_based, role) in roles.iter().enumerate().skip(1) {
                let idx = (zero_based + 1) as u64;
                match *role {
                    Role::CallStart(item) => {
                        let handle = rs
                            .claim_concurrent_start(
                                &HostFunctionName::MonotonicClockNow,
                                &DurableFunctionType::ReadLocal,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                panic!("seed {seed}: claim of item {item} at {idx} failed: {e}")
                            });
                        assert_eq!(
                            handle.start_idx(),
                            OplogIndex::from_u64(idx),
                            "seed {seed}: claim of item {item} returned the wrong Start"
                        );
                        let rs2 = rs.clone();
                        tasks.push((
                            item,
                            tokio::spawn(async move { rs2.await_resolution_outcome(handle).await }),
                        ));
                    }
                    Role::ScopeStart | Role::ScopeEnd | Role::Marker => {
                        let (got, _) = rs.get_oplog_entry().await.unwrap_or_else(|e| {
                            panic!("seed {seed}: positional read at {idx} ({role:?}) failed: {e}")
                        });
                        assert_eq!(
                            got,
                            OplogIndex::from_u64(idx),
                            "seed {seed}: positional read ({role:?}) returned the wrong index"
                        );
                    }
                    // Call terminals are auto-drained to their awaiter; hints are skipped by the
                    // preceding consume's skip_forward. Neither is walked explicitly.
                    Role::CallTerminal(_) | Role::Hint => {}
                    Role::Placeholder => unreachable!("placeholder is skipped"),
                }
            }

            // Join the suspended awaiter tasks and check each call resolved to its recorded terminal.
            for (item, task) in tasks {
                let outcome = task
                    .await
                    .expect("awaiter task panicked")
                    .unwrap_or_else(|e| panic!("seed {seed}: await of item {item} failed: {e}"));
                let kind = match items[item] {
                    ItemKind::Call(kind) => kind,
                    ItemKind::Scope => unreachable!("scopes are not awaited"),
                };
                match (kind, outcome) {
                    (
                        CallKind::Completed,
                        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }),
                    ) => assert_eq!(
                        end_idx,
                        OplogIndex::from_u64(terminal_idx[item].unwrap()),
                        "seed {seed}: item {item} resolved to the wrong End index"
                    ),
                    (
                        CallKind::Cancelled,
                        ResolutionOutcome::Resolved(Resolution::Cancelled {
                            cancelled_idx, ..
                        }),
                    ) => assert_eq!(
                        cancelled_idx,
                        OplogIndex::from_u64(terminal_idx[item].unwrap()),
                        "seed {seed}: item {item} resolved to the wrong Cancelled index"
                    ),
                    (CallKind::Incomplete, ResolutionOutcome::Incomplete) => {}
                    (kind, other) => panic!(
                        "seed {seed}: item {item} (kind {kind:?}) resolved unexpectedly: {other:?}"
                    ),
                }
            }

            assert!(
                rs.is_live(),
                "seed {seed}: replay did not reach live after the full walk"
            );
            let internal = rs.cursor.state.lock().await;
            for (i, &si) in start_idx.iter().enumerate() {
                if matches!(items[i], ItemKind::Call(_)) {
                    assert!(
                        !internal
                            .concurrent_resolver
                            .is_pending(OplogIndex::from_u64(si)),
                        "seed {seed}: item {i} left a registered awaiter"
                    );
                }
            }
        }
    }

    /// Seam 1, deleted/jump regions: a randomized generator that records a run of contiguous
    /// `Start`/`End` call pairs and then marks a random subset of those pairs as belonging to deleted
    /// oplog regions (as a `Jump`/revert would leave behind). The deleted entries must be skipped by
    /// the replay cursor entirely — never claimed, never read — and the calls outside the deleted
    /// regions must still claim at their true indices and resolve. Deleting a leading region exercises
    /// the construction-time jump; deleting a trailing region exercises the jump-to-target transition
    /// into live. Seeds are fixed, so any failure reproduces.
    #[test]
    async fn replay_skips_deleted_regions_fuzz() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const CASES: u64 = 500;

        for seed in 0..CASES {
            let mut rng = StdRng::seed_from_u64(seed);
            let num_calls = rng.random_range(1..=6usize);

            // Contiguous call pairs after the placeholder: [Start, End, Start, End, ...].
            let mut entries = vec![noop()];
            let mut start_idx = Vec::with_capacity(num_calls);
            let mut end_idx = Vec::with_capacity(num_calls);
            let mut deleted = Vec::with_capacity(num_calls);
            let mut nanos = 0u64;
            for _ in 0..num_calls {
                entries.push(start_now());
                let si = entries.len() as u64;
                nanos += 1;
                entries.push(end_for(si, nanos));
                let ei = entries.len() as u64;
                start_idx.push(si);
                end_idx.push(ei);
                deleted.push(rng.random_bool(0.4));
            }

            // Coalesce the deleted entry indices into contiguous regions.
            let mut deleted_indices: std::collections::BTreeSet<u64> =
                std::collections::BTreeSet::new();
            for i in 0..num_calls {
                if deleted[i] {
                    deleted_indices.insert(start_idx[i]);
                    deleted_indices.insert(end_idx[i]);
                }
            }
            let mut regions = Vec::new();
            let mut run: Option<(u64, u64)> = None;
            for &idx in &deleted_indices {
                match run {
                    Some((s, e)) if idx == e + 1 => run = Some((s, idx)),
                    Some((s, e)) => {
                        regions.push((s, e));
                        run = Some((idx, idx));
                    }
                    None => run = Some((idx, idx)),
                }
            }
            if let Some((s, e)) = run {
                regions.push((s, e));
            }

            let oplog = Arc::new(InMemoryOplog::new());
            for entry in entries {
                oplog.add(entry).await;
            }
            let oplog: Arc<dyn Oplog> = oplog;
            let skipped = DeletedRegions::from_regions(regions.iter().map(|&(s, e)| OplogRegion {
                start: OplogIndex::from_u64(s),
                end: OplogIndex::from_u64(e),
            }));
            let rs = ReplayState::new(test_agent_id(), oplog, skipped)
                .await
                .expect("failed to build replay state");

            // Claim only the kept calls, in order; the cursor must jump over every deleted region.
            let mut handles = Vec::new();
            for i in 0..num_calls {
                if deleted[i] {
                    continue;
                }
                let handle = rs
                    .claim_concurrent_start(
                        &HostFunctionName::MonotonicClockNow,
                        &DurableFunctionType::ReadLocal,
                    )
                    .await
                    .unwrap_or_else(|e| panic!("seed {seed}: claim of kept call {i} failed: {e}"));
                assert_eq!(
                    handle.start_idx(),
                    OplogIndex::from_u64(start_idx[i]),
                    "seed {seed}: kept call {i} claimed a wrong (possibly deleted) Start"
                );
                handles.push((i, handle));
            }

            for (i, handle) in handles {
                match rs
                    .await_resolution(handle)
                    .await
                    .unwrap_or_else(|e| panic!("seed {seed}: await of kept call {i} failed: {e}"))
                {
                    Resolution::Completed { end_idx: ei, .. } => assert_eq!(
                        ei,
                        OplogIndex::from_u64(end_idx[i]),
                        "seed {seed}: kept call {i} resolved to the wrong End"
                    ),
                    other => panic!("seed {seed}: kept call {i} expected Completed, got {other:?}"),
                }
            }

            assert!(
                rs.is_live(),
                "seed {seed}: replay did not reach live after skipping deleted regions"
            );
        }
    }

    /// Seam 1, persist-nothing zones: a randomized generator that wraps a contiguous block of call
    /// pairs (and a `NoOp`) in a `ChangePersistenceLevel(PersistNothing)` … `ChangePersistenceLevel`
    /// zone. Everything recorded inside the zone is observability-only and must be skipped by the
    /// replay cursor (never claimed, never read), while the calls outside the zone claim at their
    /// true indices and resolve. A leading zone exercises the construction-time skip. Seeds are
    /// fixed, so any failure reproduces.
    #[test]
    async fn replay_skips_persist_nothing_zones_fuzz() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const CASES: u64 = 500;

        for seed in 0..CASES {
            let mut rng = StdRng::seed_from_u64(seed);
            let before = rng.random_range(0..=2usize);
            let inside = rng.random_range(0..=2usize);
            let after = rng.random_range(0..=2usize);
            // Need at least one observable call so the assertions are meaningful.
            if before + after == 0 {
                continue;
            }

            let mut entries = vec![noop()];
            let mut kept_start = Vec::new();
            let mut kept_end = Vec::new();
            let mut nanos = 0u64;

            let push_call = |entries: &mut Vec<OplogEntry>, nanos: &mut u64| -> (u64, u64) {
                entries.push(start_now());
                let si = entries.len() as u64;
                *nanos += 1;
                entries.push(end_for(si, *nanos));
                (si, entries.len() as u64)
            };

            for _ in 0..before {
                let (si, ei) = push_call(&mut entries, &mut nanos);
                kept_start.push(si);
                kept_end.push(ei);
            }

            // Open the persist-nothing zone, record skipped filler, then close it.
            entries.push(change_persistence_nothing());
            entries.push(noop());
            for _ in 0..inside {
                push_call(&mut entries, &mut nanos);
            }
            entries.push(change_persistence_smart());

            for _ in 0..after {
                let (si, ei) = push_call(&mut entries, &mut nanos);
                kept_start.push(si);
                kept_end.push(ei);
            }

            let rs = replay_state_over(entries).await;

            let mut handles = Vec::new();
            for (k, &si) in kept_start.iter().enumerate() {
                let handle = rs
                    .claim_concurrent_start(
                        &HostFunctionName::MonotonicClockNow,
                        &DurableFunctionType::ReadLocal,
                    )
                    .await
                    .unwrap_or_else(|e| panic!("seed {seed}: claim of kept call {k} failed: {e}"));
                assert_eq!(
                    handle.start_idx(),
                    OplogIndex::from_u64(si),
                    "seed {seed}: kept call {k} claimed a Start inside the persist-nothing zone"
                );
                handles.push((k, handle));
            }

            for (k, handle) in handles {
                match rs
                    .await_resolution(handle)
                    .await
                    .unwrap_or_else(|e| panic!("seed {seed}: await of kept call {k} failed: {e}"))
                {
                    Resolution::Completed { end_idx: ei, .. } => assert_eq!(
                        ei,
                        OplogIndex::from_u64(kept_end[k]),
                        "seed {seed}: kept call {k} resolved to the wrong End"
                    ),
                    other => panic!("seed {seed}: kept call {k} expected Completed, got {other:?}"),
                }
            }

            assert!(
                rs.is_live(),
                "seed {seed}: replay did not reach live after skipping the persist-nothing zone"
            );
        }
    }
}
