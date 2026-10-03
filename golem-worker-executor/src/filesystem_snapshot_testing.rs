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
//! The store keeps the snapshots in memory. A clone shares the snapshots, so a test keeps its store
//! across a restart of the executor; a shutdown stops only the clone that was shut down. A test can
//! make saves fail, slow or held, make restores and copies fail, hold a copy, and count the
//! calls.

use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, Failed, FilesystemSnapshotStore,
    InMemorySnapshotStore, ReadError, RestoreFailure, RunSlots, SaveError, SnapshotInfo,
    SnapshotName, Unlimited, Withdrawal,
};
use crate::services::agent_filesystem_snapshots::StoreSource;
use crate::services::golem_config::FilesystemSnapshotUploadConfig;
use async_trait::async_trait;
use golem_common::model::{AgentFingerprint, OwnedAgentId};
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
    copies: AtomicUsize,
    failing_copies: AtomicUsize,
    /// The hold of the next copy, when a test asked for one.
    copy_hold: Mutex<Option<SaveHold>>,
    /// The hold of the next delete of all snapshots of an agent, when a test asked for one.
    delete_all_hold: Mutex<Option<SaveHold>>,
    /// The number of saves and listings that returned.
    returned: AtomicUsize,
    /// The value of `returned` after the save of each name returned.
    saved_at: Mutex<std::collections::HashMap<Box<str>, usize>>,
    /// The value of `returned` after the last listing returned.
    listed_at: AtomicUsize,
}

/// The store side of a held save: the save reports its name, then waits for the release.
struct SaveHold {
    started: watch::Sender<Option<Box<str>>>,
    released: watch::Receiver<bool>,
}

impl SaveHold {
    /// Reports `name` as the held call, and waits until the test releases the call or drops its
    /// [`HeldSave`].
    async fn hold(mut self, name: &str) {
        self.started.send_replace(Some(Box::from(name)));
        let _ = self.released.wait_for(|released| *released).await;
    }
}

/// The test side of a held save, copy or delete of all snapshots, which
/// [`TestFilesystemSnapshotStore::hold_next_save`], [`TestFilesystemSnapshotStore::hold_next_copy`]
/// and [`TestFilesystemSnapshotStore::hold_next_delete_all`] give. The call stays held until
/// [`HeldSave::release`] or a drop of this value.
pub struct HeldSave {
    started: watch::Receiver<Option<Box<str>>>,
    released: watch::Sender<bool>,
}

impl HeldSave {
    /// The name of the held save, the target of the held copy, or the agent of the held delete of
    /// all snapshots, once it started.
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

    /// Makes the next `count` saves fail, as a save does whose storage fails in each run.
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

    /// Holds the next copy that starts: it reports its target, then waits before it copies
    /// anything, until the test releases it or drops the [`HeldSave`].
    pub fn hold_next_copy(&self) -> HeldSave {
        let (started, started_receiver) = watch::channel(None);
        let (released, released_receiver) = watch::channel(false);
        *self
            .faults
            .copy_hold
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

    /// Holds the next delete of all snapshots of an agent that starts: it reports its agent, then
    /// waits before it deletes anything, until the test releases it or drops the [`HeldSave`].
    pub fn hold_next_delete_all(&self) -> HeldSave {
        let (started, started_receiver) = watch::channel(None);
        let (released, released_receiver) = watch::channel(false);
        *self
            .faults
            .delete_all_hold
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

    /// Makes the next `count` copies fail, as a copy does whose storage fails in each run.
    pub fn fail_next_copies(&self, count: usize) {
        self.faults.failing_copies.store(count, Ordering::SeqCst);
    }

    /// The number of copies that started.
    pub fn copy_count(&self) -> usize {
        self.faults.copies.load(Ordering::SeqCst)
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

    /// Makes each restore of the snapshot `name` give `Corrupt`, which a new try does not change.
    pub fn fail_restores_of(&self, name: &str) {
        self.faults
            .failing_restore_names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(Box::from(name));
    }

    /// Whether a listing returned after the save of `name` returned, as the retention of the job
    /// that saved `name` makes one before its deletes.
    pub fn listed_after_save_of(&self, name: &str) -> bool {
        let saved_at = self
            .faults
            .saved_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .copied();
        saved_at.is_some_and(|saved_at| self.faults.listed_at.load(Ordering::SeqCst) > saved_at)
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

    /// The names of the snapshots of the incarnation `fingerprint` of the agent, newest first.
    pub async fn snapshot_names(
        &self,
        agent: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Vec<String> {
        self.inner
            .list(&AgentSnapshots::agent(agent, fingerprint), &Unlimited)
            .await
            .map(|listing| {
                listing
                    .iter()
                    .map(|(name, _)| name.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Deletes the snapshot `name` of the incarnation `fingerprint` of the agent, as a loss of
    /// storage does.
    pub async fn lose(&self, agent: &OwnedAgentId, fingerprint: AgentFingerprint, name: &str) {
        if let Ok(name) = SnapshotName::new(name) {
            let _ = self
                .inner
                .delete(
                    &AgentSnapshots::agent(agent, fingerprint),
                    &[name],
                    &Unlimited,
                )
                .await;
        }
    }

    /// Ends an injected failed save as the store ends a save whose storage fails in each run,
    /// after its first run under a slot: the wait after the failed run, and a second run under a
    /// slot. A withdrawal at a take, or a shutdown, gives `Stopped`, and a deadline after the
    /// first run gives `Failed`.
    async fn failed_runs(&self, slots: &dyn RunSlots) -> SaveError {
        let failed = || SaveError::Failed(Failed::new(anyhow::anyhow!("an injected save failure")));
        slots.waiting_after_failure();
        match slots.take(false).await {
            Ok(slot) => {
                drop(slot);
                failed()
            }
            Err(Withdrawal::Deadline) => failed(),
            Err(Withdrawal::Stopped) => SaveError::Stopped(Withdrawal::Stopped),
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
        cancel: &tokio_util::sync::CancellationToken,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, SaveError> {
        self.faults.saves.fetch_add(1, Ordering::SeqCst);
        self.faults
            .trees
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::from(tree));
        // The first run takes its slot before it does anything, as a run of the store does, so a
        // held or slow save holds its slot.
        let slot = self.inner.slot(slots).await.map_err(SaveError::Stopped)?;
        let hold = self
            .faults
            .hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(hold) = hold {
            hold.hold(name.as_str()).await;
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
            drop(slot);
            return Err(self.failed_runs(slots).await);
        }
        let info = self
            .inner
            .save(agent, name, tree, parent, cancel, &Unlimited)
            .await;
        drop(slot);
        let info = info?;
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
        let returned = self.faults.returned.fetch_add(1, Ordering::SeqCst) + 1;
        self.faults
            .saved_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(Box::from(name.as_str()), returned);
        Ok(self.timed(name, info))
    }

    async fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, RestoreFailure> {
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
            return Err(RestoreFailure::Corrupt(anyhow::anyhow!(
                "an injected restore failure"
            )));
        }
        let restored = self.inner.restore(agent, name, into, slots).await?;
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
    ) -> Result<Option<SnapshotInfo>, ReadError> {
        Ok(self
            .inner
            .stat(agent, name)
            .await?
            .map(|info| self.timed(name, info)))
    }

    async fn list(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError> {
        let listed = self
            .inner
            .list(agent, slots)
            .await?
            .iter()
            .map(|(name, info)| (name.clone(), self.timed(name, *info)))
            .collect();
        let returned = self.faults.returned.fetch_add(1, Ordering::SeqCst) + 1;
        self.faults.listed_at.store(returned, Ordering::SeqCst);
        Ok(listed)
    }

    async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: &[SnapshotName],
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.inner.delete(agent, names, slots).await
    }

    async fn delete_all(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        let hold = self
            .faults
            .delete_all_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(hold) = hold {
            hold.hold(&format!("{agent:?}")).await;
        }
        self.inner.delete_all(agent, slots).await
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.faults.copies.fetch_add(1, Ordering::SeqCst);
        let hold = self
            .faults
            .copy_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(hold) = hold {
            hold.hold(&format!("{to:?}")).await;
        }
        let failing = self
            .faults
            .failing_copies
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            return Err(CallError::Failed(Failed::new(anyhow::anyhow!(
                "an injected copy failure"
            ))));
        }
        self.inner.copy_all(from, to, slots).await
    }

    async fn shut_down(&self) {
        self.inner.shut_down().await
    }
}
