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

mod abandoned;
mod claims;
mod cursor;
mod resolution;

use abandoned::AbandonedStarts;

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
mod tests;
