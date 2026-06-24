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
    DurableFunctionType, HostResponse, HostResponseGolemApiFork, LogLevel, OplogEntry, OplogIndex,
    PersistenceLevel,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentInvocationPayload, AgentInvocationResult, ForkResult, IdempotencyKey, OwnedAgentId,
    Timestamp,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use metrohash::MetroHash128;
use std::collections::{BTreeMap, HashSet};
use std::hash::Hasher;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::debug;
use uuid::Uuid;
use std::rc::Rc;

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

#[derive(Debug)]
struct PendingState {
    current_oplog_index: OplogIndex,
    current_oplog_chunk_region: OplogRegion,
    current_oplog_chunk: Rc<Vec<OplogEntry>>,
    last_read_non_hint_oplog_index: OplogIndex,
    // invariant: either start or end must be at or after last_read_oplog_index.
    next_skipped_region: Option<OplogRegion>,
    seen_logs: HashSet<(u64, u64)>,
    replay_events: BTreeMap<OplogIndex, ReplayEvent>,
    jumps: Vec<OplogRegion>
}

#[derive(Debug)]
struct CommittedState {
    current_oplog_index: OplogIndex,
    current_oplog_chunk_region: OplogRegion,
    current_oplog_chunk: Rc<Vec<OplogEntry>>,
    last_read_non_hint_oplog_index: OplogIndex,
    // invariant: either start or end must be at or after last_read_oplog_index.
    next_skipped_region: Option<OplogRegion>,
    seen_logs: HashSet<(u64, u64)>,
    replay_events: Vec<ReplayEvent>,
}

#[derive(Debug)]
pub struct ReplayState {
    oplog: Arc<dyn Oplog>,

    // === initialization-only ===
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
    ) -> Result<Self, WorkerExecutorError> {
        unimplemented!()
    }

    pub async fn drop_override_and_restart(&mut self) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    pub async fn switch_to_live(&mut self) {
        unimplemented!()
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
        self.last_replayed_index() >= self.replay_target()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
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

        self.advance_to_next_non_hint().await;
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
            self.advance_to_next_non_hint().await;
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
        self.committed.current_oplog_chunk = self.pending.current_oplog_chunk.clone();
        self.committed.last_read_non_hint_oplog_index = self.pending.last_read_non_hint_oplog_index;
        self.committed.next_skipped_region = self.pending.next_skipped_region.clone();


        self.pending.seen_logs.clear();
        self.pending.replay_events.clear();
        self.pending.jumps.clear();
    }

    // Reset internal state to last commit
    fn revert_to_last_commit(&mut self) {
        self.pending.current_oplog_index = self.committed.current_oplog_index;
        self.pending.current_oplog_chunk_region = self.committed.current_oplog_chunk_region.clone();
        self.pending.current_oplog_chunk = self.committed.current_oplog_chunk.clone();
        self.pending.last_read_non_hint_oplog_index = self.committed.last_read_non_hint_oplog_index;
        self.pending.next_skipped_region = self.committed.next_skipped_region.clone();
        self.pending.seen_logs.clear();
        self.pending.replay_events.clear();
        self.pending.jumps.clear();
    }

    fn record_replay_event(&mut self, oplog_index: OplogIndex, event: ReplayEvent) {
        self.pending.replay_events.insert(oplog_index, event);
    }

    // Jump to the first oplog entry that should be replayed, discarding all read entries.
    // If the cursor is at a non-skipped entry this is a noop.
    fn jump_out_of_skipped_region(&mut self) {
        while self.is_replay()
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

    // Guaranteed to read at least 1 entry and to land on a non-hint, non-skipped entry
    async fn advance_to_next_non_hint(&mut self) {
        assert!(self.is_replay());

        while self.is_replay() {
            self.advance_current_index();
            let (idx, entry) = self.get_current_entry().await;
            match entry {
                OplogEntry::SuccessfulUpdate {
                    target_revision, ..
                } => {
                    let new_revision = *target_revision;
                    self.record_replay_event(idx, ReplayEvent::UpdateReplayed { new_revision });
                }
                OplogEntry::CardRevoked { card_id, .. } => {
                    let card_id = CardId(*card_id);
                    self.record_replay_event(idx, ReplayEvent::CardRevoked { card_id });
                }
                OplogEntry::Start { .. } => {
                    todo!()
                }
                OplogEntry::End { .. } => {
                    todo!()
                }
                other if other.is_hint() => { }
                _ => { break; }
            }
        }
    }

    fn advance_current_index(&mut self) {
        self.pending.current_oplog_index = self.pending.current_oplog_index.next();
        self.jump_out_of_skipped_region();
    }

    async fn ensure_chunk(&mut self) {
        let idx = self.pending.current_oplog_index;
        if !self.pending.current_oplog_chunk_region.contains(self.pending.current_oplog_index) {
            let new_entries = self.oplog.read_many(idx, CHUNK_SIZE).await.into_values().collect();
            self.pending.current_oplog_chunk_region = OplogRegion { start: idx, end: idx.range_end(CHUNK_SIZE) };
            self.pending.current_oplog_chunk = Rc::new(new_entries);
        }
    }

    async fn get_current_entry(&mut self) -> (OplogIndex, &OplogEntry) {
        self.ensure_chunk().await;
        let idx = self.pending.current_oplog_index;
        let chunk_idx = (idx.as_u64() - self.pending.current_oplog_chunk_region.start.as_u64()) as usize;
        (idx, &self.pending.current_oplog_chunk[chunk_idx])
    }
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
        let mut rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
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
        let mut rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
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
        let mut rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
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
        assert!(
            !rs.internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2)),
            "failed typed claim must not leave a pending awaiter"
        );
    }

    #[test]
    async fn speculative_read_does_not_resolve() {
        let mut rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let start_idx = handle.start_idx();

        // A speculative read whose condition fails rolls the cursor back and must NOT resolve.
        let speculative = rs.try_get_oplog_entry(|_| false).await.unwrap();
        assert!(speculative.is_none());
        {
            assert!(
                rs.internal.concurrent_resolver.is_pending(start_idx),
                "speculative rollback must not resolve the handle"
            );
        }

        // The committed consume does resolve it.
        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(3)),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    async fn error_hint_between_start_and_end_resolves() {
        // [NoOp, Start, Error{retry_from: Start}, End] — Error is a hint, skipped transparently.
        let mut rs = replay_state_over(vec![
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
        let mut rs = replay_state_over(vec![noop(), start_now()]).await;
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
        let mut rs = replay_state_over(vec![noop(), start_now()]).await;
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
        assert!(
            !rs.internal.concurrent_resolver.is_pending(start_idx),
            "incomplete outcome must unregister the awaiter"
        );
    }

    #[test]
    async fn await_refuses_to_cross_unclaimed_start() {
        // [NoOp, Start(claimed), Start(unclaimed), End(for first)] — awaiting the first call must
        // not drive past the second, unclaimed Start.
        let mut rs =
            replay_state_over(vec![noop(), start_now(), start_now(), end_for(2, 42)]).await;
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

        let err = rs.await_resolution(handle).await.unwrap_err();
        let message = format!("{err}");
        assert!(
            message.contains("interleaving"),
            "unexpected error: {message}"
        );
    }

    #[test]
    async fn await_refuses_to_cross_non_terminal_entry() {
        // [NoOp, Start(claimed), BeginAtomicRegion, End(for first)] — awaiting the first call must
        // not drive past a non-hint, non-End/Cancelled entry (here a scope marker) that a legacy
        // positional reader still expects to consume.
        let mut rs = replay_state_over(vec![
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
        assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

        let err = rs.await_resolution(handle).await.unwrap_err();
        let message = format!("{err}");
        assert!(
            message.contains("interleaving"),
            "unexpected error: {message}"
        );
    }
}
