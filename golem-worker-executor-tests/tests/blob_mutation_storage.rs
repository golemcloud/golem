use async_trait::async_trait;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobRangeStream, BlobStorageBackend, BlobStorageNamespace, ExistsResult,
    ListedBlob, NormalizedBlobPath, PutIfAbsent,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Notify, Semaphore};

#[derive(Debug)]
pub(super) struct MutationStorage {
    inner: InMemoryBlobStorage,
    pub armed: AtomicBool,
    pub pending: Notify,
    pub release: Semaphore,
    pub completed: AtomicUsize,
    pub entered: AtomicUsize,
    pub effects: AtomicUsize,
    pub dropped: AtomicUsize,
}

impl Default for MutationStorage {
    fn default() -> Self {
        Self {
            inner: InMemoryBlobStorage::new(),
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

struct MutationWait<'a>(&'a MutationStorage, bool);

impl Drop for MutationWait<'_> {
    fn drop(&mut self) {
        if !self.1 {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl MutationStorage {
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

#[async_trait]
impl BlobStorageBackend for MutationStorage {
    async fn get_raw_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner.get_raw_at(target, op, ns, path).await
    }

    async fn get_range_stream_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<BlobRangeStream>> {
        self.inner
            .get_range_stream_at(target, op, ns, path, offset, length)
            .await
    }

    async fn get_metadata_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner.get_metadata_at(target, op, ns, path).await
    }

    async fn put_raw_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let result = self
            .mutate(self.inner.put_raw_at(target, op, ns, path, data))
            .await;
        if result.is_ok() {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    async fn put_raw_if_absent_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent_at(target, op, ns, path, data)
            .await
    }

    async fn delete_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner.delete_at(target, op, ns, path).await
    }

    async fn create_dir_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner.create_dir_at(target, op, ns, path).await
    }

    async fn list_dir_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner.list_dir_at(target, op, ns, path).await
    }

    async fn list_blobs_below_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner.list_blobs_below_at(target, op, ns, path).await
    }

    async fn delete_dir_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<u64>> {
        self.inner.delete_dir_at(target, op, ns, path).await
    }

    async fn exists_at(
        &self,
        target: &'static str,
        op: &'static str,
        ns: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.inner.exists_at(target, op, ns, path).await
    }

    async fn copy_between_at(
        &self,
        target: &'static str,
        op: &'static str,
        from_ns: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_ns: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        let result = self
            .mutate(
                self.inner
                    .copy_between_at(target, op, from_ns, from, to_ns, to),
            )
            .await;
        if matches!(result, Ok(true)) {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        result
    }
}
