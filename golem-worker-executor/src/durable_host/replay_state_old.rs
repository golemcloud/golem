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
use std::collections::HashSet;
use std::hash::Hasher;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::debug;
use uuid::Uuid;

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
struct PendingReplayRead {
    idx: OplogIndex,
    entry: OplogEntry,
    skipped_regions: Vec<OplogRegion>,
}

#[derive(Debug)]
pub struct ReplayState {
    owned_agent_id: OwnedAgentId,
    oplog: Arc<dyn Oplog>,
    replay_target: OplogIndex,
    /// The oplog index of the last replayed entry
    last_replayed_index: OplogIndex,
    /// The last replayed entry whose side effects have been committed.
    committed_replayed_index: OplogIndex,
    /// The oplog index of the last non-hint entry read
    last_replayed_non_hint_index: OplogIndex,
    internal: InternalReplayState,
    has_seen_logs: bool,
}

#[derive(Debug)]
struct InternalReplayState {
    pub skipped_regions: DeletedRegions,
    pub next_skipped_region: Option<OplogRegion>,
    pub committed_next_skipped_region: Option<OplogRegion>,
    /// Hashes of log entries persisted since the last read non-hint oplog entry
    pub log_hashes: HashSet<(u64, u64)>,
    /// Updates that were encountered while reading the oplog
    pub pending_replay_events: Vec<ReplayEvent>,
    pub pending_reads: Vec<PendingReplayRead>,
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
        let mut result = Self {
            owned_agent_id,
            oplog,
            last_replayed_index: OplogIndex::NONE,
            committed_replayed_index: OplogIndex::NONE,
            last_replayed_non_hint_index: OplogIndex::NONE,
            replay_target: last_oplog_index,
            internal: InternalReplayState {
                skipped_regions,
                next_skipped_region: next_skipped_region.clone(),
                committed_next_skipped_region: next_skipped_region,
                log_hashes: HashSet::new(),
                pending_replay_events: Vec::new(),
                pending_reads: Vec::new(),
                pending_fork_starts: HashSet::new(),
                concurrent_resolver: ConcurrentReplayResolver::default(),
            },
            has_seen_logs: false,
        };
        result.move_to_start_of_replay().await;
        result.skip_forward().await?;
        Ok(result)
    }

    pub async fn drop_override_and_restart(&mut self) -> Result<(), WorkerExecutorError> {
        self.internal.skipped_regions.drop_override();
        self.internal.next_skipped_region = self
            .internal
            .skipped_regions
            .find_next_deleted_region(OplogIndex::NONE);
        self.internal.log_hashes.clear();
        self.internal.pending_replay_events.clear();
        self.internal.pending_reads.clear();
        self.internal.committed_next_skipped_region = self.internal.next_skipped_region.clone();
        self.last_replayed_index = OplogIndex::NONE;
        self.committed_replayed_index = OplogIndex::NONE;
        self.last_replayed_non_hint_index = OplogIndex::NONE;
        self.move_to_start_of_replay().await;
        self.skip_forward().await
    }

    pub async fn switch_to_live(&mut self) {
        if !self.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished).await;
        }
        self.last_replayed_index = self.replay_target;
        self.committed_replayed_index = self.replay_target;
    }

    pub fn last_replayed_index(&self) -> OplogIndex {
        self.last_replayed_index
    }

    pub fn last_replayed_non_hint_index(&self) -> OplogIndex {
        self.last_replayed_non_hint_index
    }

    pub fn replay_target(&self) -> OplogIndex {
        self.replay_target
    }

    pub fn set_replay_target(&mut self, new_target: OplogIndex) {
        self.replay_target = new_target;
    }

    pub fn is_in_skipped_region(&self, oplog_index: OplogIndex) -> bool {
        self.internal
            .skipped_regions
            .is_in_deleted_region(oplog_index)
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.last_replayed_index == self.replay_target
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    async fn record_replay_event(&mut self, event: ReplayEvent) {
        self.internal.pending_replay_events.push(event)
    }

    pub async fn take_new_replay_events(&mut self) -> Vec<ReplayEvent> {
        std::mem::take(&mut self.internal.pending_replay_events)
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
    async fn should_skip_to(&self, entry: &OplogEntry) -> Option<OplogIndex> {
        if entry.is_hint() {
            // Keeping the last replayed index as-is, so the next attempt will read the next one
            Some(self.last_replayed_index)
        } else if let OplogEntry::ChangePersistenceLevel {
            persistence_level, ..
        } = &entry
        {
            if persistence_level == &PersistenceLevel::PersistNothing {
                let begin_index = self.last_replayed_index;
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
    /// If the condition is not met, returns None and the current replay state remains
    /// unchanged.
    ///
    /// The auto-skipped hint entries can be of two kind:
    /// - A set of oplog entry cases are always hint entries. They manipulate the worker status
    ///   but are non-deterministic from the replay's point of view.
    /// - Every oplog entry recorded in persist-nothing zones. These are there for observability,
    ///   but they never participate in the replay. A persist-nothing zone is bounded by two
    ///   ChangePersistenceLevel entries, or if the closing one is missing, it is up to the end of the
    ///   oplog.
    pub async fn try_get_oplog_entry(
        &mut self,
        condition: impl FnOnce(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let saved_replay_idx = self.last_replayed_index;
        let saved_next_skipped_region = self.internal.next_skipped_region.clone();

        let read_idx = self.last_replayed_index.next();
        let entry = self.internal_get_next_oplog_entry().await?;

        if condition(&entry) {
            self.commit_pending_replay().await?;
            self.skip_forward().await?;
            self.last_replayed_non_hint_index = read_idx;

            Ok(Some((read_idx, entry)))
        } else {
            self.revert_pending_replay().await;

            Ok(None)
        }
    }

    async fn skip_forward(&mut self) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs = HashSet::new();
        while self.is_replay() {
            let entry = self.internal_get_next_oplog_entry().await?;
            match self.should_skip_to(&entry).await {
                Some(last_read_idx) => {
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

                    if last_read_idx > self.last_replayed_index {
                        self.record_pending_skipped_region(OplogRegion {
                            start: self.last_replayed_index.next(),
                            end: last_read_idx,
                        })
                        .await;
                    }
                    // Moving the replay pointer. Leaving last_replayed_non_hint_index unchanged, because this is a hint entry.
                    self.last_replayed_index = last_read_idx;
                    self.commit_pending_replay().await?;
                    // TODO: what to do with next_skipped_region if we jumped forward to end of persist-nothing zone?
                }
                None => {
                    // We've found the first non-hint entry after the first read one,
                    // so we move everything back the last position (saved_replay_idx), including
                    // possibly skipped regions.
                    // TODO: cache the last hint entry to avoid reading it again
                    self.revert_pending_replay().await;
                    break;
                }
            }
        }

        self.has_seen_logs = !logs.is_empty();
        self.internal.log_hashes = logs;
        Ok(())
    }

    /// Returns true if the given log entry has been seen since the last non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        if self.has_seen_logs {
            let hash = Self::hash_log_entry(level, context, message);
            self.internal.log_hashes.contains(&hash)
        } else {
            false
        }
    }

    /// Removes a seen log from the set. If the set becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&mut self, level: LogLevel, context: &str, message: &str) {
        let hash = Self::hash_log_entry(level, context, message);
        self.internal.log_hashes.remove(&hash);
        self.has_seen_logs = !self.internal.log_hashes.is_empty();
    }

    fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
        let mut hasher = MetroHash128::new();
        hasher.write_u8(level as u8);
        hasher.write(context.as_bytes());
        hasher.write(message.as_bytes());
        hasher.finish128()
    }

    /// Gets the next oplog entry, no matter if it is hint or not, following jumps.
    ///
    /// Returns an error (rather than panicking) if the expected entry is missing
    /// or if the eager `GolemApiFork` payload inspection fails. The caller (and
    /// transitively any host function) propagates the error up so the worker
    /// fails the agent with a non-retriable trap instead of crashing the
    /// executor process.
    async fn internal_get_next_oplog_entry(&mut self) -> Result<OplogEntry, WorkerExecutorError> {
        let read_idx = self.last_replayed_index.next();

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
                    self.replay_target,
                    self.last_replayed_non_hint_index
                ),
            ));
        };

        let skipped_regions = self.move_replay_idx(read_idx).await;
        self.internal
            .pending_reads
            .push(PendingReplayRead {
                idx: read_idx,
                entry: oplog_entry.clone(),
                skipped_regions,
            });

        Ok(oplog_entry)
    }

    // Moves to the start of the region used for replay, handling initial skipped regions applied by manual updates correctly
    async fn move_to_start_of_replay(&mut self) {
        self.last_replayed_index = OplogIndex::INITIAL;
        self.get_out_of_skipped_region(true).await;
        self.committed_replayed_index = self.last_replayed_index;
        self.internal.committed_next_skipped_region = self.internal.next_skipped_region.clone();
    }

    async fn move_replay_idx(&mut self, new_idx: OplogIndex) -> Vec<OplogRegion> {
        self.last_replayed_index = new_idx;
        self.get_out_of_skipped_region(false).await
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
        let replay_target = self.replay_target;
        let mut start = self.last_replayed_index.next();

        let mut current_next_skip_region = self.internal.next_skipped_region.clone();
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

    async fn get_out_of_skipped_region(&mut self, initial_skip: bool) -> Vec<OplogRegion> {
        let mut skipped_regions = Vec::new();
        if self.is_replay() {
            let skipped_region = {
                match &self.internal.next_skipped_region {
                    Some(region) if region.start == (self.last_replayed_index.next()) => {
                        let target = region.end.next(); // we want to continue reading _after_ the region
                        debug!(
                            "Worker reached skipped region at {}, jumping to {} (oplog size: {})",
                            region.start, target, self.replay_target
                        );
                        self.last_replayed_index = target.previous(); // so we set the last replayed index to the end of the region
                        Some(region.clone())
                    }
                    _ => None,
                }
            };

            if let Some(skipped_region) = skipped_region {
                // Initial skip is used to advance the replay cursor to the beginning of the replay / index of the loaded snapshot.
                // All card events in that region are already part of the snapshot, so no need to consider them here.
                if !initial_skip {
                    skipped_regions.push(skipped_region);
                }
                self.internal.next_skipped_region = self
                    .internal
                    .skipped_regions
                    .find_next_deleted_region(self.last_replayed_index);
            }
        }
        skipped_regions
    }

    async fn record_card_revoked_events_in_region(&mut self, region: &OplogRegion) {
        let mut next = region.start;
        let end = region.end.as_u64();

        while next.as_u64() <= end {
            let remaining = end - next.as_u64() + 1;
            let count = remaining.min(CHUNK_SIZE);

            for (entry_index, entry) in self.read_oplog(next, count).await {
                if let OplogEntry::CardRevoked { card_id, .. } = entry {
                    self.record_replay_event(ReplayEvent::CardRevoked {
                        card_id: CardId(card_id),
                    })
                    .await;
                }
                next = entry_index.next()
            }
        }
    }

    async fn read_oplog(&self, idx: OplogIndex, n: u64) -> Vec<(OplogIndex, OplogEntry)> {
        self.oplog.read_many(idx, n).await.into_iter().collect()
    }

    async fn record_pending_skipped_region(&mut self, region: OplogRegion) {
        if let Some(read) = self.internal.pending_reads.last_mut() {
            read.skipped_regions.push(region);
        }
    }

    async fn commit_pending_replay(&mut self) -> Result<(), WorkerExecutorError> {
        let reads = std::mem::take(&mut self.internal.pending_reads);
        for read in reads {
            self.on_committed_replay_entry(read.idx, &read.entry)
                .await?;
            for skipped_region in &read.skipped_regions {
                self.record_card_revoked_events_in_region(skipped_region)
                    .await;
            }
        }

        if self.last_replayed_index == self.replay_target {
            self.record_replay_event(ReplayEvent::ReplayFinished).await;
        }
        self.committed_replayed_index = self.last_replayed_index;
        self.internal.committed_next_skipped_region = self.internal.next_skipped_region.clone();
        Ok(())
    }

    async fn revert_pending_replay(&mut self) {
        self.last_replayed_index = self.committed_replayed_index;
        self.internal.next_skipped_region = self.internal.committed_next_skipped_region.clone();
        self.internal.pending_reads.clear();
    }

    /// Feeds the concurrent replay resolver when an `End`/`Cancelled` entry is *committed*
    /// (permanently consumed). Resolves only calls that are actually being awaited
    /// (`resolve_if_pending`), so the `End`/`Cancelled` of any call not tracked by the resolver —
    /// e.g. the guest-facing manual durability pair, consumed through this same cursor but never
    /// registered — is ignored instead of leaking.
    async fn on_committed_replay_entry(
        &mut self,
        idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        match entry {
            OplogEntry::SuccessfulUpdate {
                target_revision, ..
            } => {
                self.record_replay_event(ReplayEvent::UpdateReplayed {
                    new_revision: *target_revision,
                })
                .await;
            }
            OplogEntry::CardRevoked { card_id, .. } => {
                self.record_replay_event(ReplayEvent::CardRevoked {
                    card_id: CardId(*card_id),
                })
                .await;
            }
            OplogEntry::Start { function_name, .. }
                if function_name == &HostFunctionName::GolemApiFork =>
            {
                self.internal.pending_fork_starts.insert(idx);
            }
            OplogEntry::End {
                start_index,
                response,
                forced_commit,
                ..
            } => {
                let is_pending_fork_start = {
                    let is_pending_fork_start =
                        self.internal.pending_fork_starts.remove(start_index);
                    self.internal.concurrent_resolver.resolve_if_pending(
                        *start_index,
                        Resolution::Completed {
                            end_idx: idx,
                            response: response.clone(),
                            forced_commit: *forced_commit,
                        },
                    );
                    is_pending_fork_start
                };

                if is_pending_fork_start && let Some(response_payload) = response {
                    let response = self
                        .oplog
                        .download_payload(response_payload.clone())
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to download GolemApiFork oplog payload at index {idx}: {err}"
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
            OplogEntry::Cancelled {
                start_index,
                partial,
                ..
            } => {
                self.internal.concurrent_resolver.resolve_if_pending(
                    *start_index,
                    Resolution::Cancelled {
                        cancelled_idx: idx,
                        partial: partial.clone(),
                    },
                );
            }
            _ => {}
        }
        Ok(())
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
            self.internal
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
                let receiver = { self.internal.concurrent_resolver.register(start_idx) };
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
                    self.internal
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
                // happened in between). Drop the now-stale registration and report Incomplete so the
                // caller can re-execute the side effect and complete the existing `Start`.
                self.internal
                    .concurrent_resolver
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
                self.internal
                    .concurrent_resolver
                    .unregister(start_idx);
                return Err(WorkerExecutorError::runtime(format!(
                    "concurrent replay interleaving is not supported: encountered a non-End/Cancelled entry while awaiting resolution of Start at {start_idx}"
                )));
            }
            // The consumed entry was an End/Cancelled; the committed-consume hook has resolved the
            // receiver, which the next loop iteration picks up.
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
