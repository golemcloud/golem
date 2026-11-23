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

use super::{NativeRoot, NativeRootHandles, SandboxFilesystem};
use crate::filesystem::blocking::BlockingTask;
use crate::filesystem::errors::OperationContext;
use crate::filesystem::sandbox::{FilesystemStorageError, InstalledLimits};
use crate::filesystem::volume::platform::{install_project_limits, project_allocation};
use crate::filesystem::volume::{FilesystemAllocation, FilesystemLimits};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;
use std::sync::Arc;

/// Who enforces and measures the limits of a sandbox. A sandbox with a project identity charges
/// its project. A sandbox without one has no per-sandbox accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::filesystem::sandbox) enum QuotaAuthority {
    Unsupported,
    Project {
        project_id: NonZeroU32,
        filesystem_block_bytes: NonZeroU64,
    },
}

impl NativeRoot {
    pub(super) fn from_directory(path: PathBuf, directory: Arc<cap_std::fs::Dir>) -> Self {
        Self::from_handles(
            path,
            NativeRootHandles {
                directory,
                seed_flusher: None,
            },
        )
    }
}

impl SandboxFilesystem {
    pub(in crate::filesystem::sandbox) async fn observe_project_allocation(
        &self,
    ) -> Result<Option<FilesystemAllocation>, FilesystemStorageError> {
        let QuotaAuthority::Project { project_id, .. } = self.quota_authority else {
            return Ok(None);
        };
        let root = self.root().to_path_buf();
        let volume = self.volume.clone();
        BlockingTask::Spawn
            .run(move || project_allocation(&volume, project_id))
            .await
            .map_err(|error| {
                FilesystemStorageError::from(OperationContext::task_failure(
                    "observe managed XFS project allocation",
                    Some(&root),
                    error,
                ))
            })?
            .map(Some)
            .map_err(|error| {
                FilesystemStorageError::io(
                    "observe managed XFS project allocation",
                    Some(&root),
                    error,
                )
            })
    }

    pub(in crate::filesystem::sandbox) async fn install_storage_limits(
        &self,
        limits: FilesystemLimits,
    ) -> Result<InstalledLimits, FilesystemStorageError> {
        let QuotaAuthority::Project {
            project_id,
            filesystem_block_bytes,
        } = self.quota_authority
        else {
            return Err(FilesystemStorageError::from(
                OperationContext::verification(
                    "install limits without quota authority",
                    Some(self.root()),
                ),
            ));
        };
        let root = self.root().to_path_buf();
        let volume = self.volume.clone();
        let allocation = BlockingTask::Spawn
            .run(move || {
                install_project_limits(&volume, project_id, filesystem_block_bytes, limits)?;
                project_allocation(&volume, project_id)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::from(OperationContext::task_failure(
                    "install managed XFS project limits",
                    Some(&root),
                    error,
                ))
            })?
            .map_err(|error| {
                FilesystemStorageError::io("install managed XFS project limits", Some(&root), error)
            })?;
        Ok(InstalledLimits { limits, allocation })
    }

    #[cfg(test)]
    pub(in crate::filesystem::sandbox) fn name_mode_probe_count(&self) -> usize {
        self.name_mode_probe.count()
    }

    #[cfg(test)]
    pub(crate) fn project_id_for_test(&self) -> NonZeroU32 {
        match self.quota_authority {
            QuotaAuthority::Project { project_id, .. } => project_id,
            QuotaAuthority::Unsupported => panic!("sandbox filesystem has no project identity"),
        }
    }
}
