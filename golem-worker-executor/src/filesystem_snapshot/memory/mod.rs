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

//! A filesystem snapshot store that keeps each snapshot in the memory of the process.
//!
//! The snapshots of an agent are one immutable slice. Each change makes a new slice from the old
//! one with a pure function. The store puts the new slice in place under a lock that no await
//! holds. A restore keeps the tree that it read under the lock, so a delete at the same time
//! cannot change it. Two agents whose snapshots a copy made share trees that never change.

mod tree;

#[cfg(test)]
mod tests;

use super::clock::{Clock, SystemClock};
use super::{
    AgentSnapshots, ChangeDetection, FilesystemSnapshotStore, SnapshotInfo, SnapshotName,
    SnapshotStoreError, newest_first, snapshot_time,
};
use async_trait::async_trait;
use golem_common::model::Timestamp;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tree::{TreeEntry, read_tree, tree_info, write_tree};

/// A filesystem snapshot store that keeps each snapshot in the memory of the process.
///
/// A clone of the store is one more store over the same snapshots, as another executor has. On a
/// platform other than unix, a save of a tree with a symlink gives `Source` and publishes nothing.
#[derive(Clone)]
pub(crate) struct InMemorySnapshotStore {
    agents: Arc<Mutex<HashMap<AgentSnapshots, Arc<[Stored]>>>>,
    clock: Arc<dyn Clock>,
}

impl Default for InMemorySnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

/// One snapshot of an agent.
#[derive(Clone)]
struct Stored {
    name: SnapshotName,
    info: SnapshotInfo,
    tree: Arc<[TreeEntry]>,
}

impl InMemorySnapshotStore {
    /// Makes a store that holds no snapshot.
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    /// Makes a store that holds no snapshot and reads the time from the clock.
    fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            agents: Arc::default(),
            clock,
        }
    }

    /// Gives the snapshots of each agent. No lock is held across an await, so a panic cannot
    /// leave a change half made. The store therefore uses the map of a poisoned lock as it is.
    fn agents(&self) -> MutexGuard<'_, HashMap<AgentSnapshots, Arc<[Stored]>>> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives the snapshots of the agent. An agent without snapshots gives an empty slice.
    fn snapshots_of(&self, agent: &AgentSnapshots) -> Arc<[Stored]> {
        self.agents().get(agent).cloned().unwrap_or_default()
    }
}

/// Gives the snapshot with the name.
fn found<'a>(snapshots: &'a [Stored], name: &SnapshotName) -> Option<&'a Stored> {
    snapshots.iter().find(|stored| stored.name == *name)
}

/// Gives the time of the newest snapshot.
fn newest(snapshots: &[Stored]) -> Option<Timestamp> {
    snapshots.iter().map(|stored| stored.info.created_at).max()
}

/// Gives the snapshots with `snapshot` added, or `AlreadyExists` when a snapshot has its name.
fn with_saved(snapshots: &[Stored], snapshot: Stored) -> Result<Arc<[Stored]>, SnapshotStoreError> {
    match found(snapshots, &snapshot.name) {
        Some(_) => Err(SnapshotStoreError::AlreadyExists),
        None => Ok(snapshots
            .iter()
            .cloned()
            .chain(std::iter::once(snapshot))
            .collect()),
    }
}

/// Gives the snapshots without the snapshot with the name.
fn without(snapshots: &[Stored], name: &SnapshotName) -> Arc<[Stored]> {
    snapshots
        .iter()
        .filter(|stored| stored.name != *name)
        .cloned()
        .collect()
}

/// Runs blocking work on a thread of the blocking pool, so the async runtime is not blocked.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, SnapshotStoreError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| SnapshotStoreError::Storage {
            retryable: false,
            source: anyhow::Error::new(error),
        })
}

#[async_trait]
impl FilesystemSnapshotStore for InMemorySnapshotStore {
    async fn save(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        _parent: Option<(&SnapshotName, ChangeDetection)>,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let snapshots = self.snapshots_of(agent);
        if found(&snapshots, name).is_some() {
            return Err(SnapshotStoreError::AlreadyExists);
        }
        let newest = newest(&snapshots);

        let root = tree.to_path_buf();
        let tree = blocking(move || read_tree(&root))
            .await?
            .map_err(SnapshotStoreError::Source)?;
        let info = tree_info(&tree, snapshot_time(self.clock.now(), newest));

        // A save of the same name can finish during the read, so the check runs again in the
        // step that publishes the snapshot.
        let mut agents = self.agents();
        let current = agents.get(agent).cloned().unwrap_or_default();
        let saved = with_saved(
            &current,
            Stored {
                name: name.clone(),
                info,
                tree,
            },
        )?;
        agents.insert(agent.clone(), saved);
        Ok(info)
    }

    async fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let snapshots = self.snapshots_of(agent);
        let stored = found(&snapshots, name)
            .cloned()
            .ok_or(SnapshotStoreError::NotFound)?;

        let into = into.to_path_buf();
        let tree = stored.tree;
        blocking(move || write_tree(&tree, &into))
            .await?
            .map_err(SnapshotStoreError::Destination)?;
        Ok(stored.info)
    }

    async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        Ok(found(&self.snapshots_of(agent), name).map(|stored| stored.info))
    }

    async fn list(
        &self,
        agent: &AgentSnapshots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        Ok(newest_first(
            self.snapshots_of(agent)
                .iter()
                .map(|stored| (stored.name.clone(), stored.info)),
        ))
    }

    async fn delete(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError> {
        let mut agents = self.agents();
        if let Some(snapshots) = agents.get(agent) {
            let kept = without(snapshots, name);
            agents.insert(agent.clone(), kept);
        }
        Ok(())
    }

    async fn delete_all(&self, agent: &AgentSnapshots) -> Result<(), SnapshotStoreError> {
        self.agents().remove(agent);
        Ok(())
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
    ) -> Result<(), SnapshotStoreError> {
        // The snapshots never change, so the two agents can hold the same slice and stay
        // independent. A change for one agent puts a new slice in place for that agent only.
        let mut agents = self.agents();
        if let Some(snapshots) = agents.get(from).cloned() {
            agents.insert(to.clone(), snapshots);
        }
        Ok(())
    }
}

/// The times that a test store gives its saves: each new name gets a time ten minutes after the
/// name before it, so that retention sees times far apart.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct SpacedTimes(Mutex<HashMap<String, Timestamp>>);

#[cfg(test)]
impl SpacedTimes {
    /// The time of the first save.
    pub(crate) const FIRST_MILLIS: u64 = 1_800_000_000_000;
    /// The time between two saves.
    const SPACING_MILLIS: u64 = 10 * 60 * 1000;

    /// Gives `info` with the time of `name`, and gives a new name the next time.
    pub(crate) fn timed(&self, name: &SnapshotName, info: SnapshotInfo) -> SnapshotInfo {
        let mut times = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = times.len() as u64;
        let created_at = *times
            .entry(name.as_str().to_string())
            .or_insert_with(|| Timestamp::from(Self::FIRST_MILLIS + count * Self::SPACING_MILLIS));
        SnapshotInfo { created_at, ..info }
    }

    /// Gives `info` with the time of `name` when it has one, and gives no time to a new name.
    pub(crate) fn known(&self, name: &SnapshotName, info: SnapshotInfo) -> SnapshotInfo {
        let times = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        SnapshotInfo {
            created_at: times.get(name.as_str()).copied().unwrap_or(info.created_at),
            ..info
        }
    }
}
