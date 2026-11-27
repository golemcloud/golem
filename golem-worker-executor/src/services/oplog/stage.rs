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

use super::CreateStagedError;
use crate::metrics::oplog::record_stage_reservation_cleanup_failed;
use crate::services::shutdown::Shutdown;
use crate::storage::indexed::{IndexedStorage, IndexedStorageError, IndexedStorageNamespace};
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentMode;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;
use tracing::warn;
use uuid::Uuid;

/// Tracks both admitted stages and their cancellation cleanup, including the handoff between them.
#[derive(Debug, Default)]
pub struct StageCleanupTracker {
    tasks: TaskTracker,
    cutoff: CancellationToken,
}

impl StageCleanupTracker {
    /// Closes admission, then waits for active stages and cleanup tasks within the shutdown budget.
    /// A cutoff is observable, but cannot prove that a database request did not commit.
    pub async fn drain(&self, grace: Duration) -> bool {
        self.tasks.close();
        if tokio::time::timeout(grace, self.tasks.wait()).await.is_ok() {
            return true;
        }
        warn!(
            remaining = self.tasks.len(),
            ?grace,
            "Stage reservation cleanup exceeded shutdown deadline"
        );
        record_stage_reservation_cleanup_failed();
        self.cutoff.cancel();
        false
    }

    pub(crate) fn track_shutdown(self: &Arc<Self>, shutdown: &Shutdown) {
        let tracker = self.clone();
        let token = shutdown.token();
        shutdown.spawn(async move {
            token.cancelled().await;
            tracker.drain(Duration::from_secs(5)).await;
        });
    }
}

struct Cleanup {
    indexed: Arc<dyn IndexedStorage + Send + Sync>,
    id: OwnedAgentId,
    mode: AgentMode,
    stage: Uuid,
    token: i64,
}

impl Cleanup {
    async fn discard(&self) -> Result<(), IndexedStorageError> {
        self.indexed
            .discard_stage(
                "oplog",
                "discard_staged",
                IndexedStorageNamespace::StagedOpLog {
                    owned_agent_id: self.id.clone(),
                    agent_mode: self.mode,
                },
                &format!("{}#{}", self.id.storage_key(), self.stage),
                Some(self.token),
            )
            .await
    }

    async fn retry(&self) -> Result<(), IndexedStorageError> {
        let mut delay = Duration::from_millis(25);
        for attempt in 0..3 {
            match self.discard().await {
                Ok(()) => return Ok(()),
                Err(error)
                    if attempt < 2
                        && (error.is_retriable()
                            || matches!(error, IndexedStorageError::Indeterminate(_))) =>
                {
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }
}

/// Owns a stage independently of its writer. Armed before reservation, retained through
/// publication, and disarmed only after publication or an exact-token discard succeeds.
pub struct StageCleanupGuard {
    cleanup: Option<Cleanup>,
    tracker: Arc<StageCleanupTracker>,
    runtime: Handle,
    _active: TaskTrackerToken,
}

impl std::fmt::Debug for StageCleanupGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageCleanupGuard")
            .field("armed", &self.cleanup.is_some())
            .finish()
    }
}

impl StageCleanupGuard {
    pub(crate) fn new(
        indexed: Arc<dyn IndexedStorage + Send + Sync>,
        id: OwnedAgentId,
        mode: AgentMode,
        stage: Uuid,
        tracker: Arc<StageCleanupTracker>,
    ) -> Result<Self, CreateStagedError> {
        let active = tracker.tasks.token();
        // Acquire the lifetime before checking closure, so a concurrent drain either waits
        // for us or makes us refuse before any reservation can be written.
        if tracker.tasks.is_closed() {
            return Err(CreateStagedError::Retryable(
                "Stage admission is shutting down".into(),
            ));
        }
        Ok(Self {
            cleanup: Some(Cleanup {
                indexed,
                id,
                mode,
                stage,
                token: (stage.as_u64_pair().1 & i64::MAX as u64) as i64,
            }),
            _active: active,
            tracker,
            runtime: Handle::current(),
        })
    }

    pub(crate) fn token(&self) -> i64 {
        self.cleanup.as_ref().expect("stage is still armed").token
    }

    pub fn disarm(&mut self) {
        self.cleanup = None;
    }

    pub async fn discard(&mut self) -> Result<(), String> {
        if let Some(cleanup) = &self.cleanup {
            cleanup.discard().await.map_err(|error| error.to_string())?;
        }
        self.disarm();
        Ok(())
    }
}

impl Drop for StageCleanupGuard {
    fn drop(&mut self) {
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };
        let cutoff = self.tracker.cutoff.clone();
        // Register the task before `_active` drops, so drain never sees an empty handoff.
        self.tracker.tasks.spawn_on(
            async move {
                tokio::select! {
                    _ = cutoff.cancelled() => {}
                    result = cleanup.retry() => {
                        if let Err(error) = result {
                            warn!(agent_id = %cleanup.id, stage_id = %cleanup.stage, error = %error,
                                "Failed to clean up stage reservation");
                            record_stage_reservation_cleanup_failed();
                        }
                    }
                }
            },
            &self.runtime,
        );
    }
}
