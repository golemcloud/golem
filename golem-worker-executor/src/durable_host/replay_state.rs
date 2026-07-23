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
use golem_common::model::card::{CardId, StoredCard};
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    AtomicOplogIndex, DurableFunctionType, HostRequest, HostResponse, HostResponseGolemApiFork,
    LogLevel, OplogEntry, OplogIndex, OplogPayload, PersistenceLevel,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentInvocationPayload, AgentInvocationResult, ForkResult, IdempotencyKey, OwnedAgentId,
    Timestamp,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use metrohash::MetroHash128;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;
use tokio::sync::{Mutex, MutexGuard, Notify};
use tracing::{debug, warn};
use uuid::Uuid;

const CHUNK_SIZE: u64 = 1024;

#[derive(Debug, Clone)]
pub enum ReplayEvent {
    ReplayFinished,
    UpdateReplayed { new_revision: ComponentRevision },
    ForkReplayed { new_phantom_id: Uuid },
    CardInstalled { card: StoredCard },
    CardRevoked { card_id: CardId },
    CardExpired { card_id: CardId },
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

/// Live-only abandoned durable-call records tolerated by the invocation-boundary positional read
/// ([`ReplayState::get_oplog_entry_agent_invocation_finished`]).
///
/// Live execution can make partial durable progress that replay legitimately never reproduces:
/// guest-side races (e.g. an HTTP body read racing a zero-length timer) are resolved by the
/// component runtime's scheduling at a granularity the oplog does not record, so a branch that
/// issued durable calls live — and was then abandoned by the guest — may never be re-issued on
/// replay. Those records (`Start`s no replayed call ever claims, plus the `End`/`Cancelled`
/// terminals that closed them) are dead by the time the invocation-finished marker is read: the
/// replayed guest has already produced its invocation result and nothing can claim them anymore.
///
/// The tolerance is deliberately structural and local to a single finished-marker read:
/// - only `Start`s whose committed consume is replay-inert are drained — a
///   dedicated-positional-consumer pair with commit-side replay effects (`GolemApiFork`) stays
///   fatal (see [`AbandonedStarts::can_drain`]);
/// - every drained `Start` must be closed by exactly one `End`/`Cancelled` before
///   `AgentInvocationFinished` — an unclosed or doubly-closed record stays fatal;
/// - terminals of `Start`s not drained by the same walk stay fatal;
/// - every other positional entry stays fatal;
/// - the tracker never survives past the finished-marker read, so a terminal leaking past
///   `AgentInvocationFinished` (a settlement-ordering bug at its producer) is not normalized
///   into accepted history.
#[derive(Default)]
struct AbandonedStarts {
    starts: HashMap<OplogIndex, AbandonedStart>,
}

struct AbandonedStart {
    function_name: HostFunctionName,
    parent_start_index: Option<OplogIndex>,
    terminal: Option<(&'static str, OplogIndex)>,
}

impl AbandonedStarts {
    /// Whether a never-claimed `Start` for `function_name` may be drained as live-only abandoned
    /// progress at the invocation boundary.
    ///
    /// `GolemApiFork` is excluded: it is a dedicated-positional-consumer pair whose committed
    /// consume is not inert — [`CursorTx::apply_commit_effects`] records the `Start` as a pending
    /// fork and decodes its matching `End` into a [`ReplayEvent::ForkReplayed`]. Draining such a
    /// pair here would apply a fork the replayed guest never requested, so it stays fatal. Every
    /// other `Start`/`End` commit is side-effect-free (the `End` arm of `apply_commit_effects`
    /// only fires for pending fork starts), so draining them is genuinely inert. Any new
    /// function-specific commit effect added to `apply_commit_effects` must be excluded here too.
    fn can_drain(function_name: &HostFunctionName) -> bool {
        !matches!(function_name, HostFunctionName::GolemApiFork)
    }

    fn contains(&self, start_index: OplogIndex) -> bool {
        self.starts.contains_key(&start_index)
    }

    fn record_start(
        &mut self,
        idx: OplogIndex,
        function_name: HostFunctionName,
        parent_start_index: Option<OplogIndex>,
    ) {
        self.starts.insert(
            idx,
            AbandonedStart {
                function_name,
                parent_start_index,
                terminal: None,
            },
        );
    }

    fn record_terminal(
        &mut self,
        start_index: OplogIndex,
        terminal_idx: OplogIndex,
        kind: &'static str,
    ) -> Result<(), WorkerExecutorError> {
        let start = self.starts.get_mut(&start_index).ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "abandoned-record tracker has no Start for terminal {kind} at {terminal_idx} \
                 (start_index {start_index})"
            ))
        })?;
        if let Some((prior_kind, prior_idx)) = &start.terminal {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "at most one terminal per abandoned Start",
                format!(
                    "{kind} at {terminal_idx} closing abandoned Start {start_index} ({:?}) \
                     already closed by {prior_kind} at {prior_idx}",
                    start.function_name
                ),
            ));
        }
        start.terminal = Some((kind, terminal_idx));
        Ok(())
    }

    /// Validates that every drained abandoned `Start` is terminally closed and emits a single
    /// summary warning for the tolerated records. Called when the boundary walk reaches
    /// `AgentInvocationFinished`.
    fn finish(self, owned_agent_id: &OwnedAgentId) -> Result<(), WorkerExecutorError> {
        if self.starts.is_empty() {
            return Ok(());
        }

        let open: Vec<_> = self
            .starts
            .iter()
            .filter(|(_, start)| start.terminal.is_none())
            .map(|(idx, start)| format!("{idx} ({:?})", start.function_name))
            .collect();
        if !open.is_empty() {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "an End/Cancelled closing every abandoned Start before AgentInvocationFinished",
                format!(
                    "AgentInvocationFinished with unclosed abandoned Start(s) at {}",
                    open.join(", ")
                ),
            ));
        }

        let mut records: Vec<_> = self.starts.iter().collect();
        records.sort_by_key(|(idx, _)| **idx);
        let roots = records
            .iter()
            .filter(|(_, start)| {
                start
                    .parent_start_index
                    .is_none_or(|parent| !self.starts.contains_key(&parent))
            })
            .count();
        let ended = records
            .iter()
            .filter(|(_, start)| matches!(start.terminal, Some(("End", _))))
            .count();
        let summary: Vec<_> = records
            .iter()
            .map(|(idx, start)| {
                format!(
                    "{idx}: {:?} (parent: {:?}, terminal: {:?})",
                    start.function_name, start.parent_start_index, start.terminal
                )
            })
            .collect();
        warn!(
            "replay of {owned_agent_id} skipped {} abandoned durable-call record(s) \
             ({roots} root(s), {ended} closed by End, {} by Cancelled) at the invocation \
             boundary — live-only progress the replayed guest abandoned earlier: [{}]",
            records.len(),
            records.len() - ended,
            summary.join("; ")
        );
        Ok(())
    }
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
    /// Durable calls whose successful `End` was persisted but whose completion was never
    /// delivered to the guest (the guest dropped the accessor completion future), keyed by the
    /// call's `Start` index and mapping to the physical index of the `CompletionDiscarded`
    /// marker entry. Populated by a scan bounded by the *initial replay target* at construction
    /// ([`ReplayState::new`]) — the marker always lies physically *after* its `End`, so replay
    /// must know about it before the cursor reaches the `End`; discovering it via ordinary hint
    /// skipping would be too late. When the replay target grows ([`ReplayState::set_replay_target`],
    /// e.g. a debug session stepping to a later index), exactly the newly visible range is
    /// rescanned and merged before the new target is published, so the map never contains — nor
    /// misses — a marker relative to the effective target. Extended live via
    /// [`ReplayState::record_discarded_completion`] when a marker is appended by this instance.
    ///
    /// A `std` mutex (like `log_hashes`) rather than part of [`CursorState`]: the live marker
    /// recorder is an owned tokio task that must not queue on the cursor lock, and all accesses
    /// are short synchronous map operations that never hold the lock across an `await`.
    discarded_completions: std::sync::Mutex<HashMap<OplogIndex, OplogIndex>>,
    /// Hashes of log entries persisted since the last read non-hint oplog entry, with their
    /// number of occurrences. A counted multiset (not a set) because large or repetitive
    /// stdout/stderr output regularly produces identical consecutive log entries, each of which
    /// must be deduplicated exactly once on re-run.
    ///
    /// Deliberately *not* part of [`CursorState`]: `seen_log`/`remove_seen_log` are called from
    /// the stdio host-call path, which runs on store-keeping wasm fibers, and must never queue on
    /// the cursor lock — a transaction holds that lock across oplog IO from store-polled futures,
    /// which can deadlock the store (wasmtime#11869/#11870). This std mutex is only ever held for
    /// synchronous set operations, never across an `await`.
    log_hashes: std::sync::Mutex<HashMap<(u64, u64), usize>>,
    /// Replay events (updates, forks, replay-finished) encountered while reading the oplog,
    /// pending consumption by [`ReplayState::take_new_replay_events`].
    ///
    /// A `std` mutex (like `log_hashes`) rather than part of [`CursorState`]: the consumer is the
    /// durable-call begin path, which also runs from p2 `&mut self` host calls that hold exclusive
    /// store access and therefore must never queue on the cursor lock (a transaction holds that
    /// lock across oplog IO from store-polled futures, which can deadlock the store). Accesses are
    /// short synchronous vector operations, never held across an `await`.
    pending_replay_events: std::sync::Mutex<Vec<ReplayEvent>>,
    /// Fired (via `notify_waiters`) after a transaction that advanced the cursor, registered a
    /// resolver awaiter, or switched to live commits and releases [`Self::state`]. A durable call
    /// suspended in [`ReplayState::await_resolution_outcome`] — because its `End`/`Cancelled` is not
    /// yet at the cursor head while a concurrently-replaying sibling owns it — wakes on this to
    /// re-drive the cursor. Resolver delivery is the primary wakeup; this covers the "another
    /// consumer advanced the cursor past my blocker" case that a oneshot alone cannot.
    progress: Notify,
}

/// The published, lock-free-readable cursor position. The index fields are written only while
/// [`ReplayCursor::state`] is held (through a [`CursorTx`]), so a lock-free reader never observes
/// a partially-applied advance; `has_seen_logs` is written only while [`ReplayCursor::log_hashes`]
/// is held.
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
    /// Entries prefetched from the oplog, beginning at the next speculative cursor position.
    replay_buffer: VecDeque<(OplogIndex, OplogEntry)>,
    /// `Start` entries for `GolemApiFork` whose matching `End` has not yet been replayed. When the
    /// matching `End` is read, the response is decoded and a `ForkReplayed` event is emitted. The
    /// sequential adapter only ever has at most one in flight at a time (it writes the matched
    /// `End` immediately after the `Start`), but we use a set so that future concurrent recorders cannot
    /// trip us up.
    pending_fork_starts: HashSet<OplogIndex>,
    /// Matches replayed `End`/`Cancelled` entries to the concurrent
    /// [`crate::durable_host::concurrent::CallHandle`]s awaiting them, keyed by their `Start` index.
    /// Fed only from the committed-consume hook. Lives under the cursor lock because awaited-terminal
    /// detection, terminal resolution, and `Start`-claim registration are all part of the cursor
    /// transaction; the rare slow-path `unregister` re-acquires the lock from outside a transaction.
    concurrent_resolver: ConcurrentReplayResolver,
    /// `Start` entries ahead of the cursor that were already claimed out-of-position by
    /// [`CursorTx::claim_owned_start`] (identity-keyed scan-ahead claims). When the cursor reaches
    /// such an entry it is auto-consumed like an awaited terminal instead of being handed to a
    /// positional reader; its resolver awaiter was registered at claim time.
    claimed_starts: HashSet<OplogIndex>,
}

impl ReplayCursor {
    /// Replaces the seen-log multiset and updates the `has_seen_logs` fast-path flag.
    fn set_log_hashes(&self, logs: HashMap<(u64, u64), usize>) {
        let has_logs = !logs.is_empty();
        *self.log_hashes.lock().unwrap() = logs;
        self.position
            .has_seen_logs
            .store(has_logs, Ordering::Relaxed);
    }

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
    async fn try_get_oplog_entry(
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
    async fn try_get_oplog_entry_at_invocation_boundary(
        &mut self,
        abandoned: &mut AbandonedStarts,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.try_get_oplog_entry_inner(Some(abandoned), condition)
            .await
    }

    async fn try_get_oplog_entry_inner(
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
    fn is_awaited_terminal(&self, entry: &OplogEntry) -> bool {
        let start_index = match entry {
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                *start_index
            }
            _ => return false,
        };
        self.st.concurrent_resolver.is_pending(start_index)
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` lies inside a skipped/deleted
    /// region. Such an *orphan terminal* is left behind when a jump/revert/fork/snapshot deletes
    /// the region containing a call's `Start` but not its terminal. Its `Start` can never be
    /// claimed (both the positional head consume and the scan-ahead claim jump over deleted
    /// regions), so no awaiter can ever exist for it; the cursor consumes it like a no-op instead
    /// of surfacing it to a positional reader as an unexpected entry.
    fn is_orphan_terminal(&self, entry: &OplogEntry) -> bool {
        let start_index = match entry {
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                *start_index
            }
            _ => return false,
        };
        self.st.skipped_regions.is_in_deleted_region(start_index)
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
    async fn move_replay_idx(&mut self, new_idx: OplogIndex) {
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

    async fn get_out_of_skipped_region(&mut self) {
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
    fn on_committed_replay_entry(&mut self, idx: OplogIndex, entry: &OplogEntry) {
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
    fn discarded_completion_marker(&self, start_index: OplogIndex) -> Option<OplogIndex> {
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

    fn record_replay_event(&mut self, event: ReplayEvent) {
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
    async fn claim_start_matching(
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
        let scan_result = self
            .cursor
            .scan_oplog(
                self.cursor.last_replayed_index().next(),
                self.cursor.replay_target(),
                &self.st.skipped_regions,
                self.st.next_skipped_region.clone(),
                OplogIndex::NONE,
                |entry, _begin_idx, state: &Option<OplogIndex>| {
                    state
                        .map(|idx| !already_claimed.contains(&idx))
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

    /// Claims the next top-level (unowned) durable-call `Start` **without** validating its
    /// function name or durable function type, registering a resolver receiver keyed by the
    /// `Start`'s index and returning the claimed entry's identity. The `Start` must carry a
    /// request (durable host calls always do; a request-less `Start` is a scope `Start`) and must
    /// not be owned by another durable record — owned `Start`s are claimed by their owner via
    /// [`Self::claim_owned_start`].
    async fn claim_any_concurrent_start(
        &mut self,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let (handle, entry) = self
            .claim_start_matching(
                |entry| {
                    matches!(
                        entry,
                        OplogEntry::Start {
                            request: Some(_),
                            parent_start_index: None,
                            ..
                        }
                    )
                },
                || "Start { request: Some(..), parent_start_index: None }".to_string(),
            )
            .await?;
        let OplogEntry::Start {
            timestamp,
            function_name,
            durable_function_type,
            ..
        } = *entry
        else {
            unreachable!("claim_start_matching only matches Start entries");
        };
        Ok(ClaimedConcurrentStart {
            handle,
            function_name,
            durable_function_type,
            timestamp,
        })
    }

    /// Claims the next durable-*scope* `Start` (a request-less, unowned scope `Start` such as
    /// `<scope:batched-write>` / `<scope:transaction>`) matching the expected function
    /// name and the expected durable function type, and registers a resolver awaiter keyed by its
    /// index so the matching scope `End` is routed through the resolver instead of being
    /// read positionally. Returns the scope's begin index and the handle its `end_function` /
    /// transaction-terminal awaits.
    ///
    /// The expected name must be exactly the name the live path recorded, including any
    /// discriminator suffix (a caller-supplied suffix that makes a concurrent scope claim-safe,
    /// e.g. `<scope:batched-write:req:HASH>`). There is no plain-name fallback: a discriminated
    /// claim must never match a plain scope `Start` (P3 deploys on a clean database, so every
    /// replayed oplog was recorded with the same naming scheme).
    ///
    /// Folding scope `End`s into the resolver is what lets a scope `End` be auto-drained by any
    /// cursor driver (so a positional reader never steals a concurrently-replaying sibling call's
    /// terminal, and the scope close never steals a sibling's), at the cost of nothing on the serial
    /// path: when the scope `End` is the entry at the cursor head, awaiting it resolves immediately.
    async fn claim_scope_start(
        &mut self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<(OplogIndex, ReplayCallHandle), WorkerExecutorError> {
        let (handle, _) = self
            .claim_start_matching(
                |entry| {
                    matches!(entry, OplogEntry::Start {
                        function_name,
                        request,
                        durable_function_type,
                        parent_start_index,
                        ..
                    } if request.is_none()
                        && function_name == expected_function_name
                        && durable_function_type == expected_function_type
                        && parent_start_index.is_none())
                },
                || {
                    format!(
                        "Start {{ {expected_function_name}, {expected_function_type:?}, request: None, parent_start_index: None }}"
                    )
                },
            )
            .await?;
        let start_idx = handle.start_idx();
        // Every durable scope `Start` consumed during
        // replay leaves a registered awaiter, so its `End` is always a resolver-routed *awaited
        // terminal* and never an orphan that a parked awaiter behind it could sleep on until
        // `switch_to_live`. The only un-drained terminals the cursor may leave at its head are then
        // the dedicated-positional-consumer pairs (manual durability, `GolemApiFork`).
        debug_assert!(
            self.st.concurrent_resolver.is_pending(start_idx),
            "scope Start claim at {start_idx} must leave a registered awaiter"
        );
        Ok((start_idx, handle))
    }

    /// Claims the next top-level (unowned) durable-call `Start` matching the expected function
    /// name and durable function type, registering a resolver receiver keyed by the `Start`'s
    /// index. See [`Self::claim_start_matching`] for the identity-based claim semantics.
    ///
    /// "Unowned" means the caller did not open its own durable scope; the recorded
    /// `parent_start_index` is still the scope encoded in the durable function type when there is
    /// one (batched / transaction `Some(begin_index)`), mirroring how the write side derives it.
    async fn claim_unowned_start(
        &mut self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_parent = parent_start_index_of(expected_function_type);
        let (handle, _) = self
            .claim_start_matching(
                |entry| {
                    matches!(entry, OplogEntry::Start {
                        function_name,
                        request,
                        durable_function_type,
                        parent_start_index,
                        ..
                    } if function_name == expected_function_name
                        && request.is_some()
                        && durable_function_type == expected_function_type
                        && *parent_start_index == expected_parent)
                },
                || {
                    format!(
                        "Start {{ {expected_function_name}, {expected_function_type:?}, request: Some(..), parent_start_index: {expected_parent:?} }}"
                    )
                },
            )
            .await?;
        Ok(handle)
    }

    /// Claims the `Start` entry of a durable call that is *owned* by another durable record
    /// (`parent_start_index` points at the owning scope/call `Start`), matching by identity
    /// (function name, durable function type, request presence, parent index). Matching `Start`s
    /// that share the same full identity (several chunks under one parent) are claimed in oplog
    /// order, preserving the deterministic per-parent chain order. See
    /// [`Self::claim_start_matching`] for the identity-based claim semantics.
    async fn claim_owned_start(
        &mut self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_parent_start_index: OplogIndex,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let (handle, _) = self
            .claim_start_matching(
                |entry| {
                    matches!(entry, OplogEntry::Start {
                        function_name,
                        request,
                        durable_function_type,
                        parent_start_index,
                        ..
                    } if function_name == expected_function_name
                        && request.is_some()
                        && durable_function_type == expected_function_type
                        && *parent_start_index == Some(expected_parent_start_index))
                },
                || {
                    format!(
                        "Start {{ {expected_function_name}, {expected_function_type:?}, parent_start_index: Some({expected_parent_start_index}) }}"
                    )
                },
            )
            .await?;
        Ok(handle)
    }

    /// Claims the next top-level (unowned) durable-call `Start` whose identity **and recorded
    /// request payload** match. Payload matching is what disambiguates concurrent durable calls
    /// that share the same function name and durable function type but were issued with different
    /// requests (e.g. parallel P3 HTTP sends): their `Start` entries land in the oplog in
    /// scheduling order, so identity alone would pair a replayed call with another call's record —
    /// and consequently deliver another call's recorded response. Calls with equal requests are
    /// still claimed in oplog order among the matches.
    ///
    /// `expected_request` must be the [`HostRequest`] value the live path would have persisted in
    /// the `Start` entry; see [`recorded_request_payload_matches`] for the value-based comparison.
    async fn claim_unowned_start_matching_request(
        &mut self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_parent = parent_start_index_of(expected_function_type);
        let (handle, _) = self
            .claim_start_matching(
                |entry| {
                    matches!(entry, OplogEntry::Start {
                        function_name,
                        request: Some(request),
                        durable_function_type,
                        parent_start_index,
                        ..
                    } if function_name == expected_function_name
                        && durable_function_type == expected_function_type
                        && *parent_start_index == expected_parent
                        && recorded_request_payload_matches(request, expected_request))
                },
                || {
                    format!(
                        "Start {{ {expected_function_name}, {expected_function_type:?}, request: Some(<matching payload>), parent_start_index: {expected_parent:?} }}"
                    )
                },
            )
            .await?;
        Ok(handle)
    }

    /// Claims the `Start` entry of a durable call owned by another durable record, matching by
    /// identity **and recorded request payload** — the owned counterpart of
    /// [`Self::claim_unowned_start_matching_request`]. With a claim-safe parent (a discriminated
    /// scope) the parent index already pins the call, so the payload match acts as a cheap
    /// validation that the claimed record really belongs to this call.
    async fn claim_owned_start_matching_request(
        &mut self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_parent_start_index: OplogIndex,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let (handle, _) = self
            .claim_start_matching(
                |entry| {
                    matches!(entry, OplogEntry::Start {
                        function_name,
                        request: Some(request),
                        durable_function_type,
                        parent_start_index,
                        ..
                    } if function_name == expected_function_name
                        && durable_function_type == expected_function_type
                        && *parent_start_index == Some(expected_parent_start_index)
                        && recorded_request_payload_matches(request, expected_request))
                },
                || {
                    format!(
                        "Start {{ {expected_function_name}, {expected_function_type:?}, request: Some(<matching payload>), parent_start_index: Some({expected_parent_start_index}) }}"
                    )
                },
            )
            .await?;
        Ok(handle)
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
        // Scan-ahead-claimed `Start`s the cursor never reached are moot now: their awaiters were
        // just failed with `Incomplete`, and the cursor will not read again.
        self.st.claimed_starts.clear();
        self.st.replay_buffer.clear();
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
    async fn scan_discarded_completions(
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

    pub async fn drop_override_and_restart(&self) -> Result<(), WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        let result = tx.drop_override_and_restart().await;
        cursor.finish_tx(tx);
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
    async fn run_owned_cursor_op<R, Fut>(
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
                let cursor = &*state.cursor;
                let mut tx = cursor.tx().await;
                tx.switch_to_live();
                cursor.finish_tx(tx);
                // `CursorTx::switch_to_live` publishes the cursor position directly (not via
                // `move_replay_idx`), so replay-progress observers are notified here.
                cursor
                    .oplog
                    .on_replay_progress(cursor.last_replayed_index())
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
        let mut tx = cursor.tx().await;
        let result = async {
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
        }
        .await;
        cursor.finish_tx(tx);
        result
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

    /// [`Self::get_oplog_entry`] variant for callers running inside Wasmtime accessor futures:
    /// the cursor transaction runs on an owned task (see [`Self::run_owned_cursor_op`]), so the
    /// store-polled caller never queues on the cursor mutex directly. Direct invocation-loop /
    /// p2 host-call readers keep using [`Self::get_oplog_entry`].
    pub async fn get_oplog_entry_owned(
        &self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
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
    async fn get_oplog_entry_at_invocation_boundary(
        &self,
        abandoned: &mut AbandonedStarts,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        let cursor = &*self.cursor;
        let mut tx = cursor.tx().await;
        let result = tx
            .try_get_oplog_entry_at_invocation_boundary(abandoned, |_| true)
            .await;
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

    /// Claims the next top-level (unowned) durable-call `Start` matching the expected identity
    /// (function name, durable function type, request presence) and registers a resolver receiver
    /// keyed by the `Start`'s index. See [`CursorTx::claim_start_matching`].
    ///
    /// The claim is identity-based rather than strictly positional because top-level durable calls
    /// may be issued from concurrently running host tasks (e.g. parallel P3 HTTP sends), whose
    /// `Start` entries land in the oplog in network/scheduling order that replay does not
    /// reproduce. The head fast path keeps the serial case positional and free; otherwise the
    /// first not-yet-claimed matching `Start` ahead of the cursor is scan-ahead-claimed.
    /// `Start`s sharing the same identity are claimed in oplog order, preserving the deterministic
    /// per-task initiation order.
    ///
    /// `End` entries carry no function identity, so identity matching must happen here, at claim
    /// time. The request payload is not decoded: `function_name` already pins the request type
    /// (and the `Req` associated type has no `TryFrom<HostRequest>` to decode it generically); the
    /// response is fully type-checked on the `End` side during replay.
    pub async fn claim_concurrent_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_unowned_start(&expected_function_name, &expected_function_type)
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
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
        self.run_owned_cursor_op(|state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx.claim_any_concurrent_start().await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the `Start` of a durable call owned by another durable record (its
    /// `parent_start_index`) by identity instead of position, scan-ahead-claiming a matching
    /// `Start` ahead of the cursor when concurrent host tasks interleaved the live append order.
    /// See [`CursorTx::claim_owned_start`].
    pub async fn claim_owned_concurrent_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_owned_start(
                    &expected_function_name,
                    &expected_function_type,
                    parent_start_index,
                )
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the next durable-scope `Start` matching exactly the expected name and registers a
    /// resolver awaiter for it, so its matching scope `End` is consumed through
    /// [`Self::await_resolution_outcome`] rather than a positional read. See
    /// [`CursorTx::claim_scope_start`].
    pub async fn claim_scope_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<(OplogIndex, ReplayCallHandle), WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_scope_start(&expected_function_name, &expected_function_type)
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the next top-level durable-call `Start` matching identity **and recorded request
    /// payload**; see [`CursorTx::claim_unowned_start_matching_request`].
    pub async fn claim_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        let expected_request = expected_request.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_unowned_start_matching_request(
                    &expected_function_name,
                    &expected_function_type,
                    &expected_request,
                )
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the `Start` of a durable call owned by another durable record, matching identity
    /// **and recorded request payload**; see [`CursorTx::claim_owned_start_matching_request`].
    pub async fn claim_owned_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        let expected_request = expected_request.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_owned_start_matching_request(
                    &expected_function_name,
                    &expected_function_type,
                    parent_start_index,
                    &expected_request,
                )
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Drops a resolver awaiter from outside a cursor transaction. Acquires the cursor lock briefly
    /// (on an owned task; callers are accessor futures); callers must not hold it (the await loop
    /// releases it before parking).
    async fn unregister_awaiter(&self, start_idx: OplogIndex) {
        let result = self
            .run_owned_cursor_op(move |state| async move {
                let mut st = state.cursor.state.lock().await;
                st.concurrent_resolver.unregister(start_idx);
                Ok(())
            })
            .await;
        if let Err(err) = result {
            warn!("unregister_awaiter cursor operation did not complete: {err}");
        }
    }

    /// Drains every *awaited terminal* (`End`/`Cancelled` whose `start_index` has a registered
    /// awaiter) currently at the cursor head, routing each to its awaiter, then stops at the first
    /// non-terminal entry without consuming it. This is the cursor-driving half of
    /// [`Self::await_resolution_outcome`]; it never blocks (it parks by returning, not suspending).
    async fn drain_awaited_terminals(&self) -> Result<(), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            // `|_| false` never matches a non-terminal, so the transaction only auto-drains the
            // awaited terminals at the head and then returns `None` on the first non-terminal
            // entry (or at end-of-replay) without consuming it.
            let result = tx.try_get_oplog_entry(|_| false).await;
            cursor.finish_tx(tx);
            result.map(|_| ())
        })
        .await
    }

    /// Delivery-time validation of a resolved outcome: a `CompletedButDiscarded` resolution whose
    /// marker lies *beyond* the effective replay target is an invalid replay configuration — the
    /// target falls between the call's successful `End` and its `CompletionDiscarded` marker, so
    /// the delivery status of the `End` cannot be decided from the visible oplog prefix. Debug
    /// target validation and cut-point validation reject such targets up front; this check is
    /// defense in depth for any other path that bounds replay between the two entries. Future
    /// knowledge (the marker beyond the target) is only ever used to *reject* the target, never
    /// to decide a call's outcome within it.
    fn validate_resolved_outcome(
        &self,
        outcome: ResolutionOutcome,
    ) -> Result<ResolutionOutcome, WorkerExecutorError> {
        if let ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            ..
        }) = &outcome
        {
            let target = self.replay_target();
            if *marker_idx > target {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "invalid replay target {target}: it lies between a durable call's successful End at {end_idx} and its CompletionDiscarded marker at {marker_idx}, so the delivery status of the completion is undecidable at this target"
                )));
            }
        }
        Ok(outcome)
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
        let validate = |outcome: ResolutionOutcome| self.validate_resolved_outcome(outcome);

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
                Ok(outcome) => return validate(outcome),
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
                // incomplete, so a just-resolved final terminal is never misreported. The lock is
                // taken on an owned task (this is an accessor future; see `run_owned_cursor_op`),
                // which owns the receiver for the duration of the check; every branch is terminal,
                // so the receiver never needs to be handed back.
                let outcome = self
                    .run_owned_cursor_op(move |state| async move {
                        let mut st = state
                            .cursor
                            .state.lock()
                            .await;
                        match receiver.try_recv() {
                            Ok(outcome) => Ok(outcome),
                            Err(oneshot::error::TryRecvError::Empty) => {
                                // Genuinely reached the end of the oplog without the matching
                                // `End`/`Cancelled`: a committed lone `Start` (a forced commit
                                // flushed it before its `End`, or a crash happened in between).
                                // Drop the stale registration and report Incomplete so the caller
                                // can re-execute the side effect and complete the existing `Start`.
                                st.concurrent_resolver.unregister(start_idx);
                                Ok(ResolutionOutcome::Incomplete)
                            }
                            Err(oneshot::error::TryRecvError::Closed) => {
                                st.concurrent_resolver.unregister(start_idx);
                                Err(WorkerExecutorError::runtime(format!(
                                    "concurrent replay resolver channel closed for Start at {start_idx}"
                                )))
                            }
                        }
                    })
                    .await?;
                return match outcome {
                    ResolutionOutcome::Incomplete => Ok(ResolutionOutcome::Incomplete),
                    resolved => validate(resolved),
                };
            }

            // This call's terminal is not at the cursor head and replay is not over: a
            // concurrently-replaying sibling owns the cursor head. Suspend until our resolution
            // arrives or the cursor advances, then re-drive.
            tokio::select! {
                biased;
                resolved = &mut receiver => {
                    return match resolved {
                        Ok(outcome) => validate(outcome),
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

/// The `parent_start_index` a durable call's `Start` entry is recorded with when the caller does
/// not open its own durable scope: the scope explicitly encoded in the durable function type
/// (batched / transaction `Some(begin_index)`), or `None` for top-level calls. This mirrors the
/// derivation on the write side (`persist_durable_function_invocation` and the accessor start
/// path), so identity-based claims can reproduce the recorded value.
fn parent_start_index_of(function_type: &DurableFunctionType) -> Option<OplogIndex> {
    match function_type {
        DurableFunctionType::WriteRemoteBatched(Some(idx))
        | DurableFunctionType::WriteRemoteTransaction(Some(idx)) => Some(*idx),
        _ => None,
    }
}

/// Whether a recorded `Start` request payload equals the expected request *value*. The comparison
/// must be by value, never by serialized bytes: payload types can contain `HashMap`s (e.g. the
/// header map of a P3 HTTP request head), whose serialization order depends on the process-random
/// hasher seed, so bytes recorded before a restart do not reproduce.
fn recorded_request_payload_matches(
    recorded: &OplogPayload<HostRequest>,
    expected: &HostRequest,
) -> bool {
    match recorded {
        OplogPayload::Inline(value) => value.as_ref() == expected,
        OplogPayload::SerializedInline {
            cached: Some(cached),
            ..
        }
        | OplogPayload::External {
            cached: Some(cached),
            ..
        } => cached.as_ref() == expected,
        OplogPayload::SerializedInline {
            bytes,
            cached: None,
        } => golem_common::serialization::deserialize::<HostRequest>(bytes)
            .map(|value| &value == expected)
            .unwrap_or(false),
        // The payload lives in external storage and the synchronous claim matcher cannot fetch
        // it, so it is accepted as a match: the identity filters (function name, type, parent)
        // still apply, and among equally-identified candidates the claim falls back to oplog
        // order — the pre-payload-matching behavior.
        OplogPayload::External { cached: None, .. } => true,
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
    use crate::services::oplog::{CommitLevel, OrderedOplogStart, PendingUpload};
    use async_trait::async_trait;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::payload::types::{
        SerializableP3HttpBodyChunk, SerializableP3HttpConsumeBodyResult,
    };
    use golem_common::model::oplog::{
        AgentError, DurableFunctionType, HostRequest, HostRequestNoInput,
        HostResponseMonotonicClockTimestamp, HostResponseP3HttpClientConsumeBodyChunk,
        HostResponseP3HttpClientConsumeBodyResult, OplogPayload, PayloadId, RawOplogPayload,
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

        async fn add_start_with_reserved_raw_payload(
            &self,
            serialized_request: Vec<u8>,
            build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        ) -> Result<OrderedOplogStart, String> {
            let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
            let index = self.add(entry.clone()).await;
            Ok(OrderedOplogStart {
                index,
                entry,
                pending_upload: PendingUpload::already_durable(),
            })
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

    /// A `Start` for the sequential `golem::api` fork pair. Its only special replay behaviour is the
    /// commit-only side effect in [`ReplayState::apply_commit_effects`] (recording its index in
    /// `pending_fork_starts`), which the speculative-rollback test exercises.
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

    fn stdout_log(message: &str) -> OplogEntry {
        OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            level: LogLevel::Stdout,
            context: "stdout".to_string(),
            message: message.to_string(),
        }
    }

    /// Identical log entries persisted multiple times since the last non-hint entry must each be
    /// deduplicated exactly once on re-run: the seen-log collection is a counted multiset, not a
    /// set. Large or repetitive stdout output regularly produces identical consecutive chunks, so
    /// losing multiplicity would re-persist all but the first occurrence on every recovery.
    #[test]
    async fn seen_log_tracks_multiplicity_of_identical_entries() {
        // All entries are hints: constructing the replay state skips them all and records the
        // log hashes.
        let rs = replay_state_over(vec![
            noop(),
            stdout_log("X"),
            stdout_log("X"),
            stdout_log("X"),
            stdout_log("Y"),
        ])
        .await;

        for remaining in (1..=3).rev() {
            assert!(
                rs.seen_log(LogLevel::Stdout, "stdout", "X").await,
                "X must still be seen with {remaining} unmatched occurrence(s) left"
            );
            rs.remove_seen_log(LogLevel::Stdout, "stdout", "X").await;
        }
        assert!(
            !rs.seen_log(LogLevel::Stdout, "stdout", "X").await,
            "all three occurrences of X are matched"
        );

        // Removing more occurrences than were recorded must not underflow or affect others.
        rs.remove_seen_log(LogLevel::Stdout, "stdout", "X").await;
        assert!(!rs.seen_log(LogLevel::Stdout, "stdout", "X").await);
        assert!(rs.seen_log(LogLevel::Stdout, "stdout", "Y").await);
        rs.remove_seen_log(LogLevel::Stdout, "stdout", "Y").await;
        assert!(!rs.seen_log(LogLevel::Stdout, "stdout", "Y").await);
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
            format!("{err}").contains("WriteRemote"),
            "the error must spell out the mismatched expected identity, got: {err}"
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
        // A speculative read whose predicate fails rolls the cursor back AND applies none of the
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

    /// A claimed call whose awaiter was dropped without awaiting (the accessor future awaiting
    /// the resolution was cancelled) must not wedge the cursor: when the cursor reaches the
    /// call's terminal, the drain routes it to the closed receiver (the send fails silently) and
    /// drops the registration, leaving no resolver residue behind.
    #[test]
    async fn dropped_awaiter_terminal_drains_without_residue() {
        // [NoOp, Start(2), End(2→3), NoOp(4)]
        let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42), noop()]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let start_idx = handle.start_idx();
        assert_eq!(start_idx, OplogIndex::from_u64(2));
        drop(handle);

        // A later positional read must drain the abandoned call's End on the way to NoOp(4).
        let consumed = rs
            .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::NoOp { .. }))
            .await
            .unwrap();
        assert_eq!(
            consumed.map(|(idx, _)| idx),
            Some(OplogIndex::from_u64(4)),
            "the positional reader must see NoOp(4), not the abandoned call's End"
        );

        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal.concurrent_resolver.is_pending(start_idx),
            "draining the terminal of a dropped awaiter must drop its registration"
        );
    }

    /// A scan-ahead (identity-keyed) claim whose awaiter was dropped without awaiting must leave
    /// no `claimed_starts` residue once the cursor passes the claimed `Start`, and no resolver
    /// residue once it passes the terminal — dead registrations from cancelled accessor futures
    /// must not accumulate or steal entries from later positional readers.
    #[test]
    async fn dropped_scan_ahead_claim_leaves_no_residue_once_cursor_passes() {
        fn owned_start_now(parent: u64) -> OplogEntry {
            OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: Some(OplogIndex::from_u64(parent)),
                function_name: HostFunctionName::MonotonicClockNow,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::ReadLocal,
            }
        }

        // [NoOp, Start(A=2), Start(B=3, parent=2), End(B=3→4), End(A=2→5)]
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            owned_start_now(2),
            end_for(3, 1),
            end_for(2, 2),
        ])
        .await;

        // The head is Start(A), so the owned claim scan-ahead-claims Start(B) at 3.
        let handle_b = rs
            .claim_owned_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
                OplogIndex::from_u64(2),
            )
            .await
            .unwrap();
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
        drop(handle_b);

        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

        // Resolving A drives the cursor over the claimed Start(B) (auto-consumed) and End(B)
        // (drained to the dropped receiver) before reaching End(A).
        match rs.await_resolution(handle_a).await.unwrap() {
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(5));
            }
            other => panic!("expected Completed, got {other:?}"),
        }

        let internal = rs.cursor.state.lock().await;
        assert!(
            internal.claimed_starts.is_empty(),
            "passing the claimed Start must remove it from claimed_starts"
        );
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(3)),
            "draining the terminal of the dropped scan-ahead claim must drop its registration"
        );
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2)),
            "the resolved call must not stay registered"
        );
    }

    #[test]
    async fn interrupted_call_reports_incomplete_while_sibling_completes() {
        // [NoOp, Start(A=2), Start(B=3), End(B=3→4)] — a worker interrupted mid-call commits A's
        // `Start` but never its terminal, while a concurrent sibling B completed before the
        // interrupt. Replay must resolve B normally and report A as Incomplete (so A can be
        // re-executed live), not error out or misroute B's End to A.
        let rs = replay_state_over(vec![noop(), start_now(), start_now(), end_for(3, 42)]).await;
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

        match rs.await_resolution_outcome(handle_a).await.unwrap() {
            ResolutionOutcome::Incomplete => {}
            other => panic!("expected Incomplete for the interrupted call, got {other:?}"),
        }
        match rs.await_resolution_outcome(handle_b).await.unwrap() {
            ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
                assert_eq!(end_idx, OplogIndex::from_u64(4));
            }
            other => panic!("expected Completed for the sibling call, got {other:?}"),
        }
    }

    #[test]
    async fn replay_resolves_cancelled_without_partial() {
        // [NoOp, Start, Cancelled { partial: None }] — a call dropped mid-flight live and
        // recorded as `Cancelled` with no partial result replays to a `Cancelled` resolution
        // carrying no payload. (The caller decides how to surface it; the accessor replay path
        // rejects it as an unexpected entry when a response is required.)
        let rs = replay_state_over(vec![noop(), start_now(), cancelled_for(2)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Cancelled {
                cancelled_idx,
                partial,
            } => {
                assert_eq!(cancelled_idx, OplogIndex::from_u64(3));
                assert!(partial.is_none());
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    async fn replay_resolves_cancelled_with_partial_result() {
        // [NoOp, Start, Cancelled { partial: Some(..) }] — a call cancelled live with a partial
        // result replays to a `Cancelled` resolution that preserves the recorded partial
        // response payload (the CallHandle replay path downloads and converts it).
        let rs =
            replay_state_over(vec![noop(), start_now(), cancelled_with_partial_for(2, 42)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Cancelled {
                cancelled_idx,
                partial,
            } => {
                assert_eq!(cancelled_idx, OplogIndex::from_u64(3));
                match partial {
                    Some(OplogPayload::Inline(response)) => match *response {
                        HostResponse::MonotonicClockTimestamp(
                            HostResponseMonotonicClockTimestamp { nanos },
                        ) => assert_eq!(nanos, 42),
                        other => panic!("unexpected partial response: {other:?}"),
                    },
                    other => panic!("expected an inline partial payload, got {other:?}"),
                }
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    fn discarded_for(start_index: u64) -> OplogEntry {
        OplogEntry::CompletionDiscarded {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
        }
    }

    fn start_with_parent(parent_start_index: u64) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(parent_start_index)),
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
        }
    }

    fn invocation_finished() -> OplogEntry {
        OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentInitialization)),
            method_name: None,
            consumed_fuel: 0,
            component_revision: ComponentRevision::INITIAL,
        }
    }

    async fn read_invocation_finished(
        rs: &ReplayState,
    ) -> Result<Option<AgentInvocationResult>, WorkerExecutorError> {
        rs.get_oplog_entry_agent_invocation_finished().await
    }

    #[test]
    async fn invocation_boundary_tolerates_abandoned_closed_start() {
        // [NoOp, Start(2), End(2→3), AgentInvocationFinished(4)] — the durable call was issued
        // live but the replayed guest never re-issued it (an abandoned branch). At the invocation
        // boundary the never-claimed Start and its End are drained instead of failing the
        // positional read.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            end_for(2, 42),
            invocation_finished(),
        ])
        .await;
        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));
    }

    #[test]
    async fn invocation_boundary_tolerates_abandoned_cancelled_start() {
        // Same as above but the abandoned call was closed by a `Cancelled` terminal.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            cancelled_for(2),
            invocation_finished(),
        ])
        .await;
        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));
    }

    #[test]
    async fn invocation_boundary_tolerates_nested_abandoned_scope() {
        // [NoOp, Start(2), Start(3, parent=2), End(3→4), End(2→5), AgentInvocationFinished(6)] —
        // an abandoned scope root with an abandoned child, both properly closed, is tolerated as
        // a structurally valid closed tail.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_with_parent(2),
            end_for(3, 43),
            end_for(2, 42),
            invocation_finished(),
        ])
        .await;
        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));
    }

    #[test]
    async fn invocation_boundary_rejects_unclosed_abandoned_start() {
        // [NoOp, Start(2), AgentInvocationFinished(3)] — a dangling abandoned Start with no
        // terminal before the finished marker stays fatal: the closed-tail structural validation
        // fails.
        let rs = replay_state_over(vec![noop(), start_now(), invocation_finished()]).await;
        let err = read_invocation_finished(&rs)
            .await
            .expect_err("unclosed abandoned Start must be fatal");
        assert!(
            err.to_string().contains("unclosed abandoned Start"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn invocation_boundary_rejects_duplicate_terminal() {
        // [NoOp, Start(2), End(2→3), End(2→4), AgentInvocationFinished(5)] — a second terminal
        // closing the same abandoned Start is corruption, not tolerated noise.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            end_for(2, 42),
            end_for(2, 43),
            invocation_finished(),
        ])
        .await;
        let err = read_invocation_finished(&rs)
            .await
            .expect_err("duplicate terminal for an abandoned Start must be fatal");
        assert!(
            err.to_string().contains("already closed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn invocation_boundary_rejects_terminal_without_start() {
        // [NoOp, End(7→2), AgentInvocationFinished(3)] — a terminal whose Start was never drained
        // as abandoned (and is not awaited/orphaned) is not tolerated; the positional read still
        // fails with the unexpected entry.
        let rs = replay_state_over(vec![noop(), end_for(7, 42), invocation_finished()]).await;
        let err = read_invocation_finished(&rs)
            .await
            .expect_err("terminal without a matching abandoned Start must be fatal");
        assert!(
            err.to_string().contains("AgentInvocationFinished"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn invocation_boundary_rejects_unrelated_entry() {
        // [NoOp, NoOp(2), AgentInvocationFinished(3)] — non-hint entries other than abandoned
        // durable-call records stay fatal on the walk to the finished marker (`NoOp` is not a
        // hint entry).
        let rs = replay_state_over(vec![noop(), noop(), invocation_finished()]).await;
        let err = read_invocation_finished(&rs)
            .await
            .expect_err("unrelated positional entry must be fatal");
        assert!(
            err.to_string().contains("AgentInvocationFinished"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn invocation_boundary_does_not_drain_claimed_start() {
        // [NoOp, Start(2), End(2→3), AgentInvocationFinished(4)] with the Start claimed by a
        // concurrent replay call: the claim consumes the Start, the boundary walk drains the End
        // to the claim's resolver (awaited terminal), and the finished marker is read cleanly.
        // The claimed call still resolves as Completed — it is never miscounted as abandoned.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            end_for(2, 42),
            invocation_finished(),
        ])
        .await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn invocation_boundary_tolerates_abandoned_child_of_claimed_start() {
        // [NoOp, Start(2), Start(3, parent=2), End(3→4), CompletionDiscarded(3),
        // End(2→6), AgentInvocationFinished(7)] with Start(2) claimed — the exact shape a
        // discarded response-body chunk leaves behind: the parent consume-body scope is claimed
        // by the replayed guest, but the guest dropped the body reader before the persisted
        // child chunk was delivered (the child's marker records the discard) and never demands
        // it again on replay. The boundary walk drains the abandoned child records, skips the
        // hint marker, routes the parent's awaited End to its claim, and reads the finished
        // marker cleanly.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            start_with_parent(2),
            end_for(3, 43),
            discarded_for(3),
            end_for(2, 42),
            invocation_finished(),
        ])
        .await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(6));
            }
            other => panic!("expected Completed for the claimed parent, got {other:?}"),
        }
    }

    #[test]
    async fn invocation_boundary_tolerates_abandoned_start_with_unknown_parent() {
        // [NoOp, Start(2, parent=99), End(2→3), AgentInvocationFinished(4)] — an abandoned
        // Start whose parent lies outside the walked records (a claimed scope, or a region
        // deleted by a jump/revert) is treated as a root of the abandoned tail: the parent
        // linkage is informational, only the closed-tail structure is validated.
        let rs = replay_state_over(vec![
            noop(),
            start_with_parent(99),
            end_for(2, 42),
            invocation_finished(),
        ])
        .await;
        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));
    }

    #[test]
    async fn invocation_boundary_rejects_cancelled_after_end() {
        // [NoOp, Start(2), End(2→3), Cancelled(2→4), AgentInvocationFinished(5)] — a mixed
        // duplicate terminal (a `Cancelled` closing an abandoned Start already closed by an
        // `End`) is corruption, not tolerated noise.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            end_for(2, 42),
            cancelled_for(2),
            invocation_finished(),
        ])
        .await;
        let err = read_invocation_finished(&rs)
            .await
            .expect_err("a Cancelled closing an already-Ended abandoned Start must be fatal");
        assert!(
            err.to_string().contains("already closed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn invocation_boundary_rejects_terminal_of_resolved_claimed_start() {
        // [NoOp, Start(2), End(2→3), End(2→4), AgentInvocationFinished(5)] with Start(2)
        // claimed and resolved before the boundary read: the first End resolves the claim, so
        // the second End targets a start that is neither awaited nor drained as abandoned — it
        // stays fatal on the walk to the finished marker instead of being normalized into the
        // abandoned tail.
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            end_for(2, 42),
            end_for(2, 43),
            invocation_finished(),
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
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
            }
            other => panic!("expected Completed, got {other:?}"),
        }

        let err = read_invocation_finished(&rs)
            .await
            .expect_err("a duplicate terminal of a resolved claimed Start must be fatal");
        assert!(
            err.to_string().contains("AgentInvocationFinished"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn invocation_boundary_rejects_unclaimed_fork_pair() {
        // [NoOp, Start(2, GolemApiFork), End(2→3, Forked), AgentInvocationFinished(4)] — an
        // unclaimed legacy fork pair is a dedicated-positional-consumer record whose committed
        // consume is not inert: committing it would record a pending fork and decode the End
        // into a `ForkReplayed` event the replayed guest never requested. It must stay fatal
        // at the invocation boundary, and neither its commit-side state nor its replay event
        // may be applied.
        let rs = replay_state_over(vec![
            noop(),
            fork_start(),
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(2),
                response: Some(OplogPayload::Inline(Box::new(HostResponse::GolemApiFork(
                    HostResponseGolemApiFork {
                        forked_phantom_id: Uuid::new_v4(),
                        result: Ok(ForkResult::Forked),
                    },
                )))),
                forced_commit: false,
            },
            invocation_finished(),
        ])
        .await;

        let err = read_invocation_finished(&rs)
            .await
            .expect_err("an unclaimed GolemApiFork pair must stay fatal");
        assert!(
            err.to_string().contains("GolemApiFork"),
            "unexpected error: {err}"
        );

        assert!(
            rs.take_new_replay_events().is_empty(),
            "no replay event may be emitted for the rejected fork pair"
        );
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal.pending_fork_starts.is_empty(),
            "the rejected fork Start's commit-side state must not be applied"
        );
    }

    #[test]
    async fn invocation_boundary_tolerates_abandoned_consume_body_scope_shape() {
        // The actual shape a fully abandoned P3 consume-body leaves behind:
        //
        //   [NoOp,
        //    Start(2, P3HttpClientConsumeBody, WriteRemoteBatched(None)),          — parent scope
        //    Start(3, P3HttpClientConsumeBodyChunk, WriteRemoteBatched(Some(2)),
        //          parent_start_index=2),                                          — child chunk
        //    End(3→4, Data),                                                       — persisted chunk
        //    CompletionDiscarded(3),                                               — never delivered
        //    End(2→6, Trailers(None)),                                             — scope closed
        //    AgentInvocationFinished(7)]
        //
        // The replayed guest never re-issued the consume-body call, so nothing claims the
        // parent: the boundary walk drains the whole abandoned subtree (parent scope,
        // discarded child, both terminals), skips the discard hint, and reads the finished
        // marker cleanly.
        let rs = replay_state_over(vec![
            noop(),
            OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::P3HttpClientConsumeBody,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
            },
            OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: Some(OplogIndex::from_u64(2)),
                function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(
                    OplogIndex::from_u64(2),
                )),
            },
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(3),
                response: Some(OplogPayload::Inline(Box::new(
                    HostResponse::P3HttpClientConsumeBodyChunk(
                        HostResponseP3HttpClientConsumeBodyChunk {
                            chunk: SerializableP3HttpBodyChunk::Data(vec![1, 2, 3]),
                        },
                    ),
                ))),
                forced_commit: false,
            },
            discarded_for(3),
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(2),
                response: Some(OplogPayload::Inline(Box::new(
                    HostResponse::P3HttpClientConsumeBodyResult(
                        HostResponseP3HttpClientConsumeBodyResult {
                            result: SerializableP3HttpConsumeBodyResult::Trailers(None),
                        },
                    ),
                ))),
                forced_commit: false,
            },
            invocation_finished(),
        ])
        .await;

        let result = read_invocation_finished(&rs).await.unwrap();
        assert!(matches!(
            result,
            Some(AgentInvocationResult::AgentInitialization)
        ));
        // Reaching the end of replay emits `ReplayFinished`; the drained subtree itself must not
        // emit any side-effecting event (`ForkReplayed` / `UpdateReplayed`).
        let events = rs.take_new_replay_events();
        assert!(
            events
                .iter()
                .all(|event| matches!(event, ReplayEvent::ReplayFinished)),
            "draining the abandoned consume-body subtree must not emit side-effecting replay \
             events, got {events:?}"
        );
    }

    #[test]
    async fn replay_resolves_completed_but_discarded() {
        // [NoOp, Start, End, CompletionDiscarded] — the End was persisted live but its response
        // was never delivered to the guest (the marker records the discard), so replay must
        // resolve the call as CompletedButDiscarded, carrying the recorded response so deferred
        // replay can perform the recorded post-`End` continuation before parking.
        let rs =
            replay_state_over(vec![noop(), start_now(), end_for(2, 42), discarded_for(2)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                response,
            } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
                assert_eq!(marker_idx, OplogIndex::from_u64(4));
                assert!(response.is_some());
            }
            other => panic!("expected CompletedButDiscarded, got {other:?}"),
        }
    }

    #[test]
    async fn marker_in_deleted_region_delivers_end_normally() {
        // A CompletionDiscarded marker inside a deleted region belongs to an abandoned timeline:
        // the still-visible End must be delivered normally.
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [noop(), start_now(), end_for(2, 42), discarded_for(2)] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped =
            golem_common::model::regions::DeletedRegionsBuilder::from_regions([OplogRegion {
                start: OplogIndex::from_u64(4),
                end: OplogIndex::from_u64(4),
            }])
            .build();
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

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
    async fn duplicate_completion_discarded_markers_fail_construction() {
        // Two markers referencing the same Start is oplog corruption; the upfront scan rejects it.
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [
            noop(),
            start_now(),
            end_for(2, 42),
            discarded_for(2),
            discarded_for(2),
        ] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let err = ReplayState::new(test_agent_id(), oplog, DeletedRegions::default())
            .await
            .expect_err("duplicate markers must fail replay state construction");
        assert!(
            err.to_string().contains("CompletionDiscarded"),
            "unexpected error: {err}"
        );
    }

    #[test]
    async fn marker_recorded_at_runtime_is_visible_to_replay() {
        // record_discarded_completion feeds the same map as the upfront scan: a marker appended
        // live by this instance must park a later re-replay of its End exactly like a scanned
        // marker (e.g. after a drop-override restart). The marker is appended to the oplog and
        // the replay target grown over it, mirroring the live flow; growing over the
        // already-recorded marker must be idempotent, not a duplicate-marker error.
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [noop(), start_now(), end_for(2, 42)] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let rs = ReplayState::new(test_agent_id(), oplog.clone(), DeletedRegions::default())
            .await
            .expect("failed to build replay state");
        let marker_idx = oplog.add(discarded_for(2)).await;
        rs.record_discarded_completion(OplogIndex::from_u64(2), marker_idx);
        rs.set_replay_target(marker_idx)
            .await
            .expect("growing the target over the recorded marker must be idempotent");

        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();

        match rs.await_resolution(handle).await.unwrap() {
            Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                response,
            } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
                assert_eq!(marker_idx, OplogIndex::from_u64(4));
                assert!(response.is_some());
            }
            other => panic!("expected CompletedButDiscarded, got {other:?}"),
        }
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
        let events = rs.take_new_replay_events();
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
        let events = rs.take_new_replay_events();
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
        assert!(rs.take_new_replay_events().is_empty());

        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        rs.await_resolution(handle).await.unwrap();

        assert!(rs.is_live());
        let events = rs.take_new_replay_events();
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

    fn cancelled_with_partial_for(start_index: u64, nanos: u64) -> OplogEntry {
        OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
            partial: Some(OplogPayload::Inline(Box::new(
                HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp {
                    nanos,
                }),
            ))),
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

    /// An `End` whose `Start` lies inside a deleted region (a jump/revert cut between the pair) is
    /// an *orphan terminal*: the cursor must consume it transparently instead of surfacing it to a
    /// positional reader, and a later call must still claim and resolve at its true indices.
    #[test]
    async fn orphan_end_with_deleted_start_is_skipped() {
        // [NoOp(1), Start(2), End(2→3), Start(4), End(4→5)] with deleted region [2, 2]: the End at
        // 3 is orphaned; the kept call (4, 5) must claim and resolve normally across it.
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [
            noop(),
            start_now(),
            end_for(2, 1),
            start_now(),
            end_for(4, 2),
        ] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(2),
        }]);
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");

        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(
            handle.start_idx(),
            OplogIndex::from_u64(4),
            "the claim must skip the orphan End and land on the kept Start"
        );
        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(5))
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert!(rs.is_live());
    }

    /// A `Cancelled` orphan (its `Start` deleted) is consumed transparently by a positional drain,
    /// bringing replay to live instead of erroring as an unexpected entry.
    #[test]
    async fn orphan_cancelled_with_deleted_start_is_skipped() {
        // [NoOp(1), Start(2), Cancelled(2→3)] with deleted region [2, 2].
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [noop(), start_now(), cancelled_for(2)] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(2),
        }]);
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");

        let result = rs.try_get_oplog_entry(|_| false).await.unwrap();
        assert!(result.is_none());
        assert!(
            rs.is_live(),
            "the orphan Cancelled must be drained, reaching live"
        );
    }

    /// A positional reader (`get_oplog_entry`) skips an orphan terminal and returns the next real
    /// entry instead of surfacing the orphan as an unexpected entry.
    #[test]
    async fn positional_reader_skips_orphan_terminal() {
        // [NoOp(1), Start(2), End(2→3), NoOp(4)] with deleted region [2, 2]: the positional read
        // must consume the orphan End at 3 and return the NoOp at 4.
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [noop(), start_now(), end_for(2, 1), noop()] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(2),
        }]);
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");

        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(4));
        assert!(matches!(entry, OplogEntry::NoOp { .. }));
        assert!(rs.is_live());
    }

    /// The inverse partial deletion — the `Start` kept, its terminal inside a deleted region —
    /// reports `Incomplete` (the caller may re-execute the call), never an error or a hang.
    #[test]
    async fn deleted_terminal_reports_incomplete() {
        // [NoOp(1), Start(2), End(2→3)] with deleted region [3, 3].
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in [noop(), start_now(), end_for(2, 1)] {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(3),
        }]);
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");

        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));
        match rs.await_resolution_outcome(handle).await.unwrap() {
            ResolutionOutcome::Incomplete => {}
            other => panic!("expected Incomplete, got {other:?}"),
        }
        assert!(rs.is_live());
    }

    /// How a generated call pair interacts with the deleted regions in
    /// [`replay_skips_deleted_regions_fuzz`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Deletion {
        /// Both the `Start` and its terminal are kept.
        Kept,
        /// The whole pair lies inside a deleted region (a clean jump/revert cut).
        Pair,
        /// Only the `Start` is deleted: its terminal survives as an *orphan terminal* the cursor
        /// must skip transparently.
        StartOnly,
        /// Only the terminal is deleted: the kept `Start` must report `Incomplete`.
        TerminalOnly,
    }

    /// Seam 1, deleted/jump regions: a randomized generator that records a run of contiguous
    /// call pairs — each terminating in an `End` or a `Cancelled` — and then marks, per pair,
    /// either the whole pair, only its `Start`, or only its terminal as belonging to deleted
    /// oplog regions (as a `Jump`/revert cutting at an arbitrary point would leave behind).
    /// Deleted entries must be skipped by the replay cursor entirely — never claimed, never
    /// read; orphan terminals (Start deleted, terminal kept) must be consumed transparently;
    /// kept `Start`s whose terminal was deleted must report `Incomplete`; and fully kept calls
    /// must still claim at their true indices and resolve to their recorded terminal. Deleting
    /// a leading region exercises the construction-time jump; deleting a trailing region
    /// exercises the jump-to-target transition into live. Seeds are fixed, so any failure
    /// reproduces.
    #[test]
    async fn replay_skips_deleted_regions_fuzz() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const CASES: u64 = 500;

        for seed in 0..CASES {
            let mut rng = StdRng::seed_from_u64(seed);
            let num_calls = rng.random_range(1..=6usize);

            // Contiguous call pairs after the placeholder: [Start, terminal, Start, terminal, ...],
            // where each terminal is independently an `End` or a `Cancelled`.
            let mut entries = vec![noop()];
            let mut start_idx = Vec::with_capacity(num_calls);
            let mut terminal_idx = Vec::with_capacity(num_calls);
            let mut is_cancelled = Vec::with_capacity(num_calls);
            let mut deletion = Vec::with_capacity(num_calls);
            let mut nanos = 0u64;
            for _ in 0..num_calls {
                entries.push(start_now());
                let si = entries.len() as u64;
                let cancelled = rng.random_bool(0.3);
                if cancelled {
                    entries.push(cancelled_for(si));
                } else {
                    nanos += 1;
                    entries.push(end_for(si, nanos));
                }
                let ti = entries.len() as u64;
                start_idx.push(si);
                terminal_idx.push(ti);
                is_cancelled.push(cancelled);
                deletion.push(match rng.random_range(0..10u32) {
                    0..=3 => Deletion::Kept,
                    4..=5 => Deletion::Pair,
                    6..=7 => Deletion::StartOnly,
                    _ => Deletion::TerminalOnly,
                });
            }

            // Coalesce the deleted entry indices into contiguous regions.
            let mut deleted_indices: std::collections::BTreeSet<u64> =
                std::collections::BTreeSet::new();
            for i in 0..num_calls {
                match deletion[i] {
                    Deletion::Kept => {}
                    Deletion::Pair => {
                        deleted_indices.insert(start_idx[i]);
                        deleted_indices.insert(terminal_idx[i]);
                    }
                    Deletion::StartOnly => {
                        deleted_indices.insert(start_idx[i]);
                    }
                    Deletion::TerminalOnly => {
                        deleted_indices.insert(terminal_idx[i]);
                    }
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

            // Claim only the calls whose `Start` is kept, in order; the cursor must jump over
            // every deleted region and transparently consume every orphan terminal.
            let mut handles = Vec::new();
            for i in 0..num_calls {
                if matches!(deletion[i], Deletion::Pair | Deletion::StartOnly) {
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
                match deletion[i] {
                    Deletion::Kept => match rs.await_resolution(handle).await.unwrap_or_else(|e| {
                        panic!("seed {seed}: await of kept call {i} failed: {e}")
                    }) {
                        Resolution::Completed { end_idx: ti, .. } if !is_cancelled[i] => {
                            assert_eq!(
                                ti,
                                OplogIndex::from_u64(terminal_idx[i]),
                                "seed {seed}: kept call {i} resolved to the wrong End"
                            )
                        }
                        Resolution::Cancelled {
                            cancelled_idx: ti, ..
                        } if is_cancelled[i] => {
                            assert_eq!(
                                ti,
                                OplogIndex::from_u64(terminal_idx[i]),
                                "seed {seed}: kept call {i} resolved to the wrong Cancelled"
                            )
                        }
                        other => panic!(
                            "seed {seed}: kept call {i} (cancelled: {}) resolved to the wrong terminal kind: {other:?}",
                            is_cancelled[i]
                        ),
                    },
                    Deletion::TerminalOnly => {
                        match rs
                            .await_resolution_outcome(handle)
                            .await
                            .unwrap_or_else(|e| {
                                panic!(
                                    "seed {seed}: await of terminal-deleted call {i} failed: {e}"
                                )
                            }) {
                            ResolutionOutcome::Incomplete => {}
                            other => panic!(
                                "seed {seed}: terminal-deleted call {i} expected Incomplete, got {other:?}"
                            ),
                        }
                    }
                    Deletion::Pair | Deletion::StartOnly => unreachable!(),
                }
            }

            // Any trailing orphan terminals (a Start-deleted call at the end of the layout) are
            // only consumed when something drives the cursor: drain and expect no real entry.
            let trailing = rs
                .try_get_oplog_entry(|_| false)
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: final drain failed: {e}"));
            assert!(
                trailing.is_none(),
                "seed {seed}: final drain unexpectedly returned an entry: {trailing:?}"
            );

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

    fn suspend() -> OplogEntry {
        OplogEntry::Suspend {
            timestamp: Timestamp::now_utc(),
        }
    }

    /// A batched-write scope `Start` exactly as `begin_function` records it: request-less,
    /// top-level, `WriteRemoteBatched(None)`.
    fn batched_scope_start() -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
            request: None,
            durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
        }
    }

    /// A batched-write scope `End` exactly as `end_function` records it: response-less,
    /// `forced_commit: true`.
    fn batched_scope_end(start_index: u64) -> OplogEntry {
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
            response: None,
            forced_commit: true,
        }
    }

    /// A host-call `Start` nested in the batched-write scope at `parent`, exactly as the
    /// sequential adapter records followup batched invocations:
    /// `parent_start_index: Some(scope)`, `WriteRemoteBatched(Some(scope))`.
    fn batched_child_start(parent: u64) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(parent)),
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(
                OplogIndex::from_u64(parent),
            )),
        }
    }

    /// A representative oplog written by the sequential adapter, where every host call is an
    /// *adjacent* `Start`/`End` pair appended atomically via `Oplog::add_pair`, must replay cleanly
    /// through the concurrent resolver.
    ///
    /// The fixture is synthesized in the exact shapes the sequential writers produced
    /// (`OplogOps::add_completed_host_call`, `begin_function` / `end_function`), covering:
    /// - plain adjacent host-call pairs,
    /// - a hint entry (`Suspend`) between calls,
    /// - an adjacent pair inside an atomic region (positional `Begin`/`EndAtomicRegion` markers),
    /// - a batched-write scope (request-less scope `Start`, a child call pair recorded with
    ///   `parent_start_index: Some(scope)` / `WriteRemoteBatched(Some(scope))`, and the
    ///   response-less, forced-commit scope `End`),
    /// - no `Cancelled` entries and no overlapping calls anywhere.
    ///
    /// Replay drives the same claim/await sequence the sequential durability layer performs:
    /// each call is claimed then awaited immediately, scope `End`s resolve through the
    /// resolver, and positional markers are consumed by `get_oplog_entry`.
    #[test]
    async fn pre_migration_adjacent_pair_oplog_replays_through_concurrent_resolver() {
        // [ 1: NoOp,
        //   2: Start(A), 3: End(A=2, 41),
        //   4: Suspend (hint),
        //   5: Start(B), 6: End(B=5, 42),
        //   7: BeginAtomicRegion, 8: Start(C), 9: End(C=8, 43), 10: EndAtomicRegion(7),
        //   11: Start(scope), 12: Start(D, parent=11), 13: End(D=12, 44), 14: End(scope=11) ]
        let rs = replay_state_over(vec![
            noop(),
            start_now(),
            end_for(2, 41),
            suspend(),
            start_now(),
            end_for(5, 42),
            begin_atomic_region(),
            start_now(),
            end_for(8, 43),
            end_atomic_region(7),
            batched_scope_start(),
            batched_child_start(11),
            end_for(12, 44),
            batched_scope_end(11),
        ])
        .await;

        // Call A: claim + immediate await, the sequential replay pattern. The recorded
        // response payload must round-trip through the resolution.
        let handle_a = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
        match rs.await_resolution(handle_a).await.unwrap() {
            Resolution::Completed {
                end_idx, response, ..
            } => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
                match response {
                    Some(OplogPayload::Inline(boxed)) => assert_eq!(
                        *boxed,
                        HostResponse::MonotonicClockTimestamp(
                            HostResponseMonotonicClockTimestamp { nanos: 41 }
                        )
                    ),
                    other => panic!("expected inline response payload, got {other:?}"),
                }
            }
            other => panic!("expected Completed for A, got {other:?}"),
        }

        // Call B: the Suspend hint between the pairs is skipped transparently by the claim.
        let handle_b = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(5));
        match rs.await_resolution(handle_b).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
            other => panic!("expected Completed for B, got {other:?}"),
        }

        // Atomic region markers are positional; call C replays inside the region.
        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(7));
        assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

        let handle_c = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle_c.start_idx(), OplogIndex::from_u64(8));
        match rs.await_resolution(handle_c).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(9)),
            other => panic!("expected Completed for C, got {other:?}"),
        }

        let (idx, entry) = rs.get_oplog_entry().await.unwrap();
        assert_eq!(idx, OplogIndex::from_u64(10));
        assert!(
            matches!(entry, OplogEntry::EndAtomicRegion { begin_index, .. } if begin_index == OplogIndex::from_u64(7))
        );

        // Batched-write scope: scope Start claims through the resolver, the child call
        // is claimed by identity (parent_start_index), and the scope End resolves response-less.
        let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
        let (scope_idx, scope_handle) = rs
            .claim_scope_start(&scope_name, &DurableFunctionType::WriteRemoteBatched(None))
            .await
            .unwrap();
        assert_eq!(scope_idx, OplogIndex::from_u64(11));

        let handle_d = rs
            .claim_owned_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::WriteRemoteBatched(Some(OplogIndex::from_u64(11))),
                OplogIndex::from_u64(11),
            )
            .await
            .unwrap();
        assert_eq!(handle_d.start_idx(), OplogIndex::from_u64(12));
        match rs.await_resolution(handle_d).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(13)),
            other => panic!("expected Completed for D, got {other:?}"),
        }

        match rs.await_resolution_outcome(scope_handle).await.unwrap() {
            ResolutionOutcome::Resolved(Resolution::Completed {
                end_idx, response, ..
            }) => {
                assert_eq!(end_idx, OplogIndex::from_u64(14));
                assert!(response.is_none(), "scope End must be response-less");
            }
            other => panic!("expected Completed for the scope, got {other:?}"),
        }

        // The whole sequential oplog is consumed: replay is over and nothing is left pending.
        assert!(rs.is_live(), "replay must reach live at the end");
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2))
                && !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(5))
                && !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(8))
                && !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(11))
                && !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(12)),
            "no resolver awaiter may remain pending after a full replay"
        );
    }

    #[test]
    async fn discriminated_scope_claim_never_matches_plain_scope_start() {
        // Scope claims match the expected name exactly: a discriminated claim
        // (`<scope:batched-write:DISC>`) must NOT claim a plain `<scope:batched-write>` Start —
        // there is no plain-name fallback, so a discriminated call can never steal a concurrent
        // plain sibling's recorded scope. The failed claim must not consume or claim anything:
        // the plain scope must still be claimable by its own exact name afterwards.
        let rs = replay_state_over(vec![noop(), batched_scope_start(), batched_scope_end(2)]).await;

        let discriminated =
            HostFunctionName::Custom("<scope:batched-write:consume-body:2>".to_string());
        let err = rs
            .claim_scope_start(
                &discriminated,
                &DurableFunctionType::WriteRemoteBatched(None),
            )
            .await
            .expect_err("discriminated claim must not match the plain scope Start");
        let message = format!("{err}");
        assert!(
            message.contains("no matching Start"),
            "unexpected error: {message}"
        );

        let plain = HostFunctionName::Custom("<scope:batched-write>".to_string());
        let (scope_idx, scope_handle) = rs
            .claim_scope_start(&plain, &DurableFunctionType::WriteRemoteBatched(None))
            .await
            .unwrap();
        assert_eq!(scope_idx, OplogIndex::from_u64(2));
        match rs.await_resolution_outcome(scope_handle).await.unwrap() {
            ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
            }
            other => panic!("expected Completed for the plain scope, got {other:?}"),
        }
        assert!(rs.is_live(), "replay must reach live at the end");
    }

    #[test]
    async fn plain_scope_claim_never_matches_discriminated_scope_start() {
        // The inverse direction: a plain claim must not match a discriminated scope Start.
        let discriminated_start = OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("<scope:batched-write:req:abc123>".to_string()),
            request: None,
            durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
        };
        let rs = replay_state_over(vec![noop(), discriminated_start, batched_scope_end(2)]).await;

        let plain = HostFunctionName::Custom("<scope:batched-write>".to_string());
        let err = rs
            .claim_scope_start(&plain, &DurableFunctionType::WriteRemoteBatched(None))
            .await
            .expect_err("plain claim must not match a discriminated scope Start");
        let message = format!("{err}");
        assert!(
            message.contains("no matching Start"),
            "unexpected error: {message}"
        );

        let discriminated =
            HostFunctionName::Custom("<scope:batched-write:req:abc123>".to_string());
        let (scope_idx, scope_handle) = rs
            .claim_scope_start(
                &discriminated,
                &DurableFunctionType::WriteRemoteBatched(None),
            )
            .await
            .unwrap();
        assert_eq!(scope_idx, OplogIndex::from_u64(2));
        match rs.await_resolution_outcome(scope_handle).await.unwrap() {
            ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
                assert_eq!(end_idx, OplogIndex::from_u64(3));
            }
            other => panic!("expected Completed for the discriminated scope, got {other:?}"),
        }
        assert!(rs.is_live(), "replay must reach live at the end");
    }
}
