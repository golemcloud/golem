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

use super::super::sandbox_cleanup::{
    LeaseState, NativeCleanup, acquire_filesystem_lease, remove_and_verify,
    remove_and_verify_blocking, rollback_created_filesystem, rollback_creation,
    verify_fresh_open_directory,
};
use super::super::{NativeNameModeSource, NativeRoot, QuotaAuthority, SandboxFilesystem};
use super::SandboxFilesystemName;
use crate::filesystem::blocking::BlockingTask;
use crate::filesystem::errors::OperationContext;
use crate::filesystem::sandbox::FilesystemStorageError;
use crate::filesystem::volume::{FilesystemVolume, ProjectQuotas, assign_project, stale_project};
use crate::metrics::agent_filesystem::{self, ProjectCleanup as ProjectCleanupMetric};
use golem_common::model::RetryConfig;
use golem_common::retries::RetryState;
use rustix::fs::{Mode, OFlags, mkdirat, openat};
use std::fs::File;
use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;

#[derive(Clone)]
pub(in crate::filesystem::sandbox) struct ManagedSandboxProvisioning {
    projects: ProjectQuotas,
    cleanup_retry: RetryConfig,
}

impl ManagedSandboxProvisioning {
    pub(super) fn new(projects: ProjectQuotas, cleanup_retry: RetryConfig) -> Self {
        Self {
            projects,
            cleanup_retry,
        }
    }

    pub(in crate::filesystem::sandbox) async fn create_fresh(
        &self,
        volume: FilesystemVolume,
        name: SandboxFilesystemName,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        let owner = name.relative_path();
        let lifecycle = acquire_filesystem_lease(&self.projects.root().join(&owner)).await;
        let provisioning = self.clone();
        let error_path = self.projects.root().join(&owner);
        tokio::spawn(async move { provisioning.provision(volume, name, lifecycle).await })
            .await
            .map_err(|error| {
                FilesystemStorageError::io(
                    "provision managed XFS sandbox filesystem",
                    Some(&error_path),
                    std::io::Error::other(error),
                )
            })?
    }

    async fn provision(
        &self,
        volume: FilesystemVolume,
        name: SandboxFilesystemName,
        lifecycle: OwnedMutexGuard<()>,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        let [environment, component, filesystem_name] = name.components();
        let owner = name.relative_path();
        let mut lifecycle = Some(lifecycle);
        let parent = self
            .open_filesystem_parent(environment, component)
            .map_err(|error| {
                FilesystemStorageError::io(
                    "open managed runtime directory parent",
                    Some(
                        &self
                            .projects
                            .root()
                            .join(owner.parent().expect("owner must have a parent")),
                    ),
                    error,
                )
            })?;
        let cleanup_path =
            PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(filesystem_name);

        let disk_project = match tokio::fs::symlink_metadata(&cleanup_path).await {
            Ok(metadata) if !metadata.file_type().is_symlink() => {
                let provisioning = self.clone();
                let existing_entry =
                    provisioning
                        .open_entry(&parent, filesystem_name)
                        .map_err(|error| {
                            FilesystemStorageError::cleanup_io(
                                "open stale managed XFS runtime path",
                                Some(&cleanup_path),
                                error,
                            )
                        })?;
                BlockingTask::Spawn
                    .run(move || provisioning.projects.project_id(&existing_entry))
                    .await
                    .map_err(|error| {
                        FilesystemStorageError::from(OperationContext::task_failure(
                            "inspect stale managed XFS project",
                            Some(&cleanup_path),
                            error,
                        ))
                    })?
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "inspect stale managed XFS project",
                            Some(&cleanup_path),
                            error,
                        )
                    })?
            }
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(FilesystemStorageError::cleanup_io(
                    "inspect stale managed XFS runtime path",
                    Some(&cleanup_path),
                    error,
                ));
            }
        };
        let reserved_project = self.projects.reserved_project(&owner);
        let Ok(stale_project) = stale_project(disk_project, reserved_project) else {
            return Err(FilesystemStorageError::cleanup_verification(
                "match stale managed XFS path and reserved project",
                Some(&cleanup_path),
            ));
        };

        let mut stale_cleanup = if let Some(project_id) = stale_project {
            let cleanup_parent = parent.try_clone().map_err(|error| {
                FilesystemStorageError::cleanup_io(
                    "retain managed XFS runtime directory parent",
                    Some(&cleanup_path),
                    error,
                )
            })?;
            self.projects
                .reserve_existing_project(project_id, &owner)
                .map_err(|error| {
                    FilesystemStorageError::cleanup_io(
                        "reserve stale managed XFS project for cleanup",
                        Some(&cleanup_path),
                        error,
                    )
                })?;
            Some(StaleProjectCleanup::new(
                ProjectCleanup::new(
                    self.projects.clone(),
                    project_id,
                    cleanup_path.clone(),
                    self.cleanup_retry.clone(),
                ),
                cleanup_parent,
                lifecycle
                    .take()
                    .expect("managed XFS lifecycle owner must exist"),
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
        if stale_project.is_some() {
            stale_cleanup
                .as_ref()
                .expect("stale project cleanup owner must exist")
                .project
                .finish()
                .await?;
            agent_filesystem::record_project_stale_reclaim();
            lifecycle = Some(
                stale_cleanup
                    .as_mut()
                    .expect("stale project cleanup owner must exist")
                    .disarm(),
            );
        }

        if let Err(error) = tokio::fs::create_dir(&cleanup_path).await {
            return Err(rollback_creation(
                &cleanup_path,
                FilesystemStorageError::io(
                    "create fresh managed runtime directory",
                    Some(&cleanup_path),
                    error,
                ),
                &self.cleanup_retry,
            )
            .await);
        }

        let project_id = self.projects.reserve_project(&owner).map_err(|error| {
            FilesystemStorageError::io("allocate managed XFS project", Some(&cleanup_path), error)
        });
        let project_id = match project_id {
            Ok(project_id) => project_id,
            Err(error) => {
                return Err(rollback_creation(&cleanup_path, error, &self.cleanup_retry).await);
            }
        };
        let root_fd = match open_directory(&parent, filesystem_name) {
            Ok(root) => root,
            Err(error) => {
                self.projects.release_project(project_id);
                return Err(rollback_creation(
                    &cleanup_path,
                    FilesystemStorageError::io(
                        "open fresh managed runtime directory",
                        Some(&cleanup_path),
                        error,
                    ),
                    &self.cleanup_retry,
                )
                .await);
            }
        };
        let root = PathBuf::from(format!("/proc/self/fd/{}", root_fd.as_raw_fd()));
        let assignment_result = assign_project(&root_fd, project_id).map_err(|error| {
            FilesystemStorageError::io("assign managed XFS project", Some(&root), error)
        });
        let native_root =
            NativeRoot::from_directory(root, Arc::new(cap_std::fs::Dir::from_std_file(root_fd)));
        let lease = LeaseState {
            lifecycle: lifecycle.expect("managed XFS lifecycle owner must exist"),
            cleanup: NativeCleanup::Managed(Box::new(ManagedProjectCleanup {
                project: ProjectCleanup::new(
                    self.projects.clone(),
                    project_id,
                    cleanup_path,
                    self.cleanup_retry.clone(),
                ),
                _parent: parent,
            })),
        };
        let created = SandboxFilesystem::new(
            native_root,
            lease,
            volume,
            QuotaAuthority::Project {
                project_id,
                filesystem_block_bytes: self.projects.filesystem_block_bytes(),
            },
            NativeNameModeSource::ValidatedXfs(self.projects.validated_name_mode()),
        );
        if let Err(error) = assignment_result {
            return Err(rollback_created_filesystem(created, error).await);
        }
        if let Err(error) = verify_fresh_open_directory(created.root()).await {
            return Err(rollback_created_filesystem(created, error).await);
        }

        Ok(created)
    }

    fn open_filesystem_parent(&self, environment: &str, component: &str) -> std::io::Result<File> {
        let environment = open_or_create_directory(self.projects.root_directory(), environment)?;
        open_or_create_directory(&environment, component)
    }

    fn open_entry(&self, parent: &File, name: &str) -> std::io::Result<File> {
        openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(std::io::Error::from)
    }
}

fn open_directory(parent: &File, name: &str) -> std::io::Result<File> {
    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(std::io::Error::from)
}

fn open_or_create_directory(parent: &File, name: &str) -> std::io::Result<File> {
    match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    open_directory(parent, name)
}

pub(in crate::filesystem::sandbox) struct ManagedProjectCleanup {
    pub(in crate::filesystem::sandbox) project: ProjectCleanup,
    pub(in crate::filesystem::sandbox) _parent: File,
}

impl ManagedProjectCleanup {
    pub(in crate::filesystem::sandbox) async fn delete(
        &mut self,
    ) -> Result<(), FilesystemStorageError> {
        let deleted = async {
            remove_and_verify(
                &self.project.path,
                "delete managed XFS runtime directory",
                &self.project.cleanup_retry,
            )
            .await?;
            self.project.finish().await
        }
        .await;
        if deleted.is_err() {
            agent_filesystem::record_project_cleanup_failure(ProjectCleanupMetric::Delete);
        }
        deleted
    }

    pub(in crate::filesystem::sandbox) fn delete_blocking(
        &mut self,
    ) -> Result<(), FilesystemStorageError> {
        let deleted = self.project.remove_and_finish_blocking();
        if deleted.is_err() {
            agent_filesystem::record_project_cleanup_failure(ProjectCleanupMetric::Drop);
        }
        deleted
    }
}

pub(in crate::filesystem::sandbox) struct StaleProjectCleanup {
    pub(in crate::filesystem::sandbox) project: ProjectCleanup,
    pub(in crate::filesystem::sandbox) parent: Option<File>,
    pub(in crate::filesystem::sandbox) lifecycle: Option<OwnedMutexGuard<()>>,
    pub(in crate::filesystem::sandbox) armed: bool,
}

impl StaleProjectCleanup {
    fn new(project: ProjectCleanup, parent: File, lifecycle: OwnedMutexGuard<()>) -> Self {
        Self {
            project,
            parent: Some(parent),
            lifecycle: Some(lifecycle),
            armed: true,
        }
    }

    fn disarm(&mut self) -> OwnedMutexGuard<()> {
        self.armed = false;
        self.parent.take();
        self.lifecycle
            .take()
            .expect("managed XFS stale cleanup lifecycle owner must exist")
    }
}

impl Drop for StaleProjectCleanup {
    fn drop(&mut self) {
        if self.armed {
            let project = self.project.clone();
            let parent = self.parent.take();
            let lifecycle = self.lifecycle.take();
            std::thread::spawn(move || {
                if let Err(error) = project.remove_and_finish_blocking() {
                    agent_filesystem::record_project_cleanup_failure(ProjectCleanupMetric::Stale);
                    tracing::error!(error = %error, "Failed to clean reserved managed XFS project");
                }
                drop((parent, lifecycle));
            });
        }
    }
}

#[derive(Clone)]
pub(in crate::filesystem::sandbox) struct ProjectCleanup {
    pub(in crate::filesystem::sandbox) provisioning: ProjectQuotas,
    pub(in crate::filesystem::sandbox) project_id: NonZeroU32,
    pub(in crate::filesystem::sandbox) path: PathBuf,
    pub(in crate::filesystem::sandbox) cleanup_retry: RetryConfig,
}

impl ProjectCleanup {
    fn new(
        provisioning: ProjectQuotas,
        project_id: NonZeroU32,
        path: PathBuf,
        cleanup_retry: RetryConfig,
    ) -> Self {
        Self {
            provisioning,
            project_id,
            path,
            cleanup_retry,
        }
    }

    async fn finish(&self) -> Result<(), FilesystemStorageError> {
        let mut retry = RetryState::new(&self.cleanup_retry);
        loop {
            retry.start_attempt();
            let provisioning = self.provisioning.clone();
            let project_id = self.project_id;
            let attempt = BlockingTask::Spawn
                .run(move || provisioning.finish_project_cleanup(project_id))
                .await
                .map_err(|error| {
                    let failure = FilesystemStorageError::from(OperationContext::task_failure(
                        "verify and clear managed XFS project",
                        Some(&self.path),
                        error,
                    ));
                    failure.during_cleanup()
                })?;
            match attempt {
                Ok(()) => {
                    self.provisioning.release_project(self.project_id);
                    return Ok(());
                }
                Err(error) => {
                    if !retry.failed_attempt().await {
                        return Err(FilesystemStorageError::cleanup_io(
                            "verify and clear managed XFS project",
                            Some(&self.path),
                            error,
                        ));
                    }
                }
            }
        }
    }

    fn remove_and_finish_blocking(&self) -> Result<(), FilesystemStorageError> {
        remove_and_verify_blocking(&self.path, "delete managed XFS runtime directory")?;
        let attempts = self.cleanup_retry.max_attempts.max(1);
        let mut delay = self.cleanup_retry.min_delay;
        for attempt in 1..=attempts {
            match self.provisioning.finish_project_cleanup(self.project_id) {
                Ok(()) => {
                    self.provisioning.release_project(self.project_id);
                    return Ok(());
                }
                Err(error) if attempt == attempts => {
                    return Err(FilesystemStorageError::cleanup_io(
                        "verify and clear managed XFS project",
                        Some(&self.path),
                        error,
                    ));
                }
                Err(_) => {
                    std::thread::sleep(delay);
                    delay = delay
                        .mul_f64(self.cleanup_retry.multiplier)
                        .min(self.cleanup_retry.max_delay);
                }
            }
        }
        unreachable!("managed XFS cleanup always performs at least one attempt")
    }
}

#[cfg(test)]
mod tests;
