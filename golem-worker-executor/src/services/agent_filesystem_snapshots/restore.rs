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

//! The restore that a start gives to the lifecycle.

use crate::filesystem_snapshot::{FilesystemSnapshotStore, SnapshotScope, SnapshotStoreError};
use crate::services::agent_filesystem::{RestoreError, RestoreTree};
use golem_common::model::oplog::FilesystemSnapshotName;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

/// The restore of one filesystem snapshot. It waits for a slot of the restores when the
/// lifecycle calls it, and gives the slot back when it ends, with success or with an error.
pub(crate) struct StoreRestore {
    store: Arc<dyn FilesystemSnapshotStore>,
    scope: SnapshotScope,
    name: FilesystemSnapshotName,
    restores: Arc<Semaphore>,
}

impl StoreRestore {
    pub(super) fn new(
        store: Arc<dyn FilesystemSnapshotStore>,
        scope: SnapshotScope,
        name: FilesystemSnapshotName,
        restores: Arc<Semaphore>,
    ) -> Self {
        Self {
            store,
            scope,
            name,
            restores,
        }
    }

    /// The name of the filesystem snapshot that this restore gives.
    pub(crate) fn name(&self) -> &FilesystemSnapshotName {
        &self.name
    }
}

impl RestoreTree for StoreRestore {
    async fn restore(self, into: &Path) -> Result<(), RestoreError> {
        let name = super::store_name(&self.name).map_err(|error| RestoreError {
            retryable: false,
            source: anyhow::Error::new(error),
        })?;
        let _slot = self
            .restores
            .acquire_owned()
            .await
            .map_err(|error| RestoreError {
                retryable: true,
                source: anyhow::Error::new(error).context("wait for a slot of the restores"),
            })?;
        let started = Instant::now();
        let result = self.store.restore(&self.scope, &name, into).await;
        crate::metrics::filesystem_snapshots::record_restore(
            if result.is_ok() { "restored" } else { "failed" },
            started.elapsed(),
        );
        result.map(drop).map_err(|error| RestoreError {
            retryable: matches!(
                error,
                SnapshotStoreError::Storage {
                    retryable: true,
                    ..
                }
            ),
            source: anyhow::Error::new(error)
                .context(format!("restore the filesystem snapshot {}", self.name)),
        })
    }
}
