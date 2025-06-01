use crate::debug_session::{DebugSessionId, DebugSessions};
use async_trait::async_trait;
use axum::body::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::WorkerMetadata;
use golem_worker_executor::model::ExecutionStatus;
use golem_worker_executor::services::oplog::{CommitLevel, Oplog};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

pub struct DebugOplog {
    pub inner: Arc<dyn Oplog + Send + Sync>,
    pub oplog_state: DebugOplogState,
}

impl DebugOplog {
    pub fn new(
        inner: Arc<dyn Oplog + Send + Sync>,
        debug_session_id: DebugSessionId,
        debug_session: Arc<dyn DebugSessions + Send + Sync>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        initial_worker_metadata: WorkerMetadata,
    ) -> Self {
        let oplog_state = DebugOplogState {
            debug_session_id,
            debug_session,
            _execution_status: execution_status,
            _initial_worker_metadata: initial_worker_metadata,
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
    _execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    _initial_worker_metadata: WorkerMetadata,
}

#[async_trait]
impl Oplog for DebugOplog {
    // We don't allow debugging session to add anything into oplog
    // which internally can get committed.
    async fn add(&self, _entry: OplogEntry) {}

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) {}

    // There is no need to commit anything to the indexed storage
    async fn commit(&self, _level: CommitLevel) {}

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

    async fn upload_payload(&self, _data: &[u8]) -> Result<OplogPayload, String> {
        // in a debugging session we don't need to upload anything
        Err("Workers in debug cannot upload any new data to oplog".to_string())
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        self.inner.download_payload(payload).await
    }
}
