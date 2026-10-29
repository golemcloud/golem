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

use super::sandbox_cleanup::{
    LeaseState, NativeCleanup, acquire_filesystem_lease, remove_and_verify,
    rollback_created_filesystem, rollback_creation, verify_fresh_directory,
};
use super::sandbox_provisioning::SandboxFilesystemName;
use super::{NativeNameModeSource, NativeRoot, QuotaAuthority, SandboxFilesystem};
use crate::sandbox_filesystem::FilesystemStorageError;
use crate::storage_volume::FilesystemVolume;
use golem_common::model::RetryConfig;
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone)]
pub(in crate::sandbox_filesystem) struct DirectoryProvisioning {
    pub(in crate::sandbox_filesystem) deterministic_root: Option<Arc<Path>>,
    pub(in crate::sandbox_filesystem) cleanup_retry: RetryConfig,
    pub(in crate::sandbox_filesystem) name_mode: NativeNameModeSource,
}

impl DirectoryProvisioning {
    /// Keeps the storage settings. Without a deterministic root, each sandbox gets its own
    /// temporary directory. `name_mode` tells how the sandboxes compare names.
    pub(in crate::sandbox_filesystem) fn new(
        deterministic_root: Option<Arc<Path>>,
        cleanup_retry: RetryConfig,
        name_mode: NativeNameModeSource,
    ) -> Self {
        Self {
            deterministic_root,
            cleanup_retry,
            name_mode,
        }
    }

    pub(in crate::sandbox_filesystem) async fn create_fresh(
        &self,
        volume: FilesystemVolume,
        name: SandboxFilesystemName,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        match &self.deterministic_root {
            Some(storage_root) => {
                let root = storage_root.join(name.relative_path());
                let lifecycle = acquire_filesystem_lease(&root).await;
                let provisioning = self.clone();
                let error_path = root.clone();
                spawn_creation("provision sandbox filesystem", &error_path, async move {
                    provisioning
                        .create_deterministic(volume, root, lifecycle)
                        .await
                })
                .await
            }
            None => self.create_temporary(volume).await,
        }
    }

    async fn create_temporary(
        &self,
        volume: FilesystemVolume,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        let directory = tempfile::Builder::new()
            .prefix("golem")
            .tempdir()
            .map_err(|error| {
                FilesystemStorageError::io("create temporary directory", Path::new("<temp>"), error)
            })?;
        let lifecycle = Arc::new(AsyncMutex::new(()))
            .try_lock_owned()
            .expect("new temporary sandbox filesystem lease must be available");
        let root = directory.keep();
        // The task owns the kept directory through construction and caller cancellation.
        let provisioning = self.clone();
        let error_path = root.clone().into_boxed_path();
        spawn_creation(
            "provision temporary sandbox filesystem",
            &error_path,
            async move { provisioning.finish_creation(volume, root, lifecycle).await },
        )
        .await
    }

    async fn create_deterministic(
        &self,
        volume: FilesystemVolume,
        root: PathBuf,
        lifecycle: OwnedMutexGuard<()>,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        remove_and_verify(&root, "remove stale runtime directory", &self.cleanup_retry).await?;
        let parent = root
            .parent()
            .expect("deterministic sandbox filesystem path must have a parent");
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return Err(rollback_creation(
                &root,
                FilesystemStorageError::io("create runtime directory parent", parent, error),
                &self.cleanup_retry,
            )
            .await);
        }
        if let Err(error) = tokio::fs::create_dir(&root).await {
            return Err(rollback_creation(
                &root,
                FilesystemStorageError::io("create fresh runtime directory", &root, error),
                &self.cleanup_retry,
            )
            .await);
        }
        self.finish_creation(volume, root, lifecycle).await
    }

    async fn finish_creation(
        &self,
        volume: FilesystemVolume,
        root: PathBuf,
        lifecycle: OwnedMutexGuard<()>,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        let directory = match File::open(&root) {
            Ok(directory) => directory,
            Err(error) => {
                return Err(rollback_creation(
                    &root,
                    FilesystemStorageError::io("open fresh runtime directory", &root, error),
                    &self.cleanup_retry,
                )
                .await);
            }
        };
        let native_root = match NativeRoot::initialize(root.clone(), directory).await {
            Ok(native_root) => native_root,
            Err(error) => {
                return Err(rollback_creation(&root, error, &self.cleanup_retry).await);
            }
        };
        let filesystem = SandboxFilesystem::new(
            native_root,
            LeaseState {
                lifecycle,
                cleanup: NativeCleanup::Directory {
                    path: root.clone(),
                    cleanup_retry: self.cleanup_retry.clone(),
                },
            },
            volume,
            QuotaAuthority::Unsupported,
            self.name_mode,
        );
        if let Err(error) = verify_fresh_directory(filesystem.root()).await {
            return Err(rollback_created_filesystem(filesystem, error).await);
        }
        Ok(filesystem)
    }
}

/// Runs the creation `future` in its own task, so that the task, not the caller, finishes it. A
/// task that does not finish gives an error of `operation` at `error_path`.
pub(in crate::sandbox_filesystem) async fn spawn_creation(
    operation: &'static str,
    error_path: &Path,
    future: impl Future<Output = Result<SandboxFilesystem, FilesystemStorageError>> + Send + 'static,
) -> Result<SandboxFilesystem, FilesystemStorageError> {
    tokio::spawn(future).await.map_err(|error| {
        FilesystemStorageError::io(operation, error_path, std::io::Error::other(error))
    })?
}
