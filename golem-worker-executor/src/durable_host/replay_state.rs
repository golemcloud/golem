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
use golem_common::model::card::CardId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostResponse, HostResponseGolemApiFork, LogLevel, OplogEntry, OplogIndex, OplogPayload, PersistenceLevel
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentInvocationPayload, AgentInvocationResult, ForkResult, IdempotencyKey, OwnedAgentId,
    Timestamp,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use metrohash::MetroHash128;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::hash::Hasher;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::debug;
use uuid::Uuid;
use std::rc::Rc;
use std::cmp::{Ordering, Reverse};
use super::concurrent::CallReplayOutcome;

const CHUNK_SIZE: u64 = 1024;

#[derive(Debug, Clone)]
pub enum ReplayEvent {
    ReplayFinished,
    UpdateReplayed { new_revision: ComponentRevision },
    ForkReplayed { new_phantom_id: Uuid },
    CardRevoked { card_id: CardId },
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

#[derive(Debug, PartialEq, Eq)]
enum InternalReplayEvent {
    ReplayFinished,
    UpdateReplayed { new_revision: ComponentRevision },
    CardRevoked { card_id: CardId },
    // Must sort before DurableCallEnded
    ForkStarted,
    DurableCallEnded {
        start_index: OplogIndex,
        response: Option<OplogPayload<HostResponse>>,
        forced_commit: bool
    },
    DurableCallCancelled {
        start_index: OplogIndex,
        partial_response: Option<OplogPayload<HostResponse>>,
    },
    SeenLog {
        level: LogLevel,
        context: String,
        message: String
    }
}

impl InternalReplayEvent {
    fn tag(&self) -> u8 {
        match self {
            Self::ReplayFinished => 0,
            Self::UpdateReplayed { .. } => 1,
            Self::CardRevoked { .. } => 2,
            Self::ForkStarted => 3,
            Self::DurableCallEnded { .. } => 4,
            Self::DurableCallCancelled { .. } => 5,
            Self::SeenLog { .. } => 6
        }
    }
}

// We always order by oplog index as well, and we never have
// two events of the same type from one entry -> just ordering by the event types is enough.
impl Ord for InternalReplayEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tag().cmp(&other.tag())
    }
}

impl PartialOrd for InternalReplayEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
struct PendingState {
    current_oplog_index: OplogIndex,
    last_read_non_hint_oplog_index: OplogIndex,
    current_oplog_chunk: Arc<Vec<OplogEntry>>,
    current_oplog_chunk_region: OplogRegion,
    // invariant: either start or end must be at or after last_read_oplog_index.
    next_skipped_region: Option<OplogRegion>,
    in_persist_nothing_region: bool,
    // We are doing appends in ascending order 90%+ of the time here. A binary heap is not a great choice
    // for this, but better than the alternatives.
    replay_events: BinaryHeap<(Reverse<OplogIndex>, InternalReplayEvent)>,
    jumps: Vec<OplogRegion>,
}

#[derive(Debug)]
struct CommittedState {
    current_oplog_index: OplogIndex,
    current_oplog_chunk_region: OplogRegion,
    current_oplog_chunk: Arc<Vec<OplogEntry>>,
    last_read_non_hint_oplog_index: OplogIndex,
    // invariant: either start or end must be at or after last_read_oplog_index.
    next_skipped_region: Option<OplogRegion>,
    in_persist_nothing_region: bool,
    replay_events: Vec<ReplayEvent>,
    seen_logs: HashSet<(u64, u64)>,
    fork_starts: HashSet<OplogIndex>,
}

#[derive(Debug)]
pub struct ReplayState {
    oplog: Arc<dyn Oplog>,

    // === initialization-only ===
    owned_agent_id: OwnedAgentId,
    replay_target: OplogIndex,
    skipped_regions: DeletedRegions,

    // === runtime mutable ===
    pending: PendingState,
    committed: CommittedState,
    // Must only be fed committed entries
    concurrent_resolver: ConcurrentReplayResolver,
}

impl ReplayState {
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        skipped_regions: DeletedRegions,
    ) -> Self {
        let replay_target = oplog.current_oplog_index().await;
        Self::new_with_target(owned_agent_id, oplog, skipped_regions, replay_target)
    }

    fn new_with_target(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        skipped_regions: DeletedRegions,
        replay_target: OplogIndex
    ) -> Self {
        let (start_index, initial_skipped_region) = Self::first_non_skipped_entry(&skipped_regions);
        let initial_oplog_chunk = Arc::new(Vec::new());
        let initial_oplog_chunk_region = OplogRegion { start: OplogIndex::NONE, end: OplogIndex::NONE };

        Self {
            oplog,
            owned_agent_id,
            replay_target,
            skipped_regions,
            pending: PendingState {
                current_oplog_index: start_index,
                last_read_non_hint_oplog_index: start_index,
                current_oplog_chunk: initial_oplog_chunk.clone(),
                current_oplog_chunk_region: initial_oplog_chunk_region.clone(),
                next_skipped_region: initial_skipped_region.clone(),
                in_persist_nothing_region: false,
                replay_events: BinaryHeap::new(),
                jumps: Vec::new(),
            },
            committed: CommittedState {
                current_oplog_index: start_index,
                last_read_non_hint_oplog_index: start_index,
                current_oplog_chunk: initial_oplog_chunk,
                current_oplog_chunk_region: initial_oplog_chunk_region,
                next_skipped_region: initial_skipped_region,
                in_persist_nothing_region: false,
                replay_events: Vec::new(),
                seen_logs: HashSet::new(),
                fork_starts: HashSet::new(),
            },
            concurrent_resolver: ConcurrentReplayResolver::default()
        }
    }

    pub fn drop_override_and_restart(&mut self) {
        let owned_agent_id = self.owned_agent_id.clone();
        let oplog = self.oplog.clone();
        let mut skipped_regions = self.skipped_regions.clone();
        let replay_target = self.replay_target;

        skipped_regions.drop_override();

        *self = Self::new_with_target(owned_agent_id, oplog, skipped_regions, replay_target);
    }

    pub fn last_replayed_index(&self) -> OplogIndex {
        self.committed.current_oplog_index
    }

    pub fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.committed.last_read_non_hint_oplog_index
    }

    pub fn replay_target(&self) -> OplogIndex {
        self.replay_target
    }

    pub fn set_replay_target(&mut self, new_target: OplogIndex) {
        self.replay_target = new_target
    }

    pub fn is_in_skipped_region(&self, oplog_index: OplogIndex) -> bool {
        self.skipped_regions.is_in_deleted_region(oplog_index)
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.committed.current_oplog_index >= self.replay_target
    }

    fn pending_is_live(&self) -> bool {
        self.pending.current_oplog_index >= self.replay_target
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    fn pending_is_replay(&self) -> bool {
        !self.pending_is_live()
    }

    pub async fn take_new_replay_events(&mut self) -> Vec<ReplayEvent> {
        std::mem::take(&mut self.committed.replay_events)
    }

    /// Returns true if the given log entry has been seen since the last non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        !self.committed.seen_logs.is_empty() && {
            let hash = Self::hash_log_entry(level, context, message);
            self.committed.seen_logs.contains(&hash)
        }
    }

    /// Removes a seen log from the set. If the set becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&mut self, level: LogLevel, context: &str, message: &str) {
        if !self.committed.seen_logs.is_empty() {
            let hash = Self::hash_log_entry(level, context, message);
            self.committed.seen_logs.remove(&hash);
        }
    }

    fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
        let mut hasher = MetroHash128::new();
        hasher.write_u8(level as u8);
        hasher.write(context.as_bytes());
        hasher.write(message.as_bytes());
        hasher.finish128()
    }

    /// Reads the next oplog entry, and if it matches the given condition, returns the entry.
    /// If the condition is not met, returns None and the current replay state remains
    /// unchanged.
    pub async fn try_get_oplog_entry(
        &mut self,
        condition: impl FnOnce(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        if !self.is_replay() {
            return Ok(None);
        }

        self.advance_to_next_non_hint_entry().await;
        let (index, entry) = self.get_current_entry().await;

        if condition(&entry) {
            let entry = entry.clone();
            self.commit().await;
            Ok(Some((index, entry)))
        } else {
            self.revert_to_last_commit();
            Ok(None)
        }
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    ///
    /// Returns an error if the underlying read fails (e.g. missing oplog entry,
    /// corrupted GolemApiFork payload) so the worker can fail the agent with a
    /// non-retriable trap rather than panicking the executor.
    pub async fn get_oplog_entry(
        &mut self,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        // The closure always returns true, so the outer Option is always Some(...)
        // when the underlying read succeeds.
        Ok(self
            .try_get_oplog_entry(|_| true)
            .await?
            .expect("try_get_oplog_entry with always-true predicate must return Some"))
    }

    pub async fn lookup_oplog_entry(
        &mut self,
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
        &mut self,
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

    // If the entry was not found, the replay state is guaranteed to be in live mode
    // as we reached the end of the oplog.
    pub async fn lookup_oplog_entry_with_condition_and_state<State>(
        &mut self,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex, &State) -> bool,
        mut state: State,
        mut update_state: impl FnMut(&OplogEntry, OplogIndex, &mut State),
    ) -> OplogEntryLookupResult {
        let mut violation = false;
        while self.is_replay() {
            self.advance_to_next_non_hint_entry().await;
            let (idx, entry) = self.get_current_entry().await;

            update_state(entry, idx, &mut state);

            if end_check(entry, begin_idx, &state) {
                let entry = entry.clone();
                self.commit().await;
                return OplogEntryLookupResult::Found {
                    index: idx,
                    entry: Box::new(entry),
                    violates_for_all: violation,
                };
            }

            if !for_all_intermediate(entry, begin_idx, &state) {
                violation = true;
            }
        }
        self.commit().await;
        assert!(self.is_live());

        OplogEntryLookupResult::NotFound {
            violates_for_all: violation,
        }
    }

    pub async fn get_oplog_entry_agent_invocation_started(
        &mut self,
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
        &mut self,
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
    /// to their awaiter by `start_index` (the resolver) instead of by position. In short: `Start`
    /// order is a deterministic *output* of replay; completion order is the non-deterministic
    /// *input* we recorded and must replay.
    ///
    /// This relies on the `Start` being appended synchronously at the guest's initiation point.
    /// While durable host calls are serialized (each holds the store for its whole duration) that
    /// holds trivially. Once calls genuinely overlap, the positional claim stays valid for the same
    /// reason; what must change instead is the cursor driving in [`Self::await_resolution`] (see its
    /// docs) so that the shared cursor only advances past a `Start` once it has been claimed.
    ///
    /// `End` entries carry no function identity, so validation must happen here, at claim time.
    /// The request payload is not decoded: `function_name` already pins the request type (and the
    /// `Req` associated type has no `TryFrom<HostRequest>` to decode it generically); the response
    /// is fully type-checked on the `End` side during replay.
    pub async fn claim_concurrent_start(
        &mut self,
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
            self
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
    pub async fn claim_any_concurrent_start(
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
        match entry {
            OplogEntry::Start {
                timestamp,
                function_name,
                request,
                durable_function_type,
                ..
            } => {
                if request.is_none() {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "Start { request: Some(..) }",
                        "Start { request: None }".to_string(),
                    ));
                }
                let receiver = {
                    self.concurrent_resolver.register(start_idx)
                };
                Ok(ClaimedConcurrentStart {
                    handle: ReplayCallHandle::new(start_idx, receiver),
                    function_name,
                    durable_function_type,
                    timestamp,
                })
            }
            _ => unreachable!("try_get_oplog_entry condition guarantees a Start entry"),
        }
    }

    /// Drives the replay cursor forward, feeding the committed-consume hook, until the call
    /// identified by `handle` resolves.
    ///
    /// The replay cursor is shared with legacy positional readers, and this driver only commits
    /// `End`/`Cancelled` entries. Any other non-hint entry between the claimed `Start` and its
    /// resolution (an unclaimed `Start`, a scope marker, a persistence-level change, ...) means the
    /// cursor would be driven past something a legacy positional reader still expects, so it
    /// returns an error instead of corrupting the cursor. With serialized host calls a call's oplog
    /// is its `Start` followed by its own `End`/`Cancelled` (hint entries aside), so this never
    /// triggers.
    pub async fn await_resolution(
        &mut self,
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

    /// Like [`Self::await_resolution`], but reports a lone committed `Start` (replay reached the end
    /// of the oplog without the matching `End`/`Cancelled`) as [`ResolutionOutcome::Incomplete`]
    /// rather than a hard error, so the caller can decide whether to re-execute the call. A genuine
    /// interleaving (a non-`End`/`Cancelled` entry encountered mid-await) is still a hard error.
    pub async fn await_resolution_outcome(
        &mut self,
        handle: ReplayCallHandle,
    ) -> Result<ResolutionOutcome, WorkerExecutorError> {
        let (start_idx, mut receiver) = handle.into_parts();
        loop {
            match receiver.try_recv() {
                Ok(resolution) => return Ok(ResolutionOutcome::Resolved(resolution)),
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    // The sender was dropped without resolving (anomalous). Drop any lingering
                    // registration so it cannot be matched by a later resolution.
                    self.concurrent_resolver
                        .unregister(start_idx);
                    return Err(WorkerExecutorError::runtime(format!(
                        "concurrent replay resolver channel closed for Start at {start_idx}"
                    )));
                }
            }

            if self.is_live() {
                // Reached the end of the oplog without ever seeing the matching End/Cancelled: a
                // committed lone `Start` (a forced commit flushed it before its `End`, or a crash
                // happened in between). Drop the now-stale registration and report Incomplete so the
                // caller can re-execute the side effect and complete the existing `Start`.
                self.concurrent_resolver
                    .unregister(start_idx);
                return Ok(ResolutionOutcome::Incomplete);
            }

            let consumed = self
                .try_get_oplog_entry(|entry| {
                    matches!(entry, OplogEntry::End { .. } | OplogEntry::Cancelled { .. })
                })
                .await?;
            if consumed.is_none() {
                // The next non-hint entry is not an End/Cancelled (e.g. an unclaimed `Start` or a
                // scope/persistence marker). Crossing it would corrupt the cursor shared with
                // legacy positional readers, so we refuse rather than advance past it. Drop the
                // stale registration first so it cannot be matched by a later resolution.
                self.concurrent_resolver
                    .unregister(start_idx);
                return Err(WorkerExecutorError::runtime(format!(
                    "concurrent replay interleaving is not supported: encountered a non-End/Cancelled entry while awaiting resolution of Start at {start_idx}"
                )));
            }
            // The consumed entry was an End/Cancelled; the committed-consume hook has resolved the
            // receiver, which the next loop iteration picks up.
        }
    }

    // Make current internal state visible to users and update checkpoint for reverts
    async fn commit(&mut self) {
        self.committed.current_oplog_index = self.pending.current_oplog_index;
        self.committed.current_oplog_chunk_region = self.pending.current_oplog_chunk_region.clone();
        self.committed.current_oplog_chunk = Arc::clone(&self.pending.current_oplog_chunk);
        self.committed.last_read_non_hint_oplog_index = self.pending.last_read_non_hint_oplog_index;
        self.committed.next_skipped_region = self.pending.next_skipped_region.clone();
        self.committed.in_persist_nothing_region = self.pending.in_persist_nothing_region;

        let mut replay_events_in_jumps = self.extract_replay_events_in_jumped_regions().await;
        self.pending.replay_events.append(&mut replay_events_in_jumps);

        {
            let fork_starts = &mut self.committed.fork_starts;
            let replay_events = &mut self.committed.replay_events;
            let seen_logs = &mut self.committed.seen_logs;

            for (Reverse(event_idx), event) in std::mem::take(&mut self.pending.replay_events) {
                match event {
                    InternalReplayEvent::ReplayFinished => {
                        replay_events.push(ReplayEvent::ReplayFinished);
                    }
                    InternalReplayEvent::UpdateReplayed { new_revision } => {
                        replay_events.push(ReplayEvent::UpdateReplayed { new_revision });
                    }
                    InternalReplayEvent::CardRevoked { card_id } => {
                        replay_events.push(ReplayEvent::CardRevoked { card_id });
                    }
                    InternalReplayEvent::ForkStarted => {
                        fork_starts.insert(event_idx);
                    }
                    InternalReplayEvent::DurableCallEnded {
                        start_index,
                        response,
                        forced_commit,
                    } => {
                        self.concurrent_resolver.resolve_if_pending(start_index, Resolution::Completed { end_idx: event_idx, response: response.clone(), forced_commit });
                        if fork_starts.remove(&start_index) {
                            let response = response.expect("fork end should have payload");

                            let response = self
                                .oplog
                                .download_payload(response)
                                .await
                                .expect(&format!("failed to download GolemApiFork oplog payload at index {start_index}"));

                            let result: HostResponseGolemApiFork =
                                if let HostResponse::GolemApiFork(result) = response {
                                    result
                                } else {
                                    panic!("Unexpected host response when fetching golem api fork result at {event_idx}")
                                };

                            if result.result == Ok(ForkResult::Forked) {
                                replay_events.push(ReplayEvent::ForkReplayed { new_phantom_id: result.forked_phantom_id });
                            }
                        }
                    }
                    InternalReplayEvent::DurableCallCancelled {
                        start_index,
                        partial_response,
                    } => {
                        self.concurrent_resolver.resolve_if_pending(start_index, Resolution::Cancelled { cancelled_idx: event_idx, partial: partial_response });
                    }
                    InternalReplayEvent::SeenLog {
                        level,
                        context,
                        message
                    } => {
                        seen_logs.insert(Self::hash_log_entry(level, &context, &message));
                    }
                }
            }
        }

        self.pending.jumps.clear();
    }

    // Reset internal state to last commit
    fn revert_to_last_commit(&mut self) {
        self.pending.current_oplog_index = self.committed.current_oplog_index;
        self.pending.current_oplog_chunk_region = self.committed.current_oplog_chunk_region.clone();
        self.pending.current_oplog_chunk = Arc::clone(&self.committed.current_oplog_chunk);
        self.pending.last_read_non_hint_oplog_index = self.committed.last_read_non_hint_oplog_index;
        self.pending.next_skipped_region = self.committed.next_skipped_region.clone();
        self.pending.in_persist_nothing_region = self.committed.in_persist_nothing_region;
        self.pending.replay_events.clear();
        self.pending.jumps.clear();
    }

    // Guaranteed to read at least 1 entry and to land on a non-hint, non-skipped entry
    async fn advance_to_next_non_hint_entry(&mut self) {
        assert!(self.pending_is_replay());

        while self.pending_is_replay() {
            self.advance_to_next_non_skipped_entry().await;
            let in_persist_nothing_region = self.pending.in_persist_nothing_region;
            let (idx, entry) = self.get_current_entry().await;

            let entry_is_hint = entry.is_hint();

            match entry {
                OplogEntry::SuccessfulUpdate {
                    target_revision, ..
                } => {
                    let new_revision = *target_revision;
                    self.pending.replay_events.push((Reverse(idx), InternalReplayEvent::UpdateReplayed { new_revision }));
                }
                OplogEntry::CardRevoked { card_id, .. } => {
                    let card_id = CardId(*card_id);
                    self.pending.replay_events.push((Reverse(idx), InternalReplayEvent::CardRevoked { card_id }));
                }
                OplogEntry::Start { function_name, .. } if !in_persist_nothing_region && function_name == &HostFunctionName::GolemApiFork => {
                    self.pending.replay_events.push((Reverse(idx), InternalReplayEvent::ForkStarted));
                }
                OplogEntry::End {
                    start_index,
                    response,
                    forced_commit,
                    ..
                } if !in_persist_nothing_region => {
                    let start_index = *start_index;
                    let response = response.clone();
                    let forced_commit = *forced_commit;
                    self.pending.replay_events.push((
                        Reverse(idx),
                        InternalReplayEvent::DurableCallEnded {
                            start_index,
                            response,
                            forced_commit,
                        }
                    ));
                }
                OplogEntry::Cancelled {
                    start_index,
                    partial,
                    ..
                } if !in_persist_nothing_region => {
                    let start_index = *start_index;
                    let partial_response = partial.clone();
                    self.pending.replay_events.push((
                        Reverse(idx),
                        InternalReplayEvent::DurableCallCancelled {
                            start_index,
                            partial_response,
                        }
                    ));
                }
                OplogEntry::Log { level, context, message, .. } if !in_persist_nothing_region => {
                    let level = *level;
                    let context = context.clone();
                    let message = message.clone();
                    self.pending.replay_events.push((
                        Reverse(idx),
                        InternalReplayEvent::SeenLog {
                            level,
                            context,
                            message
                        }
                    ));
                }
                OplogEntry::ChangePersistenceLevel { persistence_level, .. } => {
                    self.pending.in_persist_nothing_region = *persistence_level != PersistenceLevel::PersistNothing;
                }
                OplogEntry::AgentInvocationFinished { .. } => {
                    self.pending.in_persist_nothing_region = false;
                }
                _ => {}
            }

            if !entry_is_hint && !self.pending.in_persist_nothing_region {
                break;
            };
            self.pending.current_oplog_index = self.pending.current_oplog_index.next()
        }

        self.pending.last_read_non_hint_oplog_index = self.pending.current_oplog_index;

        if self.pending_is_live() {
            self.pending.replay_events.push((Reverse(self.pending.current_oplog_index), InternalReplayEvent::ReplayFinished));
        }
    }

    async fn advance_to_next_non_skipped_entry(&mut self) {
        self.move_to_end_of_skipped_region_in_loaded_chunk().await;
        self.jump_out_of_skipped_region();
    }

    // Move to the end of the loaded chunk in the end of the currently loaded chunk,
    // recording any events in the chunk immediately. This is guaranteed to not load any new chunks.
    async fn move_to_end_of_skipped_region_in_loaded_chunk(&mut self) {
        while self.pending_is_replay()
            && let Some(skipped_region) = &self.pending.next_skipped_region
            && skipped_region.contains(self.pending.current_oplog_index)
            && self.pending.current_oplog_chunk_region.contains(self.pending.current_oplog_index)
        {
            let exits_current_region = skipped_region.end == self.pending.current_oplog_index;

            // Guaranteed to not require a remote read
            let (entry_index, entry) = self.get_current_entry().await;

            if let Some(replay_event) = Self::skipped_oplog_entry_as_replay_event(entry) {
                self.pending.replay_events.push((Reverse(entry_index), replay_event));
            }

            self.pending.current_oplog_index = self.pending.current_oplog_index.next();

            // We moved out of the skipped region
            if exits_current_region {
                self.pending.next_skipped_region = self.skipped_regions.find_next_deleted_region(self.pending.current_oplog_index);
            }
        }
    }

    // Jump to the first oplog entry that should be replayed, discarding all read entries.
    // If the cursor is at a non-skipped entry this is a noop.
    fn jump_out_of_skipped_region(&mut self) {
        while self.pending_is_replay()
            && let Some(skipped_region) = &self.pending.next_skipped_region
            && skipped_region.contains(self.pending.current_oplog_index)
        {
            let skipped_region_end = skipped_region.end;
            let previous_index = self.pending.current_oplog_index;

            self.pending.current_oplog_index = skipped_region_end.next();
            self.pending.next_skipped_region = self.skipped_regions.find_next_deleted_region(self.pending.current_oplog_index);
            self.pending.jumps.push(OplogRegion { start: previous_index, end: skipped_region_end });
        }
    }

    async fn ensure_chunk(&mut self) {
        let idx = self.pending.current_oplog_index;
        if !self.pending.current_oplog_chunk_region.contains(self.pending.current_oplog_index) {
            let new_entries = self.oplog.read_many(idx, CHUNK_SIZE).await.into_values().collect();
            self.pending.current_oplog_chunk_region = OplogRegion { start: idx, end: idx.range_end(CHUNK_SIZE) };
            self.pending.current_oplog_chunk = Arc::new(new_entries);
        }
    }

    async fn get_current_entry(&mut self) -> (OplogIndex, &OplogEntry) {
        self.ensure_chunk().await;
        let idx = self.pending.current_oplog_index;
        let chunk_idx = (idx.as_u64() - self.pending.current_oplog_chunk_region.start.as_u64()) as usize;
        (idx, &self.pending.current_oplog_chunk[chunk_idx])
    }

    // Some operational oplog entries like card-revoked stay relevant even if they are skipped.
    // The skipped chunks are guaranteed to have not been read during the prior replay, so there
    // is nothing to but to read them here.
    async fn extract_replay_events_in_jumped_regions(&mut self) -> BinaryHeap<(Reverse<OplogIndex>, InternalReplayEvent)> {
        let mut result = BinaryHeap::new();

        for region in &self.pending.jumps {
            let mut next = region.start;
            let end = region.end.as_u64();

            while next.as_u64() <= end {
                let remaining = end - next.as_u64() + 1;
                let count = remaining.min(CHUNK_SIZE);

                for (entry_index, entry) in self.oplog.read_many(next, count).await {
                    if let Some(replay_event) = Self::skipped_oplog_entry_as_replay_event(&entry) {
                        result.push((Reverse(entry_index), replay_event));
                    }
                    next = entry_index.next()
                }
            }
        }
        result
    }

    fn skipped_oplog_entry_as_replay_event(entry: &OplogEntry) -> Option<InternalReplayEvent> {
        match entry {
            OplogEntry::CardRevoked { card_id, .. } =>
                Some(InternalReplayEvent::CardRevoked {
                    card_id: CardId(*card_id),
                }),
            _ => None
        }
    }

    fn first_non_skipped_entry(deleted_regions: &DeletedRegions) -> (OplogIndex, Option<OplogRegion>) {
        let mut oplog_index = OplogIndex::INITIAL;
        let mut next_skipped_region = deleted_regions.find_next_deleted_region(oplog_index);

        while let Some(skipped_region) = &next_skipped_region && skipped_region.contains(oplog_index)
        {
            oplog_index = skipped_region.end.next();
            next_skipped_region = deleted_regions.find_next_deleted_region(oplog_index);
        }

        (oplog_index, next_skipped_region)
    }
}
