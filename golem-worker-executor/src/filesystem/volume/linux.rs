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

use super::{FilesystemSpace, FilesystemVolume, FilesystemVolumeMode, StorageVolumeError, xfs};
use crate::filesystem::errors::OperationContext;
use golem_common::model::RetryConfig;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

pub(crate) use super::xfs::{
    ProjectQuotas, ValidatedXfsNameMode, assign_project, clone_file_contents,
    clone_file_contents_into_project, install_project_limits, project_allocation, stale_project,
};

#[cfg(test)]
mod test_exports {
    pub(crate) use super::super::xfs::{
        cow_extent_size_hint, file_project_id, get_fsxattr, has_project_identity,
        in_process_xfs_quotad, set_fsxattr, validated_xfs_name_mode_for_test,
    };
}
#[cfg(test)]
pub(crate) use test_exports::*;

#[derive(Default)]
pub(super) struct PlatformState {
    projects: Option<ProjectQuotas>,
    name_mode: Option<ValidatedXfsNameMode>,
}

#[derive(Clone)]
pub(super) struct CopyOnWrite {
    root: Arc<File>,
    identity: FilesystemIdentity,
    project_quotas: Option<Arc<golem_xfs_quotad::XfsQuotadClient>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemIdentity {
    pub(crate) device: u64,
}

impl CopyOnWrite {
    pub(super) fn root(&self) -> Option<&Arc<File>> {
        Some(&self.root)
    }

    pub(super) fn observe_space(&self) -> std::io::Result<FilesystemSpace> {
        xfs::observe_space(&self.root, self.identity)
    }
}

impl FilesystemVolume {
    pub(super) fn open_managed(
        root: &Path,
        socket: &Path,
        retry: &RetryConfig,
    ) -> Result<Self, StorageVolumeError> {
        let projects = ProjectQuotas::new(xfs::XfsRoot::open(root)?, socket, retry)?;
        let mut volume = Self::copy_on_write(
            Arc::clone(&projects.root_fd),
            projects.validated_name_mode.identity,
        )
        .with_project_quotas(Arc::clone(&projects.quotas));
        let state = volume.unique();
        state.root = Some(Arc::clone(projects.root()));
        state.platform.name_mode = Some(projects.validated_name_mode);
        state.platform.projects = Some(projects);
        Ok(volume)
    }

    pub(super) fn open_reflink(root: &Path) -> Result<Self, StorageVolumeError> {
        let (mut volume, root, name_mode) = xfs::bind_reflink(xfs::XfsRoot::open(root)?)?;
        let state = volume.unique();
        state.root = Some(root);
        state.platform.name_mode = Some(name_mode);
        Ok(volume)
    }

    pub(super) fn open_apfs(root: &Path) -> Result<Self, StorageVolumeError> {
        Err(StorageVolumeError::from(OperationContext::verification(
            "initialize APFS storage on a non-macOS platform",
            Some(root),
        )))
    }

    pub(crate) fn copy_on_write(root: Arc<File>, identity: FilesystemIdentity) -> Self {
        Self::with_copy_on_write(CopyOnWrite {
            root,
            identity,
            project_quotas: None,
        })
    }

    /// The volume with the quota helper `quotas`, which serves its project quotas.
    fn with_project_quotas(mut self, quotas: Arc<golem_xfs_quotad::XfsQuotadClient>) -> Self {
        if let FilesystemVolumeMode::CopyOnWrite(mode) = &mut self.unique().mode {
            mode.project_quotas = Some(quotas);
        }
        self
    }

    pub(super) fn quota_client(&self) -> Option<&golem_xfs_quotad::XfsQuotadClient> {
        match &self.state.mode {
            FilesystemVolumeMode::CopyOnWrite(mode) => mode.project_quotas.as_deref(),
            FilesystemVolumeMode::UnmanagedDevelopment => None,
        }
    }

    pub(crate) fn project_quotas(&self) -> Option<&ProjectQuotas> {
        self.state.platform.projects.as_ref()
    }

    pub(crate) fn validated_name_mode(&self) -> Option<ValidatedXfsNameMode> {
        self.state.platform.name_mode
    }
}

impl PlatformState {
    pub(super) fn verify_host_directory(&self, path: &Path) -> Result<(), StorageVolumeError> {
        xfs::verify_host_directory_has_no_project(path)
    }
}

/// Flushes pending changes on the filesystem containing `root`.
pub(crate) fn sync_volume(root: &Path) -> std::io::Result<()> {
    rustix::fs::syncfs(&File::open(root)?).map_err(std::io::Error::from)
}

#[cfg(test)]
mod test_support {
    use super::*;
    use std::os::fd::AsRawFd;

    impl FilesystemVolume {
        pub(crate) fn descriptor_rooted_for_test(root: Arc<File>) -> Self {
            let path = Arc::from(Path::new(&format!("/proc/self/fd/{}", root.as_raw_fd())));
            let mut volume = Self::copy_on_write(root, FilesystemIdentity { device: 0 });
            volume.unique().root = Some(path);
            volume
        }
    }

    pub(crate) fn copy_on_write_volume_for_test(root: Arc<File>) -> FilesystemVolume {
        FilesystemVolume::copy_on_write(root, FilesystemIdentity { device: 1 })
    }
}
#[cfg(test)]
pub(crate) use test_support::copy_on_write_volume_for_test;
