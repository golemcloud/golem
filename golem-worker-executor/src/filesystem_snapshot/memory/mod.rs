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
//! Each call is one run under one slot of its limiter, because the memory does not fail. A delete
//! of all snapshots of an agent waits for the calls of the agent that began before it. The
//! snapshots of an agent are one immutable slice. Each change makes a new slice from the old
//! one with a pure function. The store puts the new slice in place under a lock that no await
//! holds. A restore keeps the tree that it read under the lock, so a delete at the same time
//! cannot change it. Two agents whose snapshots a copy made share trees that never change.

mod tree;

#[cfg(test)]
mod tests;

use super::agent_work::{AgentWorks, begin_operation, drain_agent};
use super::clock::{Clock, SystemClock};
use super::{
    AgentSnapshots, CallError, ChangeDetection, Failed, FilesystemSnapshotStore, ReadError,
    RestoreFailure, RunSlots, SaveError, Slot, SnapshotInfo, SnapshotName, Withdrawal,
    newest_first, snapshot_time,
};
use async_trait::async_trait;
use golem_common::model::Timestamp;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio_util::sync::CancellationToken;
use tree::{TreeEntry, read_tree, tree_info, write_tree};

/// A filesystem snapshot store that keeps each snapshot in the memory of the process.
///
/// A clone of the store is one more store over the same snapshots, as another executor has: it
/// shares the snapshots and not the shutdown. On a platform other than unix, a save of a tree with
/// a symlink gives `Source` and publishes nothing.
pub(crate) struct InMemorySnapshotStore {
    agents: Arc<Mutex<HashMap<AgentSnapshots, Arc<[Stored]>>>>,
    clock: Arc<dyn Clock>,
    /// Cancelled when the store shuts down.
    shut_down: CancellationToken,
    /// The work of each incarnation, which a delete of all snapshots waits for.
    works: AgentWorks,
}

impl Clone for InMemorySnapshotStore {
    fn clone(&self) -> Self {
        Self {
            agents: Arc::clone(&self.agents),
            clock: Arc::clone(&self.clock),
            shut_down: CancellationToken::new(),
            works: AgentWorks::default(),
        }
    }
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
            shut_down: CancellationToken::new(),
            works: AgentWorks::default(),
        }
    }

    /// Takes the one slot of a call, or gives why the call stops.
    pub(crate) async fn slot(&self, slots: &dyn RunSlots) -> Result<Slot, Withdrawal> {
        if self.shut_down.is_cancelled() {
            return Err(Withdrawal::Stopped);
        }
        slots.take(true).await
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

/// Gives the snapshots with `snapshot` added, or `NameInUse` when a snapshot has its name.
fn with_saved(snapshots: &[Stored], snapshot: Stored) -> Result<Arc<[Stored]>, SaveError> {
    match found(snapshots, &snapshot.name) {
        Some(_) => Err(SaveError::NameInUse),
        None => Ok(snapshots
            .iter()
            .cloned()
            .chain(std::iter::once(snapshot))
            .collect()),
    }
}

/// Gives the snapshots without the snapshots with the names `names`.
fn without(snapshots: &[Stored], names: &[SnapshotName]) -> Arc<[Stored]> {
    snapshots
        .iter()
        .filter(|stored| !names.contains(&stored.name))
        .cloned()
        .collect()
}

/// Runs blocking work on a thread of the blocking pool, so the async runtime is not blocked.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Failed> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Failed::new(anyhow::Error::new(error)))
}

#[async_trait]
impl FilesystemSnapshotStore for InMemorySnapshotStore {
    async fn save(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        _parent: Option<(&SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, SaveError> {
        let _work = begin_operation(&self.works, agent);
        let _slot = self.slot(slots).await.map_err(SaveError::Stopped)?;
        let snapshots = self.snapshots_of(agent);
        if found(&snapshots, name).is_some() {
            return Err(SaveError::NameInUse);
        }
        let newest = newest(&snapshots);

        let root = tree.to_path_buf();
        let tree = blocking(move || read_tree(&root))
            .await
            .map_err(SaveError::Failed)?
            .map_err(SaveError::Source)?;
        let info = tree_info(&tree, snapshot_time(self.clock.now(), newest));

        // A cancel before the publish publishes nothing.
        if cancel.is_cancelled() {
            return Err(SaveError::Stopped(Withdrawal::Stopped));
        }
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
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, RestoreFailure> {
        let _work = begin_operation(&self.works, agent);
        let _slot = self.slot(slots).await.map_err(RestoreFailure::Stopped)?;
        let snapshots = self.snapshots_of(agent);
        let stored = found(&snapshots, name)
            .cloned()
            .ok_or(RestoreFailure::NotFound)?;

        let into = into.to_path_buf();
        let tree = stored.tree;
        blocking(move || write_tree(&tree, &into))
            .await
            .map_err(RestoreFailure::Failed)?
            .map_err(RestoreFailure::Destination)?;
        Ok(stored.info)
    }

    async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, ReadError> {
        let _work = begin_operation(&self.works, agent);
        if self.shut_down.is_cancelled() {
            return Err(ReadError::Stopped);
        }
        Ok(found(&self.snapshots_of(agent), name).map(|stored| stored.info))
    }

    async fn list(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError> {
        let _work = begin_operation(&self.works, agent);
        let _slot = self.slot(slots).await.map_err(CallError::Stopped)?;
        Ok(newest_first(
            self.snapshots_of(agent)
                .iter()
                .map(|stored| (stored.name.clone(), stored.info)),
        ))
    }

    async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: &[SnapshotName],
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        let _work = begin_operation(&self.works, agent);
        let _slot = self.slot(slots).await.map_err(CallError::Stopped)?;
        let mut agents = self.agents();
        if let Some(snapshots) = agents.get(agent) {
            let kept = without(snapshots, names);
            agents.insert(agent.clone(), kept);
        }
        Ok(())
    }

    async fn delete_all(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        // The call waits for the calls of the agent that began before it, and holds no slot
        // while it waits. A shutdown ends the wait.
        let _work = tokio::select! {
            biased;
            () = self.shut_down.cancelled() => begin_operation(&self.works, agent),
            work = drain_agent(&self.works, agent) => work,
        };
        let _slot = self.slot(slots).await.map_err(CallError::Stopped)?;
        self.agents().remove(agent);
        Ok(())
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        let _works = (
            begin_operation(&self.works, from),
            begin_operation(&self.works, to),
        );
        let _slot = self.slot(slots).await.map_err(CallError::Stopped)?;
        // The snapshots never change, so the two agents can hold the same slice and stay
        // independent. A change for one agent puts a new slice in place for that agent only. When
        // `from` has no snapshots, `to` has none either.
        let mut agents = self.agents();
        match agents.get(from).cloned() {
            Some(snapshots) => {
                agents.insert(to.clone(), snapshots);
            }
            None => {
                agents.remove(to);
            }
        }
        Ok(())
    }

    async fn shut_down(&self) {
        self.shut_down.cancel();
    }
}

/// The times that a test store gives its saves: each new name gets a time ten minutes after the
/// name before it, so that retention sees times far apart.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct SpacedTimes(Mutex<HashMap<Box<str>, Timestamp>>);

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
            .entry(Box::from(name.as_str()))
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
