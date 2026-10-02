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
//! The tracker of the store counts each call. A call does not start when the lease of its prune ran
//! out or its operation is cancelled. It ends at the expiry that the lease had when the call
//! started, when the operation is cancelled, or at the deadline. A refresh of the lease during a
//! call does not move the end of that call.

use super::fault::{LeaseExpired, OperationCancelled};
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobStorage, BlobStorageNamespace, ListedBlob, PutIfAbsent,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tokio_util::task::TaskTracker;

/// The target label of each blob storage call of the rustic store.
const TARGET_LABEL: &str = "filesystem_snapshot";

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
    lease: Option<Arc<Lease>>,
    tracker: TaskTracker,
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
            lease: None,
            tracker,
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
    /// that run after a cancel or a drop by design. The tracker still counts each call.
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

    /// Waits for one call. The tracker counts the call before any check, so a shut down either
    /// stops the call or waits for it. A call of a lease that ran out gives [`LeaseExpired`], also
    /// when the operation is cancelled. A call of a cancelled operation does not start, and a
    /// cancel ends a running call. A call without an answer within the deadline, or within the time
    /// that the lease leaves, fails. Each of these gives an error, the same as a call that failed.
    async fn answer<T>(
        &self,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        self.tracker
            .track_future(async {
                let answer = answer_or_cancel(self.deadline, &self.cancel, future);
                match &self.lease {
                    None => answer.await,
                    Some(lease) => within_lease(lease, answer).await,
                }
            })
            .await
    }

    /// Gives the size of the blob at the path, or `None` when the path has no blob.
    pub(super) async fn stat(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.answer(
            self.storage
                .get_metadata(TARGET_LABEL, op_label, self.namespace.clone(), path),
        )
        .await
    }

    /// Gives the content of the blob at the path, or `None` when the path has no blob.
    pub(super) async fn get(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.answer(
            self.storage
                .get_raw(TARGET_LABEL, op_label, self.namespace.clone(), path),
        )
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
        self.answer(self.storage.get_raw_slice(
            TARGET_LABEL,
            op_label,
            self.namespace.clone(),
            path,
            start,
            end,
        ))
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
            .answer(self.storage.copy_between(
                TARGET_LABEL,
                op_label,
                self.namespace.clone(),
                path,
                to.namespace.clone(),
                path,
            ))
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
        self.answer(self.storage.put_raw(
            TARGET_LABEL,
            op_label,
            self.namespace.clone(),
            path,
            content,
        ))
        .await
    }

    /// Writes the content as the blob at the path only when the path has no blob.
    pub(super) async fn put_if_absent(
        &self,
        op_label: &'static str,
        path: &Path,
        content: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.answer(self.storage.put_raw_if_absent(
            TARGET_LABEL,
            op_label,
            self.namespace.clone(),
            path,
            content,
        ))
        .await
    }

    /// Deletes the blob at the path. A path without a blob gives success.
    pub(super) async fn delete(&self, op_label: &'static str, path: &Path) -> anyhow::Result<()> {
        self.answer(
            self.storage
                .delete(TARGET_LABEL, op_label, self.namespace.clone(), path),
        )
        .await
    }

    /// Deletes the directory at the path and each blob below it.
    pub(super) async fn delete_dir(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.answer(
            self.storage
                .delete_dir(TARGET_LABEL, op_label, self.namespace.clone(), path),
        )
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
            .answer(
                self.storage
                    .list_dir(TARGET_LABEL, op_label, self.namespace.clone(), path),
            )
            .await?;
        Ok(listed.into_iter().map(PathBuf::into_boxed_path).collect())
    }

    /// Gives each blob below the path, at all depths, with its size.
    pub(super) async fn list_below(
        &self,
        op_label: &'static str,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.answer(self.storage.list_blobs_below(
            TARGET_LABEL,
            op_label,
            self.namespace.clone(),
            path,
        ))
        .await
    }
}

/// Gives the output of the future, or [`LeaseExpired`] when the lease runs out first. The bound is
/// the expiry of the lease that the call reads once. A call does not start when the lease ran out
/// at that read.
async fn within_lease<T>(
    lease: &Lease,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let expiry = lease.expiry();
    if Instant::now() >= expiry {
        return Err(anyhow::Error::new(LeaseExpired));
    }
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
/// is cancelled. A call of a cancelled operation does not start, and a cancel ends a call that
/// runs.
async fn answer_or_cancel<T>(
    deadline: Duration,
    cancel: &CancellationToken,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    if cancel.is_cancelled() {
        return Err(anyhow::Error::new(OperationCancelled));
    }
    tokio::select! {
        biased;
        answer = answer_within(deadline, future) => answer,
        () = cancel.cancelled() => Err(anyhow::Error::new(OperationCancelled)),
    }
}

#[cfg(test)]
mod tests;
