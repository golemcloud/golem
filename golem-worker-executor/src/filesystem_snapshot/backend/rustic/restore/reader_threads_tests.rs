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

use crate::filesystem_snapshot::backend::rustic::repository::open_existing;
use crate::filesystem_snapshot::backend::rustic::snapshot_catalog::named;
use crate::filesystem_snapshot::backend::rustic::test_support::{
    backend_of, name, repository_key, store_with_deadline,
};
use crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking;
use crate::filesystem_snapshot::test_support::fixture::{Scratch, listing};
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{
    AgentSnapshots, FilesystemSnapshotStore, SnapshotName, Unlimited,
};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use anyhow::Context;
use async_trait::async_trait;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob,
    NormalizedBlobPath, PutIfAbsent,
};
use pretty_assertions::assert_eq;
use rustic_core::RestoreOptions;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::sync::watch;

#[test]
#[timeout("60s")]
async fn a_restore_reads_data_on_at_most_its_reader_threads() {
    // Each save adds one pack with the data of its new file. The restore of the second snapshot
    // reads the data of both packs, one read for each pack.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let store_with_deadline = store_with_deadline(storage.clone(), STORAGE_CALL_DEADLINE);
    let tree = Scratch::new();
    std::fs::write(tree.path().join("first.txt"), b"first").unwrap();
    store_with_deadline
        .save(
            &scope,
            &name("p-first"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    std::fs::write(tree.path().join("second.txt"), b"second").unwrap();
    store_with_deadline
        .save(
            &scope,
            &name("p-second"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let restore = async |reader_threads| {
        let counting = Arc::new(OverlapCountingStorage::new(storage.clone()));
        let into = Scratch::new();
        restore_named(
            counting.clone(),
            &scope,
            &name("p-second"),
            into.path(),
            RestoreOptions::default().reader_threads(reader_threads),
        )
        .await
        .unwrap();
        (counting.most(), listing(into.path()))
    };
    let one_thread = restore(NonZeroUsize::new(1)).await;
    let default_threads = restore(None).await;
    assert_eq!(
        (one_thread, default_threads),
        ((1, listing(tree.path())), (2, listing(tree.path())))
    );
}

/// Finds the snapshot with the name in the scope with the lookup rule of the store. Then it
/// restores that snapshot into the empty directory `into` with the options, on a blocking thread.
async fn restore_named(
    storage: Arc<dyn BlobStorage>,
    scope: &AgentSnapshots,
    name: &SnapshotName,
    into: &Path,
    options: RestoreOptions,
) -> anyhow::Result<()> {
    let backend = backend_of(storage, scope, STORAGE_CALL_DEADLINE);
    let (name, into) = (name.clone(), Box::<Path>::from(into));
    run_blocking(move || {
        let repository =
            open_existing(backend, &repository_key())?.context("the scope has no repository")?;
        let snapshots = repository.get_all_snapshots()?;
        let snapshot = named(&snapshots, &name).context("no snapshot has the name")?;
        restore_snapshot(repository, snapshot, &into, &options)
    })
    .await
}

/// The longest time that a ranged read of a reader thread waits for a second one.
const OVERLAP_WAIT: Duration = Duration::from_millis(300);

/// Counts concurrent ranged reads on rayon reader threads. A read waits for a second read or
/// `OVERLAP_WAIT` before proceeding, so reads that can overlap are observed together.
#[derive(Debug)]
struct OverlapCountingStorage {
    inner: Arc<InMemoryBlobStorage>,
    in_progress: watch::Sender<usize>,
    most: AtomicUsize,
}

impl OverlapCountingStorage {
    fn new(inner: Arc<InMemoryBlobStorage>) -> Self {
        Self {
            inner,
            in_progress: watch::Sender::new(0),
            most: AtomicUsize::new(0),
        }
    }

    fn most(&self) -> usize {
        self.most.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BlobStorageBackend for OverlapCountingStorage {
    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        self.inner
            .copy_between_at(
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            )
            .await
    }

    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw_at(target_label, op_label, namespace, path)
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
        self.inner
            .get_range_stream_at(target_label, op_label, namespace, path, offset, length)
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
        if rayon::current_thread_index().is_none() {
            return self
                .inner
                .get_raw_slice_at(target_label, op_label, namespace, path, start, end)
                .await;
        }
        let mut changes = self.in_progress.subscribe();
        self.in_progress.send_modify(|count| {
            *count += 1;
            self.most.fetch_max(*count, Ordering::SeqCst);
        });
        // No second read before the timeout is not an error.
        let _ = tokio::time::timeout(OVERLAP_WAIT, changes.wait_for(|count| *count >= 2)).await;
        let result = self
            .inner
            .get_raw_slice_at(target_label, op_label, namespace, path, start, end)
            .await;
        self.in_progress.send_modify(|count| *count -= 1);
        result
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner
            .get_metadata_at(target_label, op_label, namespace, path)
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
        self.inner
            .put_raw_at(target_label, op_label, namespace, path, data)
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
        self.inner
            .put_raw_if_absent_at(target_label, op_label, namespace, path, data)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .delete_at(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below_at(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<u64>> {
        self.inner
            .delete_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists_at(target_label, op_label, namespace, path)
            .await
    }
}

/// Writes the tree of the snapshot into the empty directory `into`.
fn restore_snapshot(
    repository: rustic_core::Repository<rustic_core::OpenStatus>,
    snapshot: &rustic_core::repofile::SnapshotFile,
    into: &std::path::Path,
    options: &rustic_core::RestoreOptions,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let repository = repository.to_indexed()?;
    let into = into
        .to_str()
        .context("the directory of a restore must have a UTF-8 path")?;
    let destination = rustic_core::LocalDestination::new(into, false, false)?;
    let node = repository.node_from_snapshot_and_path(snapshot, "")?;
    let entries = repository.ls(&node, &rustic_core::LsOptions::default())?;
    let plan = repository.prepare_restore(options, entries.clone(), &destination, false)?;
    repository.restore(plan, options, entries, &destination)?;
    Ok(())
}
