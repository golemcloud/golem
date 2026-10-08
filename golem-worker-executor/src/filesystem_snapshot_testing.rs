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
//! make saves fail, slow or held, make restores fail for good or for a number of tries, make
//! copies fail, hold a copy or a restore, and count the calls.

use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, Failed, FilesystemSnapshotStore,
    InMemorySnapshotStore, ReadError, RestoreFailure, RunSlots, SaveError, SnapshotInfo,
    SnapshotName, Unlimited, WithdrawnCall, run_delay, withdrawn_call,
};
use crate::services::agent_filesystem_snapshots::StoreSource;
use crate::services::golem_config::FilesystemSnapshotUploadConfig;
use async_trait::async_trait;
use golem_common::model::{AgentFingerprint, OwnedAgentId, RetryConfig};
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
    /// The number of next restores that fail as a restore does whose storage fails.
    failing_restores: AtomicUsize,
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
    /// The hold of the next restore, when a test asked for one.
    restore_hold: Mutex<Option<SaveHold>>,
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

/// The test side of a held save, copy, delete of all snapshots or restore, which
/// [`TestFilesystemSnapshotStore::hold_next_save`], [`TestFilesystemSnapshotStore::hold_next_copy`],
/// [`TestFilesystemSnapshotStore::hold_next_delete_all`] and
/// [`TestFilesystemSnapshotStore::hold_next_restore`] give. The call stays held until
/// [`HeldSave::release`] or a drop of this value.
pub struct HeldSave {
    started: watch::Receiver<Option<Box<str>>>,
    released: watch::Sender<bool>,
}

impl HeldSave {
    /// The name of the held save or restore, the target of the held copy, or the agent of the held
    /// delete of all snapshots, once it started.
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

/// The failure of a test whose store got a write into an agent after a delete of all its
/// snapshots.
const WRITE_AFTER_DELETE_ALL: &str =
    "no write goes into an agent after a delete of all its snapshots";

/// Runs the test `test` with a new store, and then checks that no write went into an agent after a
/// delete of all its snapshots, also a write of a background job. It is the only way that a test
/// gets a store, so no test skips the check. It gives the error of the test when the test failed,
/// with the writes that the check found, and otherwise the failure of the check.
pub async fn with_snapshot_store<T, F, Fut>(test: F) -> anyhow::Result<T>
where
    F: FnOnce(TestFilesystemSnapshotStore) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let store = TestFilesystemSnapshotStore::new();
    let tested = test(store.clone()).await;
    let misused = store.writes_after_delete_all();
    match (tested, misused.is_empty()) {
        (Ok(value), true) => Ok(value),
        (Ok(_), false) => Err(anyhow::anyhow!("{WRITE_AFTER_DELETE_ALL}: {misused:?}")),
        (Err(error), true) => Err(error),
        (Err(error), false) => Err(error.context(format!("{WRITE_AFTER_DELETE_ALL}: {misused:?}"))),
    }
}

impl TestFilesystemSnapshotStore {
    /// Makes a store that holds no snapshot. A test gets one only through
    /// [`with_snapshot_store`].
    fn new() -> Self {
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

    /// Holds the next restore that starts: it reports its name, then waits before it restores
    /// anything, until the test releases it or drops the [`HeldSave`].
    pub fn hold_next_restore(&self) -> HeldSave {
        let (started, started_receiver) = watch::channel(None);
        let (released, released_receiver) = watch::channel(false);
        *self
            .faults
            .restore_hold
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

    /// Makes the next `count` restores fail as a restore does whose storage fails, which a new try
    /// can change.
    pub fn fail_next_restores(&self, count: usize) {
        self.faults.failing_restores.store(count, Ordering::SeqCst);
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
    /// after its first run under a slot. [`run_delay`] decides each wait and whether a run
    /// follows: the script has two runs and no wait between them, so a test spends no time in the
    /// wait. [`withdrawn_call`] decides what a withdrawal gives: a withdrawal in the wait or at a
    /// take, or a shutdown, gives `Stopped`, and a deadline after the first run gives `Failed`.
    async fn failed_runs(&self, slots: &dyn RunSlots) -> SaveError {
        let script = RetryConfig {
            max_attempts: 2,
            min_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            max_jitter_factor: None,
        };
        let failed = || SaveError::Failed(Failed::new(anyhow::anyhow!("an injected save failure")));
        let withdrawn = |cause| match withdrawn_call(cause, Some(())) {
            WithdrawnCall::Failed(()) => failed(),
            WithdrawnCall::Stopped(cause) => SaveError::Stopped(cause),
        };
        let ended = futures::stream::unfold(Some(1u32), |state| {
            let (script, failed, withdrawn) = (&script, &failed, &withdrawn);
            async move {
                let failed_runs = state?;
                let Some(delay) = run_delay(script, failed_runs, 0.0) else {
                    return Some((Some(failed()), None));
                };
                slots.waiting_after_failure();
                let waited = tokio::select! {
                    biased;
                    cause = slots.withdrawn() => Err(cause),
                    () = tokio::time::sleep(delay) => Ok(()),
                };
                let next = match waited {
                    Ok(()) => slots.take(false).await.map(drop),
                    Err(cause) => Err(cause),
                };
                Some(match next {
                    Ok(()) => (None, Some(failed_runs + 1)),
                    Err(cause) => (Some(withdrawn(cause)), None),
                })
            }
        });
        futures::StreamExt::next(&mut std::pin::pin!(futures::StreamExt::filter_map(
            ended,
            |ended| async move { ended }
        )))
        .await
        .unwrap_or_else(failed)
    }

    /// Each write that a caller made into an agent after a delete of all its snapshots, which the
    /// store refused. A caller never makes one, so a test can assert that this is empty.
    pub fn writes_after_delete_all(&self) -> Vec<String> {
        self.inner.writes_after_delete_all()
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
        let hold = self
            .faults
            .restore_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(hold) = hold {
            hold.hold(name.as_str()).await;
        }
        let failing = self
            .faults
            .failing_restores
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            return Err(RestoreFailure::Failed(Failed::new(anyhow::anyhow!(
                "an injected restore failure of the storage"
            ))));
        }
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

#[cfg(test)]
mod tests {
    use super::TestFilesystemSnapshotStore;
    use crate::filesystem_snapshot::{
        AgentSnapshots, FilesystemSnapshotStore, RunSlots, SaveError, Slot, SnapshotName,
        Withdrawal,
    };
    use futures::future::BoxFuture;
    use golem_common::model::{AgentFingerprint, OwnedAgentId};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};

    /// Slots that are always free, that count the takes and the waits after a failed run, and that
    /// withdraw the call at the take number `withdraw_at`.
    struct CountedSlots {
        takes: AtomicUsize,
        waits: AtomicUsize,
        withdraw_at: usize,
    }

    /// Whether the take number `take`, counted from 1, of slots that withdraw the call at the take
    /// number `withdraw_at` gives a slot.
    fn grants(take: usize, withdraw_at: usize) -> Result<(), Withdrawal> {
        if take < withdraw_at {
            Ok(())
        } else {
            Err(Withdrawal::Stopped)
        }
    }

    impl RunSlots for CountedSlots {
        fn take(&self, _immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
            let take = self.takes.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(std::future::ready(
                grants(take, self.withdraw_at).map(|()| Slot::new(())),
            ))
        }

        fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
            Box::pin(std::future::pending())
        }

        fn waiting_after_failure(&self) {
            self.waits.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn counted_slots_grant_each_take_before_the_withdrawing_one() {
        assert_eq!(
            [grants(1, 3), grants(2, 3), grants(3, 3), grants(4, 3)],
            [
                Ok(()),
                Ok(()),
                Err(Withdrawal::Stopped),
                Err(Withdrawal::Stopped)
            ]
        );
    }

    #[test]
    #[timeout("10s")]
    async fn an_injected_failed_save_with_free_slots_fails_after_its_two_runs() {
        let store = TestFilesystemSnapshotStore::new();
        store.fail_next_saves(1);
        let slots = CountedSlots {
            takes: AtomicUsize::new(0),
            waits: AtomicUsize::new(0),
            withdraw_at: 10,
        };

        let saved = store
            .save(
                &AgentSnapshots::agent(
                    &OwnedAgentId::new(
                        golem_common::model::environment::EnvironmentId::new(),
                        &golem_common::model::AgentId {
                            component_id: golem_common::model::component::ComponentId::new(),
                            agent_id: "failed-runs".to_string(),
                        },
                    ),
                    AgentFingerprint(uuid::Uuid::new_v4()),
                ),
                &SnapshotName::new("p-00000000-0000-4000-8000-000000000001").unwrap(),
                Path::new("/no-tree"),
                None,
                crate::filesystem_snapshot::never_cancelled(),
                &slots,
            )
            .await;

        assert!(matches!(saved, Err(SaveError::Failed(_))), "{saved:?}");
        assert_eq!(
            (
                slots.takes.load(Ordering::SeqCst),
                slots.waits.load(Ordering::SeqCst)
            ),
            (2, 1)
        );
    }
}
