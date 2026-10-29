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

//! Executor-wide storage ownership, capabilities and native operations. Sandboxes share this
//! storage, but own their confined trees and their creation and deletion separately.

mod errors;
use crate::operation_failure::OperationContext;

use crate::blocking::BlockingTask;
use crate::services::golem_config::FilesystemStorageMode;
pub(crate) use errors::StorageVolumeError;
use golem_common::model::RetryConfig;
use std::fmt::Debug;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod apfs;
#[cfg(target_os = "linux")]
mod xfs;
#[cfg(target_os = "macos")]
pub(crate) use apfs::clone_file_at;
#[cfg(all(test, target_os = "linux"))]
pub(crate) use xfs::in_process_xfs_quotad;
#[cfg(target_os = "linux")]
pub(crate) use xfs::{
    ProjectQuotas, ValidatedXfsNameMode, install_project_limits, project_allocation,
};
#[cfg(target_os = "linux")]
pub(crate) use xfs::{
    assign_project, clone_file_contents, clone_file_contents_into_project, stale_project,
};
#[cfg(all(test, target_os = "linux"))]
pub(crate) use xfs::{
    cow_extent_size_hint, file_project_id, get_fsxattr, has_project_identity, set_fsxattr,
    validated_xfs_name_mode_for_test,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemAllocation {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemLimits {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
}

/// How the selected storage accounts for each agent's files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentAccounting {
    ProjectQuotas,
    Development,
    Unaccounted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostDirectoryCheck {
    None,
    NoXfsProject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeStorageProfile {
    KnownLocal,
    Unknown,
}

#[derive(Clone)]
pub(crate) struct FilesystemVolume {
    state: Arc<VolumeState>,
}

struct VolumeState {
    mode: FilesystemVolumeMode,
    root: Option<Arc<Path>>,
    accounting: AgentAccounting,
    host_directory_check: HostDirectoryCheck,
    #[cfg(target_os = "linux")]
    projects: Option<ProjectQuotas>,
    #[cfg(target_os = "linux")]
    name_mode: Option<ValidatedXfsNameMode>,
}

#[derive(Clone)]
enum FilesystemVolumeMode {
    UnmanagedDevelopment,
    /// The checked XFS root on Linux, or the probed APFS development root on macOS.
    CopyOnWrite {
        root: Arc<File>,
        #[cfg(target_os = "linux")]
        identity: FilesystemIdentity,
        /// The quota helper of a managed XFS volume.
        #[cfg(target_os = "linux")]
        project_quotas: Option<Arc<golem_xfs_quotad::XfsQuotadClient>>,
    },
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemIdentity {
    pub(crate) device: u64,
}

impl FilesystemVolume {
    /// Opens and validates executor storage once. Sandboxes and capacity observers share the
    /// returned volume; its descriptors retain the storage root and any exclusive root lock.
    pub(crate) fn open(
        storage: &FilesystemStorageMode,
        cleanup_retry: RetryConfig,
    ) -> Result<Self, StorageVolumeError> {
        let mut volume = match storage {
            FilesystemStorageMode::Temporary => Self::unmanaged_development(),
            FilesystemStorageMode::Directory { root } => {
                let mut volume = Self::unmanaged_development();
                volume.unique().root = Some(Arc::from(&**root));
                volume
            }
            FilesystemStorageMode::ManagedXfs {
                root,
                xfs_quotad_socket,
            } => Self::open_managed(root, xfs_quotad_socket, &cleanup_retry)?,
            FilesystemStorageMode::ReflinkXfs { root } => Self::open_reflink(root)?,
            FilesystemStorageMode::Apfs { root } => Self::open_apfs(root)?,
        };
        let state = volume.unique();
        (state.accounting, state.host_directory_check) = storage_facts(storage);
        Ok(volume)
    }

    #[cfg(target_os = "linux")]
    fn open_managed(
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
        state.name_mode = Some(projects.validated_name_mode);
        state.projects = Some(projects);
        Ok(volume)
    }

    #[cfg(not(target_os = "linux"))]
    fn open_managed(
        root: &Path,
        _socket: &Path,
        _retry: &RetryConfig,
    ) -> Result<Self, StorageVolumeError> {
        Err(StorageVolumeError::from(OperationContext::verification(
            "initialize XFS storage on a non-Linux platform",
            Some(root),
        )))
    }

    #[cfg(target_os = "linux")]
    fn open_reflink(root: &Path) -> Result<Self, StorageVolumeError> {
        let (mut volume, root, name_mode) = xfs::bind_reflink(xfs::XfsRoot::open(root)?)?;
        let state = volume.unique();
        state.root = Some(root);
        state.name_mode = Some(name_mode);
        Ok(volume)
    }

    #[cfg(not(target_os = "linux"))]
    fn open_reflink(root: &Path) -> Result<Self, StorageVolumeError> {
        Err(StorageVolumeError::from(OperationContext::verification(
            "initialize XFS storage on a non-Linux platform",
            Some(root),
        )))
    }

    #[cfg(target_os = "macos")]
    fn open_apfs(root: &Path) -> Result<Self, StorageVolumeError> {
        apfs::bind(root)
    }

    #[cfg(not(target_os = "macos"))]
    fn open_apfs(root: &Path) -> Result<Self, StorageVolumeError> {
        Err(StorageVolumeError::from(OperationContext::verification(
            "initialize APFS storage on a non-macOS platform",
            Some(root),
        )))
    }

    pub(crate) fn unmanaged_development() -> Self {
        Self {
            state: Arc::new(VolumeState {
                mode: FilesystemVolumeMode::UnmanagedDevelopment,
                root: None,
                accounting: AgentAccounting::Development,
                host_directory_check: HostDirectoryCheck::None,
                #[cfg(target_os = "linux")]
                projects: None,
                #[cfg(target_os = "linux")]
                name_mode: None,
            }),
        }
    }

    fn unique(&mut self) -> &mut VolumeState {
        Arc::get_mut(&mut self.state).expect("volume construction must precede sharing")
    }

    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(crate) fn copy_on_write(
        root: Arc<File>,
        #[cfg(target_os = "linux")] identity: FilesystemIdentity,
    ) -> Self {
        let mut volume = Self::unmanaged_development();
        let state = volume.unique();
        state.mode = FilesystemVolumeMode::CopyOnWrite {
            root,
            #[cfg(target_os = "linux")]
            identity,
            #[cfg(target_os = "linux")]
            project_quotas: None,
        };
        state.accounting = AgentAccounting::Unaccounted;
        volume
    }

    /// The volume with the quota helper `quotas`, which serves its project quotas.
    #[cfg(target_os = "linux")]
    fn with_project_quotas(mut self, quotas: Arc<golem_xfs_quotad::XfsQuotadClient>) -> Self {
        if let FilesystemVolumeMode::CopyOnWrite { project_quotas, .. } = &mut self.unique().mode {
            *project_quotas = Some(quotas);
        }
        self
    }

    #[cfg(target_os = "linux")]
    fn quota_client(&self) -> Option<&golem_xfs_quotad::XfsQuotadClient> {
        match &self.state.mode {
            FilesystemVolumeMode::CopyOnWrite { project_quotas, .. } => project_quotas.as_deref(),
            FilesystemVolumeMode::UnmanagedDevelopment => None,
        }
    }

    /// Whether the volume makes copy-on-write copies of files: `copy_contents` and `seed` then
    /// share extents instead of copying bytes.
    pub(crate) fn copies_on_write(&self) -> bool {
        volume_facts(&self.state.mode).copies_on_write
    }

    pub(crate) fn storage_profile(&self) -> NativeStorageProfile {
        volume_facts(&self.state.mode).storage_profile
    }

    pub(crate) fn file_copy_mode(&self) -> FileCopyMode {
        volume_facts(&self.state.mode).file_copy_mode
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn copy_on_write_root(&self) -> Option<&Arc<File>> {
        match &self.state.mode {
            FilesystemVolumeMode::CopyOnWrite { root, .. } => Some(root),
            FilesystemVolumeMode::UnmanagedDevelopment => None,
        }
    }

    pub(crate) fn agent_accounting(&self) -> AgentAccounting {
        self.state.accounting
    }

    /// The selected storage path and its descriptor owner. Temporary storage has no shared root.
    pub(crate) fn root_location(&self) -> (Option<Arc<Path>>, Option<Arc<File>>) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let anchor = self.copy_on_write_root().cloned();
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let anchor = None;
        (self.state.root.clone(), anchor)
    }

    pub(crate) fn validate_host_directory(&self, path: &Path) -> Result<(), StorageVolumeError> {
        if self.state.host_directory_check == HostDirectoryCheck::None {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            xfs::verify_host_directory_has_no_project(path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn project_quotas(&self) -> Option<&ProjectQuotas> {
        self.state.projects.as_ref()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn validated_name_mode(&self) -> Option<ValidatedXfsNameMode> {
        self.state.name_mode
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn descriptor_rooted_for_test(root: Arc<File>) -> Self {
        use std::os::fd::AsRawFd;
        let path = Arc::from(Path::new(&format!("/proc/self/fd/{}", root.as_raw_fd())));
        let mut volume = Self::copy_on_write(root, FilesystemIdentity { device: 0 });
        volume.unique().root = Some(path);
        volume
    }
}

fn storage_facts(storage: &FilesystemStorageMode) -> (AgentAccounting, HostDirectoryCheck) {
    match storage {
        FilesystemStorageMode::Temporary | FilesystemStorageMode::Directory { .. } => {
            (AgentAccounting::Development, HostDirectoryCheck::None)
        }
        FilesystemStorageMode::ManagedXfs { .. } => (
            AgentAccounting::ProjectQuotas,
            HostDirectoryCheck::NoXfsProject,
        ),
        FilesystemStorageMode::ReflinkXfs { .. } => (
            AgentAccounting::Unaccounted,
            HostDirectoryCheck::NoXfsProject,
        ),
        FilesystemStorageMode::Apfs { .. } => {
            (AgentAccounting::Unaccounted, HostDirectoryCheck::None)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilesystemSpace {
    Unlimited,
    Observed {
        total_bytes: u64,
        available_bytes: u64,
        total_filesystem_objects: u64,
        available_filesystem_objects: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileCopyMode {
    Reflink,
    Buffered,
}

/// The facts that the mode of a volume decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VolumeFacts {
    /// Whether the volume makes copy-on-write copies of files.
    copies_on_write: bool,
    /// The storage profile of the native calls on a sandbox on the volume.
    storage_profile: NativeStorageProfile,
    /// How the files of a sandbox on the volume are copied.
    file_copy_mode: FileCopyMode,
}

/// Gives the facts of a volume in `mode`. XFS and APFS copy-on-write volumes are known local
/// storage, and their files share extents when copied, with or without a project quota.
/// The storage of other development volumes is unknown, and their files are copied by bytes.
fn volume_facts(mode: &FilesystemVolumeMode) -> VolumeFacts {
    match mode {
        FilesystemVolumeMode::CopyOnWrite { .. } => VolumeFacts {
            copies_on_write: true,
            storage_profile: NativeStorageProfile::KnownLocal,
            file_copy_mode: FileCopyMode::Reflink,
        },
        FilesystemVolumeMode::UnmanagedDevelopment => VolumeFacts {
            copies_on_write: false,
            storage_profile: NativeStorageProfile::Unknown,
            file_copy_mode: FileCopyMode::Buffered,
        },
    }
}

/// Flushes pending changes on the filesystem containing `root`.
#[cfg(target_os = "linux")]
pub(crate) fn sync_volume(root: &Path) -> std::io::Result<()> {
    rustix::fs::syncfs(&File::open(root)?).map_err(std::io::Error::from)
}

pub(crate) async fn observe_space(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, StorageVolumeError> {
    if matches!(
        &volume.state.mode,
        FilesystemVolumeMode::UnmanagedDevelopment
    ) {
        return Ok(FilesystemSpace::Unlimited);
    }
    let path = volume.state.root.clone();
    let volume = volume.clone();
    BlockingTask::Spawn
        .run(move || observe_space_blocking(&volume))
        .await
        .map_err(|error| {
            StorageVolumeError::from(OperationContext::task_failure(
                "observe filesystem volume space",
                path.as_deref(),
                error,
            ))
        })?
}

pub(crate) fn observe_space_blocking(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, StorageVolumeError> {
    let observed = match &volume.state.mode {
        FilesystemVolumeMode::UnmanagedDevelopment => Ok(FilesystemSpace::Unlimited),
        FilesystemVolumeMode::CopyOnWrite {
            root,
            #[cfg(target_os = "linux")]
            identity,
            ..
        } => {
            #[cfg(target_os = "linux")]
            {
                xfs::observe_space(root, *identity)
            }
            #[cfg(target_os = "macos")]
            {
                apfs::observe_space(root)
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = root;
                unreachable!("copy-on-write storage is unavailable on this platform")
            }
        }
    };
    observed.map_err(|error| {
        StorageVolumeError::from(OperationContext::io(
            "observe filesystem volume space",
            volume.state.root.as_deref(),
            error,
        ))
    })
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn space_from_values(
    blocks: u64,
    available_blocks: u64,
    fragment_size: u64,
    filesystem_objects: u64,
    available_filesystem_objects: u64,
) -> std::io::Result<FilesystemSpace> {
    let total_bytes = blocks.checked_mul(fragment_size).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "filesystem total capacity exceeds u64",
        )
    })?;
    let available_bytes = available_blocks.checked_mul(fragment_size).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "filesystem available capacity exceeds u64",
        )
    })?;
    Ok(FilesystemSpace::Observed {
        total_bytes,
        available_bytes,
        total_filesystem_objects: filesystem_objects,
        available_filesystem_objects,
    })
}

#[cfg(test)]
mod tests;
