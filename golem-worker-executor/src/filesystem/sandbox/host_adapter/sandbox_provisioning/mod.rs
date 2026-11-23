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
use crate::filesystem::errors::OperationContext;
use crate::filesystem::sandbox::FilesystemStorageError;
use crate::filesystem::volume::FilesystemVolume;
#[cfg(test)]
use crate::services::golem_config::FilesystemStorageMode;
use golem_common::model::RetryConfig;
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "linux")]
pub(in crate::filesystem::sandbox) mod managed;

use platform::SandboxFilesystemProvisioningMode;

/// Creates fresh isolated trees on storage already opened by the executor.
#[derive(Clone)]
pub(crate) struct SandboxFilesystemProvisioning {
    volume: FilesystemVolume,
    mode: SandboxFilesystemProvisioningMode,
}

impl SandboxFilesystemProvisioning {
    pub(crate) fn on_volume(volume: FilesystemVolume, cleanup_retry: RetryConfig) -> Self {
        let mode = SandboxFilesystemProvisioningMode::new(&volume, cleanup_retry);
        Self { volume, mode }
    }

    #[cfg(test)]
    pub(crate) fn new(
        storage: &FilesystemStorageMode,
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
        self.mode.create_fresh(self.volume.clone(), name).await
    }
}

pub(crate) struct SandboxFilesystemName {
    components: [String; 3],
}

impl SandboxFilesystemName {
    pub(crate) fn new(
        environment: String,
        type_tag: String,
        filesystem: String,
    ) -> Result<Self, FilesystemStorageError> {
        let components = [environment, type_tag, filesystem];
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
            Err(FilesystemStorageError::from(
                OperationContext::verification("validate sandbox filesystem name", None),
            ))
        }
    }

    pub(in crate::filesystem::sandbox) fn relative_path(&self) -> PathBuf {
        self.components.iter().collect()
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    #[derive(Clone)]
    pub(super) enum SandboxFilesystemProvisioningMode {
        Directories(directory_provisioning::DirectoryProvisioning),
        ProjectQuotas(managed::ManagedSandboxProvisioning),
    }

    impl SandboxFilesystemProvisioningMode {
        pub(super) fn new(volume: &FilesystemVolume, cleanup_retry: RetryConfig) -> Self {
            if let Some(projects) = volume.project_quotas() {
                return Self::ProjectQuotas(managed::ManagedSandboxProvisioning::new(
                    projects.clone(),
                    cleanup_retry,
                ));
            }
            let (root, _) = volume.root_location();
            let name_mode = volume
                .validated_name_mode()
                .map(NativeNameModeSource::ValidatedXfs)
                .unwrap_or(NativeNameModeSource::NativeDetection);
            Self::Directories(directory_provisioning::DirectoryProvisioning::new(
                root,
                cleanup_retry,
                name_mode,
            ))
        }

        pub(super) async fn create_fresh(
            &self,
            volume: FilesystemVolume,
            name: SandboxFilesystemName,
        ) -> Result<SandboxFilesystem, FilesystemStorageError> {
            match self {
                Self::Directories(directories) => directories.create_fresh(volume, name).await,
                Self::ProjectQuotas(managed) => managed.create_fresh(volume, name).await,
            }
        }
    }

    impl SandboxFilesystemName {
        pub(in crate::filesystem::sandbox) fn components(&self) -> [&str; 3] {
            self.components.each_ref().map(String::as_str)
        }
    }

    #[cfg(test)]
    impl SandboxFilesystemProvisioning {
        pub(crate) fn project_allocation_for_test(
            &self,
            project_id: std::num::NonZeroU32,
        ) -> std::io::Result<crate::filesystem::volume::FilesystemAllocation> {
            crate::filesystem::volume::platform::project_allocation(&self.volume, project_id)
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    #[derive(Clone)]
    pub(super) struct SandboxFilesystemProvisioningMode(
        directory_provisioning::DirectoryProvisioning,
    );

    impl SandboxFilesystemProvisioningMode {
        pub(super) fn new(volume: &FilesystemVolume, cleanup_retry: RetryConfig) -> Self {
            let (root, _) = volume.root_location();
            Self(directory_provisioning::DirectoryProvisioning::new(
                root,
                cleanup_retry,
                NativeNameModeSource::NativeDetection,
            ))
        }

        pub(super) async fn create_fresh(
            &self,
            volume: FilesystemVolume,
            name: SandboxFilesystemName,
        ) -> Result<SandboxFilesystem, FilesystemStorageError> {
            self.0.create_fresh(volume, name).await
        }
    }
}

#[cfg(test)]
mod tests;
