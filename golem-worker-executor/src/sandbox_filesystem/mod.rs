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

use crate::host_directories::HostPath;
use bytes::Bytes;
use std::ffi::OsString;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

mod errors;
mod host_adapter;
mod name_comparison;
mod object_identity;
mod open_handles;
mod sandbox_paths;
#[cfg(test)]
mod scripted_adapter;

pub(crate) use name_comparison::SandboxNameComparisonKey;
pub(crate) use object_identity::{SandboxDirectoryId, SandboxObjectId};
pub(crate) use open_handles::{SandboxDirectory, SandboxFile, SandboxHandle};
pub(crate) use sandbox_paths::{SandboxPath, SandboxResolvedPathTarget};
#[cfg(test)]
pub(crate) use scripted_adapter::*;

pub use errors::FilesystemStorageError;
pub(crate) use host_adapter::{
    SandboxFilesystem, SandboxFilesystemName, SandboxFilesystemProvisioning, TreeExclusions,
};

use crate::storage_volume::{FilesystemAllocation, FilesystemLimits};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InstalledLimits {
    pub limits: FilesystemLimits,
    pub allocation: FilesystemAllocation,
}

/// The kind discovered for a filesystem object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxObjectKind {
    File,
    Directory,
    Symlink,
}

/// Whether an operation may read, write, or both through an opened handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxAccessMode {
    Read,
    /// Reads, and carries the right that a change of the object's times needs. On a Unix platform
    /// that is an ordinary read open, because the change goes through the descriptor and needs
    /// nothing more. On Windows `SetFileTime` needs `FILE_WRITE_ATTRIBUTES`, which the rights of
    /// a read open do not include.
    ReadAndSetTimes,
    Write,
    ReadWrite,
}

/// One item for [`SandboxFilesystemAdapter::seed`].
///
/// The entry does not own what is at its source. The source must stay until the seed call that
/// takes the entry returns.
#[derive(Clone, Debug)]
pub(crate) struct SeedEntry {
    /// The host object that goes into the sandbox.
    pub source: HostPath,
    /// Where the object goes in the sandbox.
    pub target: SandboxPath,
    /// The write permission of the files that the entry makes.
    pub access: SeedAccess,
    /// Where the entry puts the source: at a path that holds nothing, or in place of what is there.
    pub placement: SeedPlacement,
}

/// The write permission of the files that a seed entry makes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeedAccess {
    /// A file keeps the permissions of its source.
    FromSource,
    /// A file gets the permissions of its source without write permission.
    ReadOnly,
    /// A file gets the permissions of its source with write permission for its owner.
    ReadWrite,
}

/// Where a seed entry puts its source, and what it does with a target path that is already there.
///
/// A directory entry merges into a directory that is already at its target. Below it, each path
/// that is already there follows the placement, unless that path is a directory in both the source
/// and the sandbox: such a directory merges too. A file where the source has a directory, or a
/// directory where the source has a file, is a path that is already there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeedPlacement {
    /// The entry puts the source at a path that holds nothing. At the first path that is already
    /// there, the entry gives an `AlreadyExists` error.
    CreateNew,
    /// The source takes the place of what is at the path. What is there goes away, together with
    /// all that is under it.
    Replace,
}

/// The names of one regular file or symlink that has more than one name under a copied path.
///
/// The names are relative to the copied path. `first` is the name that the copy holds. `others`
/// are the other names, in the order that the copy met them, and the copy does not hold them.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LinkGroup {
    pub first: Box<Path>,
    pub others: Box<[Box<Path>]>,
}

/// Whether path resolution follows the final symlink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxFollow {
    Yes,
    No,
}

/// The creation or truncation behavior for a file open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxFileDisposition {
    CreateIfMissing,
    CreateExclusive,
    TruncateExisting,
    CreateOrTruncate,
}

/// The requested behavior of [`SandboxFilesystemAdapter::open`].
///
/// `Existing` opens without changing the namespace. Its `expected` field is the kind required by
/// the caller; the adapter discovers the actual kind and rejects a mismatch. `File` is for opens
/// that may create or truncate a regular file and must not be used for directories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxOpenOptions {
    /// Read-only, no-follow inspection of one path component, rejecting special files.
    Inspection { expected: SandboxObjectKind },
    Existing {
        expected: SandboxObjectKind,
        access: SandboxAccessMode,
        follow: SandboxFollow,
    },
    File {
        access: SandboxAccessMode,
        disposition: SandboxFileDisposition,
        follow: SandboxFollow,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SandboxInspectionFailure {
    Symlink,
    NotRegular,
}

impl Display for SandboxInspectionFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Symlink => "inspection target is a symlink",
            Self::NotRegular => "inspection target has the wrong file type",
        })
    }
}

impl std::error::Error for SandboxInspectionFailure {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SandboxReadRange {
    pub offset: u64,
    pub length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxWritePlacement {
    At(u64),
    Append,
}

#[derive(Debug)]
pub(crate) struct SandboxWriteAttempt {
    pub written: u64,
    pub result: Result<(), FilesystemStorageError>,
}

impl SandboxWriteAttempt {
    /// Records a successful native write of `written` bytes.
    pub(crate) fn completed(written: u64) -> Self {
        Self {
            written,
            result: Ok(()),
        }
    }

    /// Records a failed native write and any prefix completed before the error.
    pub(crate) fn failed(written: u64, error: FilesystemStorageError) -> Self {
        Self {
            written,
            result: Err(error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SandboxDirectoryEntry {
    pub name: OsString,
    pub kind: SandboxObjectKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SandboxSymlinkTarget(pub PathBuf);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SandboxAttributes {
    pub kind: SandboxObjectKind,
    pub link_count: u64,
    pub size: u64,
    pub accessed: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    /// Whether the object has no write permission.
    pub read_only: bool,
    /// The identity of the object. All names of one object have the same identity.
    pub object: SandboxObjectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxTimeChange {
    Keep,
    Now,
    Set(SystemTime),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SandboxTimeChanges {
    pub accessed: SandboxTimeChange,
    pub modified: SandboxTimeChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SandboxFlushLevel {
    Data,
    DataAndMetadata,
}

/// A cloneable allocation observer that can outlive a borrow of its sandbox filesystem.
pub(crate) trait SandboxFilesystemAllocationReader: Clone + Send + Sync + 'static {
    /// Reads current authoritative allocated bytes and filesystem-object usage.
    fn read_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send;
}

pub(crate) struct DeleteError<Adapter> {
    pub(in crate::sandbox_filesystem) adapter: Adapter,
    pub(in crate::sandbox_filesystem) source: FilesystemStorageError,
}

impl<Adapter> DeleteError<Adapter> {
    pub(crate) fn new(adapter: Adapter, source: FilesystemStorageError) -> Self {
        Self { adapter, source }
    }

    pub(crate) fn into_parts(self) -> (Adapter, FilesystemStorageError) {
        (self.adapter, self.source)
    }

    pub(crate) fn into_source(self) -> FilesystemStorageError {
        self.source
    }
}

impl<Adapter> Debug for DeleteError<Adapter> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeleteError")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// Capability-confined filesystem operations within one sandbox.
///
/// Path methods accept [`SandboxPath`] and discover the object kind themselves. Methods that accept
/// [`SandboxFile`], [`SandboxDirectory`], or [`SandboxHandle`] operate on a handle returned by `open` and
/// remain valid after its path is renamed or unlinked.
///
/// No path operation provides ambient access to the executor node. [`Self::seed`] and
/// [`Self::copy_contents`] are the asymmetric operations: seed reads host paths and copy_contents
/// writes into a host path, while sandbox paths stay confined to this filesystem.
pub(crate) trait SandboxFilesystemAdapter: Send + Sync + 'static {
    /// Backend-specific configuration consumed when a fresh filesystem is created.
    type Provisioning: Send + 'static;
    /// A detached reader used by periodic allocation metering.
    type AllocationReader: SandboxFilesystemAllocationReader;

    /// Creates an empty, exclusively owned runtime filesystem and optionally installs limits.
    ///
    /// Use this once at the start of a filesystem generation. A failure must not expose a partially
    /// initialized filesystem to the caller.
    fn create_fresh(
        provisioning: Self::Provisioning,
        name: SandboxFilesystemName,
        limits: Option<FilesystemLimits>,
    ) -> impl Future<Output = Result<Self, FilesystemStorageError>> + Send
    where
        Self: Sized;

    /// Opens a path and returns the discovered typed node.
    ///
    /// Use `Existing` when the object must already exist and `File` when the operation may create
    /// or truncate a regular file. `Existing::expected` states what the caller requested. The
    /// adapter reads the actual kind and rejects a mismatch.
    fn open(
        &self,
        target: SandboxPath,
        options: SandboxOpenOptions,
    ) -> impl Future<Output = Result<SandboxHandle, FilesystemStorageError>> + Send;

    /// Resolves a path to a pinned parent plus opaque identity and name-coordination facts.
    ///
    /// Use this before coordinated namespace edits or identity-based policy checks. Execute the
    /// eventual operation against [`SandboxResolvedPathTarget::target`] to retain the pinned
    /// parent capability.
    fn resolve_path_target(
        &self,
        target: SandboxPath,
    ) -> impl Future<Output = Result<SandboxResolvedPathTarget, FilesystemStorageError>> + Send;

    /// Reads up to `range.length` bytes from an open file at `range.offset`.
    fn read(
        &self,
        file: &SandboxFile,
        range: SandboxReadRange,
    ) -> impl Future<Output = Result<Bytes, FilesystemStorageError>> + Send;

    /// Lists the immediate entries of an open directory.
    fn read_directory(
        &self,
        directory: &SandboxDirectory,
    ) -> impl Future<Output = Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>> + Send;

    /// Reads a symlink's stored target without following the final symlink.
    fn read_link(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<SandboxSymlinkTarget, FilesystemStorageError>> + Send;

    /// Attempts one positioned or append write through an open file.
    ///
    /// The result preserves partial progress so the caller can settle written bytes before it
    /// decides whether retrying is safe. Append callers must hold the file's append coordinator.
    fn write(
        &self,
        file: &SandboxFile,
        placement: SandboxWritePlacement,
        bytes: Bytes,
    ) -> impl Future<Output = Result<SandboxWriteAttempt, FilesystemStorageError>> + Send;

    /// Reads attributes from an already-open file or directory.
    ///
    /// Use this when descriptor identity matters, including after rename or unlink. The node is the
    /// opaque result of `open`; the caller does not choose its file or directory representation.
    fn get_handle_attributes(
        &self,
        node: SandboxHandle,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send;

    /// Resolves a path and reads attributes for whichever object is present.
    ///
    /// [`SandboxAttributes::kind`] reports the discovered file, directory, or symlink kind. `follow`
    /// controls only whether the final symlink itself or its target is inspected.
    fn get_path_attributes(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send;

    /// Reports whether two handles returned by `open` refer to the same filesystem object.
    ///
    /// This deliberately accepts handles rather than paths: paths can be renamed or unlinked after
    /// opening, while descriptor identity remains stable.
    fn is_same_open_object(
        &self,
        left: SandboxHandle,
        right: SandboxHandle,
    ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send;

    /// Changes the length of an open file.
    fn set_size(
        &self,
        file: &SandboxFile,
        size: u64,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Changes timestamps through an already-open file or directory.
    fn set_handle_times(
        &self,
        node: SandboxHandle,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Resolves a path and changes timestamps on the discovered object.
    ///
    /// `follow` chooses whether a final symlink or its target receives the change.
    fn set_path_times(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Creates an empty directory at a capability-relative path.
    fn create_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Creates a symlink at `path` whose stored contents are `target`.
    fn create_symlink(
        &self,
        path: SandboxPath,
        target: SandboxSymlinkTarget,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Creates a hard link from `source` to `destination`.
    ///
    /// A symlink at `source` is not followed, so the new name refers to the symlink itself. This is
    /// the POSIX `linkat` behaviour without `AT_SYMLINK_FOLLOW`.
    fn hard_link(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Renames or moves `source` to `destination` where the host permits it.
    fn rename(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Removes an empty directory at a capability-relative path.
    fn remove_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Unlinks a file or symlink without following the final symlink.
    fn unlink_file(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Flushes an open node's data, or its data and metadata, to stable storage.
    fn flush(
        &self,
        node: &SandboxHandle,
        level: SandboxFlushLevel,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Closes one owned open node.
    ///
    /// Closing the final handle to an unlinked file may free allocated storage.
    fn close(
        &self,
        node: SandboxHandle,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Puts host content into this sandbox, one entry after the other. The opposite of
    /// copy_contents.
    ///
    /// Each source is read without following a symlink. A file becomes one file, a directory
    /// brings all that is under it, and a symlink becomes a symlink. Directories and symlinks are
    /// made again, and permissions and modification times are copied. `access` sets the write
    /// permission of seeded files, or keeps the permissions of the source. `placement` decides
    /// what happens to a target path that is already there. Each file is a copy-on-write copy on
    /// XFS or Apfs storage, and a byte copy on other storage. With project quotas the reflink goes
    /// into the project of this filesystem, so the quota is charged here, and EDQUOT and ENOSPC
    /// appear here.
    /// Nothing is written outside the sandbox root. The first entry that fails stops the call. The
    /// entries before it stay. Linux XFS seeds synchronize the volume after the entries. macOS
    /// seeds fsync each file and affected directory, then request one full device flush through an
    /// unlinked regular descriptor owned by the filesystem. Both finalizers run after an entry
    /// failure. A synchronization failure is an error, without a weaker fallback.
    fn seed(
        &self,
        entries: Box<[SeedEntry]>,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Reads authoritative allocation for a quota-managed filesystem.
    ///
    /// This fails when the backend has no per-filesystem allocation authority.
    fn observe_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send;

    /// Returns a cloneable reader for periodic allocation sampling.
    fn allocation_reader(&self) -> Self::AllocationReader;

    /// Installs byte and filesystem-object limits and reports their effective values.
    fn install_limits(
        &self,
        limits: FilesystemLimits,
    ) -> impl Future<Output = Result<InstalledLimits, FilesystemStorageError>> + Send;

    /// Takes a stable copy of the contents under `source` out of this sandbox into `target`,
    /// minus the paths in `excluded`. The opposite of seed.
    ///
    /// `target` must be an empty directory, or the call fails. The caller decides what to leave
    /// out. The sandbox applies the set and nothing else. The set is shared and already
    /// normalized, so a copy does no work per excluded path before the walk. Directories and
    /// symlinks are made again. Permissions and modification times are copied, and `target` gets
    /// the permissions and the modification time of `source`. On XFS, with or without project
    /// quotas, each file is one reflink: the cost follows the number of files, not the bytes, and
    /// the copy adds nothing to any quota, because a HostPath is outside every agent project.
    /// Apfs also makes copy-on-write copies. Other storage makes byte copies.
    ///
    /// A regular file or a symlink with more than one name under `source` is copied once, at the
    /// first name that the copy meets, and the result gives its names as one [`LinkGroup`].
    /// `excluded` leaves out paths, not objects: the other names of an object with an excluded
    /// name are copied as usual, and an excluded name is in no group.
    fn copy_contents(
        &self,
        source: SandboxPath,
        excluded: Arc<TreeExclusions>,
        target: &HostPath,
    ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send;

    /// Deletes the runtime filesystem and verifies its absence.
    ///
    /// This consumes exclusive ownership. On failure, [`DeleteError`] returns that ownership so the
    /// cleanup owner can retry without releasing the filesystem's exclusive lease.
    fn delete_and_verify(self) -> impl Future<Output = Result<(), DeleteError<Self>>> + Send
    where
        Self: Sized;
}
