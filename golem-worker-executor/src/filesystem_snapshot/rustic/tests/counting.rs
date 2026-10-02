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

//! A blob storage for tests that counts the calls by operation and by top directory, and that
//! can make each read of a snapshot file take a time.
//!
//! This module is test code, and it compiles only for tests.

use async_trait::async_trait;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob,
    NormalizedBlobPath, PutIfAbsent,
};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// A blob storage that passes each call to an in-memory storage, and counts it under its
/// operation label and the first segment of its path. Each read of a snapshot file waits for
/// `snapshot_read_delay` first, and the storage records how many of those reads ran at once.
pub(crate) struct CountingBlobStorage {
    inner: Arc<InMemoryBlobStorage>,
    snapshot_read_delay: Duration,
    counts: Mutex<BTreeMap<(&'static str, Box<str>), u64>>,
    snapshot_reads_now: AtomicUsize,
    most_snapshot_reads_at_once: AtomicUsize,
}

impl CountingBlobStorage {
    /// A storage over `inner` whose reads of snapshot files each take `snapshot_read_delay`.
    pub(crate) fn new(inner: Arc<InMemoryBlobStorage>, snapshot_read_delay: Duration) -> Self {
        Self {
            inner,
            snapshot_read_delay,
            counts: Mutex::default(),
            snapshot_reads_now: AtomicUsize::new(0),
            most_snapshot_reads_at_once: AtomicUsize::new(0),
        }
    }

    /// The number of calls with `op_label` below the top directory `directory` since the last
    /// reset.
    pub(crate) fn count(&self, op_label: &str, directory: &str) -> u64 {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|((op, dir), _)| *op == op_label && &**dir == directory)
            .map(|(_, count)| *count)
            .sum()
    }

    /// The number of calls with `op_label` since the last reset, below any directory.
    pub(crate) fn count_of(&self, op_label: &str) -> u64 {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|((op, _), _)| *op == op_label)
            .map(|(_, count)| *count)
            .sum()
    }

    /// The largest number of reads of snapshot files that ran at once since the last reset.
    pub(crate) fn most_snapshot_reads_at_once(&self) -> usize {
        self.most_snapshot_reads_at_once.load(Ordering::SeqCst)
    }

    /// Clears the counts.
    pub(crate) fn reset(&self) {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.most_snapshot_reads_at_once.store(0, Ordering::SeqCst);
    }

    /// Counts the call, and waits for the delay first when it reads a snapshot file.
    async fn counted<T>(
        &self,
        op_label: &'static str,
        path: &Path,
        call: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let directory = path
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .unwrap_or("")
            .to_string()
            .into_boxed_str();
        let snapshot_read = op_label == "read" && &*directory == "snapshots";
        *self
            .counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((op_label, directory))
            .or_default() += 1;
        if !snapshot_read {
            return call.await;
        }
        let now = self.snapshot_reads_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_snapshot_reads_at_once
            .fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.snapshot_read_delay).await;
        let result = call.await;
        self.snapshot_reads_now.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

impl Debug for CountingBlobStorage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CountingBlobStorage")
    }
}

#[async_trait]
impl BlobStorageBackend for CountingBlobStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.counted(
            op_label,
            path,
            self.inner
                .get_raw_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<golem_service_base::storage::blob::BlobRangeStream>> {
        self.counted(
            op_label,
            path,
            self.inner
                .get_range_stream_at(target_label, op_label, namespace, path, offset, length),
        )
        .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.counted(
            op_label,
            path,
            self.inner
                .get_raw_slice_at(target_label, op_label, namespace, path, start, end),
        )
        .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.counted(
            op_label,
            path,
            self.inner
                .get_metadata_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.counted(
            op_label,
            path,
            self.inner
                .put_raw_at(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.counted(
            op_label,
            path,
            self.inner
                .put_raw_if_absent_at(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.counted(
            op_label,
            path,
            self.inner
                .delete_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.counted(
            op_label,
            path,
            self.inner
                .create_dir_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.counted(
            op_label,
            path,
            self.inner
                .list_dir_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.counted(
            op_label,
            path,
            self.inner
                .list_blobs_below_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        self.counted(
            op_label,
            path,
            self.inner
                .delete_dir_at(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.counted(
            op_label,
            path,
            self.inner
                .exists_at(target_label, op_label, namespace, path),
        )
        .await
    }
}
