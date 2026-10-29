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

use self::path_resolution::resolve_host_path_target;
use self::sandbox_cleanup::{
    ExclusiveFilesystemLease, LeaseState, RestoringLeaseState, rollback_created_filesystem,
};
use self::tree_copy::{CapabilityTempFile, file_transfer};
use super::object_identity::{SandboxDirectoryId, SandboxObjectId, native_file_identity};
use super::open_handles::{
    AppendCoordinatorRegistry, NativeAppendCoordination, NativeDirectoryHandle, NativeFileHandle,
    SandboxDirectory, SandboxFile, SandboxFilePermissions, SandboxHandle,
};
use super::sandbox_paths::{SandboxPath, SandboxPathBase, SandboxResolvedPathTarget};
#[cfg(test)]
use super::scripted_adapter::scripted_handle_error;
use super::{
    DeleteError, InstalledLimits, LinkGroup, SandboxAccessMode, SandboxAttributes,
    SandboxDirectoryEntry, SandboxFileDisposition, SandboxFilesystemAdapter,
    SandboxFilesystemAllocationReader, SandboxFlushLevel, SandboxFollow, SandboxInspectionFailure,
    SandboxObjectKind, SandboxOpenOptions, SandboxReadRange, SandboxSymlinkTarget,
    SandboxTimeChange, SandboxTimeChanges, SandboxWriteAttempt, SandboxWritePlacement, SeedEntry,
};
use crate::blocking::BlockingTask;
use crate::host_directories::HostPath;
use crate::sandbox_filesystem::FilesystemStorageError;
use crate::storage_volume::{AgentAccounting, FilesystemAllocation, FilesystemLimits};
use crate::storage_volume::{FilesystemVolume, NativeStorageProfile};
#[cfg(target_os = "linux")]
use crate::storage_volume::{ValidatedXfsNameMode, install_project_limits, project_allocation};
use bytes::Bytes;
use cap_fs_ext::{
    DirExt as _, FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt as _,
    OpenOptionsMaybeDirExt, OpenOptionsSyncExt,
};
use cap_std::fs::FileExt as _;
use fs_set_times::{SetTimes as _, SystemTimeSpec};
use std::fmt::Debug;
use std::fs::File;
use std::future::Future;
use std::io::{Seek, SeekFrom, Write};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

mod allocation_observer;
pub(in crate::sandbox_filesystem) mod directory_provisioning;
pub(in crate::sandbox_filesystem) mod path_resolution;
pub(in crate::sandbox_filesystem) mod sandbox_cleanup;
pub(in crate::sandbox_filesystem) mod sandbox_provisioning;
pub(in crate::sandbox_filesystem) mod tree_copy;

pub(crate) use allocation_observer::SandboxFilesystemAllocationObserver;
pub(crate) use sandbox_provisioning::{SandboxFilesystemName, SandboxFilesystemProvisioning};
pub(crate) use tree_copy::TreeExclusions;

pub(crate) struct SandboxFilesystem {
    pub(in crate::sandbox_filesystem) root: NativeRoot,
    pub(in crate::sandbox_filesystem) lease: ExclusiveFilesystemLease,
    pub(in crate::sandbox_filesystem) volume: FilesystemVolume,
    pub(in crate::sandbox_filesystem) quota_authority: QuotaAuthority,
    pub(in crate::sandbox_filesystem) name_mode_source: NativeNameModeSource,
    pub(in crate::sandbox_filesystem) name_mode_probe: NativeNameModeProbe,
    pub(in crate::sandbox_filesystem) append_coordinators: Arc<AppendCoordinatorRegistry>,
}

#[derive(Clone, Copy)]
pub(in crate::sandbox_filesystem) enum NativeNameModeSource {
    NativeDetection,
    #[cfg(target_os = "linux")]
    ValidatedXfs(ValidatedXfsNameMode),
}

#[derive(Clone, Default)]
pub(in crate::sandbox_filesystem) struct NativeNameModeProbe {
    #[cfg(all(test, target_os = "linux"))]
    pub(in crate::sandbox_filesystem) count: Arc<std::sync::atomic::AtomicUsize>,
}

impl NativeNameModeProbe {
    #[cfg(target_os = "linux")]
    fn record(&self) {
        #[cfg(test)]
        self.count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(all(test, target_os = "linux"))]
    fn count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub(in crate::sandbox_filesystem) struct NativeRoot {
    pub(in crate::sandbox_filesystem) path: Box<Path>,
    pub(in crate::sandbox_filesystem) directory: Arc<Mutex<Option<NativeRootHandles>>>,
}

/// One admission keeps the directory and, where the root has one, its unlinked regular
/// seed_flusher alive together.
#[derive(Clone)]
pub(in crate::sandbox_filesystem) struct NativeRootHandles {
    pub(in crate::sandbox_filesystem) directory: Arc<cap_std::fs::Dir>,
    pub(in crate::sandbox_filesystem) seed_flusher: Option<Arc<cap_std::fs::File>>,
}

impl NativeRootHandles {
    fn directory(&self) -> Arc<cap_std::fs::Dir> {
        Arc::clone(&self.directory)
    }
}

impl NativeRoot {
    /// The root at `path`, with its opened `directory`. The root keeps a seed_flusher when
    /// [`tree_copy::root_keeps_seed_flusher`] says so for `platform`.
    pub(in crate::sandbox_filesystem) fn new(
        path: PathBuf,
        directory: File,
        platform: tree_copy::SeedSyncPlatform,
    ) -> std::io::Result<Self> {
        let directory = Arc::new(cap_std::fs::Dir::from_std_file(directory));
        let seed_flusher = if tree_copy::root_keeps_seed_flusher(platform) {
            Some(Arc::new(tree_copy::finalize_operation(
                || CapabilityTempFile::new(&directory)?.into_unlinked(),
                || tree_copy::sync_ordinary_descriptor(directory.as_ref()),
            )?))
        } else {
            None
        };
        Ok(Self::from_handles(
            path,
            NativeRootHandles {
                directory,
                seed_flusher,
            },
        ))
    }

    async fn initialize(path: PathBuf, directory: File) -> Result<Self, FilesystemStorageError> {
        let error_path = path.clone().into_boxed_path();
        BlockingTask::Spawn
            .run(move || Self::new(path, directory, tree_copy::native_seed_sync_platform()))
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "initialize fresh runtime directory",
                    Some(&error_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "initialize fresh runtime directory",
                    Some(&error_path),
                    error,
                )
            })
    }

    #[cfg(target_os = "linux")]
    fn from_directory(path: PathBuf, directory: Arc<cap_std::fs::Dir>) -> Self {
        Self::from_handles(
            path,
            NativeRootHandles {
                directory,
                seed_flusher: None,
            },
        )
    }

    fn from_handles(path: PathBuf, handles: NativeRootHandles) -> Self {
        Self {
            path: path.into_boxed_path(),
            directory: Arc::new(Mutex::new(Some(handles))),
        }
    }

    pub(in crate::sandbox_filesystem) fn close(&self) {
        self.directory
            .lock()
            .expect("sandbox filesystem root descriptor lock poisoned")
            .take();
    }
}

/// Who enforces and measures the limits of a sandbox. A sandbox with a project identity charges
/// its project. A sandbox without one has no per-sandbox accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sandbox_filesystem) enum QuotaAuthority {
    Unsupported,
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "project quotas are installed only on Linux")
    )]
    Project {
        project_id: NonZeroU32,
        filesystem_block_bytes: NonZeroU64,
    },
}

impl SandboxFilesystem {
    pub(in crate::sandbox_filesystem) fn new(
        root: NativeRoot,
        lease: LeaseState,
        volume: FilesystemVolume,
        quota_authority: QuotaAuthority,
        name_mode_source: NativeNameModeSource,
    ) -> Self {
        Self {
            root,
            lease: ExclusiveFilesystemLease {
                state: Mutex::new(Some(lease)),
            },
            volume,
            quota_authority,
            name_mode_source,
            name_mode_probe: NativeNameModeProbe::default(),
            append_coordinators: Arc::new(AppendCoordinatorRegistry::default()),
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(in crate::sandbox_filesystem) fn name_mode_probe_count(&self) -> usize {
        self.name_mode_probe.count()
    }

    pub(in crate::sandbox_filesystem) fn root(&self) -> &Path {
        &self.root.path
    }

    pub(in crate::sandbox_filesystem) async fn observe_project_allocation(
        &self,
    ) -> Result<Option<FilesystemAllocation>, FilesystemStorageError> {
        let QuotaAuthority::Project {
            project_id: _project_id,
            ..
        } = self.quota_authority
        else {
            return Ok(None);
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root().to_path_buf();
            let volume = self.volume.clone();
            BlockingTask::Spawn
                .run(move || project_allocation(&volume, _project_id))
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "observe managed XFS project allocation",
                        Some(&root),
                        error,
                    )
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
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }

    pub(in crate::sandbox_filesystem) async fn install_storage_limits(
        &self,
        _limits: FilesystemLimits,
    ) -> Result<InstalledLimits, FilesystemStorageError> {
        let QuotaAuthority::Project {
            project_id: _project_id,
            filesystem_block_bytes: _filesystem_block_bytes,
        } = self.quota_authority
        else {
            return Err(FilesystemStorageError::verification(
                "install limits without quota authority",
                Some(self.root()),
            ));
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root().to_path_buf();
            let volume = self.volume.clone();
            let allocation = BlockingTask::Spawn
                .run(move || {
                    install_project_limits(&volume, _project_id, _filesystem_block_bytes, _limits)?;
                    project_allocation(&volume, _project_id)
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "install managed XFS project limits",
                        Some(&root),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "install managed XFS project limits",
                        Some(&root),
                        error,
                    )
                })?;
            Ok(InstalledLimits {
                limits: _limits,
                allocation,
            })
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }

    pub(in crate::sandbox_filesystem) async fn delete_owned_filesystem(
        self,
    ) -> Result<(), DeleteError<Self>> {
        self.root.close();
        let Some(mut state) = RestoringLeaseState::take(&self.lease.state) else {
            return Ok(());
        };
        match state.cleanup().delete().await {
            Ok(()) => {
                state.disarm();
                Ok(())
            }
            Err(source) => {
                drop(state);
                Err(DeleteError::new(self, source))
            }
        }
    }

    fn delete_and_verify_blocking(&self) -> Result<(), FilesystemStorageError> {
        self.root.close();
        let Some(mut state) = RestoringLeaseState::take(&self.lease.state) else {
            return Ok(());
        };
        state.cleanup().delete_blocking()?;
        state.disarm();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn root_for_test(&self) -> &Path {
        self.root()
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn project_id_for_test(&self) -> NonZeroU32 {
        match self.quota_authority {
            QuotaAuthority::Project { project_id, .. } => project_id,
            QuotaAuthority::Unsupported => panic!("sandbox filesystem has no project identity"),
        }
    }
}

impl Drop for SandboxFilesystem {
    fn drop(&mut self) {
        if let Err(error) = self.delete_and_verify_blocking() {
            tracing::error!(error = %error, "Failed to delete sandbox filesystem during fallback cleanup");
        }
    }
}

#[cfg(all(test, unix))]
pub(in crate::sandbox_filesystem) fn running_as_root() -> bool {
    // SAFETY: `geteuid` has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// What an open asks for, from its options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sandbox_filesystem) struct OpenRequest {
    /// The kind of object that the open expects.
    pub(in crate::sandbox_filesystem) expected: SandboxObjectKind,
    pub(in crate::sandbox_filesystem) access: SandboxAccessMode,
    pub(in crate::sandbox_filesystem) follow: SandboxFollow,
    /// The creation or truncation rule of a file open, and `None` when the open creates and
    /// truncates nothing.
    pub(in crate::sandbox_filesystem) disposition: Option<SandboxFileDisposition>,
    /// Whether the open is an inspection.
    pub(in crate::sandbox_filesystem) inspection: bool,
}

/// The flags of a native open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sandbox_filesystem) struct NativeOpenFlags {
    pub(in crate::sandbox_filesystem) read: bool,
    pub(in crate::sandbox_filesystem) write: bool,
    pub(in crate::sandbox_filesystem) create: bool,
    pub(in crate::sandbox_filesystem) create_new: bool,
    pub(in crate::sandbox_filesystem) truncate: bool,
    pub(in crate::sandbox_filesystem) nonblock: bool,
    pub(in crate::sandbox_filesystem) follow: bool,
}

pub(in crate::sandbox_filesystem) enum HostHandle {
    File(Arc<cap_std::fs::File>),
    Directory(Arc<cap_std::fs::Dir>),
}

/// Tells whether `metadata` describes a regular file without write permission.
pub(in crate::sandbox_filesystem) fn is_read_only_file(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_file() && metadata.permissions().readonly()
}

impl SandboxFilePermissions {
    pub(in crate::sandbox_filesystem) fn from_metadata(metadata: &cap_std::fs::Metadata) -> Self {
        if is_read_only_file(metadata) {
            Self::ReadOnly
        } else {
            Self::Writable
        }
    }
}

/// Asks for the rights that a change of an object's times needs on Windows.
///
/// A read open maps to `GENERIC_READ`, which does not carry `FILE_WRITE_ATTRIBUTES`, so
/// `SetFileTime` on such a handle is refused. An explicit access mode wins over the read and
/// write flags, so this asks for the rights of a read together with the one the change needs.
#[cfg(windows)]
pub(in crate::sandbox_filesystem) fn windows_add_set_times_right(
    options: &mut cap_std::fs::OpenOptions,
) {
    use cap_std::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, FILE_WRITE_ATTRIBUTES};

    options.access_mode(FILE_GENERIC_READ | FILE_WRITE_ATTRIBUTES);
}

/// Gives what `options` ask for.
///
/// An inspection reads its object, does not follow a symlink, and creates nothing. An open of an
/// existing object creates nothing. A file open expects a regular file and has a disposition.
pub(in crate::sandbox_filesystem) fn open_request(options: SandboxOpenOptions) -> OpenRequest {
    match options {
        SandboxOpenOptions::Inspection { expected } => OpenRequest {
            expected,
            access: SandboxAccessMode::Read,
            follow: SandboxFollow::No,
            disposition: None,
            inspection: true,
        },
        SandboxOpenOptions::Existing {
            expected,
            access,
            follow,
        } => OpenRequest {
            expected,
            access,
            follow,
            disposition: None,
            inspection: false,
        },
        SandboxOpenOptions::File {
            access,
            disposition,
            follow,
        } => OpenRequest {
            expected: SandboxObjectKind::File,
            access,
            follow,
            disposition: Some(disposition),
            inspection: false,
        },
    }
}

/// Whether `path` is exactly one normal path component, as an inspection requires.
pub(in crate::sandbox_filesystem) fn is_single_normal_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(name)) if name == path.as_os_str())
        && components.next().is_none()
}

/// Gives the native open flags of `options`.
///
/// A directory is opened for read only, whatever access the options ask for. A file is opened
/// for read with `Read` and `ReadAndSetTimes`, for write with `Write`, and for both with
/// `ReadWrite`. Each disposition writes: `CreateIfMissing` creates a missing file,
/// `CreateExclusive` creates a new file and fails on an existing one, `TruncateExisting` truncates
/// an existing file, and `CreateOrTruncate` does both of the first and the third. An inspection
/// does not block on a special file.
pub(in crate::sandbox_filesystem) fn native_open_flags(
    options: SandboxOpenOptions,
) -> NativeOpenFlags {
    let request = open_request(options);
    let (read, write) = if request.expected == SandboxObjectKind::Directory {
        (true, false)
    } else {
        match request.access {
            SandboxAccessMode::Read | SandboxAccessMode::ReadAndSetTimes => (true, false),
            SandboxAccessMode::Write => (false, true),
            SandboxAccessMode::ReadWrite => (true, true),
        }
    };
    let (create, create_new, truncate) = match request.disposition {
        None => (false, false, false),
        Some(SandboxFileDisposition::CreateIfMissing) => (true, false, false),
        Some(SandboxFileDisposition::CreateExclusive) => (false, true, false),
        Some(SandboxFileDisposition::TruncateExisting) => (false, false, true),
        Some(SandboxFileDisposition::CreateOrTruncate) => (true, false, true),
    };
    NativeOpenFlags {
        read,
        write: write || request.disposition.is_some(),
        create,
        create_new,
        truncate,
        nonblock: request.inspection,
        follow: request.follow == SandboxFollow::Yes,
    }
}

/// Checks that an opened object has the kind that the open expects. Another kind gives an
/// `InvalidInput` error that names both kinds.
pub(in crate::sandbox_filesystem) fn check_opened_kind(
    opened: SandboxObjectKind,
    expected: SandboxObjectKind,
) -> std::io::Result<()> {
    if opened == expected {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("expected {expected:?}, opened {opened:?}"),
        ))
    }
}

pub(in crate::sandbox_filesystem) fn check_inspection_kind(
    metadata: &cap_std::fs::Metadata,
    expected: SandboxObjectKind,
) -> std::io::Result<()> {
    let failure = if metadata.is_symlink() {
        Some(SandboxInspectionFailure::Symlink)
    } else if match expected {
        SandboxObjectKind::Directory => metadata.is_dir(),
        SandboxObjectKind::File => metadata.is_file(),
        SandboxObjectKind::Symlink => false,
    } {
        None
    } else {
        Some(SandboxInspectionFailure::NotRegular)
    };
    match failure {
        Some(failure) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            failure,
        )),
        None => Ok(()),
    }
}

impl SandboxFilesystemAdapter for SandboxFilesystem {
    type Provisioning = SandboxFilesystemProvisioning;
    type AllocationReader = SandboxFilesystemAllocationObserver;

    fn agent_accounting(&self) -> AgentAccounting {
        self.volume.agent_accounting()
    }

    async fn create_fresh(
        provisioning: Self::Provisioning,
        name: SandboxFilesystemName,
        limits: Option<FilesystemLimits>,
    ) -> Result<Self, FilesystemStorageError> {
        let filesystem = provisioning.create_fresh(name).await?;
        if let Some(limits) = limits
            && let Err(error) = SandboxFilesystem::install_storage_limits(&filesystem, limits).await
        {
            return Err(rollback_created_filesystem(filesystem, error).await);
        }
        Ok(filesystem)
    }

    fn open(
        &self,
        target: SandboxPath,
        options: SandboxOpenOptions,
    ) -> impl Future<Output = Result<SandboxHandle, FilesystemStorageError>> + Send {
        let operation_path = target.operation_path(self.root());
        let opened_path = operation_path.clone();
        let append_coordinators = Arc::clone(&self.append_coordinators);
        let task = self.short_task();
        let root_directory = self.root_directory_state();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &target)?;
                let mut native_options = cap_std::fs::OpenOptions::new();
                native_options.maybe_dir(true);
                let request = open_request(options);
                if request.inspection {
                    if !is_single_normal_component(&target.path) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "inspection requires a single normal path component",
                        ));
                    }
                    // Reject FIFOs/devices before opening. Recheck the opened descriptor below.
                    check_inspection_kind(
                        &directory.symlink_metadata(&target.path)?,
                        request.expected,
                    )?;
                }
                let flags = native_open_flags(options);
                if flags.nonblock {
                    native_options.nonblock(true);
                }
                native_options.read(flags.read).write(flags.write);
                // A directory takes the same right, because its times change the same way.
                #[cfg(windows)]
                if request.access == SandboxAccessMode::ReadAndSetTimes {
                    windows_add_set_times_right(&mut native_options);
                }
                native_options
                    .create(flags.create)
                    .create_new(flags.create_new)
                    .truncate(flags.truncate);
                native_options.follow(if flags.follow {
                    FollowSymlinks::Yes
                } else {
                    FollowSymlinks::No
                });
                let opened = directory.open_with(&target.path, &native_options)?;
                let metadata = opened.metadata()?;
                if request.inspection {
                    check_inspection_kind(&metadata, request.expected)?;
                }
                let kind = object_kind(&metadata);
                check_opened_kind(kind, request.expected)?;
                let node = match kind {
                    SandboxObjectKind::Directory => {
                        let identity = native_file_identity(&metadata)?;
                        SandboxHandle::Directory(SandboxDirectory {
                            handle: NativeDirectoryHandle::Host(Arc::new(
                                cap_std::fs::Dir::from_std_file(opened.into_std()),
                            )),
                            path: opened_path.clone(),
                            directory_id: SandboxDirectoryId(identity),
                        })
                    }
                    SandboxObjectKind::File => {
                        let identity = native_file_identity(&metadata)?;
                        SandboxHandle::File(SandboxFile {
                            handle: NativeFileHandle::Host(Arc::new(opened)),
                            path: opened_path.clone(),
                            append_coordination: NativeAppendCoordination::new(
                                identity,
                                append_coordinators,
                            ),
                            permissions: SandboxFilePermissions::from_metadata(&metadata),
                        })
                    }
                    SandboxObjectKind::Symlink => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "opening a symlink without following it is not an open node",
                        ));
                    }
                };
                Ok(node)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "open sandbox filesystem path",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "open sandbox filesystem path",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn resolve_path_target(
        &self,
        target: SandboxPath,
    ) -> impl Future<Output = Result<SandboxResolvedPathTarget, FilesystemStorageError>> + Send
    {
        let operation_path = target.operation_path(self.root());
        let parent_path = operation_path
            .parent()
            .unwrap_or_else(|| self.root())
            .to_path_buf();
        let name_mode_source = self.name_mode_source;
        let name_mode_probe = self.name_mode_probe.clone();
        let task = self.short_task();
        let root_directory = self.root_directory_state();
        async move {
            task.run(move || {
                let base = directory_for(&root_directory, &target)?;
                resolve_host_path_target(
                    base,
                    target,
                    parent_path,
                    name_mode_source,
                    name_mode_probe,
                )
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "resolve sandbox filesystem namespace target",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "resolve sandbox filesystem namespace target",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn read(
        &self,
        file: &SandboxFile,
        range: SandboxReadRange,
    ) -> impl Future<Output = Result<Bytes, FilesystemStorageError>> + Send {
        let operation_path = file.path.clone();
        let file = file.host().cloned();
        let task = self.transfer_task(range.length);
        async move {
            let file = file?;
            task.run(move || {
                let mut buffer = vec![0; range.length];
                let mut read = 0;
                while read < buffer.len() {
                    let offset = range.offset.checked_add(read as u64).ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "read range overflow")
                    })?;
                    #[cfg(unix)]
                    let count = file.read_at(&mut buffer[read..], offset)?;
                    #[cfg(windows)]
                    let count = file.seek_read(&mut buffer[read..], offset)?;
                    if count == 0 {
                        break;
                    }
                    read += count;
                }
                buffer.truncate(read);
                Ok(Bytes::from(buffer))
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "read sandbox filesystem file",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "read sandbox filesystem file",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn read_directory(
        &self,
        directory: &SandboxDirectory,
    ) -> impl Future<Output = Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>> + Send
    {
        let host = directory.host().cloned();
        let path = directory.path.clone();
        async move {
            let directory = host?;
            BlockingTask::Spawn
                .run(move || {
                    let mut result = Vec::new();
                    for entry in directory.entries()? {
                        let entry = entry?;
                        let file_type = entry.file_type()?;
                        result.push(SandboxDirectoryEntry {
                            name: entry.file_name(),
                            kind: file_type_kind(&file_type),
                        });
                    }
                    Ok(result)
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "read sandbox filesystem directory",
                        Some(&path),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "read sandbox filesystem directory",
                        Some(&path),
                        error,
                    )
                })
        }
    }

    fn read_link(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<SandboxSymlinkTarget, FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.read_link(&path.path).map(SandboxSymlinkTarget)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "read sandbox filesystem symlink",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "read sandbox filesystem symlink",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn write(
        &self,
        file: &SandboxFile,
        placement: SandboxWritePlacement,
        bytes: Bytes,
    ) -> impl Future<Output = Result<SandboxWriteAttempt, FilesystemStorageError>> + Send {
        let host = file.host().cloned();
        let path = file.path.clone();
        let task = self.transfer_task(bytes.len());
        async move {
            let file = match host {
                Ok(file) => file,
                Err(error) => return Ok(SandboxWriteAttempt::failed(0, error)),
            };
            let attempt = task
                .run(move || {
                    match placement {
                        SandboxWritePlacement::At(offset) => {
                            #[cfg(unix)]
                            let result = file.write_at(&bytes, offset);
                            #[cfg(windows)]
                            let result = file.seek_write(&bytes, offset);
                            result
                        }
                        SandboxWritePlacement::Append => {
                            let mut file = file.as_ref().try_clone()?.into_std();
                            file.seek(SeekFrom::End(0))?;
                            file.write(&bytes)
                        }
                    }
                    .map(|written| written as u64)
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "write sandbox filesystem file",
                        Some(&path),
                        error,
                    )
                })?;
            Ok(match attempt {
                Ok(written) => SandboxWriteAttempt::completed(written),
                Err(error) => SandboxWriteAttempt::failed(
                    0,
                    FilesystemStorageError::io("write sandbox filesystem file", Some(&path), error),
                ),
            })
        }
    }

    fn get_handle_attributes(
        &self,
        node: SandboxHandle,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        let path = handle_path(&node);
        let node = into_host_handle(node).map_err(|error| {
            FilesystemStorageError::io("read sandbox filesystem attributes", Some(&path), error)
        });
        let task = self.short_task();
        async move {
            let node = node?;
            task.run(move || node.metadata().and_then(attributes))
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "read sandbox filesystem attributes",
                        Some(&path),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "read sandbox filesystem attributes",
                        Some(&path),
                        error,
                    )
                })
        }
    }

    fn get_path_attributes(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        let path = target.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &target)?;
                path_metadata(&directory, &target.path, follow).and_then(attributes)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "read sandbox filesystem attributes",
                    Some(&path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io("read sandbox filesystem attributes", Some(&path), error)
            })
        }
    }

    fn is_same_open_object(
        &self,
        left: SandboxHandle,
        right: SandboxHandle,
    ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send {
        let left = into_host_handle(left).map_err(|error| {
            FilesystemStorageError::io("compare sandbox filesystem objects", None, error)
        });
        let right = into_host_handle(right).map_err(|error| {
            FilesystemStorageError::io("compare sandbox filesystem objects", None, error)
        });
        let task = self.short_task();
        async move {
            let left = left?;
            let right = right?;
            task.run(move || {
                Ok(native_file_identity(&left.metadata()?)?
                    == native_file_identity(&right.metadata()?)?)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "compare sandbox filesystem objects",
                    None,
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io("compare sandbox filesystem objects", None, error)
            })
        }
    }

    fn set_size(
        &self,
        file: &SandboxFile,
        size: u64,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let host = file.host().cloned();
        let path = file.path.clone();
        let task = self.short_task();
        async move {
            let file = host?;
            task.run(move || file.set_len(size))
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "resize sandbox filesystem file",
                        Some(&path),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("resize sandbox filesystem file", Some(&path), error)
                })
        }
    }

    fn set_handle_times(
        &self,
        node: SandboxHandle,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let path = handle_path(&node);
        let node = into_host_handle(node).map_err(|error| {
            FilesystemStorageError::io("set sandbox filesystem times", Some(&path), error)
        });
        let task = self.short_task();
        async move {
            let node = node?;
            task.run(move || node.set_times(times))
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "set sandbox filesystem times",
                        Some(&path),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("set sandbox filesystem times", Some(&path), error)
                })
        }
    }

    fn set_path_times(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let path = target.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &target)?;
                set_path_times(&directory, &target.path, follow, times)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "set sandbox filesystem times",
                    Some(&path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io("set sandbox filesystem times", Some(&path), error)
            })
        }
    }

    fn create_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.create_dir(path.path)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "create sandbox filesystem directory",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "create sandbox filesystem directory",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn create_symlink(
        &self,
        path: SandboxPath,
        target: SandboxSymlinkTarget,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.symlink(target.0, path.path)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "create sandbox filesystem symlink",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "create sandbox filesystem symlink",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn hard_link(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let destination_path = destination.operation_path(self.root());
        let task = self.short_task();
        let root_directory = self.root_directory_state();
        async move {
            task.run(move || {
                let source_directory = directory_for(&root_directory, &source)?;
                let destination_directory = directory_for(&root_directory, &destination)?;
                source_directory.hard_link(source.path, &destination_directory, destination.path)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "hard-link sandbox filesystem path",
                    Some(&destination_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "hard-link sandbox filesystem path",
                    Some(&destination_path),
                    error,
                )
            })
        }
    }

    fn rename(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let destination_path = destination.operation_path(self.root());
        let task = self.short_task();
        let root_directory = self.root_directory_state();
        async move {
            task.run(move || {
                let source_directory = directory_for(&root_directory, &source)?;
                let destination_directory = directory_for(&root_directory, &destination)?;
                source_directory.rename(source.path, &destination_directory, destination.path)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "rename sandbox filesystem path",
                    Some(&destination_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "rename sandbox filesystem path",
                    Some(&destination_path),
                    error,
                )
            })
        }
    }

    fn remove_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.remove_dir(path.path)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "remove sandbox filesystem directory",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "remove sandbox filesystem directory",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn unlink_file(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let task = self.short_task();
        async move {
            task.run(move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.remove_file_or_symlink(path.path)
            })
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "unlink sandbox filesystem file",
                    Some(&operation_path),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "unlink sandbox filesystem file",
                    Some(&operation_path),
                    error,
                )
            })
        }
    }

    fn flush(
        &self,
        node: &SandboxHandle,
        level: SandboxFlushLevel,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let path = match node {
            SandboxHandle::File(file) => file.path.clone(),
            SandboxHandle::Directory(directory) => directory.path.clone(),
        };
        let node = node.clone();
        async move {
            BlockingTask::Spawn
                .run(move || host_handle(&node)?.flush(level))
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "flush sandbox filesystem node",
                        Some(&path),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("flush sandbox filesystem node", Some(&path), error)
                })
        }
    }

    async fn close(&self, node: SandboxHandle) -> Result<(), FilesystemStorageError> {
        drop(node);
        Ok(())
    }

    fn seed(
        &self,
        entries: Box<[SeedEntry]>,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let materialization_root: Arc<Path> = Arc::from(self.root());
        let root_directory = self.root_directory_state();
        let platform = tree_copy::native_seed_sync_platform();
        let policy = tree_copy::SeedSyncPolicy::new(
            platform,
            file_transfer(platform, self.volume.file_copy_mode(), self.quota_authority),
        );
        async move {
            let error_path = Arc::clone(&materialization_root);
            BlockingTask::Spawn
                .run(move || {
                    let admitted = if policy.requires_batch_root() {
                        Some(admit_root(&root_directory).map_err(|error| {
                            FilesystemStorageError::io(
                                "admit sandbox filesystem seed",
                                Some(&materialization_root),
                                error,
                            )
                        })?)
                    } else {
                        None
                    };
                    tree_copy::finalize_operation(
                        || {
                            entries.iter().try_for_each(|entry| {
                                let context = tree_copy::SeedContext {
                                    policy,
                                    access: entry.access,
                                    placement: entry.placement,
                                };
                                match &admitted {
                                    Some(root) => {
                                        base_directory(&entry.target, || Ok(root.directory()))
                                    }
                                    None => directory_for(&root_directory, &entry.target),
                                }
                                .and_then(|base| {
                                    tree_copy::seed_entry(
                                        context,
                                        &base,
                                        entry.source.as_path(),
                                        &entry.target.path,
                                    )
                                })
                                .map_err(|error| {
                                    FilesystemStorageError::io(
                                        "seed sandbox filesystem entry",
                                        Some(&entry.target.operation_path(&materialization_root)),
                                        error,
                                    )
                                })
                            })
                        },
                        || {
                            tree_copy::sync_seed_batch(
                                policy,
                                admitted.as_ref(),
                                &materialization_root,
                            )
                            .map_err(|error| {
                                FilesystemStorageError::io(
                                    "sync seeded sandbox filesystem",
                                    Some(&materialization_root),
                                    error,
                                )
                            })
                        },
                    )
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "seed sandbox filesystem",
                        Some(&error_path),
                        error,
                    )
                })?
        }
    }

    async fn observe_allocation(&self) -> Result<FilesystemAllocation, FilesystemStorageError> {
        SandboxFilesystem::observe_project_allocation(self)
            .await?
            .ok_or_else(|| FilesystemStorageError::allocation_unsupported(Some(self.root())))
    }

    fn allocation_reader(&self) -> Self::AllocationReader {
        SandboxFilesystemAllocationObserver::new(self)
    }

    fn install_limits(
        &self,
        limits: FilesystemLimits,
    ) -> impl Future<Output = Result<InstalledLimits, FilesystemStorageError>> + Send {
        SandboxFilesystem::install_storage_limits(self, limits)
    }

    fn copy_contents(
        &self,
        source: SandboxPath,
        excluded: Arc<TreeExclusions>,
        target: &HostPath,
    ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send {
        let transfer = file_transfer(
            tree_copy::native_seed_sync_platform(),
            self.volume.file_copy_mode(),
            QuotaAuthority::Unsupported,
        );
        let operation_path = source.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let target = target.clone();
        async move {
            BlockingTask::Spawn
                .run(move || {
                    let base = directory_for(&root_directory, &source)?;
                    tree_copy::copy_contents(
                        &base,
                        &source.path,
                        &excluded,
                        target.as_path(),
                        transfer,
                    )
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "copy sandbox filesystem contents",
                        Some(&operation_path),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "copy sandbox filesystem contents",
                        Some(&operation_path),
                        error,
                    )
                })
        }
    }

    async fn delete_and_verify(self) -> Result<(), DeleteError<Self>> {
        SandboxFilesystem::delete_owned_filesystem(self).await
    }
}

impl SandboxFilesystemAllocationReader for SandboxFilesystemAllocationObserver {
    fn read_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send {
        self.observe()
    }
}

impl SandboxFilesystem {
    // Only bounded work on known-local storage may block in place. Enumeration, copying,
    // flushing and quota operations always use the blocking pool.
    fn short_task(&self) -> BlockingTask {
        match self.volume.storage_profile() {
            NativeStorageProfile::KnownLocal => BlockingTask::InPlace,
            NativeStorageProfile::Unknown => BlockingTask::Spawn,
        }
    }

    fn transfer_task(&self, bytes: usize) -> BlockingTask {
        const MAX_SHORT_TRANSFER_BYTES: usize = 64 * 1024;
        if bytes <= MAX_SHORT_TRANSFER_BYTES {
            self.short_task()
        } else {
            BlockingTask::Spawn
        }
    }

    pub(in crate::sandbox_filesystem) fn root_directory_state(
        &self,
    ) -> Arc<Mutex<Option<NativeRootHandles>>> {
        Arc::clone(&self.root.directory)
    }
}

pub(in crate::sandbox_filesystem) fn handle_path(node: &SandboxHandle) -> PathBuf {
    match node {
        SandboxHandle::File(file) => file.path.clone(),
        SandboxHandle::Directory(directory) => directory.path.clone(),
    }
}

pub(in crate::sandbox_filesystem) fn admit_root(
    root: &Arc<Mutex<Option<NativeRootHandles>>>,
) -> std::io::Result<NativeRootHandles> {
    root.lock()
        .expect("sandbox filesystem root descriptor lock poisoned")
        .as_ref()
        .cloned()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "sandbox filesystem root has been released",
            )
        })
}

/// Gives the directory that `target` is relative to. `root` gives the root directory of the
/// sandbox, and is called only for a target at the root.
pub(in crate::sandbox_filesystem) fn base_directory(
    target: &SandboxPath,
    root: impl FnOnce() -> std::io::Result<Arc<cap_std::fs::Dir>>,
) -> std::io::Result<Arc<cap_std::fs::Dir>> {
    match &target.base {
        SandboxPathBase::Directory(directory) => {
            directory.host().cloned().map_err(std::io::Error::other)
        }
        SandboxPathBase::Root => root(),
    }
}

pub(in crate::sandbox_filesystem) fn directory_for(
    root: &Arc<Mutex<Option<NativeRootHandles>>>,
    target: &SandboxPath,
) -> std::io::Result<Arc<cap_std::fs::Dir>> {
    base_directory(target, || {
        let directory = admit_root(root)?.directory();
        #[cfg(test)]
        if benchmark_disable_root_capability_reuse() {
            return directory.try_clone().map(Arc::new);
        }
        Ok(directory)
    })
}

#[cfg(test)]
pub(in crate::sandbox_filesystem) fn benchmark_disable_root_capability_reuse() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE").as_deref()
            == Ok("1")
    })
}

pub(in crate::sandbox_filesystem) fn path_metadata(
    directory: &cap_std::fs::Dir,
    path: &Path,
    follow: SandboxFollow,
) -> std::io::Result<cap_std::fs::Metadata> {
    if path.as_os_str().is_empty() {
        return directory.dir_metadata();
    }
    match follow {
        SandboxFollow::Yes => directory.metadata(path),
        SandboxFollow::No => directory.symlink_metadata(path),
    }
}

pub(in crate::sandbox_filesystem) fn set_path_times(
    directory: &cap_std::fs::Dir,
    path: &Path,
    follow: SandboxFollow,
    times: SandboxTimeChanges,
) -> std::io::Result<()> {
    let now = SystemTime::now();
    let accessed = time_spec(times.accessed, now);
    let modified = time_spec(times.modified, now);
    match follow {
        SandboxFollow::Yes => cap_fs_ext::DirExt::set_times(
            directory,
            path,
            accessed.map(cap_time_spec),
            modified.map(cap_time_spec),
        ),
        SandboxFollow::No => directory.set_symlink_times(
            path,
            accessed.map(cap_time_spec),
            modified.map(cap_time_spec),
        ),
    }
}

impl HostHandle {
    pub(in crate::sandbox_filesystem) fn metadata(&self) -> std::io::Result<cap_std::fs::Metadata> {
        match self {
            Self::File(file) => file.metadata(),
            Self::Directory(directory) => directory.dir_metadata(),
        }
    }

    pub(in crate::sandbox_filesystem) fn set_times(
        &self,
        times: SandboxTimeChanges,
    ) -> std::io::Result<()> {
        let now = SystemTime::now();
        let accessed = time_spec(times.accessed, now);
        let modified = time_spec(times.modified, now);
        match self {
            Self::File(file) => file.set_times(accessed, modified),
            Self::Directory(directory) => directory.set_times(accessed, modified),
        }
    }

    pub(in crate::sandbox_filesystem) fn flush(
        &self,
        level: SandboxFlushLevel,
    ) -> std::io::Result<()> {
        #[cfg(windows)]
        let is_file = matches!(self, Self::File(_));
        let file = match self {
            Self::File(file) => file.as_ref().try_clone()?.into_std(),
            Self::Directory(directory) => directory.open(Component::CurDir)?.into_std(),
        };
        let result = match level {
            SandboxFlushLevel::Data => file.sync_data(),
            SandboxFlushLevel::DataAndMetadata => file.sync_all(),
        };
        #[cfg(windows)]
        if is_file
            && result.as_ref().is_err_and(|error| {
                error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as _)
            })
        {
            return Ok(());
        }
        result
    }
}

pub(in crate::sandbox_filesystem) fn host_handle(
    node: &SandboxHandle,
) -> std::io::Result<HostHandle> {
    match node {
        SandboxHandle::File(file) => Ok(HostHandle::File(
            file.host().cloned().map_err(std::io::Error::other)?,
        )),
        SandboxHandle::Directory(directory) => Ok(HostHandle::Directory(
            directory.host().cloned().map_err(std::io::Error::other)?,
        )),
    }
}

pub(in crate::sandbox_filesystem) fn into_host_handle(
    node: SandboxHandle,
) -> std::io::Result<HostHandle> {
    match node {
        SandboxHandle::File(file) => match file.handle {
            NativeFileHandle::Host(file) => Ok(HostHandle::File(file)),
            #[cfg(test)]
            NativeFileHandle::Scripted(_) => Err(std::io::Error::other(scripted_handle_error(
                "use scripted file",
            ))),
        },
        SandboxHandle::Directory(directory) => match directory.handle {
            NativeDirectoryHandle::Host(directory) => Ok(HostHandle::Directory(directory)),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(_) => Err(std::io::Error::other(
                scripted_handle_error("use scripted directory"),
            )),
        },
    }
}

pub(in crate::sandbox_filesystem) fn object_kind(
    metadata: &cap_std::fs::Metadata,
) -> SandboxObjectKind {
    let file_type = metadata.file_type();
    file_type_kind(&file_type)
}

pub(in crate::sandbox_filesystem) fn file_type_kind(
    file_type: &cap_std::fs::FileType,
) -> SandboxObjectKind {
    if file_type.is_dir() {
        SandboxObjectKind::Directory
    } else if file_type.is_symlink() {
        SandboxObjectKind::Symlink
    } else {
        SandboxObjectKind::File
    }
}

pub(in crate::sandbox_filesystem) fn attributes(
    metadata: cap_std::fs::Metadata,
) -> std::io::Result<SandboxAttributes> {
    Ok(SandboxAttributes {
        kind: object_kind(&metadata),
        link_count: metadata.nlink(),
        size: metadata.len(),
        accessed: metadata.accessed().ok().map(|time| time.into_std()),
        modified: metadata.modified().ok().map(|time| time.into_std()),
        read_only: metadata.permissions().readonly(),
        object: SandboxObjectId(native_file_identity(&metadata)?),
    })
}

pub(in crate::sandbox_filesystem) fn time_spec(
    change: SandboxTimeChange,
    now: SystemTime,
) -> Option<SystemTimeSpec> {
    match change {
        SandboxTimeChange::Keep => None,
        SandboxTimeChange::Now => Some(SystemTimeSpec::Absolute(now)),
        SandboxTimeChange::Set(time) => Some(SystemTimeSpec::Absolute(time)),
    }
}

pub(in crate::sandbox_filesystem) fn cap_time_spec(
    spec: SystemTimeSpec,
) -> cap_fs_ext::SystemTimeSpec {
    match spec {
        SystemTimeSpec::Absolute(time) => {
            cap_fs_ext::SystemTimeSpec::Absolute(cap_std::time::SystemTime::from_std(time))
        }
        SystemTimeSpec::SymbolicNow => cap_fs_ext::SystemTimeSpec::SymbolicNow,
    }
}

#[cfg(test)]
mod tests;
