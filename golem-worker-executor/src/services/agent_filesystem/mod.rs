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
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, FilesystemStorageConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::component::InitialAgentFile;
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
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Instant;
use tokio::sync::{Mutex, OwnedMutexGuard};

#[allow(
    dead_code,
    reason = "mutation classification is exposed for filesystem host adapters"
)]
mod failure;
mod initial_files;
mod mutation;
mod postcondition;
mod quota;

#[cfg(target_os = "linux")]
mod xfs;

#[allow(
    unused_imports,
    reason = "mutation classification is exposed for filesystem host adapters"
)]
pub(crate) use failure::{
    AgentFilesystemInvalidationCallback, AgentFilesystemPressureRecoveryCallback,
    AgentFilesystemRetryCallback, FILESYSTEM_MUTATION_MAX_ATTEMPTS,
    FILESYSTEM_MUTATION_RETRY_TIMEOUT, MutationDecision, MutationEffect, MutationFailure,
    MutationOperation, native_write_failure_effect, proven_write_progress_effect,
};
pub(crate) use mutation::{
    AgentFilesystemEffectAdmission, AgentFilesystemEffectLease, AgentFilesystemUpdateEffectLease,
    ClassifiedFileOutputStream, FilesystemStreamMode, NativeMutationGuestError, NativeOpenOptions,
    NativeOpenResult, classified_filesystem_stream_error_code, create_directory, hard_link, open,
    remove_directory, rename, resize_file, run_blocking_filesystem_mutation, set_descriptor_times,
    set_path_times, symlink, sync_descriptor, unlink_file, validate_descriptor_times,
    validate_directory_mutation, validate_open, validate_resize, validate_two_directory_mutation,
};
#[allow(
    unused_imports,
    reason = "shared probe vocabulary is consumed across host adapters and tests"
)]
pub(crate) use postcondition::{
    MutationPostcondition, ObjectIdentity, PathObjectType, PathState, RequestedTime, SymlinkState,
    TimesState, create_directory_postcondition, descriptor_state, descriptor_times,
    link_postcondition, open_postcondition, path_state, path_state_with_follow, path_times,
    remove_postcondition, rename_postcondition, resize_postcondition, same_object,
    same_optional_object, state_postcondition, symlink_postcondition, symlink_state,
    times_postcondition,
};
use quota::FilesystemLimitExceededCallback;
pub(crate) use quota::{
    AgentFilesystemStorageLimit, AgentFilesystemUsage, FilesystemCapacity,
    ResolvedAgentFilesystemLimits,
};

#[cfg(test)]
use initial_files::InitialFileUpdateTransaction;
#[cfg(test)]
use quota::FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION;

#[cfg(test)]
mod tests;

static LIFECYCLE_LOCKS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    OnceLock::new();

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

    pub(crate) fn resource_billing_transition(operation: &'static str) -> Self {
        Self::verification(operation, Path::new("<resource-meter>"))
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

    pub(crate) fn is_storage_exhaustion(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        })
    }

    fn is_terminal_failure(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::InvalidData
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::ReadOnlyFilesystem
            ) || is_terminal_storage_errno(source)
        })
    }
}

#[cfg(target_os = "linux")]
fn is_terminal_storage_errno(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(errno) if matches!(errno, libc::EIO | libc::ESTALE | libc::ENODEV))
}

#[cfg(not(target_os = "linux"))]
fn is_terminal_storage_errno(_error: &std::io::Error) -> bool {
    false
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
    filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig,
    pressure: FilesystemPressureConfig,
    #[cfg(target_os = "linux")]
    managed_xfs: Option<Arc<xfs::XfsBackend>>,
}

pub(crate) struct CreateAgentFilesystem {
    pub agent_id: OwnedAgentId,
    pub initial_files: Vec<InitialAgentFile>,
    pub file_loader: Arc<FileLoader>,
    pub resource_limits: Option<Arc<AtomicResourceEntry>>,
    pub limit_exceeded: Option<FilesystemLimitExceededCallback>,
}

impl AgentFilesystems {
    pub(crate) fn new(settings: &FilesystemStorageConfig) -> Result<Self, FilesystemStorageError> {
        if settings.deterministic_root_dir.is_some() && settings.managed_xfs_root_dir.is_some() {
            return Err(FilesystemStorageError::verification(
                "select exactly one filesystem storage backend",
                Path::new("<configuration>"),
            ));
        }

        if settings.managed_xfs_root_dir.is_some() {
            settings.filesystem_object_limit_policy.validate()?;
            settings.pressure.validate()?;
        }

        #[cfg(target_os = "linux")]
        let managed_xfs = match settings.managed_xfs_root_dir.as_deref() {
            Some(root) => {
                let backend = Arc::new(xfs::XfsBackend::new(root, &settings.cleanup_retry)?);
                settings
                    .pressure
                    .validate_capacity(backend.capacity().map_err(|error| {
                        FilesystemStorageError::io(
                            "validate managed XFS pressure capacity",
                            root,
                            error,
                        )
                    })?)?;
                Some(backend)
            }
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
            filesystem_object_limit_policy: settings.filesystem_object_limit_policy.clone(),
            pressure: settings.pressure.clone(),
            #[cfg(target_os = "linux")]
            managed_xfs,
        })
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
        let mut filesystem = self.create_owned_empty(&request.agent_id).await?;
        filesystem
            .runtime
            .set_limit_exceeded_callback(request.limit_exceeded);
        if let Some(resource_limits) = request.resource_limits {
            if let Err(error) = resource_limits
                .register_agent_filesystem(request.agent_id.clone(), filesystem.runtime())
                .await
            {
                return Err(rollback_owned_filesystem(filesystem, error).await);
            }
            filesystem.limit_registration = Some(AgentFilesystemLimitRegistration {
                resource_limits,
                agent_id: request.agent_id.clone(),
                runtime: filesystem.runtime(),
            });
        }
        if let Err(error) = filesystem
            .runtime
            .replace_initial_files(
                &request.file_loader,
                request.agent_id.environment_id,
                &request.initial_files,
            )
            .await
        {
            if error.is_storage_exhaustion() {
                filesystem.runtime.notify_limit_state(true).await;
            }
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
            runtime: AgentFilesystemRuntime::new(
                AgentFilesystemMaterializer::Managed {
                    root: path.clone(),
                    backend: Arc::clone(&backend),
                    project_id,
                },
                self.filesystem_object_limit_policy.clone(),
                self.pressure.clone(),
            ),
            cleanup_retry: self.cleanup_retry.clone(),
            storage: AgentFilesystemStorage::Managed {
                backend: Arc::clone(&backend),
                project_id,
                parent: Some(parent),
                root: Some(root),
            },
            lifecycle: Some(lifecycle),
            delete_on_drop: true,
            limit_registration: None,
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
            runtime: AgentFilesystemRuntime::new(
                AgentFilesystemMaterializer::Unmanaged { root: path.clone() },
                self.filesystem_object_limit_policy.clone(),
                self.pressure.clone(),
            ),
            cleanup_retry: self.cleanup_retry.clone(),
            storage: AgentFilesystemStorage::Unmanaged,
            lifecycle: Some(lifecycle),
            delete_on_drop: true,
            limit_registration: None,
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
            runtime: AgentFilesystemRuntime::new(
                AgentFilesystemMaterializer::Unmanaged { root: path.clone() },
                self.filesystem_object_limit_policy.clone(),
                self.pressure.clone(),
            ),
            cleanup_retry: self.cleanup_retry.clone(),
            storage: AgentFilesystemStorage::Unmanaged,
            lifecycle: Some(lifecycle),
            delete_on_drop: true,
            limit_registration: None,
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
    limit_registration: Option<AgentFilesystemLimitRegistration>,
}

struct AgentFilesystemLimitRegistration {
    resource_limits: Arc<AtomicResourceEntry>,
    agent_id: OwnedAgentId,
    runtime: AgentFilesystemRuntime,
}

impl Drop for AgentFilesystemLimitRegistration {
    fn drop(&mut self) {
        self.resource_limits
            .unregister_agent_filesystem(&self.agent_id, &self.runtime);
    }
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

    pub(crate) async fn close_and_delete(mut self) -> Result<(), FilesystemStorageError> {
        let started = Instant::now();
        self.limit_registration.take();
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
        // Preserve the fallback owner when the explicit attempt exhausts its retries.
        self.delete_on_drop = result.is_err();
        record_agent_filesystem_lifecycle("delete", result.is_ok(), started.elapsed());
        result
    }
}

impl Drop for AgentFilesystem {
    fn drop(&mut self) {
        if self.delete_on_drop {
            self.limit_registration.take();
            self.runtime.seal();
            let started = Instant::now();
            if self.runtime.has_active_effects() {
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
                    parent,
                    root,
                } => {
                    let backend = Arc::clone(backend);
                    let project_id = *project_id;
                    let parent = parent.take();
                    let root = root.take();
                    let cleanup_path = self.cleanup_path.clone();
                    let cleanup_retry = self.cleanup_retry.clone();
                    let lifecycle = self.lifecycle.take();
                    std::thread::spawn(move || {
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
                            tracing::error!(error = %error, "Failed to delete agent runtime filesystem during fallback cleanup");
                        }
                        drop((parent, lifecycle));
                    });
                    self.delete_on_drop = false;
                    return;
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
/// Cloneable handle to the synchronization and backend state shared by filesystem
/// adapters, completion tasks, quota enforcement, and usage sampling.
pub struct AgentFilesystemRuntime {
    inner: Arc<AgentFilesystemRuntimeInner>,
}

impl std::fmt::Debug for AgentFilesystemRuntime {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFilesystemRuntime")
            .finish_non_exhaustive()
    }
}

struct AgentFilesystemRuntimeInner {
    state: AtomicUsize,
    usage_sampling: AtomicBool,
    usage_effect_epoch: std::sync::atomic::AtomicU64,
    last_effect_completion_millis: std::sync::atomic::AtomicU64,
    drained: tokio::sync::Notify,
    admission_resumed: tokio::sync::Notify,
    append: Arc<Mutex<()>>,
    namespace: Arc<Mutex<()>>,
    operations: Arc<tokio::sync::RwLock<()>>,
    materializer: AgentFilesystemMaterializer,
    initial_files: std::sync::RwLock<HashMap<PathBuf, InitialAgentFile>>,
    filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig,
    pressure: FilesystemPressureConfig,
    applied_limits: std::sync::RwLock<Option<ResolvedAgentFilesystemLimits>>,
    limit_exceeded: std::sync::Mutex<Option<FilesystemLimitExceededCallback>>,
    #[allow(
        dead_code,
        reason = "runtime invalidation is exposed for filesystem host adapters"
    )]
    invalidation_notified: AtomicBool,
    #[allow(
        dead_code,
        reason = "runtime invalidation is exposed for filesystem host adapters"
    )]
    invalidated: std::sync::Mutex<Option<failure::AgentFilesystemInvalidationCallback>>,
    retry_permitted: std::sync::Mutex<Option<failure::AgentFilesystemRetryCallback>>,
    pressure_recovery: std::sync::Mutex<Option<failure::AgentFilesystemPressureRecoveryCallback>>,
    usage_observer: std::sync::Mutex<
        Option<Arc<dyn crate::services::agent_resource_billing::FilesystemUsageObserver>>,
    >,
    #[cfg(test)]
    failure_observations:
        std::sync::RwLock<Option<(Option<AgentFilesystemUsage>, FilesystemCapacity)>>,
    #[cfg(test)]
    usage_observation_fails: AtomicBool,
}

/// Backend identity used for filesystem materialization and authoritative observations.
/// Unmanaged filesystems deliberately have no authoritative per-agent usage source.
enum AgentFilesystemMaterializer {
    Unmanaged {
        root: PathBuf,
    },
    #[cfg(target_os = "linux")]
    Managed {
        root: PathBuf,
        backend: Arc<xfs::XfsBackend>,
        project_id: NonZeroU32,
    },
}

impl AgentFilesystemRuntime {
    fn new(
        materializer: AgentFilesystemMaterializer,
        filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig,
        pressure: FilesystemPressureConfig,
    ) -> Self {
        Self {
            inner: Arc::new(AgentFilesystemRuntimeInner {
                state: AtomicUsize::new(0),
                usage_sampling: AtomicBool::new(false),
                usage_effect_epoch: std::sync::atomic::AtomicU64::new(0),
                last_effect_completion_millis: std::sync::atomic::AtomicU64::new(0),
                drained: tokio::sync::Notify::new(),
                admission_resumed: tokio::sync::Notify::new(),
                append: Arc::new(Mutex::new(())),
                namespace: Arc::new(Mutex::new(())),
                operations: Arc::new(tokio::sync::RwLock::new(())),
                materializer,
                initial_files: std::sync::RwLock::new(HashMap::new()),
                filesystem_object_limit_policy,
                pressure,
                applied_limits: std::sync::RwLock::new(None),
                limit_exceeded: std::sync::Mutex::new(None),
                invalidation_notified: AtomicBool::new(false),
                invalidated: std::sync::Mutex::new(None),
                retry_permitted: std::sync::Mutex::new(None),
                pressure_recovery: std::sync::Mutex::new(None),
                usage_observer: std::sync::Mutex::new(None),
                #[cfg(test)]
                failure_observations: std::sync::RwLock::new(None),
                #[cfg(test)]
                usage_observation_fails: AtomicBool::new(false),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new_for_test_with_observations(
            None,
            None,
            FilesystemCapacity {
                total_bytes: 1,
                available_bytes: 1,
                total_filesystem_objects: 1,
                available_filesystem_objects: 1,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_observations(
        usage: Option<AgentFilesystemUsage>,
        limits: Option<ResolvedAgentFilesystemLimits>,
        capacity: FilesystemCapacity,
    ) -> Self {
        let runtime = Self::new(
            AgentFilesystemMaterializer::Unmanaged {
                root: PathBuf::from("<test>"),
            },
            FilesystemObjectLimitPolicyConfig::default(),
            FilesystemPressureConfig {
                minimum_available_bytes: 0,
                target_available_bytes: 0,
                minimum_available_filesystem_objects: 0,
                target_available_filesystem_objects: 0,
                ..FilesystemPressureConfig::default()
            },
        );
        *runtime
            .inner
            .applied_limits
            .write()
            .expect("agent filesystem applied-limit lock poisoned") = limits;
        *runtime
            .inner
            .failure_observations
            .write()
            .expect("agent filesystem test observation lock poisoned") = Some((usage, capacity));
        runtime
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_failed_observations() -> Self {
        let runtime = Self::new_for_test_with_capacity_observation_failure();
        runtime
            .inner
            .usage_observation_fails
            .store(true, Ordering::Release);
        runtime
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_capacity_observation_failure() -> Self {
        Self::new(
            AgentFilesystemMaterializer::Unmanaged {
                root: PathBuf::from("<missing-test-filesystem>"),
            },
            FilesystemObjectLimitPolicyConfig::default(),
            FilesystemPressureConfig {
                minimum_available_bytes: 0,
                target_available_bytes: 0,
                minimum_available_filesystem_objects: 0,
                target_available_filesystem_objects: 0,
                ..FilesystemPressureConfig::default()
            },
        )
    }
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
