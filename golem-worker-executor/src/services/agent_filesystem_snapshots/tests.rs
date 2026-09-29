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
use async_trait::async_trait;
use futures::StreamExt as _;
use golem_common::model::RetryConfig;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;

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
    /// Whether each delete of all snapshots fails with a retryable storage error.
    all_deletes_fail: std::sync::atomic::AtomicBool,
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
    all_deletes: AtomicUsize,
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
        agent: &SnapshotScope,
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
                let _ = self.memory.save(agent, name, tree, parent).await;
            }
            return Err(retryable("the publish failed"));
        }
        let info = self.memory.save(agent, name, tree, parent).await?;
        Ok(self.timed(name, info))
    }

    async fn restore(
        &self,
        agent: &SnapshotScope,
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
            self.memory.restore(agent, name, into).await
        };
        self.restores_now.fetch_sub(1, Ordering::SeqCst);
        result
    }

    async fn stat(
        &self,
        agent: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        self.memory.stat(agent, name).await
    }

    async fn list(
        &self,
        agent: &SnapshotScope,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        let times = self.times.lock().unwrap().clone();
        Ok(self
            .memory
            .list(agent)
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
        agent: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError> {
        let gate = self.delete_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let _count = self.enter();
        tokio::task::yield_now().await;
        self.deletes.lock().unwrap().push(name.as_str().to_string());
        self.memory.delete(agent, name).await
    }

    async fn delete_scope(&self, agent: &SnapshotScope) -> Result<(), SnapshotStoreError> {
        let _count = self.enter();
        self.all_deletes.fetch_add(1, Ordering::SeqCst);
        if self.all_deletes_fail.load(Ordering::SeqCst) {
            return Err(retryable("the agent delete failed"));
        }
        self.memory.delete_scope(agent).await
    }

    async fn copy_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        self.memory.copy_scope(from, to).await
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

    async fn confirm(&self, name: FilesystemSnapshotName) -> ConfirmOutcome {
        self.names.lock().unwrap().push(name);
        if let Some(gate) = &self.gate {
            gate.pass().await;
        }
        *self.outcome.lock().unwrap()
    }
}

fn confirmer(confirm: &Arc<ScriptedConfirmer>) -> Confirm {
    let confirm = Arc::clone(confirm);
    Box::new(move |name| Box::pin(async move { confirm.confirm(name).await }))
}

/// A capture in a temporary directory with one file of `content`. Its discard is counted, and
/// waits for `discard_gate` when it is set.
fn gated_capture(
    content: &[u8],
    discarded: &Arc<AtomicUsize>,
    discard_gate: Option<Arc<Gate>>,
) -> CapturedTree {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("content"), content).unwrap();
    let discarded = Arc::clone(discarded);
    CapturedTree::new(
        Arc::from(directory.path()),
        Box::pin(async move {
            if let Some(gate) = &discard_gate {
                gate.pass().await;
            }
            drop(directory);
            discarded.fetch_add(1, Ordering::SeqCst);
        }),
    )
}

/// A capture in a temporary directory with one file of `content`. Its discard is counted.
fn capture(content: &[u8], discarded: &Arc<AtomicUsize>) -> CapturedTree {
    gated_capture(content, discarded, None)
}

/// Runs `test` on a runtime whose time is paused, so each wait of the service ends as soon as
/// the runtime has nothing else to do.
fn paused<T>(test: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(test)
}

impl AgentFilesystemSnapshots {
    /// Admits a job of `kind` without the wait of a manual update.
    async fn admit(
        &self,
        agent: &SnapshotScope,
        kind: SnapshotKind,
    ) -> Result<Admission, SnapshotSkip> {
        match &self.core {
            Some(core) => Core::admit(core, agent, kind)
                .await
                .map_err(|refusal| refusal.skip),
            None => Err(SnapshotSkip::Disabled),
        }
    }

    /// Whether no job runs for `agent`.
    fn is_free(&self, agent: &SnapshotScope) -> bool {
        self.core
            .as_ref()
            .is_none_or(|core| core.registry.read(|state| rules::is_free(state, agent)))
    }
}

/// A receiver that never reports a terminal interrupt.
fn no_interrupt() -> watch::Receiver<bool> {
    watch::channel(false).1
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

/// A service over `store` with `settings`, and room on the volume.
fn service(
    store: &Arc<ScriptedStore>,
    settings: FilesystemSnapshotUploadConfig,
) -> AgentFilesystemSnapshots {
    AgentFilesystemSnapshots::enabled(
        Arc::clone(store) as Arc<dyn FilesystemSnapshotStore>,
        settings,
        VolumeRoom::Unlimited,
        CancellationToken::new(),
    )
}

fn agent_snapshots(name: &str) -> SnapshotScope {
    SnapshotScope::agent(&OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        },
    ))
}

/// The time that a test waits for a condition. The time of the tests is paused, so it only has
/// to be longer than the waits of the service.
const PATIENCE: Duration = Duration::from_secs(600);

/// Waits until `condition` holds, and fails the test after [`PATIENCE`].
async fn eventually(condition: impl Fn() -> bool) {
    let held = futures::stream::repeat(())
        .then(|()| tokio::time::sleep(Duration::from_millis(5)))
        .filter(|()| std::future::ready(condition()));
    assert!(
        tokio::time::timeout(PATIENCE, std::pin::pin!(held).next())
            .await
            .is_ok(),
        "the condition did not hold in time"
    );
}

/// Waits until no job runs for `agent`, and fails the test after [`PATIENCE`].
async fn ended(snapshots: &AgentFilesystemSnapshots, agent: &SnapshotScope) {
    if let Some(core) = &snapshots.core {
        assert!(
            tokio::time::timeout(PATIENCE, core.registry.until_agent_free(agent))
                .await
                .is_ok(),
            "the job did not end in time"
        );
    }
}

#[test]
fn each_admission_makes_a_new_name_with_the_prefix_of_its_kind() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("names");

        let names = futures::stream::iter([
            SnapshotKind::Periodic,
            SnapshotKind::Periodic,
            SnapshotKind::Update,
            SnapshotKind::Update,
        ])
        .then(|kind| {
            let snapshots = &snapshots;
            let agent = &agent;
            async move { snapshots.admit(agent, kind).await.unwrap().name().clone() }
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
    })
}

#[test]
fn a_second_admission_of_an_agent_gets_upload_in_flight_until_the_first_ends() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("in-flight");
        let other = agent_snapshots("other");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        let while_admitted = snapshots.admit(&agent, SnapshotKind::Update).await.err();
        let other_agent = snapshots
            .admit(&other, SnapshotKind::Periodic)
            .await
            .is_ok();
        admission.submit(capture(b"one", &discarded), None, confirmer(&confirm));
        gate.wait_reached(1).await;
        let while_uploading = snapshots.admit(&agent, SnapshotKind::Periodic).await.err();
        gate.open();
        ended(&snapshots, &agent).await;
        let after = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .is_ok();

        assert_eq!(
            (while_admitted, other_agent, while_uploading, after),
            (
                Some(SnapshotSkip::UploadInFlight),
                true,
                Some(SnapshotSkip::UploadInFlight),
                true
            )
        );
    })
}

#[test]
fn a_dropped_admission_frees_its_agent_and_writes_nothing() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("dropped");

        drop(
            snapshots
                .admit(&agent, SnapshotKind::Periodic)
                .await
                .unwrap(),
        );
        let again = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .is_ok();

        assert!(again);
        assert!(store.saved_names().is_empty());
    })
}

#[test]
fn a_disabled_service_admits_nothing_and_restores_nothing() {
    paused(async {
        let snapshots = AgentFilesystemSnapshots::disabled();
        let agent = agent_snapshots("disabled");

        let admitted = snapshots.admit(&agent, SnapshotKind::Periodic).await.err();
        let restored = snapshots
            .restore(&agent, &FilesystemSnapshotName::periodic())
            .err();

        assert_eq!(
            (admitted, restored),
            (Some(SnapshotSkip::Disabled), Some(SnapshotsDisabled))
        );
    })
}

#[test]
fn a_job_saves_discards_and_calls_the_confirmer_once_with_its_own_name() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("confirmed");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        ended(&snapshots, &agent).await;

        let listed = store
            .memory
            .list(&agent)
            .await
            .unwrap()
            .iter()
            .map(|(name, _)| name.as_str().to_string())
            .collect::<Vec<_>>();
        assert_eq!(confirm.names(), vec![name.clone()]);
        assert_eq!(listed, vec![name.as_str().to_string()]);
        assert_eq!(discarded.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn after_the_retry_budget_the_job_discards_the_capture_and_calls_no_confirmer() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        store.failing_saves.store(usize::MAX, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4, 3));
        let agent = agent_snapshots("budget");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        let started = tokio::time::Instant::now();
        ended(&snapshots, &agent).await;

        assert_eq!(store.saved_names().len(), 3);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(2) + Duration::from_secs(8)
        );
        assert!(confirm.names().is_empty());
        assert_eq!(discarded.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn an_error_that_allows_no_retry_ends_the_job_after_one_attempt() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        store.saves_fail_for_good.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4, 5));
        let agent = agent_snapshots("for-good");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        ended(&snapshots, &agent).await;

        assert_eq!(store.saved_names().len(), 1);
        assert!(confirm.names().is_empty());
    })
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
    let snapshots = service(&store, settings);
    let agent = agent_snapshots("dropped-confirmation");
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirmed = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let older = futures::stream::iter(0..3)
        .then(|index| {
            let snapshots = &snapshots;
            let agent = &agent;
            let discarded = &discarded;
            let confirmed = &confirmed;
            async move {
                let admission = snapshots
                    .admit(agent, SnapshotKind::Periodic)
                    .await
                    .unwrap();
                let name = admission.name().clone();
                admission.submit(
                    capture(format!("older-{index}").as_bytes(), discarded),
                    None,
                    confirmer(confirmed),
                );
                ended(snapshots, agent).await;
                name
            }
        })
        .collect::<Vec<_>>()
        .await;
    let deletes_before = store.deletes.lock().unwrap().len();
    let confirm = ScriptedConfirmer::answering(outcome);

    let admission = snapshots
        .admit(&agent, SnapshotKind::Periodic)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(capture(b"newest", &discarded), None, confirmer(&confirm));
    ended(&snapshots, &agent).await;

    let deletes = store.deletes.lock().unwrap()[deletes_before..].to_vec();
    let held = store
        .memory
        .stat(&agent, &store_name(&name).unwrap())
        .await
        .unwrap()
        .is_some();
    let older_held = futures::stream::iter(older.iter())
        .then(|older| {
            let store = &store;
            let agent = &agent;
            async move {
                store
                    .memory
                    .stat(agent, &store_name(older).unwrap())
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
fn superseded_deletes_the_snapshot_and_runs_no_retention() {
    paused(async {
        let (deletes, name, held, older_held) =
            after_older_uploads(ConfirmOutcome::Superseded).await;

        assert_eq!(deletes, vec![name]);
        assert!(!held);
        assert_eq!(older_held, vec![true, true, true]);
    })
}

#[test]
fn deferred_keeps_the_snapshot_and_runs_no_retention() {
    paused(async {
        let (deletes, _, held, older_held) = after_older_uploads(ConfirmOutcome::Deferred).await;

        assert!(deletes.is_empty());
        assert!(held);
        assert_eq!(older_held, vec![true, true, true]);
    })
}

#[test]
fn confirmed_runs_retention_after_the_confirmation() {
    paused(async {
        let (deletes, _, held, older_held) = after_older_uploads(ConfirmOutcome::Confirmed).await;

        assert_eq!(deletes.len(), 1);
        assert!(held);
        assert_eq!(older_held, vec![false, true, true]);
    })
}

#[test]
fn a_confirmed_periodic_upload_keeps_the_newest_by_kind() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("retention");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let upload = |kind: SnapshotKind, index: usize| {
            let snapshots = &snapshots;
            let agent = &agent;
            let discarded = &discarded;
            let confirm = &confirm;
            async move {
                let admission = snapshots.admit(agent, kind).await.unwrap();
                let name = admission.name().clone();
                match kind {
                    SnapshotKind::Periodic => admission.submit(
                        capture(format!("{index}").as_bytes(), discarded),
                        None,
                        confirmer(confirm),
                    ),
                    SnapshotKind::Update => {
                        drop(
                            admission
                                .upload_now(
                                    capture(format!("{index}").as_bytes(), discarded),
                                    no_interrupt(),
                                )
                                .await
                                .unwrap(),
                        );
                    }
                }
                ended(snapshots, agent).await;
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
            .list(&agent)
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
    })
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
fn retention_neither_counts_nor_deletes_a_snapshot_within_the_clock_skew_margin() {
    let before_own = |seconds: u64| SnapshotInfo {
        created_at: golem_common::model::Timestamp::from(
            FIRST_SAVE_MILLIS + 100 * 60 * 1000 - seconds * 1000,
        ),
        files: 0,
        bytes: 0,
    };
    let listing = [
        ("p-own", 0),
        ("p-inside", 90),
        ("p-at-the-margin", 120),
        ("p-old", 600),
    ]
    .map(|(name, seconds)| (SnapshotName::new(name).unwrap(), before_own(seconds)));
    let own = SnapshotName::new("p-own").unwrap();

    let victims = names(&retention::victims(&listing, &own, &before_own(0), 1, None));

    assert_eq!(victims, vec!["p-old"]);
}

#[test]
fn the_labels_and_the_messages_of_the_service_name_what_they_count_and_report() {
    let labels = [
        SnapshotKind::Periodic.label(),
        SnapshotKind::Update.label(),
        ConfirmOutcome::Confirmed.label(),
        ConfirmOutcome::Superseded.label(),
        ConfirmOutcome::Deferred.label(),
    ];
    let messages = [
        SnapshotSkip::Disabled.to_string(),
        SnapshotSkip::UploadInFlight.to_string(),
        SnapshotSkip::VolumeUnderPressure.to_string(),
        SnapshotSkip::DeletingAllSnapshots.to_string(),
        SnapshotsDisabled.to_string(),
    ];

    assert_eq!(
        labels,
        ["periodic", "update", "confirmed", "superseded", "deferred"]
    );
    assert_eq!(
        messages,
        [
            "filesystem snapshots are disabled on this executor",
            "an upload of a filesystem snapshot of the agent runs now",
            "the volume of the agent filesystems is under pressure",
            "the filesystem snapshots of the agent are being deleted",
            "the record names a filesystem snapshot, and filesystem snapshots are disabled on \
             this executor",
        ]
        .map(String::from)
    );
}

#[test]
fn an_admission_waits_for_the_open_file_calls_as_long_as_the_settings_say() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let settings = settings(4, 4, 1);
        let snapshots = service(&store, settings.clone());
        let agent = agent_snapshots("capture-wait");

        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();

        assert_eq!(admission.capture_wait(), settings.capture_wait());
        assert_ne!(admission.capture_wait(), Duration::ZERO);
    })
}

#[test]
fn a_copy_of_all_snapshots_holds_each_snapshot_of_the_agent() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let from = agent_snapshots("copied-from");
        let to = agent_snapshots("copied-to");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit(&from, SnapshotKind::Periodic)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        ended(&snapshots, &from).await;

        snapshots.copy_all_snapshots(&from, &to).await.unwrap();
        let listed = store
            .memory
            .list(&to)
            .await
            .unwrap()
            .iter()
            .map(|(name, _)| name.as_str().to_string())
            .collect::<Vec<_>>();

        assert_eq!(listed, vec![name.as_str().to_string()]);
    })
}

#[test]
fn an_admission_during_a_delete_of_all_snapshots_gets_deleting_all_snapshots_until_the_delete_ends()
{
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        store.all_deletes_fail.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4, 2));
        let agent = agent_snapshots("deleting");

        snapshots.delete_all_snapshots(&agent);
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;
        let while_deleting = snapshots.admit(&agent, SnapshotKind::Periodic).await.err();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 2).await;
        let after = {
            let admitted = snapshots.admit(&agent, SnapshotKind::Periodic).await;
            let retried = futures::stream::repeat(())
                .then(|()| tokio::time::sleep(Duration::from_millis(5)))
                .then(|()| snapshots.admit(&agent, SnapshotKind::Periodic))
                .filter(|admitted| std::future::ready(admitted.is_ok()));
            match admitted {
                Ok(_) => true,
                Err(_) => {
                    tokio::time::timeout(Duration::from_secs(5), std::pin::pin!(retried).next())
                        .await
                        .is_ok()
                }
            }
        };

        assert_eq!(while_deleting, Some(SnapshotSkip::DeletingAllSnapshots));
        assert!(after);
    })
}

#[test]
fn the_decision_of_a_job_carries_its_outcome_before_the_job_ends() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4, 1));
        let core = snapshots.core.as_ref().unwrap();
        let agent = agent_snapshots("decided");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        let name = admission.name().clone();

        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        gate.wait_reached(1).await;
        let waiting = WaitTicket::start_wait(&core.registry, &agent, &name);
        gate.open();
        let decision = match &waiting {
            Ok(ticket) => Some(ticket.decided().await),
            Err(_) => None,
        };
        ended(&snapshots, &agent).await;
        let after_end = match &waiting {
            Ok(ticket) => Some(ticket.decided().await),
            Err(_) => None,
        };

        assert_eq!(
            (decision, after_end),
            (
                Some(JobDecision::Confirmed(ConfirmOutcome::Deferred)),
                Some(JobDecision::Confirmed(ConfirmOutcome::Deferred))
            )
        );
    })
}

/// A service whose save waits at a gate, with a periodic job of `agent` that saves now. Gives the
/// store, the service, the gate and the name of the job.
async fn with_a_save_held(
    agent: &SnapshotScope,
    outcome: ConfirmOutcome,
) -> (
    Arc<ScriptedStore>,
    AgentFilesystemSnapshots,
    Arc<Gate>,
    FilesystemSnapshotName,
) {
    let store = Arc::new(ScriptedStore::default());
    let gate = Arc::new(Gate::default());
    *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let snapshots = service(&store, settings(4, 4, 1));
    let discarded = Arc::new(AtomicUsize::new(0));
    let admission = snapshots
        .admit(agent, SnapshotKind::Periodic)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(
        capture(b"tree", &discarded),
        None,
        confirmer(&ScriptedConfirmer::answering(outcome)),
    );
    gate.wait_reached(1).await;
    (store, snapshots, gate, name)
}

#[test]
fn a_manual_update_waits_for_a_running_upload_up_to_the_limit_and_then_is_refused() {
    paused(async {
        let agent = agent_snapshots("update-at-limit");
        let (_store, snapshots, gate, _) =
            with_a_save_held(&agent, ConfirmOutcome::Confirmed).await;

        let started = tokio::time::Instant::now();
        let admitted = snapshots.admit_update(&agent, no_interrupt()).await.err();
        let waited = started.elapsed();
        gate.open();
        ended(&snapshots, &agent).await;

        assert_eq!(
            admitted,
            Some(UpdateRefusal::Skip(SnapshotSkip::UploadInFlight))
        );
        assert_eq!(waited, Duration::from_secs(60));
    })
}

#[test]
fn a_manual_update_is_admitted_once_the_running_upload_ends() {
    paused(async {
        let agent = agent_snapshots("update-after-end");
        let (_store, snapshots, gate, _) =
            with_a_save_held(&agent, ConfirmOutcome::Confirmed).await;

        let started = tokio::time::Instant::now();
        let admitting = snapshots.admit_update(&agent, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (admitted, ()) = futures::join!(admitting, opening);
        let waited = started.elapsed();
        let without_job = snapshots
            .admit_update(&agent_snapshots("update-without-job"), no_interrupt())
            .await
            .is_ok();

        assert!(admitted.is_ok());
        assert!(waited < Duration::from_secs(60), "waited {waited:?}");
        assert!(without_job);
    })
}

#[test]
fn a_terminal_interrupt_ends_the_wait_of_a_manual_update() {
    paused(async {
        let agent = agent_snapshots("update-interrupted");
        let (_store, snapshots, gate, _) =
            with_a_save_held(&agent, ConfirmOutcome::Confirmed).await;
        let (interrupt, interrupted) = watch::channel(false);

        let admitting = snapshots.admit_update(&agent, interrupted);
        let interrupting = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            interrupt.send_replace(true);
        };
        let (admitted, ()) = futures::join!(admitting, interrupting);
        gate.open();

        assert_eq!(admitted.err(), Some(UpdateRefusal::Interrupted));
    })
}

#[test]
fn a_start_without_a_running_upload_asks_the_store_at_once() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("start-without-upload");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(
            capture(b"tree", &discarded),
            None,
            confirmer(&ScriptedConfirmer::answering(ConfirmOutcome::Deferred)),
        );
        ended(&snapshots, &agent).await;

        let started = tokio::time::Instant::now();
        let stored = snapshots.prepare_start(&agent, &name, no_interrupt()).await;
        let other = snapshots
            .prepare_start(&agent, &FilesystemSnapshotName::periodic(), no_interrupt())
            .await;
        let disabled = AgentFilesystemSnapshots::disabled()
            .prepare_start(&agent, &name, no_interrupt())
            .await;

        assert_eq!(
            (stored, other, disabled),
            (
                StartCheck::Stored,
                StartCheck::NotStored,
                StartCheck::NotStored
            )
        );
        assert_eq!(started.elapsed(), Duration::ZERO);
    })
}

#[test]
fn a_start_waits_for_the_upload_of_its_name_up_to_the_limit() {
    paused(async {
        let agent = agent_snapshots("start-at-limit");
        let (_store, snapshots, gate, name) =
            with_a_save_held(&agent, ConfirmOutcome::Deferred).await;

        let started = tokio::time::Instant::now();
        let at_limit = snapshots.prepare_start(&agent, &name, no_interrupt()).await;
        let waited = started.elapsed();
        gate.open();
        ended(&snapshots, &agent).await;

        assert_eq!(at_limit, StartCheck::NotStored);
        assert_eq!(waited, Duration::from_secs(60));
    })
}

#[test]
fn a_start_that_waited_checks_the_store_after_the_decision() {
    paused(async {
        let agent = agent_snapshots("start-after-decision");
        let (_store, snapshots, gate, name) =
            with_a_save_held(&agent, ConfirmOutcome::Deferred).await;

        let started = tokio::time::Instant::now();
        let preparing = snapshots.prepare_start(&agent, &name, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (stored, ()) = futures::join!(preparing, opening);

        assert_eq!(stored, StartCheck::Stored);
        assert!(started.elapsed() < Duration::from_secs(60));
    })
}

#[test]
fn a_start_after_a_superseded_upload_does_not_ask_the_store() {
    paused(async {
        let agent = agent_snapshots("start-after-superseded");
        let (store, snapshots, gate, name) =
            with_a_save_held(&agent, ConfirmOutcome::Superseded).await;
        let delete_gate = Arc::new(Gate::default());
        *store.delete_gate.lock().unwrap() = Some(Arc::clone(&delete_gate));

        let preparing = snapshots.prepare_start(&agent, &name, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (checked, ()) = futures::join!(preparing, opening);
        let held = store
            .memory
            .stat(&agent, &store_name(&name).unwrap())
            .await
            .unwrap()
            .is_some();
        delete_gate.open();

        assert_eq!(checked, StartCheck::NotStored);
        assert!(held);
    })
}

#[test]
fn a_terminal_interrupt_ends_the_wait_of_a_start_and_skips_the_check() {
    paused(async {
        let agent = agent_snapshots("start-interrupted");
        let (store, snapshots, gate, name) =
            with_a_save_held(&agent, ConfirmOutcome::Deferred).await;
        let (interrupt, interrupted) = watch::channel(false);

        let started = tokio::time::Instant::now();
        let preparing = snapshots.prepare_start(&agent, &name, interrupted.clone());
        let interrupting = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            interrupt.send_replace(true);
        };
        let (during_wait, ()) = futures::join!(preparing, interrupting);
        let waited = started.elapsed();
        gate.open();
        ended(&snapshots, &agent).await;
        let stored = store
            .memory
            .stat(&agent, &store_name(&name).unwrap())
            .await
            .unwrap()
            .is_some();
        let after_upload = snapshots.prepare_start(&agent, &name, interrupted).await;

        assert_eq!(during_wait, StartCheck::NotStored);
        assert_eq!(waited, Duration::from_secs(1));
        assert!(stored);
        assert_eq!(after_upload, StartCheck::NotStored);
    })
}

#[test]
fn delete_all_snapshots_cancels_the_job_and_deletes_them_after_the_job_ended() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let save_gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&save_gate));
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("delete-all");
        let discarded = Arc::new(AtomicUsize::new(0));
        let discard_gate = Arc::new(Gate::default());
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        admission.submit(
            gated_capture(b"tree", &discarded, Some(Arc::clone(&discard_gate))),
            None,
            confirmer(&confirm),
        );
        save_gate.wait_reached(1).await;

        snapshots.delete_all_snapshots(&agent);
        discard_gate.wait_reached(1).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let while_the_job_runs = store.all_deletes.load(Ordering::SeqCst);
        discard_gate.open();
        ended(&snapshots, &agent).await;
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;

        assert_eq!(while_the_job_runs, 0);
        assert!(confirm.names().is_empty());
        assert!(store.deletes.lock().unwrap().is_empty());
        assert_eq!(discarded.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn a_cancelled_confirmation_deletes_nothing() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        let confirm = Arc::new(ScriptedConfirmer {
            outcome: Mutex::new(ConfirmOutcome::Confirmed),
            names: Mutex::default(),
            gate: Some(Arc::clone(&gate)),
        });
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("cancelled-confirmation");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        gate.wait_reached(1).await;

        snapshots.delete_all_snapshots(&agent);
        ended(&snapshots, &agent).await;
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;

        assert!(
            !store
                .deletes
                .lock()
                .unwrap()
                .contains(&name.as_str().to_string())
        );
        assert!(store.memory.list(&agent).await.unwrap().is_empty());
    })
}

#[test]
fn a_store_that_fails_every_delete_of_all_snapshots_leaves_the_service_running() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        store.all_deletes_fail.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4, 3));
        let agent = agent_snapshots("failing-delete");

        snapshots.delete_all_snapshots(&agent);
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 3).await;
        let discarded = Arc::new(AtomicUsize::new(0));
        let saved = snapshots
            .admit(&agent, SnapshotKind::Update)
            .await
            .unwrap()
            .upload_now(capture(b"after", &discarded), no_interrupt())
            .await
            .is_ok();

        assert!(saved);
    })
}

#[test]
fn each_delete_holds_a_slot_of_the_uploads() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(1, 4, 1));
        let uploading = agent_snapshots("uploading");
        let deleted_names = agent_snapshots("deleted-names");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit(&uploading, SnapshotKind::Periodic)
            .await
            .unwrap();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        gate.wait_reached(1).await;

        snapshots.delete_snapshots(
            &deleted_names,
            Box::new([
                FilesystemSnapshotName::periodic(),
                FilesystemSnapshotName::periodic(),
            ]),
        );
        snapshots.delete_all_snapshots(&agent_snapshots("deleted"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before = (
            store.deletes.lock().unwrap().len(),
            store.all_deletes.load(Ordering::SeqCst),
        );
        gate.open();
        ended(&snapshots, &uploading).await;
        eventually(|| {
            store.deletes.lock().unwrap().len() == 2
                && store.all_deletes.load(Ordering::SeqCst) == 1
        })
        .await;

        assert_eq!(before, (0, 0));
        assert_eq!(store.most_store_calls_at_once.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn at_most_the_configured_number_of_restores_run_at_the_same_time() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 2, 1));
        let agent = agent_snapshots("restores");
        let discarded = Arc::new(AtomicUsize::new(0));
        let name = {
            let admission = snapshots.admit(&agent, SnapshotKind::Update).await.unwrap();
            let name = admission.name().clone();
            drop(
                admission
                    .upload_now(capture(b"tree", &discarded), no_interrupt())
                    .await
                    .unwrap(),
            );
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
                let restore = snapshots.restore(&agent, &name).unwrap();
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
    })
}

#[test]
fn a_restore_gives_the_saved_tree() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 2, 1));
        let agent = agent_snapshots("restore");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots.admit(&agent, SnapshotKind::Update).await.unwrap();
        let name = admission.name().clone();
        drop(
            admission
                .upload_now(capture(b"restored", &discarded), no_interrupt())
                .await
                .unwrap(),
        );
        let into = tempfile::tempdir().unwrap();

        snapshots
            .restore(&agent, &name)
            .unwrap()
            .restore(into.path())
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(into.path().join("content")).unwrap(),
            b"restored"
        );
    })
}

#[test]
fn a_name_whose_save_failed_is_never_given_to_another_capture() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        // The first save publishes its tree and then fails, as a PUT that lands after its error does.
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4, 3));
        let agent = agent_snapshots("names-of-failed-saves");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let first = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        let first_name = first.name().clone();
        first.submit(
            capture(b"first tree", &discarded),
            None,
            confirmer(&confirm),
        );
        ended(&snapshots, &agent).await;
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(false, Ordering::SeqCst);
        let second = snapshots.admit(&agent, SnapshotKind::Update).await.unwrap();
        let second_name = second.name().clone();
        let second_saved = second
            .upload_now(capture(b"second tree", &discarded), no_interrupt())
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
    })
}

#[test]
fn a_shutdown_before_the_confirmation_stops_the_job_without_a_confirmation_or_a_delete() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let shutdown = CancellationToken::new();
        let snapshots = AgentFilesystemSnapshots::enabled(
            Arc::clone(&store) as Arc<dyn FilesystemSnapshotStore>,
            settings(4, 4, 1),
            VolumeRoom::Unlimited,
            shutdown.clone(),
        );
        let agent = agent_snapshots("shutdown");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
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
        assert!(snapshots.is_free(&agent));
    })
}

#[test]
fn a_parent_reaches_the_store_with_its_detection() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("parent");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let parent = FilesystemSnapshotName::periodic();

        let admission = snapshots
            .admit(&agent, SnapshotKind::Periodic)
            .await
            .unwrap();
        admission.submit(
            capture(b"tree", &discarded),
            Some((parent.clone(), ChangeDetection::SizeMtime)),
            confirmer(&confirm),
        );
        ended(&snapshots, &agent).await;

        assert_eq!(
            store.parents.lock().unwrap().clone(),
            vec![Some((
                parent.as_str().to_string(),
                ChangeDetection::SizeMtime
            ))]
        );
    })
}

/// Uploads three manual-update snapshots of one agent, each followed by the delete of its older
/// snapshots, and gives the names in the order of the uploads, with the name of a periodic snapshot
/// uploaded first.
async fn update_uploads_with_retention(
    snapshots: &AgentFilesystemSnapshots,
    agent: &SnapshotScope,
) -> (String, Vec<String>) {
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let periodic = snapshots
        .admit(agent, SnapshotKind::Periodic)
        .await
        .unwrap();
    let periodic_name = periodic.name().as_str().to_string();
    periodic.submit(capture(b"periodic", &discarded), None, confirmer(&confirm));
    ended(snapshots, agent).await;
    let updates = futures::stream::iter(0..3)
        .then(|index| {
            let discarded = &discarded;
            async move {
                let admission = snapshots.admit(agent, SnapshotKind::Update).await.unwrap();
                let name = admission.name().as_str().to_string();
                admission
                    .upload_now(
                        capture(format!("{index}").as_bytes(), discarded),
                        no_interrupt(),
                    )
                    .await
                    .unwrap()
                    .delete_older_snapshots(None);
                ended(snapshots, agent).await;
                name
            }
        })
        .collect::<Vec<_>>()
        .await;
    (periodic_name, updates)
}

#[test]
fn an_update_retention_keeps_the_own_snapshot_and_the_newest_older_updates() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("update-retention");

        let (periodic, updates) = update_uploads_with_retention(&snapshots, &agent).await;

        let kept = store
            .memory
            .list(&agent)
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
    })
}

#[test]
fn a_dropped_update_retention_deletes_nothing_and_frees_the_agent() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("dropped-update-retention");
        let discarded = Arc::new(AtomicUsize::new(0));

        let names = futures::stream::iter(0..3)
            .then(|index| {
                let snapshots = &snapshots;
                let agent = &agent;
                let discarded = &discarded;
                async move {
                    let admission = snapshots.admit(agent, SnapshotKind::Update).await.unwrap();
                    let name = admission.name().as_str().to_string();
                    let retention = admission
                        .upload_now(
                            capture(format!("{index}").as_bytes(), discarded),
                            no_interrupt(),
                        )
                        .await
                        .unwrap();
                    let while_held = snapshots.admit(agent, SnapshotKind::Periodic).await.err();
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
                .admit(&agent, SnapshotKind::Periodic)
                .await
                .is_ok()
        );
    })
}

/// A service whose deletes wait at a gate, after two confirmed periodic uploads and a third
/// upload that `outcome` answers. Gives the service, the agent and the gate, once a delete waits.
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
    let snapshots = service(&store, settings);
    let agent = agent_snapshots("held-delete");
    let discarded = Arc::new(AtomicUsize::new(0));
    let deferred = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    let older = snapshots
        .admit(&agent, SnapshotKind::Periodic)
        .await
        .unwrap();
    older.submit(capture(b"older", &discarded), None, confirmer(&deferred));
    ended(&snapshots, &agent).await;
    let gate = Arc::new(Gate::default());
    *store.delete_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let answering = ScriptedConfirmer::answering(outcome);
    let newest = snapshots
        .admit(&agent, SnapshotKind::Periodic)
        .await
        .unwrap();
    newest.submit(capture(b"newest", &discarded), None, confirmer(&answering));
    gate.wait_reached(1).await;
    (store, snapshots, agent, gate)
}

#[test]
fn a_shutdown_ends_a_retention_that_waits_for_a_delete() {
    paused(async {
        let (_store, snapshots, _agent, gate) = with_a_delete_held(ConfirmOutcome::Confirmed).await;

        let stopped = tokio::time::timeout(Duration::from_secs(2), snapshots.shut_down()).await;

        gate.open();
        assert!(stopped.is_ok(), "the shutdown waited for the retention");
    })
}

#[test]
fn delete_all_snapshots_ends_a_delete_of_a_superseded_snapshot_at_once() {
    paused(async {
        let (_store, snapshots, agent, gate) = with_a_delete_held(ConfirmOutcome::Superseded).await;
        let core = snapshots.core.as_ref().unwrap();

        snapshots.delete_all_snapshots(&agent);
        let ended = tokio::time::timeout(
            Duration::from_secs(2),
            core.registry.until_agent_free(&agent),
        )
        .await;

        gate.open();
        assert!(
            ended.is_ok(),
            "the job waited for its delete after delete_all_snapshots"
        );
    })
}

#[test]
fn a_stop_of_an_upload_now_ends_the_save_and_discards_the_capture() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4, 1));
        let agent = agent_snapshots("stopped-upload-now");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots.admit(&agent, SnapshotKind::Update).await.unwrap();
        let (stop, stopped) = watch::channel(false);

        let uploading = admission.upload_now(capture(b"tree", &discarded), stopped);
        let stopping = async {
            gate.wait_reached(1).await;
            stop.send_replace(true);
        };
        let (uploaded, ()) = futures::join!(uploading, stopping);
        gate.open();

        assert!(uploaded.is_err());
        assert_eq!(discarded.load(Ordering::SeqCst), 1);
        assert!(
            snapshots
                .admit(&agent, SnapshotKind::Periodic)
                .await
                .is_ok()
        );
    })
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[test_r::timeout("60s")]
async fn managed_xfs_a_volume_below_the_pressure_target_admits_no_periodic_upload() {
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let storage = crate::services::golem_config::FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root),
        ..crate::services::golem_config::FilesystemStorageConfig::default()
    };
    let provisioning = crate::sandbox_filesystem::SandboxFilesystemProvisioning::new(
        storage.deterministic_root_dir.clone(),
        storage.managed_xfs_root_dir.clone(),
        storage.cleanup_retry.clone(),
    )
    .unwrap();
    let pressure = FilesystemPressureConfig::new(1, u64::MAX, 1, 2, 1, Duration::ZERO).unwrap();
    let snapshots = AgentFilesystemSnapshots::enabled(
        Arc::new(InMemorySnapshotStore::default()) as Arc<dyn FilesystemSnapshotStore>,
        settings(4, 4, 1),
        VolumeRoom::Pressure {
            volume: provisioning.volume().clone(),
            pressure,
        },
        CancellationToken::new(),
    );

    let admitted = snapshots
        .admit_periodic(&agent_snapshots("managed-xfs-pressure"))
        .await;

    assert_eq!(admitted.err(), Some(SnapshotSkip::VolumeUnderPressure));
}
