// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use bytes::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_worker_executor::services::oplog::{CommitLevel, Oplog};
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
    async fn add(&self, _entry: OplogEntry) -> OplogIndex {
        OplogIndex::NONE
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

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        Ok(OplogPayload::Inline(data.to_vec()))
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        self.inner.download_payload(payload).await
    }
}
