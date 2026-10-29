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

use super::{NativeNameModeSource, SandboxFilesystem, directory_provisioning};
use crate::sandbox_filesystem::FilesystemStorageError;
use crate::storage_volume::FilesystemVolume;
use golem_common::model::RetryConfig;
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "linux")]
pub(in crate::sandbox_filesystem) mod managed;

/// Creates fresh isolated trees on storage already opened by the executor.
#[derive(Clone)]
pub(crate) struct SandboxFilesystemProvisioning {
    volume: FilesystemVolume,
    mode: SandboxFilesystemProvisioningMode,
}

#[derive(Clone)]
enum SandboxFilesystemProvisioningMode {
    Directories(directory_provisioning::DirectoryProvisioning),
    #[cfg(target_os = "linux")]
    ProjectQuotas(managed::ManagedSandboxProvisioning),
}

impl SandboxFilesystemProvisioning {
    pub(crate) fn on_volume(volume: FilesystemVolume, cleanup_retry: RetryConfig) -> Self {
        #[cfg(target_os = "linux")]
        if let Some(projects) = volume.project_quotas() {
            let mode = SandboxFilesystemProvisioningMode::ProjectQuotas(
                managed::ManagedSandboxProvisioning::new(projects.clone(), cleanup_retry),
            );
            return Self { volume, mode };
        }

        let (root, _) = volume.root_location();
        let name_mode = NativeNameModeSource::NativeDetection;
        #[cfg(target_os = "linux")]
        let name_mode = volume
            .validated_name_mode()
            .map(NativeNameModeSource::ValidatedXfs)
            .unwrap_or(name_mode);
        let mode = SandboxFilesystemProvisioningMode::Directories(
            directory_provisioning::DirectoryProvisioning::new(root, cleanup_retry, name_mode),
        );
        Self { volume, mode }
    }

    #[cfg(test)]
    pub(crate) fn new(
        storage: &crate::services::golem_config::FilesystemStorageMode,
        cleanup_retry: RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        let volume = FilesystemVolume::open(storage, cleanup_retry.clone())?;
        Ok(Self::on_volume(volume, cleanup_retry))
    }

    pub(crate) fn volume(&self) -> &FilesystemVolume {
        &self.volume
    }

    pub(crate) async fn create_fresh(
        &self,
        name: SandboxFilesystemName,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        match &self.mode {
            SandboxFilesystemProvisioningMode::Directories(directories) => {
                directories.create_fresh(self.volume.clone(), name).await
            }
            #[cfg(target_os = "linux")]
            SandboxFilesystemProvisioningMode::ProjectQuotas(managed) => {
                managed.create_fresh(self.volume.clone(), name).await
            }
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn project_allocation_for_test(
        &self,
        project_id: std::num::NonZeroU32,
    ) -> std::io::Result<crate::storage_volume::FilesystemAllocation> {
        crate::storage_volume::project_allocation(&self.volume, project_id)
    }
}

pub(crate) struct SandboxFilesystemName {
    components: [String; 3],
}

impl SandboxFilesystemName {
    pub(crate) fn new(
        environment: String,
        component: String,
        filesystem: String,
    ) -> Result<Self, FilesystemStorageError> {
        let components = [environment, component, filesystem];
        if components.iter().all(|component| {
            let path = Path::new(component);
            !component.starts_with('.')
                && matches!(
                    path.components().collect::<Vec<_>>().as_slice(),
                    [Component::Normal(_)]
                )
        }) {
            Ok(Self { components })
        } else {
            Err(FilesystemStorageError::verification_without_path(
                "validate sandbox filesystem name",
            ))
        }
    }

    pub(in crate::sandbox_filesystem) fn relative_path(&self) -> PathBuf {
        self.components.iter().collect()
    }

    #[cfg(target_os = "linux")]
    pub(in crate::sandbox_filesystem) fn components(&self) -> [&str; 3] {
        self.components.each_ref().map(String::as_str)
    }
}

#[cfg(test)]
mod tests;
