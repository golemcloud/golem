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

pub(crate) use platform::SandboxFilesystemAllocationObserver;

#[cfg(target_os = "linux")]
mod platform {
    use super::super::{QuotaAuthority, SandboxFilesystem};
    use crate::filesystem::blocking::BlockingTask;
    use crate::filesystem::errors::OperationContext;
    use crate::filesystem::sandbox::FilesystemStorageError;
    use crate::filesystem::volume::platform::project_allocation;
    use crate::filesystem::volume::{FilesystemAllocation, FilesystemVolume};
    use std::path::PathBuf;

    #[derive(Clone)]
    pub(crate) struct SandboxFilesystemAllocationObserver {
        pub(in crate::filesystem::sandbox) root: PathBuf,
        pub(in crate::filesystem::sandbox) volume: FilesystemVolume,
        pub(in crate::filesystem::sandbox) quota_authority: QuotaAuthority,
    }

    impl SandboxFilesystemAllocationObserver {
        pub(in crate::filesystem::sandbox::host_adapter) fn new(
            filesystem: &SandboxFilesystem,
        ) -> Self {
            Self {
                root: filesystem.root().to_path_buf(),
                volume: filesystem.volume.clone(),
                quota_authority: filesystem.quota_authority,
            }
        }

        pub(in crate::filesystem::sandbox::host_adapter) async fn observe(
            &self,
        ) -> Result<FilesystemAllocation, FilesystemStorageError> {
            let QuotaAuthority::Project { project_id, .. } = self.quota_authority else {
                return Err(FilesystemStorageError::allocation_unsupported(Some(
                    &self.root,
                )));
            };
            let root = self.root.clone();
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
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed XFS project allocation",
                        Some(&self.root),
                        error,
                    )
                })
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::super::{QuotaAuthority, SandboxFilesystem};
    use crate::filesystem::sandbox::FilesystemStorageError;
    use crate::filesystem::volume::FilesystemAllocation;
    use std::path::PathBuf;

    #[derive(Clone)]
    pub(crate) struct SandboxFilesystemAllocationObserver {
        pub(in crate::filesystem::sandbox) root: PathBuf,
        pub(in crate::filesystem::sandbox) quota_authority: QuotaAuthority,
    }

    impl SandboxFilesystemAllocationObserver {
        pub(in crate::filesystem::sandbox::host_adapter) fn new(
            filesystem: &SandboxFilesystem,
        ) -> Self {
            Self {
                root: filesystem.root().to_path_buf(),
                quota_authority: filesystem.quota_authority,
            }
        }

        pub(in crate::filesystem::sandbox::host_adapter) async fn observe(
            &self,
        ) -> Result<FilesystemAllocation, FilesystemStorageError> {
            match self.quota_authority {
                QuotaAuthority::Unsupported => Err(FilesystemStorageError::allocation_unsupported(
                    Some(&self.root),
                )),
                QuotaAuthority::Project { .. } => {
                    unreachable!("managed XFS is unavailable on this platform")
                }
            }
        }
    }
}
