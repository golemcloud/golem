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

//! A filesystem snapshot store and the service over it, for the tests of the executor.
//!
//! The store keeps the snapshots in memory. A clone shares the snapshots, so a test keeps its
//! store across a restart of the executor. A test can make saves fail or slow, make restores
//! fail, and count the calls.

use crate::filesystem_snapshot::{
    ChangeDetection, FilesystemSnapshotStore, InMemorySnapshotStore, SnapshotInfo, SnapshotName,
    SnapshotScope, SnapshotStoreError,
};
use crate::services::agent_filesystem_snapshots::{AgentFilesystemSnapshots, TokioClock};
use crate::services::golem_config::FilesystemSnapshotUploadConfig;
use crate::services::shutdown::Shutdown;
use async_trait::async_trait;
use golem_common::model::OwnedAgentId;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The faults and the counts of a test store.
#[derive(Default)]
struct Faults {
    failing_saves: AtomicUsize,
    save_delay: Mutex<Duration>,
    restores_fail: AtomicBool,
    saves: AtomicUsize,
    restored: Mutex<Vec<String>>,
    stats: AtomicUsize,
}

/// A filesystem snapshot store in memory, with faults and counts for tests.
#[derive(Clone, Default)]
pub struct TestFilesystemSnapshotStore {
    inner: InMemorySnapshotStore,
    faults: Arc<Faults>,
}

impl TestFilesystemSnapshotStore {
    /// Makes a store that holds no snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes the next `count` saves fail with an error that allows a retry.
    pub fn fail_next_saves(&self, count: usize) {
        self.faults.failing_saves.store(count, Ordering::SeqCst);
    }

    /// Makes each save wait for `delay` before it runs.
    pub fn set_save_delay(&self, delay: Duration) {
        *self
            .faults
            .save_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = delay;
    }

    /// Makes each restore fail with an error that allows no retry, or not.
    pub fn fail_restores(&self, fail: bool) {
        self.faults.restores_fail.store(fail, Ordering::SeqCst);
    }

    /// The number of saves that started.
    pub fn save_count(&self) -> usize {
        self.faults.saves.load(Ordering::SeqCst)
    }

    /// The number of restores that started.
    pub fn restore_count(&self) -> usize {
        self.restored_names().len()
    }

    /// The names that the restores asked for, in the order of the restores.
    pub fn restored_names(&self) -> Vec<String> {
        self.faults
            .restored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The number of stat calls.
    pub fn stat_count(&self) -> usize {
        self.faults.stats.load(Ordering::SeqCst)
    }

    /// The names of the snapshots of the agent, newest first.
    pub async fn snapshot_names(&self, agent: &OwnedAgentId) -> Vec<String> {
        self.inner
            .list(&SnapshotScope::agent(agent))
            .await
            .map(|listing| {
                listing
                    .iter()
                    .map(|(name, _)| name.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Deletes the snapshot `name` of the agent, as a loss of storage does.
    pub async fn lose(&self, agent: &OwnedAgentId, name: &str) {
        if let Ok(name) = SnapshotName::new(name) {
            let _ = self.inner.delete(&SnapshotScope::agent(agent), &name).await;
        }
    }

    /// Gives the service over this store with `uploads`. The service stops when the executor
    /// shuts down.
    pub fn service(
        &self,
        uploads: FilesystemSnapshotUploadConfig,
        shutdown: &Shutdown,
    ) -> Arc<AgentFilesystemSnapshots> {
        let snapshots = Arc::new(AgentFilesystemSnapshots::enabled(
            Arc::new(self.clone()),
            uploads,
            Arc::new(TokioClock),
            Arc::new(crate::services::agent_filesystem_snapshots::UnlimitedRoom),
            shutdown.token(),
        ));
        let stopping = Arc::clone(&snapshots);
        let token = shutdown.token();
        shutdown.spawn(async move {
            token.cancelled().await;
            stopping.shut_down().await;
        });
        snapshots
    }
}

#[async_trait]
impl FilesystemSnapshotStore for TestFilesystemSnapshotStore {
    async fn save(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        self.faults.saves.fetch_add(1, Ordering::SeqCst);
        let delay = *self
            .faults
            .save_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let failing = self
            .faults
            .failing_saves
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            return Err(SnapshotStoreError::Storage {
                retryable: true,
                source: anyhow::anyhow!("an injected save failure"),
            });
        }
        self.inner.save(scope, name, tree, parent).await
    }

    async fn restore(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        self.faults
            .restored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(name.as_str().to_string());
        if self.faults.restores_fail.load(Ordering::SeqCst) {
            return Err(SnapshotStoreError::Corrupt(anyhow::anyhow!(
                "an injected restore failure"
            )));
        }
        self.inner.restore(scope, name, into).await
    }

    async fn stat(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        self.faults.stats.fetch_add(1, Ordering::SeqCst);
        self.inner.stat(scope, name).await
    }

    async fn list(
        &self,
        scope: &SnapshotScope,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        self.inner.list(scope).await
    }

    async fn delete(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError> {
        self.inner.delete(scope, name).await
    }

    async fn delete_scope(&self, scope: &SnapshotScope) -> Result<(), SnapshotStoreError> {
        self.inner.delete_scope(scope).await
    }

    async fn copy_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        self.inner.copy_scope(from, to).await
    }
}
