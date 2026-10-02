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

//! A filesystem snapshot store for the tests of the executor.
//!
//! The store keeps the snapshots in memory. A clone shares the snapshots, so a test keeps its
//! store across a restart of the executor. A test can make saves fail, slow or held, make
//! restores fail, and count the calls.

use crate::filesystem_snapshot::{
    AgentSnapshots, ChangeDetection, FilesystemSnapshotStore, InMemorySnapshotStore, SnapshotInfo,
    SnapshotName, SnapshotStoreError,
};
use crate::services::agent_filesystem_snapshots::StoreSource;
use crate::services::golem_config::FilesystemSnapshotUploadConfig;
use async_trait::async_trait;
use golem_common::model::OwnedAgentId;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::watch;

/// The faults and the counts of a test store.
#[derive(Default)]
struct Faults {
    failing_saves: AtomicUsize,
    save_delay: Mutex<Duration>,
    failing_restore_names: Mutex<std::collections::HashSet<Box<str>>>,
    /// The directory of the tree of each save, in the order of the saves.
    trees: Mutex<Vec<Box<Path>>>,
    /// The time that the store adds to the time of each later save.
    clock_offset: Mutex<Duration>,
    /// The time of each saved name, with the offset of its save.
    times: Mutex<std::collections::HashMap<Box<str>, golem_common::model::Timestamp>>,
    saves: AtomicUsize,
    /// The hold of the next save, when a test asked for one.
    hold: Mutex<Option<SaveHold>>,
    restored: Mutex<Vec<Box<str>>>,
    /// The names of the restores that gave a tree, in the order of their ends.
    completed_restores: Mutex<Vec<Box<str>>>,
}

/// The store side of a held save: the save reports its name, then waits for the release.
struct SaveHold {
    started: watch::Sender<Option<Box<str>>>,
    released: watch::Receiver<bool>,
}

impl SaveHold {
    /// Reports `name` as the held save, and waits until the test releases the save or drops its
    /// [`HeldSave`].
    async fn hold(mut self, name: &SnapshotName) {
        self.started.send_replace(Some(Box::from(name.as_str())));
        let _ = self.released.wait_for(|released| *released).await;
    }
}

/// The test side of a held save, which [`TestFilesystemSnapshotStore::hold_next_save`] gives.
/// The save stays held until [`HeldSave::release`] or a drop of this value.
pub struct HeldSave {
    started: watch::Receiver<Option<Box<str>>>,
    released: watch::Sender<bool>,
}

impl HeldSave {
    /// The name of the held save, once it started.
    pub fn name(&self) -> Option<String> {
        self.started.borrow().as_deref().map(String::from)
    }

    /// Lets the held save go on.
    pub fn release(self) {
        self.released.send_replace(true);
    }
}

/// A filesystem snapshot store in memory, with faults and counts for tests.
#[derive(Clone)]
pub struct TestFilesystemSnapshotStore {
    inner: InMemorySnapshotStore,
    faults: Arc<Faults>,
}

impl Default for TestFilesystemSnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TestFilesystemSnapshotStore {
    /// Makes a store that holds no snapshot.
    pub fn new() -> Self {
        Self {
            inner: InMemorySnapshotStore::new(),
            faults: Arc::default(),
        }
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

    /// Holds the next save that starts: it reports its name, then waits before it stores
    /// anything, until the test releases it or drops the [`HeldSave`]. A save that is stopped
    /// while it is held stores nothing.
    pub fn hold_next_save(&self) -> HeldSave {
        let (started, started_receiver) = watch::channel(None);
        let (released, released_receiver) = watch::channel(false);
        *self
            .faults
            .hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(SaveHold {
            started,
            released: released_receiver,
        });
        HeldSave {
            started: started_receiver,
            released,
        }
    }

    /// The directory of the tree of each save that started, in the order of the saves.
    pub fn saved_trees(&self) -> Vec<std::path::PathBuf> {
        self.faults
            .trees
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|tree| tree.to_path_buf())
            .collect()
    }

    /// Moves the clock of the store forward by `by`: each later save gets a time that much later,
    /// as a save on an executor whose clock is ahead does.
    pub fn advance_clock(&self, by: Duration) {
        *self
            .faults
            .clock_offset
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += by;
    }

    /// Gives `info` with the time that the store gave the save of `name`.
    fn timed(&self, name: &SnapshotName, info: SnapshotInfo) -> SnapshotInfo {
        let times = self
            .faults
            .times
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        SnapshotInfo {
            created_at: times.get(name.as_str()).copied().unwrap_or(info.created_at),
            ..info
        }
    }

    /// Makes each restore of the snapshot `name` fail with an error that allows no retry.
    pub fn fail_restores_of(&self, name: &str) {
        self.faults
            .failing_restore_names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(Box::from(name));
    }

    /// The number of saves that started.
    pub fn save_count(&self) -> usize {
        self.faults.saves.load(Ordering::SeqCst)
    }

    /// The names that the restores asked for, in the order of the restores.
    pub fn restored_names(&self) -> Vec<String> {
        self.faults
            .restored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    /// The names of the restores that gave a tree, in the order of their ends.
    pub fn completed_restore_names(&self) -> Vec<String> {
        self.faults
            .completed_restores
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    /// The names of the snapshots of the agent, newest first.
    pub async fn snapshot_names(&self, agent: &OwnedAgentId) -> Vec<String> {
        self.inner
            .list(&AgentSnapshots::agent(agent))
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
            let _ = self
                .inner
                .delete(&AgentSnapshots::agent(agent), &[name])
                .await;
        }
    }

    /// Gives this store as the store of the service, with `uploads`, on any storage mode and
    /// whatever the configuration says.
    pub fn source(&self, uploads: FilesystemSnapshotUploadConfig) -> StoreSource {
        StoreSource::given(Arc::new(self.clone()), uploads)
    }
}

#[async_trait]
impl FilesystemSnapshotStore for TestFilesystemSnapshotStore {
    async fn save(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        self.faults.saves.fetch_add(1, Ordering::SeqCst);
        self.faults
            .trees
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::from(tree));
        let hold = self
            .faults
            .hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(hold) = hold {
            hold.hold(name).await;
        }
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
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            return Err(SnapshotStoreError::Storage {
                retryable: true,
                source: anyhow::anyhow!("an injected save failure"),
            });
        }
        let info = self.inner.save(agent, name, tree, parent).await?;
        let offset = *self
            .faults
            .clock_offset
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let millis = u64::try_from(offset.as_millis()).unwrap_or(u64::MAX);
        self.faults
            .times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                Box::from(name.as_str()),
                golem_common::model::Timestamp::from(
                    info.created_at.to_millis().saturating_add(millis),
                ),
            );
        Ok(self.timed(name, info))
    }

    async fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        self.faults
            .restored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::from(name.as_str()));
        if self
            .faults
            .failing_restore_names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(name.as_str())
        {
            return Err(SnapshotStoreError::Corrupt(anyhow::anyhow!(
                "an injected restore failure"
            )));
        }
        let restored = self.inner.restore(agent, name, into).await?;
        self.faults
            .completed_restores
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::from(name.as_str()));
        Ok(restored)
    }

    async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        Ok(self
            .inner
            .stat(agent, name)
            .await?
            .map(|info| self.timed(name, info)))
    }

    async fn list(
        &self,
        agent: &AgentSnapshots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        Ok(self
            .inner
            .list(agent)
            .await?
            .iter()
            .map(|(name, info)| (name.clone(), self.timed(name, *info)))
            .collect())
    }

    async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: &[SnapshotName],
    ) -> Result<(), SnapshotStoreError> {
        self.inner.delete(agent, names).await
    }

    async fn delete_all(&self, agent: &AgentSnapshots) -> Result<(), SnapshotStoreError> {
        self.inner.delete_all(agent).await
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
    ) -> Result<(), SnapshotStoreError> {
        self.inner.copy_all(from, to).await
    }
}
