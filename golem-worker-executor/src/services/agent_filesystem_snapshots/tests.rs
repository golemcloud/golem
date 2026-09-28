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

use super::*;
use crate::filesystem_snapshot::InMemorySnapshotStore;
use crate::services::agent_filesystem::RestoreTree;
use futures::StreamExt as _;
use golem_common::model::RetryConfig;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicUsize;
use test_r::{test, timeout};
use tokio::sync::Notify;

/// A store over the in-memory store that a test can make fail, hold and count.
#[derive(Default)]
struct ScriptedStore {
    memory: InMemorySnapshotStore,
    /// The number of saves that still fail with a retryable storage error.
    failing_saves: AtomicUsize,
    /// Whether a failing save publishes the tree before it fails, as a late PUT does.
    publish_before_failing: std::sync::atomic::AtomicBool,
    /// Whether each save fails with an error that allows no retry.
    saves_fail_for_good: std::sync::atomic::AtomicBool,
    /// Whether each scope delete fails with a retryable storage error.
    scope_deletes_fail: std::sync::atomic::AtomicBool,
    /// When set, each save waits for it before it runs.
    save_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, each restore waits for it before it runs.
    restore_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, each delete of a name waits for it before it runs.
    delete_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, each restore fails.
    restores_fail: std::sync::atomic::AtomicBool,
    /// The names that the saves were given, with the content of the tree of each call.
    saved: Mutex<Vec<(String, Vec<u8>)>>,
    /// The parent of each save.
    parents: Mutex<Vec<Option<(String, ChangeDetection)>>>,
    deletes: Mutex<Vec<String>>,
    scope_deletes: AtomicUsize,
    restores_now: AtomicUsize,
    most_restores_at_once: AtomicUsize,
    store_calls_now: AtomicUsize,
    most_store_calls_at_once: AtomicUsize,
    /// The time of each saved name. Each save is ten minutes after the one before it, so
    /// retention sees times that are far apart.
    times: Mutex<HashMap<String, golem_common::model::Timestamp>>,
}

/// The time of the first save of a scripted store.
const FIRST_SAVE_MILLIS: u64 = 1_800_000_000_000;
/// The time between two saves of a scripted store.
const SAVE_SPACING_MILLIS: u64 = 10 * 60 * 1000;

/// A gate that holds each caller until the test opens it.
struct Gate {
    reached: tokio::sync::watch::Sender<usize>,
    opened: tokio::sync::watch::Sender<bool>,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            reached: tokio::sync::watch::channel(0).0,
            opened: tokio::sync::watch::channel(false).0,
        }
    }
}

impl Gate {
    async fn pass(&self) {
        let mut opened = self.opened.subscribe();
        self.reached.send_modify(|reached| *reached += 1);
        let _ = opened.wait_for(|opened| *opened).await;
    }

    async fn wait_reached(&self, count: usize) {
        let _ = self
            .reached
            .subscribe()
            .wait_for(|reached| *reached >= count)
            .await;
    }

    fn open(&self) {
        self.opened.send_replace(true);
    }
}

impl ScriptedStore {
    /// Gives `info` with the time of the save of `name`, which the store gives to a new name.
    fn timed(&self, name: &SnapshotName, info: SnapshotInfo) -> SnapshotInfo {
        let mut times = self.times.lock().unwrap();
        let count = times.len() as u64;
        let created_at = *times.entry(name.as_str().to_string()).or_insert_with(|| {
            golem_common::model::Timestamp::from(FIRST_SAVE_MILLIS + count * SAVE_SPACING_MILLIS)
        });
        SnapshotInfo { created_at, ..info }
    }

    fn saved_names(&self) -> Vec<String> {
        self.saved
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn enter(&self) -> CallCount<'_> {
        let now = self.store_calls_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_store_calls_at_once
            .fetch_max(now, Ordering::SeqCst);
        CallCount(&self.store_calls_now)
    }
}

/// Counts one store call that saves or deletes while it lives.
struct CallCount<'a>(&'a AtomicUsize);

impl Drop for CallCount<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn retryable(text: &str) -> SnapshotStoreError {
    SnapshotStoreError::Storage {
        retryable: true,
        source: anyhow::anyhow!(text.to_string()),
    }
}

#[async_trait]
impl FilesystemSnapshotStore for ScriptedStore {
    async fn save(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let gate = self.save_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let _count = self.enter();
        let content = std::fs::read(tree.join("content")).unwrap_or_default();
        self.saved
            .lock()
            .unwrap()
            .push((name.as_str().to_string(), content));
        self.parents
            .lock()
            .unwrap()
            .push(parent.map(|(name, detection)| (name.as_str().to_string(), detection)));
        if self.saves_fail_for_good.load(Ordering::SeqCst) {
            return Err(SnapshotStoreError::Source(std::io::Error::other(
                "the tree cannot be read",
            )));
        }
        let failing = self
            .failing_saves
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            if self.publish_before_failing.load(Ordering::SeqCst) {
                let _ = self.memory.save(scope, name, tree, parent).await;
            }
            return Err(retryable("the publish failed"));
        }
        let info = self.memory.save(scope, name, tree, parent).await?;
        Ok(self.timed(name, info))
    }

    async fn restore(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let now = self.restores_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_restores_at_once.fetch_max(now, Ordering::SeqCst);
        let gate = self.restore_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let result = if self.restores_fail.load(Ordering::SeqCst) {
            Err(SnapshotStoreError::Corrupt(anyhow::anyhow!("corrupt")))
        } else {
            self.memory.restore(scope, name, into).await
        };
        self.restores_now.fetch_sub(1, Ordering::SeqCst);
        result
    }

    async fn stat(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        self.memory.stat(scope, name).await
    }

    async fn list(
        &self,
        scope: &SnapshotScope,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        let times = self.times.lock().unwrap().clone();
        Ok(self
            .memory
            .list(scope)
            .await?
            .iter()
            .map(|(name, info)| {
                let created_at = times.get(name.as_str()).copied().unwrap_or(info.created_at);
                (
                    name.clone(),
                    SnapshotInfo {
                        created_at,
                        ..*info
                    },
                )
            })
            .collect())
    }

    async fn delete(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError> {
        let gate = self.delete_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let _count = self.enter();
        tokio::task::yield_now().await;
        self.deletes.lock().unwrap().push(name.as_str().to_string());
        self.memory.delete(scope, name).await
    }

    async fn delete_scope(&self, scope: &SnapshotScope) -> Result<(), SnapshotStoreError> {
        let _count = self.enter();
        self.scope_deletes.fetch_add(1, Ordering::SeqCst);
        if self.scope_deletes_fail.load(Ordering::SeqCst) {
            return Err(retryable("the scope delete failed"));
        }
        self.memory.delete_scope(scope).await
    }

    async fn copy_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        self.memory.copy_scope(from, to).await
    }
}

/// A clock whose sleeps complete when the test releases them. It records each sleep.
#[derive(Default)]
struct ManualClock {
    sleeps: Mutex<Vec<Duration>>,
    /// Whether each sleep completes at once.
    immediate: std::sync::atomic::AtomicBool,
    released: Arc<Notify>,
}

impl ManualClock {
    fn immediate() -> Arc<Self> {
        let clock = Self::default();
        clock.immediate.store(true, Ordering::SeqCst);
        Arc::new(clock)
    }

    fn held() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn release(&self) {
        self.released.notify_waiters();
    }

    fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl SnapshotClock for ManualClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.sleeps.lock().unwrap().push(duration);
        if self.immediate.load(Ordering::SeqCst) {
            Box::pin(std::future::ready(()))
        } else {
            let released = Arc::clone(&self.released);
            Box::pin(async move { released.notified().await })
        }
    }
}

/// A confirmer that answers with a programmed outcome and records each name.
struct ScriptedConfirmer {
    outcome: Mutex<ConfirmOutcome>,
    names: Mutex<Vec<FilesystemSnapshotName>>,
    /// When set, each confirmation waits for it before it answers.
    gate: Option<Arc<Gate>>,
}

impl ScriptedConfirmer {
    fn answering(outcome: ConfirmOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(outcome),
            names: Mutex::default(),
            gate: None,
        })
    }

    fn names(&self) -> Vec<FilesystemSnapshotName> {
        self.names.lock().unwrap().clone()
    }
}

#[async_trait]
impl ConfirmSnapshot for ScriptedConfirmer {
    async fn confirm(&self, name: &FilesystemSnapshotName) -> ConfirmOutcome {
        self.names.lock().unwrap().push(name.clone());
        if let Some(gate) = &self.gate {
            gate.pass().await;
        }
        *self.outcome.lock().unwrap()
    }
}

fn confirmer(confirm: &Arc<ScriptedConfirmer>) -> SnapshotConfirmer {
    SnapshotConfirmer::new(Arc::clone(confirm) as Arc<dyn ConfirmSnapshot>)
}

/// A capture in a temporary directory with one file of `content`. Its discard is counted.
struct TestCapture {
    directory: tempfile::TempDir,
    discarded: Arc<AtomicUsize>,
    /// When set, the discard waits for it.
    discard_gate: Option<Arc<Gate>>,
}

impl TestCapture {
    fn with(content: &[u8], discarded: &Arc<AtomicUsize>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("content"), content).unwrap();
        Self {
            directory,
            discarded: Arc::clone(discarded),
            discard_gate: None,
        }
    }
}

impl CapturedTree for TestCapture {
    fn directory(&self) -> &Path {
        self.directory.path()
    }

    fn discard(self) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            if let Some(gate) = &self.discard_gate {
                gate.pass().await;
            }
            self.discarded.fetch_add(1, Ordering::SeqCst);
        })
    }
}

/// A volume without room.
struct NoRoom;

#[async_trait]
impl VolumeRoom for NoRoom {
    async fn has_room(&self) -> bool {
        false
    }
}

fn retry(max_attempts: u32) -> RetryConfig {
    RetryConfig {
        max_attempts,
        min_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(120),
        multiplier: 4.0,
        max_jitter_factor: None,
    }
}

fn settings(
    max_uploads: usize,
    max_restores: usize,
    max_attempts: u32,
) -> FilesystemSnapshotUploadConfig {
    FilesystemSnapshotUploadConfig::new(
        max_uploads,
        max_restores,
        Duration::from_secs(60),
        Duration::from_secs(5),
        Duration::from_secs(5),
        2,
        2,
        retry(max_attempts),
    )
    .unwrap()
}

/// A service over `store` with `settings` and `clock`, and room on the volume.
fn service(
    store: &Arc<ScriptedStore>,
    settings: FilesystemSnapshotUploadConfig,
    clock: Arc<ManualClock>,
) -> AgentFilesystemSnapshots {
    AgentFilesystemSnapshots::enabled(
        Arc::clone(store) as Arc<dyn FilesystemSnapshotStore>,
        settings,
        clock,
        Arc::new(UnlimitedRoom),
        CancellationToken::new(),
    )
}

fn scope(name: &str) -> SnapshotScope {
    SnapshotScope::agent(&OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        },
    ))
}

/// Waits until `condition` holds, and fails the test after five seconds.
async fn eventually(condition: impl Fn() -> bool) {
    let held = futures::stream::repeat(())
        .then(|()| tokio::time::sleep(Duration::from_millis(5)))
        .filter(|()| std::future::ready(condition()));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), std::pin::pin!(held).next())
            .await
            .is_ok(),
        "the condition did not hold in time"
    );
}

/// Waits until the job of `scope` ended, and fails the test after five seconds.
async fn ended(snapshots: &AgentFilesystemSnapshots, scope: &SnapshotScope) {
    let job = snapshots
        .enabled
        .as_ref()
        .and_then(|enabled| enabled.jobs_of_scope().jobs.get(scope).cloned());
    if let Some(job) = job {
        assert!(
            tokio::time::timeout(Duration::from_secs(5), job.ended.cancelled())
                .await
                .is_ok(),
            "the job did not end in time"
        );
    }
}

#[test]
#[timeout("10s")]
async fn each_admission_makes_a_new_name_with_the_prefix_of_its_kind() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("names");

    let names = futures::stream::iter([
        SnapshotKind::Periodic,
        SnapshotKind::Periodic,
        SnapshotKind::Update,
        SnapshotKind::Update,
    ])
    .then(|kind| {
        let snapshots = &snapshots;
        let scope = &scope;
        async move { snapshots.admit(scope, kind).await.unwrap().name().clone() }
    })
    .collect::<Vec<_>>()
    .await;

    let prefixes = names
        .iter()
        .map(|name| &name.as_str()[..2])
        .collect::<Vec<_>>();
    let unique = names.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(prefixes, vec!["p-", "p-", "u-", "u-"]);
    assert_eq!(unique.len(), 4);
}

#[test]
#[timeout("10s")]
async fn a_second_admission_of_a_scope_gets_upload_in_flight_until_the_first_ends() {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("in-flight");
    let other = self::scope("other");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let while_admitted = snapshots.admit(&scope, SnapshotKind::Update).await.err();
    let other_scope = snapshots
        .admit(&other, SnapshotKind::Periodic)
        .await
        .is_ok();
    admission.submit(
        TestCapture::with(b"one", &discarded),
        None,
        confirmer(&confirm),
    );
    gate.wait_reached(1).await;
    let while_uploading = snapshots.admit(&scope, SnapshotKind::Periodic).await.err();
    gate.open();
    ended(&snapshots, &scope).await;
    let after = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .is_ok();

    assert_eq!(
        (while_admitted, other_scope, while_uploading, after),
        (
            Some(SnapshotSkip::UploadInFlight),
            true,
            Some(SnapshotSkip::UploadInFlight),
            true
        )
    );
}

#[test]
#[timeout("10s")]
async fn a_dropped_admission_frees_its_scope_and_writes_nothing() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("dropped");

    drop(
        snapshots
            .admit(&scope, SnapshotKind::Periodic)
            .await
            .unwrap(),
    );
    let again = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .is_ok();

    assert!(again);
    assert!(store.saved_names().is_empty());
}

#[test]
#[timeout("10s")]
async fn a_disabled_service_admits_nothing_and_restores_nothing() {
    let snapshots = AgentFilesystemSnapshots::disabled();
    let scope = scope("disabled");

    let admitted = snapshots.admit(&scope, SnapshotKind::Periodic).await.err();
    let restored = snapshots
        .restore(&scope, &FilesystemSnapshotName::periodic())
        .err();

    assert_eq!(
        (admitted, restored),
        (Some(SnapshotSkip::Disabled), Some(SnapshotsDisabled))
    );
}

#[test]
#[timeout("10s")]
async fn a_volume_without_room_gives_volume_under_pressure() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = AgentFilesystemSnapshots::enabled(
        Arc::clone(&store) as Arc<dyn FilesystemSnapshotStore>,
        settings(4, 4, 1),
        ManualClock::immediate(),
        Arc::new(NoRoom),
        CancellationToken::new(),
    );

    let admitted = snapshots
        .admit(&scope("full"), SnapshotKind::Periodic)
        .await
        .err();

    assert_eq!(admitted, Some(SnapshotSkip::VolumeUnderPressure));
}

#[test]
#[timeout("10s")]
async fn a_job_saves_discards_and_calls_the_confirmer_once_with_its_own_name() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("confirmed");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    ended(&snapshots, &scope).await;

    let listed = store
        .memory
        .list(&scope)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(confirm.names(), vec![name.clone()]);
    assert_eq!(listed, vec![name.as_str().to_string()]);
    assert_eq!(discarded.load(Ordering::SeqCst), 1);
}

#[test]
#[timeout("10s")]
async fn after_the_retry_budget_the_job_discards_the_capture_and_calls_no_confirmer() {
    let store = Arc::new(ScriptedStore::default());
    store.failing_saves.store(usize::MAX, Ordering::SeqCst);
    let clock = ManualClock::immediate();
    let snapshots = service(&store, settings(4, 4, 3), Arc::clone(&clock));
    let scope = scope("budget");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    ended(&snapshots, &scope).await;

    assert_eq!(store.saved_names().len(), 3);
    assert_eq!(
        clock.sleeps(),
        vec![Duration::from_secs(2), Duration::from_secs(8)]
    );
    assert!(confirm.names().is_empty());
    assert_eq!(discarded.load(Ordering::SeqCst), 1);
}

#[test]
#[timeout("10s")]
async fn an_error_that_allows_no_retry_ends_the_job_after_one_attempt() {
    let store = Arc::new(ScriptedStore::default());
    store.saves_fail_for_good.store(true, Ordering::SeqCst);
    let snapshots = service(&store, settings(4, 4, 5), ManualClock::immediate());
    let scope = scope("for-good");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    ended(&snapshots, &scope).await;

    assert_eq!(store.saved_names().len(), 1);
    assert!(confirm.names().is_empty());
}

/// Uploads three older periodic snapshots that the confirmer confirms, then one that it answers
/// with `outcome`. Gives the deletes that the last job made, its name, whether the store holds it,
/// and whether the store holds each older one.
async fn after_older_uploads(outcome: ConfirmOutcome) -> (Vec<String>, String, bool, Vec<bool>) {
    let store = Arc::new(ScriptedStore::default());
    let mut settings = settings(4, 4, 1);
    settings = FilesystemSnapshotUploadConfig::new(
        settings.max_concurrent_uploads().get(),
        settings.max_concurrent_restores().get(),
        settings.confirmation_wait(),
        settings.store_check_limit(),
        settings.capture_wait(),
        3,
        2,
        settings.upload_retry().clone(),
    )
    .unwrap();
    let snapshots = service(&store, settings, ManualClock::immediate());
    let scope = scope("dropped-confirmation");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirmed = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let older = futures::stream::iter(0..3)
        .then(|index| {
            let snapshots = &snapshots;
            let scope = &scope;
            let discarded = &discarded;
            let confirmed = &confirmed;
            async move {
                let admission = snapshots
                    .admit(scope, SnapshotKind::Periodic)
                    .await
                    .unwrap();
                let name = admission.name().clone();
                admission.submit(
                    TestCapture::with(format!("older-{index}").as_bytes(), discarded),
                    None,
                    confirmer(confirmed),
                );
                ended(snapshots, scope).await;
                name
            }
        })
        .collect::<Vec<_>>()
        .await;
    let deletes_before = store.deletes.lock().unwrap().len();
    let confirm = ScriptedConfirmer::answering(outcome);

    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(
        TestCapture::with(b"newest", &discarded),
        None,
        confirmer(&confirm),
    );
    ended(&snapshots, &scope).await;

    let deletes = store.deletes.lock().unwrap()[deletes_before..].to_vec();
    let held = store
        .memory
        .stat(&scope, &store_name(&name).unwrap())
        .await
        .unwrap()
        .is_some();
    let older_held = futures::stream::iter(older.iter())
        .then(|older| {
            let store = &store;
            let scope = &scope;
            async move {
                store
                    .memory
                    .stat(scope, &store_name(older).unwrap())
                    .await
                    .unwrap()
                    .is_some()
            }
        })
        .collect::<Vec<_>>()
        .await;
    (deletes, name.as_str().to_string(), held, older_held)
}

#[test]
#[timeout("10s")]
async fn superseded_deletes_the_snapshot_and_runs_no_retention() {
    let (deletes, name, held, older_held) = after_older_uploads(ConfirmOutcome::Superseded).await;

    assert_eq!(deletes, vec![name]);
    assert!(!held);
    assert_eq!(older_held, vec![true, true, true]);
}

#[test]
#[timeout("10s")]
async fn deferred_keeps_the_snapshot_and_runs_no_retention() {
    let (deletes, _, held, older_held) = after_older_uploads(ConfirmOutcome::Deferred).await;

    assert!(deletes.is_empty());
    assert!(held);
    assert_eq!(older_held, vec![true, true, true]);
}

#[test]
#[timeout("10s")]
async fn confirmed_runs_retention_after_the_confirmation() {
    let (deletes, _, held, older_held) = after_older_uploads(ConfirmOutcome::Confirmed).await;

    assert_eq!(deletes.len(), 1);
    assert!(held);
    assert_eq!(older_held, vec![false, true, true]);
}

#[test]
#[timeout("10s")]
async fn a_confirmed_periodic_upload_keeps_the_newest_by_kind() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("retention");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let upload = |kind: SnapshotKind, index: usize| {
        let snapshots = &snapshots;
        let scope = &scope;
        let discarded = &discarded;
        let confirm = &confirm;
        async move {
            let admission = snapshots.admit(scope, kind).await.unwrap();
            let name = admission.name().clone();
            match kind {
                SnapshotKind::Periodic => admission.submit(
                    TestCapture::with(format!("{index}").as_bytes(), discarded),
                    None,
                    confirmer(confirm),
                ),
                SnapshotKind::Update => {
                    admission
                        .upload_now(
                            TestCapture::with(format!("{index}").as_bytes(), discarded),
                            std::future::pending(),
                        )
                        .await
                        .unwrap();
                }
            }
            ended(snapshots, scope).await;
            name
        }
    };

    let updates = futures::stream::iter(0..3)
        .then(|index| upload(SnapshotKind::Update, index))
        .collect::<Vec<_>>()
        .await;
    let periodic = futures::stream::iter(10..14)
        .then(|index| upload(SnapshotKind::Periodic, index))
        .collect::<Vec<_>>()
        .await;

    let kept = store
        .memory
        .list(&scope)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = periodic[2..]
        .iter()
        .chain(&updates)
        .map(|name| name.as_str().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(kept, expected);
}

fn info_at(minutes: u64) -> SnapshotInfo {
    SnapshotInfo {
        created_at: golem_common::model::Timestamp::from(FIRST_SAVE_MILLIS + minutes * 60 * 1000),
        files: 0,
        bytes: 0,
    }
}

fn names(names: &[SnapshotName]) -> Vec<String> {
    names.iter().map(|name| name.as_str().to_string()).collect()
}

#[test]
fn retention_keeps_the_own_snapshot_and_the_newest_older_ones_of_its_kind() {
    let listing = [
        ("p-own", 100),
        ("p-newer", 130),
        ("p-close", 99),
        ("p-3", 90),
        ("p-2", 80),
        ("p-1", 70),
        ("u-2", 60),
        ("u-1", 50),
        ("x-1", 40),
    ]
    .map(|(name, minutes)| (SnapshotName::new(name).unwrap(), info_at(minutes)));
    let own = SnapshotName::new("p-own").unwrap();

    let periodic = names(&retention::victims(&listing, &own, &info_at(100), 2, None));
    let only_own = names(&retention::victims(&listing, &own, &info_at(100), 1, None));

    assert_eq!(periodic, vec!["p-2", "p-1"]);
    assert_eq!(only_own, vec!["p-3", "p-2", "p-1"]);
}

#[test]
fn retention_counts_update_snapshots_apart_from_periodic_ones() {
    let listing = [("u-own", 100), ("u-2", 90), ("u-1", 80), ("p-1", 70)]
        .map(|(name, minutes)| (SnapshotName::new(name).unwrap(), info_at(minutes)));
    let own = SnapshotName::new("u-own").unwrap();

    let victims = names(&retention::victims(&listing, &own, &info_at(100), 2, None));

    assert_eq!(victims, vec!["u-1"]);
}

#[test]
fn retention_never_deletes_the_kept_snapshot() {
    let listing = [("u-own", 100), ("u-2", 90), ("u-1", 80), ("u-baseline", 70)]
        .map(|(name, minutes)| (SnapshotName::new(name).unwrap(), info_at(minutes)));
    let own = SnapshotName::new("u-own").unwrap();
    let baseline = SnapshotName::new("u-baseline").unwrap();

    let victims = names(&retention::victims(
        &listing,
        &own,
        &info_at(100),
        2,
        Some(&baseline),
    ));

    assert_eq!(victims, vec!["u-1"]);
}

#[test]
#[timeout("10s")]
async fn an_admission_during_a_scope_delete_gets_scope_deleting_until_the_delete_ends() {
    let store = Arc::new(ScriptedStore::default());
    store.scope_deletes_fail.store(true, Ordering::SeqCst);
    let clock = ManualClock::held();
    let snapshots = service(&store, settings(4, 4, 2), Arc::clone(&clock));
    let scope = scope("deleting");

    snapshots.forget_scope(&scope);
    eventually(|| store.scope_deletes.load(Ordering::SeqCst) == 1).await;
    let while_deleting = snapshots.admit(&scope, SnapshotKind::Periodic).await.err();
    eventually(|| !clock.sleeps().is_empty()).await;
    clock.release();
    eventually(|| store.scope_deletes.load(Ordering::SeqCst) == 2).await;
    let after = {
        let admitted = snapshots.admit(&scope, SnapshotKind::Periodic).await;
        let retried = futures::stream::repeat(())
            .then(|()| tokio::time::sleep(Duration::from_millis(5)))
            .then(|()| snapshots.admit(&scope, SnapshotKind::Periodic))
            .filter(|admitted| std::future::ready(admitted.is_ok()));
        match admitted {
            Ok(_) => true,
            Err(_) => tokio::time::timeout(Duration::from_secs(5), std::pin::pin!(retried).next())
                .await
                .is_ok(),
        }
    };

    assert_eq!(while_deleting, Some(SnapshotSkip::ScopeDeleting));
    assert!(after);
}

#[test]
#[timeout("10s")]
async fn the_decision_of_a_job_carries_its_outcome_before_the_job_ends() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("decided");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let job = snapshots
        .enabled
        .as_ref()
        .and_then(|enabled| enabled.jobs_of_scope().jobs.get(&scope).cloned())
        .unwrap();
    let mut decided = job.decided.subscribe();

    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    let decision = *decided
        .wait_for(|decision| decision.is_some())
        .await
        .unwrap();
    ended(&snapshots, &scope).await;

    assert_eq!(
        decision,
        Some(JobDecision::Confirmed(ConfirmOutcome::Deferred))
    );
    assert!(job.holds_slot.load(Ordering::SeqCst));
}

#[test]
#[timeout("10s")]
async fn the_wait_for_an_upload_of_a_scope_returns_at_the_limit_and_when_the_job_ends() {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let clock = ManualClock::held();
    let snapshots = service(&store, settings(4, 4, 1), Arc::clone(&clock));
    let scope = scope("pending");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    gate.wait_reached(1).await;

    let job_runs = || {
        snapshots
            .enabled
            .as_ref()
            .is_some_and(|enabled| enabled.jobs_of_scope().jobs.contains_key(&scope))
    };
    let waiting = snapshots.wait_for_upload_of_scope(&scope);
    let at_limit = {
        let mut waiting = std::pin::pin!(waiting);
        let not_yet = tokio::time::timeout(Duration::from_millis(50), &mut waiting)
            .await
            .is_err();
        eventually(|| clock.sleeps().contains(&Duration::from_secs(60))).await;
        clock.release();
        waiting.await;
        (not_yet, job_runs())
    };
    let second = snapshots.wait_for_upload_of_scope(&scope);
    gate.open();
    second.await;
    let after_end = !job_runs();
    tokio::time::timeout(
        Duration::from_millis(50),
        snapshots.wait_for_upload_of_scope(&self::scope("no-job")),
    )
    .await
    .unwrap();

    assert_eq!((at_limit, after_end), ((true, true), true));
}

#[test]
#[timeout("10s")]
async fn forget_scope_cancels_the_job_and_deletes_the_scope_after_the_job_ended() {
    let store = Arc::new(ScriptedStore::default());
    let save_gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&save_gate));
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("forget-scope");
    let discarded = Arc::new(AtomicUsize::new(0));
    let discard_gate = Arc::new(Gate::default());
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture {
            discard_gate: Some(Arc::clone(&discard_gate)),
            ..TestCapture::with(b"tree", &discarded)
        },
        None,
        confirmer(&confirm),
    );
    save_gate.wait_reached(1).await;

    snapshots.forget_scope(&scope);
    discard_gate.wait_reached(1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let while_the_job_runs = store.scope_deletes.load(Ordering::SeqCst);
    discard_gate.open();
    ended(&snapshots, &scope).await;
    eventually(|| store.scope_deletes.load(Ordering::SeqCst) == 1).await;

    assert_eq!(while_the_job_runs, 0);
    assert!(confirm.names().is_empty());
    assert!(store.deletes.lock().unwrap().is_empty());
    assert_eq!(discarded.load(Ordering::SeqCst), 1);
}

#[test]
#[timeout("10s")]
async fn a_cancelled_confirmation_deletes_nothing() {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    let confirm = Arc::new(ScriptedConfirmer {
        outcome: Mutex::new(ConfirmOutcome::Confirmed),
        names: Mutex::default(),
        gate: Some(Arc::clone(&gate)),
    });
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("cancelled-confirmation");
    let discarded = Arc::new(AtomicUsize::new(0));
    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    gate.wait_reached(1).await;

    snapshots.forget_scope(&scope);
    ended(&snapshots, &scope).await;
    eventually(|| store.scope_deletes.load(Ordering::SeqCst) == 1).await;

    assert!(
        !store
            .deletes
            .lock()
            .unwrap()
            .contains(&name.as_str().to_string())
    );
    assert!(store.memory.list(&scope).await.unwrap().is_empty());
}

#[test]
#[timeout("10s")]
async fn a_store_that_fails_every_scope_delete_leaves_the_service_running() {
    let store = Arc::new(ScriptedStore::default());
    store.scope_deletes_fail.store(true, Ordering::SeqCst);
    let clock = ManualClock::immediate();
    let snapshots = service(&store, settings(4, 4, 3), Arc::clone(&clock));
    let scope = scope("failing-delete");

    snapshots.forget_scope(&scope);
    eventually(|| store.scope_deletes.load(Ordering::SeqCst) == 3).await;
    let discarded = Arc::new(AtomicUsize::new(0));
    let saved = snapshots
        .admit(&scope, SnapshotKind::Update)
        .await
        .unwrap()
        .upload_now(
            TestCapture::with(b"after", &discarded),
            std::future::pending(),
        )
        .await
        .is_ok();

    assert!(saved);
}

#[test]
#[timeout("10s")]
async fn each_delete_holds_a_slot_of_the_uploads() {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let snapshots = service(&store, settings(1, 4, 1), ManualClock::immediate());
    let uploading = scope("uploading");
    let forgotten = scope("forgotten");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let admission = snapshots
        .admit(&uploading, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    gate.wait_reached(1).await;

    snapshots.forget(
        &forgotten,
        Box::new([
            FilesystemSnapshotName::periodic(),
            FilesystemSnapshotName::periodic(),
        ]),
    );
    snapshots.forget_scope(&self::scope("deleted"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = (
        store.deletes.lock().unwrap().len(),
        store.scope_deletes.load(Ordering::SeqCst),
    );
    gate.open();
    ended(&snapshots, &uploading).await;
    eventually(|| {
        store.deletes.lock().unwrap().len() == 2 && store.scope_deletes.load(Ordering::SeqCst) == 1
    })
    .await;

    assert_eq!(before, (0, 0));
    assert_eq!(store.most_store_calls_at_once.load(Ordering::SeqCst), 1);
}

#[test]
#[timeout("10s")]
async fn at_most_the_configured_number_of_restores_run_at_the_same_time() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 2, 1), ManualClock::immediate());
    let scope = scope("restores");
    let discarded = Arc::new(AtomicUsize::new(0));
    let name = {
        let admission = snapshots.admit(&scope, SnapshotKind::Update).await.unwrap();
        let name = admission.name().clone();
        admission
            .upload_now(
                TestCapture::with(b"tree", &discarded),
                std::future::pending(),
            )
            .await
            .unwrap();
        name
    };
    let gate = Arc::new(Gate::default());
    *store.restore_gate.lock().unwrap() = Some(Arc::clone(&gate));
    store.restores_fail.store(true, Ordering::SeqCst);
    let targets = (0..5)
        .map(|_| tempfile::tempdir().unwrap())
        .collect::<Vec<_>>();

    let restores = targets
        .iter()
        .map(|target| {
            let restore = snapshots.restore(&scope, &name).unwrap();
            let into = target.path().to_path_buf();
            tokio::spawn(async move { restore.restore(&into).await })
        })
        .collect::<Vec<_>>();
    gate.wait_reached(2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let while_held = store.restores_now.load(Ordering::SeqCst);
    gate.open();
    let results = futures::future::join_all(restores).await;

    assert_eq!(while_held, 2);
    assert_eq!(store.most_restores_at_once.load(Ordering::SeqCst), 2);
    assert!(
        results
            .into_iter()
            .all(|result| result.unwrap().is_err_and(|error| !error.retryable))
    );
}

#[test]
#[timeout("10s")]
async fn a_restore_gives_the_saved_tree() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 2, 1), ManualClock::immediate());
    let scope = scope("restore");
    let discarded = Arc::new(AtomicUsize::new(0));
    let admission = snapshots.admit(&scope, SnapshotKind::Update).await.unwrap();
    let name = admission.name().clone();
    admission
        .upload_now(
            TestCapture::with(b"restored", &discarded),
            std::future::pending(),
        )
        .await
        .unwrap();
    let into = tempfile::tempdir().unwrap();

    snapshots
        .restore(&scope, &name)
        .unwrap()
        .restore(into.path())
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(into.path().join("content")).unwrap(),
        b"restored"
    );
}

#[test]
#[timeout("10s")]
async fn a_name_whose_save_failed_is_never_given_to_another_capture() {
    let store = Arc::new(ScriptedStore::default());
    // The first save publishes its tree and then fails, as a PUT that lands after its error does.
    store.failing_saves.store(1, Ordering::SeqCst);
    store.publish_before_failing.store(true, Ordering::SeqCst);
    let snapshots = service(&store, settings(4, 4, 3), ManualClock::immediate());
    let scope = scope("names-of-failed-saves");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

    let first = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let first_name = first.name().clone();
    first.submit(
        TestCapture::with(b"first tree", &discarded),
        None,
        confirmer(&confirm),
    );
    ended(&snapshots, &scope).await;
    store.failing_saves.store(1, Ordering::SeqCst);
    store.publish_before_failing.store(false, Ordering::SeqCst);
    let second = snapshots.admit(&scope, SnapshotKind::Update).await.unwrap();
    let second_name = second.name().clone();
    let second_saved = second
        .upload_now(
            TestCapture::with(b"second tree", &discarded),
            std::future::pending(),
        )
        .await
        .is_ok();

    let trees_of_names = store.saved.lock().unwrap().iter().fold(
        BTreeMap::<String, std::collections::BTreeSet<Vec<u8>>>::new(),
        |mut names, (name, tree)| {
            names.entry(name.clone()).or_default().insert(tree.clone());
            names
        },
    );
    assert_ne!(first_name, second_name);
    assert!(second_saved);
    // The retry of the first job used its own name, and the store gave `AlreadyExists` for the
    // tree that the failed attempt published.
    assert_eq!(
        store.saved_names(),
        vec![
            first_name.as_str().to_string(),
            first_name.as_str().to_string(),
            second_name.as_str().to_string(),
            second_name.as_str().to_string(),
        ]
    );
    assert!(trees_of_names.values().all(|trees| trees.len() == 1));
    assert_eq!(confirm.names(), vec![first_name]);
}

#[test]
#[timeout("10s")]
async fn a_shutdown_before_the_confirmation_stops_the_job_without_a_confirmation_or_a_delete() {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let shutdown = CancellationToken::new();
    let snapshots = AgentFilesystemSnapshots::enabled(
        Arc::clone(&store) as Arc<dyn FilesystemSnapshotStore>,
        settings(4, 4, 1),
        ManualClock::immediate(),
        Arc::new(UnlimitedRoom),
        shutdown.clone(),
    );
    let scope = scope("shutdown");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        None,
        confirmer(&confirm),
    );
    gate.wait_reached(1).await;

    let stopping = tokio::spawn(async move {
        snapshots.shut_down().await;
        snapshots
    });
    eventually(|| shutdown.is_cancelled()).await;
    gate.open();
    let snapshots = stopping.await.unwrap();

    assert!(confirm.names().is_empty());
    assert!(store.deletes.lock().unwrap().is_empty());
    assert_eq!(discarded.load(Ordering::SeqCst), 1);
    assert!(
        snapshots
            .enabled
            .as_ref()
            .is_some_and(|enabled| !enabled.jobs_of_scope().jobs.contains_key(&scope))
    );
}

#[test]
#[timeout("10s")]
async fn a_parent_reaches_the_store_with_its_detection() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("parent");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let parent = FilesystemSnapshotName::periodic();

    let admission = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    admission.submit(
        TestCapture::with(b"tree", &discarded),
        Some((parent.clone(), ChangeDetection::SizeMtime)),
        confirmer(&confirm),
    );
    ended(&snapshots, &scope).await;

    assert_eq!(
        store.parents.lock().unwrap().clone(),
        vec![Some((
            parent.as_str().to_string(),
            ChangeDetection::SizeMtime
        ))]
    );
}

/// Uploads three manual-update snapshots of one scope, each followed by its retention, and gives
/// the names in the order of the uploads, with the name of a periodic snapshot uploaded first.
async fn update_uploads_with_retention(
    snapshots: &AgentFilesystemSnapshots,
    scope: &SnapshotScope,
) -> (String, Vec<String>) {
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let periodic = snapshots
        .admit(scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    let periodic_name = periodic.name().as_str().to_string();
    periodic.submit(
        TestCapture::with(b"periodic", &discarded),
        None,
        confirmer(&confirm),
    );
    ended(snapshots, scope).await;
    let updates = futures::stream::iter(0..3)
        .then(|index| {
            let discarded = &discarded;
            async move {
                let admission = snapshots.admit(scope, SnapshotKind::Update).await.unwrap();
                let name = admission.name().as_str().to_string();
                admission
                    .upload_now(
                        TestCapture::with(format!("{index}").as_bytes(), discarded),
                        std::future::pending(),
                    )
                    .await
                    .unwrap()
                    .run(None);
                ended(snapshots, scope).await;
                name
            }
        })
        .collect::<Vec<_>>()
        .await;
    (periodic_name, updates)
}

#[test]
#[timeout("10s")]
async fn an_update_retention_keeps_the_own_snapshot_and_the_newest_older_updates() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("update-retention");

    let (periodic, updates) = update_uploads_with_retention(&snapshots, &scope).await;

    let kept = store
        .memory
        .list(&scope)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [periodic, updates[1].clone(), updates[2].clone()]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(kept, expected);
    assert_eq!(*store.deletes.lock().unwrap(), vec![updates[0].clone()]);
}

#[test]
#[timeout("10s")]
async fn a_dropped_update_retention_deletes_nothing_and_frees_the_scope() {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("dropped-update-retention");
    let discarded = Arc::new(AtomicUsize::new(0));

    let names = futures::stream::iter(0..3)
        .then(|index| {
            let snapshots = &snapshots;
            let scope = &scope;
            let discarded = &discarded;
            async move {
                let admission = snapshots.admit(scope, SnapshotKind::Update).await.unwrap();
                let name = admission.name().as_str().to_string();
                let retention = admission
                    .upload_now(
                        TestCapture::with(format!("{index}").as_bytes(), discarded),
                        std::future::pending(),
                    )
                    .await
                    .unwrap();
                let while_held = snapshots.admit(scope, SnapshotKind::Periodic).await.err();
                drop(retention);
                (name, while_held)
            }
        })
        .collect::<Vec<_>>()
        .await;

    assert!(store.deletes.lock().unwrap().is_empty());
    assert!(
        names
            .iter()
            .all(|(_, while_held)| *while_held == Some(SnapshotSkip::UploadInFlight))
    );
    assert!(
        snapshots
            .admit(&scope, SnapshotKind::Periodic)
            .await
            .is_ok()
    );
}

/// A service whose deletes wait at a gate, after two confirmed periodic uploads and a third
/// upload that `outcome` answers. Gives the service, the scope and the gate, once a delete waits.
async fn with_a_delete_held(
    outcome: ConfirmOutcome,
) -> (
    Arc<ScriptedStore>,
    AgentFilesystemSnapshots,
    SnapshotScope,
    Arc<Gate>,
) {
    let store = Arc::new(ScriptedStore::default());
    let mut settings = settings(4, 4, 1);
    settings = FilesystemSnapshotUploadConfig::new(
        settings.max_concurrent_uploads().get(),
        settings.max_concurrent_restores().get(),
        settings.confirmation_wait(),
        settings.store_check_limit(),
        settings.capture_wait(),
        1,
        1,
        settings.upload_retry().clone(),
    )
    .unwrap();
    let snapshots = service(&store, settings, ManualClock::immediate());
    let scope = scope("held-delete");
    let discarded = Arc::new(AtomicUsize::new(0));
    let deferred = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    let older = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    older.submit(
        TestCapture::with(b"older", &discarded),
        None,
        confirmer(&deferred),
    );
    ended(&snapshots, &scope).await;
    let gate = Arc::new(Gate::default());
    *store.delete_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let answering = ScriptedConfirmer::answering(outcome);
    let newest = snapshots
        .admit(&scope, SnapshotKind::Periodic)
        .await
        .unwrap();
    newest.submit(
        TestCapture::with(b"newest", &discarded),
        None,
        confirmer(&answering),
    );
    gate.wait_reached(1).await;
    (store, snapshots, scope, gate)
}

#[test]
#[timeout("10s")]
async fn a_shutdown_ends_a_retention_that_waits_for_a_delete() {
    let (_store, snapshots, _scope, gate) = with_a_delete_held(ConfirmOutcome::Confirmed).await;

    let stopped = tokio::time::timeout(Duration::from_secs(2), snapshots.shut_down()).await;

    gate.open();
    assert!(stopped.is_ok(), "the shutdown waited for the retention");
}

#[test]
#[timeout("10s")]
async fn forget_scope_ends_a_delete_of_a_superseded_snapshot_at_once() {
    let (_store, snapshots, scope, gate) = with_a_delete_held(ConfirmOutcome::Superseded).await;
    let job = snapshots
        .enabled
        .as_ref()
        .and_then(|enabled| enabled.jobs_of_scope().jobs.get(&scope).cloned())
        .unwrap();

    snapshots.forget_scope(&scope);
    let ended = tokio::time::timeout(Duration::from_secs(2), job.ended.cancelled()).await;

    gate.open();
    assert!(
        ended.is_ok(),
        "the job waited for its delete after forget_scope"
    );
}

#[test]
#[timeout("10s")]
async fn a_stop_of_an_upload_now_ends_the_save_and_discards_the_capture() {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let snapshots = service(&store, settings(4, 4, 1), ManualClock::immediate());
    let scope = scope("stopped-upload-now");
    let discarded = Arc::new(AtomicUsize::new(0));
    let admission = snapshots.admit(&scope, SnapshotKind::Update).await.unwrap();
    let stop = Arc::new(Notify::new());

    let uploading = admission.upload_now(TestCapture::with(b"tree", &discarded), {
        let stop = Arc::clone(&stop);
        async move { stop.notified().await }
    });
    let stopping = async {
        gate.wait_reached(1).await;
        stop.notify_one();
    };
    let (uploaded, ()) = futures::join!(uploading, stopping);
    gate.open();

    assert!(uploaded.is_err());
    assert_eq!(discarded.load(Ordering::SeqCst), 1);
    assert!(
        snapshots
            .admit(&scope, SnapshotKind::Periodic)
            .await
            .is_ok()
    );
}
