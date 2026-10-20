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

use super::*;
use cap_fs_ext::{
    FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt, OpenOptionsMaybeDirExt,
    OpenOptionsSyncExt,
};
use cap_std::fs::FileExt as _;
use fs_set_times::{SetTimes as _, SystemTimeSpec};
use std::io::{Seek, SeekFrom, Write};

/// Asks for the rights that a change of an object's times needs on Windows.
///
/// A read open maps to `GENERIC_READ`, which does not carry `FILE_WRITE_ATTRIBUTES`, so
/// `SetFileTime` on such a handle is refused. An explicit access mode wins over the read and
/// write flags, so this asks for the rights of a read together with the one the change needs.
#[cfg(windows)]
fn windows_add_set_times_right(options: &mut cap_std::fs::OpenOptions) {
    use cap_std::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, FILE_WRITE_ATTRIBUTES};

    options.access_mode(FILE_GENERIC_READ | FILE_WRITE_ATTRIBUTES);
}

/// Gives what `options` ask for.
///
/// An inspection reads its object, does not follow a symlink, and creates nothing. An open of an
/// existing object creates nothing. A file open expects a regular file and has a disposition.
pub(super) fn open_request(options: SandboxOpenOptions) -> OpenRequest {
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
pub(super) fn is_single_normal_component(path: &Path) -> bool {
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
pub(super) fn native_open_flags(options: SandboxOpenOptions) -> NativeOpenFlags {
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
pub(super) fn check_opened_kind(
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

fn check_inspection_kind(
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

    async fn create_fresh(
        provisioning: Self::Provisioning,
        name: SandboxFilesystemName,
        limits: Option<FilesystemLimits>,
    ) -> Result<Self, FilesystemStorageError> {
        let filesystem = provisioning.create_fresh(name).await?;
        if let Some(limits) = limits
            && let Err(error) = SandboxFilesystem::install_limits(&filesystem, limits).await
        {
            return Err(rollback_created_filesystem(filesystem, error).await);
        }
        Ok(filesystem)
    }

    fn open(
        &self,
        target: SandboxPath,
        options: SandboxOpenOptions,
    ) -> impl Future<Output = Result<SandboxOpened, FilesystemStorageError>> + Send {
        let operation_path = target.operation_path(self.root());
        let opened_path = operation_path.clone();
        let append_coordinators = Arc::clone(&self.append_coordinators);
        let storage_profile = storage_profile(&self.volume);
        let root_directory = self.root_directory_state();
        async move {
            execute_native(storage_profile, NativeOperation::Open, move || {
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
                let read_only_file = is_read_only_file(&metadata);
                let node = match kind {
                    SandboxObjectKind::Directory => {
                        let identity = native_file_identity(&metadata)?;
                        SandboxNode::Directory(SandboxDirectory {
                            handle: NativeDirectoryHandle::Host(Arc::new(
                                cap_std::fs::Dir::from_std_file(opened.into_std()),
                            )),
                            path: opened_path.clone(),
                            coordination_key: SandboxDirectoryCoordinationKey(identity),
                        })
                    }
                    SandboxObjectKind::File => {
                        let identity = native_file_identity(&metadata)?;
                        SandboxNode::File(SandboxFile {
                            handle: NativeFileHandle::Host(Arc::new(opened)),
                            path: opened_path.clone(),
                            append_coordination: NativeAppendCoordination::new(
                                identity,
                                append_coordinators,
                            ),
                        })
                    }
                    SandboxObjectKind::Symlink => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "opening a symlink without following it is not an open node",
                        ));
                    }
                };
                Ok(SandboxOpened {
                    node,
                    read_only_file,
                })
            })
            .await
            .map_err(|error| task_error("open sandbox filesystem path", &operation_path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("open sandbox filesystem path", &operation_path, error)
            })
        }
    }

    fn resolve_namespace_target(
        &self,
        target: SandboxPath,
    ) -> impl Future<Output = Result<SandboxResolvedNamespaceTarget, FilesystemStorageError>> + Send
    {
        let operation_path = target.operation_path(self.root());
        let parent_path = operation_path
            .parent()
            .unwrap_or_else(|| Path::new("<sandbox-filesystem>"))
            .to_path_buf();
        let name_mode_source = self.name_mode_source;
        let name_mode_probe = self.name_mode_probe.clone();
        let storage_profile = storage_profile(&self.volume);
        let root_directory = self.root_directory_state();
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let base = directory_for(&root_directory, &target)?;
                resolve_host_namespace_target(
                    base,
                    target,
                    parent_path,
                    name_mode_source,
                    name_mode_probe,
                )
            })
            .await
            .map_err(|error| {
                task_error(
                    "resolve sandbox filesystem namespace target",
                    &operation_path,
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "resolve sandbox filesystem namespace target",
                    &operation_path,
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            let file = file?;
            execute_native(
                storage_profile,
                NativeOperation::Read(range.length),
                move || {
                    let mut buffer = vec![0; range.length];
                    let mut read = 0;
                    while read < buffer.len() {
                        let offset = range.offset.checked_add(read as u64).ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "read range overflow",
                            )
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
                },
            )
            .await
            .map_err(|error| task_error("read sandbox filesystem file", &operation_path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("read sandbox filesystem file", &operation_path, error)
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            let directory = host?;
            execute_native(
                storage_profile,
                NativeOperation::DirectoryEnumeration,
                move || {
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
                },
            )
            .await
            .map_err(|error| task_error("read sandbox filesystem directory", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("read sandbox filesystem directory", &path, error)
            })
        }
    }

    fn read_link(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<SandboxSymlinkTarget, FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.read_link(&path.path).map(SandboxSymlinkTarget)
            })
            .await
            .map_err(|error| task_error("read sandbox filesystem symlink", &operation_path, error))?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "read sandbox filesystem symlink",
                    &operation_path,
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
        let storage_profile = storage_profile(&self.volume);
        let transfer_size = bytes.len();
        async move {
            let file = match host {
                Ok(file) => file,
                Err(error) => return Ok(SandboxWriteAttempt::failed(0, error)),
            };
            let attempt = execute_native(
                storage_profile,
                NativeOperation::Write(transfer_size),
                move || {
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
                },
            )
            .await
            .map_err(|error| task_error("write sandbox filesystem file", &path, error))?;
            Ok(match attempt {
                Ok(written) => SandboxWriteAttempt::completed(written),
                Err(error) => SandboxWriteAttempt::failed(
                    0,
                    FilesystemStorageError::io("write sandbox filesystem file", &path, error),
                ),
            })
        }
    }

    fn get_node_attributes(
        &self,
        node: SandboxNode,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        let path = node_path(&node);
        let node = into_host_node(node).map_err(|error| {
            FilesystemStorageError::io("read sandbox filesystem attributes", &path, error)
        });
        let storage_profile = storage_profile(&self.volume);
        async move {
            let node = node?;
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                node.metadata().and_then(attributes)
            })
            .await
            .map_err(|error| task_error("read sandbox filesystem attributes", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("read sandbox filesystem attributes", &path, error)
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                let directory = directory_for(&root_directory, &target)?;
                path_metadata(&directory, &target.path, follow).and_then(attributes)
            })
            .await
            .map_err(|error| task_error("read sandbox filesystem attributes", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("read sandbox filesystem attributes", &path, error)
            })
        }
    }

    fn is_same_open_object(
        &self,
        left: SandboxNode,
        right: SandboxNode,
    ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send {
        let left = into_host_node(left).map_err(|error| {
            FilesystemStorageError::io(
                "compare sandbox filesystem objects",
                Path::new("<sandbox-filesystem>"),
                error,
            )
        });
        let right = into_host_node(right).map_err(|error| {
            FilesystemStorageError::io(
                "compare sandbox filesystem objects",
                Path::new("<sandbox-filesystem>"),
                error,
            )
        });
        let storage_profile = storage_profile(&self.volume);
        async move {
            let left = left?;
            let right = right?;
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                Ok(native_file_identity(&left.metadata()?)?
                    == native_file_identity(&right.metadata()?)?)
            })
            .await
            .map_err(|error| {
                task_error(
                    "compare sandbox filesystem objects",
                    Path::new("<sandbox-filesystem>"),
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "compare sandbox filesystem objects",
                    Path::new("<sandbox-filesystem>"),
                    error,
                )
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            let file = host?;
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                file.set_len(size)
            })
            .await
            .map_err(|error| task_error("resize sandbox filesystem file", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("resize sandbox filesystem file", &path, error)
            })
        }
    }

    fn set_node_times(
        &self,
        node: SandboxNode,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let path = node_path(&node);
        let node = into_host_node(node).map_err(|error| {
            FilesystemStorageError::io("set sandbox filesystem times", &path, error)
        });
        let storage_profile = storage_profile(&self.volume);
        async move {
            let node = node?;
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                node.set_times(times)
            })
            .await
            .map_err(|error| task_error("set sandbox filesystem times", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("set sandbox filesystem times", &path, error)
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Metadata, move || {
                let directory = directory_for(&root_directory, &target)?;
                set_path_times(&directory, &target.path, follow, times)
            })
            .await
            .map_err(|error| task_error("set sandbox filesystem times", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("set sandbox filesystem times", &path, error)
            })
        }
    }

    fn create_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let operation_path = path.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.create_dir(path.path)
            })
            .await
            .map_err(|error| {
                task_error(
                    "create sandbox filesystem directory",
                    &operation_path,
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "create sandbox filesystem directory",
                    &operation_path,
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.symlink(target.0, path.path)
            })
            .await
            .map_err(|error| {
                task_error("create sandbox filesystem symlink", &operation_path, error)
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "create sandbox filesystem symlink",
                    &operation_path,
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
        let storage_profile = storage_profile(&self.volume);
        let root_directory = self.root_directory_state();
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let source_directory = directory_for(&root_directory, &source)?;
                let destination_directory = directory_for(&root_directory, &destination)?;
                source_directory.hard_link(source.path, &destination_directory, destination.path)
            })
            .await
            .map_err(|error| {
                task_error(
                    "hard-link sandbox filesystem path",
                    &destination_path,
                    error,
                )
            })?
            .map_err(|error| {
                namespace_operation_error(
                    "hard-link sandbox filesystem path",
                    &destination_path,
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
        let storage_profile = storage_profile(&self.volume);
        let root_directory = self.root_directory_state();
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let source_directory = directory_for(&root_directory, &source)?;
                let destination_directory = directory_for(&root_directory, &destination)?;
                source_directory.rename(source.path, &destination_directory, destination.path)
            })
            .await
            .map_err(|error| {
                task_error("rename sandbox filesystem path", &destination_path, error)
            })?
            .map_err(|error| {
                namespace_operation_error(
                    "rename sandbox filesystem path",
                    &destination_path,
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.remove_dir(path.path)
            })
            .await
            .map_err(|error| {
                task_error(
                    "remove sandbox filesystem directory",
                    &operation_path,
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "remove sandbox filesystem directory",
                    &operation_path,
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
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Namespace, move || {
                let directory = directory_for(&root_directory, &path)?;
                directory.remove_file_or_symlink(path.path)
            })
            .await
            .map_err(|error| task_error("unlink sandbox filesystem file", &operation_path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("unlink sandbox filesystem file", &operation_path, error)
            })
        }
    }

    fn flush(
        &self,
        node: &SandboxNode,
        level: SandboxFlushLevel,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let path = match node {
            SandboxNode::File(file) => file.path.clone(),
            SandboxNode::Directory(directory) => directory.path.clone(),
        };
        let node = node.clone();
        let storage_profile = storage_profile(&self.volume);
        async move {
            execute_native(storage_profile, NativeOperation::Flush, move || {
                host_node(&node)?.flush(level)
            })
            .await
            .map_err(|error| task_error("flush sandbox filesystem node", &path, error))?
            .map_err(|error| {
                FilesystemStorageError::io("flush sandbox filesystem node", &path, error)
            })
        }
    }

    async fn close(&self, node: SandboxNode) -> Result<(), FilesystemStorageError> {
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
            file_transfer(platform, file_copy_mode(&self.volume), self.quota_authority),
        );
        let storage_profile = storage_profile(&self.volume);
        async move {
            let error_path = Arc::clone(&materialization_root);
            execute_native(storage_profile, NativeOperation::TreeCopy, move || {
                let admitted = if policy.requires_batch_root() {
                    Some(admit_root(&root_directory).map_err(|error| {
                        FilesystemStorageError::io(
                            "admit sandbox filesystem seed",
                            &materialization_root,
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
                                    &entry.target.operation_path(&materialization_root),
                                    error,
                                )
                            })
                        })
                    },
                    || {
                        tree_copy::sync_seed_batch(policy, admitted.as_ref(), &materialization_root)
                            .map_err(|error| {
                                FilesystemStorageError::io(
                                    "sync seeded sandbox filesystem",
                                    &materialization_root,
                                    error,
                                )
                            })
                    },
                )
            })
            .await
            .map_err(|error| task_error("seed sandbox filesystem", &error_path, error))?
        }
    }

    async fn observe_allocation(&self) -> Result<FilesystemAllocation, FilesystemStorageError> {
        SandboxFilesystem::observe_allocation(self)
            .await?
            .ok_or_else(|| FilesystemStorageError::allocation_unsupported(self.root()))
    }

    fn allocation_reader(&self) -> Self::AllocationReader {
        SandboxFilesystemAllocationObserver::new(self)
    }

    fn install_limits(
        &self,
        limits: FilesystemLimits,
    ) -> impl Future<Output = Result<InstalledLimits, FilesystemStorageError>> + Send {
        SandboxFilesystem::install_limits(self, limits)
    }

    fn copy_contents(
        &self,
        source: SandboxPath,
        excluded: Arc<TreeExclusions>,
        target: &HostPath,
    ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send {
        let transfer = file_transfer(
            tree_copy::native_seed_sync_platform(),
            file_copy_mode(&self.volume),
            QuotaAuthority::Unsupported,
        );
        let operation_path = source.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let storage_profile = storage_profile(&self.volume);
        let target = target.clone();
        async move {
            execute_native(storage_profile, NativeOperation::TreeCopy, move || {
                let base = directory_for(&root_directory, &source)?;
                tree_copy::copy_contents(&base, &source.path, &excluded, target.as_path(), transfer)
            })
            .await
            .map_err(|error| {
                task_error("copy sandbox filesystem contents", &operation_path, error)
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "copy sandbox filesystem contents",
                    &operation_path,
                    error,
                )
            })
        }
    }

    async fn delete_and_verify(self) -> Result<(), DeleteError<Self>> {
        SandboxFilesystem::delete_and_verify(self).await
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
    pub(super) fn root_directory_state(&self) -> Arc<Mutex<Option<NativeRootHandles>>> {
        Arc::clone(&self.root.directory)
    }
}

fn node_path(node: &SandboxNode) -> PathBuf {
    match node {
        SandboxNode::File(file) => file.path.clone(),
        SandboxNode::Directory(directory) => directory.path.clone(),
    }
}

pub(super) fn admit_root(
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
fn base_directory(
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

pub(super) fn directory_for(
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
fn benchmark_disable_root_capability_reuse() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE").as_deref()
            == Ok("1")
    })
}

fn path_metadata(
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

fn set_path_times(
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

impl HostNode {
    pub(super) fn metadata(&self) -> std::io::Result<cap_std::fs::Metadata> {
        match self {
            Self::File(file) => file.metadata(),
            Self::Directory(directory) => directory.dir_metadata(),
        }
    }

    pub(super) fn set_times(&self, times: SandboxTimeChanges) -> std::io::Result<()> {
        let now = SystemTime::now();
        let accessed = time_spec(times.accessed, now);
        let modified = time_spec(times.modified, now);
        match self {
            Self::File(file) => file.set_times(accessed, modified),
            Self::Directory(directory) => directory.set_times(accessed, modified),
        }
    }

    pub(super) fn flush(&self, level: SandboxFlushLevel) -> std::io::Result<()> {
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

fn host_node(node: &SandboxNode) -> std::io::Result<HostNode> {
    match node {
        SandboxNode::File(file) => Ok(HostNode::File(
            file.host().cloned().map_err(std::io::Error::other)?,
        )),
        SandboxNode::Directory(directory) => Ok(HostNode::Directory(
            directory.host().cloned().map_err(std::io::Error::other)?,
        )),
    }
}

fn into_host_node(node: SandboxNode) -> std::io::Result<HostNode> {
    match node {
        SandboxNode::File(file) => match file.handle {
            NativeFileHandle::Host(file) => Ok(HostNode::File(file)),
            #[cfg(test)]
            NativeFileHandle::Scripted(_) => Err(std::io::Error::other(scripted_handle_error(
                "use scripted file",
            ))),
        },
        SandboxNode::Directory(directory) => match directory.handle {
            NativeDirectoryHandle::Host(directory) => Ok(HostNode::Directory(directory)),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(_) => Err(std::io::Error::other(
                scripted_handle_error("use scripted directory"),
            )),
        },
    }
}

fn object_kind(metadata: &cap_std::fs::Metadata) -> SandboxObjectKind {
    let file_type = metadata.file_type();
    file_type_kind(&file_type)
}

fn file_type_kind(file_type: &cap_std::fs::FileType) -> SandboxObjectKind {
    if file_type.is_dir() {
        SandboxObjectKind::Directory
    } else if file_type.is_symlink() {
        SandboxObjectKind::Symlink
    } else {
        SandboxObjectKind::File
    }
}

fn attributes(metadata: cap_std::fs::Metadata) -> std::io::Result<SandboxAttributes> {
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

fn time_spec(change: SandboxTimeChange, now: SystemTime) -> Option<SystemTimeSpec> {
    match change {
        SandboxTimeChange::Keep => None,
        SandboxTimeChange::Now => Some(SystemTimeSpec::Absolute(now)),
        SandboxTimeChange::Set(time) => Some(SystemTimeSpec::Absolute(time)),
    }
}

fn cap_time_spec(spec: SystemTimeSpec) -> cap_fs_ext::SystemTimeSpec {
    match spec {
        SystemTimeSpec::Absolute(time) => {
            cap_fs_ext::SystemTimeSpec::Absolute(cap_std::time::SystemTime::from_std(time))
        }
        SystemTimeSpec::SymbolicNow => cap_fs_ext::SystemTimeSpec::SymbolicNow,
    }
}

fn task_error(
    operation: &'static str,
    path: &Path,
    error: NativeExecutionError,
) -> FilesystemStorageError {
    FilesystemStorageError::task_failure(operation, path, error)
}

pub(super) fn namespace_operation_error(
    operation: &'static str,
    destination_path: &Path,
    error: std::io::Error,
) -> FilesystemStorageError {
    FilesystemStorageError::io(operation, destination_path, error)
}
