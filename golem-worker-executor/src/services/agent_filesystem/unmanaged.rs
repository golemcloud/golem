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

use super::backend::{
    AgentFilesystemBackend, AgentFilesystemCleanup, FilesystemBackendProvisioner,
    InitialFileMaterialization, ProvisionedAgentFilesystem, agent_filesystem_owner_path,
};
use super::quota::observe_path_capacity;
use super::{
    FilesystemCapacity, FilesystemStorageError, OwnedAgentId, OwnedMutexGuard, Path, PathBuf,
    RetryConfig, acquire_lifecycle_lock, create_materialization_parent, remove_and_verify,
    remove_and_verify_blocking, rollback_creation, set_initial_file_permissions,
    verify_fresh_directory,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub(super) struct UnmanagedBackend {
    deterministic_root: Option<PathBuf>,
    cleanup_retry: RetryConfig,
}

impl UnmanagedBackend {
    pub fn new(deterministic_root: Option<PathBuf>, cleanup_retry: RetryConfig) -> Self {
        Self {
            deterministic_root,
            cleanup_retry,
        }
    }

    async fn create_temporary(&self) -> Result<ProvisionedAgentFilesystem, FilesystemStorageError> {
        let directory = tempfile::Builder::new()
            .prefix("golem")
            .tempdir()
            .map_err(|error| {
                FilesystemStorageError::io("create temporary directory", Path::new("<temp>"), error)
            })?;
        let lifecycle = Arc::new(Mutex::new(()))
            .try_lock_owned()
            .expect("new temporary filesystem lifecycle lock must be available");
        let root = directory.keep();
        let backend: Arc<dyn AgentFilesystemBackend> =
            Arc::new(UnmanagedAgentFilesystem { root: root.clone() });
        let created = ProvisionedAgentFilesystem::new(
            backend,
            Box::new(UnmanagedCleanup {
                root,
                cleanup_retry: self.cleanup_retry.clone(),
            }),
            lifecycle,
        );
        if let Err(error) = verify_fresh_directory(created.backend().root()).await {
            return Err(created.rollback(error).await);
        }
        Ok(created)
    }

    async fn create_deterministic(
        &self,
        root: PathBuf,
        lifecycle: OwnedMutexGuard<()>,
    ) -> Result<ProvisionedAgentFilesystem, FilesystemStorageError> {
        remove_and_verify(&root, "remove stale runtime directory", &self.cleanup_retry).await?;
        let parent = root
            .parent()
            .expect("deterministic agent filesystem path must have a parent");
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

        let backend: Arc<dyn AgentFilesystemBackend> =
            Arc::new(UnmanagedAgentFilesystem { root: root.clone() });
        let created = ProvisionedAgentFilesystem::new(
            backend,
            Box::new(UnmanagedCleanup {
                root,
                cleanup_retry: self.cleanup_retry.clone(),
            }),
            lifecycle,
        );
        if let Err(error) = verify_fresh_directory(created.backend().root()).await {
            return Err(created.rollback(error).await);
        }
        Ok(created)
    }
}

#[async_trait]
impl FilesystemBackendProvisioner for UnmanagedBackend {
    fn initial_file_cache_root(&self) -> Option<&Path> {
        None
    }

    async fn provision_for(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<ProvisionedAgentFilesystem, FilesystemStorageError> {
        let Some(storage_root) = &self.deterministic_root else {
            return self.create_temporary().await;
        };
        let root = storage_root.join(agent_filesystem_owner_path(agent_id));
        let lifecycle = acquire_lifecycle_lock(&root).await;
        let backend = self.clone();
        let error_path = root.clone();
        tokio::spawn(async move { backend.create_deterministic(root, lifecycle).await })
            .await
            .map_err(|error| {
                FilesystemStorageError::io(
                    "provision unmanaged agent filesystem",
                    &error_path,
                    std::io::Error::other(error),
                )
            })?
    }

    async fn observe_capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError> {
        Err(FilesystemStorageError::verification(
            "observe capacity for unmanaged filesystem storage",
            Path::new("<unmanaged>"),
        ))
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub(super) struct UnmanagedAgentFilesystem {
    root: PathBuf,
}

impl UnmanagedAgentFilesystem {
    #[cfg(test)]
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl AgentFilesystemBackend for UnmanagedAgentFilesystem {
    fn root(&self) -> &Path {
        &self.root
    }

    fn create_staging_dir(&self) -> std::io::Result<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix(".golem-initial-files-update-")
            .tempdir_in(&self.root)
    }

    async fn materialize_initial_file(
        &self,
        materialization: InitialFileMaterialization,
    ) -> Result<(), FilesystemStorageError> {
        let InitialFileMaterialization {
            materialization_root,
            source,
            target,
            read_only,
            effect,
            staging,
        } = materialization;
        let operation_target = target.clone();
        tokio::task::spawn_blocking(move || {
            let _effect = effect;
            let _staging = staging;
            materialize_unmanaged(
                &materialization_root,
                source.path(),
                &operation_target,
                read_only,
            )
        })
        .await
        .map_err(|error| {
            FilesystemStorageError::io(
                "materialize unmanaged initial file",
                &target,
                std::io::Error::other(error),
            )
        })?
        .map_err(|error| {
            FilesystemStorageError::io("materialize unmanaged initial file", &target, error)
        })
    }

    async fn observe_capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError> {
        observe_path_capacity(&self.root).await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct UnmanagedCleanup {
    root: PathBuf,
    cleanup_retry: RetryConfig,
}

#[async_trait]
impl AgentFilesystemCleanup for UnmanagedCleanup {
    async fn delete(&mut self) -> Result<(), FilesystemStorageError> {
        remove_and_verify(&self.root, "delete runtime directory", &self.cleanup_retry).await
    }

    fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError> {
        remove_and_verify_blocking(&self.root)
    }
}

fn materialize_unmanaged(
    root: &Path,
    source: &Path,
    target: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    let parent = create_materialization_parent(root, target)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut source = std::fs::File::open(source)?;
    std::io::copy(&mut source, &mut temporary)?;
    temporary.as_file().sync_all()?;
    set_initial_file_permissions(temporary.as_file(), read_only)?;
    temporary
        .persist_noclobber(target)
        .map_err(|error| error.error)?;
    Ok(())
}
