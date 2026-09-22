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

//! A blob storage for tests that holds the calls that a rule selects.
//!
//! This module is test code, and it compiles only for tests.

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::time::error::Elapsed;
use tokio_util::sync::{CancellationToken, DropGuard};

/// A rule that selects the calls to hold, from the operation label and the path of a call.
type Rule = Box<dyn Fn(&str, &Path) -> bool + Send + Sync>;

/// A blob storage that passes each call to an in-memory storage, and holds each call that its rule
/// selects.
///
/// A held call waits until its gate opens, and then goes to the in-memory storage. Only the
/// [`Gate`] of the storage opens the gate, so a held call waits until the test drops that value.
pub(super) struct HoldingBlobStorage {
    inner: Arc<InMemoryBlobStorage>,
    rule: Rule,
    gate: CancellationToken,
    /// The storage does not send on this sender. Its receiver resolves when the storage is dropped.
    _dropped: oneshot::Sender<()>,
}

/// The gate of the held calls of a [`HoldingBlobStorage`].
///
/// The gate opens when the test drops this value, also when the test fails.
pub(super) struct Gate {
    _open_on_drop: DropGuard,
}

/// Gives a storage over `inner` that holds each call that `rule` selects, the gate of the held
/// calls, and a receiver that resolves when the last owner of the storage drops it.
///
/// So the receiver resolves only when no value holds a copy of the storage, for example a backend
/// on a thread of rustic.
pub(super) fn holding_storage(
    inner: Arc<InMemoryBlobStorage>,
    rule: impl Fn(&str, &Path) -> bool + Send + Sync + 'static,
) -> (Arc<HoldingBlobStorage>, Gate, oneshot::Receiver<()>) {
    let gate = CancellationToken::new();
    let (dropped, on_drop) = oneshot::channel();
    (
        Arc::new(HoldingBlobStorage {
            inner,
            rule: Box::new(rule),
            gate: gate.clone(),
            _dropped: dropped,
        }),
        Gate {
            _open_on_drop: gate.drop_guard(),
        },
        on_drop,
    )
}

/// Tells whether the error or an error in its chain of sources is tokio's `Elapsed`, which is the
/// root cause of the error of a call that got no answer within its deadline.
pub(super) fn reached_deadline(error: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(error), |error| error.source()).any(|error| error.is::<Elapsed>())
}

impl HoldingBlobStorage {
    /// Waits until the gate opens when the rule selects the call, and then gives the result of the
    /// call.
    async fn answer<T>(
        &self,
        op_label: &'static str,
        path: &Path,
        call: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        if (self.rule)(op_label, path) {
            self.gate.cancelled().await;
        }
        call.await
    }
}

impl Debug for HoldingBlobStorage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HoldingBlobStorage")
    }
}

#[async_trait]
impl BlobStorage for HoldingBlobStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.answer(
            op_label,
            path,
            self.inner.get_raw(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.answer(
            op_label,
            path,
            self.inner
                .get_stream(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.answer(
            op_label,
            path,
            self.inner
                .get_raw_slice(target_label, op_label, namespace, path, start, end),
        )
        .await
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.answer(
            op_label,
            path,
            self.inner
                .get_metadata(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.answer(
            op_label,
            path,
            self.inner
                .put_raw(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.answer(
            op_label,
            path,
            self.inner
                .put_raw_if_absent(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.answer(
            op_label,
            path,
            self.inner
                .put_stream(target_label, op_label, namespace, path, stream),
        )
        .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.answer(
            op_label,
            path,
            self.inner.delete(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.answer(
            op_label,
            path,
            self.inner
                .create_dir(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.answer(
            op_label,
            path,
            self.inner.list_dir(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.answer(
            op_label,
            path,
            self.inner
                .list_blobs_below(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.answer(
            op_label,
            path,
            self.inner
                .delete_dir(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<ExistsResult> {
        self.answer(
            op_label,
            path,
            self.inner.exists(target_label, op_label, namespace, path),
        )
        .await
    }
}
