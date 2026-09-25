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

//! The blobs of the repository of one scope, and the blob storage calls of the store on them.
//!
//! Each call waits for at most the deadline of the scope, and a cancel of its operation ends it.

use super::backend::answer_within;
use super::fault::OperationCancelled;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageNamespace, ListedBlob, PutIfAbsent,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The target label of each blob storage call of the rustic store.
pub(super) const TARGET_LABEL: &str = "filesystem_snapshot";

/// The blobs of one scope: the storage, the namespace of the scope, the deadline of each call, and
/// the token of the operation.
#[derive(Clone, Debug)]
pub(super) struct SnapshotFiles {
    pub(super) storage: Arc<dyn BlobStorage>,
    pub(super) namespace: BlobStorageNamespace,
    pub(super) deadline: Duration,
    pub(super) cancel: CancellationToken,
}

impl SnapshotFiles {
    /// Waits for one call within the deadline. A call of a cancelled operation does not start, and
    /// a cancel ends a running call. Both give an error.
    async fn answer<T>(
        &self,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        if self.cancel.is_cancelled() {
            return Err(anyhow::Error::new(OperationCancelled));
        }
        tokio::select! {
            biased;
            answer = answer_within(self.deadline, future) => answer,
            () = self.cancel.cancelled() => Err(anyhow::Error::new(OperationCancelled)),
        }
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
