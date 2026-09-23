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

//! A blob storage for tests that records each call and follows a script for each call.
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
use std::sync::{Arc, Mutex, PoisonError};

/// What the storage does with one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Script {
    /// Passes the call to the in-memory storage.
    Pass,
    /// Gives an error and does not pass the call.
    Refuse,
    /// Passes the call, and then gives an error in place of its answer.
    LoseTheAnswer,
    /// Passes the call, and then never answers.
    NeverAnswer,
}

/// A rule that gives the script of a call from its operation label and its path.
type Rule = Box<dyn Fn(&str, &Path) -> Script + Send + Sync>;

/// A blob storage that records the operation label and the path of each call, and does with each
/// call what its rule gives.
pub(super) struct ScriptedBlobStorage {
    inner: Arc<InMemoryBlobStorage>,
    rule: Rule,
    calls: Mutex<Vec<(&'static str, Box<Path>)>>,
}

impl ScriptedBlobStorage {
    pub(super) fn new(
        inner: Arc<InMemoryBlobStorage>,
        rule: impl Fn(&str, &Path) -> Script + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            rule: Box::new(rule),
            calls: Mutex::new(Vec::new()),
        })
    }

    /// Gives the operation label and the path of each call, in the order of the calls.
    pub(super) fn calls(&self) -> Vec<(&'static str, String)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(op_label, path)| (*op_label, path.display().to_string()))
            .collect()
    }

    async fn answer<T>(
        &self,
        op_label: &'static str,
        path: &Path,
        call: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((op_label, path.into()));
        match (self.rule)(op_label, path) {
            Script::Pass => call.await,
            Script::Refuse => Err(anyhow::anyhow!("the storage refused the call")),
            Script::LoseTheAnswer => {
                call.await?;
                Err(anyhow::anyhow!("the answer of the call was lost"))
            }
            Script::NeverAnswer => {
                call.await?;
                std::future::pending().await
            }
        }
    }
}

impl Debug for ScriptedBlobStorage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ScriptedBlobStorage")
    }
}

#[async_trait]
impl BlobStorage for ScriptedBlobStorage {
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
