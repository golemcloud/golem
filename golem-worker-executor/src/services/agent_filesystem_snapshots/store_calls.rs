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
//! returns, also when its caller stops waiting for it. The store takes a slot of the limiter of
//! each call for each run of the call; the limiter of this module withdraws a call at a stop, and
//! at the deadline of a manual update. A stop ends only waits. Only a shutdown drops a call. The
//! slots of the store operations, the rule of one save at a time for each agent, the discard of a
//! capture, the cancel of a save on a lost shard, and the order of the shutdown are hidden here.
//! Each decision is a private pure function.

use super::CapturedTree;
use super::registry::{JobRuns, JobTicket, Registry};
use super::rules::{self, CallKind};
use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, Failed, FilesystemSnapshotStore, ReadError,
    RestoreFailure, RunSlots, SaveError, Slot, SnapshotInfo, SnapshotName, Withdrawal,
};
use crate::services::agent_filesystem::{RestoreError, RestoreTree};
use crate::services::golem_config::{
    FilesystemSnapshotStoreConfig, FilesystemSnapshotUploadConfig,
};
use futures::StreamExt as _;
use futures::future::{BoxFuture, FutureExt as _, Shared};
use golem_common::model::oplog::FilesystemSnapshotName;
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
    /// The save failed.
    Store(SaveError),
    /// The deadline passed while the upload waited for the running save of the agent.
    SaveRunning,
    /// The deadline passed while the upload waited for a slot of the uploads, and no run of the
    /// save failed.
    NoSlot,
}

/// What a delete gave.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub(super) enum Deleted {
    /// The store deleted the snapshots.
    Deleted,
    /// A stop or the shutdown ended the delete. A run that ran then goes on.
    Stopped,
    /// The delete failed.
    Leaked(Failed),
}

/// A stop that many owners wait for.
type Stop = Shared<BoxFuture<'static, ()>>;

/// What stops an upload, beside the stop of its job.
pub(super) struct Stops {
    /// The stop of the caller: a terminal interrupt, or a caller that stopped waiting.
    caller: Stop,
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

    /// A stop that completes when the caller stops, the shard is lost, `job` is cancelled, or
    /// `shutdown` is cancelled.
    fn with(&self, job: CancellationToken, shutdown: CancellationToken) -> Stop {
        let (caller, lost_shard) = (self.caller.clone(), self.lost_shard.clone());
        async move {
            tokio::select! {
                () = caller => {}
                () = lost_shard_set(lost_shard) => {}
                () = job.cancelled() => {}
                () = shutdown.cancelled() => {}
            }
        }
        .boxed()
        .shared()
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

/// A stop that completes when `stop` completes or `shutdown` is cancelled.
fn or_shutdown(stop: Stop, shutdown: CancellationToken) -> Stop {
    async move {
        tokio::select! {
            () = stop => {}
            () = shutdown.cancelled() => {}
        }
    }
    .boxed()
    .shared()
}

/// How a store call ended.
enum CallResult<T> {
    /// The call returned.
    Done(T),
    /// The shutdown dropped the call.
    ShutDown,
}

/// What a take of a slot does, as [`take_step`] decides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TakeStep {
    Withdraw(Withdrawal),
    Grant,
    Wait,
}

/// Decides a take of a slot. The order is fixed: a stop first; after the deadline, only an
/// immediate take, which no wait came before, with a free slot is granted, and every other take is
/// withdrawn at once; before the deadline, a free slot is granted, and otherwise the take waits.
fn take_step(stopped: bool, slot_free: bool, deadline_passed: bool, immediate: bool) -> TakeStep {
    match (stopped, deadline_passed, immediate, slot_free) {
        (true, _, _, _) => TakeStep::Withdraw(Withdrawal::Stopped),
        (false, true, true, true) | (false, false, _, true) => TakeStep::Grant,
        (false, true, _, _) => TakeStep::Withdraw(Withdrawal::Deadline),
        (false, false, _, false) => TakeStep::Wait,
    }
}

/// What a grant of a slot reports.
enum Grant {
    /// A run of the save of a job: the job is saving from its first grant, a grant for a job that
    /// is no longer live is refused, and the slot counts in the gauge of the uploads.
    Saving(JobRuns),
    Nothing,
}

/// The limiter of one store call of the service: a semaphore of the slots, the stops that withdraw
/// the call, and the deadline of the waits of a manual update.
struct ServiceSlots {
    semaphore: Arc<Semaphore>,
    /// The stops that withdraw the call, as each call site gives them; the shutdown is in each.
    stop: Stop,
    /// The deadline of the waits of a manual update.
    deadline: Option<Instant>,
    on_grant: Grant,
    /// The runs of saves that hold a slot now, for the tests.
    #[cfg(test)]
    upload_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

/// The slot of one run of a save. It counts the run in the gauge of the uploads in progress while
/// it lives.
struct SaveSlot {
    _permit: OwnedSemaphorePermit,
    #[cfg(test)]
    attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for SaveSlot {
    fn drop(&mut self) {
        crate::metrics::filesystem_snapshots::dec_uploads_in_progress();
        #[cfg(test)]
        self.attempts
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl ServiceSlots {
    /// Whether a stop came.
    fn stopped(&self) -> bool {
        self.stop.clone().now_or_never().is_some()
    }

    /// Whether the deadline passed.
    fn deadline_passed(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Gives the slot of `permit` as the grant reports it.
    fn grant(&self, permit: OwnedSemaphorePermit) -> Result<Slot, Withdrawal> {
        match &self.on_grant {
            Grant::Nothing => Ok(Slot::new(permit)),
            Grant::Saving(runs) => {
                if !runs.granted() {
                    return Err(Withdrawal::Stopped);
                }
                crate::metrics::filesystem_snapshots::inc_uploads_in_progress();
                #[cfg(test)]
                self.upload_attempts
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Slot::new(SaveSlot {
                    _permit: permit,
                    #[cfg(test)]
                    attempts: Arc::clone(&self.upload_attempts),
                }))
            }
        }
    }

    /// Decides a take with `permit`, the slot that is free now.
    fn decide(
        &self,
        permit: Option<OwnedSemaphorePermit>,
        immediate: bool,
    ) -> Result<Result<Slot, Withdrawal>, ()> {
        match take_step(
            self.stopped(),
            permit.is_some(),
            self.deadline_passed(),
            immediate,
        ) {
            TakeStep::Withdraw(cause) => Ok(Err(cause)),
            TakeStep::Grant => match permit {
                Some(permit) => Ok(self.grant(permit)),
                None => Err(()),
            },
            TakeStep::Wait => Err(()),
        }
    }
}

impl RunSlots for ServiceSlots {
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        async move {
            let free = Arc::clone(&self.semaphore).try_acquire_owned();
            if matches!(free, Err(tokio::sync::TryAcquireError::Closed)) {
                return Err(Withdrawal::Stopped);
            }
            if let Ok(decided) = self.decide(free.ok(), immediate) {
                return decided;
            }
            let woken = tokio::select! {
                biased;
                () = self.stop.clone() => None,
                permit = Arc::clone(&self.semaphore).acquire_owned() => Some(permit),
                () = until(self.deadline) => None,
            };
            match woken {
                Some(Err(_closed)) => Err(Withdrawal::Stopped),
                Some(Ok(permit)) => self
                    .decide(Some(permit), immediate)
                    .unwrap_or(Err(Withdrawal::Stopped)),
                None => self
                    .decide(None, immediate)
                    .unwrap_or(Err(Withdrawal::Stopped)),
            }
        }
        .boxed()
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        async move {
            tokio::select! {
                biased;
                () = self.stop.clone() => Withdrawal::Stopped,
                () = until(self.deadline) => Withdrawal::Deadline,
            }
        }
        .boxed()
    }

    fn waiting_after_failure(&self) {
        if let Grant::Saving(runs) = &self.on_grant {
            runs.failed();
        }
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
    /// The tasks of the calls, and of the discards that follow a call whose caller stopped.
    jobs: TaskTracker,
    /// Cancelled when the executor shuts down. It drops each call.
    shutdown: CancellationToken,
    /// Cancelled when the store has stopped, after the shutdown. A capture whose save the
    /// shutdown dropped is discarded only then.
    store_stopped: CancellationToken,
    /// The runs of saves that hold a slot now: a count of the same runs as the gauge of the
    /// metrics, which all services of the process share.
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
            jobs,
            shutdown,
            store_stopped: CancellationToken::new(),
            #[cfg(test)]
            upload_attempts: Arc::default(),
        }
    }

    /// Gives the limiter of a call on `semaphore`, withdrawn by `stop`, with the deadline and the
    /// report of a grant.
    fn slots(
        &self,
        semaphore: &Arc<Semaphore>,
        stop: Stop,
        deadline: Option<Instant>,
        on_grant: Grant,
    ) -> ServiceSlots {
        ServiceSlots {
            semaphore: Arc::clone(semaphore),
            stop,
            deadline,
            on_grant,
            #[cfg(test)]
            upload_attempts: Arc::clone(&self.upload_attempts),
        }
    }

    /// Saves `capture` under `name` for the job of `job`, one save at a time for the agent. The
    /// store takes a slot of the uploads for each run, and each grant marks the job as saving. A
    /// stop of the job or of `stops` gives `Stopped` at once: it ends a wait for the running save
    /// of the agent, and a save that runs then goes on inside, until it returns; the store ends its
    /// wait for a slot or between two runs at once. A lost shard also cancels that save in the
    /// store. With a `deadline`, a wait for the running save of the agent that reaches it gives
    /// `SaveRunning`, and a call that the deadline withdrew gives `NoSlot` when no run of it failed.
    /// The capture is always discarded, after the last save that reads it returned; a save that the
    /// shutdown dropped is followed by the stop of the store first.
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
        let seen = UploadSeen {
            save_running: self
                .registry
                .read(|state| rules::save_running(state, agent)),
            stopped: job.stop_requested() || stops.stopped(),
            deadline_passed: deadline.is_some_and(|deadline| Instant::now() >= deadline),
            last: run.last.take(),
        };
        match upload_step(seen) {
            UploadStep::WaitForSave => {
                tokio::select! {
                    () = self.registry.until_save_may_start(agent) => {}
                    () = job.until_stopped() => {}
                    () = stops.fired() => {}
                    () = until(deadline) => {}
                }
                (None, run)
            }
            UploadStep::Call => {
                let Some(directory) = run
                    .capture
                    .as_ref()
                    .map(|capture| Arc::clone(&capture.directory))
                else {
                    return (Some(Upload::Stopped), run);
                };
                let cancel = CancellationToken::new();
                let slots = self.slots(
                    &self.uploads,
                    stops.with(job.stop_token(), self.shutdown.clone()),
                    deadline,
                    Grant::Saving(job.runs()),
                );
                match self.save_call(
                    agent,
                    slots,
                    name,
                    directory,
                    parent.clone(),
                    cancel.clone(),
                ) {
                    Some(handle) => self.await_save(run, handle, cancel, job, stops).await,
                    // Another save of the agent started first: the upload waits for that save.
                    None => (None, run),
                }
            }
            UploadStep::Answer(upload) => (Some(upload), run),
        }
    }

    /// Starts a save of the tree in `directory` under `name` with `parent`, with the limiter
    /// `slots` and with `cancel` as the cancel of the save in the store. Gives `None` when another
    /// save of the agent runs.
    fn save_call(
        &self,
        agent: &AgentSnapshots,
        slots: ServiceSlots,
        name: &SnapshotName,
        directory: Arc<Path>,
        parent: Option<Arc<(SnapshotName, ChangeDetection)>>,
        cancel: CancellationToken,
    ) -> Option<JoinHandle<CallResult<Result<SnapshotInfo, SaveError>>>> {
        let (store, agent_of_call, name) = (Arc::clone(&self.store), agent.clone(), name.clone());
        self.store_call_with(agent, CallKind::Save, move || {
            async move {
                let parent = parent
                    .as_deref()
                    .map(|(parent, detection)| (parent, *detection));
                store
                    .save(&agent_of_call, &name, &directory, parent, &cancel, &slots)
                    .await
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
        mut handle: JoinHandle<CallResult<Result<SnapshotInfo, SaveError>>>,
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

    /// Deletes `names` of `agent` as one batch, in one counted call. `stop` or the shutdown
    /// withdraws the call, and `stop` answers `Stopped` at once, also while a run of the call runs,
    /// which then goes on inside.
    pub(super) async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: Arc<[SnapshotName]>,
        stop: impl Future<Output = ()> + Send + 'static,
    ) -> Deleted {
        let (store, agent_of_call) = (Arc::clone(&self.store), agent.clone());
        self.deleting(agent, stop.boxed().shared(), move |slots| {
            async move { store.delete(&agent_of_call, &names, &slots).await }.boxed()
        })
        .await
    }

    /// Deletes every snapshot of `agent`, in one counted call. Only the shutdown ends it.
    pub(super) async fn delete_all(&self, agent: &AgentSnapshots) -> Deleted {
        let (store, agent_of_call) = (Arc::clone(&self.store), agent.clone());
        self.deleting(
            agent,
            std::future::pending().boxed().shared(),
            move |slots| async move { store.delete_all(&agent_of_call, &slots).await }.boxed(),
        )
        .await
    }

    /// Runs the delete that `call` makes in one call counted for `agent`, with a limiter on the
    /// slots of the uploads that `stop` and the shutdown withdraw. `stop` answers `Stopped` at
    /// once.
    async fn deleting(
        &self,
        agent: &AgentSnapshots,
        stop: Stop,
        call: impl FnOnce(ServiceSlots) -> BoxFuture<'static, Result<(), CallError>> + Send + 'static,
    ) -> Deleted {
        let slots = self.slots(
            &self.uploads,
            or_shutdown(stop.clone(), self.shutdown.clone()),
            None,
            Grant::Nothing,
        );
        let Some(mut handle) = self.store_call_with(agent, CallKind::Other, move || call(slots))
        else {
            return Deleted::Stopped;
        };
        let returned = tokio::select! {
            biased;
            result = &mut handle => result.unwrap_or(CallResult::ShutDown),
            () = stop => CallResult::ShutDown,
        };
        match returned {
            CallResult::Done(Ok(())) => Deleted::Deleted,
            CallResult::Done(Err(CallError::Stopped(_))) | CallResult::ShutDown => Deleted::Stopped,
            CallResult::Done(Err(CallError::Failed(failed))) => Deleted::Leaked(failed),
        }
    }

    /// Copies every snapshot of `from` into `to`, in one call that counts on both agents until it
    /// returns, also when the caller stops waiting. Only the shutdown withdraws it.
    pub(super) async fn copy(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
    ) -> Result<(), CallError> {
        let (store, from_of_call, to_of_call) = (Arc::clone(&self.store), from.clone(), to.clone());
        let slots = self.slots(
            &self.uploads,
            or_shutdown(
                std::future::pending().boxed().shared(),
                self.shutdown.clone(),
            ),
            None,
            Grant::Nothing,
        );
        let Some(handle) =
            self.store_call_with(from, CallKind::Copy { to: to.clone() }, move || {
                async move { store.copy_all(&from_of_call, &to_of_call, &slots).await }.boxed()
            })
        else {
            return Err(CallError::Stopped(Withdrawal::Stopped));
        };
        let started = Instant::now();
        let copied = match handle.await.unwrap_or(CallResult::ShutDown) {
            CallResult::Done(result) => result,
            CallResult::ShutDown => Err(CallError::Stopped(Withdrawal::Stopped)),
        };
        crate::metrics::filesystem_snapshots::record_copy(
            match &copied {
                Ok(()) => "copied",
                Err(CallError::Stopped(_)) => "stopped",
                Err(CallError::Failed(_)) => "failed",
            },
            started.elapsed(),
        );
        copied
    }

    /// Runs `op`, a store operation of `agent`, in a task of the jobs. The begin transition and
    /// the spawn happen in this function with no await between them, and the end transition is
    /// the last statement of the task, so no return path of a caller can leave a count or the
    /// save flag set. A caller that stops waiting for the handle does not end the call. When
    /// another save of the agent runs, a save does not begin, and the function gives `None`.
    fn store_call_with<T: Send + 'static>(
        &self,
        agent: &AgentSnapshots,
        kind: CallKind,
        op: impl FnOnce() -> BoxFuture<'static, T> + Send + 'static,
    ) -> Option<JoinHandle<CallResult<T>>> {
        if !self
            .registry
            .apply(|state| rules::begin_call(state, agent, &kind))
        {
            return None;
        }
        let (registry, shutdown, agent) = (
            Arc::clone(&self.registry),
            self.shutdown.clone(),
            agent.clone(),
        );
        Some(self.jobs.spawn(async move {
            let result = tokio::select! {
                biased;
                () = shutdown.cancelled() => CallResult::ShutDown,
                result = op() => CallResult::Done(result),
            };
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
    ) -> Result<Option<SnapshotInfo>, ReadError> {
        tokio::time::timeout(limit, self.store.stat(agent, name))
            .await
            .unwrap_or_else(|_| {
                Err(ReadError::Failed(Failed::new(anyhow::anyhow!(
                    "the check of the filesystem snapshot took more than {limit:?}"
                ))))
            })
    }

    /// Lists the snapshots of `agent`, with a limiter on the slots of the uploads that `stop` and
    /// the shutdown withdraw. It does not count: it only reads.
    pub(super) async fn list(
        &self,
        agent: &AgentSnapshots,
        stop: impl Future<Output = ()> + Send + 'static,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError> {
        let slots = self.slots(
            &self.uploads,
            or_shutdown(stop.boxed().shared(), self.shutdown.clone()),
            None,
            Grant::Nothing,
        );
        self.store.list(agent, &slots).await
    }

    /// Gives the restore of the snapshot `name` of `agent`. Each run of it takes a slot of the
    /// restores when the lifecycle calls it, and it does not count: it only reads.
    pub(super) fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
    ) -> StoreRestore {
        StoreRestore {
            store: Arc::clone(&self.store),
            agent: agent.clone(),
            name: name.clone(),
            slots: self.slots(
                &self.restores,
                or_shutdown(
                    std::future::pending().boxed().shared(),
                    self.shutdown.clone(),
                ),
                None,
                Grant::Nothing,
            ),
        }
    }

    /// Shuts the calls down, in this order: the shutdown drops each call and ends each wait; the
    /// store stops and waits for its own work; the captures of the dropped saves are discarded;
    /// then each task of the jobs ends. Clean-up work that is still pending is lost and counted.
    pub(super) async fn shut_down(&self) {
        self.shutdown.cancel();
        let lost = self.registry.read(rules::cleanups_lost_at_shutdown);
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

    /// The number of runs of saves that hold a slot now.
    #[cfg(test)]
    pub(super) fn upload_attempts(&self) -> usize {
        self.upload_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The state of one upload between its steps.
struct UploadRun {
    /// What the save gave, when it has not been decided on.
    last: Option<Result<SnapshotInfo, SaveError>>,
    /// The capture, until it is discarded or handed to the task that discards it.
    capture: Option<CapturedTree>,
}

/// What an upload sees before its next step.
struct UploadSeen {
    /// Whether a save of the agent runs now.
    save_running: bool,
    /// Whether a stop of the job, of the caller or a lost shard came.
    stopped: bool,
    /// Whether the deadline of the waits passed.
    deadline_passed: bool,
    /// What the save gave, when it has not been decided on.
    last: Option<Result<SnapshotInfo, SaveError>>,
}

/// The next step of an upload.
#[derive(Debug)]
enum UploadStep {
    /// Wait for the end of the running save of the agent.
    WaitForSave,
    /// Save.
    Call,
    /// End with this answer.
    Answer(Upload),
}

/// The next step of an upload that sees `seen`. A save that succeeded answers, also after a stop.
/// Then a stop answers `Stopped`, also after a failed save. A save that the deadline withdrew
/// without a failed run answers `NoSlot`, and each other failure answers with its error. A
/// running save of the agent is waited for, until the deadline passes.
fn upload_step(seen: UploadSeen) -> UploadStep {
    match seen.last {
        Some(Ok(info)) => UploadStep::Answer(Upload::Saved(info)),
        Some(Err(_)) if seen.stopped => UploadStep::Answer(Upload::Stopped),
        Some(Err(SaveError::Stopped(Withdrawal::Deadline))) => {
            UploadStep::Answer(Upload::Failed(UploadError::NoSlot))
        }
        Some(Err(SaveError::Stopped(Withdrawal::Stopped))) => UploadStep::Answer(Upload::Stopped),
        Some(Err(error)) => UploadStep::Answer(Upload::Failed(UploadError::Store(error))),
        None if seen.stopped => UploadStep::Answer(Upload::Stopped),
        None if seen.save_running && seen.deadline_passed => {
            UploadStep::Answer(Upload::Failed(UploadError::SaveRunning))
        }
        None if seen.save_running => UploadStep::WaitForSave,
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

/// The restore of one filesystem snapshot. Each run of it waits for a slot of the restores when
/// the lifecycle calls it, and gives the slot back when the run ends.
pub(crate) struct StoreRestore {
    store: Arc<dyn FilesystemSnapshotStore>,
    agent: AgentSnapshots,
    name: FilesystemSnapshotName,
    slots: ServiceSlots,
}

/// Whether a restore that failed with `failure` can succeed when the start runs again: `Failed`
/// and `Stopped` can, and `NotFound`, `Corrupt` and `Destination` give the same answer again.
fn restore_retryable(failure: &RestoreFailure) -> bool {
    match failure {
        RestoreFailure::Failed(_) | RestoreFailure::Stopped(_) => true,
        RestoreFailure::NotFound | RestoreFailure::Corrupt(_) | RestoreFailure::Destination(_) => {
            false
        }
    }
}

impl RestoreTree for StoreRestore {
    async fn restore(self, into: &Path) -> Result<(), RestoreError> {
        let name = super::store_name(&self.name).map_err(|error| RestoreError {
            retryable: false,
            source: anyhow::Error::new(error),
        })?;
        let started = std::time::Instant::now();
        let result = self
            .store
            .restore(&self.agent, &name, into, &self.slots)
            .await;
        crate::metrics::filesystem_snapshots::record_restore(
            if result.is_ok() { "restored" } else { "failed" },
            started.elapsed(),
        );
        result.map(drop).map_err(|error| RestoreError {
            retryable: restore_retryable(&error),
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

    fn info() -> SnapshotInfo {
        SnapshotInfo {
            created_at: Timestamp::from(1),
            files: 1,
            bytes: 2,
        }
    }

    fn failed() -> SaveError {
        SaveError::Failed(Failed::new(anyhow::anyhow!("storage")))
    }

    /// What an upload sees with no save yet and nothing else.
    fn seen() -> UploadSeen {
        UploadSeen {
            save_running: false,
            stopped: false,
            deadline_passed: false,
            last: None,
        }
    }

    fn step_of(seen: UploadSeen) -> String {
        format!("{:?}", upload_step(seen))
    }

    #[test]
    fn an_upload_calls_only_without_a_running_save_of_its_agent() {
        assert_eq!(
            [
                step_of(seen()),
                step_of(UploadSeen {
                    save_running: true,
                    ..seen()
                }),
                step_of(UploadSeen {
                    deadline_passed: true,
                    ..seen()
                }),
            ],
            ["Call", "WaitForSave", "Call"].map(String::from)
        );
    }

    #[test]
    fn an_error_after_a_stop_answers_stopped_and_a_stop_answers_stopped_at_each_wait() {
        assert_eq!(
            [
                step_of(UploadSeen {
                    stopped: true,
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
                    last: Some(Err(failed())),
                    ..seen()
                }),
                step_of(UploadSeen {
                    stopped: true,
                    last: Some(Err(SaveError::Stopped(Withdrawal::Deadline))),
                    ..seen()
                }),
                step_of(UploadSeen {
                    stopped: true,
                    last: Some(Ok(info())),
                    ..seen()
                }),
            ],
            [
                "Answer(Stopped)".to_string(),
                "Answer(Stopped)".to_string(),
                "Answer(Stopped)".to_string(),
                "Answer(Stopped)".to_string(),
                format!("Answer(Saved({:?}))", info()),
            ]
        );
    }

    #[test]
    fn the_answer_of_the_save_gives_the_answer_of_the_upload_by_its_cause() {
        assert_eq!(
            [
                step_of(UploadSeen {
                    last: Some(Err(SaveError::Stopped(Withdrawal::Deadline))),
                    ..seen()
                }),
                step_of(UploadSeen {
                    last: Some(Err(SaveError::Stopped(Withdrawal::Stopped))),
                    ..seen()
                }),
                step_of(UploadSeen {
                    last: Some(Err(failed())),
                    ..seen()
                }),
                step_of(UploadSeen {
                    last: Some(Err(SaveError::NameInUse)),
                    ..seen()
                }),
                step_of(UploadSeen {
                    deadline_passed: true,
                    save_running: true,
                    ..seen()
                }),
            ],
            [
                "Answer(Failed(NoSlot))",
                "Answer(Stopped)",
                "Answer(Failed(Store(Failed(Failed(storage)))))",
                "Answer(Failed(Store(NameInUse)))",
                "Answer(Failed(SaveRunning))",
            ]
            .map(String::from)
        );
    }

    #[test]
    fn take_step_withdraws_at_a_stop_first_and_grants_after_the_deadline_only_an_immediate_take_with_a_free_slot()
     {
        let rows = [
            (true, true, true, true),
            (true, false, false, false),
            (false, true, true, false),
            (false, false, true, false),
            (false, true, true, true),
            (false, false, true, true),
            (false, true, false, false),
            (false, false, false, false),
            (false, true, false, true),
            (false, false, false, true),
        ];

        assert_eq!(
            rows.map(
                |(stopped, slot_free, deadline_passed, immediate)| take_step(
                    stopped,
                    slot_free,
                    deadline_passed,
                    immediate
                )
            ),
            [
                TakeStep::Withdraw(Withdrawal::Stopped),
                TakeStep::Withdraw(Withdrawal::Stopped),
                TakeStep::Withdraw(Withdrawal::Deadline),
                TakeStep::Withdraw(Withdrawal::Deadline),
                TakeStep::Grant,
                TakeStep::Withdraw(Withdrawal::Deadline),
                TakeStep::Grant,
                TakeStep::Wait,
                TakeStep::Grant,
                TakeStep::Wait,
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
    fn a_failed_or_stopped_restore_is_retryable_and_the_other_failures_are_not() {
        assert_eq!(
            [
                RestoreFailure::Failed(Failed::new(anyhow::anyhow!("storage"))),
                RestoreFailure::Stopped(Withdrawal::Stopped),
                RestoreFailure::NotFound,
                RestoreFailure::Corrupt(anyhow::anyhow!("corrupt")),
                RestoreFailure::Destination(std::io::Error::other("full")),
            ]
            .map(|failure| restore_retryable(&failure)),
            [true, true, false, false, false]
        );
    }

    /// A periodic job of a new agent whose first run failed, so it waits for its next run.
    fn job_after_a_failed_run(registry: &Arc<Registry>) -> (AgentSnapshots, JobTicket) {
        let agent = AgentSnapshots::agent(
            &golem_common::model::OwnedAgentId::new(
                golem_common::model::environment::EnvironmentId::new(),
                &golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "next-take".to_string(),
                },
            ),
            golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        );
        let (job, _) = JobTicket::admit(
            registry,
            &agent,
            &FilesystemSnapshotName::periodic(),
            super::super::SnapshotKind::Periodic,
            CancellationToken::new(),
            true,
        )
        .unwrap_or_else(|_| panic!("the first job is admitted"));
        assert!(job.runs().granted());
        job.runs().failed();
        (agent, job)
    }

    /// The limiter of the next run of the save of `job`, with one slot that no stop withdraws.
    fn next_run_slots(semaphore: &Arc<Semaphore>, job: &JobTicket) -> ServiceSlots {
        ServiceSlots {
            semaphore: Arc::clone(semaphore),
            stop: futures::future::pending().boxed().shared(),
            deadline: None,
            on_grant: Grant::Saving(job.runs()),
            upload_attempts: Arc::default(),
        }
    }

    /// A new periodic admission for `agent`: whether it replaced a job, or why it was refused.
    fn admit_new(
        registry: &Arc<Registry>,
        agent: &AgentSnapshots,
    ) -> Result<bool, super::super::SnapshotSkip> {
        JobTicket::admit(
            registry,
            agent,
            &FilesystemSnapshotName::periodic(),
            super::super::SnapshotKind::Periodic,
            CancellationToken::new(),
            true,
        )
        .map(|(_, replaced)| replaced.is_some())
        .map_err(|refusal| refusal.skip)
    }

    #[test]
    async fn the_old_jobs_next_take_and_a_new_admission_race_in_both_orders() {
        // The admission first: it replaces the waiting job, and the next take of the old job then
        // gets a free slot, which its grant refuses and gives back. The take first: the grant
        // makes the old job run again, and the admission is refused.
        let registry = Arc::new(Registry::default());
        let semaphore = Arc::new(Semaphore::new(1));
        let (agent, old) = job_after_a_failed_run(&registry);
        let replaced = admit_new(&registry, &agent);
        let refused_take = next_run_slots(&semaphore, &old).take(false).await.err();
        let permits_after_refusal = semaphore.available_permits();

        let (other_agent, other_old) = job_after_a_failed_run(&registry);
        let granted_take = next_run_slots(&semaphore, &other_old).take(false).await;
        let refused_admission = admit_new(&registry, &other_agent);
        let permits_while_granted = semaphore.available_permits();
        drop(granted_take);

        assert_eq!(
            (
                replaced,
                refused_take,
                permits_after_refusal,
                refused_admission,
                permits_while_granted,
            ),
            (
                Ok(true),
                Some(Withdrawal::Stopped),
                1,
                Err(super::super::SnapshotSkip::UploadInFlight),
                0,
            )
        );
    }

    /// The answers of an upload carry a [`SaveError`] of 16 bytes and add no tag of their own;
    /// `Upload` has the size of the answer of a save, `Result<SnapshotInfo, SaveError>`.
    #[test]
    fn the_answers_of_an_upload_stay_small() {
        assert_eq!(
            [
                std::mem::size_of::<UploadError>(),
                std::mem::size_of::<Upload>(),
                std::mem::size_of::<super::super::UploadNowError>(),
            ],
            [16, 32, 16]
        );
    }
}
