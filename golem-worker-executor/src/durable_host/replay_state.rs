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
use tokio::sync::{Mutex, Notify, RwLock};
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
    owned_agent_id: OwnedAgentId,
    oplog: Arc<dyn Oplog>,
    replay_target: AtomicOplogIndex,
    /// The oplog index of the last replayed entry
    last_replayed_index: AtomicOplogIndex,
    /// The oplog index of the last non-hint entry read
    last_replayed_non_hint_index: AtomicOplogIndex,
    internal: Arc<RwLock<InternalReplayState>>,
    has_seen_logs: Arc<AtomicBool>,
    /// Serializes every advance of the shared replay cursor. The cursor itself is a set of atomics,
    /// but advancing it is a read-modify-commit transaction spanning several `await`s (oplog reads,
    /// commit-effect downloads), not a single CAS, so concurrently-replaying durable calls (and the
    /// positional readers they interleave with) must take this lock for the duration of one cursor
    /// transaction. The transaction holds it across those internal `await`s, but it is *not* held
    /// while a call is parked on its resolution / cursor progress — an awaiter releases it before
    /// sleeping (see [`Self::await_resolution_outcome`]) — and no operation awaited under it
    /// re-enters `cursor_lock`. Lock order is always `cursor_lock` → `internal` (never the reverse).
    cursor_lock: Arc<Mutex<()>>,
    /// Fired (via `notify_waiters`) on every committed cursor advance, on resolver registration,
    /// and at `switch_to_live`. A durable call suspended in [`Self::await_resolution_outcome`] —
    /// because its `End`/`Cancelled` is not yet at the cursor head while a concurrently-replaying
    /// sibling owns it — wakes on this to re-drive the cursor. Resolver delivery is the primary
    /// wakeup; this is the wakeup for the "another consumer advanced the cursor past my blocker"
    /// case that a oneshot alone cannot cover.
    progress: Arc<Notify>,
}

#[derive(Debug)]
struct InternalReplayState {
    pub skipped_regions: DeletedRegions,
    pub next_skipped_region: Option<OplogRegion>,
    /// Hashes of log entries persisted since the last read non-hint oplog entry
    pub log_hashes: HashSet<(u64, u64)>,
    /// Updates that were encountered while reading the oplog
    pub pending_replay_events: Vec<ReplayEvent>,
    /// `Start` entries for `GolemApiFork` whose matching `End` has not yet
    /// been replayed. When the matching `End` is read, the response is
    /// decoded and a `ForkReplayed` event is emitted. The legacy adapter only
    /// ever has at most one in flight at a time (it writes the matched `End`
    /// immediately after the `Start`), but we use a set so that future
    /// concurrent recorders cannot trip us up.
    pub pending_fork_starts: HashSet<OplogIndex>,
    /// Matches replayed `End`/`Cancelled` entries to the concurrent [`crate::durable_host::concurrent::CallHandle`]s
    /// awaiting them, keyed by their `Start` index. Fed only from the committed-consume hook.
    pub concurrent_resolver: ConcurrentReplayResolver,
}

impl ReplayState {
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        skipped_regions: DeletedRegions,
    ) -> Result<Self, WorkerExecutorError> {
        let next_skipped_region = skipped_regions.find_next_deleted_region(OplogIndex::NONE);
        let last_oplog_index = oplog.current_oplog_index().await;
        let result = Self {
            owned_agent_id,
            oplog,
            last_replayed_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
            last_replayed_non_hint_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
            replay_target: AtomicOplogIndex::from_oplog_index(last_oplog_index),
            internal: Arc::new(RwLock::new(InternalReplayState {
                skipped_regions,
                next_skipped_region,
                log_hashes: HashSet::new(),
                pending_replay_events: Vec::new(),
                pending_fork_starts: HashSet::new(),
                concurrent_resolver: ConcurrentReplayResolver::default(),
            })),
            has_seen_logs: Arc::new(AtomicBool::new(false)),
            cursor_lock: Arc::new(Mutex::new(())),
            progress: Arc::new(Notify::new()),
        };
        result.move_replay_idx(OplogIndex::INITIAL).await; // By this we handle initial skipped regions applied by manual updates correctly
        // No concurrency during construction: the replay state is not shared yet, so driving the
        // cursor without the lock is sound.
        result.skip_forward_locked().await?;
        Ok(result)
    }

    pub async fn drop_override_and_restart(&self) -> Result<(), WorkerExecutorError> {
        let _cursor = self.cursor_lock.lock().await;
        {
            let mut internal = self.internal.write().await;
            internal.skipped_regions.drop_override();
            internal.next_skipped_region = internal
                .skipped_regions
                .find_next_deleted_region(OplogIndex::NONE);
            internal.log_hashes.clear();
            internal.pending_replay_events.clear();
        }
        self.last_replayed_index.set(OplogIndex::NONE);
        self.last_replayed_non_hint_index.set(OplogIndex::NONE);
        self.move_replay_idx(OplogIndex::INITIAL).await;
        self.skip_forward_locked().await
    }

    pub async fn switch_to_live(&self) {
        let _cursor = self.cursor_lock.lock().await;
        if !self.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished).await;
        }
        self.last_replayed_index.set(self.replay_target.get());
        // Replay is over: any durable call whose `Start` was committed but whose terminal never was
        // is incomplete. Wake every still-suspended awaiter so it returns `Incomplete` instead of
        // sleeping forever waiting for a cursor that will not advance again.
        self.internal
            .write()
            .await
            .concurrent_resolver
            .fail_all_pending_incomplete();
        self.signal_progress();
    }

    pub fn last_replayed_index(&self) -> OplogIndex {
        self.last_replayed_index.get()
    }

    pub fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.last_replayed_non_hint_index.get()
    }

    pub fn replay_target(&self) -> OplogIndex {
        self.replay_target.get()
    }

    pub fn set_replay_target(&self, new_target: OplogIndex) {
        self.replay_target.set(new_target)
    }

    /// Wakes every awaiter suspended on cursor progress in [`Self::await_resolution_outcome`].
    ///
    /// Fired on every committed cursor advance, on resolver registration (a new awaited terminal
    /// may now be drainable), and at `switch_to_live`. `notify_waiters` only wakes awaiters that
    /// have already registered interest; the await loop registers (via `Notified::enable`) *before*
    /// it inspects the cursor, so a progress signal between the inspection and the actual sleep is
    /// not lost.
    fn signal_progress(&self) {
        self.progress.notify_waiters();
    }

    pub async fn is_in_skipped_region(&self, oplog_index: OplogIndex) -> bool {
        let internal = self.internal.read().await;
        internal.skipped_regions.is_in_deleted_region(oplog_index)
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.last_replayed_index.get() == self.replay_target.get()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    async fn record_replay_event(&self, event: ReplayEvent) {
        self.internal
            .write()
            .await
            .pending_replay_events
            .push(event)
    }

    pub async fn take_new_replay_events(&self) -> Vec<ReplayEvent> {
        std::mem::take(&mut self.internal.write().await.pending_replay_events)
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    ///
    /// Returns an error if the underlying read fails (e.g. missing oplog entry,
    /// corrupted GolemApiFork payload) so the worker can fail the agent with a
    /// non-retriable trap rather than panicking the executor.
    pub async fn get_oplog_entry(&self) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        let _cursor = self.cursor_lock.lock().await;
        // The closure always returns true, so the only `None` case is end-of-replay (a positional
        // reader expecting an entry that the oplog does not contain).
        self.try_get_oplog_entry_locked(|_| true)
            .await?
            .ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    "next oplog entry to replay",
                    format!(
                        "end of replay for {} at index {}; replay target = {}",
                        self.owned_agent_id,
                        self.last_replayed_index.get(),
                        self.replay_target.get(),
                    ),
                )
            })
    }

    /// Checks whether the currently read `entry` is a hint entry is valid for replay, or
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
    ///
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
                let end_index = self
                    .lookup_oplog_entry(begin_index, |entry, _idx| match entry {
                        OplogEntry::ChangePersistenceLevel {
                            persistence_level, ..
                        } => persistence_level != &PersistenceLevel::PersistNothing,
                        OplogEntry::AgentInvocationFinished { .. } => true,
                        _ => false,
                    })
                    .await;

                if let Some(end_index) = end_index {
                    Some(end_index)
                } else {
                    // The zone has not been closed
                    Some(self.replay_target())
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Reads the next oplog entry, and if it matches the given condition, skips
    /// every hint entry following it and returns the oplog index of the entry read.
    /// If the condition is not met, returns `None` and the candidate entry is left unconsumed with
    /// the cursor, skipped-region state, and side effects untouched. (Any *awaited terminals* sitting
    /// ahead of the candidate are drained to their awaiters first — see
    /// [`Self::try_get_oplog_entry_locked`] — and those drains stay committed.)
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
        let _cursor = self.cursor_lock.lock().await;
        self.try_get_oplog_entry_locked(condition).await
    }

    /// The single cursor transaction, run under [`Self::cursor_lock`].
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
    async fn try_get_oplog_entry_locked(
        &self,
        mut condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        loop {
            if self.is_live() {
                // No further entries to read: nothing to drain, condition cannot match.
                return Ok(None);
            }

            // Speculative read: does not advance the published cursor (see
            // `raw_read_next_oplog_entry`). The cursor is advanced only when an entry is committed
            // below, so a rolled-back probe leaves no globally observable state behind — other tasks
            // never see a transient cursor position that is about to be undone.
            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;

            if self.is_awaited_terminal(&entry).await {
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

    /// Commits a just-read non-terminal-skipping entry: skip any trailing hint entries, advance the
    /// non-hint marker, apply this entry's commit-only side effects, route it to the concurrent
    /// resolver, and signal cursor progress to any suspended awaiter.
    async fn commit_consumed_entry(
        &self,
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
        // `skip_forward_locked` (which reads forward from the advanced cursor).
        self.move_replay_idx(read_idx).await;
        self.skip_forward_locked().await?;
        self.last_replayed_non_hint_index.set(read_idx);
        // Committed-consume hook: this entry is now permanently consumed (speculative reads never
        // reach here — they return before committing), so it is safe to feed the concurrent replay
        // resolver.
        self.on_committed_replay_entry(read_idx, entry).await;
        self.signal_progress();
        Ok(())
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` currently has a registered
    /// resolver awaiter (and is therefore an *awaited terminal* the cursor auto-drains to its owner
    /// rather than handing to a positional reader).
    async fn is_awaited_terminal(&self, entry: &OplogEntry) -> bool {
        let start_index = match entry {
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                *start_index
            }
            _ => return false,
        };
        self.internal
            .read()
            .await
            .concurrent_resolver
            .is_pending(start_index)
    }

    /// Skips trailing hint entries (and persist-nothing zones) following the just-committed entry,
    /// recording any log hints, then leaves the cursor on the next non-hint entry without consuming
    /// it. Assumes [`Self::cursor_lock`] is held.
    async fn skip_forward_locked(&self) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs = HashSet::new();
        while self.is_replay() {
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
                        let hash = Self::hash_log_entry(*level, context, message);
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

        self.has_seen_logs
            .store(!logs.is_empty(), Ordering::Relaxed);
        let mut internal = self.internal.write().await;
        internal.log_hashes = logs;
        Ok(())
    }

    /// Returns true if the given log entry has been seen since the last non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        if self.has_seen_logs.load(Ordering::Relaxed) {
            let hash = Self::hash_log_entry(level, context, message);
            let internal = self.internal.read().await;
            internal.log_hashes.contains(&hash)
        } else {
            false
        }
    }

    /// Removes a seen log from the set. If the set becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&self, level: LogLevel, context: &str, message: &str) {
        let hash = Self::hash_log_entry(level, context, message);
        let mut internal = self.internal.write().await;
        internal.log_hashes.remove(&hash);
        self.has_seen_logs
            .store(!internal.log_hashes.is_empty(), Ordering::Relaxed);
    }

    fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
        let mut hasher = MetroHash128::new();
        hasher.write_u8(level as u8);
        hasher.write(context.as_bytes());
        hasher.write(message.as_bytes());
        hasher.finish128()
    }

    /// Reads the next oplog entry (the one right after the committed cursor) **without** advancing
    /// the published cursor and **without** applying any replay side effects. This is the
    /// *speculative* read: the caller either commits it (via [`Self::commit_consumed_entry`] / the
    /// skip path, which call [`Self::move_replay_idx`] to publish the advance and
    /// [`Self::apply_commit_effects`] to apply side effects) or discards it. Because nothing is
    /// published, a discarded read leaves no globally observable state behind — other tasks never see
    /// a transient cursor position or a half-applied side effect. This is what the concurrent cursor
    /// relies on, since a speculative read whose predicate fails (parking) is now a normal path.
    ///
    /// Returns the index it read and the entry. Returns an error (rather than panicking) if the
    /// expected entry is missing, so the caller propagates a non-retriable trap instead of crashing
    /// the executor process.
    async fn raw_read_next_oplog_entry(
        &self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        let read_idx = self.last_replayed_index.get().next();

        let oplog_entries = self.read_oplog(read_idx, 1).await;
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
                    self.owned_agent_id,
                    read_idx,
                    self.replay_target.get(),
                    self.last_replayed_non_hint_index.get()
                ),
            ));
        };

        Ok((read_idx, oplog_entry))
    }

    /// Applies the replay side effects of an entry that is being **permanently consumed** at
    /// `read_idx`. Split out of the raw read so it fires only on commit, never on a rolled-back
    /// speculative read. Called for the entry returned to a caller, and for each hint /
    /// persist-nothing-zone entry skipped past in [`Self::skip_forward_locked`].
    async fn apply_commit_effects(
        &self,
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
            })
            .await
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
                let mut internal = self.internal.write().await;
                internal.pending_fork_starts.insert(read_idx);
            }
            OplogEntry::End {
                start_index,
                response: Some(response_payload),
                ..
            } => {
                let is_pending = {
                    let mut internal = self.internal.write().await;
                    internal.pending_fork_starts.remove(start_index)
                };
                if is_pending {
                    let response = self
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
                        })
                        .await;
                    }
                }
            }
            _ => {}
        }

        if read_idx == self.replay_target.get() {
            self.record_replay_event(ReplayEvent::ReplayFinished).await
        }

        Ok(())
    }

    async fn move_replay_idx(&self, new_idx: OplogIndex) {
        self.last_replayed_index.set(new_idx);
        self.get_out_of_skipped_region().await;
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

    pub async fn lookup_oplog_entry_with_condition_and_state<State>(
        &self,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        mut state: State,
        mut update_state: impl FnMut(&OplogEntry, OplogIndex, &mut State),
    ) -> OplogEntryLookupResult {
        let replay_target = self.replay_target.get();
        let mut start = self.last_replayed_index.get().next();

        const CHUNK_SIZE: u64 = 1024;

        let mut current_next_skip_region = self.internal.read().await.next_skipped_region.clone();
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
                    current_next_skip_region = self
                        .internal
                        .read()
                        .await
                        .skipped_regions
                        .find_next_deleted_region(idx.next());
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
                        let invocation_payload =
                            self.oplog.download_payload(payload).await.map_err(|err| {
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
                        let result: AgentInvocationResult =
                            self.oplog.download_payload(result).await.map_err(|err| {
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

    async fn get_out_of_skipped_region(&self) {
        if self.is_replay() {
            let mut internal = self.internal.write().await;
            let update_next_skipped_region = match &internal.next_skipped_region {
                Some(region) if region.start == (self.last_replayed_index.get().next()) => {
                    let target = region.end.next(); // we want to continue reading _after_ the region
                    debug!(
                        "Worker reached skipped region at {}, jumping to {} (oplog size: {})",
                        region.start,
                        target,
                        self.replay_target.get()
                    );
                    self.last_replayed_index.set(target.previous()); // so we set the last replayed index to the end of the region

                    true
                }
                _ => false,
            };

            if update_next_skipped_region {
                internal.next_skipped_region = internal
                    .skipped_regions
                    .find_next_deleted_region(self.last_replayed_index.get());
            }
        }
    }

    async fn read_oplog(&self, idx: OplogIndex, n: u64) -> Vec<(OplogIndex, OplogEntry)> {
        self.oplog.read_many(idx, n).await.into_iter().collect()
    }

    /// Feeds the concurrent replay resolver when an `End`/`Cancelled` entry is *committed*
    /// (permanently consumed). Resolves only calls that are actually being awaited
    /// (`resolve_if_pending`), so the `End`/`Cancelled` of any call not tracked by the resolver —
    /// e.g. the guest-facing manual durability pair, consumed through this same cursor but never
    /// registered — is ignored instead of leaking.
    async fn on_committed_replay_entry(&self, idx: OplogIndex, entry: &OplogEntry) {
        match entry {
            OplogEntry::End {
                start_index,
                response,
                forced_commit,
                ..
            } => {
                let mut internal = self.internal.write().await;
                internal.concurrent_resolver.resolve_if_pending(
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
                let mut internal = self.internal.write().await;
                internal.concurrent_resolver.resolve_if_pending(
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
    /// to their awaiter by `start_index` (the resolver) instead of by position. In short: `Start`
    /// order is a deterministic *output* of replay; completion order is the non-deterministic
    /// *input* we recorded and must replay.
    ///
    /// This relies on the `Start` being appended synchronously at the guest's initiation point.
    /// While durable host calls are serialized (each holds the store for its whole duration) that
    /// holds trivially. Once calls genuinely overlap, the positional claim stays valid for the same
    /// reason; the cursor advances past a `Start` only by a claim (here), while `End`/`Cancelled`
    /// entries are auto-drained to their awaiter by [`Self::try_get_oplog_entry_locked`] whoever
    /// happens to drive the cursor to them.
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
            self.internal
                .write()
                .await
                .concurrent_resolver
                .unregister(claimed.handle.start_idx());
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
    /// function name to the guest and therefore has no expected name to validate against. Callers
    /// that do know the expected identity should use [`Self::claim_concurrent_start`] so the
    /// name/type mismatch is caught at claim time (an `End` carries no identity of its own).
    ///
    /// The positional claim is sound for the same reason explained on
    /// [`Self::claim_concurrent_start`]: `Start` order is a deterministic output of replay.
    ///
    /// The `Start` consume and the resolver registration happen **atomically** under the cursor
    /// lock. This is required for concurrent replay: if the cursor advanced past the `Start` before
    /// the awaiter was registered, this call's `End` arriving at the head in that window would not
    /// be recognised as an awaited terminal and could be wrongly consumed by a positional reader.
    pub async fn claim_any_concurrent_start(
        &self,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let claimed = {
            let _cursor = self.cursor_lock.lock().await;
            let read = self
                .try_get_oplog_entry_locked(|entry| matches!(entry, OplogEntry::Start { .. }))
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
            // Register while still holding the cursor lock, so the `Start` consume + registration is
            // a single transaction.
            let receiver = {
                let mut internal = self.internal.write().await;
                internal.concurrent_resolver.register(start_idx)
            };
            ClaimedConcurrentStart {
                handle: ReplayCallHandle::new(start_idx, receiver),
                function_name,
                durable_function_type,
                timestamp,
            }
        };
        // A newly-registered awaiter means an `End`/`Cancelled` already sitting at (or arriving at)
        // the cursor head may now be a drainable awaited terminal: wake suspended awaiters so they
        // re-drive the cursor.
        self.signal_progress();
        Ok(claimed)
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

    /// Drains every *awaited terminal* (`End`/`Cancelled` whose `start_index` has a registered
    /// awaiter) currently at the cursor head, routing each to its awaiter, then stops at the first
    /// non-terminal entry without consuming it. This is the cursor-driving half of
    /// [`Self::await_resolution_outcome`]; it never blocks (it parks by returning, not suspending).
    async fn drain_awaited_terminals(&self) -> Result<(), WorkerExecutorError> {
        let _cursor = self.cursor_lock.lock().await;
        // `|_| false` never matches a non-terminal, so the locked transaction only auto-drains the
        // awaited terminals at the head and then returns `None` on the first non-terminal entry (or
        // at end-of-replay) without consuming it.
        let _ = self.try_get_oplog_entry_locked(|_| false).await?;
        Ok(())
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
    /// suspension, so other in-flight calls can drive the cursor while this one sleeps — which is
    /// what lets overlapping calls' `End`s, recorded in a non-deterministic completion order,
    /// replay out of claim order.
    pub async fn await_resolution_outcome(
        &self,
        handle: ReplayCallHandle,
    ) -> Result<ResolutionOutcome, WorkerExecutorError> {
        let (start_idx, mut receiver) = handle.into_parts();

        loop {
            // Register interest in cursor progress before inspecting the cursor, so a signal that
            // races the inspection below is delivered to the suspension at the end of the loop.
            let progress = self.progress.notified();
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
                    self.internal
                        .write()
                        .await
                        .concurrent_resolver
                        .unregister(start_idx);
                    return Err(WorkerExecutorError::runtime(format!(
                        "concurrent replay resolver channel closed for Start at {start_idx}"
                    )));
                }
            }

            if self.is_live() {
                // Reached the end of the oplog without ever seeing the matching End/Cancelled: a
                // committed lone `Start` (a forced commit flushed it before its `End`, or a crash
                // happened in between). Drop the stale registration and report Incomplete so the
                // caller can re-execute the side effect and complete the existing `Start`.
                self.internal
                    .write()
                    .await
                    .concurrent_resolver
                    .unregister(start_idx);
                return Ok(ResolutionOutcome::Incomplete);
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
                            self.internal
                                .write()
                                .await
                                .concurrent_resolver
                                .unregister(start_idx);
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
        let internal = rs.internal.read().await;
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
            let internal = rs.internal.read().await;
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
            let internal = rs.internal.read().await;
            assert!(
                internal.pending_fork_starts.is_empty(),
                "rolled-back speculative read must not apply the fork Start side effect"
            );
        }

        // The committed consume does apply the side effect.
        let (idx, _) = rs.try_get_oplog_entry(|_| true).await.unwrap().unwrap();
        assert_eq!(idx, OplogIndex::from_u64(2));
        let internal = rs.internal.read().await;
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
        let internal = rs.internal.read().await;
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
            let internal = rs.internal.read().await;
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
            let internal = rs.internal.read().await;
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
            let internal = rs.internal.read().await;
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
        let internal = rs.internal.read().await;
        assert!(
            !internal.concurrent_resolver.is_pending(start_idx),
            "switch_to_live must unregister the parked awaiter"
        );
    }
}
