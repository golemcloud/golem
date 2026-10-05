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

use super::fork::Copied;
use super::store_calls::StoreOf;
use super::*;
use crate::filesystem_snapshot::{
    CallError, Failed, FilesystemSnapshotStore, InMemorySnapshotStore, ReadError, RestoreFailure,
    RunSlots, SpacedTimes, Unlimited, Withdrawal,
};
use crate::services::agent_filesystem::RestoreTree;
use crate::services::golem_config::FilesystemSnapshotUploadValues;
use async_trait::async_trait;
use futures::FutureExt as _;
use futures::StreamExt as _;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;

/// The name of one save and the content of its tree.
type SavedTree = (Box<str>, Box<[u8]>);

/// The parent of one save, by name, with its change detection.
type SavedParent = Option<(Box<str>, ChangeDetection)>;

/// A store over the in-memory store that a test can make fail, hold and count. Each call runs as
/// the rustic store runs it: each scripted run takes a slot of the limiter of the call, and a run
/// that fails is followed by a wait of [`scripted_wait`] that a withdrawal ends, until the call has
/// made `runs` runs.
#[derive(Default)]
struct ScriptedStore {
    memory: InMemorySnapshotStore,
    /// The runs of one call after a run that failed. No value is one run.
    runs: AtomicUsize,
    /// The runs that started, over all calls.
    runs_started: AtomicUsize,
    /// The calls of `save`, also those that no run of followed.
    save_calls: AtomicUsize,
    /// The cancel of each call of `save`, in the order of the calls.
    save_cancels: Mutex<Vec<CancellationToken>>,
    /// The number of save runs that still fail.
    failing_saves: AtomicUsize,
    /// Whether a failing save run publishes the tree after its error, as a PUT that ended without
    /// an answer and lands later does. The call then waits for the late publish without a slot,
    /// checks its own name, and answers with the info of the published tree.
    publish_before_failing: std::sync::atomic::AtomicBool,
    /// Whether a failing save run ends with a publish that ended without an answer and never
    /// lands: the call waits for it without a slot, finds no own file, and goes on as after a
    /// failed run.
    late_publish_never_lands: std::sync::atomic::AtomicBool,
    /// When set, the wait of a call for a late publish waits for it.
    late_gate: Mutex<Option<Arc<Gate>>>,
    /// Whether a run that answers at once still reports a wait for late writes, as a stale report
    /// would. The call returns at once after it.
    reports_late_on_success: std::sync::atomic::AtomicBool,
    /// The number of calls that wait for a late publish now.
    late_waits: AtomicUsize,
    /// Whether the store was shut down.
    shut_down: std::sync::atomic::AtomicBool,
    /// Whether each save fails with an error that no run can fix.
    saves_fail_for_good: std::sync::atomic::AtomicBool,
    /// Whether each run of a delete of all snapshots fails.
    all_deletes_fail: std::sync::atomic::AtomicBool,
    /// When set, each save waits for it before it runs.
    save_gate: Mutex<Option<Arc<Gate>>>,
    /// Each save of one of these agents waits at the gate of the agent before it runs.
    held_saves: Mutex<Vec<(AgentSnapshots, Arc<Gate>)>>,
    /// Each save of one of these agents runs in a task of its own, which waits at the gate of the
    /// agent and then saves, also when the caller drops the call, as a blocking backup does.
    detached_saves: Mutex<Vec<(AgentSnapshots, Arc<Gate>)>>,
    /// The number of detached saves that ended.
    detached_saves_ended: Arc<AtomicUsize>,
    /// The number of runs of deletes of names that still fail.
    failing_deletes: AtomicUsize,
    /// The number of runs of restores that still fail.
    failing_restores: AtomicUsize,
    /// The number of deletes of a name that failed.
    failed_deletes: AtomicUsize,
    /// The number of listings.
    lists: AtomicUsize,
    /// When set, each restore waits for it before it runs.
    restore_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, each delete of a name waits for it before it runs.
    delete_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, each copy waits for it before it runs.
    copy_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, the shutdown of the store waits for it.
    shutdown_gate: Mutex<Option<Arc<Gate>>>,
    /// When set, each restore fails.
    restores_fail: std::sync::atomic::AtomicBool,
    /// The names that the saves were given, with the content of the tree of each call.
    saved: Mutex<Vec<SavedTree>>,
    /// The parent of each save.
    parents: Mutex<Vec<SavedParent>>,
    deletes: Mutex<Vec<Box<str>>>,
    all_deletes: AtomicUsize,
    restores_now: AtomicUsize,
    most_restores_at_once: AtomicUsize,
    store_calls_now: AtomicUsize,
    most_store_calls_at_once: AtomicUsize,
    /// The time of each saved name.
    times: SpacedTimes,
    /// The registry of the service over the store. Each write that arrives while no work of its
    /// agent is counted there fails, and is recorded in `unbusy_writes`.
    registry: Mutex<Option<Arc<Registry>>>,
    /// The writes that arrived while no work of their agent was counted.
    unbusy_writes: Mutex<Vec<&'static str>>,
}

impl Drop for ScriptedStore {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_eq!(
                *self.unbusy_writes.lock().unwrap(),
                Vec::<&'static str>::new(),
                "every store write arrives while its agent is busy"
            );
        }
    }
}

/// The time of the first save of a scripted store.
const FIRST_SAVE_MILLIS: u64 = SpacedTimes::FIRST_MILLIS;

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
    /// Holds each save of `agent` at the gate that this gives, until the test opens it.
    fn hold_saves_of(&self, agent: &AgentSnapshots) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        self.held_saves
            .lock()
            .unwrap()
            .push((agent.clone(), Arc::clone(&gate)));
        gate
    }

    /// Runs each save of `agent` in a task of its own that waits at the gate that this gives, and
    /// that goes on when the caller drops the call.
    fn detach_saves_of(&self, agent: &AgentSnapshots) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        self.detached_saves
            .lock()
            .unwrap()
            .push((agent.clone(), Arc::clone(&gate)));
        gate
    }

    /// The content of the tree of each save call, in the order of the calls.
    fn saved_contents(&self) -> Vec<Vec<u8>> {
        self.saved
            .lock()
            .unwrap()
            .iter()
            .map(|(_, content)| content.to_vec())
            .collect()
    }

    fn saved_names(&self) -> Vec<String> {
        self.saved
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Checks that the work of each of `agents` is counted, as each write of the service must be.
    /// A write that is not counted is recorded.
    fn check_busy(&self, operation: &'static str, agents: &[&AgentSnapshots]) -> bool {
        let registry = self.registry.lock().unwrap().clone();
        let unbusy = registry.is_some_and(|registry| {
            agents
                .iter()
                .any(|agent| registry.read(|state| rules::busy(state, agent)) == 0)
        });
        if unbusy {
            self.unbusy_writes.lock().unwrap().push(operation);
        }
        !unbusy
    }

    /// Runs `run` as the runs of one call with the limiter `slots`: each run takes a slot first, a
    /// run that fails is followed by a wait of [`scripted_wait`] with no slot, and a withdrawal at
    /// a take or in a wait ends the call with `Stopped`, or with the last failure when the cause is
    /// the deadline and a run failed. The call answers the last failure after its runs. A run whose
    /// write lands late is followed by the wait for that write with no slot, which tells the
    /// limiter nothing; then the call answers what the check of its own name found, or goes on as
    /// after a failed run. After a shutdown, a call gives `Stopped`.
    async fn runs<T, E, F, R>(
        &self,
        slots: &dyn RunSlots,
        failed: fn(Failed) -> E,
        stopped: fn(Withdrawal) -> E,
        mut run: impl FnMut() -> F,
    ) -> Result<T, E>
    where
        F: Future<Output = R>,
        R: Into<ScriptedRun<T, E>>,
    {
        if self.shut_down.load(Ordering::SeqCst) {
            return Err(stopped(Withdrawal::Stopped));
        }
        let most = self.runs.load(Ordering::SeqCst).max(1);
        let ended = futures::stream::unfold(
            Some((1usize, None::<anyhow::Error>, &mut run)),
            |state| async move {
                let (number, last, run) = state?;
                let immediate = number == 1;
                let slot = match slots.take(immediate).await {
                    Ok(slot) => slot,
                    Err(cause) => {
                        return Some((Some(withdrawn(cause, last, failed, stopped)), None));
                    }
                };
                self.runs_started.fetch_add(1, Ordering::SeqCst);
                let ran = run().await.into();
                drop(slot);
                let ran = match ran {
                    ScriptedRun::Answered(answer) => {
                        if self.reports_late_on_success.load(Ordering::SeqCst) {
                            slots.waiting_for_late_writes();
                        }
                        Ok(answer)
                    }
                    ScriptedRun::Failed(failure) => Err(failure),
                    ScriptedRun::Late { failure, landed } => {
                        // The call waits for a write that can still land, as the shell of the
                        // store reports it before its waits.
                        slots.waiting_for_late_writes();
                        self.late_waits.fetch_add(1, Ordering::SeqCst);
                        let gate = self.late_gate.lock().unwrap().clone();
                        if let Some(gate) = gate {
                            gate.pass().await;
                        }
                        self.late_waits.fetch_sub(1, Ordering::SeqCst);
                        landed.ok_or(failure)
                    }
                };
                match ran {
                    Ok(answer) => Some((Some(answer), None)),
                    Err(failure) if number >= most => {
                        Some((Some(Err(failed(Failed::new(failure)))), None))
                    }
                    Err(failure) => {
                        slots.waiting_after_failure();
                        tokio::select! {
                            biased;
                            cause = slots.withdrawn() => Some((
                                Some(withdrawn(cause, Some(failure), failed, stopped)),
                                None,
                            )),
                            () = tokio::time::sleep(scripted_wait(number)) => {
                                Some((None, Some((number + 1, Some(failure), run))))
                            }
                        }
                    }
                }
            },
        );
        std::pin::pin!(ended.filter_map(|ended| async move { ended }))
            .next()
            .await
            .unwrap_or_else(|| Err(stopped(Withdrawal::Stopped)))
    }

    fn enter(&self) -> CallCount<'_> {
        let now = self.store_calls_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_store_calls_at_once
            .fetch_max(now, Ordering::SeqCst);
        CallCount(&self.store_calls_now)
    }
}

/// What one scripted run gave.
enum ScriptedRun<T, E> {
    Answered(Result<T, E>),
    /// The run failed, and a new run can succeed.
    Failed(anyhow::Error),
    /// The run failed with a write that lands late. `landed` is what the check of the own name
    /// finds after the wait: the answer when the write landed.
    Late {
        failure: anyhow::Error,
        landed: Option<Result<T, E>>,
    },
}

impl<T, E> From<Result<Result<T, E>, anyhow::Error>> for ScriptedRun<T, E> {
    fn from(ran: Result<Result<T, E>, anyhow::Error>) -> Self {
        match ran {
            Ok(answer) => Self::Answered(answer),
            Err(failure) => Self::Failed(failure),
        }
    }
}

/// Counts one store call that saves or deletes while it lives.
struct CallCount<'a>(&'a AtomicUsize);

impl Drop for CallCount<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The wait of a scripted call after its run number `failed_run` failed: 2 s, then four times the
/// wait before, at most 120 s, as the runs of the store wait.
fn scripted_wait(failed_run: usize) -> Duration {
    (Duration::from_secs(2)
        * 4u32.saturating_pow(u32::try_from(failed_run).unwrap_or(u32::MAX) - 1))
    .min(Duration::from_secs(120))
}

/// The answer of a scripted call that a withdrawal ended after `last`, the failure of its last
/// run.
fn withdrawn<T, E>(
    cause: Withdrawal,
    last: Option<anyhow::Error>,
    failed: fn(Failed) -> E,
    stopped: fn(Withdrawal) -> E,
) -> Result<T, E> {
    match (cause, last) {
        (Withdrawal::Deadline, Some(last)) => Err(failed(Failed::new(last))),
        (cause, _) => Err(stopped(cause)),
    }
}

/// The failure of a scripted write that arrived while its agent was not busy.
fn unbusy(operation: &str) -> Failed {
    Failed::new(anyhow::anyhow!(
        "a {operation} arrived while its agent was not busy"
    ))
}

#[async_trait]
impl FilesystemSnapshotStore for ScriptedStore {
    async fn save(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, SaveError> {
        self.save_calls.fetch_add(1, Ordering::SeqCst);
        self.save_cancels.lock().unwrap().push(cancel.clone());
        self.runs(slots, SaveError::Failed, SaveError::Stopped, || {
            self.save_run(agent, name, tree, parent, cancel)
        })
        .await
    }

    async fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, RestoreFailure> {
        self.runs(
            slots,
            RestoreFailure::Failed,
            RestoreFailure::Stopped,
            || async {
                let now = self.restores_now.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_restores_at_once.fetch_max(now, Ordering::SeqCst);
                let gate = self.restore_gate.lock().unwrap().clone();
                if let Some(gate) = gate {
                    gate.pass().await;
                }
                let failing = self
                    .failing_restores
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        left.checked_sub(1)
                    })
                    .is_ok();
                let result = if failing {
                    Err(anyhow::anyhow!("the restore failed"))
                } else if self.restores_fail.load(Ordering::SeqCst) {
                    Ok(Err(RestoreFailure::Corrupt(anyhow::anyhow!("corrupt"))))
                } else {
                    Ok(self.memory.restore(agent, name, into, &Unlimited).await)
                };
                self.restores_now.fetch_sub(1, Ordering::SeqCst);
                result
            },
        )
        .await
    }

    async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, ReadError> {
        self.memory.stat(agent, name).await
    }

    async fn list(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError> {
        self.runs(slots, CallError::Failed, CallError::Stopped, || async {
            self.lists.fetch_add(1, Ordering::SeqCst);
            Ok(self.memory.list(agent, &Unlimited).await.map(|listed| {
                listed
                    .iter()
                    .map(|(name, info)| (name.clone(), self.times.known(name, *info)))
                    .collect()
            }))
        })
        .await
    }

    async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: &[SnapshotName],
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.runs(slots, CallError::Failed, CallError::Stopped, || async {
            if !self.check_busy("delete", &[agent]) {
                return Ok(Err(CallError::Failed(unbusy("delete"))));
            }
            let gate = self.delete_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.pass().await;
            }
            let _count = self.enter();
            tokio::task::yield_now().await;
            if self
                .failing_deletes
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
            {
                self.failed_deletes.fetch_add(1, Ordering::SeqCst);
                return Err(anyhow::anyhow!("the delete failed"));
            }
            self.deletes
                .lock()
                .unwrap()
                .extend(names.iter().map(|name| Box::from(name.as_str())));
            Ok(self.memory.delete(agent, names, &Unlimited).await)
        })
        .await
    }

    async fn delete_all(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.runs(slots, CallError::Failed, CallError::Stopped, || async {
            if !self.check_busy("delete_all", &[agent]) {
                return Ok(Err(CallError::Failed(unbusy("delete_all"))));
            }
            let _count = self.enter();
            self.all_deletes.fetch_add(1, Ordering::SeqCst);
            if self.all_deletes_fail.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("the agent delete failed"));
            }
            Ok(self.memory.delete_all(agent, &Unlimited).await)
        })
        .await
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.runs(slots, CallError::Failed, CallError::Stopped, || async {
            if !self.check_busy("copy_all", &[from, to]) {
                return Ok(Err(CallError::Failed(unbusy("copy_all"))));
            }
            let gate = self.copy_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.pass().await;
            }
            Ok(self.memory.copy_all(from, to, &Unlimited).await)
        })
        .await
    }

    async fn shut_down(&self) {
        let gate = self.shutdown_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        self.shut_down.store(true, Ordering::SeqCst);
        self.memory.shut_down().await;
    }
}

impl ScriptedStore {
    /// One run of a save. A failure that a new run can fix is the error of the outer result.
    async fn save_run(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
    ) -> ScriptedRun<SnapshotInfo, SaveError> {
        if !self.check_busy("save", &[agent]) {
            return ScriptedRun::Answered(Err(SaveError::Failed(unbusy("save"))));
        }
        let detached = self
            .detached_saves
            .lock()
            .unwrap()
            .iter()
            .find(|(detached, _)| detached == agent)
            .map(|(_, gate)| Arc::clone(gate));
        if let Some(gate) = detached {
            let memory = self.memory.clone();
            let ended = Arc::clone(&self.detached_saves_ended);
            let (agent, name) = (agent.clone(), name.clone());
            let (tree, cancel) = (tree.to_path_buf(), cancel.clone());
            return tokio::spawn(async move {
                gate.pass().await;
                let saved = memory
                    .save(&agent, &name, &tree, None, &cancel, &Unlimited)
                    .await;
                ended.fetch_add(1, Ordering::SeqCst);
                saved
            })
            .await
            .map_err(anyhow::Error::new)
            .into();
        }
        let gate = self.save_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let held = self
            .held_saves
            .lock()
            .unwrap()
            .iter()
            .find(|(held, _)| held == agent)
            .map(|(_, gate)| Arc::clone(gate));
        if let Some(gate) = held {
            gate.pass().await;
        }
        let _count = self.enter();
        let content = std::fs::read(tree.join("content")).unwrap_or_default();
        self.saved
            .lock()
            .unwrap()
            .push((Box::from(name.as_str()), content.into_boxed_slice()));
        self.parents
            .lock()
            .unwrap()
            .push(parent.map(|(name, detection)| (Box::from(name.as_str()), detection)));
        if self.saves_fail_for_good.load(Ordering::SeqCst) {
            return ScriptedRun::Answered(Err(SaveError::Source(std::io::Error::other(
                "the tree cannot be read",
            ))));
        }
        let failing = self
            .failing_saves
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failing {
            let failure = anyhow::anyhow!("the publish failed");
            if self.late_publish_never_lands.load(Ordering::SeqCst) {
                return ScriptedRun::Late {
                    failure,
                    landed: None,
                };
            }
            if !self.publish_before_failing.load(Ordering::SeqCst) {
                return ScriptedRun::Failed(failure);
            }
            // The own file of the call is the one that this run published. A name that another
            // writer holds is not the own file, and the check finds no own file then.
            let landed = self
                .memory
                .save(agent, name, tree, parent, cancel, &Unlimited)
                .await
                .ok()
                .map(|info| Ok(self.times.timed(name, info)));
            return ScriptedRun::Late { failure, landed };
        }
        ScriptedRun::Answered(
            self.memory
                .save(agent, name, tree, parent, cancel, &Unlimited)
                .await
                .map(|info| self.times.timed(name, info)),
        )
    }
}

/// A confirmer that answers with a programmed outcome and records each name.
struct ScriptedConfirmer {
    outcome: Mutex<ConfirmOutcome>,
    /// The names that a `Confirmed` answer carries.
    selectable: Mutex<Box<[FilesystemSnapshotName]>>,
    names: Mutex<Vec<FilesystemSnapshotName>>,
    /// When set, each confirmation waits for it before it answers.
    gate: Option<Arc<Gate>>,
}

impl ScriptedConfirmer {
    fn answering(outcome: ConfirmOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(outcome),
            selectable: Mutex::default(),
            names: Mutex::default(),
            gate: None,
        })
    }

    /// A confirmer that answers `outcome` after `gate` opens.
    fn answering_after(outcome: ConfirmOutcome, gate: &Arc<Gate>) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(outcome),
            selectable: Mutex::default(),
            names: Mutex::default(),
            gate: Some(Arc::clone(gate)),
        })
    }

    fn names(&self) -> Vec<FilesystemSnapshotName> {
        self.names.lock().unwrap().clone()
    }

    async fn confirm(&self, name: FilesystemSnapshotName) -> Confirmation {
        self.names.lock().unwrap().push(name);
        if let Some(gate) = &self.gate {
            gate.pass().await;
        }
        let outcome = *self.outcome.lock().unwrap();
        match outcome {
            ConfirmOutcome::Confirmed => Confirmation::Confirmed {
                selectable: self.selectable.lock().unwrap().clone(),
            },
            ConfirmOutcome::Superseded => Confirmation::Superseded,
            ConfirmOutcome::Deferred => Confirmation::Deferred,
        }
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

/// The paused time after which a test on a paused runtime fails. The runtime advances its paused
/// time when it has nothing else to do, so a test that waits for something that never comes fails
/// after this time.
const TEST_PATIENCE: Duration = Duration::from_secs(3600);

/// The wall-clock time after which a test on a paused runtime fails. A task that never gives the
/// thread of the runtime back, such as a loop that does not wait, stops the paused time, so only
/// a limit on the wall clock ends such a test.
const TEST_WALL_LIMIT: Duration = Duration::from_secs(20);

/// Runs `test` on a runtime whose time is paused, so each wait of the service ends as soon as
/// the runtime has nothing else to do. The runtime runs on a thread of its own. The test fails
/// after [`TEST_PATIENCE`] of paused time, and after [`TEST_WALL_LIMIT`] of wall-clock time; the
/// thread of a test that fails on the wall clock is left behind.
fn paused<T: Send + 'static>(test: impl std::future::Future<Output = T> + Send + 'static) -> T {
    let (running, ended) = std::sync::mpsc::channel::<()>();
    let runner = std::thread::spawn(move || {
        let _running = running;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(TEST_PATIENCE, test)
                    .await
                    .unwrap_or_else(|_| panic!("the test did not end within {TEST_PATIENCE:?}"))
            })
    });
    if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = ended.recv_timeout(TEST_WALL_LIMIT) {
        panic!("the test did not end within {TEST_WALL_LIMIT:?} of wall-clock time");
    }
    runner
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// Admits a job of `kind` for `agent` through the interface of the service. The callers ask for
/// a manual update only while no upload of the agent runs, so it does not wait.
async fn admit(
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
    kind: SnapshotKind,
) -> Admission {
    match kind {
        SnapshotKind::Periodic => snapshots
            .admit_periodic(agent, AgentMode::Durable)
            .await
            .unwrap(),
        SnapshotKind::Update => snapshots
            .admit_update(agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap(),
    }
}

/// A receiver that never reports a terminal interrupt.
fn no_interrupt() -> watch::Receiver<bool> {
    watch::channel(false).1
}

/// A receiver that never reports a lost shard.
fn no_lost_shard() -> watch::Receiver<bool> {
    watch::channel(false).1
}

fn values(max_uploads: usize, max_restores: usize) -> FilesystemSnapshotUploadValues {
    FilesystemSnapshotUploadValues {
        max_concurrent_uploads: max_uploads,
        max_concurrent_restores: max_restores,
        confirmation_wait: Duration::from_secs(60),
        store_check_limit: Duration::from_secs(5),
        capture_wait: Duration::from_secs(5),
        retained_periodic_snapshots: 2,
        retained_update_snapshots: 2,
        max_pending_deletes_per_agent: 1024,
    }
}

fn settings(max_uploads: usize, max_restores: usize) -> FilesystemSnapshotUploadConfig {
    FilesystemSnapshotUploadConfig::new(values(max_uploads, max_restores)).unwrap()
}

/// A scripted store whose calls make up to `runs` runs.
fn store_with_runs(runs: usize) -> Arc<ScriptedStore> {
    let store = ScriptedStore::default();
    store.runs.store(runs, Ordering::SeqCst);
    Arc::new(store)
}

/// A service over `store` with `settings`, and room on the volume.
fn service(
    store: &Arc<ScriptedStore>,
    settings: FilesystemSnapshotUploadConfig,
) -> AgentFilesystemSnapshots {
    service_until(store, settings, CancellationToken::new())
}

/// A service over `store` with `settings`, and room on the volume, which `shutdown` shuts down.
/// The store checks each write against the registry of the service.
fn service_until(
    store: &Arc<ScriptedStore>,
    settings: FilesystemSnapshotUploadConfig,
    shutdown: CancellationToken,
) -> AgentFilesystemSnapshots {
    let snapshots = AgentFilesystemSnapshots::enabled(
        StoreOf::Given(Arc::clone(store) as Arc<dyn FilesystemSnapshotStore>),
        settings,
        VolumeRoom::Unlimited,
        shutdown,
    );
    *store.registry.lock().unwrap() = snapshots
        .core
        .as_ref()
        .map(|core| Arc::clone(&core.registry));
    snapshots
}

fn agent_snapshots(name: &str) -> AgentSnapshots {
    AgentSnapshots::agent(
        &OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: name.to_string(),
            },
        ),
        golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
    )
}

/// The target of a fork, with a new stage id and the snapshots of that stage.
fn fork_target(name: &str) -> (OwnedAgentId, uuid::Uuid, AgentSnapshots) {
    let target = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        },
    );
    let stage_id = uuid::Uuid::new_v4();
    let stage = AgentSnapshots::agent(&target, golem_common::model::AgentFingerprint(stage_id));
    (target, stage_id, stage)
}

/// Copies the snapshots of `from` into the stage `stage_id` of `target` as a fork attempt does,
/// and publishes the stage.
async fn fork_copy(
    snapshots: &AgentFilesystemSnapshots,
    from: &AgentSnapshots,
    target: &OwnedAgentId,
    stage_id: uuid::Uuid,
) -> Result<(), CallError> {
    let copied = snapshots
        .begin_fork(from, fork_flight(&target.agent_id, [1; 32]))
        .await
        .unwrap()
        .copy(target, stage_id, None)
        .await?;
    copied
        .publish(
            |_publication| async { PublishFound::<(), ()>::Published(()) }.boxed(),
            || async { None }.boxed(),
        )
        .await
        .unwrap();
    Ok(())
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
async fn ended(snapshots: &AgentFilesystemSnapshots, agent: &AgentSnapshots) {
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
        let snapshots = service(&store, settings(4, 4));
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
            async move { admit(snapshots, agent, kind).await.name().clone() }
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
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("in-flight");
        let other = agent_snapshots("other");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .unwrap();
        let while_admitted = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .err();
        let other_agent = snapshots
            .admit_periodic(&other, AgentMode::Durable)
            .await
            .is_ok();
        admission.submit(capture(b"one", &discarded), None, confirmer(&confirm));
        gate.wait_reached(1).await;
        let while_uploading = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .err();
        gate.open();
        ended(&snapshots, &agent).await;
        let after = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("dropped");

        drop(
            snapshots
                .admit_periodic(&agent, AgentMode::Durable)
                .await
                .unwrap(),
        );
        let again = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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

        let admitted = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .without_name();
        let update = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .err();
        let restored = snapshots
            .restore(&agent, &FilesystemSnapshotName::periodic())
            .err();

        assert_eq!(
            (admitted, update, restored),
            (
                true,
                Some(UpdateNotAdmitted::WithoutName),
                Some(SnapshotsDisabled)
            )
        );
    })
}

#[test]
fn a_job_saves_discards_and_calls_the_confirmer_once_with_its_own_name() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("confirmed");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        ended(&snapshots, &agent).await;

        let listed = store
            .memory
            .list(&agent, &crate::filesystem_snapshot::Unlimited)
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
        let store = store_with_runs(3);
        store.failing_saves.store(usize::MAX, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("budget");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
        let store = store_with_runs(5);
        store.saves_fail_for_good.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("for-good");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
    let settings = FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
        retained_periodic_snapshots: 3,
        retained_update_snapshots: 2,
        ..values(4, 4)
    })
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
                    .admit_periodic(agent, AgentMode::Durable)
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
        .admit_periodic(&agent, AgentMode::Durable)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(capture(b"newest", &discarded), None, confirmer(&confirm));
    ended(&snapshots, &agent).await;

    let deletes = store.deletes.lock().unwrap()[deletes_before..]
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
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
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("retention");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let upload = |kind: SnapshotKind, index: usize| {
            let snapshots = &snapshots;
            let agent = &agent;
            let discarded = &discarded;
            let confirm = &confirm;
            async move {
                let admission = admit(snapshots, agent, kind).await;
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
                                    no_lost_shard(),
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
            .list(&agent, &crate::filesystem_snapshot::Unlimited)
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

    let periodic = names(&retention::victims(&listing, &own, &info_at(100), 2, &[]));
    let only_own = names(&retention::victims(&listing, &own, &info_at(100), 1, &[]));

    assert_eq!(periodic, vec!["p-2", "p-1"]);
    assert_eq!(only_own, vec!["p-3", "p-2", "p-1"]);
}

#[test]
fn retention_counts_update_snapshots_apart_from_periodic_ones() {
    let listing = [("u-own", 100), ("u-2", 90), ("u-1", 80), ("p-1", 70)]
        .map(|(name, minutes)| (SnapshotName::new(name).unwrap(), info_at(minutes)));
    let own = SnapshotName::new("u-own").unwrap();

    let victims = names(&retention::victims(&listing, &own, &info_at(100), 2, &[]));

    assert_eq!(victims, vec!["u-1"]);
}

#[test]
fn retention_never_deletes_a_kept_name_and_does_not_count_it() {
    let listing = [
        ("u-own", 100),
        ("u-kept", 95),
        ("u-2", 90),
        ("u-1", 80),
        ("u-old-kept", 70),
    ]
    .map(|(name, minutes)| (SnapshotName::new(name).unwrap(), info_at(minutes)));
    let own = SnapshotName::new("u-own").unwrap();
    let kept = ["u-kept", "u-old-kept"].map(|name| SnapshotName::new(name).unwrap());

    let victims = names(&retention::victims(&listing, &own, &info_at(100), 2, &kept));

    assert_eq!(victims, vec!["u-1"]);
}

#[test]
fn each_recorded_capture_counts_once_under_its_outcome() {
    let snapshots = AgentFilesystemSnapshots::disabled();

    snapshots.record_capture("captured", Duration::ZERO);
    snapshots.record_capture("captured", Duration::ZERO);
    snapshots.record_capture("unchanged", Duration::ZERO);

    assert_eq!(
        ["captured", "unchanged", "initial_files"].map(|outcome| snapshots.captures(outcome)),
        [2, 1, 0]
    );
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

    let victims = names(&retention::victims(&listing, &own, &before_own(0), 1, &[]));

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
        let settings = settings(4, 4);
        let snapshots = service(&store, settings.clone());
        let agent = agent_snapshots("capture-wait");

        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
        let snapshots = service(&store, settings(4, 4));
        let from = agent_snapshots("copied-from");
        let (target, stage_id, to) = fork_target("copied-to");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit_periodic(&from, AgentMode::Durable)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        ended(&snapshots, &from).await;

        fork_copy(&snapshots, &from, &target, stage_id)
            .await
            .unwrap();
        let listed = store
            .memory
            .list(&to, &crate::filesystem_snapshot::Unlimited)
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
        let store = store_with_runs(2);
        store.all_deletes_fail.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("deleting");

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;
        let while_deleting = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .err();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 2).await;
        let after = {
            let admitted = snapshots.admit_periodic(&agent, AgentMode::Durable).await;
            let retried = futures::stream::repeat(())
                .then(|()| tokio::time::sleep(Duration::from_millis(5)))
                .then(|()| snapshots.admit_periodic(&agent, AgentMode::Durable))
                .filter_map(|admitted| std::future::ready(admitted.is_ok().then_some(())));
            match admitted {
                Admitted::Upload(_) => true,
                Admitted::WithoutName | Admitted::Skip(_) => {
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
        let snapshots = service(&store, settings(4, 4));
        let core = snapshots.core.as_ref().unwrap();
        let agent = agent_snapshots("decided");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
    agent: &AgentSnapshots,
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
    let snapshots = service(&store, settings(4, 4));
    let discarded = Arc::new(AtomicUsize::new(0));
    let admission = snapshots
        .admit_periodic(agent, AgentMode::Durable)
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
        let admitted = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .err();
        let waited = started.elapsed();
        gate.open();
        ended(&snapshots, &agent).await;

        assert_eq!(
            admitted,
            Some(UpdateNotAdmitted::Skip(SnapshotSkip::UploadInFlight))
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
        let admitting = snapshots.admit_update(&agent, AgentMode::Durable, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (admitted, ()) = futures::join!(admitting, opening);
        let waited = started.elapsed();
        let without_job = snapshots
            .admit_update(
                &agent_snapshots("update-without-job"),
                AgentMode::Durable,
                no_interrupt(),
            )
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

        let admitting = snapshots.admit_update(&agent, AgentMode::Durable, interrupted);
        let interrupting = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            interrupt.send_replace(true);
        };
        let (admitted, ()) = futures::join!(admitting, interrupting);
        gate.open();

        assert_eq!(admitted.err(), Some(UpdateNotAdmitted::Interrupted));
    })
}

#[test]
fn a_start_without_a_running_upload_asks_the_store_at_once() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("start-without-upload");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
fn delete_all_snapshots_cancels_the_job_and_deletes_them_after_its_save_returned() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let save_gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&save_gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("delete-all");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .unwrap();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        save_gate.wait_reached(1).await;

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        ended(&snapshots, &agent).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let while_the_save_runs = (
            store.all_deletes.load(Ordering::SeqCst),
            discarded.load(Ordering::SeqCst),
        );
        save_gate.open();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;

        assert_eq!(while_the_save_runs, (0, 0));
        assert!(confirm.names().is_empty());
        assert!(store.deletes.lock().unwrap().is_empty());
        assert_eq!(discarded.load(Ordering::SeqCst), 1);
        assert!(
            store
                .memory
                .list(&agent, &crate::filesystem_snapshot::Unlimited)
                .await
                .unwrap()
                .is_empty()
        );
    })
}

#[test]
fn a_cancelled_confirmation_deletes_nothing() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        let confirm = Arc::new(ScriptedConfirmer {
            outcome: Mutex::new(ConfirmOutcome::Confirmed),
            selectable: Mutex::default(),
            names: Mutex::default(),
            gate: Some(Arc::clone(&gate)),
        });
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("cancelled-confirmation");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .unwrap();
        let name = admission.name().clone();
        admission.submit(capture(b"tree", &discarded), None, confirmer(&confirm));
        gate.wait_reached(1).await;

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        ended(&snapshots, &agent).await;
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;

        assert!(
            !store
                .deletes
                .lock()
                .unwrap()
                .contains(&Box::from(name.as_str()))
        );
        assert!(
            store
                .memory
                .list(&agent, &crate::filesystem_snapshot::Unlimited)
                .await
                .unwrap()
                .is_empty()
        );
    })
}

#[test]
fn a_store_that_fails_every_delete_of_all_snapshots_leaves_the_service_running() {
    paused(async {
        let store = store_with_runs(3);
        store.all_deletes_fail.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("failing-delete");

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 3).await;
        let discarded = Arc::new(AtomicUsize::new(0));
        let saved = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap()
            .upload_now(
                capture(b"after", &discarded),
                no_interrupt(),
                no_lost_shard(),
            )
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
        let snapshots = service(&store, settings(1, 4));
        let uploading = agent_snapshots("uploading");
        let deleted_names = agent_snapshots("deleted-names");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit_periodic(&uploading, AgentMode::Durable)
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
        snapshots.delete_all_snapshots(&agent_snapshots("deleted"), AgentMode::Durable);
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
        let snapshots = service(&store, settings(4, 2));
        let agent = agent_snapshots("restores");
        let discarded = Arc::new(AtomicUsize::new(0));
        let name = {
            let admission = snapshots
                .admit_update(&agent, AgentMode::Durable, no_interrupt())
                .await
                .unwrap();
            let name = admission.name().clone();
            drop(
                admission
                    .upload_now(
                        capture(b"tree", &discarded),
                        no_interrupt(),
                        no_lost_shard(),
                    )
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
        let snapshots = service(&store, settings(4, 2));
        let agent = agent_snapshots("restore");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let name = admission.name().clone();
        drop(
            admission
                .upload_now(
                    capture(b"restored", &discarded),
                    no_interrupt(),
                    no_lost_shard(),
                )
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
        let store = store_with_runs(3);
        // The first save publishes its tree and then fails, as a PUT that lands after its error does.
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(true, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("names-of-failed-saves");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let first = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
        let second = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let second_name = second.name().clone();
        let second_saved = second
            .upload_now(
                capture(b"second tree", &discarded),
                no_interrupt(),
                no_lost_shard(),
            )
            .await
            .is_ok();

        let trees_of_names = store.saved.lock().unwrap().iter().fold(
            BTreeMap::<String, std::collections::BTreeSet<Vec<u8>>>::new(),
            |mut names, (name, tree)| {
                names
                    .entry(name.to_string())
                    .or_default()
                    .insert(tree.to_vec());
                names
            },
        );
        assert_ne!(first_name, second_name);
        assert!(second_saved);
        // The first job made one run: the call waited for its late publish and found its own
        // file. The second job ran again with its own name after its failed run.
        assert_eq!(
            store.saved_names(),
            vec![
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
        let snapshots = service_until(&store, settings(4, 4), shutdown.clone());
        let agent = agent_snapshots("shutdown");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
        assert!(
            snapshots
                .admit_periodic(&agent, AgentMode::Durable)
                .await
                .is_ok()
        );
    })
}

#[test]
fn a_parent_reaches_the_store_with_its_detection() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("parent");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let parent = FilesystemSnapshotName::periodic();

        let admission = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
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
                Box::from(parent.as_str()),
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
    agent: &AgentSnapshots,
) -> (String, Vec<String>) {
    let discarded = Arc::new(AtomicUsize::new(0));
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
    let periodic = snapshots
        .admit_periodic(agent, AgentMode::Durable)
        .await
        .unwrap();
    let periodic_name = periodic.name().as_str().to_string();
    periodic.submit(capture(b"periodic", &discarded), None, confirmer(&confirm));
    ended(snapshots, agent).await;
    let updates = futures::stream::iter(0..3)
        .then(|index| {
            let discarded = &discarded;
            async move {
                let admission = snapshots
                    .admit_update(agent, AgentMode::Durable, no_interrupt())
                    .await
                    .unwrap();
                let name = admission.name().as_str().to_string();
                admission
                    .upload_now(
                        capture(format!("{index}").as_bytes(), discarded),
                        no_interrupt(),
                        no_lost_shard(),
                    )
                    .await
                    .unwrap()
                    .delete_older_snapshots(&[]);
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
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("update-retention");

        let (periodic, updates) = update_uploads_with_retention(&snapshots, &agent).await;

        let kept = store
            .memory
            .list(&agent, &crate::filesystem_snapshot::Unlimited)
            .await
            .unwrap()
            .iter()
            .map(|(name, _)| name.as_str().to_string())
            .collect::<std::collections::BTreeSet<_>>();
        let expected = [periodic, updates[1].clone(), updates[2].clone()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(kept, expected);
        assert_eq!(
            *store.deletes.lock().unwrap(),
            vec![Box::from(updates[0].as_str())]
        );
    })
}

#[test]
fn a_dropped_update_retention_deletes_nothing_and_frees_the_agent() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("dropped-update-retention");
        let discarded = Arc::new(AtomicUsize::new(0));

        let names = futures::stream::iter(0..3)
            .then(|index| {
                let snapshots = &snapshots;
                let agent = &agent;
                let discarded = &discarded;
                async move {
                    let admission = snapshots
                        .admit_update(agent, AgentMode::Durable, no_interrupt())
                        .await
                        .unwrap();
                    let name = admission.name().as_str().to_string();
                    let retention = admission
                        .upload_now(
                            capture(format!("{index}").as_bytes(), discarded),
                            no_interrupt(),
                            no_lost_shard(),
                        )
                        .await
                        .unwrap();
                    let while_held = snapshots
                        .admit_periodic(agent, AgentMode::Durable)
                        .await
                        .err();
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
                .admit_periodic(&agent, AgentMode::Durable)
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
    AgentSnapshots,
    Arc<Gate>,
) {
    let store = Arc::new(ScriptedStore::default());
    let settings = FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
        retained_periodic_snapshots: 1,
        retained_update_snapshots: 1,
        ..values(4, 4)
    })
    .unwrap();
    let snapshots = service(&store, settings);
    let agent = agent_snapshots("held-delete");
    let discarded = Arc::new(AtomicUsize::new(0));
    let deferred = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    let older = snapshots
        .admit_periodic(&agent, AgentMode::Durable)
        .await
        .unwrap();
    older.submit(capture(b"older", &discarded), None, confirmer(&deferred));
    ended(&snapshots, &agent).await;
    let gate = Arc::new(Gate::default());
    *store.delete_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let answering = ScriptedConfirmer::answering(outcome);
    let newest = snapshots
        .admit_periodic(&agent, AgentMode::Durable)
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

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
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
fn upload_answers_stopped_at_once_and_discards_the_capture_after_the_save_returned() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let core = snapshots.core.as_ref().unwrap();
        let agent = agent_snapshots("stopped-upload-now");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let (stop, stopped) = watch::channel(false);

        let uploading =
            admission.upload_now(capture(b"tree", &discarded), stopped, no_lost_shard());
        let stopping = async {
            gate.wait_reached(1).await;
            stop.send_replace(true);
        };
        let (uploaded, ()) = futures::join!(uploading, stopping);
        let after_the_answer = (
            discarded.load(Ordering::SeqCst),
            core.calls.upload_attempts(),
            snapshots
                .admit_periodic(&agent, AgentMode::Durable)
                .await
                .is_ok(),
        );
        gate.open();
        eventually(|| discarded.load(Ordering::SeqCst) == 1).await;

        assert!(matches!(uploaded, Err(UploadNowError::Stopped)));
        assert_eq!(after_the_answer, (0, 1, true));
        assert_eq!(core.calls.upload_attempts(), 0);
    })
}

/// Admits a periodic job of `agent` and submits the tree `content` with `confirm`. Gives the name
/// of the job.
async fn submit(
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
    content: &[u8],
    confirm: Confirm,
) -> FilesystemSnapshotName {
    let admission = snapshots
        .admit_periodic(agent, AgentMode::Durable)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(
        capture(content, &Arc::new(AtomicUsize::new(0))),
        None,
        confirm,
    );
    name
}

#[test]
fn a_manual_update_cuts_the_deletes_of_a_running_job_and_never_its_save() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("update-during-save");
        let gate = store.hold_saves_of(&agent);
        let snapshots = service(&store, settings(4, 4));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"tree", confirmer(&confirm)).await;
        gate.wait_reached(1).await;

        let admitting = snapshots.admit_update(&agent, AgentMode::Durable, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (admitted, ()) = futures::join!(admitting, opening);

        assert!(admitted.is_ok());
        assert_eq!(confirm.names().len(), 1);
        assert_eq!(store.lists.load(Ordering::SeqCst), 0);
        assert!(store.deletes.lock().unwrap().is_empty());
    })
}

#[test]
fn an_upload_whose_slots_are_gone_stops_and_frees_the_agent() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let holder = agent_snapshots("slot-holder");
        let gate = store.hold_saves_of(&holder);
        let snapshots = service(&store, settings(1, 4));
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        gate.wait_reached(1).await;
        let agent = agent_snapshots("closed-slots");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"waiting", confirmer(&confirm)).await;
        tokio::time::sleep(Duration::from_secs(1)).await;

        snapshots.core.as_ref().unwrap().calls.close_slots();
        ended(&snapshots, &agent).await;
        gate.open();

        assert!(confirm.names().is_empty());
        assert!(!store.saved_contents().contains(&b"waiting".to_vec()));
    })
}

#[test]
fn the_delete_of_a_superseded_snapshot_waits_for_a_slot_of_the_uploads() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(1, 4));
        let agent = agent_snapshots("superseded-waits");
        let confirm_gate = Arc::new(Gate::default());
        let superseded =
            ScriptedConfirmer::answering_after(ConfirmOutcome::Superseded, &confirm_gate);
        let name = submit(&snapshots, &agent, b"superseded", confirmer(&superseded)).await;
        confirm_gate.wait_reached(1).await;
        let holder = agent_snapshots("slot-holder");
        let gate = store.hold_saves_of(&holder);
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        gate.wait_reached(1).await;

        confirm_gate.open();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_held = store.deletes.lock().unwrap().clone();
        gate.open();
        ended(&snapshots, &agent).await;

        assert!(
            while_held.is_empty(),
            "deleted {while_held:?} without a slot"
        );
        assert_eq!(
            *store.deletes.lock().unwrap(),
            vec![Box::from(name.as_str())]
        );
    })
}

#[test]
fn a_call_that_runs_on_keeps_its_slot_until_it_returns() {
    paused(async {
        let agent = agent_snapshots("stopped-during-save");
        let (_store, snapshots, gate, _) =
            with_a_save_held(&agent, ConfirmOutcome::Confirmed).await;
        let core = snapshots.core.as_ref().unwrap();
        let during = (core.calls.upload_attempts(), core.calls.free_slots());

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        ended(&snapshots, &agent).await;
        let after_the_stop = (core.calls.upload_attempts(), core.calls.free_slots());
        gate.open();
        eventually(|| core.calls.free_slots() == 4).await;

        assert_eq!((during, after_the_stop), ((1, 3), (1, 3)));
        assert_eq!(core.calls.upload_attempts(), 0);
    })
}

/// A service with one slot of the uploads, and another agent whose saves wait at the gate that
/// this gives, so that its save holds the slot.
fn one_slot_with_a_held_agent() -> (
    Arc<ScriptedStore>,
    AgentFilesystemSnapshots,
    AgentSnapshots,
    Arc<Gate>,
) {
    let store = store_with_runs(3);
    let holder = agent_snapshots("slot-holder");
    let gate = store.hold_saves_of(&holder);
    let snapshots = service(&store, settings(1, 4));
    (store, snapshots, holder, gate)
}

/// The time that a manual update of `agent` takes to be admitted, and whether it was.
async fn update_admission_time(
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
) -> (bool, Duration) {
    let started = tokio::time::Instant::now();
    let admitted = snapshots
        .admit_update(agent, AgentMode::Durable, no_interrupt())
        .await
        .is_ok();
    (admitted, started.elapsed())
}

#[test]
fn a_manual_update_that_arrives_while_the_job_saves_does_not_wait_for_its_retention_slot() {
    paused(async {
        let (store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let agent = agent_snapshots("update-while-saving");
        let job_gate = store.hold_saves_of(&agent);
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"tree", confirmer(&confirm)).await;
        job_gate.wait_reached(1).await;
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;

        let admitting = update_admission_time(&snapshots, &agent);
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            job_gate.open();
        };
        let ((admitted, waited), ()) = futures::join!(admitting, opening);
        holder_gate.open();

        assert!(admitted);
        assert!(waited < Duration::from_secs(10), "waited {waited:?}");
    })
}

#[test]
fn a_manual_update_ends_the_wait_of_a_superseded_delete_for_its_slot() {
    paused(async {
        let (store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let agent = agent_snapshots("update-after-superseded");
        let job_gate = store.hold_saves_of(&agent);
        let superseded = ScriptedConfirmer::answering(ConfirmOutcome::Superseded);
        submit(&snapshots, &agent, b"tree", confirmer(&superseded)).await;
        job_gate.wait_reached(1).await;
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        job_gate.open();
        holder_gate.wait_reached(1).await;
        tokio::time::sleep(Duration::from_secs(1)).await;

        let (admitted, waited) = update_admission_time(&snapshots, &agent).await;
        holder_gate.open();

        assert_eq!(superseded.names().len(), 1);
        assert!(admitted);
        assert!(waited < Duration::from_secs(10), "waited {waited:?}");
    })
}

#[test]
fn a_manual_update_ends_the_wait_of_the_retention_of_an_earlier_update_for_its_slot() {
    paused(async {
        let (_store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let agent = agent_snapshots("update-after-update");
        let first = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let saved = first
            .upload_now(
                capture(b"first", &Arc::new(AtomicUsize::new(0))),
                no_interrupt(),
                no_lost_shard(),
            )
            .await
            .unwrap();
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        holder_gate.wait_reached(1).await;
        saved.delete_older_snapshots(&[]);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let (admitted, waited) = update_admission_time(&snapshots, &agent).await;
        holder_gate.open();

        assert!(admitted);
        assert!(waited < Duration::from_secs(10), "waited {waited:?}");
    })
}

#[test]
fn a_manual_update_ends_a_delete_that_waits_between_two_attempts() {
    paused(async {
        let (store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let agent = agent_snapshots("update-during-backoff");
        let confirmed = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        futures::stream::iter([b"one".as_slice(), b"two".as_slice()])
            .for_each(|content| {
                let (snapshots, agent, confirmed) = (&snapshots, &agent, &confirmed);
                async move {
                    submit(snapshots, agent, content, confirmer(confirmed)).await;
                    ended(snapshots, agent).await;
                }
            })
            .await;
        store.failing_deletes.store(1, Ordering::SeqCst);
        submit(&snapshots, &agent, b"three", confirmer(&confirmed)).await;
        eventually(|| store.failed_deletes.load(Ordering::SeqCst) == 1).await;
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;

        let (admitted, waited) = update_admission_time(&snapshots, &agent).await;
        holder_gate.open();

        assert!(admitted);
        assert!(waited < Duration::from_secs(1), "waited {waited:?}");
        assert!(store.deletes.lock().unwrap().is_empty());
    })
}

#[test]
fn a_save_of_another_agent_runs_while_an_upload_waits_between_two_attempts() {
    paused(async {
        let store = store_with_runs(2);
        store.failing_saves.store(1, Ordering::SeqCst);
        let snapshots = service(&store, settings(1, 4));
        let retrying_agent = agent_snapshots("retrying");
        let other = agent_snapshots("other");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        submit(&snapshots, &retrying_agent, b"first", confirmer(&confirm)).await;
        eventually(|| store.saved_contents().len() == 1).await;
        submit(&snapshots, &other, b"second", confirmer(&confirm)).await;
        ended(&snapshots, &retrying_agent).await;
        ended(&snapshots, &other).await;

        assert_eq!(
            store.saved_contents(),
            vec![b"first".to_vec(), b"second".to_vec(), b"first".to_vec()]
        );
    })
}

#[test]
fn delete_all_snapshots_starts_only_after_the_held_save_ended_and_leaves_no_snapshot() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("held-detached-save");
        let gate = store.detach_saves_of(&agent);
        let snapshots = service(&store, settings(4, 4));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"tree", confirmer(&confirm)).await;
        gate.wait_reached(1).await;

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_the_save_runs = store.all_deletes.load(Ordering::SeqCst);
        gate.open();
        eventually(|| {
            store.detached_saves_ended.load(Ordering::SeqCst) == 1
                && store.all_deletes.load(Ordering::SeqCst) == 1
        })
        .await;

        assert_eq!(while_the_save_runs, 0);
        assert!(
            store
                .memory
                .list(&agent, &crate::filesystem_snapshot::Unlimited)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(confirm.names().is_empty());
    })
}

/// Whether a save of `agent` runs now, as the registry of `snapshots` counts it.
fn save_running(snapshots: &AgentFilesystemSnapshots, agent: &AgentSnapshots) -> bool {
    snapshots.core.as_ref().is_some_and(|core| {
        core.registry
            .read(|state| rules::save_running(state, agent))
    })
}

/// Admits a periodic job of `agent`, submits the tree `content` that `discarded` counts, and
/// gives the name of the job.
async fn submit_counted(
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
    content: &[u8],
    discarded: &Arc<AtomicUsize>,
    confirm: Confirm,
) -> FilesystemSnapshotName {
    let admission = snapshots
        .admit_periodic(agent, AgentMode::Durable)
        .await
        .unwrap();
    let name = admission.name().clone();
    admission.submit(capture(content, discarded), None, confirm);
    name
}

#[test]
fn a_stop_at_each_wait_before_the_save_call_leaves_no_save_flag() {
    paused(async {
        // The wait for a slot: the slot is held by the save of another agent.
        let (store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        holder_gate.wait_reached(1).await;
        let waiting_for_a_slot = agent_snapshots("waits-for-a-slot");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        submit(
            &snapshots,
            &waiting_for_a_slot,
            b"slot",
            confirmer(&confirm),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        snapshots.delete_all_snapshots(&waiting_for_a_slot, AgentMode::Durable);
        ended(&snapshots, &waiting_for_a_slot).await;
        let after_the_slot_wait = save_running(&snapshots, &waiting_for_a_slot);
        holder_gate.open();
        ended(&snapshots, &holder).await;

        // The wait for the running save of the agent: the save of a stopped job runs on.
        let agent = agent_snapshots("waits-for-a-save");
        let save_gate = store.hold_saves_of(&agent);
        let first = submit(&snapshots, &agent, b"first", confirmer(&confirm)).await;
        save_gate.wait_reached(1).await;
        snapshots.delete_snapshots(&agent, Box::new([first.clone()]));
        ended(&snapshots, &agent).await;
        let second = submit(&snapshots, &agent, b"second", confirmer(&confirm)).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        snapshots.delete_snapshots(&agent, Box::new([second]));
        ended(&snapshots, &agent).await;
        let while_the_first_runs = save_running(&snapshots, &agent);
        save_gate.open();
        eventually(|| !save_running(&snapshots, &agent)).await;

        // The wait between two attempts.
        let retrying = agent_snapshots("waits-to-retry");
        store.failing_saves.store(1, Ordering::SeqCst);
        let saves_before = store.saved_contents().len();
        submit(&snapshots, &retrying, b"retry", confirmer(&confirm)).await;
        eventually(|| store.saved_contents().len() == saves_before + 1).await;
        snapshots.delete_all_snapshots(&retrying, AgentMode::Durable);
        ended(&snapshots, &retrying).await;
        tokio::time::sleep(Duration::from_secs(60)).await;

        assert!(!after_the_slot_wait);
        assert!(while_the_first_runs);
        assert!(!save_running(&snapshots, &retrying));
        assert_eq!(
            store.saved_contents()[1..],
            [b"first".to_vec(), b"retry".to_vec()]
        );
        assert!(confirm.names().is_empty());
    })
}

#[test]
fn a_start_waits_only_for_a_job_that_got_a_slot() {
    paused(async {
        let (_store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        holder_gate.wait_reached(1).await;
        let agent = agent_snapshots("no-slot-yet");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        let name = submit(&snapshots, &agent, b"waiting", confirmer(&confirm)).await;
        tokio::time::sleep(Duration::from_secs(1)).await;

        let started = tokio::time::Instant::now();
        let checked = snapshots.prepare_start(&agent, &name, no_interrupt()).await;
        let waited = started.elapsed();
        holder_gate.open();

        assert_eq!((checked, waited), (StartCheck::NotStored, Duration::ZERO));
    })
}

#[test]
fn a_manual_update_after_a_revert_stop_is_admitted_while_the_old_save_still_runs() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("update-after-revert");
        let gate = store.hold_saves_of(&agent);
        let snapshots = service(&store, settings(4, 4));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let periodic = submit(&snapshots, &agent, b"periodic", confirmer(&confirm)).await;
        gate.wait_reached(1).await;
        snapshots.delete_snapshots(&agent, Box::new([periodic]));

        let started = tokio::time::Instant::now();
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await;
        let admitted_after = started.elapsed();
        let uploading = async {
            admission
                .unwrap()
                .upload_now(
                    capture(b"update", &Arc::new(AtomicUsize::new(0))),
                    no_interrupt(),
                    no_lost_shard(),
                )
                .await
                .map(drop)
        };
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (uploaded, ()) = futures::join!(uploading, opening);

        assert_eq!(admitted_after, Duration::ZERO);
        assert!(uploaded.is_ok(), "{:?}", uploaded.err());
        assert_eq!(
            store.saved_contents(),
            vec![b"periodic".to_vec(), b"update".to_vec()]
        );
        assert!(confirm.names().is_empty());
    })
}

#[test]
fn a_manual_update_gets_save_running_or_no_slot_by_its_cause() {
    paused(async {
        // A save of the agent that runs on after its job stopped.
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("update-behind-a-save");
        let gate = store.hold_saves_of(&agent);
        let snapshots = service(&store, settings(4, 4));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let periodic = submit(&snapshots, &agent, b"periodic", confirmer(&confirm)).await;
        gate.wait_reached(1).await;
        snapshots.delete_snapshots(&agent, Box::new([periodic]));
        let started = tokio::time::Instant::now();
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        let behind_a_save = admission
            .upload_now(
                capture(b"update", &Arc::new(AtomicUsize::new(0))),
                no_interrupt(),
                no_lost_shard(),
            )
            .await;
        let save_running_after = started.elapsed();
        gate.open();

        // No slot: the only slot is held by the save of another agent.
        let (_store, snapshots, holder, holder_gate) = one_slot_with_a_held_agent();
        let held = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        submit(&snapshots, &holder, b"held", confirmer(&held)).await;
        holder_gate.wait_reached(1).await;
        let started = tokio::time::Instant::now();
        let without_a_slot = snapshots
            .admit_update(
                &agent_snapshots("update-without-a-slot"),
                AgentMode::Durable,
                no_interrupt(),
            )
            .await
            .unwrap()
            .upload_now(
                capture(b"update", &Arc::new(AtomicUsize::new(0))),
                no_interrupt(),
                no_lost_shard(),
            )
            .await;
        let no_slot_after = started.elapsed();
        holder_gate.open();

        assert!(matches!(behind_a_save, Err(UploadNowError::SaveRunning)));
        assert_eq!(save_running_after, Duration::from_secs(60));
        assert!(matches!(without_a_slot, Err(UploadNowError::NoSlot)));
        assert_eq!(no_slot_after, Duration::from_secs(60));
    })
}

#[test]
fn a_manual_update_waits_for_a_running_save_at_most_the_rest_of_confirmation_wait() {
    paused(async {
        // The update first waits for the running job, then in its upload for the save that runs
        // on: both waits together end at `confirmation_wait`.
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("update-deadline");
        let gate = store.hold_saves_of(&agent);
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let periodic = submit(&snapshots, &agent, b"periodic", confirmer(&confirm)).await;
        gate.wait_reached(1).await;
        let stopping = {
            let snapshots = Arc::clone(&snapshots);
            let agent = agent.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(20)).await;
                snapshots.delete_snapshots(&agent, Box::new([periodic]));
            }
        };

        let started = tokio::time::Instant::now();
        let updating = async {
            let admission = snapshots
                .admit_update(&agent, AgentMode::Durable, no_interrupt())
                .await
                .unwrap();
            let admitted_after = started.elapsed();
            let uploaded = admission
                .upload_now(
                    capture(b"update", &Arc::new(AtomicUsize::new(0))),
                    no_interrupt(),
                    no_lost_shard(),
                )
                .await;
            (admitted_after, uploaded)
        };
        let ((admitted_after, uploaded), ()) = futures::join!(updating, stopping);
        let failed_after = started.elapsed();
        gate.open();

        assert_eq!(admitted_after, Duration::from_secs(20));
        assert!(matches!(uploaded, Err(UploadNowError::SaveRunning)));
        assert_eq!(failed_after, Duration::from_secs(60));
    })
}

#[test]
fn a_dropped_upload_now_caller_ends_the_admission_and_the_save_runs_to_its_end() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("dropped-caller");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let name = admission.name().clone();

        let dropped = tokio::time::timeout(
            Duration::from_secs(1),
            admission.upload_now(
                capture(b"tree", &discarded),
                no_interrupt(),
                no_lost_shard(),
            ),
        )
        .await;
        ended(&snapshots, &agent).await;
        let while_the_save_runs = (
            discarded.load(Ordering::SeqCst),
            save_running(&snapshots, &agent),
        );
        gate.open();
        eventually(|| discarded.load(Ordering::SeqCst) == 1).await;

        assert!(dropped.is_err());
        assert_eq!(while_the_save_runs, (0, true));
        assert!(
            store
                .memory
                .stat(&agent, &store_name(&name).unwrap())
                .await
                .unwrap()
                .is_some()
        );
    })
}

#[test]
fn a_cancelled_save_is_discarded_after_the_call_returned_and_never_published() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let core = snapshots.core.as_ref().unwrap();
        let agent = agent_snapshots("lost-shard");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let (lose, lost) = watch::channel(false);

        let uploading = admission.upload_now(capture(b"tree", &discarded), no_interrupt(), lost);
        let losing = async {
            gate.wait_reached(1).await;
            lose.send_replace(true);
        };
        let (uploaded, ()) = futures::join!(uploading, losing);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_the_save_runs = (discarded.load(Ordering::SeqCst), core.calls.free_slots());
        gate.open();
        eventually(|| discarded.load(Ordering::SeqCst) == 1).await;

        assert!(matches!(uploaded, Err(UploadNowError::Stopped)));
        assert_eq!(while_the_save_runs, (0, 3));
        assert_eq!(core.calls.free_slots(), 4);
        assert!(
            store
                .memory
                .list(&agent, &crate::filesystem_snapshot::Unlimited)
                .await
                .unwrap()
                .is_empty()
        );
    })
}

/// Uploads a tree for a manual update of a new agent with the stop `stop` and the lost shard
/// `lost`, which both already hold their value. Gives the answer, the calls of `save`, the runs
/// of the store and the free slots after the answer, and fails when the tree is not discarded.
async fn upload_after_a_stop(stop: bool, lost: bool) -> (Result<(), String>, usize, usize, usize) {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4));
    let core = snapshots.core.as_ref().unwrap();
    let agent = agent_snapshots("stopped-before-the-upload");
    let discarded = Arc::new(AtomicUsize::new(0));
    let admission = snapshots
        .admit_update(&agent, AgentMode::Durable, no_interrupt())
        .await
        .unwrap();
    let (_interrupt, interrupted) = watch::channel(stop);
    let (_lose, lost) = watch::channel(lost);

    let uploaded = admission
        .upload_now(capture(b"tree", &discarded), interrupted, lost)
        .await;
    let after_the_answer = (
        uploaded.map(drop).map_err(|error| format!("{error:?}")),
        store.save_calls.load(Ordering::SeqCst),
        store.runs_started.load(Ordering::SeqCst),
        core.calls.free_slots(),
    );
    eventually(|| discarded.load(Ordering::SeqCst) == 1).await;
    after_the_answer
}

#[test]
fn an_upload_whose_shard_is_lost_before_it_starts_answers_stopped_and_makes_no_save_call() {
    let answered = paused(upload_after_a_stop(false, true));

    assert_eq!(answered, (Err("Stopped".to_string()), 0, 0, 4));
}

#[test]
fn an_upload_whose_caller_stopped_before_it_starts_answers_stopped_and_makes_no_save_call() {
    let answered = paused(upload_after_a_stop(true, false));

    assert_eq!(answered, (Err("Stopped".to_string()), 0, 0, 4));
}

#[test]
fn a_lost_shard_during_a_save_cancels_the_save_in_the_store_before_the_upload_answers() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("lost-during-the-save");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let (lose, lost) = watch::channel(false);

        let uploading = admission.upload_now(capture(b"tree", &discarded), no_interrupt(), lost);
        let losing = async {
            gate.wait_reached(1).await;
            lose.send_replace(true);
        };
        let (uploaded, ()) = futures::join!(uploading, losing);
        let cancelled_at_the_answer = store
            .save_cancels
            .lock()
            .unwrap()
            .iter()
            .map(CancellationToken::is_cancelled)
            .collect::<Vec<_>>();
        gate.open();
        eventually(|| discarded.load(Ordering::SeqCst) == 1).await;

        assert!(matches!(uploaded, Err(UploadNowError::Stopped)));
        assert_eq!(cancelled_at_the_answer, [true]);
    })
}

#[test]
fn a_shutdown_discards_a_dropped_save_only_after_the_store_stopped() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let save_gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&save_gate));
        let shutdown_gate = Arc::new(Gate::default());
        *store.shutdown_gate.lock().unwrap() = Some(Arc::clone(&shutdown_gate));
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let agent = agent_snapshots("shutdown-discard");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit_counted(&snapshots, &agent, b"tree", &discarded, confirmer(&confirm)).await;
        save_gate.wait_reached(1).await;

        let stopping = {
            let snapshots = Arc::clone(&snapshots);
            tokio::spawn(async move { snapshots.shut_down().await })
        };
        shutdown_gate.wait_reached(1).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_the_store_stops = discarded.load(Ordering::SeqCst);
        shutdown_gate.open();
        stopping.await.unwrap();

        assert_eq!(while_the_store_stops, 0);
        assert_eq!(discarded.load(Ordering::SeqCst), 1);
        assert!(confirm.names().is_empty());
    })
}

#[test]
fn a_delete_of_all_snapshots_waits_for_a_running_delete_of_names_of_its_agent() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.delete_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("names-then-all");

        snapshots.delete_snapshots(&agent, Box::new([FilesystemSnapshotName::periodic()]));
        gate.wait_reached(1).await;
        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_the_names_run = store.all_deletes.load(Ordering::SeqCst);
        gate.open();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;

        assert_eq!(while_the_names_run, 0);
        assert_eq!(store.deletes.lock().unwrap().len(), 1);
    })
}

#[test]
fn an_all_ends_the_retry_sleep_of_a_names_of_its_agent() {
    paused(async {
        let store = store_with_runs(3);
        store.failing_deletes.store(1, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("retry-then-all");

        snapshots.delete_snapshots(&agent, Box::new([FilesystemSnapshotName::periodic()]));
        eventually(|| store.failed_deletes.load(Ordering::SeqCst) == 1).await;
        let started = tokio::time::Instant::now();
        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;
        tokio::time::sleep(Duration::from_secs(60)).await;

        assert!(started.elapsed() < Duration::from_secs(61));
        assert!(store.deletes.lock().unwrap().is_empty());
        assert_eq!(store.failed_deletes.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn a_delete_of_names_stops_the_upload_of_one_of_them_and_runs_after_its_store_call_ended() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("revert-of-a-held-save");
        let gate = store.detach_saves_of(&agent);
        let snapshots = service(&store, settings(4, 4));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let name = submit(&snapshots, &agent, b"tree", confirmer(&confirm)).await;
        gate.wait_reached(1).await;

        snapshots.delete_snapshots(&agent, Box::new([name.clone()]));
        ended(&snapshots, &agent).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_the_save_runs = store.deletes.lock().unwrap().len();
        gate.open();
        eventually(|| store.deletes.lock().unwrap().len() == 1).await;

        assert_eq!(while_the_save_runs, 0);
        assert_eq!(
            *store.deletes.lock().unwrap(),
            vec![Box::from(name.as_str())]
        );
        assert!(
            store
                .memory
                .list(&agent, &crate::filesystem_snapshot::Unlimited)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(confirm.names().is_empty());
    })
}

#[test]
fn no_admission_succeeds_while_a_job_deletes_its_older_snapshots() {
    paused(async {
        let (_store, snapshots, agent, gate) = with_a_delete_held(ConfirmOutcome::Confirmed).await;

        let while_it_deletes = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .err();
        gate.open();
        ended(&snapshots, &agent).await;

        assert_eq!(while_it_deletes, Some(SnapshotSkip::UploadInFlight));
    })
}

#[test]
fn a_confirmed_upload_keeps_the_names_of_its_confirmation_whatever_their_age() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let settings = FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
            retained_periodic_snapshots: 1,
            ..values(4, 4)
        })
        .unwrap();
        let snapshots = service(&store, settings);
        let agent = agent_snapshots("kept-by-confirmation");
        let deferred = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        let oldest = submit(&snapshots, &agent, b"oldest", confirmer(&deferred)).await;
        ended(&snapshots, &agent).await;
        let older = submit(&snapshots, &agent, b"older", confirmer(&deferred)).await;
        ended(&snapshots, &agent).await;
        let confirmed = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        *confirmed.selectable.lock().unwrap() = Box::new([oldest.clone()]);

        let newest = submit(&snapshots, &agent, b"newest", confirmer(&confirmed)).await;
        ended(&snapshots, &agent).await;

        let kept = store
            .memory
            .list(&agent, &crate::filesystem_snapshot::Unlimited)
            .await
            .unwrap()
            .iter()
            .map(|(name, _)| name.as_str().to_string())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            kept,
            [newest, oldest]
                .iter()
                .map(|name| name.as_str().to_string())
                .collect()
        );
        assert_eq!(
            *store.deletes.lock().unwrap(),
            vec![Box::from(older.as_str())]
        );
    })
}

#[test]
fn an_ephemeral_admission_is_disabled_and_makes_no_store_call() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("ephemeral");

        let periodic = snapshots
            .admit_periodic(&agent, AgentMode::Ephemeral)
            .await
            .without_name();
        let update = snapshots
            .admit_update(&agent, AgentMode::Ephemeral, no_interrupt())
            .await
            .err();
        snapshots.delete_all_snapshots(&agent, AgentMode::Ephemeral);
        tokio::time::sleep(Duration::from_secs(1)).await;

        assert_eq!(
            (periodic, update, store.all_deletes.load(Ordering::SeqCst)),
            (true, Some(UpdateNotAdmitted::WithoutName), 0)
        );
        assert!(store.saved_names().is_empty());
        assert_eq!(store.lists.load(Ordering::SeqCst), 0);
    })
}

#[test]
fn every_store_write_arrives_while_its_agent_is_busy() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("writes");
        let (target_id, stage_id, target) = fork_target("copied");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let name = submit(&snapshots, &agent, b"tree", confirmer(&confirm)).await;
        ended(&snapshots, &agent).await;
        fork_copy(&snapshots, &agent, &target_id, stage_id)
            .await
            .unwrap();
        snapshots.delete_snapshots(&agent, Box::new([name]));
        snapshots.delete_all_snapshots(&target, AgentMode::Durable);
        eventually(|| {
            store.deletes.lock().unwrap().len() == 1
                && store.all_deletes.load(Ordering::SeqCst) == 1
        })
        .await;
        let through_the_service = store.unbusy_writes.lock().unwrap().clone();

        // A write past the service is refused and recorded.
        let tree = tempfile::tempdir().unwrap();
        let past_the_service = store
            .save(
                &agent,
                &SnapshotName::new("p-outside").unwrap(),
                tree.path(),
                None,
                &CancellationToken::new(),
                &crate::filesystem_snapshot::Unlimited,
            )
            .await
            .is_err();
        let recorded = std::mem::take(&mut *store.unbusy_writes.lock().unwrap());

        assert!(through_the_service.is_empty(), "{through_the_service:?}");
        assert!(past_the_service);
        assert_eq!(recorded, vec!["save"]);
    })
}

#[test]
fn a_dropped_fork_keeps_both_incarnations_busy_until_its_copy_returns() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.copy_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let source = agent_snapshots("fork-source");
        let (target_id, stage_id, target) = fork_target("fork-target");

        let dropped = tokio::time::timeout(
            Duration::from_secs(1),
            fork_copy(&snapshots, &source, &target_id, stage_id),
        )
        .await;
        snapshots.delete_all_snapshots(&source, AgentMode::Durable);
        snapshots.delete_all_snapshots(&target, AgentMode::Durable);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_the_copy_runs = store.all_deletes.load(Ordering::SeqCst);
        gate.open();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 2).await;

        assert!(dropped.is_err());
        assert_eq!(while_the_copy_runs, 0);
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
        StoreOf::Given(Arc::new(InMemorySnapshotStore::new()) as Arc<dyn FilesystemSnapshotStore>),
        settings(4, 4),
        VolumeRoom::Pressure {
            volume: provisioning.volume().clone(),
            pressure,
        },
        CancellationToken::new(),
    );

    let admitted = snapshots
        .admit_periodic(&agent_snapshots("managed-xfs-pressure"), AgentMode::Durable)
        .await;

    assert_eq!(admitted.err(), Some(SnapshotSkip::VolumeUnderPressure));
}

/// Whether the job of `agent` waits for its next run after a failed run.
fn waits_after_failure(snapshots: &AgentFilesystemSnapshots, agent: &AgentSnapshots) -> bool {
    snapshots.core.as_ref().is_some_and(|core| {
        core.registry
            .read(|state| rules::job_waits_after_failure(state, agent))
    })
}

#[test]
fn a_newer_periodic_admission_replaces_a_save_in_its_backoff_and_discards_its_capture() {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("replaced-in-backoff");
        let discarded = Arc::new(AtomicUsize::new(0));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let old = submit_counted(&snapshots, &agent, b"old", &discarded, confirmer(&confirm)).await;
        eventually(|| waits_after_failure(&snapshots, &agent)).await;

        let started = tokio::time::Instant::now();
        let replacing = snapshots.admit_periodic(&agent, AgentMode::Durable).await;
        assert!(replacing.is_ok());
        eventually(|| discarded.load(Ordering::SeqCst) == 1).await;
        // The call of the old job returned at once: before the wait of 2 s after its failed run.
        let returned_after = started.elapsed();
        drop(replacing);
        ended(&snapshots, &agent).await;
        let freed_after = started.elapsed();
        tokio::time::sleep(Duration::from_secs(10)).await;

        assert!(
            returned_after < Duration::from_secs(2),
            "{returned_after:?}"
        );
        assert!(freed_after < Duration::from_secs(2), "{freed_after:?}");
        assert_eq!(
            (
                store.runs_started.load(Ordering::SeqCst),
                store.saved_names(),
                confirm.names(),
            ),
            (1, vec![old.as_str().to_string()], Vec::new())
        );
    })
}

#[test]
fn a_periodic_admission_during_a_run_is_refused_at_once_and_the_old_job_confirms_when_the_run_succeeds()
 {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("refused-during-run");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let old = submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        gate.wait_reached(1).await;

        let started = tokio::time::Instant::now();
        let refused = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .err();
        let waited = started.elapsed();
        gate.open();
        ended(&snapshots, &agent).await;

        assert_eq!(
            (refused, waited, confirm.names()),
            (
                Some(SnapshotSkip::UploadInFlight),
                Duration::ZERO,
                vec![old]
            )
        );
    })
}

#[test]
fn a_manual_update_is_woken_by_the_failed_run_replaces_the_job_and_never_sleeps_through_the_backoff()
 {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("update-replaces");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let _old = submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        gate.wait_reached(1).await;

        let started = tokio::time::Instant::now();
        let admitting = snapshots.admit_update(&agent, AgentMode::Durable, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.open();
        };
        let (admitted, ()) = futures::join!(admitting, opening);
        let waited = started.elapsed();
        drop(admitted);
        ended(&snapshots, &agent).await;

        assert_eq!(
            (
                waited,
                confirm.names(),
                store.runs_started.load(Ordering::SeqCst)
            ),
            (Duration::from_secs(1), Vec::new(), 1)
        );
    })
}

#[test]
fn a_manual_update_that_finds_a_decided_job_in_its_deletes_waits_for_its_end() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let delete_gate = Arc::new(Gate::default());
        *store.delete_gate.lock().unwrap() = Some(Arc::clone(&delete_gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("decided-in-deletes");
        let superseded = ScriptedConfirmer::answering(ConfirmOutcome::Superseded);
        submit(&snapshots, &agent, b"old", confirmer(&superseded)).await;
        delete_gate.wait_reached(1).await;

        let started = tokio::time::Instant::now();
        let admitting = snapshots.admit_update(&agent, AgentMode::Durable, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            delete_gate.open();
        };
        let (admitted, ()) = futures::join!(admitting, opening);
        let waited = started.elapsed();

        assert!(admitted.is_ok());
        assert!(waited <= Duration::from_secs(1), "waited {waited:?}");
    })
}

#[test]
fn a_start_waiting_for_a_replaced_job_asks_the_store_once() {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let agent = agent_snapshots("start-of-replaced");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let old = submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        eventually(|| waits_after_failure(&snapshots, &agent)).await;
        let starting = {
            let (snapshots, agent, old) = (Arc::clone(&snapshots), agent.clone(), old.clone());
            tokio::spawn(async move { snapshots.prepare_start(&agent, &old, no_interrupt()).await })
        };
        tokio::task::yield_now().await;

        let started = tokio::time::Instant::now();
        let replacing = snapshots.admit_periodic(&agent, AgentMode::Durable).await;
        let checked = starting.await.unwrap();

        assert!(replacing.is_ok());
        assert_eq!(
            (checked, started.elapsed() < Duration::from_secs(5)),
            (StartCheck::NotStored, true)
        );
    })
}

#[test]
fn a_manual_update_after_its_deadline_answers_no_slot_and_makes_no_further_run() {
    paused(async {
        let (store, snapshots, holder, gate) = one_slot_with_a_held_agent();
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"holder", confirmer(&confirm)).await;
        gate.wait_reached(1).await;
        let agent = agent_snapshots("update-without-slot");
        let discarded = Arc::new(AtomicUsize::new(0));

        let uploaded = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap()
            .upload_now(
                capture(b"update", &discarded),
                no_interrupt(),
                no_lost_shard(),
            )
            .await;
        let runs = store.runs_started.load(Ordering::SeqCst);
        gate.open();

        assert!(
            matches!(uploaded, Err(UploadNowError::NoSlot)),
            "{:?}",
            uploaded.as_ref().err()
        );
        assert_eq!((runs, discarded.load(Ordering::SeqCst)), (1, 1));
    })
}

#[test]
fn a_manual_update_whose_run_failed_before_its_deadline_answers_the_storage_error() {
    paused(async {
        let store = store_with_runs(5);
        store.failing_saves.store(usize::MAX, Ordering::SeqCst);
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("update-failed-run");
        let discarded = Arc::new(AtomicUsize::new(0));

        let started = tokio::time::Instant::now();
        let uploaded = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap()
            .upload_now(
                capture(b"update", &discarded),
                no_interrupt(),
                no_lost_shard(),
            )
            .await;

        assert!(
            matches!(&uploaded, Err(UploadNowError::Store(SaveError::Failed(_)))),
            "{:?}",
            uploaded.as_ref().err()
        );
        assert_eq!(
            (store.runs_started.load(Ordering::SeqCst), started.elapsed()),
            (4, Duration::from_secs(60))
        );
    })
}

#[test]
fn a_manual_update_whose_first_take_comes_after_its_deadline_with_a_free_slot_runs_once() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("update-late-take");
        let discarded = Arc::new(AtomicUsize::new(0));
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(61)).await;

        let uploaded = admission
            .upload_now(
                capture(b"update", &discarded),
                no_interrupt(),
                no_lost_shard(),
            )
            .await;

        assert!(uploaded.is_ok());
        assert_eq!(store.runs_started.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn a_stop_and_the_deadline_together_answer_stopped() {
    paused(async {
        let (_store, snapshots, holder, gate) = one_slot_with_a_held_agent();
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &holder, b"holder", confirmer(&confirm)).await;
        gate.wait_reached(1).await;
        let agent = agent_snapshots("update-stop-and-deadline");
        let discarded = Arc::new(AtomicUsize::new(0));
        let (interrupt, interrupted) = watch::channel(false);
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let interrupting = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            interrupt.send_replace(true);
        };

        let (uploaded, ()) = futures::join!(
            admission.upload_now(capture(b"update", &discarded), interrupted, no_lost_shard()),
            interrupting
        );
        gate.open();

        assert!(
            matches!(uploaded, Err(UploadNowError::Stopped)),
            "{:?}",
            uploaded.as_ref().err()
        );
    })
}

#[test]
fn a_stop_answers_a_delete_in_its_run_at_once() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let delete_gate = Arc::new(Gate::default());
        *store.delete_gate.lock().unwrap() = Some(Arc::clone(&delete_gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("stopped-delete");
        let core = snapshots.core.as_ref().unwrap();
        let stop = CancellationToken::new();
        let deleting = core.calls.delete(
            &agent,
            Arc::from([SnapshotName::new("p-1").unwrap()]),
            stop.clone().cancelled_owned(),
        );
        let stopping = async {
            delete_gate.wait_reached(1).await;
            stop.cancel();
        };

        let (deleted, ()) = futures::join!(deleting, stopping);
        let busy = core.registry.read(|state| rules::busy(state, &agent));
        delete_gate.open();
        eventually(|| core.registry.read(|state| rules::busy(state, &agent)) == 0).await;

        assert!(
            matches!(deleted, store_calls::Deleted::Stopped),
            "{deleted:?}"
        );
        assert_eq!(busy, 1);
    })
}

#[test]
fn a_restore_gives_its_slot_back_between_two_runs() {
    paused(async {
        let store = store_with_runs(2);
        let snapshots = service(&store, settings(4, 1));
        let (first, second) = (
            agent_snapshots("restore-runs"),
            agent_snapshots("restore-other"),
        );
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let first_name = submit(&snapshots, &first, b"first", confirmer(&confirm)).await;
        ended(&snapshots, &first).await;
        let second_name = submit(&snapshots, &second, b"second", confirmer(&confirm)).await;
        ended(&snapshots, &second).await;
        store.failing_restores.store(1, Ordering::SeqCst);
        let (first_into, second_into) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let order = Arc::new(Mutex::new(Vec::new()));

        let restoring_first = {
            let restore = snapshots.restore(&first, &first_name).unwrap();
            let order = Arc::clone(&order);
            let into = first_into.path().to_path_buf();
            async move {
                let restored = restore.restore(&into).await;
                order.lock().unwrap().push("first");
                restored
            }
        };
        let restoring_second = {
            let restore = snapshots.restore(&second, &second_name).unwrap();
            let order = Arc::clone(&order);
            let into = second_into.path().to_path_buf();
            async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let restored = restore.restore(&into).await;
                order.lock().unwrap().push("second");
                restored
            }
        };
        let (first_restored, second_restored) = futures::join!(restoring_first, restoring_second);

        assert!(first_restored.is_ok() && second_restored.is_ok());
        assert_eq!(
            (
                order.lock().unwrap().clone(),
                store.most_restores_at_once.load(Ordering::SeqCst)
            ),
            (vec!["second", "first"], 1)
        );
    })
}

fn retention_stopped(snapshots: &AgentFilesystemSnapshots, agent: &AgentSnapshots) -> Option<bool> {
    snapshots.core.as_ref().and_then(|core| {
        core.registry
            .read(|state| rules::job_retention_stopped(state, agent))
    })
}

#[test]
fn a_manual_update_stops_the_deletes_of_the_running_job_before_it_waits_for_the_job() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let agent = agent_snapshots("update-stops-retention-first");
        let gate = store.hold_saves_of(&agent);
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let old = submit(&snapshots, &agent, b"tree", confirmer(&confirm)).await;
        gate.wait_reached(1).await;
        let before = retention_stopped(&snapshots, &agent);

        let admitting = {
            let (snapshots, agent) = (Arc::clone(&snapshots), agent.clone());
            tokio::spawn(async move {
                snapshots
                    .admit_update(&agent, AgentMode::Durable, no_interrupt())
                    .await
                    .is_ok()
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        let while_it_waits = (
            retention_stopped(&snapshots, &agent),
            admitting.is_finished(),
        );
        gate.open();
        let admitted = admitting.await.unwrap();

        assert_eq!(
            (before, while_it_waits, admitted, confirm.names()),
            (Some(false), (Some(true), false), true, vec![old])
        );
        assert!(store.deletes.lock().unwrap().is_empty());
    })
}

/// The number of store calls that count on `agent` now.
fn busy_of(snapshots: &AgentFilesystemSnapshots, agent: &AgentSnapshots) -> u32 {
    snapshots.core.as_ref().map_or(0, |core| {
        core.registry.read(|state| rules::busy(state, agent))
    })
}

#[test]
fn a_save_waiting_for_a_late_publish_is_replaced_by_a_periodic_admission_and_its_tail_keeps_the_agent_busy()
 {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(true, Ordering::SeqCst);
        let late = Arc::new(Gate::default());
        *store.late_gate.lock().unwrap() = Some(Arc::clone(&late));
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let agent = agent_snapshots("late-publish-replaced");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        late.wait_reached(1).await;
        // The new save is held, so the test reads the counts while both calls live.
        let held = store.hold_saves_of(&agent);

        let new = submit(&snapshots, &agent, b"new", confirmer(&confirm)).await;
        held.wait_reached(1).await;
        let while_both_live = (
            store.runs_started.load(Ordering::SeqCst),
            busy_of(&snapshots, &agent),
        );
        late.open();
        eventually(|| store.late_waits.load(Ordering::SeqCst) == 0).await;
        eventually(|| busy_of(&snapshots, &agent) == 1).await;
        held.open();
        ended(&snapshots, &agent).await;

        assert_eq!(
            (
                while_both_live,
                confirm.names(),
                busy_of(&snapshots, &agent)
            ),
            ((2, 2), vec![new], 0)
        );
    })
}

#[test]
fn a_clean_save_is_not_replaced_while_its_job_confirms_although_a_stale_report_came() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        store.reports_late_on_success.store(true, Ordering::SeqCst);
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let agent = agent_snapshots("clean-save-not-replaced");
        let confirming = Arc::new(Gate::default());
        let confirm = ScriptedConfirmer::answering_after(ConfirmOutcome::Confirmed, &confirming);
        let old = submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        confirming.wait_reached(1).await;

        let periodic = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .err();
        let updating = {
            let (snapshots, agent) = (Arc::clone(&snapshots), agent.clone());
            tokio::spawn(async move {
                snapshots
                    .admit_update(&agent, AgentMode::Durable, no_interrupt())
                    .await
                    .is_ok()
            })
        };
        tokio::time::sleep(Duration::from_secs(5)).await;
        let update_waited = !updating.is_finished();
        confirming.open();
        let updated = updating.await.unwrap();

        assert_eq!(
            (periodic, update_waited, updated, confirm.names()),
            (Some(SnapshotSkip::UploadInFlight), true, true, vec![old])
        );
    })
}

#[test]
fn a_job_replaced_before_its_stop_is_cancelled_writes_no_confirmation() {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(true, Ordering::SeqCst);
        let late = Arc::new(Gate::default());
        *store.late_gate.lock().unwrap() = Some(Arc::clone(&late));
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let agent = agent_snapshots("replaced-without-a-stop");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        late.wait_reached(1).await;
        let core = snapshots.core.as_ref().unwrap();

        // The replacement as `Core::admit` makes it, without the cancel of the replaced stop that
        // follows it there: the old call ends and answers `Saved` with its stop not cancelled.
        let (_new, replaced) = registry::JobTicket::admit(
            &core.registry,
            &agent,
            &FilesystemSnapshotName::periodic(),
            SnapshotKind::Periodic,
            CancellationToken::new(),
            true,
        )
        .unwrap();
        late.open();
        eventually(|| store.late_waits.load(Ordering::SeqCst) == 0).await;
        eventually(|| busy_of(&snapshots, &agent) == 0).await;
        tokio::time::sleep(Duration::from_secs(5)).await;

        assert_eq!(
            (
                replaced.is_some_and(|stop| !stop.is_cancelled()),
                confirm.names()
            ),
            (true, Vec::new())
        );
    })
}

#[test]
fn a_manual_update_replaces_a_periodic_save_that_waits_out_a_late_write_and_a_delete_all_waits_for_that_write()
 {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(true, Ordering::SeqCst);
        let late = Arc::new(Gate::default());
        *store.late_gate.lock().unwrap() = Some(Arc::clone(&late));
        let settings = FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
            confirmation_wait: Duration::from_secs(5),
            ..values(4, 4)
        })
        .unwrap();
        let snapshots = Arc::new(service(&store, settings));
        let agent = agent_snapshots("update-replaces-a-late-save");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"periodic", confirmer(&confirm)).await;
        late.wait_reached(1).await;

        let started = tokio::time::Instant::now();
        let admission = snapshots
            .admit_update(&agent, AgentMode::Durable, no_interrupt())
            .await
            .unwrap();
        let saved = admission
            .upload_now(
                capture(b"update", &Arc::new(AtomicUsize::new(0))),
                no_interrupt(),
                no_lost_shard(),
            )
            .await;
        let update_took = started.elapsed();
        let update_saved = saved.is_ok();
        // The update's job ends with its retention, so only the tail of the replaced call counts
        // on the agent from here.
        drop(saved);
        eventually(|| {
            snapshots
                .core
                .as_ref()
                .is_some_and(|core| core.registry.read(|state| rules::is_free(state, &agent)))
        })
        .await;
        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let deletes_while_the_tail_waits = store.all_deletes.load(Ordering::SeqCst);
        late.open();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;

        assert!(update_saved);
        assert!(
            update_took < Duration::from_secs(5),
            "the update took {update_took:?}"
        );
        assert_eq!(
            (confirm.names(), deletes_while_the_tail_waits),
            (Vec::new(), 0)
        );
    })
}

#[test]
fn a_manual_update_waits_for_the_end_of_a_job_that_confirmed_when_its_run_succeeded() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let gate = Arc::new(Gate::default());
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("update-after-confirmed-run");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        let old = submit(&snapshots, &agent, b"old", confirmer(&confirm)).await;
        gate.wait_reached(1).await;

        let started = tokio::time::Instant::now();
        let admitting = snapshots.admit_update(&agent, AgentMode::Durable, no_interrupt());
        let opening = async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            gate.open();
        };
        let (admitted, ()) = futures::join!(admitting, opening);

        assert!(admitted.is_ok());
        assert_eq!(
            (started.elapsed(), confirm.names()),
            (Duration::from_secs(3), vec![old])
        );
    })
}

/// Runs a clean-up of the names of a revert whose delete the store holds at its gate, and an
/// upload of the agent admitted while the delete is held: the save publishes before the delete
/// goes on when `save_first`, and after the delete ended otherwise. The scripted store does not
/// prune; the rustic store test
/// `a_save_and_a_prune_of_one_agent_in_both_orders_of_the_publish_and_the_index_read_give_whole_snapshots`
/// runs the same overlap with a prune. Gives whether the upload was
/// admitted, the names that the store keeps, and the content of the tree of the new name.
async fn save_during_a_clean_up(save_first: bool) -> (bool, usize, Option<Vec<u8>>) {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4));
    let agent = agent_snapshots("save-during-clean-up");
    let deferred = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    let reverted = submit(&snapshots, &agent, b"reverted", confirmer(&deferred)).await;
    ended(&snapshots, &agent).await;
    let delete_gate = Arc::new(Gate::default());
    *store.delete_gate.lock().unwrap() = Some(Arc::clone(&delete_gate));
    let save_gate = Arc::new(Gate::default());
    if !save_first {
        *store.save_gate.lock().unwrap() = Some(Arc::clone(&save_gate));
    }
    snapshots.delete_snapshots(&agent, Box::new([reverted]));
    delete_gate.wait_reached(1).await;

    let admitted = snapshots.admit_periodic(&agent, AgentMode::Durable).await;
    let is_admitted = admitted.is_ok();
    let new = admitted.map(|admission| {
        let name = admission.name().clone();
        admission.submit(
            capture(b"new tree", &Arc::new(AtomicUsize::new(0))),
            None,
            confirmer(&deferred),
        );
        name
    });
    if save_first {
        eventually(|| store.saved_names().len() == 2).await;
        delete_gate.open();
    } else {
        save_gate.wait_reached(1).await;
        delete_gate.open();
        eventually(|| store.deletes.lock().unwrap().len() == 1).await;
        save_gate.open();
    }
    ended(&snapshots, &agent).await;
    eventually(|| store.deletes.lock().unwrap().len() == 1).await;
    let kept = store
        .memory
        .list(&agent, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap()
        .len();
    let restored = match new {
        Ok(new) => {
            let into = tempfile::tempdir().unwrap();
            store
                .memory
                .restore(
                    &agent,
                    &store_name(&new).unwrap(),
                    into.path(),
                    &crate::filesystem_snapshot::Unlimited,
                )
                .await
                .ok()
                .and_then(|_| std::fs::read(into.path().join("content")).ok())
        }
        Err(_) => None,
    };
    (is_admitted, kept, restored)
}

#[test]
fn a_save_during_a_names_clean_up_whose_delete_runs_gives_whole_snapshots() {
    paused(async {
        let save_first = save_during_a_clean_up(true).await;
        let delete_first = save_during_a_clean_up(false).await;

        assert_eq!(
            (save_first, delete_first),
            (
                (true, 1, Some(b"new tree".to_vec())),
                (true, 1, Some(b"new tree".to_vec()))
            )
        );
    })
}

#[test]
fn a_delete_all_during_the_wait_for_a_late_publish_removes_the_landed_file() {
    paused(async {
        let store = store_with_runs(3);
        store.failing_saves.store(1, Ordering::SeqCst);
        store.publish_before_failing.store(true, Ordering::SeqCst);
        let late = Arc::new(Gate::default());
        *store.late_gate.lock().unwrap() = Some(Arc::clone(&late));
        let snapshots = service(&store, settings(4, 4));
        let agent = agent_snapshots("delete-all-during-late-wait");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"landed", confirmer(&confirm)).await;
        late.wait_reached(1).await;

        snapshots.delete_all_snapshots(&agent, AgentMode::Durable);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let while_it_waits = store.all_deletes.load(Ordering::SeqCst);
        late.open();
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;
        ended(&snapshots, &agent).await;

        assert_eq!(while_it_waits, 0);
        assert!(confirm.names().is_empty());
        assert!(
            store
                .memory
                .list(&agent, &crate::filesystem_snapshot::Unlimited)
                .await
                .unwrap()
                .is_empty()
        );
    })
}

/// Runs a manual update whose save fails with a publish that ends without an answer, and whose
/// wait for that publish ends 70 s after it began, after the deadline of the update. The publish
/// lands when `lands`.
async fn update_through_a_late_publish(lands: bool) -> Result<(), String> {
    let store = store_with_runs(3);
    store.failing_saves.store(1, Ordering::SeqCst);
    store.publish_before_failing.store(lands, Ordering::SeqCst);
    store
        .late_publish_never_lands
        .store(!lands, Ordering::SeqCst);
    let late = Arc::new(Gate::default());
    *store.late_gate.lock().unwrap() = Some(Arc::clone(&late));
    let snapshots = service(&store, settings(4, 4));
    let agent = agent_snapshots("update-deadline-in-late-wait");
    let admission = snapshots
        .admit_update(&agent, AgentMode::Durable, no_interrupt())
        .await
        .unwrap();
    let uploading = admission.upload_now(
        capture(b"update", &Arc::new(AtomicUsize::new(0))),
        no_interrupt(),
        no_lost_shard(),
    );
    let opening = async {
        late.wait_reached(1).await;
        tokio::time::sleep(Duration::from_secs(70)).await;
        late.open();
    };
    let (uploaded, ()) = futures::join!(uploading, opening);
    match uploaded {
        Ok(_) => Ok(()),
        Err(UploadNowError::Store(SaveError::Failed(_))) => Err("failed".to_string()),
        Err(other) => Err(format!("{other:?}")),
    }
}

#[test]
fn a_manual_update_whose_deadline_comes_during_the_wait_for_a_late_publish_gets_saved_or_the_storage_error()
 {
    paused(async {
        let landed = update_through_a_late_publish(true).await;
        let never_landed = update_through_a_late_publish(false).await;

        assert_eq!((landed, never_landed), (Ok(()), Err("failed".to_string())));
    })
}

#[test]
fn a_revert_hold_ends_a_retention_delete_at_once() {
    paused(async {
        let (store, snapshots, agent, gate) = with_a_delete_held(ConfirmOutcome::Confirmed).await;
        let core = snapshots.core.as_ref().unwrap();

        let hold = snapshots.begin_revert(&agent);
        let ended = tokio::time::timeout(
            Duration::from_secs(2),
            core.registry.until_agent_free(&agent),
        )
        .await;
        gate.open();
        drop(hold);

        assert!(ended.is_ok(), "the job waited for its retention delete");
        // The delete in flight runs to its end, counted; the job sends no further delete.
        eventually(|| store.deletes.lock().unwrap().len() == 1).await;
    })
}

/// Saves an older snapshot of an agent that keeps one periodic snapshot, and then begins a revert
/// of the agent. Gives the store, the service, the agent and the hold.
async fn with_a_revert_hold() -> (
    Arc<ScriptedStore>,
    AgentFilesystemSnapshots,
    AgentSnapshots,
    RevertHold,
) {
    let store = Arc::new(ScriptedStore::default());
    let settings = FilesystemSnapshotUploadConfig::new(FilesystemSnapshotUploadValues {
        retained_periodic_snapshots: 1,
        ..values(4, 4)
    })
    .unwrap();
    let snapshots = service(&store, settings);
    let agent = agent_snapshots("revert-hold");
    let deferred = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    submit(&snapshots, &agent, b"older", confirmer(&deferred)).await;
    ended(&snapshots, &agent).await;
    let hold = snapshots.begin_revert(&agent);
    (store, snapshots, agent, hold)
}

#[test]
fn a_job_admitted_during_a_revert_hold_deletes_nothing() {
    paused(async {
        let (store, snapshots, agent, hold) = with_a_revert_hold().await;
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        let newest = submit(&snapshots, &agent, b"newest", confirmer(&confirm)).await;
        ended(&snapshots, &agent).await;
        drop(hold);

        assert_eq!(
            (
                confirm.names(),
                store.lists.load(Ordering::SeqCst),
                store.deletes.lock().unwrap().len()
            ),
            (vec![newest], 0, 0)
        );
    })
}

#[test]
fn a_dropped_revert_hold_frees_the_deletes_of_later_jobs() {
    paused(async {
        let (store, snapshots, agent, hold) = with_a_revert_hold().await;
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);

        drop(hold);
        submit(&snapshots, &agent, b"newest", confirmer(&confirm)).await;
        ended(&snapshots, &agent).await;

        assert_eq!(store.deletes.lock().unwrap().len(), 1);
    })
}

#[test]
fn the_deletes_of_a_revert_run_after_its_hold_and_end_it() {
    paused(async {
        let (store, snapshots, agent, hold) = with_a_revert_hold().await;
        let reverted = FilesystemSnapshotName::periodic();

        hold.delete_snapshots(Box::new([reverted.clone()]));
        eventually(|| store.deletes.lock().unwrap().len() == 1).await;
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Confirmed);
        submit(&snapshots, &agent, b"newest", confirmer(&confirm)).await;
        ended(&snapshots, &agent).await;

        assert_eq!(
            store.deletes.lock().unwrap().first().cloned(),
            Some(Box::from(reverted.as_str()))
        );
        assert_eq!(store.deletes.lock().unwrap().len(), 2);
    })
}

/// Whether the store holds a snapshot of `agent`.
async fn holds_snapshots(store: &ScriptedStore, agent: &AgentSnapshots) -> bool {
    !store
        .memory
        .list(agent, &crate::filesystem_snapshot::Unlimited)
        .await
        .unwrap()
        .is_empty()
}

/// Copies one snapshot of a source into the stage of a fork attempt, then lets `publish` end the
/// attempt. Gives whether the store still holds the snapshots of the stage after the end of the
/// attempt and the clean-ups.
async fn stage_after(
    publish: impl FnOnce(Copied, golem_common::model::AgentFingerprint),
) -> (bool, bool) {
    let store = Arc::new(ScriptedStore::default());
    let snapshots = service(&store, settings(4, 4));
    let source = agent_snapshots("fork-source");
    let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
    submit(&snapshots, &source, b"tree", confirmer(&confirm)).await;
    ended(&snapshots, &source).await;
    let (target, stage_id, stage) = fork_target("fork-target");
    let copied = snapshots
        .begin_fork(&source, fork_flight(&target.agent_id, [2; 32]))
        .await
        .unwrap()
        .copy(&target, stage_id, None)
        .await
        .unwrap();
    let copied_some = holds_snapshots(&store, &stage).await;

    publish(copied, golem_common::model::AgentFingerprint(stage_id));
    tokio::time::sleep(Duration::from_secs(5)).await;
    (copied_some, holds_snapshots(&store, &stage).await)
}

/// Publishes `copied` with the answer `found` of the publication, and `live` as the live target
/// that a refused publication reads.
fn publish_with(
    copied: Copied,
    found: PublishFound<(), ()>,
    live: Option<golem_common::model::AgentFingerprint>,
) {
    tokio::spawn(async move {
        let _ = copied
            .publish(
                |_publication| async move { found }.boxed(),
                move || async move { live }.boxed(),
            )
            .await;
    });
}

fn other_instance() -> golem_common::model::AgentFingerprint {
    golem_common::model::AgentFingerprint(uuid::Uuid::new_v4())
}

#[test]
fn a_dropped_fork_before_its_publication_deletes_its_stage_scope_after_its_copy_returns() {
    paused(async {
        assert_eq!(stage_after(|copied, _| drop(copied)).await, (true, false));
    })
}

#[test]
fn a_loser_deletes_its_stage_scope_when_the_live_target_has_another_instance_id() {
    paused(async {
        let published_to_another = stage_after(|copied, _| {
            publish_with(copied, PublishFound::Live(other_instance(), ()), None)
        })
        .await;
        let lost_before_its_publication =
            stage_after(|copied, _| copied.lost_to(other_instance())).await;

        assert_eq!(
            (published_to_another, lost_before_its_publication),
            ((true, false), (true, false))
        );
    })
}

#[test]
fn a_live_target_with_the_own_stage_id_is_published_not_deleted() {
    paused(async {
        let published =
            stage_after(|copied, _| publish_with(copied, PublishFound::Published(()), None)).await;
        let live_own =
            stage_after(|copied, own| publish_with(copied, PublishFound::Live(own, ()), None))
                .await;
        let lost_to_own = stage_after(|copied, own| copied.lost_to(own)).await;

        assert_eq!(
            [published, live_own, lost_to_own],
            [(true, true), (true, true), (true, true)]
        );
    })
}

#[test]
fn a_refused_publication_whose_target_is_the_own_stage_keeps_it() {
    paused(async {
        let own =
            stage_after(|copied, own| publish_with(copied, PublishFound::Refused(()), Some(own)))
                .await;
        let other = stage_after(|copied, _| {
            publish_with(copied, PublishFound::Refused(()), Some(other_instance()))
        })
        .await;
        let none =
            stage_after(|copied, _| publish_with(copied, PublishFound::Refused(()), None)).await;
        let unknown =
            stage_after(|copied, _| publish_with(copied, PublishFound::Unknown(()), None)).await;

        assert_eq!(
            [own, other, none, unknown],
            [(true, true), (true, false), (true, true), (true, true)]
        );
    })
}

#[test]
fn an_export_conflict_after_the_session_deadline_keeps_the_own_stage() {
    paused(async {
        // The receipt of the own stage is no longer live, so the fork answers a conflict; the
        // instance id of the receipt is the own stage id.
        let kept = stage_after(|copied, own| {
            tokio::spawn(async move {
                let answer: Result<Result<(), &str>, ()> = copied
                    .publish(
                        |_publication| {
                            async move { PublishFound::Live(own, Err("conflict")) }.boxed()
                        },
                        || async { None }.boxed(),
                    )
                    .await;
                assert_eq!(answer, Ok(Err("conflict")));
            });
        })
        .await;

        assert_eq!(kept, (true, true));
    })
}

#[test]
fn a_fork_waits_for_another_attempt_of_its_request_and_for_a_pending_delete_of_its_source() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = Arc::new(service(&store, settings(4, 4)));
        let source = agent_snapshots("fork-source");
        let (target, _, _) = fork_target("fork-target");
        let flight = fork_flight(&target.agent_id, [3; 32]);
        let first = snapshots.begin_fork(&source, flight.clone()).await.unwrap();

        let second = {
            let (snapshots, source, flight) =
                (Arc::clone(&snapshots), source.clone(), flight.clone());
            tokio::spawn(async move {
                snapshots
                    .begin_fork(&source, flight)
                    .await
                    .map(|fork| fork.waited())
            })
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        let second_waited_while_the_first_lives = !second.is_finished();
        drop(first);
        let second_waited = second.await.unwrap().unwrap();

        snapshots.delete_all_snapshots(&source, AgentMode::Durable);
        let third = {
            let (snapshots, source) = (Arc::clone(&snapshots), source.clone());
            tokio::spawn(async move {
                snapshots
                    .begin_fork(&source, fork_flight(&target.agent_id, [4; 32]))
                    .await
                    .map(|fork| fork.waited())
            })
        };
        eventually(|| store.all_deletes.load(Ordering::SeqCst) == 1).await;
        let third_waited = third.await.unwrap().unwrap();

        assert_eq!(
            (
                second_waited_while_the_first_lives,
                second_waited,
                third_waited
            ),
            (true, true, true)
        );
    })
}

#[test]
fn a_revert_delete_of_the_source_waits_for_the_fork() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let source = agent_snapshots("fork-source");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        let name = submit(&snapshots, &source, b"tree", confirmer(&confirm)).await;
        ended(&snapshots, &source).await;
        let (target, _, _) = fork_target("fork-target");
        let fork = snapshots
            .begin_fork(&source, fork_flight(&target.agent_id, [5; 32]))
            .await
            .unwrap();

        snapshots
            .begin_revert(&source)
            .delete_snapshots(Box::new([name]));
        tokio::time::sleep(Duration::from_secs(5)).await;
        let while_the_fork_holds = store.deletes.lock().unwrap().len();
        drop(fork);
        eventually(|| store.deletes.lock().unwrap().len() == 1).await;

        assert_eq!(while_the_fork_holds, 0);
    })
}

#[test]
fn the_copy_of_a_fork_checks_the_snapshot_of_the_baseline_in_the_stage() {
    paused(async {
        let store = Arc::new(ScriptedStore::default());
        let snapshots = service(&store, settings(4, 4));
        let source = agent_snapshots("fork-source");
        let confirm = ScriptedConfirmer::answering(ConfirmOutcome::Deferred);
        let name = submit(&snapshots, &source, b"tree", confirmer(&confirm)).await;
        ended(&snapshots, &source).await;
        let other = FilesystemSnapshotName::periodic();
        let baseline_of = |baseline: Option<FilesystemSnapshotName>| {
            let (snapshots, source) = (&snapshots, &source);
            async move {
                let (target, stage_id, _) = fork_target("fork-target");
                snapshots
                    .begin_fork(source, fork_flight(&target.agent_id, [6; 32]))
                    .await
                    .unwrap()
                    .copy(&target, stage_id, baseline.as_ref())
                    .await
                    .unwrap()
                    .baseline()
            }
        };

        let missing = (
            snapshots.missing(&source, &name).await.unwrap(),
            snapshots.missing(&source, &other).await.unwrap(),
        );

        assert_eq!(missing, (false, true));
        assert_eq!(
            [
                baseline_of(Some(name)).await,
                baseline_of(Some(other.clone())).await,
                baseline_of(None).await,
            ],
            [
                Baseline::Present,
                Baseline::Missing(other),
                Baseline::NotChecked
            ]
        );
    })
}
