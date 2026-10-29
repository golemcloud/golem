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

use super::{QuotaAuthority, SandboxFilesystem};
use crate::blocking::BlockingTask;
use crate::sandbox_filesystem::FilesystemStorageError;
use crate::storage_volume::FilesystemAllocation;

#[cfg(target_os = "linux")]
use crate::storage_volume::{FilesystemVolume, project_allocation};
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct SandboxFilesystemAllocationObserver {
    pub(in crate::sandbox_filesystem) root: PathBuf,
    #[cfg(target_os = "linux")]
    pub(in crate::sandbox_filesystem) volume: FilesystemVolume,
    pub(in crate::sandbox_filesystem) quota_authority: QuotaAuthority,
}

impl SandboxFilesystemAllocationObserver {
    pub(super) fn new(filesystem: &SandboxFilesystem) -> Self {
        Self {
            root: filesystem.root().to_path_buf(),
            #[cfg(target_os = "linux")]
            volume: filesystem.volume.clone(),
            quota_authority: filesystem.quota_authority,
        }
    }

    pub(super) async fn observe(&self) -> Result<FilesystemAllocation, FilesystemStorageError> {
        let QuotaAuthority::Project {
            project_id: _project_id,
            ..
        } = self.quota_authority
        else {
            return Err(FilesystemStorageError::allocation_unsupported(&self.root));
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root.clone();
            let volume = self.volume.clone();
            BlockingTask::Spawn
                .run(move || project_allocation(&volume, _project_id))
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "observe managed XFS project allocation",
                        &root,
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed XFS project allocation",
                        &self.root,
                        error,
                    )
                })
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }
}
