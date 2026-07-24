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

use crate::debug_session::{DebugSessionId, DebugSessions};
use async_trait::async_trait;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use golem_worker_executor::services::oplog::{
    CommitLevel, Oplog, OrderedOplogStart, PendingUpload,
};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

pub struct DebugOplog {
    pub inner: Arc<dyn Oplog>,
    pub oplog_state: DebugOplogState,
}

impl DebugOplog {
    pub fn new(
        inner: Arc<dyn Oplog>,
        debug_session_id: DebugSessionId,
        debug_session: Arc<dyn DebugSessions>,
    ) -> Self {
        let oplog_state = DebugOplogState {
            debug_session_id,
            debug_session,
        };

        Self { inner, oplog_state }
    }

    pub async fn get_oplog_entry_applying_overrides(
        playback_overrides: HashMap<OplogIndex, OplogEntry>,
        oplog_index: OplogIndex,
        oplog: Arc<dyn Oplog + Send + Sync>,
    ) -> OplogEntry {
        if let Some(entry) = playback_overrides.get(&oplog_index) {
            entry.clone()
        } else {
            oplog.read(oplog_index).await
        }
    }
}

impl Debug for DebugOplog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugOplog").finish()
    }
}

pub struct DebugOplogState {
    debug_session_id: DebugSessionId,
    debug_session: Arc<dyn DebugSessions + Send + Sync>,
}

#[async_trait]
impl Oplog for DebugOplog {
    // We don't allow debugging session to add anything into oplog
    // which internally can get committed.
    //
    // The returned `OplogIndex::NONE` is safe: it is never persisted (all writes are discarded)
    // and replay-side pairing always reads real indices from the recorded oplog. Live-mode
    // bookkeeping (`begin_index`, `parent_start_index`, an `End`'s `start_index`) may carry
    // `NONE`, but that state only feeds further discarded writes. Debug sessions also never
    // live-repair an incomplete durable call
    // (`DebugContext::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS` is `false`), so no repaired
    // `Start`/`End` pair is ever created against a `NONE` index during replay.
    async fn add(&self, _entry: OplogEntry) -> OplogIndex {
        OplogIndex::NONE
    }

    // Mirrors `add`: a debugging session never writes to the oplog, so both entries are built (to
    // satisfy the closure contract) and discarded.
    async fn add_pair(
        &self,
        _start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        let _second = make_second(OplogIndex::NONE);
        (OplogIndex::NONE, OplogIndex::NONE)
    }

    // Mirrors `add`: a debugging session never writes to the oplog, so this builds the `Start` (to
    // satisfy the return type) but does not persist it.
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

    // There is no need to commit anything to the indexed storage
    async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        BTreeMap::new()
    }

    // Current Oplog Index acts as the Replay Target
    // In a new worker, ReplayState begins with last_replayed_index
    async fn current_oplog_index(&self) -> OplogIndex {
        let debug_session_data = self
            .oplog_state
            .debug_session
            .get(&self.oplog_state.debug_session_id)
            .await
            .expect("Internal Error. Current Oplog Index failed. Debug session not found");

        // If a debug session not found but hasn't been set up with a target index,
        // it implies, we only connected to the worker and haven't started debugging yet.
        if let Some(index) = debug_session_data.target_oplog_index {
            index
        } else {
            self.inner.current_oplog_index().await
        }
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.inner.wait_for_replicas(replicas, timeout).await
    }

    // Reads never move the debug session's replay position: replay's single-entry reads are
    // speculative (progress is only committed via `on_replay_progress`), and other components
    // (for example P3 request-body reconstruction) perform unrelated point lookups.
    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let debug_session_data = self
            .oplog_state
            .debug_session
            .get(&self.oplog_state.debug_session_id)
            .await
            .expect("Internal Error. Read failed. Debug session not found");
        let playback_overrides = debug_session_data.playback_overrides.clone();

        Self::get_oplog_entry_applying_overrides(
            playback_overrides.overrides,
            oplog_index,
            self.inner.clone(),
        )
        .await
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        // The read must be clamped to the debug session's view of the oplog end (the playback
        // target index, when one is set): replay lookahead scans (for example resolving concurrent
        // durable-call pairings) read fixed-size chunks that can extend past it, and the
        // underlying oplog's single-entry `read` panics on a missing index. Clamping to the
        // target — not the real recorded end — also guarantees that a durable call whose `End`
        // lies beyond the playback target is seen as incomplete, so debug playback refuses to
        // resolve it from entries the session is not supposed to observe yet. A target past the
        // recorded end (playback running into live mode) is still bounded by the real oplog.
        let last_recorded = self
            .current_oplog_index()
            .await
            .min(self.inner.current_oplog_index().await);
        let mut result = BTreeMap::new();
        if n == 0 || oplog_index > last_recorded {
            return result;
        }
        let available = u64::from(last_recorded) - u64::from(oplog_index) + 1;
        let count = n.min(available);

        // Like `read`, this never moves the debug session's replay position; it only applies the
        // playback overrides on top of the underlying entries.
        let debug_session_data = self
            .oplog_state
            .debug_session
            .get(&self.oplog_state.debug_session_id)
            .await
            .expect("Internal Error. Read failed. Debug session not found");
        let playback_overrides = debug_session_data.playback_overrides;

        for (idx, entry) in self.inner.read_many(oplog_index, count).await {
            let entry = playback_overrides
                .overrides
                .get(&idx)
                .cloned()
                .unwrap_or(entry);
            result.insert(idx, entry);
        }
        result
    }

    // The single source of the debug session's replay position: the replay cursor publishes its
    // committed advances here (speculative reads never fire this), so the session's
    // `current_oplog_index` always reflects entries that were really replayed. The reported index
    // is clamped to the recorded oplog end because switching to live mode publishes the replay
    // target, which can lie past the recorded end when playback runs into live mode.
    async fn on_replay_progress(&self, last_replayed_index: OplogIndex) {
        let clamped = last_replayed_index.min(self.inner.current_oplog_index().await);
        self.oplog_state
            .debug_session
            .update_oplog_index(&self.oplog_state.debug_session_id, clamped)
            .await;
    }

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner.download_raw_payload(payload_id, md5_hash).await
    }

    async fn switch_persistence_level(&self, mode: PersistenceLevel) {
        self.inner.switch_persistence_level(mode).await
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}
