use async_trait::async_trait;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorageNamespace, NormalizedBlobPath};
use golem_worker_executor_test_utils::{
    BlobStorageDecorator, BlobStorageInner, DecoratedBlobStorage,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Notify, Semaphore};

/// An in-memory blob storage whose mutations a test can hold; see [`MutationGate`].
pub(super) type MutationStorage = DecoratedBlobStorage<MutationGate, InMemoryBlobStorage>;

#[derive(Debug)]
pub(super) struct MutationGate {
    pub armed: AtomicBool,
    pub pending: Notify,
    pub release: Semaphore,
    pub completed: AtomicUsize,
    pub entered: AtomicUsize,
    pub effects: AtomicUsize,
    pub dropped: AtomicUsize,
}

impl Default for MutationGate {
    fn default() -> Self {
        Self {
            armed: AtomicBool::new(false),
            pending: Notify::new(),
            release: Semaphore::new(0),
            completed: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        }
    }
}

struct MutationWait<'a>(&'a MutationGate, bool);

impl Drop for MutationWait<'_> {
    fn drop(&mut self) {
        if !self.1 {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl MutationGate {
    async fn mutate<T>(&self, work: impl std::future::Future<Output = T>) -> T {
        self.entered.fetch_add(1, Ordering::SeqCst);
        if !self.armed.swap(false, Ordering::SeqCst) {
            return work.await;
        }
        let mut wait = MutationWait(self, false);
        let mut acquire = std::pin::pin!(self.release.acquire());
        futures::future::poll_fn(|cx| {
            let result = acquire.as_mut().poll(cx);
            if result.is_pending() {
                self.pending.notify_one();
            }
            result
        })
        .await
        .unwrap()
        .forget();
        let result = work.await;
        wait.1 = true;
        self.completed.fetch_add(1, Ordering::SeqCst);
        result
    }
}

/// Gates the blob mutations of the in-memory storage it decorates: once armed, the next
/// mutation waits for `release`, and the counters record what entered, completed, took effect or
/// was dropped.
#[async_trait]
impl BlobStorageDecorator<InMemoryBlobStorage> for MutationGate {
    async fn put_raw_at(
        &self,
        inner: &InMemoryBlobStorage,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let result = self
            .mutate(BlobStorageInner::put_raw_at(
                inner, target, op, ns, path, data,
            ))
            .await;
        if result.is_ok() {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    async fn copy_between_at(
        &self,
        inner: &InMemoryBlobStorage,
        target: &'static str,
        op: &'static str,
        from_ns: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_ns: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        let result = self
            .mutate(BlobStorageInner::copy_between_at(
                inner, target, op, from_ns, from, to_ns, to,
            ))
            .await;
        if matches!(result, Ok(true)) {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        result
    }
}
