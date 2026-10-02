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

//! The calls of the service to the filesystem snapshot store. This module is the only owner of
//! the store: in production code, only it can make the store, and only it holds it.
//!
//! Each call that writes runs in a task of its own and counts as work of its agent until it
//! returns, also when its caller stops waiting for it. A stop ends only waits. Only a shutdown
//! drops a call. The slots of the store operations, the retries, the rule of one save at a time
//! for each agent, the discard of a capture, the cancel of a save on a lost shard, and the order
//! of the shutdown are hidden here. Each decision is a private pure function.

use super::CapturedTree;
use super::registry::{JobTicket, Registry};
use super::rules::{self, CallKind, SaveAttempt};
use crate::filesystem_snapshot::{
    AgentSnapshots, ChangeDetection, FilesystemSnapshotStore, SnapshotInfo, SnapshotName,
    SnapshotStoreError,
};
use crate::services::agent_filesystem::{RestoreError, RestoreTree};
use crate::services::golem_config::{
    FilesystemSnapshotStoreConfig, FilesystemSnapshotUploadConfig,
};
use futures::StreamExt as _;
use futures::future::{BoxFuture, FutureExt as _, Shared};
use golem_common::model::RetryConfig;
use golem_common::model::oplog::FilesystemSnapshotName;
use rand::Rng as _;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The permission to make the managed filesystem snapshot store. Only this module can make one.
pub(crate) struct StoreKey(());

/// The store that the calls go to.
pub(super) enum StoreOf<'a> {
    /// The managed store on the blob storage, with the key and the settings of the configuration.
    Managed(
        Arc<dyn golem_service_base::storage::blob::BlobStorage>,
        &'a FilesystemSnapshotStoreConfig,
    ),
    /// A store that a test gives.
    #[cfg(any(test, feature = "test-utils"))]
    Given(Arc<dyn FilesystemSnapshotStore>),
}

/// What an upload gave.
#[derive(Debug)]
pub(super) enum Upload {
    /// The store holds the snapshot.
    Saved(SnapshotInfo),
    /// The upload failed.
    Failed(UploadError),
    /// A stop ended the upload. A save that ran then goes on, and the capture is discarded after
    /// it returned.
    Stopped,
}

/// Why an upload failed.
#[derive(Debug)]
pub(super) enum UploadError {
    /// The save failed, after the retries when the error allows them.
    Store(SnapshotStoreError),
    /// The deadline passed while the upload waited for the running save of the agent.
    SaveRunning,
    /// The deadline passed while the upload waited for a slot of the uploads.
    NoSlot,
}

/// What a delete gave.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub(super) enum Deleted {
    /// The store deleted the snapshots.
    Deleted,
    /// A stop or the shutdown ended the delete. An attempt that ran then goes on.
    Stopped,
    /// The delete failed after the retries.
    Leaked(SnapshotStoreError),
}

/// What stops an upload, beside the stop of its job.
pub(super) struct Stops {
    /// The stop of the caller: a terminal interrupt, or a caller that stopped waiting.
    caller: Shared<BoxFuture<'static, ()>>,
    /// Whether the shard of the agent is lost. A lost shard also cancels a running save in the
    /// store, so that it publishes nothing.
    lost_shard: Option<watch::Receiver<bool>>,
}

impl Stops {
    /// No stop beside the stop of the job.
    pub(super) fn none() -> Self {
        Self {
            caller: std::future::pending().boxed().shared(),
            lost_shard: None,
        }
    }

    /// The stop `caller`, and the lost shard that `lost_shard` reports.
    pub(super) fn of(
        caller: impl Future<Output = ()> + Send + 'static,
        lost_shard: watch::Receiver<bool>,
    ) -> Self {
        Self {
            caller: caller.boxed().shared(),
            lost_shard: Some(lost_shard),
        }
    }

    /// Whether the shard is lost now.
    fn lost_shard(&self) -> bool {
        self.lost_shard
            .as_ref()
            .is_some_and(|lost_shard| *lost_shard.borrow())
    }

    /// Whether the caller stopped or the shard is lost now.
    fn stopped(&self) -> bool {
        self.lost_shard() || self.caller.clone().now_or_never().is_some()
    }

    /// Completes when the caller stops or the shard is lost.
    async fn fired(&self) {
        tokio::select! {
            () = self.caller.clone() => {}
            () = lost_shard_set(self.lost_shard.clone()) => {}
        }
    }
}

/// Completes when `lost_shard` reports a lost shard. It never completes without a receiver or
/// when its sender is gone.
async fn lost_shard_set(lost_shard: Option<watch::Receiver<bool>>) {
    if let Some(mut lost_shard) = lost_shard
        && lost_shard.wait_for(|lost| *lost).await.is_ok()
    {
        return;
    }
    std::future::pending().await
}

/// How a store call ended.
enum CallResult<T> {
    /// The call returned.
    Done(Result<T, SnapshotStoreError>),
    /// The shutdown dropped the call.
    ShutDown,
}

/// A slot of the uploads that one store attempt of an upload holds. It counts the attempt in the
/// gauge of the uploads in progress while it lives.
struct UploadSlot {
    _permit: OwnedSemaphorePermit,
    #[cfg(test)]
    attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl UploadSlot {
    fn new(permit: OwnedSemaphorePermit, calls: &StoreCalls) -> Self {
        crate::metrics::filesystem_snapshots::inc_uploads_in_progress();
        #[cfg(test)]
        calls
            .upload_attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let _ = calls;
        Self {
            _permit: permit,
            #[cfg(test)]
            attempts: Arc::clone(&calls.upload_attempts),
        }
    }
}

impl Drop for UploadSlot {
    fn drop(&mut self) {
        crate::metrics::filesystem_snapshots::dec_uploads_in_progress();
        #[cfg(test)]
        self.attempts
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The store, and all that a call to it needs.
pub(super) struct StoreCalls {
    store: Arc<dyn FilesystemSnapshotStore>,
    /// The slots of the store operations that save, delete, copy or list.
    uploads: Arc<Semaphore>,
    /// The slots of the restores.
    restores: Arc<Semaphore>,
    registry: Arc<Registry>,
    retry: RetryConfig,
    /// The tasks of the calls, and of the discards that follow a call whose caller stopped.
    jobs: TaskTracker,
    /// Cancelled when the executor shuts down. It drops each call.
    shutdown: CancellationToken,
    /// Cancelled when the store has stopped, after the shutdown. A capture whose save the
    /// shutdown dropped is discarded only then.
    store_stopped: CancellationToken,
    /// The store attempts of the uploads that hold a slot now: a count of the same attempts as the
    /// gauge of the metrics, which all services of the process share.
    #[cfg(test)]
    upload_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl StoreCalls {
    /// The calls to `store` with `settings`, whose work counts in `registry`. The calls and the
    /// discards run on `jobs`, and `shutdown` drops the calls.
    pub(super) fn bind(
        store: StoreOf<'_>,
        settings: &FilesystemSnapshotUploadConfig,
        registry: Arc<Registry>,
        shutdown: CancellationToken,
        jobs: TaskTracker,
    ) -> Self {
        let store = match store {
            StoreOf::Managed(storage, config) => {
                crate::filesystem_snapshot::managed_store(storage, config, StoreKey(()))
            }
            #[cfg(any(test, feature = "test-utils"))]
            StoreOf::Given(store) => store,
        };
        Self {
            store,
            uploads: Arc::new(Semaphore::new(settings.max_concurrent_uploads().get())),
            restores: Arc::new(Semaphore::new(settings.max_concurrent_restores().get())),
            registry,
            retry: settings.upload_retry().clone(),
            jobs,
            shutdown,
            store_stopped: CancellationToken::new(),
            #[cfg(test)]
            upload_attempts: Arc::default(),
        }
    }

    /// Saves `capture` under `name` for the job of `job`, with the retries, one save at a time for
    /// the agent, and a slot for each attempt. Each slot grant marks the job as saving. A stop of
    /// the job or of `stops` gives `Stopped` at once: it ends a wait for the running save of the
    /// agent, for a slot or between two attempts, and a save that runs then goes on inside, with
    /// its slot, until it returns. A lost shard also cancels that save in the store. With a
    /// `deadline`, a wait for the running save of the agent or for a slot that reaches it gives
    /// `SaveRunning` or `NoSlot`. The capture is always discarded, after the last save that reads
    /// it returned; a save that the shutdown dropped is followed by the stop of the store first.
    pub(super) async fn upload(
        &self,
        job: &JobTicket,
        name: &SnapshotName,
        capture: CapturedTree,
        parent: Option<(SnapshotName, ChangeDetection)>,
        stops: Stops,
        deadline: Option<Instant>,
    ) -> Upload {
        let parent = parent.map(Arc::new);
        let run = futures::stream::unfold(
            UploadRun {
                attempt: 1,
                slot: None,
                last: None,
                capture: Some(capture),
            },
            |run| {
                let (stops, parent) = (&stops, &parent);
                async move {
                    let (answer, mut run) = self
                        .upload_step_of(run, job, name, parent, stops, deadline)
                        .await;
                    if answer.is_some()
                        && let Some(capture) = run.capture.take()
                    {
                        capture.discard.await;
                    }
                    Some((answer, run))
                }
            },
        );
        std::pin::pin!(run.filter_map(|answer| async move { answer }))
            .next()
            .await
            .unwrap_or(Upload::Stopped)
    }

    /// Runs one step of an upload, as [`upload_step`] decides it, and gives the answer when the
    /// upload ends with this step.
    async fn upload_step_of(
        &self,
        mut run: UploadRun,
        job: &JobTicket,
        name: &SnapshotName,
        parent: &Option<Arc<(SnapshotName, ChangeDetection)>>,
        stops: &Stops,
        deadline: Option<Instant>,
    ) -> (Option<Upload>, UploadRun) {
        let agent = job.agent();
        if run.slot.is_none() && run.last.is_none() {
            run.slot = self.try_slot(job);
        }
        let seen = UploadSeen {
            attempt: run.attempt,
            save_running: self
                .registry
                .read(|state| rules::save_running(state, agent)),
            slot: run.slot.is_some(),
            stopped: job.stop_requested() || stops.stopped(),
            deadline_passed: deadline.is_some_and(|deadline| Instant::now() >= deadline),
            last: run.last.take(),
            jitter: jitter(&self.retry),
        };
        match upload_step(seen, &self.retry) {
            UploadStep::WaitForSave => {
                tokio::select! {
                    () = self.registry.until_save_may_start(agent) => {}
                    () = job.until_stopped() => {}
                    () = stops.fired() => {}
                    () = until(deadline) => {}
                }
                (None, run)
            }
            UploadStep::WaitForSlot => {
                let granted = tokio::select! {
                    permit = Arc::clone(&self.uploads).acquire_owned() => Some(permit.ok()),
                    () = job.until_stopped() => None,
                    () = stops.fired() => None,
                    () = until(deadline) => None,
                };
                match granted {
                    Some(Some(permit)) => {
                        run.slot = Some(UploadSlot::new(permit, self));
                        job.saving();
                        (None, run)
                    }
                    // The slots are gone.
                    Some(None) => (Some(Upload::Stopped), run),
                    None => (None, run),
                }
            }
            UploadStep::Call => {
                let (Some(slot), Some(directory)) = (
                    run.slot.take(),
                    run.capture
                        .as_ref()
                        .map(|capture| Arc::clone(&capture.directory)),
                ) else {
                    return (Some(Upload::Stopped), run);
                };
                let cancel = CancellationToken::new();
                match self.save_call(agent, slot, name, directory, parent.clone(), cancel.clone()) {
                    Ok(handle) => self.await_save(run, handle, cancel, job, stops).await,
                    // Another save of the agent started first: the slot goes back, and the upload
                    // waits for that save.
                    Err(slot) => {
                        drop(slot);
                        (None, run)
                    }
                }
            }
            UploadStep::Retry(delay) => {
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = job.until_stopped() => {}
                    () = stops.fired() => {}
                }
                run.attempt += 1;
                (None, run)
            }
            UploadStep::Answer(upload) => (Some(upload), run),
        }
    }

    /// Takes a slot of the uploads when one is free, and marks the job as saving.
    fn try_slot(&self, job: &JobTicket) -> Option<UploadSlot> {
        let permit = Arc::clone(&self.uploads).try_acquire_owned().ok()?;
        job.saving();
        Some(UploadSlot::new(permit, self))
    }

    /// Starts a save of the tree in `directory` under `name` with `parent`, and with `cancel` as
    /// the cancel of the save in the store. A save that finds the name taken asks for the info of
    /// the name, in the same call.
    fn save_call(
        &self,
        agent: &AgentSnapshots,
        slot: UploadSlot,
        name: &SnapshotName,
        directory: Arc<Path>,
        parent: Option<Arc<(SnapshotName, ChangeDetection)>>,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<CallResult<SnapshotInfo>>, UploadSlot> {
        let (store, agent_of_call, name) = (Arc::clone(&self.store), agent.clone(), name.clone());
        self.store_call_with(agent, slot, CallKind::Save, move || {
            async move {
                let parent = parent
                    .as_deref()
                    .map(|(parent, detection)| (parent, *detection));
                match rules::save_attempt(
                    store
                        .save(&agent_of_call, &name, &directory, parent, &cancel)
                        .await,
                ) {
                    SaveAttempt::Saved(info) => Ok(info),
                    SaveAttempt::StatOwn => {
                        store.stat(&agent_of_call, &name).await.and_then(|found| {
                            found.ok_or_else(|| SnapshotStoreError::Storage {
                                retryable: true,
                                source: anyhow::anyhow!(
                                    "the filesystem snapshot {name} exists at the save and not after it"
                                ),
                            })
                        })
                    }
                    SaveAttempt::Failed(error) => Err(error),
                }
            }
            .boxed()
        })
    }

    /// Waits for the save of `handle`. A stop gives `Stopped` at once, and hands the capture to a
    /// task that discards it after the save returned. A lost shard cancels the save in the store,
    /// also after the stop. A save that the shutdown dropped is discarded after the store stopped.
    async fn await_save(
        &self,
        mut run: UploadRun,
        mut handle: JoinHandle<CallResult<SnapshotInfo>>,
        cancel: CancellationToken,
        job: &JobTicket,
        stops: &Stops,
    ) -> (Option<Upload>, UploadRun) {
        let returned = tokio::select! {
            biased;
            result = &mut handle => Some(result.unwrap_or(CallResult::ShutDown)),
            () = job.until_stopped() => None,
            () = stops.fired() => None,
        };
        match returned {
            Some(CallResult::Done(result)) => {
                run.last = Some(result);
                (None, run)
            }
            Some(CallResult::ShutDown) => {
                self.store_stopped.cancelled().await;
                (Some(Upload::Stopped), run)
            }
            None => {
                let lost_shard = stops.lost_shard.clone();
                if on_stop(stops.lost_shard()) == OnStop::CancelStore {
                    cancel.cancel();
                }
                let capture = run.capture.take();
                let store_stopped = self.store_stopped.clone();
                self.jobs.spawn(async move {
                    let result = tokio::select! {
                        result = &mut handle => result,
                        () = lost_shard_set(lost_shard) => {
                            cancel.cancel();
                            handle.await
                        }
                    };
                    if dropped_by_the_shutdown(&result) {
                        store_stopped.cancelled().await;
                    }
                    if let Some(capture) = capture {
                        capture.discard.await;
                    }
                });
                (Some(Upload::Stopped), run)
            }
        }
    }

    /// Deletes `names` of `agent` as one batch, with the retries, each attempt under a slot. A
    /// `stop` or the shutdown ends a wait for a slot, a wait between two attempts, and the wait for
    /// a running attempt, which then goes on inside.
    pub(super) async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: Arc<[SnapshotName]>,
        stop: impl Future<Output = ()> + Send,
    ) -> Deleted {
        let store = Arc::clone(&self.store);
        let agent_of_call = agent.clone();
        self.deleting(agent, stop, move || {
            let (store, agent, names) = (
                Arc::clone(&store),
                agent_of_call.clone(),
                Arc::clone(&names),
            );
            async move { store.delete(&agent, &names).await }.boxed()
        })
        .await
    }

    /// Deletes every snapshot of `agent`, with the retries. Only the shutdown ends it.
    pub(super) async fn delete_all(&self, agent: &AgentSnapshots) -> Deleted {
        let store = Arc::clone(&self.store);
        let agent_of_call = agent.clone();
        self.deleting(agent, std::future::pending(), move || {
            let (store, agent) = (Arc::clone(&store), agent_of_call.clone());
            async move { store.delete_all(&agent).await }.boxed()
        })
        .await
    }

    /// Runs the delete that `call` makes, with the retries, each attempt under a slot and counted
    /// for `agent`.
    async fn deleting(
        &self,
        agent: &AgentSnapshots,
        stop: impl Future<Output = ()> + Send,
        call: impl Fn() -> BoxFuture<'static, Result<(), SnapshotStoreError>> + Send + Sync + 'static,
    ) -> Deleted {
        match self
            .retried(agent, CallKind::Other, stop.boxed().shared(), call)
            .await
        {
            Retried::Done(()) => Deleted::Deleted,
            Retried::Stopped => Deleted::Stopped,
            Retried::Failed(error) => Deleted::Leaked(error),
        }
    }

    /// Copies every snapshot of `from` into `to`, with the retries, each attempt under a slot.
    /// Each attempt counts on both agents until it returns, also when the caller stops waiting.
    pub(super) async fn copy(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
    ) -> Result<(), SnapshotStoreError> {
        let (store, from_of_call, to_of_call) = (Arc::clone(&self.store), from.clone(), to.clone());
        match self
            .retried(
                from,
                CallKind::Copy { to: to.clone() },
                std::future::pending::<()>().boxed().shared(),
                move || {
                    let (store, from, to) =
                        (Arc::clone(&store), from_of_call.clone(), to_of_call.clone());
                    async move { store.copy_all(&from, &to).await }.boxed()
                },
            )
            .await
        {
            Retried::Done(()) => Ok(()),
            Retried::Stopped => Err(SnapshotStoreError::Storage {
                retryable: false,
                source: anyhow::anyhow!("the copy of the filesystem snapshots was stopped"),
            }),
            Retried::Failed(error) => Err(error),
        }
    }

    /// Runs the call that `call` makes for `agent` with the retries, each attempt under a slot and
    /// counted as `kind`, until it succeeds, fails for good, or `stop` or the shutdown ends it.
    async fn retried<T: Send + 'static>(
        &self,
        agent: &AgentSnapshots,
        kind: CallKind,
        stop: Shared<BoxFuture<'_, ()>>,
        call: impl Fn() -> BoxFuture<'static, Result<T, SnapshotStoreError>> + Send + Sync + 'static,
    ) -> Retried<T> {
        let call = Arc::new(call);
        let run = futures::stream::unfold(
            (1u32, None::<Result<T, SnapshotStoreError>>),
            |(attempt, last)| {
                let (stop, call, kind) = (stop.clone(), Arc::clone(&call), kind.clone());
                async move {
                    let stopped =
                        self.shutdown.is_cancelled() || stop.clone().now_or_never().is_some();
                    match call_step(attempt, stopped, last, jitter(&self.retry), &self.retry) {
                        CallStep::Call => {
                            let permit = tokio::select! {
                                permit = Arc::clone(&self.uploads).acquire_owned() => permit.ok(),
                                () = stop.clone() => None,
                                () = self.shutdown.cancelled() => None,
                            };
                            let Some(permit) = permit else {
                                return Some((Some(Retried::Stopped), (attempt, None)));
                            };
                            let Ok(mut handle) =
                                self.store_call_with(agent, permit, kind, move || call())
                            else {
                                return Some((Some(Retried::Stopped), (attempt, None)));
                            };
                            let returned = tokio::select! {
                                biased;
                                result = &mut handle => result.unwrap_or(CallResult::ShutDown),
                                () = stop.clone() => CallResult::ShutDown,
                            };
                            match returned {
                                CallResult::Done(result) => Some((None, (attempt, Some(result)))),
                                CallResult::ShutDown => {
                                    Some((Some(Retried::Stopped), (attempt, None)))
                                }
                            }
                        }
                        CallStep::Retry(delay) => {
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                () = stop.clone() => {}
                                () = self.shutdown.cancelled() => {}
                            }
                            Some((None, (attempt + 1, None)))
                        }
                        CallStep::Answer(answer) => Some((Some(answer), (attempt, None))),
                    }
                }
            },
        );
        std::pin::pin!(run.filter_map(|answer| async move { answer }))
            .next()
            .await
            .unwrap_or(Retried::Stopped)
    }

    /// Runs `op`, a store operation of `agent`, under `slot`, in a task of the jobs. The begin
    /// transition and the spawn happen in this function with no await between them, and the end
    /// transition is the last statement of the task, so no return path of a caller can leave a
    /// count or the save flag set. The slot is given back only when `op` returned, or when the
    /// shutdown dropped it. A caller that stops waiting for the handle ends neither the call nor
    /// its slot. When another save of the agent runs, a save does not begin, and the slot comes
    /// back.
    fn store_call_with<S: Send + 'static, T: Send + 'static>(
        &self,
        agent: &AgentSnapshots,
        slot: S,
        kind: CallKind,
        op: impl FnOnce() -> BoxFuture<'static, Result<T, SnapshotStoreError>> + Send + 'static,
    ) -> Result<JoinHandle<CallResult<T>>, S> {
        if !self
            .registry
            .apply(|state| rules::begin_call(state, agent, &kind))
        {
            return Err(slot);
        }
        let (registry, shutdown, agent) = (
            Arc::clone(&self.registry),
            self.shutdown.clone(),
            agent.clone(),
        );
        Ok(self.jobs.spawn(async move {
            let result = tokio::select! {
                biased;
                () = shutdown.cancelled() => CallResult::ShutDown,
                result = op() => CallResult::Done(result),
            };
            drop(slot);
            registry.apply(|state| rules::store_call_ended(state, &agent, &kind));
            result
        }))
    }

    /// Asks whether the store holds the snapshot `name` of `agent`, for at most `limit`. It holds
    /// no slot and does not count: it only reads.
    pub(super) async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        limit: Duration,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        tokio::time::timeout(limit, self.store.stat(agent, name))
            .await
            .unwrap_or_else(|_| {
                Err(SnapshotStoreError::Storage {
                    retryable: true,
                    source: anyhow::anyhow!(
                        "the check of the filesystem snapshot took more than {limit:?}"
                    ),
                })
            })
    }

    /// Lists the snapshots of `agent` under a slot of the uploads. It does not count: it only
    /// reads.
    pub(super) async fn list(
        &self,
        agent: &AgentSnapshots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        let _slot = tokio::select! {
            permit = Arc::clone(&self.uploads).acquire_owned() => permit.ok(),
            () = self.shutdown.cancelled() => None,
        }
        .ok_or_else(|| SnapshotStoreError::Storage {
            retryable: false,
            source: anyhow::anyhow!("the filesystem snapshot service is shut down"),
        })?;
        self.store.list(agent).await
    }

    /// Gives the restore of the snapshot `name` of `agent`. It waits for a slot of the restores
    /// when the lifecycle calls it, and it does not count: it only reads.
    pub(super) fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
    ) -> StoreRestore {
        StoreRestore {
            store: Arc::clone(&self.store),
            agent: agent.clone(),
            name: name.clone(),
            restores: Arc::clone(&self.restores),
        }
    }

    /// Shuts the calls down, in this order: the shutdown drops each call and ends each wait; the
    /// store stops and waits for its own work; the captures of the dropped saves are discarded;
    /// then each task of the jobs ends. Clean-up work that is still pending is lost and counted.
    pub(super) async fn shut_down(&self) {
        self.shutdown.cancel();
        let lost = self.registry.read(rules::pending_cleanups);
        std::iter::repeat_n("shutdown", lost)
            .for_each(crate::metrics::filesystem_snapshots::record_leaked_cleanup);
        self.store.shut_down().await;
        self.store_stopped.cancel();
        self.jobs.close();
        self.jobs.wait().await;
    }

    /// Closes the slots of the uploads, so each wait for one ends.
    #[cfg(test)]
    pub(super) fn close_slots(&self) {
        self.uploads.close();
    }

    /// The number of free slots of the uploads.
    #[cfg(test)]
    pub(super) fn free_slots(&self) -> usize {
        self.uploads.available_permits()
    }

    /// The number of store attempts of uploads that hold a slot now.
    #[cfg(test)]
    pub(super) fn upload_attempts(&self) -> usize {
        self.upload_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The state of one upload between its steps.
#[derive(Default)]
struct UploadRun {
    /// The number of the attempt that runs next, or that ran last.
    attempt: u32,
    slot: Option<UploadSlot>,
    last: Option<Result<SnapshotInfo, SnapshotStoreError>>,
    /// The capture, until it is discarded or handed to the task that discards it.
    capture: Option<CapturedTree>,
}

/// What an upload sees before its next step.
struct UploadSeen {
    /// The number of the attempt that runs next, or that ran last.
    attempt: u32,
    /// Whether a save of the agent runs now.
    save_running: bool,
    /// Whether the upload holds a slot.
    slot: bool,
    /// Whether a stop of the job, of the caller or a lost shard came.
    stopped: bool,
    /// Whether the deadline of the waits passed.
    deadline_passed: bool,
    /// What the last attempt gave, when it has not been decided on.
    last: Option<Result<SnapshotInfo, SnapshotStoreError>>,
    /// The jitter factor for the delay of a retry.
    jitter: f64,
}

/// The next step of an upload.
#[derive(Debug)]
enum UploadStep {
    /// Wait for the end of the running save of the agent.
    WaitForSave,
    /// Wait for a slot of the uploads.
    WaitForSlot,
    /// Save, under the slot that the upload holds.
    Call,
    /// Wait this long, then try again.
    Retry(Duration),
    /// End with this answer.
    Answer(Upload),
}

/// The next step of an upload that sees `seen`. The result of an attempt decides first: a
/// success answers, and a failure retries while `retry` allows and no stop came. Then a stop
/// answers `Stopped`. A running save of the agent and a missing slot are waited for, until the
/// deadline passes.
fn upload_step(seen: UploadSeen, retry: &RetryConfig) -> UploadStep {
    match seen.last {
        Some(Ok(info)) => UploadStep::Answer(Upload::Saved(info)),
        Some(Err(error)) => match rules::retry_delay(retry, seen.attempt, &error, seen.jitter) {
            Some(_) if seen.stopped => UploadStep::Answer(Upload::Stopped),
            Some(delay) => UploadStep::Retry(delay),
            None => UploadStep::Answer(Upload::Failed(UploadError::Store(error))),
        },
        None if seen.stopped => UploadStep::Answer(Upload::Stopped),
        None if seen.save_running && seen.deadline_passed => {
            UploadStep::Answer(Upload::Failed(deadline_error(true)))
        }
        None if seen.save_running => UploadStep::WaitForSave,
        None if !seen.slot && seen.deadline_passed => {
            UploadStep::Answer(Upload::Failed(deadline_error(false)))
        }
        None if !seen.slot => UploadStep::WaitForSlot,
        None => UploadStep::Call,
    }
}

/// What a stop during its save does to an upload, which answers `Stopped` at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnStop {
    /// The save runs on a lost shard: it is cancelled in the store, so it publishes nothing.
    CancelStore,
    /// The save goes on, and its capture is discarded after it returned.
    KeepTail,
}

/// What a stop during its save does to an upload, when `lost_shard` tells whether the shard is
/// lost.
fn on_stop(lost_shard: bool) -> OnStop {
    if lost_shard {
        OnStop::CancelStore
    } else {
        OnStop::KeepTail
    }
}

/// The error of an upload whose deadline passed while it waited: for the running save of the
/// agent when `save_running`, for a slot otherwise.
fn deadline_error(save_running: bool) -> UploadError {
    if save_running {
        UploadError::SaveRunning
    } else {
        UploadError::NoSlot
    }
}

/// What a call with retries gave.
#[derive(Debug)]
enum Retried<T> {
    Done(T),
    Stopped,
    Failed(SnapshotStoreError),
}

/// The next step of a delete or a copy with retries.
#[derive(Debug)]
enum CallStep<T> {
    /// Call the store, under a new slot.
    Call,
    /// Wait this long, then call again.
    Retry(Duration),
    /// End with this answer.
    Answer(Retried<T>),
}

/// The next step of a delete or a copy whose attempt number `attempt` gave `last`. A success
/// answers. A failure retries while `retry` allows and no stop came, and fails otherwise. Before
/// the first attempt and after a retry, a stop answers `Stopped`.
fn call_step<T>(
    attempt: u32,
    stopped: bool,
    last: Option<Result<T, SnapshotStoreError>>,
    jitter: f64,
    retry: &RetryConfig,
) -> CallStep<T> {
    match last {
        Some(Ok(value)) => CallStep::Answer(Retried::Done(value)),
        Some(Err(error)) => match rules::retry_delay(retry, attempt, &error, jitter) {
            Some(_) if stopped => CallStep::Answer(Retried::Stopped),
            Some(delay) => CallStep::Retry(delay),
            None => CallStep::Answer(Retried::Failed(error)),
        },
        None if stopped => CallStep::Answer(Retried::Stopped),
        None => CallStep::Call,
    }
}

/// Whether the shutdown dropped the save that gave `result`, so its tail waits until the store
/// stopped before it discards the capture.
fn dropped_by_the_shutdown<T>(result: &Result<CallResult<T>, tokio::task::JoinError>) -> bool {
    matches!(result, Ok(CallResult::ShutDown) | Err(_))
}

/// Completes at `deadline`, and never without one.
async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Draws the jitter factor of a retry delay below the `max_jitter_factor` of `retry`. The
/// settings allow a factor from 0 to 1, and a factor of 0 gives no jitter.
fn jitter(retry: &RetryConfig) -> f64 {
    retry
        .max_jitter_factor
        .filter(|factor| *factor > 0.0)
        .map_or(0.0, |factor| rand::rng().random_range(0.0..factor))
}

/// The restore of one filesystem snapshot. It waits for a slot of the restores when the
/// lifecycle calls it, and gives the slot back when it ends, with success or with an error.
pub(crate) struct StoreRestore {
    store: Arc<dyn FilesystemSnapshotStore>,
    agent: AgentSnapshots,
    name: FilesystemSnapshotName,
    restores: Arc<Semaphore>,
}

impl RestoreTree for StoreRestore {
    async fn restore(self, into: &Path) -> Result<(), RestoreError> {
        let name = super::store_name(&self.name).map_err(|error| RestoreError {
            retryable: false,
            source: anyhow::Error::new(error),
        })?;
        let _slot = self
            .restores
            .acquire_owned()
            .await
            .map_err(|error| RestoreError {
                retryable: true,
                source: anyhow::Error::new(error).context("wait for a slot of the restores"),
            })?;
        let started = std::time::Instant::now();
        let result = self.store.restore(&self.agent, &name, into).await;
        crate::metrics::filesystem_snapshots::record_restore(
            if result.is_ok() { "restored" } else { "failed" },
            started.elapsed(),
        );
        result.map(drop).map_err(|error| RestoreError {
            retryable: matches!(
                error,
                SnapshotStoreError::Storage {
                    retryable: true,
                    ..
                }
            ),
            source: anyhow::Error::new(error)
                .context(format!("restore the filesystem snapshot {}", self.name)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use test_r::test;

    fn retry(max_attempts: u32) -> RetryConfig {
        RetryConfig {
            max_attempts,
            min_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            multiplier: 4.0,
            max_jitter_factor: None,
        }
    }

    #[test]
    fn a_jitter_is_drawn_below_a_positive_factor_and_is_zero_otherwise() {
        let retry = |max_jitter_factor| RetryConfig {
            max_jitter_factor,
            ..retry(3)
        };
        let drawn = (0..200)
            .map(|_| jitter(&retry(Some(0.5))))
            .collect::<Vec<_>>();

        assert_eq!(
            (jitter(&retry(None)), jitter(&retry(Some(0.0)))),
            (0.0, 0.0)
        );
        assert!(drawn.iter().all(|factor| (0.0..0.5).contains(factor)));
        assert!(drawn.iter().any(|factor| *factor > 0.0));
    }

    fn info() -> SnapshotInfo {
        SnapshotInfo {
            created_at: Timestamp::from(1),
            files: 1,
            bytes: 2,
        }
    }

    fn storage(retryable: bool) -> SnapshotStoreError {
        SnapshotStoreError::Storage {
            retryable,
            source: anyhow::anyhow!("storage"),
        }
    }

    /// What an upload sees with no attempt yet, a free slot, and nothing else.
    fn seen() -> UploadSeen {
        UploadSeen {
            attempt: 1,
            save_running: false,
            slot: true,
            stopped: false,
            deadline_passed: false,
            last: None,
            jitter: 0.0,
        }
    }

    fn step_of(seen: UploadSeen) -> String {
        format!("{:?}", upload_step(seen, &retry(3)))
    }

    #[test]
    fn an_upload_calls_only_with_a_slot_and_without_a_running_save_of_its_agent() {
        assert_eq!(
            [
                step_of(seen()),
                step_of(UploadSeen {
                    slot: false,
                    ..seen()
                }),
                step_of(UploadSeen {
                    save_running: true,
                    ..seen()
                }),
                step_of(UploadSeen {
                    save_running: true,
                    slot: false,
                    ..seen()
                }),
            ],
            ["Call", "WaitForSlot", "WaitForSave", "WaitForSave"].map(String::from)
        );
    }

    #[test]
    fn a_stop_before_the_call_answers_stopped_at_each_wait() {
        assert_eq!(
            [
                step_of(UploadSeen {
                    stopped: true,
                    ..seen()
                }),
                step_of(UploadSeen {
                    stopped: true,
                    slot: false,
                    ..seen()
                }),
                step_of(UploadSeen {
                    stopped: true,
                    save_running: true,
                    deadline_passed: true,
                    ..seen()
                }),
                step_of(UploadSeen {
                    stopped: true,
                    last: Some(Err(storage(true))),
                    ..seen()
                }),
            ],
            ["Answer(Stopped)"; 4].map(String::from)
        );
    }

    #[test]
    fn a_passed_deadline_fails_only_a_wait_and_names_its_cause() {
        assert_eq!(
            [
                step_of(UploadSeen {
                    deadline_passed: true,
                    save_running: true,
                    ..seen()
                }),
                step_of(UploadSeen {
                    deadline_passed: true,
                    slot: false,
                    ..seen()
                }),
                step_of(UploadSeen {
                    deadline_passed: true,
                    ..seen()
                }),
            ],
            [
                "Answer(Failed(SaveRunning))",
                "Answer(Failed(NoSlot))",
                "Call"
            ]
            .map(String::from)
        );
    }

    #[test]
    fn the_result_of_an_attempt_answers_or_retries_within_the_budget() {
        assert_eq!(
            [
                step_of(UploadSeen {
                    last: Some(Ok(info())),
                    stopped: true,
                    ..seen()
                }),
                step_of(UploadSeen {
                    last: Some(Err(storage(true))),
                    ..seen()
                }),
                step_of(UploadSeen {
                    attempt: 3,
                    last: Some(Err(storage(true))),
                    ..seen()
                }),
                step_of(UploadSeen {
                    last: Some(Err(storage(false))),
                    ..seen()
                }),
            ],
            [
                format!("Answer(Saved({:?}))", info()),
                "Retry(2s)".to_string(),
                "Answer(Failed(Store(Storage { retryable: true, source: storage })))".to_string(),
                "Answer(Failed(Store(Storage { retryable: false, source: storage })))".to_string(),
            ]
        );
    }

    #[test]
    fn only_a_stop_during_a_save_on_a_lost_shard_cancels_the_save() {
        assert_eq!(
            [false, true].map(on_stop),
            [OnStop::KeepTail, OnStop::CancelStore]
        );
    }

    #[test]
    fn the_deadline_error_names_the_wait_that_it_ended() {
        assert_eq!(
            [true, false].map(|save_running| format!("{:?}", deadline_error(save_running))),
            ["SaveRunning", "NoSlot"].map(String::from)
        );
    }

    fn call_step_of(
        attempt: u32,
        stopped: bool,
        last: Option<Result<(), SnapshotStoreError>>,
    ) -> String {
        format!("{:?}", call_step(attempt, stopped, last, 0.0, &retry(3)))
    }

    #[test]
    fn a_delete_or_a_copy_calls_retries_within_the_budget_and_stops_only_between_attempts() {
        assert_eq!(
            [
                call_step_of(1, false, None),
                call_step_of(1, true, None),
                call_step_of(1, true, Some(Ok(()))),
                call_step_of(1, false, Some(Err(storage(true)))),
                call_step_of(1, true, Some(Err(storage(true)))),
                call_step_of(3, false, Some(Err(storage(true)))),
                call_step_of(1, false, Some(Err(storage(false)))),
            ],
            [
                "Call",
                "Answer(Stopped)",
                "Answer(Done(()))",
                "Retry(2s)",
                "Answer(Stopped)",
                "Answer(Failed(Storage { retryable: true, source: storage }))",
                "Answer(Failed(Storage { retryable: false, source: storage }))",
            ]
            .map(String::from)
        );
    }
}
