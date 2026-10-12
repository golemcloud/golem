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

use super::*;

#[derive(Clone)]
pub(super) struct DirectoryProvisioning {
    deterministic_root: Option<Arc<Path>>,
    cleanup_retry: RetryConfig,
    name_mode: NativeNameModeSource,
}

impl DirectoryProvisioning {
    /// Keeps the storage settings. Without a deterministic root, each sandbox gets its own
    /// temporary directory. `name_mode` tells how the sandboxes compare names.
    pub(super) fn new(
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

    /// The configured root of the sandboxes and the host directories, if one is configured.
    pub(super) fn deterministic_root(&self) -> Option<&Arc<Path>> {
        self.deterministic_root.as_ref()
    }

    pub(super) async fn create_fresh(
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
                tokio::spawn(async move {
                    provisioning
                        .create_deterministic(volume, root, lifecycle)
                        .await
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "provision sandbox filesystem",
                        &error_path,
                        std::io::Error::other(error),
                    )
                })?
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
        #[cfg(target_os = "macos")]
        {
            // The task owns the kept directory through construction and caller cancellation.
            let provisioning = self.clone();
            let error_path = root.clone().into_boxed_path();
            tokio::spawn(async move { provisioning.finish_creation(volume, root, lifecycle).await })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "provision temporary sandbox filesystem",
                        &error_path,
                        std::io::Error::other(error),
                    )
                })?
        }
        #[cfg(not(target_os = "macos"))]
        self.finish_creation(volume, root, lifecycle).await
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
        #[cfg(target_os = "macos")]
        let native_root = {
            let native_path = root.clone();
            execute_native(
                storage_profile(&volume),
                NativeOperation::Flush,
                move || NativeRoot::new(native_path, directory),
            )
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "initialize fresh runtime directory",
                    &root,
                    error,
                )
            })
            .and_then(|result| {
                result.map_err(|error| {
                    FilesystemStorageError::io("initialize fresh runtime directory", &root, error)
                })
            })
        };
        #[cfg(not(target_os = "macos"))]
        let native_root = NativeRoot::new(root.clone(), directory).map_err(|error| {
            FilesystemStorageError::io("initialize fresh runtime directory", &root, error)
        });
        let native_root = match native_root {
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
