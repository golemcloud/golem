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

use crate::metrics::workers::record_agent_filesystem_lifecycle;
use crate::services::file_loader::{FileLoader, InitialFileSource};
use crate::services::golem_config::FilesystemStorageConfig;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::component::{AgentFilePermissions, InitialAgentFile};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{OwnedAgentId, RetryConfig};
use golem_common::retries::RetryState;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::num::NonZeroU32;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Instant;
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedRwLockReadGuard};
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::{StreamError, StreamResult};

#[cfg(target_os = "linux")]
mod xfs;

static LIFECYCLE_LOCKS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentFilesystemUsage {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "authoritative capacity observation is part of the filesystem interface"
)]
pub(crate) struct FilesystemCapacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub total_filesystem_objects: u64,
    pub available_filesystem_objects: u64,
}

#[derive(Debug)]
pub struct FilesystemStorageError {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
}

impl FilesystemStorageError {
    fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: false,
        }
    }

    fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: false,
        }
    }

    fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: true,
        }
    }

    fn cleanup_verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: true,
        }
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }
}

impl Display for FilesystemStorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to {} agent filesystem {}",
            self.operation,
            self.path.display()
        )?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone)]
pub(crate) struct AgentFilesystems {
    deterministic_root: Option<PathBuf>,
    cleanup_retry: RetryConfig,
    #[cfg(target_os = "linux")]
    managed_xfs: Option<Arc<xfs::XfsBackend>>,
}

pub(crate) struct CreateAgentFilesystem {
    pub agent_id: OwnedAgentId,
    pub initial_files: Vec<InitialAgentFile>,
    pub file_loader: Arc<FileLoader>,
}

impl AgentFilesystems {
    pub(crate) fn new(settings: &FilesystemStorageConfig) -> Result<Self, FilesystemStorageError> {
        if settings.deterministic_root_dir.is_some() && settings.managed_xfs_root_dir.is_some() {
            return Err(FilesystemStorageError::verification(
                "select exactly one filesystem storage backend",
                Path::new("<configuration>"),
            ));
        }

        #[cfg(target_os = "linux")]
        let managed_xfs = match settings.managed_xfs_root_dir.as_deref() {
            Some(root) => Some(Arc::new(xfs::XfsBackend::new(
                root,
                &settings.cleanup_retry,
            )?)),
            None => None,
        };

        #[cfg(not(target_os = "linux"))]
        if let Some(root) = &settings.managed_xfs_root_dir {
            return Err(FilesystemStorageError::verification(
                "initialize managed XFS backend on a non-Linux platform",
                root,
            ));
        }

        Ok(Self {
            deterministic_root: settings.deterministic_root_dir.clone(),
            cleanup_retry: settings.cleanup_retry.clone(),
            #[cfg(target_os = "linux")]
            managed_xfs,
        })
    }

    #[allow(
        dead_code,
        reason = "authoritative capacity observation is part of the filesystem interface"
    )]
    pub(crate) async fn capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError> {
        #[cfg(target_os = "linux")]
        if let Some(backend) = &self.managed_xfs {
            let backend = Arc::clone(backend);
            let root = backend.root().to_path_buf();
            return tokio::task::spawn_blocking(move || backend.capacity())
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed XFS capacity",
                        &root,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("observe managed XFS capacity", &root, error)
                });
        }

        Err(FilesystemStorageError::verification(
            "observe capacity for unmanaged filesystem storage",
            Path::new("<unmanaged>"),
        ))
    }

    pub(crate) fn initial_file_cache_root(&self) -> Option<&Path> {
        #[cfg(target_os = "linux")]
        if let Some(backend) = &self.managed_xfs {
            return Some(backend.root());
        }
        None
    }

    pub(crate) async fn create_fresh(
        &self,
        request: CreateAgentFilesystem,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let filesystem = self.create_owned_empty(&request.agent_id).await?;
        if let Err(error) = filesystem
            .runtime
            .replace_initial_files(
                &request.file_loader,
                request.agent_id.environment_id,
                &request.initial_files,
            )
            .await
        {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }
        Ok(filesystem)
    }

    async fn create_owned_empty(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let started = Instant::now();
        #[cfg(target_os = "linux")]
        if let Some(backend) = &self.managed_xfs {
            let result = self
                .create_managed_filesystem(Arc::clone(backend), agent_id)
                .await;
            record_agent_filesystem_lifecycle("create", result.is_ok(), started.elapsed());
            return result;
        }

        let result = match &self.deterministic_root {
            Some(root) => self.create_deterministic_filesystem(root, agent_id).await,
            None => self.create_temporary_filesystem().await,
        };
        record_agent_filesystem_lifecycle("create", result.is_ok(), started.elapsed());
        result
    }

    #[cfg(target_os = "linux")]
    async fn create_managed_filesystem(
        &self,
        backend: Arc<xfs::XfsBackend>,
        agent_id: &OwnedAgentId,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let environment = agent_id.environment_id.to_string();
        let component = agent_id.agent_id.component_id.to_string();
        let agent = agent_id.agent_id.agent_name_encoded();
        let owner = PathBuf::from(&environment).join(&component).join(&agent);
        let lifecycle = acquire_lifecycle_lock(&backend.root().join(&owner)).await;
        let parent = backend
            .open_agent_parent(&environment, &component)
            .map_err(|error| {
                FilesystemStorageError::io(
                    "open managed runtime directory parent",
                    &backend
                        .root()
                        .join(owner.parent().expect("owner must have a parent")),
                    error,
                )
            })?;
        let cleanup_path =
            PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(&agent);

        let disk_project = match tokio::fs::symlink_metadata(&cleanup_path).await {
            Ok(metadata) if !metadata.file_type().is_symlink() => {
                let backend = Arc::clone(&backend);
                let existing_entry = backend.open_entry(&parent, &agent).map_err(|error| {
                    FilesystemStorageError::cleanup_io(
                        "open stale managed XFS runtime path",
                        &cleanup_path,
                        error,
                    )
                })?;
                tokio::task::spawn_blocking(move || backend.project_id(&existing_entry))
                    .await
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "inspect stale managed XFS project",
                            &cleanup_path,
                            std::io::Error::other(error),
                        )
                    })?
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "inspect stale managed XFS project",
                            &cleanup_path,
                            error,
                        )
                    })?
            }
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(FilesystemStorageError::cleanup_io(
                    "inspect stale managed XFS runtime path",
                    &cleanup_path,
                    error,
                ));
            }
        };
        let reserved_project = backend.reserved_project(&owner);
        let stale_project = match (disk_project, reserved_project) {
            (Some(disk_project), Some(reserved_project)) if disk_project != reserved_project => {
                return Err(FilesystemStorageError::cleanup_verification(
                    "match stale managed XFS path and reserved project",
                    &cleanup_path,
                ));
            }
            (disk_project, reserved_project) => disk_project.or(reserved_project),
        };

        let mut stale_cleanup = if let Some(project_id) = stale_project {
            backend
                .reserve_existing_project(project_id, &owner)
                .map_err(|error| {
                    FilesystemStorageError::cleanup_io(
                        "reserve stale managed XFS project for cleanup",
                        &cleanup_path,
                        error,
                    )
                })?;
            Some(ManagedProjectCleanup::new(
                Arc::clone(&backend),
                project_id,
                cleanup_path.clone(),
                parent.try_clone().map_err(|error| {
                    FilesystemStorageError::cleanup_io(
                        "retain managed XFS runtime directory parent",
                        &cleanup_path,
                        error,
                    )
                })?,
                self.cleanup_retry.clone(),
            ))
        } else {
            None
        };
        remove_and_verify(
            &cleanup_path,
            "remove stale managed XFS runtime directory",
            &self.cleanup_retry,
        )
        .await?;
        if let Some(project_id) = stale_project {
            finish_managed_project_cleanup(
                Arc::clone(&backend),
                project_id,
                &cleanup_path,
                &self.cleanup_retry,
            )
            .await?;
            stale_cleanup
                .as_mut()
                .expect("stale project cleanup owner must exist")
                .disarm();
        }

        tokio::fs::create_dir(&cleanup_path)
            .await
            .map_err(|error| {
                FilesystemStorageError::io(
                    "create fresh managed runtime directory",
                    &cleanup_path,
                    error,
                )
            })?;

        let project_id = backend.reserve_project(&owner).map_err(|error| {
            FilesystemStorageError::io("allocate managed XFS project", &cleanup_path, error)
        });
        let project_id = match project_id {
            Ok(project_id) => project_id,
            Err(error) => {
                return Err(rollback_creation(&cleanup_path, error, &self.cleanup_retry).await);
            }
        };
        let root = match backend.open_directory(&parent, &agent) {
            Ok(root) => root,
            Err(error) => {
                backend.release_project(project_id);
                return Err(rollback_creation(
                    &cleanup_path,
                    FilesystemStorageError::io(
                        "open fresh managed runtime directory",
                        &cleanup_path,
                        error,
                    ),
                    &self.cleanup_retry,
                )
                .await);
            }
        };
        let path = PathBuf::from(format!("/proc/self/fd/{}", root.as_raw_fd()));
        let assignment_result = backend.assign_project(&root, project_id).map_err(|error| {
            FilesystemStorageError::io("assign managed XFS project", &path, error)
        });

        let filesystem = AgentFilesystem {
            path: path.clone(),
            cleanup_path: cleanup_path.clone(),
            runtime: AgentFilesystemRuntime::new(AgentFilesystemMaterializer::Managed {
                root: path.clone(),
                staging_parent: cleanup_path
                    .parent()
                    .expect("managed filesystem path must have a parent")
                    .to_path_buf(),
                backend: Arc::clone(&backend),
                project_id,
            }),
            cleanup_retry: self.cleanup_retry.clone(),
            storage: AgentFilesystemStorage::Managed {
                backend: Arc::clone(&backend),
                project_id,
                parent: Some(parent),
                root: Some(root),
            },
            lifecycle: Some(lifecycle),
            delete_on_drop: true,
        };
        if let Err(error) = assignment_result {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }
        if let Err(error) = verify_fresh_open_directory(filesystem.path()).await {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }

        Ok(filesystem)
    }

    async fn create_temporary_filesystem(&self) -> Result<AgentFilesystem, FilesystemStorageError> {
        let directory = tempfile::Builder::new()
            .prefix("golem")
            .tempdir()
            .map_err(|error| {
                FilesystemStorageError::io("create temporary directory", Path::new("<temp>"), error)
            })?;
        let lifecycle = Arc::new(Mutex::new(()))
            .try_lock_owned()
            .expect("new temporary filesystem lifecycle lock must be available");
        let path = directory.keep();
        let filesystem = AgentFilesystem {
            cleanup_path: path.clone(),
            path: path.clone(),
            runtime: AgentFilesystemRuntime::new(AgentFilesystemMaterializer::Unmanaged {
                root: path.clone(),
                staging_parent: path
                    .parent()
                    .expect("temporary filesystem path must have a parent")
                    .to_path_buf(),
            }),
            cleanup_retry: self.cleanup_retry.clone(),
            storage: AgentFilesystemStorage::Unmanaged,
            lifecycle: Some(lifecycle),
            delete_on_drop: true,
        };

        if let Err(error) = verify_fresh_directory(filesystem.path()).await {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }

        Ok(filesystem)
    }

    async fn create_deterministic_filesystem(
        &self,
        root: &Path,
        agent_id: &OwnedAgentId,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let path = root
            .join(agent_id.environment_id.to_string())
            .join(agent_id.agent_id.component_id.to_string())
            .join(agent_id.agent_id.agent_name_encoded());

        let lifecycle = acquire_lifecycle_lock(&path).await;

        remove_and_verify(&path, "remove stale runtime directory", &self.cleanup_retry).await?;
        let parent = path
            .parent()
            .expect("deterministic agent filesystem path must have a parent");
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return Err(rollback_creation(
                &path,
                FilesystemStorageError::io("create runtime directory parent", parent, error),
                &self.cleanup_retry,
            )
            .await);
        }
        if let Err(error) = tokio::fs::create_dir(&path).await {
            return Err(rollback_creation(
                &path,
                FilesystemStorageError::io("create fresh runtime directory", &path, error),
                &self.cleanup_retry,
            )
            .await);
        }

        let filesystem = AgentFilesystem {
            cleanup_path: path.clone(),
            path: path.clone(),
            runtime: AgentFilesystemRuntime::new(AgentFilesystemMaterializer::Unmanaged {
                root: path.clone(),
                staging_parent: parent.to_path_buf(),
            }),
            cleanup_retry: self.cleanup_retry.clone(),
            storage: AgentFilesystemStorage::Unmanaged,
            lifecycle: Some(lifecycle),
            delete_on_drop: true,
        };

        if let Err(error) = verify_fresh_directory(filesystem.path()).await {
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }

        Ok(filesystem)
    }
}

pub(crate) struct AgentFilesystem {
    path: PathBuf,
    cleanup_path: PathBuf,
    runtime: AgentFilesystemRuntime,
    cleanup_retry: RetryConfig,
    storage: AgentFilesystemStorage,
    lifecycle: Option<OwnedMutexGuard<()>>,
    delete_on_drop: bool,
}

enum AgentFilesystemStorage {
    Unmanaged,
    #[cfg(target_os = "linux")]
    Managed {
        backend: Arc<xfs::XfsBackend>,
        project_id: NonZeroU32,
        parent: Option<File>,
        root: Option<File>,
    },
}

impl AgentFilesystem {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn runtime(&self) -> AgentFilesystemRuntime {
        self.runtime.clone()
    }

    pub(crate) fn seal(&self) {
        self.runtime.seal();
    }

    #[allow(
        dead_code,
        reason = "authoritative usage observation is part of the filesystem interface"
    )]
    pub(crate) async fn usage(
        &self,
    ) -> Result<Option<AgentFilesystemUsage>, FilesystemStorageError> {
        match &self.storage {
            AgentFilesystemStorage::Unmanaged => Ok(None),
            #[cfg(target_os = "linux")]
            AgentFilesystemStorage::Managed {
                backend,
                project_id,
                ..
            } => {
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                let path = self.path.clone();
                tokio::task::spawn_blocking(move || backend.usage(project_id))
                    .await
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS project usage",
                            &path,
                            std::io::Error::other(error),
                        )
                    })?
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS project usage",
                            &path,
                            error,
                        )
                    })
                    .map(Some)
            }
        }
    }

    pub(crate) async fn close_and_delete(mut self) -> Result<(), FilesystemStorageError> {
        let started = Instant::now();
        self.seal();
        self.runtime.drain().await;
        let result = match &mut self.storage {
            AgentFilesystemStorage::Unmanaged => {
                remove_and_verify(
                    &self.cleanup_path,
                    "delete runtime directory",
                    &self.cleanup_retry,
                )
                .await
            }
            #[cfg(target_os = "linux")]
            AgentFilesystemStorage::Managed {
                backend,
                project_id,
                root,
                ..
            } => {
                root.take();
                let path_result = remove_and_verify(
                    &self.cleanup_path,
                    "delete managed XFS runtime directory",
                    &self.cleanup_retry,
                )
                .await;
                match path_result {
                    Ok(()) => {
                        finish_managed_project_cleanup(
                            Arc::clone(backend),
                            *project_id,
                            &self.cleanup_path,
                            &self.cleanup_retry,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                }
            }
        };
        // A completed explicit attempt is authoritative. The fallback is only
        // for owners dropped before explicit cleanup can finish.
        self.delete_on_drop = false;
        record_agent_filesystem_lifecycle("delete", result.is_ok(), started.elapsed());
        result
    }
}

impl Drop for AgentFilesystem {
    fn drop(&mut self) {
        if self.delete_on_drop {
            let started = Instant::now();
            if self.runtime.has_active_effects() {
                self.runtime.seal();
                let runtime = self.runtime.clone();
                let cleanup_path = self.cleanup_path.clone();
                let cleanup_retry = self.cleanup_retry.clone();
                let lifecycle = self.lifecycle.take();
                match &mut self.storage {
                    AgentFilesystemStorage::Unmanaged => {
                        std::thread::spawn(move || {
                            while runtime.has_active_effects() {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                            let result = remove_and_verify_blocking(&cleanup_path);
                            record_agent_filesystem_lifecycle(
                                "delete_fallback",
                                result.is_ok(),
                                started.elapsed(),
                            );
                            if let Err(error) = result {
                                tracing::error!(error = %error, "Failed to delete agent runtime filesystem during deferred fallback cleanup");
                            }
                            drop(lifecycle);
                        });
                    }
                    #[cfg(target_os = "linux")]
                    AgentFilesystemStorage::Managed {
                        backend,
                        project_id,
                        parent,
                        root,
                    } => {
                        let backend = Arc::clone(backend);
                        let project_id = *project_id;
                        let parent = parent.take();
                        let root = root.take();
                        std::thread::spawn(move || {
                            while runtime.has_active_effects() {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                            drop(root);
                            let result = remove_managed_and_verify_blocking(
                                &backend,
                                project_id,
                                &cleanup_path,
                                &cleanup_retry,
                            );
                            record_agent_filesystem_lifecycle(
                                "delete_fallback",
                                result.is_ok(),
                                started.elapsed(),
                            );
                            if let Err(error) = result {
                                tracing::error!(error = %error, "Failed to delete agent runtime filesystem during deferred fallback cleanup");
                            }
                            drop((parent, lifecycle));
                        });
                    }
                }
                self.delete_on_drop = false;
                return;
            }
            let result = match &mut self.storage {
                AgentFilesystemStorage::Unmanaged => remove_and_verify_blocking(&self.cleanup_path),
                #[cfg(target_os = "linux")]
                AgentFilesystemStorage::Managed {
                    backend,
                    project_id,
                    root,
                    ..
                } => {
                    root.take();
                    remove_managed_and_verify_blocking(
                        backend,
                        *project_id,
                        &self.cleanup_path,
                        &self.cleanup_retry,
                    )
                }
            };
            record_agent_filesystem_lifecycle("delete_fallback", result.is_ok(), started.elapsed());
            if let Err(error) = result {
                tracing::error!(error = %error, "Failed to delete agent runtime filesystem during fallback cleanup");
            }
        }
    }
}

#[derive(Clone)]
pub struct AgentFilesystemRuntime {
    inner: Arc<AgentFilesystemRuntimeInner>,
}

struct AgentFilesystemRuntimeInner {
    state: AtomicUsize,
    drained: tokio::sync::Notify,
    append: Arc<Mutex<()>>,
    namespace: Arc<Mutex<()>>,
    operations: Arc<tokio::sync::RwLock<()>>,
    materializer: AgentFilesystemMaterializer,
    initial_files: std::sync::RwLock<HashMap<PathBuf, InitialAgentFile>>,
}

enum AgentFilesystemMaterializer {
    Unmanaged {
        root: PathBuf,
        staging_parent: PathBuf,
    },
    #[cfg(target_os = "linux")]
    Managed {
        root: PathBuf,
        staging_parent: PathBuf,
        backend: Arc<xfs::XfsBackend>,
        project_id: NonZeroU32,
    },
}

const FILESYSTEM_RUNTIME_SEALED: usize = 1 << (usize::BITS - 1);
const FILESYSTEM_RUNTIME_ACTIVE_EFFECTS: usize = !FILESYSTEM_RUNTIME_SEALED;

impl AgentFilesystemRuntime {
    fn new(materializer: AgentFilesystemMaterializer) -> Self {
        Self {
            inner: Arc::new(AgentFilesystemRuntimeInner {
                state: AtomicUsize::new(0),
                drained: tokio::sync::Notify::new(),
                append: Arc::new(Mutex::new(())),
                namespace: Arc::new(Mutex::new(())),
                operations: Arc::new(tokio::sync::RwLock::new(())),
                materializer,
                initial_files: std::sync::RwLock::new(HashMap::new()),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new(AgentFilesystemMaterializer::Unmanaged {
            root: PathBuf::from("<test>"),
            staging_parent: std::env::temp_dir(),
        })
    }

    pub(crate) fn is_read_only(&self, path: &Path) -> bool {
        self.is_read_only_path(path, true)
    }

    pub(crate) fn is_read_only_path(&self, path: &Path, follow_final_symlink: bool) -> bool {
        let path = resolve_policy_path(path, follow_final_symlink);
        self.inner
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .iter()
            .any(|(initial_path, file)| {
                file.permissions == AgentFilePermissions::ReadOnly
                    && resolve_policy_path(initial_path, true) == path
            })
    }

    pub(crate) fn contains_read_only_path(&self, path: &Path, follow_final_symlink: bool) -> bool {
        let path = resolve_policy_path(path, follow_final_symlink);
        self.inner
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .iter()
            .any(|(initial_path, file)| {
                let initial_path = resolve_policy_path(initial_path, true);
                file.permissions == AgentFilePermissions::ReadOnly
                    && (initial_path == path || initial_path.starts_with(&path))
            })
    }

    pub(crate) async fn replace_initial_files(
        &self,
        file_loader: &FileLoader,
        environment_id: EnvironmentId,
        files: &[InitialAgentFile],
    ) -> Result<(), FilesystemStorageError> {
        let effect = self.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "admit initial-file materialization",
                self.inner.materializer.root(),
                std::io::Error::other(error),
            )
        })?;
        let mut materialized = HashMap::new();
        let mut sources: HashMap<AgentFileContentHash, InitialFileSource> = HashMap::new();
        for file in files {
            let target = self.inner.materializer.target(file);
            let source = match sources.get(&file.content_hash) {
                Some(source) if source.size() == file.size => source,
                Some(_) => {
                    return Err(FilesystemStorageError::verification(
                        "verify consistent initial-file source size",
                        &target,
                    ));
                }
                None => {
                    let source = file_loader
                        .get_source(environment_id, file.content_hash, file.size)
                        .await
                        .map_err(|error| {
                            FilesystemStorageError::io(
                                "load verified initial-file source",
                                &target,
                                std::io::Error::other(error),
                            )
                        })?;
                    sources.entry(file.content_hash).or_insert(source)
                }
            };
            self.inner
                .materializer
                .materialize(source, file, effect.clone())
                .await?;
            if materialized.insert(target.clone(), file.clone()).is_some() {
                return Err(FilesystemStorageError::verification(
                    "materialize unique initial-file target",
                    &target,
                ));
            }
        }
        *self
            .inner
            .initial_files
            .write()
            .expect("initial-files policy lock poisoned") = materialized;
        Ok(())
    }

    pub(crate) async fn update_initial_files(
        &self,
        file_loader: &FileLoader,
        environment_id: EnvironmentId,
        files: &[InitialAgentFile],
    ) -> Result<AgentFilesystemUpdateEffectLease, FilesystemStorageError> {
        let effect = self.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "admit initial-file update",
                self.inner.materializer.root(),
                std::io::Error::other(error),
            )
        })?;
        let current = self
            .inner
            .initial_files
            .read()
            .expect("initial-files policy lock poisoned")
            .clone();
        let mut desired = HashMap::new();
        for file in files {
            let target = self.inner.materializer.target(file);
            if desired.insert(target.clone(), file.clone()).is_some() {
                return Err(FilesystemStorageError::verification(
                    "materialize unique initial-file update target",
                    &target,
                ));
            }
        }

        for (path, file) in &desired {
            if current.get(path).is_some_and(|existing| {
                existing.permissions == AgentFilePermissions::ReadWrite
                    && file.permissions == AgentFilePermissions::ReadOnly
            }) {
                return Err(FilesystemStorageError::verification(
                    "replace read-write initial file with read-only content",
                    path,
                ));
            }
        }

        let staging = self
            .inner
            .materializer
            .create_staging_dir()
            .map_err(|error| {
                FilesystemStorageError::io(
                    "create initial-file update staging directory",
                    self.inner.materializer.root(),
                    error,
                )
            })?;
        let mut sources: HashMap<AgentFileContentHash, InitialFileSource> = HashMap::new();
        let mut staged = Vec::new();
        for (path, file) in &desired {
            match current.get(path) {
                Some(existing)
                    if existing.permissions == AgentFilePermissions::ReadWrite
                        && file.permissions == AgentFilePermissions::ReadWrite => {}
                Some(existing)
                    if existing.permissions == AgentFilePermissions::ReadOnly
                        && existing.content_hash == file.content_hash
                        && file.permissions == AgentFilePermissions::ReadOnly => {}
                None if file.permissions == AgentFilePermissions::ReadWrite
                    && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) => {
                }
                _ => {
                    let source = match sources.get(&file.content_hash) {
                        Some(source) if source.size() == file.size => source,
                        Some(_) => {
                            return Err(FilesystemStorageError::verification(
                                "verify consistent initial-file update source size",
                                path,
                            ));
                        }
                        None => {
                            let source = file_loader
                                .get_source(environment_id, file.content_hash, file.size)
                                .await
                                .map_err(|error| {
                                    FilesystemStorageError::io(
                                        "load verified initial-file update source",
                                        path,
                                        std::io::Error::other(error),
                                    )
                                })?;
                            sources.entry(file.content_hash).or_insert(source)
                        }
                    };
                    let staged_path = staging.path().join(staged.len().to_string());
                    self.inner
                        .materializer
                        .materialize_at(source, file, staging.path(), &staged_path, effect.clone())
                        .await?;
                    staged.push((path.clone(), staged_path));
                }
            }
        }

        let backup_root = staging.path().join("backups");
        std::fs::create_dir(&backup_root).map_err(|error| {
            FilesystemStorageError::io(
                "create initial-file update backup directory",
                &backup_root,
                error,
            )
        })?;
        let mut replacements: Vec<PathBuf> = staged.iter().map(|(path, _)| path.clone()).collect();
        replacements.extend(
            current
                .iter()
                .filter(|(path, existing)| {
                    existing.permissions == AgentFilePermissions::ReadOnly
                        && !desired.contains_key(*path)
                })
                .map(|(path, _)| path.clone()),
        );
        replacements.sort();
        replacements.dedup();

        let mut transaction = InitialFileUpdateTransaction::new(backup_root);
        for path in &replacements {
            if let Err(error) = transaction
                .create_parent(self.inner.materializer.root(), path)
                .and_then(|_| validate_replaceable_target(path))
            {
                return Err(transaction.fail("prepare initial-file update target", path, error));
            }
            let exists = std::fs::symlink_metadata(path).is_ok();
            if exists && !current.contains_key(path) {
                return Err(transaction.fail(
                    "preserve existing guest filesystem target",
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "initial-file update target already exists",
                    ),
                ));
            }
            if exists && let Err(error) = transaction.back_up(path) {
                return Err(transaction.fail("stage existing initial-file target", path, error));
            }
        }

        for (target, staged_path) in &staged {
            if let Err(error) = transaction.install(staged_path, target) {
                return Err(transaction.fail("install initial-file update target", target, error));
            }
        }

        let staging_path = staging.path().to_path_buf();
        if let Err(error) = staging.close() {
            let error = transaction.fail(
                "remove initial-file update staging directory",
                &staging_path,
                error,
            );
            self.seal();
            return Err(error);
        }
        transaction.commit();
        *self
            .inner
            .initial_files
            .write()
            .expect("initial-files policy lock poisoned") = desired;
        Ok(effect)
    }

    pub(crate) async fn begin_effect(&self) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin().await)
    }

    pub(crate) async fn begin_append_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin_append().await)
    }

    pub(crate) async fn begin_path_effect(
        &self,
    ) -> Result<AgentFilesystemEffectLease, wasmtime::Error> {
        Ok(self.admit_effect()?.begin_path().await)
    }

    async fn begin_update_effect(
        &self,
    ) -> Result<AgentFilesystemUpdateEffectLease, wasmtime::Error> {
        let admission = self.admit_effect()?;
        let operation_guard = Arc::clone(&self.inner.operations).write_owned().await;
        Ok(AgentFilesystemUpdateEffectLease {
            _inner: Arc::new(AgentFilesystemUpdateEffectLeaseInner {
                _admission: admission,
                _operation_guard: operation_guard,
            }),
        })
    }

    pub(crate) fn admit_effect(&self) -> Result<AgentFilesystemEffectAdmission, wasmtime::Error> {
        let mut state = self.inner.state.load(Ordering::Acquire);
        loop {
            if state & FILESYSTEM_RUNTIME_SEALED != 0 {
                return Err(wasmtime::Error::msg("agent filesystem is closing"));
            }
            let active_effects = state & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS;
            let next = active_effects
                .checked_add(1)
                .expect("agent filesystem effect count overflowed");
            match self.inner.state.compare_exchange_weak(
                state,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => state = observed,
            }
        }
        Ok(AgentFilesystemEffectAdmission {
            inner: Arc::clone(&self.inner),
        })
    }

    fn seal(&self) {
        self.inner
            .state
            .fetch_or(FILESYSTEM_RUNTIME_SEALED, Ordering::AcqRel);
    }

    async fn drain(&self) {
        while self.has_active_effects() {
            let drained = self.inner.drained.notified();
            if !self.has_active_effects() {
                break;
            }
            drained.await;
        }
    }

    fn has_active_effects(&self) -> bool {
        self.inner.state.load(Ordering::Acquire) & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS != 0
    }
}

impl AgentFilesystemMaterializer {
    fn root(&self) -> &Path {
        match self {
            Self::Unmanaged { root, .. } => root,
            #[cfg(target_os = "linux")]
            Self::Managed { root, .. } => root,
        }
    }

    fn create_staging_dir(&self) -> std::io::Result<tempfile::TempDir> {
        let staging_parent = match self {
            Self::Unmanaged { staging_parent, .. } => staging_parent,
            #[cfg(target_os = "linux")]
            Self::Managed { staging_parent, .. } => staging_parent,
        };
        let staging = tempfile::Builder::new()
            .prefix(".golem-initial-files-update-")
            .tempdir_in(staging_parent)?;
        #[cfg(target_os = "linux")]
        if let Self::Managed {
            backend,
            project_id,
            ..
        } = self
        {
            let directory = File::open(staging.path())?;
            backend.assign_project(&directory, *project_id)?;
        }
        Ok(staging)
    }

    fn target(&self, file: &InitialAgentFile) -> PathBuf {
        self.root().join(file.path.to_rel_string())
    }

    async fn materialize(
        &self,
        source: &InitialFileSource,
        file: &InitialAgentFile,
        effect: AgentFilesystemUpdateEffectLease,
    ) -> Result<(), FilesystemStorageError> {
        let target = self.target(file);
        self.materialize_at(source, file, self.root(), &target, effect)
            .await
    }

    async fn materialize_at(
        &self,
        source: &InitialFileSource,
        file: &InitialAgentFile,
        materialization_root: &Path,
        target: &Path,
        effect: AgentFilesystemUpdateEffectLease,
    ) -> Result<(), FilesystemStorageError> {
        let target = target.to_path_buf();
        let source = source.clone();
        let read_only = file.permissions == AgentFilePermissions::ReadOnly;
        match self {
            Self::Unmanaged { .. } => {
                let root = materialization_root.to_path_buf();
                let operation_target = target.clone();
                tokio::task::spawn_blocking(move || {
                    let _effect = effect;
                    materialize_unmanaged(&root, source.path(), &operation_target, read_only)
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
            #[cfg(target_os = "linux")]
            Self::Managed {
                backend,
                project_id,
                ..
            } => {
                let root = materialization_root.to_path_buf();
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                let operation_target = target.clone();
                tokio::task::spawn_blocking(move || {
                    let _effect = effect;
                    backend.materialize_initial_file(
                        &root,
                        project_id,
                        source.path(),
                        &operation_target,
                        read_only,
                    )
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "reflink managed XFS initial file",
                        &target,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("reflink managed XFS initial file", &target, error)
                })
            }
        }
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

pub(super) fn create_materialization_parent<'a>(
    root: &Path,
    target: &'a Path,
) -> std::io::Result<&'a Path> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "initial-file target has no parent",
        )
    })?;
    let relative = parent.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "initial-file target escapes the agent filesystem",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "initial-file target contains an invalid path component",
            ));
        };
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "initial-file parent is not a directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(parent)
}

pub(super) fn set_initial_file_permissions(
    file: &std::fs::File,
    read_only: bool,
) -> std::io::Result<()> {
    let mut permissions = file.metadata()?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if read_only { 0o444 } else { 0o644 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(read_only);
    file.set_permissions(permissions)
}

fn validate_replaceable_target(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => Ok(()),
        Ok(metadata) if metadata.is_dir() => {
            if std::fs::read_dir(path)?.next().is_some() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "initial-file target directory is not empty",
                ))
            } else {
                Ok(())
            }
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "initial-file target is not replaceable",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

struct InitialFileUpdateTransaction {
    backup_root: PathBuf,
    backups: Vec<(PathBuf, PathBuf)>,
    installed: Vec<PathBuf>,
    created_directories: Vec<PathBuf>,
    committed: bool,
}

impl InitialFileUpdateTransaction {
    fn new(backup_root: PathBuf) -> Self {
        Self {
            backup_root,
            backups: Vec::new(),
            installed: Vec::new(),
            created_directories: Vec::new(),
            committed: false,
        }
    }

    fn create_parent(&mut self, root: &Path, target: &Path) -> std::io::Result<()> {
        let parent = target.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "initial-file target has no parent",
            )
        })?;
        let relative = parent.strip_prefix(root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "initial-file target escapes the agent filesystem",
            )
        })?;
        let mut current = root.to_path_buf();
        for component in relative.components() {
            let std::path::Component::Normal(component) = component else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "initial-file target contains an invalid path component",
                ));
            };
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "initial-file parent is not a directory",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current)?;
                    self.created_directories.push(current.clone());
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn back_up(&mut self, path: &Path) -> std::io::Result<()> {
        let backup = self.backup_root.join(self.backups.len().to_string());
        std::fs::rename(path, &backup)?;
        self.backups.push((path.to_path_buf(), backup));
        Ok(())
    }

    fn install(&mut self, staged: &Path, target: &Path) -> std::io::Result<()> {
        std::fs::rename(staged, target)?;
        self.installed.push(target.to_path_buf());
        Ok(())
    }

    fn fail(
        &mut self,
        operation: &'static str,
        path: &Path,
        error: std::io::Error,
    ) -> FilesystemStorageError {
        match self.rollback() {
            Ok(()) => FilesystemStorageError::io(operation, path, error),
            Err((rollback_path, rollback_error)) => FilesystemStorageError::cleanup_io(
                "roll back failed initial-file update",
                &rollback_path,
                rollback_error,
            ),
        }
    }

    fn rollback(&mut self) -> Result<(), (PathBuf, std::io::Error)> {
        let mut failure = None;
        for path in self.installed.drain(..).rev() {
            let result = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => std::fs::remove_dir(&path),
                Ok(_) => std::fs::remove_file(&path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                failure = Some((path, error));
            }
        }
        for (original, backup) in self.backups.drain(..).rev() {
            if let Err(error) = std::fs::rename(&backup, &original) {
                failure = Some((original, error));
            }
        }
        for path in self.created_directories.drain(..).rev() {
            if let Err(error) = std::fs::remove_dir(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                failure = Some((path, error));
            }
        }
        self.committed = true;
        failure.map_or(Ok(()), Err)
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for InitialFileUpdateTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

fn resolve_policy_path(path: &Path, follow_final_symlink: bool) -> PathBuf {
    if !follow_final_symlink && let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        return resolve_policy_path(parent, true).join(name);
    }
    let mut unresolved = Vec::new();
    let mut current = path;
    loop {
        match std::fs::canonicalize(current) {
            Ok(mut resolved) => {
                for component in unresolved.iter().rev() {
                    resolved.push(component);
                }
                return resolved;
            }
            Err(_) => match (current.parent(), current.file_name()) {
                (Some(parent), Some(name)) => {
                    unresolved.push(name.to_os_string());
                    current = parent;
                }
                _ => return path.to_path_buf(),
            },
        }
    }
}

impl AgentFilesystemRuntimeInner {
    fn finish_effect(&self) {
        let previous = self.state.fetch_sub(1, Ordering::AcqRel);
        let previous_active = previous & FILESYSTEM_RUNTIME_ACTIVE_EFFECTS;
        debug_assert!(previous_active > 0);
        if previous_active == 1 {
            self.drained.notify_waiters();
        }
    }
}

#[derive(Debug)]
pub(crate) struct AgentFilesystemEffectLease {
    _admission: AgentFilesystemEffectAdmission,
    _operation_guard: OwnedRwLockReadGuard<()>,
    _append_guard: Option<OwnedMutexGuard<()>>,
    _namespace_guard: Option<OwnedMutexGuard<()>>,
}

#[derive(Clone)]
pub(crate) struct AgentFilesystemUpdateEffectLease {
    _inner: Arc<AgentFilesystemUpdateEffectLeaseInner>,
}

struct AgentFilesystemUpdateEffectLeaseInner {
    _admission: AgentFilesystemEffectAdmission,
    _operation_guard: tokio::sync::OwnedRwLockWriteGuard<()>,
}

pub(crate) struct AgentFilesystemEffectAdmission {
    inner: Arc<AgentFilesystemRuntimeInner>,
}

impl Drop for AgentFilesystemEffectAdmission {
    fn drop(&mut self) {
        self.inner.finish_effect();
    }
}

impl AgentFilesystemEffectAdmission {
    pub(crate) async fn begin(self) -> AgentFilesystemEffectLease {
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: None,
        }
    }

    pub(crate) async fn begin_append(self) -> AgentFilesystemEffectLease {
        let guard = Arc::clone(&self.inner.append).lock_owned().await;
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: Some(guard),
            _namespace_guard: None,
        }
    }

    pub(crate) async fn begin_path(self) -> AgentFilesystemEffectLease {
        let operation_guard = Arc::clone(&self.inner.operations).read_owned().await;
        let namespace_guard = Arc::clone(&self.inner.namespace).lock_owned().await;
        AgentFilesystemEffectLease {
            _admission: self,
            _operation_guard: operation_guard,
            _append_guard: None,
            _namespace_guard: Some(namespace_guard),
        }
    }
}

impl std::fmt::Debug for AgentFilesystemEffectAdmission {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFilesystemEffectAdmission")
            .finish_non_exhaustive()
    }
}

pub(crate) struct CoordinatedFileOutputStream {
    inner: Arc<Mutex<DynOutputStream>>,
    active: Arc<AtomicBool>,
    ready: Arc<tokio::sync::Notify>,
    cancel: Arc<std::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    prepared_effect: std::sync::Mutex<Option<AgentFilesystemEffectLease>>,
}

struct ActiveFilesystemEffect {
    active: Arc<AtomicBool>,
    ready: Arc<tokio::sync::Notify>,
    _effect: Option<AgentFilesystemEffectLease>,
}

impl Drop for ActiveFilesystemEffect {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.ready.notify_waiters();
    }
}

impl CoordinatedFileOutputStream {
    pub(crate) fn new(inner: DynOutputStream) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            active: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(tokio::sync::Notify::new()),
            cancel: Arc::new(std::sync::Mutex::new(None)),
            prepared_effect: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn into_dyn(self) -> DynOutputStream {
        Box::new(self)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn prepare_effect(&self, effect: AgentFilesystemEffectLease) {
        let previous = self
            .prepared_effect
            .lock()
            .expect("filesystem output stream effect lock poisoned")
            .replace(effect);
        debug_assert!(previous.is_none());
    }

    pub(crate) fn clear_unused_effect(&self) {
        self.prepared_effect
            .lock()
            .expect("filesystem output stream effect lock poisoned")
            .take();
    }

    async fn wait_until_ready(&self) {
        while self.is_active() {
            let notified = self.ready.notified();
            if !self.is_active() {
                break;
            }
            notified.await;
        }
    }
}

#[async_trait]
impl OutputStream for CoordinatedFileOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if self.is_active() {
            return Err(StreamError::Trap(wasmtime::Error::msg(
                "write not permitted: check_write not called first",
            )));
        }

        let result = self
            .inner
            .try_lock()
            .expect("inactive filesystem output stream must not be locked")
            .write(bytes);
        if result.is_ok() {
            let effect = self
                .prepared_effect
                .lock()
                .expect("filesystem output stream effect lock poisoned")
                .take();
            self.active.store(true, Ordering::Release);
            let cancellation = tokio_util::sync::CancellationToken::new();
            self.cancel
                .lock()
                .expect("filesystem output stream cancellation lock poisoned")
                .replace(cancellation.clone());
            let inner = Arc::clone(&self.inner);
            let active = Arc::clone(&self.active);
            let ready = Arc::clone(&self.ready);
            tokio::spawn(async move {
                let _effect = ActiveFilesystemEffect {
                    active,
                    ready,
                    _effect: effect,
                };
                let mut inner = inner.lock().await;
                tokio::select! {
                    _ = inner.ready() => {}
                    _ = cancellation.cancelled() => inner.cancel().await,
                }
            });
        } else {
            self.clear_unused_effect();
        }
        result
    }

    fn flush(&mut self) -> StreamResult<()> {
        if self.is_active() {
            // Native file streams buffer only in the active write task, so their
            // flush operation is a successful no-op while that task is pending.
            Ok(())
        } else {
            self.inner
                .try_lock()
                .expect("inactive filesystem output stream must not be locked")
                .flush()
        }
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if self.is_active() {
            Ok(0)
        } else {
            self.inner
                .try_lock()
                .expect("inactive filesystem output stream must not be locked")
                .check_write()
        }
    }

    async fn cancel(&mut self) {
        let was_active = self.is_active();
        if was_active
            && let Some(cancellation) = self
                .cancel
                .lock()
                .expect("filesystem output stream cancellation lock poisoned")
                .as_ref()
        {
            cancellation.cancel();
        }
        self.wait_until_ready().await;
        if !was_active {
            self.inner.lock().await.cancel().await;
        }
        self.clear_unused_effect();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Pollable for CoordinatedFileOutputStream {
    async fn ready(&mut self) {
        self.wait_until_ready().await;
        self.inner.lock().await.ready().await;
    }
}

impl Drop for CoordinatedFileOutputStream {
    fn drop(&mut self) {
        if let Some(cancellation) = self
            .cancel
            .lock()
            .expect("filesystem output stream cancellation lock poisoned")
            .as_ref()
        {
            cancellation.cancel();
        }
        self.clear_unused_effect();
    }
}

async fn acquire_lifecycle_lock(path: &Path) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = LIFECYCLE_LOCKS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .expect("agent filesystem lifecycle lock registry poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(path).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    lock.lock_owned().await
}

async fn verify_fresh_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }

    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", path, error)
    })?;
    let empty = entries
        .next_entry()
        .await
        .map_err(|error| FilesystemStorageError::io("verify empty runtime directory", path, error))?
        .is_none();
    if !empty {
        return Err(FilesystemStorageError::verification(
            "verify empty runtime directory",
            path,
        ));
    }

    Ok(())
}

async fn verify_fresh_open_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }

    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", path, error)
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| FilesystemStorageError::io("verify empty runtime directory", path, error))?
        .is_some()
    {
        return Err(FilesystemStorageError::verification(
            "verify empty runtime directory",
            path,
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct ManagedProjectCleanup {
    backend: Arc<xfs::XfsBackend>,
    project_id: NonZeroU32,
    path: PathBuf,
    _parent: File,
    cleanup_retry: RetryConfig,
    armed: bool,
}

#[cfg(target_os = "linux")]
impl ManagedProjectCleanup {
    fn new(
        backend: Arc<xfs::XfsBackend>,
        project_id: NonZeroU32,
        path: PathBuf,
        parent: File,
        cleanup_retry: RetryConfig,
    ) -> Self {
        Self {
            backend,
            project_id,
            path,
            _parent: parent,
            cleanup_retry,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(target_os = "linux")]
impl Drop for ManagedProjectCleanup {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = remove_managed_and_verify_blocking(
                &self.backend,
                self.project_id,
                &self.path,
                &self.cleanup_retry,
            )
        {
            tracing::error!(error = %error, "Failed to clean reserved managed XFS project");
        }
    }
}

async fn rollback_creation(
    path: &Path,
    creation_error: FilesystemStorageError,
    cleanup_retry: &RetryConfig,
) -> FilesystemStorageError {
    match remove_and_verify(path, "roll back runtime directory", cleanup_retry).await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

async fn rollback_owned_filesystem(
    filesystem: AgentFilesystem,
    creation_error: FilesystemStorageError,
) -> FilesystemStorageError {
    match filesystem.close_and_delete().await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

#[cfg(target_os = "linux")]
async fn finish_managed_project_cleanup(
    backend: Arc<xfs::XfsBackend>,
    project_id: NonZeroU32,
    path: &Path,
    cleanup_retry: &RetryConfig,
) -> Result<(), FilesystemStorageError> {
    let mut retry = RetryState::new(cleanup_retry);
    loop {
        retry.start_attempt();
        let attempt_backend = Arc::clone(&backend);
        let attempt =
            tokio::task::spawn_blocking(move || attempt_backend.finish_project_cleanup(project_id))
                .await
                .map_err(|error| {
                    FilesystemStorageError::cleanup_io(
                        "verify and clear managed XFS project",
                        path,
                        std::io::Error::other(error),
                    )
                })?;
        match attempt {
            Ok(()) => {
                backend.release_project(project_id);
                return Ok(());
            }
            Err(error) => {
                if !retry.failed_attempt().await {
                    return Err(FilesystemStorageError::cleanup_io(
                        "verify and clear managed XFS project",
                        path,
                        error,
                    ));
                }
            }
        }
    }
}

async fn remove_and_verify(
    path: &Path,
    operation: &'static str,
    cleanup_retry: &RetryConfig,
) -> Result<(), FilesystemStorageError> {
    let mut retry = RetryState::new(cleanup_retry);
    loop {
        retry.start_attempt();
        match remove_and_verify_once(path, operation).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !retry.failed_attempt().await {
                    return Err(error);
                }
            }
        }
    }
}

async fn remove_and_verify_once(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Ok(_) => {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }

    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            operation, path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }
}

fn remove_and_verify_blocking(path: &Path) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|error| {
                FilesystemStorageError::cleanup_io("delete runtime directory", path, error)
            })?;
        }
        Ok(_) => {
            std::fs::remove_file(path).map_err(|error| {
                FilesystemStorageError::cleanup_io("delete runtime directory", path, error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilesystemStorageError::cleanup_io(
                "delete runtime directory",
                path,
                error,
            ));
        }
    }

    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            "delete runtime directory",
            path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(
            "delete runtime directory",
            path,
            error,
        )),
    }
}

#[cfg(target_os = "linux")]
fn remove_managed_and_verify_blocking(
    backend: &Arc<xfs::XfsBackend>,
    project_id: NonZeroU32,
    path: &Path,
    cleanup_retry: &RetryConfig,
) -> Result<(), FilesystemStorageError> {
    remove_and_verify_blocking(path)?;
    let attempts = cleanup_retry.max_attempts.max(1);
    let mut delay = cleanup_retry.min_delay;
    for attempt in 1..=attempts {
        match backend.finish_project_cleanup(project_id) {
            Ok(()) => {
                backend.release_project(project_id);
                return Ok(());
            }
            Err(error) if attempt == attempts => {
                return Err(FilesystemStorageError::cleanup_io(
                    "verify and clear managed XFS project",
                    path,
                    error,
                ));
            }
            Err(_) => {
                std::thread::sleep(delay);
                delay = delay
                    .mul_f64(cleanup_retry.multiplier)
                    .min(cleanup_retry.max_delay);
            }
        }
    }
    unreachable!("managed XFS cleanup always performs at least one attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::{AgentFilePath, ComponentId};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use golem_common::widen_infallible;
    use golem_service_base::replayable_stream::ReplayableStream as _;
    use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use test_r::test;

    struct DelayedOutputStream {
        ready: Arc<tokio::sync::Semaphore>,
        cancelled: Arc<AtomicBool>,
    }

    #[async_trait]
    impl OutputStream for DelayedOutputStream {
        fn write(&mut self, _bytes: Bytes) -> StreamResult<()> {
            Ok(())
        }

        fn flush(&mut self) -> StreamResult<()> {
            Ok(())
        }

        fn check_write(&mut self) -> StreamResult<usize> {
            Ok(1024)
        }

        async fn cancel(&mut self) {
            self.cancelled.store(true, Ordering::Release);
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[async_trait]
    impl Pollable for DelayedOutputStream {
        async fn ready(&mut self) {
            self.ready
                .acquire()
                .await
                .expect("test readiness semaphore closed")
                .forget();
        }
    }

    fn agent_id() -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId::from_agent_name_string(ComponentId::new(), "agent").unwrap(),
        )
    }

    async fn file_loader_with_content(
        environment_id: EnvironmentId,
        cache_parent: Option<&Path>,
        content: &[u8],
    ) -> (
        Arc<FileLoader>,
        golem_common::model::agent::AgentFileContentHash,
    ) {
        let service = Arc::new(InitialAgentFilesService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        let hash = service
            .put_if_not_exists(
                environment_id,
                content
                    .to_vec()
                    .map_error(widen_infallible::<anyhow::Error>)
                    .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
            )
            .await
            .unwrap();
        (
            Arc::new(FileLoader::new(service, None, cache_parent).unwrap()),
            hash,
        )
    }

    fn initial_file(
        content_hash: golem_common::model::agent::AgentFileContentHash,
        path: &str,
        permissions: AgentFilePermissions,
        size: u64,
    ) -> InitialAgentFile {
        InitialAgentFile {
            content_hash,
            path: AgentFilePath::from_abs_str(path).unwrap(),
            permissions,
            size,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn managed_backend_fails_closed_on_non_xfs() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            managed_xfs_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };

        let error = match AgentFilesystems::new(&settings) {
            Ok(_) => panic!("managed backend unexpectedly accepted a non-XFS root"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("validate managed XFS root"));
    }

    #[cfg(all(target_os = "linux", feature = "managed-xfs-tests"))]
    #[test]
    async fn managed_xfs_owns_observes_and_cleans_project_filesystem() {
        let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
            .map(PathBuf::from)
            .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
        let settings = FilesystemStorageConfig {
            managed_xfs_root_dir: Some(root.clone()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();

        let second_owner = AgentFilesystems::new(&settings);
        assert!(second_owner.is_err());

        let escaped_id = agent_id();
        let outside = tempfile::tempdir().unwrap();
        let environment_link = root.join(escaped_id.environment_id.to_string());
        std::os::unix::fs::symlink(outside.path(), &environment_link).unwrap();
        assert!(filesystems.create_owned_empty(&escaped_id).await.is_err());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        std::fs::remove_file(environment_link).unwrap();

        let stale_file_id = agent_id();
        let backend = Arc::clone(filesystems.managed_xfs.as_ref().unwrap());
        let environment = stale_file_id.environment_id.to_string();
        let component = stale_file_id.agent_id.component_id.to_string();
        let agent = stale_file_id.agent_id.agent_name_encoded();
        let owner = PathBuf::from(&environment).join(&component).join(&agent);
        let parent = backend.open_agent_parent(&environment, &component).unwrap();
        let parent_path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
        let stale_file = parent_path.join(&agent);
        let staging = parent_path.join(format!("{agent}.staging"));
        std::fs::create_dir(&staging).unwrap();
        let stale_project = backend.reserve_project(&owner).unwrap();
        let staging_directory = File::open(&staging).unwrap();
        backend
            .assign_project(&staging_directory, stale_project)
            .unwrap();
        std::fs::write(staging.join("file"), b"stale").unwrap();
        std::fs::rename(staging.join("file"), &stale_file).unwrap();
        drop(staging_directory);
        std::fs::remove_dir(staging).unwrap();
        drop(parent);

        let stale_file_replacement = filesystems
            .create_owned_empty(&stale_file_id)
            .await
            .unwrap();
        assert!(stale_file_replacement.path().is_dir());
        stale_file_replacement.close_and_delete().await.unwrap();
        assert_eq!(
            backend.usage(stale_project).unwrap(),
            AgentFilesystemUsage {
                allocated_bytes: 0,
                filesystem_objects: 0,
            }
        );

        let capacity = filesystems.capacity().await.unwrap();
        assert!(capacity.total_bytes > 0);
        assert!(capacity.available_bytes <= capacity.total_bytes);
        assert!(capacity.total_filesystem_objects > 0);
        assert!(capacity.available_filesystem_objects <= capacity.total_filesystem_objects);

        let materialized_id = agent_id();
        let content = vec![0x5a; 8192];
        let (file_loader, content_hash) = file_loader_with_content(
            materialized_id.environment_id,
            filesystems.initial_file_cache_root(),
            &content,
        )
        .await;
        let filesystem = filesystems
            .create_fresh(CreateAgentFilesystem {
                agent_id: materialized_id.clone(),
                initial_files: vec![
                    initial_file(
                        content_hash,
                        "/immutable-a",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/immutable-b",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/writable",
                        AgentFilePermissions::ReadWrite,
                        content.len() as u64,
                    ),
                ],
                file_loader: Arc::clone(&file_loader),
            })
            .await
            .unwrap();
        let path = filesystem.path().to_path_buf();
        let (backend, project_id) = match &filesystem.storage {
            AgentFilesystemStorage::Managed {
                backend,
                project_id,
                ..
            } => (Arc::clone(backend), *project_id),
            AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
        };
        let materialized_usage = filesystem.usage().await.unwrap().unwrap();
        assert!(materialized_usage.allocated_bytes >= 3 * 8192);
        assert!(materialized_usage.filesystem_objects >= 4);
        assert_eq!(std::fs::read(path.join("immutable-a")).unwrap(), content);
        assert_eq!(std::fs::read(path.join("immutable-b")).unwrap(), content);
        assert_eq!(std::fs::read(path.join("writable")).unwrap(), content);
        let immutable_a = File::open(path.join("immutable-a")).unwrap();
        let immutable_b = File::open(path.join("immutable-b")).unwrap();
        assert_eq!(backend.project_id(&immutable_a).unwrap(), Some(project_id));
        assert_eq!(backend.project_id(&immutable_b).unwrap(), Some(project_id));
        drop((immutable_a, immutable_b));

        filesystem
            .runtime()
            .update_initial_files(
                &file_loader,
                materialized_id.environment_id,
                &[
                    initial_file(
                        content_hash,
                        "/immutable-a",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/immutable-c",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/writable",
                        AgentFilePermissions::ReadWrite,
                        content.len() as u64,
                    ),
                ],
            )
            .await
            .unwrap();
        assert!(!path.join("immutable-b").exists());
        assert_eq!(std::fs::read(path.join("immutable-c")).unwrap(), content);
        let immutable_c = File::open(path.join("immutable-c")).unwrap();
        assert_eq!(backend.project_id(&immutable_c).unwrap(), Some(project_id));
        drop(immutable_c);

        filesystem.close_and_delete().await.unwrap();
        assert!(!path.exists());
        assert_eq!(
            backend.usage(project_id).unwrap(),
            AgentFilesystemUsage {
                allocated_bytes: 0,
                filesystem_objects: 0,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    async fn unmanaged_materialization_creates_distinct_owned_files() {
        use std::os::unix::fs::MetadataExt;

        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let content = b"shared initial content";
        let (file_loader, content_hash) =
            file_loader_with_content(id.environment_id, None, content).await;
        let filesystem = filesystems
            .create_fresh(CreateAgentFilesystem {
                agent_id: id,
                initial_files: vec![
                    initial_file(
                        content_hash,
                        "/first/immutable",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/second/immutable",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/writable",
                        AgentFilePermissions::ReadWrite,
                        content.len() as u64,
                    ),
                ],
                file_loader,
            })
            .await
            .unwrap();

        let first = filesystem.path().join("first/immutable");
        let second = filesystem.path().join("second/immutable");
        let writable = filesystem.path().join("writable");
        assert_eq!(std::fs::read(&first).unwrap(), content);
        assert_eq!(std::fs::read(&second).unwrap(), content);
        assert_eq!(std::fs::read(&writable).unwrap(), content);
        assert_ne!(
            first.metadata().unwrap().ino(),
            second.metadata().unwrap().ino()
        );
        assert_ne!(
            first.metadata().unwrap().ino(),
            writable.metadata().unwrap().ino()
        );
        assert!(filesystem.runtime().is_read_only(&first));
        assert!(filesystem.runtime().is_read_only(&second));
        assert!(!filesystem.runtime().is_read_only(&writable));
        assert!(
            filesystem
                .runtime()
                .is_read_only(&filesystem.path().join("first/../first/immutable"))
        );
        std::os::unix::fs::symlink(&first, filesystem.path().join("immutable-link")).unwrap();
        assert!(
            filesystem
                .runtime()
                .is_read_only(&filesystem.path().join("immutable-link"))
        );
        assert!(
            !filesystem
                .runtime()
                .is_read_only_path(&filesystem.path().join("immutable-link"), false,)
        );
        tokio::fs::write(&writable, b"changed").await.unwrap();

        let path = filesystem.path().to_path_buf();
        filesystem.close_and_delete().await.unwrap();
        assert!(!path.exists());
    }

    #[test]
    async fn failed_initial_file_update_preserves_current_files() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let content = b"initial content";
        let (file_loader, content_hash) =
            file_loader_with_content(id.environment_id, None, content).await;
        let filesystem = filesystems
            .create_fresh(CreateAgentFilesystem {
                agent_id: id.clone(),
                initial_files: vec![initial_file(
                    content_hash,
                    "/current",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                )],
                file_loader: Arc::clone(&file_loader),
            })
            .await
            .unwrap();

        let result = filesystem
            .runtime()
            .update_initial_files(
                &file_loader,
                id.environment_id,
                &[
                    initial_file(
                        content_hash,
                        "/new",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/invalid",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64 + 1,
                    ),
                ],
            )
            .await;

        assert!(result.is_err());
        let current = filesystem.path().join("current");
        assert_eq!(std::fs::read(&current).unwrap(), content);
        assert!(filesystem.runtime().is_read_only(&current));
        assert!(!filesystem.path().join("new").exists());
        assert!(!filesystem.path().join("invalid").exists());
        filesystem.close_and_delete().await.unwrap();
    }

    #[test]
    async fn initial_file_update_commits_staged_files_and_policy_together() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let content = b"initial content";
        let (file_loader, content_hash) =
            file_loader_with_content(id.environment_id, None, content).await;
        let filesystem = filesystems
            .create_fresh(CreateAgentFilesystem {
                agent_id: id.clone(),
                initial_files: vec![initial_file(
                    content_hash,
                    "/old",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                )],
                file_loader: Arc::clone(&file_loader),
            })
            .await
            .unwrap();

        filesystem
            .runtime()
            .update_initial_files(
                &file_loader,
                id.environment_id,
                &[
                    initial_file(
                        content_hash,
                        "/new",
                        AgentFilePermissions::ReadOnly,
                        content.len() as u64,
                    ),
                    initial_file(
                        content_hash,
                        "/writable",
                        AgentFilePermissions::ReadWrite,
                        content.len() as u64,
                    ),
                ],
            )
            .await
            .unwrap();

        let new = filesystem.path().join("new");
        let writable = filesystem.path().join("writable");
        assert!(!filesystem.path().join("old").exists());
        assert_eq!(std::fs::read(&new).unwrap(), content);
        assert_eq!(std::fs::read(&writable).unwrap(), content);
        assert!(filesystem.runtime().is_read_only(&new));
        assert!(!filesystem.runtime().is_read_only(&writable));
        filesystem.close_and_delete().await.unwrap();
    }

    #[test]
    async fn initial_file_updates_are_exclusive_with_filesystem_effects() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let effect = runtime.begin_effect().await.unwrap();
        let update_runtime = runtime.clone();
        let update =
            tokio::spawn(async move { update_runtime.begin_update_effect().await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!update.is_finished());

        drop(effect);
        let update = update.await.unwrap();
        let effect_runtime = runtime.clone();
        let next_effect = tokio::spawn(async move { effect_runtime.begin_effect().await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!next_effect.is_finished());

        drop(update);
        drop(next_effect.await.unwrap());
    }

    #[test]
    fn dropped_initial_file_transaction_restores_backups() {
        let root = tempfile::tempdir().unwrap();
        let live = root.path().join("live");
        let staged = root.path().join("staged");
        let backup = root.path().join("backups");
        std::fs::write(&live, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();
        std::fs::create_dir(&backup).unwrap();

        {
            let mut transaction = InitialFileUpdateTransaction::new(backup);
            transaction.back_up(&live).unwrap();
            transaction.install(&staged, &live).unwrap();
        }

        assert_eq!(std::fs::read(&live).unwrap(), b"old");
    }

    #[test]
    async fn initial_file_update_rejects_guest_file_collision() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let content = b"initial content";
        let (file_loader, content_hash) =
            file_loader_with_content(id.environment_id, None, content).await;
        let filesystem = filesystems
            .create_fresh(CreateAgentFilesystem {
                agent_id: id.clone(),
                initial_files: Vec::new(),
                file_loader: Arc::clone(&file_loader),
            })
            .await
            .unwrap();
        let collision = filesystem.path().join("collision");
        std::fs::write(&collision, b"guest data").unwrap();

        let result = filesystem
            .runtime()
            .update_initial_files(
                &file_loader,
                id.environment_id,
                &[initial_file(
                    content_hash,
                    "/collision",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                )],
            )
            .await;

        assert!(result.is_err());
        assert_eq!(std::fs::read(collision).unwrap(), b"guest data");
        filesystem.close_and_delete().await.unwrap();
    }

    #[test]
    async fn initial_file_update_preserves_guest_file_for_read_write_target() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let content = b"initial content";
        let (file_loader, content_hash) =
            file_loader_with_content(id.environment_id, None, content).await;
        let filesystem = filesystems
            .create_fresh(CreateAgentFilesystem {
                agent_id: id.clone(),
                initial_files: Vec::new(),
                file_loader: Arc::clone(&file_loader),
            })
            .await
            .unwrap();
        let collision = filesystem.path().join("collision");
        std::fs::write(&collision, b"guest data").unwrap();

        let update = filesystem
            .runtime()
            .update_initial_files(
                &file_loader,
                id.environment_id,
                &[initial_file(
                    content_hash,
                    "/collision",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                )],
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read(collision).unwrap(), b"guest data");
        drop(update);
        filesystem.close_and_delete().await.unwrap();
    }

    #[test]
    async fn deterministic_creation_removes_existing_garbage() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();

        let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
        assert_eq!(filesystem.usage().await.unwrap(), None);
        let path = filesystem.path().to_path_buf();
        tokio::fs::write(path.join("garbage"), b"old")
            .await
            .unwrap();
        drop(filesystem);
        tokio::fs::create_dir_all(&path).await.unwrap();
        tokio::fs::write(path.join("garbage"), b"old")
            .await
            .unwrap();

        let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
        assert!(!filesystem.path().join("garbage").exists());
        filesystem.close_and_delete().await.unwrap();
        assert!(!path.exists());
    }

    #[test]
    async fn seal_rejects_new_effects_without_waiting_for_existing_effects() {
        let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
        let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
        let runtime = filesystem.runtime();
        let effect = runtime.begin_effect().await.unwrap();

        filesystem.seal();
        assert!(runtime.begin_effect().await.is_err());
        assert!(filesystem.path().exists());
        drop(effect);
        filesystem.close_and_delete().await.unwrap();
    }

    #[test]
    async fn close_waits_for_an_existing_effect_before_deleting() {
        let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
        let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
        let path = filesystem.path().to_path_buf();
        let effect = filesystem.runtime().begin_effect().await.unwrap();

        let close = tokio::spawn(filesystem.close_and_delete());
        tokio::task::yield_now().await;
        assert!(!close.is_finished());
        assert!(path.exists());
        drop(effect);
        close.await.unwrap().unwrap();
        assert!(!path.exists());
    }

    #[test]
    async fn dropped_owner_defers_cleanup_and_retains_lifecycle_until_effects_finish() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
        let path = filesystem.path().to_path_buf();
        let effect = filesystem.runtime().begin_effect().await.unwrap();
        drop(filesystem);

        let replacement = tokio::spawn({
            let filesystems = filesystems.clone();
            let id = id.clone();
            async move { filesystems.create_owned_empty(&id).await }
        });
        tokio::task::yield_now().await;
        assert!(!replacement.is_finished());
        assert!(path.exists());

        drop(effect);
        let replacement = tokio::time::timeout(std::time::Duration::from_secs(5), replacement)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        replacement.close_and_delete().await.unwrap();
    }

    #[test]
    async fn deterministic_creation_is_exclusive_for_the_full_owner_lifetime() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let filesystems = AgentFilesystems::new(&settings).unwrap();
        let id = agent_id();
        let first = filesystems.create_owned_empty(&id).await.unwrap();
        tokio::fs::write(first.path().join("owned"), b"first")
            .await
            .unwrap();

        let second = tokio::spawn({
            let filesystems = filesystems.clone();
            let id = id.clone();
            async move { filesystems.create_owned_empty(&id).await }
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());
        assert!(first.path().join("owned").exists());

        first.close_and_delete().await.unwrap();
        let second = second.await.unwrap().unwrap();
        assert!(!second.path().join("owned").exists());
        second.close_and_delete().await.unwrap();
    }

    #[test]
    async fn append_effect_ends_at_native_completion_without_waiting_for_guest_polling() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let ready = Arc::new(tokio::sync::Semaphore::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
            ready: Arc::clone(&ready),
            cancelled,
        }));
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"first")).unwrap();

        assert!(stream.is_active());
        assert!(stream.write(Bytes::from_static(b"second")).is_err());
        let next_effect = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!next_effect.is_finished());

        ready.add_permits(1);
        let next_effect = next_effect.await.unwrap().unwrap();
        assert!(!stream.is_active());
        drop(next_effect);
    }

    #[test]
    async fn positioned_effect_does_not_wait_for_active_append() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let append = runtime.begin_append_effect().await.unwrap();

        let positioned = runtime.begin_effect().await.unwrap();

        drop(positioned);
        drop(append);
    }

    #[test]
    async fn cancelling_p2_stream_forwards_cancellation_and_releases_the_effect() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
            ready: Arc::new(tokio::sync::Semaphore::new(0)),
            cancelled: Arc::clone(&cancelled),
        }));
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"write")).unwrap();

        let next_append = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.begin_append_effect().await }
        });
        tokio::task::yield_now().await;
        assert!(!next_append.is_finished());

        stream.cancel().await;

        assert!(cancelled.load(Ordering::Acquire));
        assert!(!stream.is_active());
        assert!(next_append.await.unwrap().is_ok());
    }

    #[test]
    async fn dropping_p2_stream_requests_cancellation() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
            ready: Arc::new(tokio::sync::Semaphore::new(0)),
            cancelled: Arc::clone(&cancelled),
        }));
        stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
        stream.write(Bytes::from_static(b"write")).unwrap();

        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !cancelled.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stream drop did not request cancellation");
        assert!(runtime.begin_append_effect().await.is_ok());
    }
}
