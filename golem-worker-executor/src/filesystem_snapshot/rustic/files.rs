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

//! The blobs of the repository of one scope, and the one policy of each blob storage call of the
//! store and of its rustic backends.
//!
//! The tracker of the store counts each call. A call has up to [`IN_CALL_TRIES`] tries, which share
//! one window of the deadline; [`call_again`] decides each new try. A try does not start when the
//! lease of its prune ran out or its operation is cancelled. It ends at the expiry that the lease
//! had when the try started, when the operation is cancelled, or at the end of the window. A
//! refresh of the lease during a try does not move the end of that try.

use super::fault::{CallFailure, LeaseExpired, OperationCancelled, call_failure, may_land};
use crate::filesystem_snapshot::agent_work::OperationWork;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobStorage, BlobStorageNamespace, ListedBlob, PutIfAbsent,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;
use tracing::debug;

/// The target label of each blob storage call of the rustic store.
const TARGET_LABEL: &str = "filesystem_snapshot";

/// The tries of one blob call in a run.
pub(super) const IN_CALL_TRIES: u32 = 3;

/// The waits before the second and the third try: about 1 s in all.
pub(super) const IN_CALL_WAITS: [Duration; 2] =
    [Duration::from_millis(250), Duration::from_millis(750)];

/// What a blob call does after a try that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallAgain {
    /// Try again after `wait`, and cut the try at `cut`.
    After { wait: Duration, cut: Duration },
    /// Give the failure.
    End,
}

/// What a blob call does after its try number `tried` of `tries` failed with `failure`, when its
/// tries took `spent` in all and the storage call deadline is `deadline`. All tries of one call
/// share one window of `deadline`, so the call holds its run for at most `deadline` and the waits.
/// It tries again after the next wait while tries remain, the failure allows it, and the window
/// has time left; the next try is cut at the time left.
pub(super) fn call_again(
    tried: u32,
    tries: u32,
    failure: CallFailure,
    spent: Duration,
    deadline: Duration,
) -> CallAgain {
    let left = deadline.saturating_sub(spent);
    let wait = usize::try_from(tried.saturating_sub(1))
        .ok()
        .and_then(|index| IN_CALL_WAITS.get(index));
    match (failure, wait) {
        (CallFailure::Failed, Some(wait)) if tried < tries && !left.is_zero() => CallAgain::After {
            wait: *wait,
            cut: left,
        },
        _ => CallAgain::End,
    }
}

/// Gives the end of the latest try that can still land, after a try that ended at `ended`, when
/// `latest` was the latest one before.
pub(super) fn later(latest: Option<Instant>, ended: Instant) -> Instant {
    latest.map_or(ended, |latest| latest.max(ended))
}

/// Gives the instant after which a try that ended at `ended` has landed or never lands: one
/// deadline after its end.
pub(super) fn lands_by(ended: Option<Instant>, deadline: Duration) -> Option<Instant> {
    ended.map(|ended| ended + deadline)
}

/// The end of the latest try of a write of a run that was sent and ended without an answer. Such a
/// write can still land, within one deadline after the end of the try.
#[derive(Debug, Default)]
pub(super) struct LateWrites(Mutex<Option<Instant>>);

impl LateWrites {
    /// Records a try of a write that ended at `ended` without an answer.
    pub(super) fn record(&self, ended: Instant) {
        let mut latest = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *latest = Some(later(*latest, ended));
    }

    /// The end of the latest recorded try, when there is one.
    pub(super) fn latest(&self) -> Option<Instant> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Tells whether a call is a write, whose try that ended without an answer can still land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Read,
    Write,
}

/// What a try of a blob call starts with: the instant now, the time that the tries before it took,
/// and the storage call deadline of the blobs.
#[derive(Clone, Copy, Debug)]
pub(super) struct TryStart {
    pub(super) now: Instant,
    pub(super) spent: Duration,
    pub(super) deadline: Duration,
}

/// What a blob call does before a try. Only `Try` sends anything, so only a try that started can
/// be recorded as a write that can still land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BeforeTry {
    /// The lease of the prune ran out: the call gives `LeaseExpired`.
    LeaseOut,
    /// The operation of the blobs is cancelled: the call gives `OperationCancelled`.
    Cancelled,
    /// The stop of the caller is cancelled: a bounded call gives `Stopped`.
    Stop,
    /// The cut gives no time: a bounded call gives `NoTimeLeft`.
    NoTimeLeft,
    /// Start a try that is cut at the duration.
    Try(Duration),
}

/// Decides the start of a try, in this order: a lease that ran out, a cancelled operation, a stop
/// of the caller, no time left. A refusal sends nothing, so it records nothing.
pub(super) fn before_try(
    lease_out: bool,
    cancelled: bool,
    stopped: bool,
    cut: Option<Duration>,
) -> BeforeTry {
    match (lease_out, cancelled, stopped, cut) {
        (true, _, _, _) => BeforeTry::LeaseOut,
        (false, true, _, _) => BeforeTry::Cancelled,
        (false, false, true, _) => BeforeTry::Stop,
        (false, false, false, None) => BeforeTry::NoTimeLeft,
        (false, false, false, Some(cut)) => BeforeTry::Try(cut),
    }
}

/// The cut of a try of a call that has the window of one deadline for all its tries: what is left
/// of the window.
fn window_cut(start: TryStart) -> Option<Duration> {
    Some(start.deadline.saturating_sub(start.spent))
}

/// Why a call ended without its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unanswered {
    /// The last try, or the refusal before a try, failed with this class, and the call tries no
    /// more.
    Failed(CallFailure),
    /// The cut of the call gave no time for the next try.
    NoTimeLeft,
    /// The stop of the caller was cancelled before a try, or the wait between two tries ended.
    Stopped,
}

/// The failure of a call that ended without its answer.
#[derive(Debug)]
pub(super) struct NotAnswered {
    pub(super) error: anyhow::Error,
    pub(super) why: Unanswered,
}

/// The time until which a prune may make storage calls. A prune holds its claim until the time in
/// its newest marker plus the hold, as the other deletes see it. The lease ends before that, so a
/// prune stops before another delete can take its claim over.
#[derive(Debug)]
pub(super) struct Lease {
    expiry: Mutex<Instant>,
}

impl Lease {
    /// Gives a lease that ends at the instant.
    pub(super) fn until(expiry: Instant) -> Self {
        Self {
            expiry: Mutex::new(expiry),
        }
    }

    /// Moves the end of the lease as [`extended`] tells, for a marker write that started at
    /// `started` and succeeded.
    pub(super) fn extend_from(&self, started: Instant, span: Duration) {
        let mut current = self.expiry.lock().unwrap_or_else(PoisonError::into_inner);
        *current = extended(*current, started, span);
    }

    /// Gives the end of the lease.
    pub(super) fn expiry(&self) -> Instant {
        *self.expiry.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Gives the end of a lease that ends at `current` after a marker write that started at `started`
/// and succeeded: `span` after `started`, when that is later. A write that started at or after the
/// end of the lease does not move it, so a lease that ran out stays out. Another delete can have
/// taken the claim over before the marker of that write was visible. A write that started before
/// the end and ends late can still move the lease. That is safe, because no other delete can take
/// the claim over before that marker is visible.
fn extended(current: Instant, started: Instant, span: Duration) -> Instant {
    if started < current {
        current.max(started + span)
    } else {
        current
    }
}

/// The blobs of one scope: the storage, the namespace of the scope, and the policy of each call on
/// them. The policy is the deadline, the token of the operation, the lease of a prune, and the
/// tracker of the store.
#[derive(Clone, Debug)]
pub(super) struct SnapshotFiles {
    storage: Arc<dyn BlobStorage>,
    namespace: BlobStorageNamespace,
    deadline: Duration,
    cancel: CancellationToken,
    /// Ends the waits between two tries: a child of the root of the store, also in detached files.
    retry_stop: CancellationToken,
    /// The tries of each call.
    tries: u32,
    lease: Option<Arc<Lease>>,
    tracker: TaskTracker,
    /// Records the writes that ended without an answer, when a run asks for them.
    late: Option<Arc<LateWrites>>,
    /// The share of the operation in the work of its incarnation, which each clone of the blobs
    /// holds, so a drain of the incarnation waits for each holder of the blobs.
    _work: Option<OperationWork>,
    /// A token of the tracker of one run, which each clone of the blobs holds, so the run can wait
    /// until no holder of its blobs is left, and so until no blob call of it runs or can start.
    _run: Option<TaskTrackerToken>,
}

impl SnapshotFiles {
    /// Gives the blobs of the namespace, whose calls wait for at most `deadline`, stop when
    /// `cancel` is cancelled, and are counted by `tracker`. The calls have no lease.
    pub(super) fn new(
        storage: Arc<dyn BlobStorage>,
        namespace: BlobStorageNamespace,
        deadline: Duration,
        cancel: CancellationToken,
        tracker: TaskTracker,
    ) -> Self {
        Self {
            storage,
            namespace,
            deadline,
            cancel,
            retry_stop: CancellationToken::new(),
            tries: IN_CALL_TRIES,
            lease: None,
            tracker,
            late: None,
            _work: None,
            _run: None,
        }
    }

    /// Gives the same blobs, which hold a share of the work `work` of the incarnation of the
    /// operation.
    pub(super) fn of_work(&self, work: &OperationWork) -> Self {
        Self {
            _work: Some(work.clone()),
            ..self.clone()
        }
    }

    /// Gives the same blobs, which hold `run`, a token of the tracker of their run.
    pub(super) fn held_by(&self, run: TaskTrackerToken) -> Self {
        Self {
            _run: Some(run),
            ..self.clone()
        }
    }

    /// Gives the same blobs, whose waits between two tries end when `retry_stop` is cancelled.
    pub(super) fn with_retry_stop(&self, retry_stop: CancellationToken) -> Self {
        Self {
            retry_stop,
            ..self.clone()
        }
    }

    /// Gives the same blobs with one try for each call.
    pub(super) fn once(&self) -> Self {
        self.with_tries(1)
    }

    /// Gives the same blobs with `tries` tries for each call, at least one.
    pub(super) fn with_tries(&self, tries: u32) -> Self {
        Self {
            tries: tries.max(1),
            ..self.clone()
        }
    }

    /// Gives the same blobs, whose writes that end without an answer `late` records.
    pub(super) fn recording(&self, late: Arc<LateWrites>) -> Self {
        Self {
            late: Some(late),
            ..self.clone()
        }
    }

    /// Gives the same blobs, whose calls the lease also fences. A call does not start when the
    /// lease has run out. A call that runs ends at the expiry that the lease had when the call
    /// started.
    pub(super) fn leased(&self, lease: Arc<Lease>) -> Self {
        Self {
            lease: Some(lease),
            ..self.clone()
        }
    }

    /// Gives the same blobs with a token that nothing cancels and no lease. They are for the calls
    /// that run after a cancel or a drop by design. The tracker still counts each call, and the
    /// tries keep their number and their stop.
    pub(super) fn detached(&self) -> Self {
        Self {
            cancel: CancellationToken::new(),
            lease: None,
            ..self.clone()
        }
    }

    /// Gives the place of the blobs, for a message.
    pub(super) fn location(&self) -> String {
        format!("golem-blob-storage:{:?}", self.namespace)
    }

    /// Completes when the operation of the blobs is cancelled.
    pub(super) fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.cancel.cancelled()
    }

    /// Waits for one call, with its tries, each cut at what is left of the window of the call.
    /// The errors are those of [`Self::tries`].
    async fn answer<T, F>(&self, effect: Effect, call: impl Fn() -> F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>>,
    {
        self.tries(effect, window_cut, None, call)
            .await
            .map_err(|not_answered| not_answered.error)
    }

    /// Waits for one call, with its tries. The tracker counts the call before any check, so a shut
    /// down either stops the call or waits for it. Before each try, [`before_try`] decides from the
    /// lease, the cancel of the operation, `stop` and `cut`: a lease that ran out gives
    /// [`LeaseExpired`], also when the operation is cancelled, and a cancelled operation gives
    /// [`OperationCancelled`]. A refused try sends nothing and is not recorded. A cancel ends a
    /// running try and a wait between two tries. A try without an answer within its cut, or within
    /// the time that the lease leaves, fails. [`call_again`] decides each new try. A write whose
    /// try started and failed in a way that can still land is recorded.
    async fn tries<T, F>(
        &self,
        effect: Effect,
        cut: impl Fn(TryStart) -> Option<Duration>,
        stop: Option<&CancellationToken>,
        call: impl Fn() -> F,
    ) -> Result<T, NotAnswered>
    where
        F: Future<Output = anyhow::Result<T>>,
    {
        self.tracker
            .track_future(async {
                let tries = futures::stream::unfold(Some((1u32, Duration::ZERO)), |state| {
                    let (call, cut) = (&call, &cut);
                    async move {
                        let (tried, spent) = state?;
                        let started = super::runs::now();
                        let expiry = self.lease.as_ref().map(|lease| lease.expiry());
                        let decided = before_try(
                            expiry.is_some_and(|expiry| started >= expiry),
                            self.cancel.is_cancelled(),
                            stop.is_some_and(CancellationToken::is_cancelled),
                            cut(TryStart {
                                now: started,
                                spent,
                                deadline: self.deadline,
                            }),
                        );
                        let cut = match decided {
                            BeforeTry::Try(cut) => cut,
                            refused => return Some((Some(Err(refusal(refused))), None)),
                        };
                        let answer = self.one_try(cut, expiry, call()).await;
                        let ended = super::runs::now();
                        let spent = spent + ended.saturating_duration_since(started);
                        let Err(error) = answer else {
                            return Some((Some(answer.map_err(unreachable_error)), None));
                        };
                        let failure = call_failure(&error);
                        if effect == Effect::Write
                            && may_land(failure)
                            && let Some(late) = &self.late
                        {
                            late.record(ended);
                        }
                        match call_again(tried, self.tries, failure, spent, self.deadline) {
                            CallAgain::After { wait, .. } => {
                                debug!(
                                    tried,
                                    error = %format!("{error:#}"),
                                    "A filesystem snapshot blob call failed and tries again"
                                );
                                tokio::select! {
                                    biased;
                                    () = self.cancel.cancelled() => {
                                        Some((Some(Err(between_tries())), None))
                                    }
                                    () = self.retry_stop.cancelled() => {
                                        Some((Some(Err(between_tries())), None))
                                    }
                                    () = tokio::time::sleep(wait) => {
                                        Some((None, Some((tried + 1, spent))))
                                    }
                                }
                            }
                            CallAgain::End => Some((
                                Some(Err(NotAnswered {
                                    error,
                                    why: Unanswered::Failed(failure),
                                })),
                                None,
                            )),
                        }
                    }
                });
                futures::StreamExt::next(&mut std::pin::pin!(futures::StreamExt::filter_map(
                    tries,
                    |answer| async move { answer }
                )))
                .await
                .unwrap_or_else(|| Err(between_tries()))
            })
            .await
    }

    /// Runs one try, cut at `cut`, and within the lease that ends at `expiry` when the blobs have
    /// one.
    async fn one_try<T>(
        &self,
        cut: Duration,
        expiry: Option<Instant>,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let answer = answer_or_cancel(cut, &self.cancel, future);
        match expiry {
            None => answer.await,
            Some(expiry) => within_lease(expiry, answer).await,
        }
    }

    /// Writes `content` at `path` only when the path has no blob, with the tries of a blob call.
    /// Each try is cut at what `cut` gives for it, and no try starts when `cut` gives `None` or
    /// `stop` is cancelled. The tracker counts the call. A try that can still land is recorded
    /// when the blobs record late writes.
    pub(super) async fn put_if_absent_bounded(
        &self,
        op_label: &'static str,
        path: &Path,
        content: &[u8],
        cut: impl Fn(TryStart) -> Option<Duration>,
        stop: &CancellationToken,
    ) -> Result<PutIfAbsent, NotAnswered> {
        self.tries(Effect::Write, cut, Some(stop), || {
            self.storage.put_raw_if_absent(
                TARGET_LABEL,
                op_label,
                self.namespace.clone(),
                path,
                content,
            )
        })
        .await
    }

    /// Gives the size of the blob at the path, or `None` when the path has no blob.
    pub(super) async fn stat(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.answer(Effect::Read, || {
            self.storage
                .get_metadata(TARGET_LABEL, op_label, self.namespace.clone(), path)
        })
        .await
    }

    /// Gives the content of the blob at the path, or `None` when the path has no blob.
    pub(super) async fn get(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.answer(Effect::Read, || {
            self.storage
                .get_raw(TARGET_LABEL, op_label, self.namespace.clone(), path)
        })
        .await
    }

    /// Gives the bytes from `start` to `end` of the blob at the path, both inclusive, or `None`
    /// when the path has no blob.
    pub(super) async fn get_slice(
        &self,
        op_label: &'static str,
        path: &Path,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.answer(Effect::Read, || {
            self.storage.get_raw_slice(
                TARGET_LABEL,
                op_label,
                self.namespace.clone(),
                path,
                start,
                end,
            )
        })
        .await
    }

    /// Copies the blob at the path into the same path of the blobs `to`, on the side of the
    /// storage, so no byte comes to this process. Gives false when the path has no blob, and then
    /// writes nothing. The blobs `to` must be of the same storage.
    pub(super) async fn copy_to(
        &self,
        to: &SnapshotFiles,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<bool> {
        match self
            .answer(Effect::Write, || {
                self.storage.copy_between(
                    TARGET_LABEL,
                    op_label,
                    self.namespace.clone(),
                    path,
                    to.namespace.clone(),
                    path,
                )
            })
            .await
        {
            Ok(()) => Ok(true),
            Err(error) if error.downcast_ref::<BlobMissingError>().is_some() => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Writes the content as the blob at the path, over the blob that was there.
    pub(super) async fn put(
        &self,
        op_label: &'static str,
        path: &Path,
        content: &[u8],
    ) -> anyhow::Result<()> {
        self.answer(Effect::Write, || {
            self.storage.put_raw(
                TARGET_LABEL,
                op_label,
                self.namespace.clone(),
                path,
                content,
            )
        })
        .await
    }

    /// Writes the content as the blob at the path only when the path has no blob.
    pub(super) async fn put_if_absent(
        &self,
        op_label: &'static str,
        path: &Path,
        content: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.answer(Effect::Write, || {
            self.storage.put_raw_if_absent(
                TARGET_LABEL,
                op_label,
                self.namespace.clone(),
                path,
                content,
            )
        })
        .await
    }

    /// Deletes the blob at the path. A path without a blob gives success.
    pub(super) async fn delete(&self, op_label: &'static str, path: &Path) -> anyhow::Result<()> {
        self.answer(Effect::Write, || {
            self.storage
                .delete(TARGET_LABEL, op_label, self.namespace.clone(), path)
        })
        .await
    }

    /// Deletes the directory at the path and each blob below it.
    pub(super) async fn delete_dir(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.answer(Effect::Write, || {
            self.storage
                .delete_dir(TARGET_LABEL, op_label, self.namespace.clone(), path)
        })
        .await
    }

    /// Gives each blob directly below the path, and each directory that the storage keeps an
    /// entry for below the path.
    pub(super) async fn list_dir(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Box<[Box<Path>]>> {
        let listed = self
            .answer(Effect::Read, || {
                self.storage
                    .list_dir(TARGET_LABEL, op_label, self.namespace.clone(), path)
            })
            .await?;
        Ok(listed.into_iter().map(PathBuf::into_boxed_path).collect())
    }

    /// Gives each blob below the path, at all depths, with its size.
    pub(super) async fn list_below(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.answer(Effect::Read, || {
            self.storage
                .list_blobs_below(TARGET_LABEL, op_label, self.namespace.clone(), path)
        })
        .await
    }
}

/// The failure of a call that [`before_try`] refused, with the error that each class gives.
fn refusal(refused: BeforeTry) -> NotAnswered {
    match refused {
        BeforeTry::LeaseOut => NotAnswered {
            error: anyhow::Error::new(LeaseExpired),
            why: Unanswered::Failed(CallFailure::LeaseExpired),
        },
        BeforeTry::Cancelled => NotAnswered {
            error: anyhow::Error::new(OperationCancelled),
            why: Unanswered::Failed(CallFailure::Cancelled),
        },
        BeforeTry::Stop => NotAnswered {
            error: anyhow::anyhow!("the caller stopped the blob call before a try"),
            why: Unanswered::Stopped,
        },
        BeforeTry::NoTimeLeft | BeforeTry::Try(_) => NotAnswered {
            error: anyhow::anyhow!("the blob call has no time left for a try"),
            why: Unanswered::NoTimeLeft,
        },
    }
}

/// The failure of a call whose wait between two tries a cancel or a stop of the tries ended.
fn between_tries() -> NotAnswered {
    NotAnswered {
        error: anyhow::Error::new(OperationCancelled),
        why: Unanswered::Stopped,
    }
}

/// Never runs: an answer that is not an error has no failure to map.
fn unreachable_error(error: anyhow::Error) -> NotAnswered {
    NotAnswered {
        why: Unanswered::Failed(call_failure(&error)),
        error,
    }
}

/// Gives the output of the future, or [`LeaseExpired`] when the lease runs out first, at `expiry`,
/// the expiry of the lease that the call read before the try.
async fn within_lease<T>(
    expiry: Instant,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout_at(tokio::time::Instant::from_std(expiry), future)
        .await
        .unwrap_or_else(|_| Err(anyhow::Error::new(LeaseExpired)))
}

/// Gives the output of the future, or an error when the future gives no output within the deadline.
///
/// The timer starts at the first poll of the returned future, in the runtime of that poll. Thus a
/// thread without a runtime context can wait for the result with `Handle::block_on`. A future that
/// is ready at its first poll always gives its output. At the deadline, the function drops the
/// future and gives an error whose root cause is tokio's `Elapsed`.
async fn answer_within<T>(
    deadline: Duration,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(deadline, future)
        .await
        .unwrap_or_else(|elapsed| {
            Err(anyhow::Error::new(elapsed).context(format!(
                "the blob storage gave no answer within {deadline:?}"
            )))
        })
}

/// Gives the output of the future within the deadline, or an error when the operation of the token
/// is cancelled. The future is polled first, so a cancel that comes after [`before_try`] still lets
/// the try start; a cancel ends a call that runs.
async fn answer_or_cancel<T>(
    deadline: Duration,
    cancel: &CancellationToken,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::select! {
        biased;
        answer = answer_within(deadline, future) => answer,
        () = cancel.cancelled() => Err(anyhow::Error::new(OperationCancelled)),
    }
}

#[cfg(test)]
mod tests;
