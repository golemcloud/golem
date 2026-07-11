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

use crate::services::oplog::{Oplog, OplogOps};
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    AtomicOplogIndex, HostResponse, HostResponseGolemApiFork, LogLevel, OplogEntry, OplogIndex,
    PersistenceLevel,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentInvocationPayload, AgentInvocationResult, ForkResult, IdempotencyKey, OwnedAgentId,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use metrohash::MetroHash128;
use std::collections::{HashSet, VecDeque};
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
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
    replay_buffer: VecDeque<(OplogIndex, OplogEntry)>,
}

const REPLAY_READ_CHUNK_SIZE: u64 = 1024;

#[derive(Debug, Clone)]
struct InternalReplayState {
    pub skipped_regions: DeletedRegions,
    pub next_skipped_region: Option<OplogRegion>,
    /// Hashes of log entries persisted since the last read non-hint oplog entry
    pub log_hashes: HashSet<(u64, u64)>,
    /// Updates that were encountered while reading the oplog
    pub pending_replay_events: Vec<ReplayEvent>,
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
            last_replayed_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
            last_replayed_non_hint_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
            replay_target: AtomicOplogIndex::from_oplog_index(last_oplog_index),
            internal: Arc::new(RwLock::new(InternalReplayState {
                skipped_regions,
                next_skipped_region,
                log_hashes: HashSet::new(),
                pending_replay_events: Vec::new(),
            })),
            has_seen_logs: Arc::new(AtomicBool::new(false)),
            replay_buffer: VecDeque::new(),
        };
        result.move_replay_idx(OplogIndex::INITIAL).await; // By this we handle initial skipped regions applied by manual updates correctly
        result.skip_forward().await?;
        Ok(result)
    }

    pub async fn drop_override_and_restart(&mut self) -> Result<(), WorkerExecutorError> {
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
        self.skip_forward().await
    }

    pub async fn switch_to_live(&mut self) {
        if !self.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished).await;
        }
        self.last_replayed_index.set(self.replay_target.get());
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

    pub fn set_replay_target(&mut self, new_target: OplogIndex) {
        if new_target < self.replay_target.get() {
            self.replay_buffer.clear();
        }
        self.replay_target.set(new_target)
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

    async fn record_replay_event(&mut self, event: ReplayEvent) {
        self.internal
            .write()
            .await
            .pending_replay_events
            .push(event)
    }

    pub async fn take_new_replay_events(&mut self) -> Vec<ReplayEvent> {
        std::mem::take(&mut self.internal.write().await.pending_replay_events)
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
            Some(self.last_replayed_index())
        } else if let OplogEntry::ChangePersistenceLevel {
            persistence_level, ..
        } = &entry
        {
            if persistence_level == &PersistenceLevel::PersistNothing {
                let begin_index = self.last_replayed_index();
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
        let saved_replay_idx = self.last_replayed_index.get();
        let saved_next_skipped_region = {
            let internal = self.internal.read().await;
            internal.next_skipped_region.clone()
        };

        let read_idx = self.last_replayed_index.get().next();
        let entry = self.internal_get_next_oplog_entry().await?;

        if condition(&entry) {
            self.skip_forward().await?;
            self.last_replayed_non_hint_index.set(read_idx);

            Ok(Some((read_idx, entry)))
        } else {
            self.rewind_replay_buffer(read_idx, entry);
            self.last_replayed_index.set(saved_replay_idx);
            let mut internal = self.internal.write().await;
            internal.next_skipped_region = saved_next_skipped_region;

            Ok(None)
        }
    }

    fn rewind_replay_buffer(&mut self, idx: OplogIndex, entry: OplogEntry) {
        if self
            .replay_buffer
            .front()
            .map(|(front_idx, _)| *front_idx != idx)
            .unwrap_or(true)
        {
            self.replay_buffer.push_front((idx, entry));
        }
    }

    async fn skip_forward(&mut self) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs = HashSet::new();
        while self.is_replay() {
            let saved_replay_idx = self.last_replayed_index.get();
            let saved_next_skipped_region = {
                let internal = self.internal.read().await;
                internal.next_skipped_region.clone()
            };
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

                    // Moving the replay pointer. Leaving last_replayed_non_hint_index unchanged, because this is a hint entry.
                    self.last_replayed_index.set(last_read_idx);
                    // TODO: what to do with next_skipped_region if we jumped forward to end of persist-nothing zone?
                }
                None => {
                    // We've found the first non-hint entry after the first read one,
                    // so we move everything back the last position (saved_replay_idx), including
                    // possibly skipped regions.
                    self.rewind_replay_buffer(saved_replay_idx.next(), entry);
                    self.last_replayed_index.set(saved_replay_idx);
                    let mut internal = self.internal.write().await;
                    // TODO: cache the last hint entry to avoid reading it again
                    internal.next_skipped_region = saved_next_skipped_region;
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

    /// Gets the next oplog entry, no matter if it is hint or not, following jumps.
    ///
    /// Returns an error (rather than panicking) if the expected entry is missing
    /// or if the eager `GolemApiFork` payload inspection fails. The caller (and
    /// transitively any host function) propagates the error up so the worker
    /// fails the agent with a non-retriable trap instead of crashing the
    /// executor process.
    async fn internal_get_next_oplog_entry(&mut self) -> Result<OplogEntry, WorkerExecutorError> {
        let read_idx = self.last_replayed_index.get().next();

        while self
            .replay_buffer
            .front()
            .map(|(idx, _)| *idx < read_idx)
            .unwrap_or(false)
        {
            self.replay_buffer.pop_front();
        }

        if self
            .replay_buffer
            .front()
            .map(|(idx, _)| *idx > read_idx)
            .unwrap_or(false)
        {
            self.replay_buffer.clear();
        }

        if self.replay_buffer.is_empty() {
            let remaining = u64::from(self.replay_target.get())
                .saturating_sub(u64::from(read_idx))
                .saturating_add(1);
            self.replay_buffer = self
                .read_oplog(read_idx, remaining.min(REPLAY_READ_CHUNK_SIZE))
                .await
                .into_iter()
                .collect();

            if self
                .replay_buffer
                .front()
                .map(|(idx, _)| *idx != read_idx)
                .unwrap_or(true)
            {
                self.replay_buffer = self.read_oplog(read_idx, 1).await.into_iter().collect();
            }
        }

        let oplog_entry = if let Some((idx, oplog_entry)) = self.replay_buffer.pop_front()
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
                    self.owned_agent_id,
                    read_idx,
                    self.replay_target.get(),
                    self.last_replayed_non_hint_index.get()
                ),
            ));
        };

        // record side effects that need to be applied at the next opportunity
        if let OplogEntry::SuccessfulUpdate {
            target_revision, ..
        } = oplog_entry
        {
            self.record_replay_event(ReplayEvent::UpdateReplayed {
                new_revision: target_revision,
            })
            .await
        }
        if let OplogEntry::HostCall {
            function_name,
            response,
            ..
        } = &oplog_entry
            && function_name == &HostFunctionName::GolemApiFork
        {
            let response = self
                .oplog
                .download_payload(response.clone())
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

        if read_idx == self.replay_target.get() {
            self.record_replay_event(ReplayEvent::ReplayFinished).await
        }

        self.move_replay_idx(read_idx).await;

        Ok(oplog_entry)
    }

    async fn move_replay_idx(&mut self, new_idx: OplogIndex) {
        self.last_replayed_index.set(new_idx);
        self.get_out_of_skipped_region().await;
        while self
            .replay_buffer
            .front()
            .map(|(idx, _)| *idx <= self.last_replayed_index.get())
            .unwrap_or(false)
        {
            self.replay_buffer.pop_front();
        }
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

        let mut current_next_skip_region = self.internal.read().await.next_skipped_region.clone();
        let mut violation = false;

        while start < replay_target {
            let entries = self.read_oplog(start, REPLAY_READ_CHUNK_SIZE).await;
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

    async fn get_out_of_skipped_region(&mut self) {
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
        let result: Vec<(OplogIndex, OplogEntry)> =
            self.oplog.read_many(idx, n).await.into_iter().collect();
        result
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
    use golem_common::model::oplog::{PayloadId, RawOplogPayload};
    use golem_common::model::{AgentId, Timestamp};
    use std::collections::BTreeMap;
    use std::fmt::{Debug, Formatter};
    use std::sync::Mutex;
    use std::time::Duration;
    use test_r::test;

    struct SparseBatchOplog;

    struct MutableBatchOplog {
        entries: Mutex<BTreeMap<OplogIndex, OplogEntry>>,
    }

    impl MutableBatchOplog {
        fn new(entries: BTreeMap<OplogIndex, OplogEntry>) -> Self {
            Self {
                entries: Mutex::new(entries),
            }
        }

        fn replace(&self, index: OplogIndex, entry: OplogEntry) {
            self.entries.lock().unwrap().insert(index, entry);
        }
    }

    impl Debug for MutableBatchOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MutableBatchOplog").finish()
        }
    }

    #[async_trait]
    impl Oplog for MutableBatchOplog {
        async fn add(&self, _entry: OplogEntry) -> OplogIndex {
            unimplemented!()
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            unimplemented!()
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            unimplemented!()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            *self.entries.lock().unwrap().last_key_value().unwrap().0
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            None
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unimplemented!()
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            self.entries.lock().unwrap()[&oplog_index].clone()
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            self.entries
                .lock()
                .unwrap()
                .range(oplog_index..)
                .take(n as usize)
                .map(|(index, entry)| (*index, entry.clone()))
                .collect()
        }

        async fn length(&self) -> u64 {
            self.entries.lock().unwrap().len() as u64
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

        async fn switch_persistence_level(&self, _mode: PersistenceLevel) {
            unimplemented!()
        }
    }

    impl Debug for SparseBatchOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SparseBatchOplog").finish()
        }
    }

    #[async_trait]
    impl Oplog for SparseBatchOplog {
        async fn add(&self, _entry: OplogEntry) -> OplogIndex {
            unimplemented!()
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            unimplemented!()
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            unimplemented!()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            OplogIndex::from_u64(3)
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            None
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unimplemented!()
        }

        async fn read(&self, _oplog_index: OplogIndex) -> OplogEntry {
            OplogEntry::NoOp {
                timestamp: Timestamp::now_utc(),
            }
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let entry = OplogEntry::NoOp {
                timestamp: Timestamp::now_utc(),
            };
            if n == 1 || oplog_index == OplogIndex::INITIAL.next() {
                BTreeMap::from([(oplog_index, entry)])
            } else {
                BTreeMap::from([(oplog_index.next(), entry)])
            }
        }

        async fn length(&self) -> u64 {
            3
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

        async fn switch_persistence_level(&self, _mode: PersistenceLevel) {
            unimplemented!()
        }
    }

    #[test]
    async fn replay_reads_sparse_batch_entries_individually() {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "test".to_string(),
        };
        let mut state = ReplayState::new(
            OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            Arc::new(SparseBatchOplog),
            DeletedRegions::new(),
        )
        .await
        .unwrap();

        assert!(matches!(
            state.get_oplog_entry().await.unwrap().1,
            OplogEntry::NoOp { .. }
        ));
        assert!(matches!(
            state.get_oplog_entry().await.unwrap().1,
            OplogEntry::NoOp { .. }
        ));
    }

    #[test]
    async fn lowering_replay_target_discards_prefetched_future_entries() {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "test".to_string(),
        };
        let original = OplogEntry::NoOp {
            timestamp: Timestamp::from(1),
        };
        let replacement = OplogEntry::NoOp {
            timestamp: Timestamp::from(2),
        };
        let oplog = Arc::new(MutableBatchOplog::new(BTreeMap::from([
            (OplogIndex::INITIAL, original.clone()),
            (OplogIndex::INITIAL.next(), original.clone()),
            (OplogIndex::INITIAL.next().next(), original.clone()),
        ])));
        let mut state = ReplayState::new(
            OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            oplog.clone(),
            DeletedRegions::new(),
        )
        .await
        .unwrap();

        state.set_replay_target(OplogIndex::INITIAL.next());
        state.get_oplog_entry().await.unwrap();
        oplog.replace(OplogIndex::INITIAL.next().next(), replacement.clone());
        state.set_replay_target(OplogIndex::INITIAL.next().next());

        assert_eq!(
            state.get_oplog_entry().await.unwrap().1,
            replacement,
            "replay must not consume entries prefetched before its target moved backward"
        );
    }
}
