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
use bytes::Bytes;
use cap_fs_ext::{FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::FileExt as _;
use fs_set_times::{SetTimes as _, SystemTimeSpec};
use std::ffi::OsString;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::{Seek, SeekFrom, Write};
use std::time::SystemTime;

#[cfg(windows)]
fn windows_directory_is_case_sensitive(directory: &cap_std::fs::Dir) -> std::io::Result<bool> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_CASE_SENSITIVE_INFO, FileCaseSensitiveInfo, GetFileInformationByHandleEx,
    };

    let mut information = MaybeUninit::<FILE_CASE_SENSITIVE_INFO>::zeroed();
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle() as _,
            FileCaseSensitiveInfo,
            information.as_mut_ptr().cast(),
            size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(code) if code == ERROR_INVALID_PARAMETER as i32 || code == ERROR_NOT_SUPPORTED as i32
        ) {
            return Ok(false);
        }
        return Err(error);
    }
    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 1;
    Ok(unsafe { information.assume_init() }.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
}

#[cfg(windows)]
fn windows_names_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let left = left.encode_wide().collect::<Vec<_>>();
    let right = right.encode_wide().collect::<Vec<_>>();
    unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        ) == CSTR_EQUAL
    }
}

#[cfg(all(test, not(windows)))]
fn windows_names_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[derive(Clone)]
pub(crate) struct SandboxFile {
    handle: NativeFileHandle,
    path: PathBuf,
    append_coordination: NativeAppendCoordination,
}

#[derive(Clone)]
enum NativeFileHandle {
    Host(Arc<cap_std::fs::File>),
    #[cfg(test)]
    Scripted(u64),
}

#[derive(Clone)]
struct NativeAppendCoordination {
    identity: NativeFileIdentity,
    coordinators: Arc<AppendCoordinatorRegistry>,
    #[cfg(test)]
    benchmark_eager: Option<Arc<AsyncMutex<()>>>,
}

impl NativeAppendCoordination {
    fn new(identity: NativeFileIdentity, coordinators: Arc<AppendCoordinatorRegistry>) -> Self {
        #[cfg(test)]
        let benchmark_eager = benchmark_eager_append_coordination()
            .then(|| coordinators.coordinator(identity.clone()));
        Self {
            identity,
            coordinators,
            #[cfg(test)]
            benchmark_eager,
        }
    }
}

#[cfg(test)]
fn benchmark_eager_append_coordination() -> bool {
    static EAGER: OnceLock<bool> = OnceLock::new();
    *EAGER.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION").as_deref() == Ok("1")
    })
}

impl Debug for SandboxFile {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.handle {
            NativeFileHandle::Host(_) => formatter
                .debug_tuple("file")
                .field(&self.path.display().to_string())
                .finish(),
            #[cfg(test)]
            NativeFileHandle::Scripted(id) => formatter.debug_tuple("file").field(id).finish(),
        }
    }
}

impl SandboxFile {
    fn host(&self) -> Result<&Arc<cap_std::fs::File>, FilesystemStorageError> {
        match &self.handle {
            NativeFileHandle::Host(file) => Ok(file),
            #[cfg(test)]
            NativeFileHandle::Scripted(_) => Err(scripted_handle_error("use scripted file")),
        }
    }

    /// Acquires the per-object lock required around a complete append operation.
    ///
    /// Callers use this only for append placement. Positioned writes do not need coordination.
    pub(crate) async fn coordinate_append(&self) -> OwnedMutexGuard<()> {
        let guard = self.append_coordinator().lock_owned().await;
        #[cfg(test)]
        self.append_coordination
            .coordinators
            .record_lock_acquisition();
        guard
    }

    fn append_coordinator(&self) -> Arc<AsyncMutex<()>> {
        #[cfg(test)]
        if let Some(coordinator) = &self.append_coordination.benchmark_eager {
            return Arc::clone(coordinator);
        }
        self.append_coordination
            .coordinators
            .coordinator(self.append_coordination.identity.clone())
    }

    #[cfg(test)]
    pub(crate) fn append_coordination_counts(&self) -> AppendCoordinationCounts {
        self.append_coordination.coordinators.counts()
    }

    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self {
            handle: NativeFileHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-file-{id}>")),
            append_coordination: NativeAppendCoordination::new(
                NativeFileIdentity::Scripted(format!("scripted-file-{id}")),
                Arc::new(AppendCoordinatorRegistry::default()),
            ),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SandboxDirectory {
    handle: NativeDirectoryHandle,
    path: PathBuf,
    coordination_key: SandboxDirectoryCoordinationKey,
}

#[derive(Clone)]
enum NativeDirectoryHandle {
    Host(Arc<cap_std::fs::Dir>),
    #[cfg(test)]
    Scripted(u64),
}

impl Debug for SandboxDirectory {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.handle {
            NativeDirectoryHandle::Host(_) => formatter
                .debug_tuple("directory")
                .field(&self.path.display().to_string())
                .finish(),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(id) => {
                formatter.debug_tuple("directory").field(id).finish()
            }
        }
    }
}

impl SandboxDirectory {
    fn host(&self) -> Result<&Arc<cap_std::fs::Dir>, FilesystemStorageError> {
        match &self.handle {
            NativeDirectoryHandle::Host(directory) => Ok(directory),
            #[cfg(test)]
            NativeDirectoryHandle::Scripted(_) => {
                Err(scripted_handle_error("use scripted directory"))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self::scripted_with_identity(id, format!("directory-{id}"))
    }

    #[cfg(test)]
    fn scripted_with_identity(id: u64, identity: String) -> Self {
        Self {
            handle: NativeDirectoryHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-directory-{id}>")),
            coordination_key: SandboxDirectoryCoordinationKey(NativeFileIdentity::Scripted(
                identity,
            )),
        }
    }

    fn coordination_key(&self) -> SandboxDirectoryCoordinationKey {
        self.coordination_key.clone()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SandboxNode {
    File(SandboxFile),
    Directory(SandboxDirectory),
}

#[derive(Clone, Debug)]
pub(crate) struct SandboxOpened {
    node: SandboxNode,
}

impl SandboxOpened {
    /// Consumes the open result and returns its opaque file-or-directory handle.
    pub(crate) fn into_node(self) -> SandboxNode {
        self.node
    }

    /// Returns the opened directory's identity used for namespace coordination.
    ///
    /// File opens return `None`.
    pub(crate) fn directory_coordination_key(&self) -> Option<SandboxDirectoryCoordinationKey> {
        match &self.node {
            SandboxNode::Directory(directory) => Some(directory.coordination_key()),
            SandboxNode::File(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_file(id: u64) -> Self {
        Self {
            node: SandboxNode::File(SandboxFile::scripted(id)),
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_file_aliases(
        first_id: u64,
        second_id: u64,
        identity: u64,
    ) -> (Self, Self) {
        let coordinators = Arc::new(AppendCoordinatorRegistry::default());
        let identity = NativeFileIdentity::Scripted(format!("programmed-file-{identity}"));
        let file = |id, coordinators| SandboxFile {
            handle: NativeFileHandle::Scripted(id),
            path: PathBuf::from(format!("<scripted-file-{id}>")),
            append_coordination: NativeAppendCoordination::new(identity.clone(), coordinators),
        };
        (
            Self {
                node: SandboxNode::File(file(first_id, Arc::clone(&coordinators))),
            },
            Self {
                node: SandboxNode::File(file(second_id, coordinators)),
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn scripted_directory(id: u64) -> Self {
        Self {
            node: SandboxNode::Directory(SandboxDirectory::scripted(id)),
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_directory_with_identity(id: u64, identity: u64) -> Self {
        Self {
            node: SandboxNode::Directory(SandboxDirectory::scripted_with_identity(
                id,
                format!("programmed-directory-{identity}"),
            )),
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct SandboxDirectoryCoordinationKey(NativeFileIdentity);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum NativeNameComparisonMode {
    Exact,
    Conservative,
    #[cfg(any(test, windows))]
    WindowsInsensitive,
}

#[derive(Clone)]
struct NativeNameCoordinationKey {
    name: OsString,
    mode: NativeNameComparisonMode,
}

impl PartialEq for NativeNameCoordinationKey {
    fn eq(&self, other: &Self) -> bool {
        match (self.mode, other.mode) {
            (NativeNameComparisonMode::Exact, NativeNameComparisonMode::Exact) => {
                self.name == other.name
            }
            (NativeNameComparisonMode::Conservative, NativeNameComparisonMode::Conservative) => {
                true
            }
            #[cfg(any(test, windows))]
            (
                NativeNameComparisonMode::WindowsInsensitive,
                NativeNameComparisonMode::WindowsInsensitive,
            ) => windows_names_equal(&self.name, &other.name),
            _ => false,
        }
    }
}

impl Eq for NativeNameCoordinationKey {}

impl Hash for NativeNameCoordinationKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mode.hash(state);
        if self.mode == NativeNameComparisonMode::Exact {
            self.name.hash(state);
        }
    }
}

impl NativeNameCoordinationKey {
    fn may_be_equivalent(&self, other: &Self) -> bool {
        match (self.mode, other.mode) {
            (NativeNameComparisonMode::Exact, NativeNameComparisonMode::Exact) => {
                self.name == other.name
            }
            (NativeNameComparisonMode::Conservative, _)
            | (_, NativeNameComparisonMode::Conservative) => true,
            #[cfg(any(test, windows))]
            (
                NativeNameComparisonMode::WindowsInsensitive,
                NativeNameComparisonMode::WindowsInsensitive,
            ) => windows_names_equal(&self.name, &other.name),
            #[cfg(any(test, windows))]
            _ => true,
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct SandboxNamespaceCoordinationKey {
    parent: SandboxDirectoryCoordinationKey,
    name: NativeNameCoordinationKey,
}

impl SandboxNamespaceCoordinationKey {
    /// Reports whether two resolved names may denote the same entry in one parent directory.
    ///
    /// Conservative name modes return conflicts rather than risk concurrent aliasing edits.
    pub(crate) fn may_conflict_with(&self, other: &Self) -> bool {
        self.parent == other.parent && self.name.may_be_equivalent(&other.name)
    }

    #[cfg(test)]
    pub(crate) fn scripted_exact(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(parent, name.into(), NativeNameComparisonMode::Exact)
    }

    #[cfg(test)]
    pub(crate) fn scripted_conservative(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(parent, name.into(), NativeNameComparisonMode::Conservative)
    }

    #[cfg(test)]
    fn scripted_windows_insensitive(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(
            parent,
            name.into(),
            NativeNameComparisonMode::WindowsInsensitive,
        )
    }

    #[cfg(test)]
    fn scripted(parent: u64, name: OsString, mode: NativeNameComparisonMode) -> Self {
        Self {
            parent: SandboxDirectoryCoordinationKey(NativeFileIdentity::Scripted(format!(
                "comparison-parent-{parent}"
            ))),
            name: NativeNameCoordinationKey { name, mode },
        }
    }
}

impl PartialEq<SandboxDirectoryCoordinationKey> for SandboxNamespaceCoordinationKey {
    fn eq(&self, other: &SandboxDirectoryCoordinationKey) -> bool {
        self.parent == *other
    }
}

impl PartialEq<SandboxNamespaceCoordinationKey> for SandboxDirectoryCoordinationKey {
    fn eq(&self, other: &SandboxNamespaceCoordinationKey) -> bool {
        other == self
    }
}

#[derive(Clone)]
pub(crate) struct SandboxResolvedNamespaceTarget {
    parent: SandboxDirectory,
    name: OsString,
    coordination_key: SandboxNamespaceCoordinationKey,
    final_directory_key: Option<SandboxDirectoryCoordinationKey>,
    read_only_file: bool,
    followed_read_only_file: NativeReadOnlyResolution,
}

#[derive(Clone)]
enum NativeReadOnlyResolution {
    Resolved(bool),
    Failed {
        kind: std::io::ErrorKind,
        raw_os_error: Option<i32>,
        message: String,
    },
}

impl SandboxResolvedNamespaceTarget {
    /// Returns the semantic key used to coordinate access to this namespace entry.
    pub(crate) fn coordination_key(&self) -> SandboxNamespaceCoordinationKey {
        self.coordination_key.clone()
    }

    /// Returns the identity of the final directory when the resolved entry is a directory.
    pub(crate) fn final_directory_key(&self) -> Option<SandboxDirectoryCoordinationKey> {
        self.final_directory_key.clone()
    }

    /// Rebuilds a path relative to the parent capability pinned during resolution.
    ///
    /// Namespace operations should use this instead of resolving the original ambient path again.
    pub(crate) fn target(&self) -> SandboxPath {
        SandboxPath::at(self.parent.clone(), PathBuf::from(&self.name))
    }

    /// Tells whether the resolved object is a regular file without write permission.
    ///
    /// `follow` selects the final symlink or its referent. A missing object is not a read-only
    /// file. A failure to read the referent gives that failure as a storage error.
    pub(crate) fn is_read_only_file(
        &self,
        follow: SandboxFollow,
    ) -> Result<bool, FilesystemStorageError> {
        match (follow, &self.followed_read_only_file) {
            (SandboxFollow::No, _) => Ok(self.read_only_file),
            (SandboxFollow::Yes, NativeReadOnlyResolution::Resolved(read_only)) => Ok(*read_only),
            (
                SandboxFollow::Yes,
                NativeReadOnlyResolution::Failed {
                    kind,
                    raw_os_error,
                    message,
                },
            ) => {
                let source = raw_os_error.map_or_else(
                    || std::io::Error::new(*kind, message.clone()),
                    std::io::Error::from_raw_os_error,
                );
                Err(FilesystemStorageError::io(
                    "resolve sandbox filesystem target permissions",
                    &self.parent.path.join(&self.name),
                    source,
                ))
            }
        }
    }
}

/// A path resolved relative to either the filesystem root or an open directory.
///
/// This never denotes an ambient host path. Absolute paths and parent traversal cannot grant
/// access outside the selected root or directory capability.
///
/// Use [`SandboxPath::at_root`] for root-relative operations. Use [`SandboxPath::at`] when the
/// operation must remain relative to an exact open directory even if its namespace entry moves or
/// is replaced.
#[derive(Clone, Debug)]
pub(crate) struct SandboxPath {
    base: SandboxPathBase,
    path: PathBuf,
}

#[derive(Clone, Debug)]
enum SandboxPathBase {
    Root,
    Directory(SandboxDirectory),
}

impl SandboxPath {
    /// Creates a path relative to the runtime filesystem root.
    pub(crate) fn at_root(path: impl Into<PathBuf>) -> Self {
        Self {
            base: SandboxPathBase::Root,
            path: path.into(),
        }
    }

    /// Creates a path relative to an already-open directory capability.
    ///
    /// Use this for descriptor-relative operations that must survive rename or path replacement.
    pub(crate) fn at(directory: SandboxDirectory, path: impl Into<PathBuf>) -> Self {
        Self {
            base: SandboxPathBase::Directory(directory),
            path: path.into(),
        }
    }

    fn operation_path(&self, root: &Path) -> PathBuf {
        match &self.base {
            SandboxPathBase::Root => root.join(&self.path),
            SandboxPathBase::Directory(directory) => directory.path.join(&self.path),
        }
    }
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
    /// What the entry does with a target path that is already there.
    pub existing: OnExisting,
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

/// What a seed entry does with a target path that is already there.
///
/// A directory entry merges into a directory that is already at its target. Below it, each path
/// that is already there follows the rule, unless that path is a directory in both the source and
/// the sandbox: such a directory merges too. A file where the source has a directory, or a
/// directory where the source has a file, is a path that is already there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OnExisting {
    /// The entry fails with an `AlreadyExists` error at the first path that is already there.
    Fail,
    /// What is there goes away, together with all that is under it, and the source takes its
    /// place.
    Replace,
}

/// The names of one regular file that has more than one name under a copied path.
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
    /// The creation time of the object, where the filesystem records it.
    pub created: Option<SystemTime>,
    /// Whether the object has no write permission.
    pub read_only: bool,
    /// The identity of the object. All names of one object have the same identity.
    pub object: SandboxObjectId,
}

/// The identity of one filesystem object in a sandbox.
///
/// Two values are equal when they identify the same object. The identity of a deleted object can
/// identify a new object later.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SandboxObjectId(NativeFileIdentity);

impl SandboxObjectId {
    #[cfg(test)]
    pub(crate) fn scripted(id: u64) -> Self {
        Self(NativeFileIdentity::Scripted(format!(
            "programmed-object-{id}"
        )))
    }
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

/// Capability-confined filesystem operations within one sandbox.
///
/// Path methods accept [`SandboxPath`] and discover the object kind themselves. Methods that accept
/// [`SandboxFile`], [`SandboxDirectory`], or [`SandboxNode`] operate on a handle returned by `open` and
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
    ) -> impl Future<Output = Result<SandboxOpened, FilesystemStorageError>> + Send;

    /// Resolves a path to a pinned parent plus opaque identity and name-coordination facts.
    ///
    /// Use this before coordinated namespace edits or identity-based policy checks. Execute the
    /// eventual operation against [`SandboxResolvedNamespaceTarget::target`] to retain the pinned
    /// parent capability.
    fn resolve_namespace_target(
        &self,
        target: SandboxPath,
    ) -> impl Future<Output = Result<SandboxResolvedNamespaceTarget, FilesystemStorageError>> + Send;

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
    fn get_node_attributes(
        &self,
        node: SandboxNode,
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
        left: SandboxNode,
        right: SandboxNode,
    ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send;

    /// Changes the length of an open file.
    fn set_size(
        &self,
        file: &SandboxFile,
        size: u64,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Changes timestamps through an already-open file or directory.
    fn set_node_times(
        &self,
        node: SandboxNode,
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
        node: &SandboxNode,
        level: SandboxFlushLevel,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Closes one owned open node.
    ///
    /// Closing the final handle to an unlinked file may free allocated storage.
    fn close(
        &self,
        node: SandboxNode,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send;

    /// Puts host content into this sandbox, one entry after the other. The opposite of
    /// copy_contents.
    ///
    /// Each source is read without following a symlink. A file becomes one file, a directory
    /// brings all that is under it, and a symlink becomes a symlink. Directories and symlinks are
    /// made again, and permissions and modification times are copied. `access` sets the write
    /// permission of seeded files, or keeps the permissions of the source. `existing` decides
    /// what happens to a target path that is already there. On managed XFS each file is one
    /// reflink into the project of this filesystem, so the quota is charged here, and EDQUOT and
    /// ENOSPC appear here. On unmanaged storage each file is copied. Nothing is written outside
    /// the sandbox root. The first entry that fails stops the call. The entries before it stay.
    /// On managed XFS, what the call wrote is on stable storage when the call returns, also when
    /// an entry fails.
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
    /// symlinks are made again. Permissions and modification times are copied. On managed XFS
    /// each file is one reflink: the cost follows the number of files, not the bytes, and the
    /// copy adds nothing to any quota, because a HostPath is outside every agent project. On
    /// unmanaged storage each file is copied.
    ///
    /// A regular file with more than one name under `source` is copied once, at the first name
    /// that the copy meets, and the result gives its names as one [`LinkGroup`]. `excluded` leaves
    /// out paths, not files: the other names of a file with an excluded name are copied as usual,
    /// and an excluded name is in no group.
    fn copy_contents(
        &self,
        source: SandboxPath,
        excluded: Arc<TreeExclusions>,
        target: &HostPath,
    ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send;

    /// Consumes exclusive ownership, deletes the runtime filesystem, and verifies its absence.
    fn delete_and_verify(self) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send
    where
        Self: Sized;
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
        let filesystem = Arc::try_unwrap(filesystem).map_err(|filesystem| {
            FilesystemStorageError::verification(
                "take exclusive ownership of fresh sandbox filesystem",
                filesystem.root(),
            )
        })?;
        if let Some(limits) = limits
            && let Err(error) = SandboxFilesystem::install_limits(&filesystem, limits).await
        {
            return Err(
                match SandboxFilesystem::delete_and_verify(&filesystem).await {
                    Ok(()) => error,
                    Err(cleanup_error) => cleanup_error,
                },
            );
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
        let storage_profile = self.storage_profile();
        let root_directory = self.root_directory_state();
        async move {
            execute_native(storage_profile, NativeOperation::Open, move || {
                let directory = directory_for(&root_directory, &target)?;
                let mut native_options = cap_std::fs::OpenOptions::new();
                native_options.maybe_dir(true);
                let (expected, access, follow, disposition) = match options {
                    SandboxOpenOptions::Existing {
                        expected,
                        access,
                        follow,
                    } => (expected, access, follow, None),
                    SandboxOpenOptions::File {
                        access,
                        disposition,
                        follow,
                    } => (SandboxObjectKind::File, access, follow, Some(disposition)),
                };
                if expected == SandboxObjectKind::Directory {
                    native_options.read(true);
                } else {
                    match access {
                        SandboxAccessMode::Read => {
                            native_options.read(true);
                        }
                        SandboxAccessMode::Write => {
                            native_options.write(true);
                        }
                        SandboxAccessMode::ReadWrite => {
                            native_options.read(true).write(true);
                        }
                    }
                }
                match disposition {
                    None => {}
                    Some(SandboxFileDisposition::CreateIfMissing) => {
                        native_options.create(true).write(true);
                    }
                    Some(SandboxFileDisposition::CreateExclusive) => {
                        native_options.create_new(true).write(true);
                    }
                    Some(SandboxFileDisposition::TruncateExisting) => {
                        native_options.truncate(true).write(true);
                    }
                    Some(SandboxFileDisposition::CreateOrTruncate) => {
                        native_options.create(true).truncate(true).write(true);
                    }
                }
                native_options.follow(match follow {
                    SandboxFollow::Yes => FollowSymlinks::Yes,
                    SandboxFollow::No => FollowSymlinks::No,
                });
                let opened = directory.open_with(&target.path, &native_options)?;
                let metadata = opened.metadata()?;
                let kind = object_kind(&metadata);
                if kind != expected {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("expected {expected:?}, opened {kind:?}"),
                    ));
                }
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
                Ok(SandboxOpened { node })
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
                        let count = file.read_at(&mut buffer[read..], offset)?;
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
                        SandboxWritePlacement::At(offset) => file.write_at(&bytes, offset),
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let storage_profile = self.storage_profile();
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
        let mode = self.file_copy_mode;
        let quota_authority = self.quota_authority;
        let storage_profile = self.storage_profile();
        async move {
            let error_path = Arc::clone(&materialization_root);
            execute_native(storage_profile, NativeOperation::TreeCopy, move || {
                let seeded = entries.iter().try_for_each(|entry| {
                    let context = tree_copy::SeedContext {
                        mode,
                        quota_authority,
                        access: entry.access,
                        existing: entry.existing,
                    };
                    directory_for(&root_directory, &entry.target)
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
                });
                let synced =
                    tree_copy::sync_after_reflink(mode, &materialization_root).map_err(|error| {
                        FilesystemStorageError::io(
                            "sync seeded sandbox filesystem",
                            &materialization_root,
                            error,
                        )
                    });
                seeded.and(synced)
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
        let operation_path = source.operation_path(self.root());
        let root_directory = self.root_directory_state();
        let copy_mode = self.file_copy_mode;
        let storage_profile = self.storage_profile();
        let target = target.clone();
        async move {
            execute_native(storage_profile, NativeOperation::TreeCopy, move || {
                let base = directory_for(&root_directory, &source)?;
                tree_copy::copy_contents(
                    &base,
                    &source.path,
                    &excluded,
                    target.as_path(),
                    copy_mode,
                )
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

    async fn delete_and_verify(self) -> Result<(), FilesystemStorageError> {
        SandboxFilesystem::delete_and_verify(&self).await
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
    fn root_directory_state(&self) -> Arc<Mutex<Option<Arc<cap_std::fs::Dir>>>> {
        Arc::clone(&self.root.directory)
    }
}

fn node_path(node: &SandboxNode) -> PathBuf {
    match node {
        SandboxNode::File(file) => file.path.clone(),
        SandboxNode::Directory(directory) => directory.path.clone(),
    }
}

fn directory_for(
    root: &Arc<Mutex<Option<Arc<cap_std::fs::Dir>>>>,
    target: &SandboxPath,
) -> std::io::Result<Arc<cap_std::fs::Dir>> {
    match &target.base {
        SandboxPathBase::Directory(directory) => {
            directory.host().cloned().map_err(std::io::Error::other)
        }
        SandboxPathBase::Root => {
            let directory = {
                let root = root
                    .lock()
                    .expect("sandbox filesystem root descriptor lock poisoned");
                Arc::clone(root.as_ref().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "sandbox filesystem root has been released",
                    )
                })?)
            };
            #[cfg(test)]
            if benchmark_disable_root_capability_reuse() {
                return directory.try_clone().map(Arc::new);
            }
            Ok(directory)
        }
    }
}

#[cfg(test)]
fn benchmark_disable_root_capability_reuse() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE").as_deref()
            == Ok("1")
    })
}

fn resolve_host_namespace_target(
    base: Arc<cap_std::fs::Dir>,
    target: SandboxPath,
    parent_path: PathBuf,
    name_mode_source: NativeNameModeSource,
    name_mode_probe: NativeNameModeProbe,
) -> std::io::Result<SandboxResolvedNamespaceTarget> {
    let (relative_parent, name) = split_namespace_target(&target.path)?;
    let parent = if relative_parent.as_os_str().is_empty() {
        base
    } else {
        Arc::new(base.open_dir(relative_parent)?)
    };
    let parent_key = native_directory_coordination_key(&parent)?;
    let name_mode =
        native_name_comparison_mode(&parent, &parent_key, name_mode_source, &name_mode_probe)?;
    let coordination_key = SandboxNamespaceCoordinationKey {
        parent: parent_key,
        name: NativeNameCoordinationKey {
            name: name.clone(),
            mode: name_mode,
        },
    };
    let (final_directory_key, read_only_file, followed_read_only_file) =
        match parent.symlink_metadata(&name) {
            Ok(metadata) => {
                let directory = (metadata.is_dir() && !metadata.file_type().is_symlink())
                    .then(|| native_file_identity(&metadata).map(SandboxDirectoryCoordinationKey))
                    .transpose()?;
                let read_only_file = is_read_only_file(&metadata);
                let followed = if metadata.file_type().is_symlink() {
                    match parent.metadata(&name) {
                        Ok(metadata) => {
                            NativeReadOnlyResolution::Resolved(is_read_only_file(&metadata))
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            NativeReadOnlyResolution::Resolved(false)
                        }
                        Err(error) => NativeReadOnlyResolution::Failed {
                            kind: error.kind(),
                            raw_os_error: error.raw_os_error(),
                            message: error.to_string(),
                        },
                    }
                } else {
                    NativeReadOnlyResolution::Resolved(read_only_file)
                };
                (directory, read_only_file, followed)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (None, false, NativeReadOnlyResolution::Resolved(false))
            }
            Err(error) => return Err(error),
        };
    Ok(SandboxResolvedNamespaceTarget {
        parent: SandboxDirectory {
            handle: NativeDirectoryHandle::Host(parent),
            path: parent_path,
            coordination_key: coordination_key.parent.clone(),
        },
        name,
        coordination_key,
        final_directory_key,
        read_only_file,
        followed_read_only_file,
    })
}

/// Tells whether `metadata` describes a regular file without write permission.
fn is_read_only_file(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_file() && metadata.permissions().readonly()
}

fn split_namespace_target(path: &Path) -> std::io::Result<(PathBuf, OsString)> {
    if path.as_os_str().is_empty() {
        return Ok((PathBuf::new(), OsString::from(".")));
    }
    let component = path.components().next_back().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "namespace target has no final component",
        )
    })?;
    let name = match component {
        std::path::Component::Normal(name) => name.to_os_string(),
        std::path::Component::CurDir => OsString::from("."),
        std::path::Component::ParentDir => OsString::from(".."),
        std::path::Component::Prefix(_) | std::path::Component::RootDir => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "namespace target must be relative to its directory capability",
            ));
        }
    };
    Ok((
        path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        name,
    ))
}

fn native_directory_coordination_key(
    directory: &cap_std::fs::Dir,
) -> std::io::Result<SandboxDirectoryCoordinationKey> {
    let metadata = directory.dir_metadata()?;
    Ok(SandboxDirectoryCoordinationKey(native_file_identity(
        &metadata,
    )?))
}

pub(super) fn native_file_identity(
    metadata: &cap_std::fs::Metadata,
) -> std::io::Result<NativeFileIdentity> {
    #[cfg(unix)]
    return Ok(NativeFileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    });
    #[cfg(windows)]
    return Ok(NativeFileIdentity::Windows {
        volume_serial_number: u32::try_from(metadata.dev()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "native volume serial number does not fit its opaque identity",
            )
        })?,
        file_index: metadata.ino(),
    });
    #[cfg(not(any(unix, windows)))]
    return Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native file identity is unavailable on this target",
    ));
}

fn native_name_comparison_mode(
    directory: &cap_std::fs::Dir,
    parent: &SandboxDirectoryCoordinationKey,
    source: NativeNameModeSource,
    probe: &NativeNameModeProbe,
) -> std::io::Result<NativeNameComparisonMode> {
    #[cfg(target_os = "linux")]
    {
        Ok(linux_name_comparison_mode(
            source,
            parent,
            managed_xfs_name_mode_shortcut_enabled(),
            || {
                probe.record();
                rustix::fs::ioctl_getflags(directory)
                    .ok()
                    .map(|flags| flags.bits())
            },
        ))
    }
    #[cfg(windows)]
    {
        let _ = (parent, source, probe);
        return Ok(if windows_directory_is_case_sensitive(directory)? {
            NativeNameComparisonMode::Exact
        } else {
            NativeNameComparisonMode::WindowsInsensitive
        });
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (directory, parent, source, probe);
        Ok(NativeNameComparisonMode::Conservative)
    }
}

#[cfg(target_os = "linux")]
const MANAGED_XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED: bool = true;

#[cfg(target_os = "linux")]
fn managed_xfs_name_mode_shortcut_enabled() -> bool {
    let enabled = MANAGED_XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED;
    #[cfg(test)]
    let enabled = enabled && !managed_xfs_name_mode_shortcut_disabled_for_test();
    enabled
}

#[cfg(all(test, target_os = "linux"))]
fn managed_xfs_name_mode_shortcut_disabled_for_test() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT").as_deref()
            == Ok("1")
    })
}

#[cfg(target_os = "linux")]
fn linux_name_comparison_mode(
    source: NativeNameModeSource,
    parent: &SandboxDirectoryCoordinationKey,
    shortcut_enabled: bool,
    detect_flags: impl FnOnce() -> Option<u32>,
) -> NativeNameComparisonMode {
    const FS_CASEFOLD_FL: u32 = 0x4000_0000;
    let parent_device = match &parent.0 {
        NativeFileIdentity::Unix { device, .. } => *device,
        #[allow(unreachable_patterns)]
        _ => return NativeNameComparisonMode::Conservative,
    };
    if shortcut_enabled
        && matches!(source, NativeNameModeSource::ValidatedManagedXfs(proof) if proof.matches_device(parent_device))
    {
        return NativeNameComparisonMode::Exact;
    }
    match detect_flags() {
        Some(flags) if flags & FS_CASEFOLD_FL == 0 => NativeNameComparisonMode::Exact,
        Some(_) | None => NativeNameComparisonMode::Conservative,
    }
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

enum HostNode {
    File(Arc<cap_std::fs::File>),
    Directory(Arc<cap_std::fs::Dir>),
}

impl HostNode {
    fn metadata(&self) -> std::io::Result<cap_std::fs::Metadata> {
        match self {
            Self::File(file) => file.metadata(),
            Self::Directory(directory) => directory.dir_metadata(),
        }
    }

    fn set_times(&self, times: SandboxTimeChanges) -> std::io::Result<()> {
        let now = SystemTime::now();
        let accessed = time_spec(times.accessed, now);
        let modified = time_spec(times.modified, now);
        match self {
            Self::File(file) => file.set_times(accessed, modified),
            Self::Directory(directory) => directory.set_times(accessed, modified),
        }
    }

    fn flush(&self, level: SandboxFlushLevel) -> std::io::Result<()> {
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
        created: metadata.created().ok().map(|time| time.into_std()),
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

fn namespace_operation_error(
    operation: &'static str,
    destination_path: &Path,
    error: std::io::Error,
) -> FilesystemStorageError {
    FilesystemStorageError::io(operation, destination_path, error)
}

#[cfg(test)]
fn scripted_handle_error(operation: &'static str) -> FilesystemStorageError {
    FilesystemStorageError::verification(operation, Path::new("<scripted-handle>"))
}

#[cfg(test)]
mod scripted {
    use super::*;
    use std::collections::HashMap;
    use std::collections::VecDeque;

    #[derive(Clone)]
    pub(crate) struct ScriptedSandboxFilesystem {
        state: Arc<Mutex<ScriptedState>>,
    }

    impl Debug for ScriptedSandboxFilesystem {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("ScriptedSandboxFilesystem")
                .finish_non_exhaustive()
        }
    }

    pub(crate) struct ScriptedSandboxFilesystemProvisioning {
        state: Arc<Mutex<ScriptedState>>,
    }

    #[derive(Clone)]
    pub(crate) struct ScriptedSandboxFilesystemControl {
        state: Arc<Mutex<ScriptedState>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) struct ScriptedSandboxPath {
        pub(crate) base: ScriptedSandboxPathBase,
        pub(crate) path: PathBuf,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) enum ScriptedSandboxPathBase {
        Root,
        Directory(u64),
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) enum ScriptedSandboxPathCall {
        HardLink {
            source: ScriptedSandboxPath,
            destination: ScriptedSandboxPath,
        },
        Rename {
            source: ScriptedSandboxPath,
            destination: ScriptedSandboxPath,
        },
    }

    pub(crate) struct ScriptedSandboxFilesystemGate {
        started: Arc<tokio::sync::Semaphore>,
        released: Arc<tokio::sync::Semaphore>,
        completed: Arc<tokio::sync::Semaphore>,
    }

    #[derive(Clone)]
    struct ScriptedGateState {
        started: Arc<tokio::sync::Semaphore>,
        released: Arc<tokio::sync::Semaphore>,
        completed: Arc<tokio::sync::Semaphore>,
    }

    #[derive(Default)]
    struct ScriptedState {
        calls: Vec<String>,
        sandbox_path_calls: Vec<ScriptedSandboxPathCall>,
        append_contents: HashMap<NativeFileIdentity, Vec<u8>>,
        gates: HashMap<String, ScriptedGateState>,
        creation: Option<Result<(), FilesystemStorageError>>,
        resolve_namespace_target:
            VecDeque<Result<ScriptedNamespaceResolution, FilesystemStorageError>>,
        open: VecDeque<Result<SandboxOpened, FilesystemStorageError>>,
        read: VecDeque<Result<Bytes, FilesystemStorageError>>,
        read_directory: VecDeque<Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>>,
        read_link: VecDeque<Result<SandboxSymlinkTarget, FilesystemStorageError>>,
        write: VecDeque<Result<SandboxWriteAttempt, FilesystemStorageError>>,
        get_attributes: VecDeque<Result<SandboxAttributes, FilesystemStorageError>>,
        set_size: VecDeque<Result<(), FilesystemStorageError>>,
        set_times: VecDeque<Result<(), FilesystemStorageError>>,
        create_directory: VecDeque<Result<(), FilesystemStorageError>>,
        create_symlink: VecDeque<Result<(), FilesystemStorageError>>,
        hard_link: VecDeque<Result<(), FilesystemStorageError>>,
        rename: VecDeque<Result<(), FilesystemStorageError>>,
        remove_directory: VecDeque<Result<(), FilesystemStorageError>>,
        unlink_file: VecDeque<Result<(), FilesystemStorageError>>,
        flush: VecDeque<Result<(), FilesystemStorageError>>,
        close: VecDeque<Result<(), FilesystemStorageError>>,
        seed: VecDeque<Result<(), FilesystemStorageError>>,
        observe_allocation: VecDeque<Result<FilesystemAllocation, FilesystemStorageError>>,
        install_limits: VecDeque<Result<InstalledLimits, FilesystemStorageError>>,
        copy_contents: VecDeque<Result<Box<[LinkGroup]>, FilesystemStorageError>>,
        delete_and_verify: VecDeque<Result<(), FilesystemStorageError>>,
    }

    struct ScriptedNamespaceResolution {
        parent_identity: u64,
        equivalent_name: OsString,
        final_directory_identity: Option<u64>,
        read_only_file: bool,
        followed_read_only_file: bool,
    }

    impl ScriptedSandboxFilesystemProvisioning {
        pub(crate) fn new() -> (Self, ScriptedSandboxFilesystemControl) {
            let state = Arc::new(Mutex::new(ScriptedState {
                creation: Some(Ok(())),
                ..ScriptedState::default()
            }));
            (
                Self {
                    state: Arc::clone(&state),
                },
                ScriptedSandboxFilesystemControl { state },
            )
        }
    }

    impl ScriptedSandboxFilesystemControl {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
                .calls
                .clone()
        }

        pub(crate) fn sandbox_path_calls(&self) -> Vec<ScriptedSandboxPathCall> {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
                .sandbox_path_calls
                .clone()
        }

        pub(crate) fn programmed_append_contents(&self, identity: u64) -> Vec<u8> {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
                .append_contents
                .get(&NativeFileIdentity::Scripted(format!(
                    "programmed-file-{identity}"
                )))
                .cloned()
                .unwrap_or_default()
        }

        pub(crate) fn set_creation(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
                .creation = Some(outcome);
        }

        pub(crate) fn block(&self, operation: impl Into<String>) -> ScriptedSandboxFilesystemGate {
            let operation = operation.into();
            let gate = ScriptedGateState {
                started: Arc::new(tokio::sync::Semaphore::new(0)),
                released: Arc::new(tokio::sync::Semaphore::new(0)),
                completed: Arc::new(tokio::sync::Semaphore::new(0)),
            };
            self.state().gates.insert(operation, gate.clone());
            ScriptedSandboxFilesystemGate {
                started: gate.started,
                released: gate.released,
                completed: gate.completed,
            }
        }

        pub(crate) fn push_open(&self, outcome: Result<SandboxOpened, FilesystemStorageError>) {
            self.state().open.push_back(outcome);
        }

        pub(crate) fn push_namespace_resolution(
            &self,
            parent_identity: u64,
            equivalent_name: impl Into<OsString>,
            final_directory_identity: Option<u64>,
        ) {
            self.state()
                .resolve_namespace_target
                .push_back(Ok(ScriptedNamespaceResolution {
                    parent_identity,
                    equivalent_name: equivalent_name.into(),
                    final_directory_identity,
                    read_only_file: false,
                    followed_read_only_file: false,
                }));
        }

        /// Programs the next namespace resolution of a target that is not a directory. The flags
        /// tell whether the target, and the object that a final symlink names, is a read-only
        /// file.
        pub(crate) fn push_read_only_resolution(
            &self,
            parent_identity: u64,
            equivalent_name: impl Into<OsString>,
            read_only_file: bool,
            followed_read_only_file: bool,
        ) {
            self.state()
                .resolve_namespace_target
                .push_back(Ok(ScriptedNamespaceResolution {
                    parent_identity,
                    equivalent_name: equivalent_name.into(),
                    final_directory_identity: None,
                    read_only_file,
                    followed_read_only_file,
                }));
        }

        pub(crate) fn push_namespace_resolution_error(&self, error: FilesystemStorageError) {
            self.state().resolve_namespace_target.push_back(Err(error));
        }

        pub(crate) fn push_read(&self, outcome: Result<Bytes, FilesystemStorageError>) {
            self.state().read.push_back(outcome);
        }

        pub(crate) fn push_read_directory(
            &self,
            outcome: Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>,
        ) {
            self.state().read_directory.push_back(outcome);
        }

        pub(crate) fn push_read_link(
            &self,
            outcome: Result<SandboxSymlinkTarget, FilesystemStorageError>,
        ) {
            self.state().read_link.push_back(outcome);
        }

        pub(crate) fn push_write(
            &self,
            outcome: Result<SandboxWriteAttempt, FilesystemStorageError>,
        ) {
            self.state().write.push_back(outcome);
        }

        pub(crate) fn push_get_attributes(
            &self,
            outcome: Result<SandboxAttributes, FilesystemStorageError>,
        ) {
            self.state().get_attributes.push_back(outcome);
        }

        pub(crate) fn push_set_size(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().set_size.push_back(outcome);
        }

        pub(crate) fn push_set_times(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().set_times.push_back(outcome);
        }

        pub(crate) fn push_create_directory(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().create_directory.push_back(outcome);
        }

        pub(crate) fn push_create_symlink(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().create_symlink.push_back(outcome);
        }

        pub(crate) fn push_hard_link(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().hard_link.push_back(outcome);
        }

        pub(crate) fn push_rename(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().rename.push_back(outcome);
        }

        pub(crate) fn push_remove_directory(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().remove_directory.push_back(outcome);
        }

        pub(crate) fn push_unlink_file(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().unlink_file.push_back(outcome);
        }

        pub(crate) fn push_flush(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().flush.push_back(outcome);
        }

        pub(crate) fn push_close(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().close.push_back(outcome);
        }

        pub(crate) fn push_seed(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().seed.push_back(outcome);
        }

        pub(crate) fn push_observe_allocation(
            &self,
            outcome: Result<FilesystemAllocation, FilesystemStorageError>,
        ) {
            self.state().observe_allocation.push_back(outcome);
        }

        pub(crate) fn push_install_limits(
            &self,
            outcome: Result<InstalledLimits, FilesystemStorageError>,
        ) {
            self.state().install_limits.push_back(outcome);
        }

        pub(crate) fn push_copy_contents(
            &self,
            outcome: Result<Box<[LinkGroup]>, FilesystemStorageError>,
        ) {
            self.state().copy_contents.push_back(outcome);
        }

        pub(crate) fn push_delete_and_verify(&self, outcome: Result<(), FilesystemStorageError>) {
            self.state().delete_and_verify.push_back(outcome);
        }

        fn state(&self) -> std::sync::MutexGuard<'_, ScriptedState> {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
        }
    }

    impl ScriptedSandboxFilesystemGate {
        pub(crate) async fn wait_started(&self) {
            self.started
                .acquire()
                .await
                .expect("scripted sandbox filesystem start gate closed")
                .forget();
        }

        pub(crate) fn release(&self) {
            self.released.add_permits(1);
        }

        pub(crate) async fn wait_completed(&self) {
            self.completed
                .acquire()
                .await
                .expect("scripted sandbox filesystem completion gate closed")
                .forget();
        }
    }

    impl SandboxFilesystemAdapter for ScriptedSandboxFilesystem {
        type Provisioning = ScriptedSandboxFilesystemProvisioning;
        type AllocationReader = ScriptedSandboxFilesystem;

        async fn create_fresh(
            provisioning: Self::Provisioning,
            name: SandboxFilesystemName,
            limits: Option<FilesystemLimits>,
        ) -> Result<Self, FilesystemStorageError> {
            let mut state = provisioning
                .state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned");
            state.calls.push(format!(
                "create_fresh(name={}, limits={limits:?})",
                name.relative_path().display()
            ));
            take_outcome(&mut state.creation, "create_fresh")?;
            drop(state);
            Ok(Self {
                state: provisioning.state,
            })
        }

        fn open(
            &self,
            target: SandboxPath,
            options: SandboxOpenOptions,
        ) -> impl Future<Output = Result<SandboxOpened, FilesystemStorageError>> + Send {
            self.outcome(
                format!("open(target={target:?}, options={options:?})"),
                |state| &mut state.open,
            )
        }

        fn resolve_namespace_target(
            &self,
            target: SandboxPath,
        ) -> impl Future<Output = Result<SandboxResolvedNamespaceTarget, FilesystemStorageError>> + Send
        {
            let state = Arc::clone(&self.state);
            async move {
                let call = format!("resolve_namespace_target(target={target:?})");
                let (gate, programmed) = {
                    let mut state = state
                        .lock()
                        .expect("scripted sandbox filesystem lock poisoned");
                    state.calls.push(call);
                    (
                        state.gates.remove("resolve_namespace_target"),
                        state.resolve_namespace_target.pop_front(),
                    )
                };
                if let Some(gate) = gate {
                    gate.started.add_permits(1);
                    gate.released
                        .acquire()
                        .await
                        .expect("scripted sandbox filesystem release gate closed")
                        .forget();
                    gate.completed.add_permits(1);
                }
                match programmed {
                    Some(Ok(programmed)) => scripted_namespace_resolution(target, programmed),
                    Some(Err(error)) => Err(error),
                    None => default_scripted_namespace_resolution(target),
                }
            }
        }

        fn read(
            &self,
            file: &SandboxFile,
            range: SandboxReadRange,
        ) -> impl Future<Output = Result<Bytes, FilesystemStorageError>> + Send {
            self.outcome(format!("read(file={file:?}, range={range:?})"), |state| {
                &mut state.read
            })
        }

        fn read_directory(
            &self,
            directory: &SandboxDirectory,
        ) -> impl Future<Output = Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>> + Send
        {
            self.outcome(
                format!("read_directory(directory={directory:?})"),
                |state| &mut state.read_directory,
            )
        }

        fn read_link(
            &self,
            path: SandboxPath,
        ) -> impl Future<Output = Result<SandboxSymlinkTarget, FilesystemStorageError>> + Send
        {
            self.outcome(format!("read_link(path={path:?})"), |state| {
                &mut state.read_link
            })
        }

        fn write(
            &self,
            file: &SandboxFile,
            placement: SandboxWritePlacement,
            bytes: Bytes,
        ) -> impl Future<Output = Result<SandboxWriteAttempt, FilesystemStorageError>> + Send
        {
            let identity = file.append_coordination.identity.clone();
            let appended = bytes.clone();
            let outcome = self.outcome(
                format!("write(file={file:?}, placement={placement:?}, bytes={bytes:?})"),
                |state| &mut state.write,
            );
            let state = Arc::clone(&self.state);
            async move {
                let attempt = outcome.await?;
                if placement == SandboxWritePlacement::Append
                    && let Ok(written) = usize::try_from(attempt.written)
                    && written <= appended.len()
                {
                    state
                        .lock()
                        .expect("scripted sandbox filesystem lock poisoned")
                        .append_contents
                        .entry(identity)
                        .or_default()
                        .extend_from_slice(&appended[..written]);
                }
                Ok(attempt)
            }
        }

        fn get_node_attributes(
            &self,
            node: SandboxNode,
        ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send
        {
            self.outcome(format!("get_node_attributes(node={node:?})"), |state| {
                &mut state.get_attributes
            })
        }

        fn get_path_attributes(
            &self,
            target: SandboxPath,
            follow: SandboxFollow,
        ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send
        {
            self.outcome(
                format!("get_path_attributes(target={target:?}, follow={follow:?})"),
                |state| &mut state.get_attributes,
            )
        }

        fn is_same_open_object(
            &self,
            left: SandboxNode,
            right: SandboxNode,
        ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send {
            let same = match (left, right) {
                (SandboxNode::File(left), SandboxNode::File(right)) => {
                    match (&left.handle, &right.handle) {
                        (NativeFileHandle::Scripted(left), NativeFileHandle::Scripted(right)) => {
                            left == right
                        }
                        _ => false,
                    }
                }
                (SandboxNode::Directory(left), SandboxNode::Directory(right)) => {
                    left.coordination_key == right.coordination_key
                }
                _ => false,
            };
            async move { Ok(same) }
        }

        fn set_size(
            &self,
            file: &SandboxFile,
            size: u64,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(format!("set_size(file={file:?}, size={size})"), |state| {
                &mut state.set_size
            })
        }

        fn set_node_times(
            &self,
            node: SandboxNode,
            times: SandboxTimeChanges,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(
                format!("set_node_times(node={node:?}, times={times:?})"),
                |state| &mut state.set_times,
            )
        }

        fn set_path_times(
            &self,
            target: SandboxPath,
            follow: SandboxFollow,
            times: SandboxTimeChanges,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(
                format!("set_path_times(target={target:?}, follow={follow:?}, times={times:?})"),
                |state| &mut state.set_times,
            )
        }

        fn create_directory(
            &self,
            path: SandboxPath,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(format!("create_directory(path={path:?})"), |state| {
                &mut state.create_directory
            })
        }

        fn create_symlink(
            &self,
            path: SandboxPath,
            target: SandboxSymlinkTarget,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(
                format!("create_symlink(path={path:?}, target={target:?})"),
                |state| &mut state.create_symlink,
            )
        }

        fn hard_link(
            &self,
            source: SandboxPath,
            destination: SandboxPath,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
                .sandbox_path_calls
                .push(ScriptedSandboxPathCall::HardLink {
                    source: scripted_sandbox_path(&source),
                    destination: scripted_sandbox_path(&destination),
                });
            self.outcome(
                format!("hard_link(source={source:?}, destination={destination:?})"),
                |state| &mut state.hard_link,
            )
        }

        fn rename(
            &self,
            source: SandboxPath,
            destination: SandboxPath,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned")
                .sandbox_path_calls
                .push(ScriptedSandboxPathCall::Rename {
                    source: scripted_sandbox_path(&source),
                    destination: scripted_sandbox_path(&destination),
                });
            self.outcome(
                format!("rename(source={source:?}, destination={destination:?})"),
                |state| &mut state.rename,
            )
        }

        fn remove_directory(
            &self,
            path: SandboxPath,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(format!("remove_directory(path={path:?})"), |state| {
                &mut state.remove_directory
            })
        }

        fn unlink_file(
            &self,
            path: SandboxPath,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(format!("unlink_file(path={path:?})"), |state| {
                &mut state.unlink_file
            })
        }

        fn flush(
            &self,
            node: &SandboxNode,
            level: SandboxFlushLevel,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(format!("flush(node={node:?}, level={level:?})"), |state| {
                &mut state.flush
            })
        }

        fn close(
            &self,
            node: SandboxNode,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome(format!("close(node={node:?})"), |state| &mut state.close)
        }

        fn seed(
            &self,
            entries: Box<[SeedEntry]>,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            let entries = entries
                .iter()
                .map(|entry| {
                    format!(
                        "{{source={}, target={:?}, access={:?}, existing={:?}}}",
                        entry.source.as_path().display(),
                        entry.target,
                        entry.access,
                        entry.existing
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            self.outcome(format!("seed(entries=[{entries}])"), |state| {
                &mut state.seed
            })
        }

        fn observe_allocation(
            &self,
        ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send
        {
            self.outcome("observe_allocation()".to_string(), |state| {
                &mut state.observe_allocation
            })
        }

        fn allocation_reader(&self) -> Self::AllocationReader {
            self.clone()
        }

        fn install_limits(
            &self,
            limits: FilesystemLimits,
        ) -> impl Future<Output = Result<InstalledLimits, FilesystemStorageError>> + Send {
            self.outcome(format!("install_limits(limits={limits:?})"), |state| {
                &mut state.install_limits
            })
        }

        fn copy_contents(
            &self,
            source: SandboxPath,
            excluded: Arc<TreeExclusions>,
            target: &HostPath,
        ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send {
            let mut excluded = excluded
                .paths()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            excluded.sort();
            self.outcome(
                format!(
                    "copy_contents(source={source:?}, excluded={excluded:?}, target={})",
                    target.as_path().display()
                ),
                |state| &mut state.copy_contents,
            )
        }

        fn delete_and_verify(
            self,
        ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
            self.outcome("delete_and_verify()".to_string(), |state| {
                &mut state.delete_and_verify
            })
        }
    }

    impl SandboxFilesystemAllocationReader for ScriptedSandboxFilesystem {
        fn read_allocation(
            &self,
        ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send
        {
            self.outcome("observe_allocation()".to_string(), |state| {
                &mut state.observe_allocation
            })
        }
    }

    impl ScriptedSandboxFilesystem {
        fn outcome<T: Send + 'static>(
            &self,
            call: String,
            outcomes: fn(&mut ScriptedState) -> &mut VecDeque<Result<T, FilesystemStorageError>>,
        ) -> impl Future<Output = Result<T, FilesystemStorageError>> + Send + use<T> {
            scripted_outcome(Arc::clone(&self.state), call, outcomes)
        }
    }

    async fn scripted_outcome<T: Send + 'static>(
        state: Arc<Mutex<ScriptedState>>,
        call: String,
        outcomes: fn(&mut ScriptedState) -> &mut VecDeque<Result<T, FilesystemStorageError>>,
    ) -> Result<T, FilesystemStorageError> {
        let operation = call
            .split_once('(')
            .map_or_else(|| call.clone(), |(operation, _)| operation.to_string());
        let (gate, outcome) = {
            let mut state = state
                .lock()
                .expect("scripted sandbox filesystem lock poisoned");
            state.calls.push(call);
            let gate = state.gates.remove(&operation);
            let outcome = outcomes(&mut state).pop_front().unwrap_or_else(|| {
                Err(FilesystemStorageError::verification(
                    "consume programmed scripted sandbox filesystem outcome",
                    Path::new("<scripted-sandbox-filesystem>"),
                ))
            });
            (gate, outcome)
        };
        if let Some(gate) = gate {
            gate.started.add_permits(1);
            gate.released
                .acquire()
                .await
                .expect("scripted sandbox filesystem release gate closed")
                .forget();
            gate.completed.add_permits(1);
        }
        outcome
    }

    fn scripted_namespace_resolution(
        target: SandboxPath,
        programmed: ScriptedNamespaceResolution,
    ) -> Result<SandboxResolvedNamespaceTarget, FilesystemStorageError> {
        let (_, name) = split_namespace_target(&target.path).map_err(|error| {
            FilesystemStorageError::io(
                "resolve scripted sandbox filesystem namespace target",
                Path::new("<scripted-sandbox-filesystem>"),
                error,
            )
        })?;
        let parent_identity = NativeFileIdentity::Scripted(format!(
            "programmed-directory-{}",
            programmed.parent_identity
        ));
        let parent_key = SandboxDirectoryCoordinationKey(parent_identity);
        Ok(SandboxResolvedNamespaceTarget {
            parent: SandboxDirectory::scripted_with_identity(
                programmed.parent_identity,
                format!("programmed-directory-{}", programmed.parent_identity),
            ),
            name,
            coordination_key: SandboxNamespaceCoordinationKey {
                parent: parent_key,
                name: NativeNameCoordinationKey {
                    name: programmed.equivalent_name,
                    mode: NativeNameComparisonMode::Exact,
                },
            },
            final_directory_key: programmed.final_directory_identity.map(|identity| {
                SandboxDirectoryCoordinationKey(NativeFileIdentity::Scripted(format!(
                    "programmed-directory-{identity}"
                )))
            }),
            read_only_file: programmed.read_only_file,
            followed_read_only_file: NativeReadOnlyResolution::Resolved(
                programmed.followed_read_only_file,
            ),
        })
    }

    fn scripted_sandbox_path(path: &SandboxPath) -> ScriptedSandboxPath {
        let base = match &path.base {
            SandboxPathBase::Root => ScriptedSandboxPathBase::Root,
            SandboxPathBase::Directory(directory) => {
                ScriptedSandboxPathBase::Directory(match &directory.handle {
                    NativeDirectoryHandle::Scripted(id) => *id,
                    NativeDirectoryHandle::Host(_) => {
                        panic!("scripted adapter received a production native directory")
                    }
                })
            }
        };
        ScriptedSandboxPath {
            base,
            path: path.path.clone(),
        }
    }

    pub(super) fn default_scripted_namespace_resolution(
        target: SandboxPath,
    ) -> Result<SandboxResolvedNamespaceTarget, FilesystemStorageError> {
        let (relative_parent, name) = split_namespace_target(&target.path).map_err(|error| {
            FilesystemStorageError::io(
                "resolve scripted sandbox filesystem namespace target",
                Path::new("<scripted-sandbox-filesystem>"),
                error,
            )
        })?;
        let relative_parent = normalize_scripted_parent(&relative_parent);
        let (base_id, base_identity) = match target.base {
            SandboxPathBase::Directory(directory) => match directory.handle {
                NativeDirectoryHandle::Scripted(id) => (id, directory.coordination_key.0),
                NativeDirectoryHandle::Host(_) => {
                    return Err(scripted_handle_error("resolve scripted namespace target"));
                }
            },
            SandboxPathBase::Root => (0, NativeFileIdentity::Scripted("scripted-root".to_string())),
        };
        let parent_identity = if relative_parent.as_os_str().is_empty() {
            base_identity
        } else {
            NativeFileIdentity::Scripted(format!("{base_identity:?}/{}", relative_parent.display()))
        };
        let parent_handle = if relative_parent.as_os_str().is_empty() {
            base_id
        } else {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            parent_identity.hash(&mut hasher);
            hasher.finish()
        };
        let parent_key = SandboxDirectoryCoordinationKey(parent_identity.clone());
        Ok(SandboxResolvedNamespaceTarget {
            parent: SandboxDirectory::scripted_with_identity(
                parent_handle,
                match parent_identity {
                    NativeFileIdentity::Scripted(identity) => identity,
                    #[allow(unreachable_patterns)]
                    _ => unreachable!("scripted namespace parent identity must be scripted"),
                },
            ),
            coordination_key: SandboxNamespaceCoordinationKey {
                parent: parent_key,
                name: NativeNameCoordinationKey {
                    name: name.clone(),
                    mode: NativeNameComparisonMode::Exact,
                },
            },
            name,
            final_directory_key: None,
            read_only_file: false,
            followed_read_only_file: NativeReadOnlyResolution::Resolved(false),
        })
    }

    fn normalize_scripted_parent(path: &Path) -> PathBuf {
        path.components()
            .fold(PathBuf::new(), |mut normalized, component| {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        normalized.pop();
                    }
                    std::path::Component::Normal(component) => normalized.push(component),
                    std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
                    std::path::Component::RootDir => normalized.clear(),
                }
                normalized
            })
    }

    fn take_outcome(
        outcome: &mut Option<Result<(), FilesystemStorageError>>,
        operation: &'static str,
    ) -> Result<(), FilesystemStorageError> {
        outcome.take().unwrap_or_else(|| {
            Err(FilesystemStorageError::verification(
                operation,
                Path::new("<scripted-sandbox-filesystem>"),
            ))
        })
    }
}

#[cfg(test)]
pub(crate) use scripted::*;

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn name() -> SandboxFilesystemName {
        SandboxFilesystemName::new(
            "environment".to_string(),
            "component".to_string(),
            "filesystem".to_string(),
        )
        .unwrap()
    }

    fn unmanaged_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
        SandboxFilesystemProvisioning::new(Some(root), None, RetryConfig::default()).unwrap()
    }

    /// Makes a host directory with `name` under `root` on unmanaged storage.
    async fn host_directory_at(root: &Path, name: &str) -> HostDirectory {
        HostDirectory::create_at_root(
            &unmanaged_provisioning(root.to_path_buf()),
            std::ffi::OsStr::new(name),
        )
        .await
        .unwrap()
    }

    fn host_child(directory: &HostDirectory, name: &str) -> HostPath {
        directory.path().child(std::ffi::OsStr::new(name)).unwrap()
    }

    fn seed_entry(
        source: HostPath,
        target: impl Into<PathBuf>,
        access: SeedAccess,
        existing: OnExisting,
    ) -> SeedEntry {
        SeedEntry {
            source,
            target: SandboxPath::at_root(target),
            access,
            existing,
        }
    }

    async fn seed_one(
        filesystem: &SandboxFilesystem,
        entry: SeedEntry,
    ) -> Result<(), FilesystemStorageError> {
        <SandboxFilesystem as SandboxFilesystemAdapter>::seed(filesystem, Box::new([entry])).await
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }

    #[cfg(target_os = "linux")]
    fn linux_parent_key(device: u64) -> SandboxDirectoryCoordinationKey {
        SandboxDirectoryCoordinationKey(NativeFileIdentity::Unix { device, inode: 1 })
    }

    #[cfg(target_os = "linux")]
    async fn validated_name_mode_test_filesystem() -> SandboxFilesystem {
        let root = tempfile::Builder::new()
            .prefix("golem-validated-name-mode")
            .tempdir()
            .unwrap()
            .keep();
        let directory = File::open(&root).unwrap();
        let device = std::os::unix::fs::MetadataExt::dev(&directory.metadata().unwrap());
        let lifecycle = Arc::new(AsyncMutex::new(()))
            .try_lock_owned()
            .expect("new name-mode test filesystem lease must be available");
        SandboxFilesystem::new(
            NativeRoot::new(root.clone(), directory),
            LeaseState {
                lifecycle,
                cleanup: NativeCleanup::Unmanaged {
                    path: root,
                    cleanup_retry: RetryConfig::default(),
                },
            },
            FilesystemVolume::unmanaged_development(),
            FileCopyMode::Buffered,
            QuotaAuthority::Unsupported,
            NativeNameModeSource::ValidatedManagedXfs(
                xfs::validated_managed_xfs_name_mode_for_test(device),
            ),
        )
    }

    #[cfg(target_os = "linux")]
    fn host_directory(path: &Path) -> SandboxDirectory {
        let directory = Arc::new(cap_std::fs::Dir::from_std_file(File::open(path).unwrap()));
        let coordination_key = native_directory_coordination_key(&directory).unwrap();
        SandboxDirectory {
            handle: NativeDirectoryHandle::Host(directory),
            path: path.to_path_buf(),
            coordination_key,
        }
    }

    fn scripted_error(operation: &'static str) -> FilesystemStorageError {
        FilesystemStorageError::verification(operation, Path::new("<scripted-test>"))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_operation_errors_preserve_raw_terminal_errno() {
        for (operation, errno) in [
            ("hard-link sandbox filesystem path", libc::EIO),
            ("rename sandbox filesystem path", libc::ESTALE),
            ("rename sandbox filesystem path", libc::ENODEV),
        ] {
            let error = namespace_operation_error(
                operation,
                Path::new("<destination>"),
                std::io::Error::from_raw_os_error(errno),
            );
            assert_eq!(
                error.io_error().and_then(std::io::Error::raw_os_error),
                Some(errno)
            );
            assert!(error.is_terminal_failure());
        }

        let cross_device = namespace_operation_error(
            "hard-link sandbox filesystem path",
            Path::new("<destination>"),
            std::io::Error::from_raw_os_error(libc::EXDEV),
        );
        assert_eq!(
            cross_device
                .io_error()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::EXDEV)
        );
    }

    async fn create_scripted(
        provisioning: ScriptedSandboxFilesystemProvisioning,
    ) -> ScriptedSandboxFilesystem {
        <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap()
    }

    async fn open_native_directory(
        filesystem: &SandboxFilesystem,
        path: impl Into<PathBuf>,
    ) -> SandboxDirectory {
        match filesystem
            .open(
                SandboxPath::at_root(path),
                SandboxOpenOptions::Existing {
                    expected: SandboxObjectKind::Directory,
                    access: SandboxAccessMode::ReadWrite,
                    follow: SandboxFollow::Yes,
                },
            )
            .await
            .unwrap()
            .into_node()
        {
            SandboxNode::Directory(directory) => directory,
            SandboxNode::File(_) => panic!("path must open as a directory"),
        }
    }

    async fn open_native_file(
        filesystem: &SandboxFilesystem,
        path: impl Into<PathBuf>,
    ) -> SandboxFile {
        match filesystem
            .open(
                SandboxPath::at_root(path),
                SandboxOpenOptions::Existing {
                    expected: SandboxObjectKind::File,
                    access: SandboxAccessMode::ReadWrite,
                    follow: SandboxFollow::Yes,
                },
            )
            .await
            .unwrap()
            .into_node()
        {
            SandboxNode::File(file) => file,
            SandboxNode::Directory(_) => panic!("path must open as a file"),
        }
    }

    #[test]
    async fn scripted_adapter_returns_programmed_outcomes_and_records_exact_order() {
        let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
        let seed_parent = tempfile::tempdir().unwrap();
        let seed_directory = host_directory_at(seed_parent.path(), ".seed-source").await;
        let seed_source = host_child(&seed_directory, "seed-source");
        let seed_call = format!(
            "seed(entries=[{{source={}, target=SandboxPath {{ base: Root, path: \"seed-destination\" }}, access=ReadWrite, existing=Fail}}])",
            seed_source.as_path().display()
        );
        let file = SandboxFile::scripted(7);
        let directory = SandboxDirectory::scripted(9);
        let attributes = SandboxAttributes {
            kind: SandboxObjectKind::File,
            link_count: 1,
            size: 12,
            accessed: None,
            modified: None,
            created: None,
            read_only: false,
            object: SandboxObjectId::scripted(12),
        };
        let limits = FilesystemLimits {
            allocated_bytes: 4096,
            filesystem_objects: 8,
        };
        let allocation = FilesystemAllocation {
            allocated_bytes: 1024,
            filesystem_objects: 3,
        };
        control.push_open(Ok(SandboxOpened::scripted_file(7)));
        control.push_read(Ok(Bytes::from_static(b"programmed")));
        control.push_read_directory(Ok(vec![SandboxDirectoryEntry {
            name: OsString::from("entry"),
            kind: SandboxObjectKind::Directory,
        }]));
        control.push_read_link(Ok(SandboxSymlinkTarget(PathBuf::from("target"))));
        control.push_write(Ok(SandboxWriteAttempt::completed(4)));
        control.push_get_attributes(Ok(attributes.clone()));
        control.push_set_size(Ok(()));
        control.push_set_times(Ok(()));
        control.push_create_directory(Ok(()));
        control.push_create_symlink(Ok(()));
        control.push_hard_link(Ok(()));
        control.push_rename(Ok(()));
        control.push_remove_directory(Ok(()));
        control.push_unlink_file(Ok(()));
        control.push_flush(Ok(()));
        control.push_close(Ok(()));
        control.push_seed(Ok(()));
        control.push_observe_allocation(Ok(allocation));
        control.push_install_limits(Ok(InstalledLimits { limits, allocation }));
        control.push_delete_and_verify(Ok(()));

        let filesystem = create_scripted(provisioning).await;
        let opened = filesystem
            .open(
                SandboxPath::at_root("opened"),
                SandboxOpenOptions::Existing {
                    expected: SandboxObjectKind::File,
                    access: SandboxAccessMode::Read,
                    follow: SandboxFollow::Yes,
                },
            )
            .await
            .unwrap();
        assert!(matches!(opened.into_node(), SandboxNode::File(_)));
        assert_eq!(
            filesystem
                .read(
                    &file,
                    SandboxReadRange {
                        offset: 2,
                        length: 10,
                    },
                )
                .await
                .unwrap(),
            Bytes::from_static(b"programmed")
        );
        assert_eq!(
            filesystem.read_directory(&directory).await.unwrap(),
            vec![SandboxDirectoryEntry {
                name: OsString::from("entry"),
                kind: SandboxObjectKind::Directory,
            }]
        );
        assert_eq!(
            filesystem
                .read_link(SandboxPath::at(directory.clone(), "link"))
                .await
                .unwrap(),
            SandboxSymlinkTarget(PathBuf::from("target"))
        );
        let attempt = filesystem
            .write(
                &file,
                SandboxWritePlacement::At(3),
                Bytes::from_static(b"data"),
            )
            .await
            .unwrap();
        assert_eq!(attempt.written, 4);
        assert!(attempt.result.is_ok());
        assert_eq!(
            filesystem
                .get_node_attributes(SandboxNode::File(file.clone()))
                .await
                .unwrap(),
            attributes
        );
        filesystem.set_size(&file, 20).await.unwrap();
        filesystem
            .set_node_times(
                SandboxNode::File(file.clone()),
                SandboxTimeChanges {
                    accessed: SandboxTimeChange::Keep,
                    modified: SandboxTimeChange::Now,
                },
            )
            .await
            .unwrap();
        filesystem
            .create_directory(SandboxPath::at(directory.clone(), "new-dir"))
            .await
            .unwrap();
        filesystem
            .create_symlink(
                SandboxPath::at(directory.clone(), "new-link"),
                SandboxSymlinkTarget(PathBuf::from("target")),
            )
            .await
            .unwrap();
        filesystem
            .hard_link(
                SandboxPath::at(directory.clone(), "source"),
                SandboxPath::at(directory.clone(), "hard"),
            )
            .await
            .unwrap();
        filesystem
            .rename(
                SandboxPath::at(directory.clone(), "before"),
                SandboxPath::at(directory.clone(), "after"),
            )
            .await
            .unwrap();
        filesystem
            .remove_directory(SandboxPath::at(directory.clone(), "old-dir"))
            .await
            .unwrap();
        filesystem
            .unlink_file(SandboxPath::at(directory.clone(), "old-file"))
            .await
            .unwrap();
        filesystem
            .flush(
                &SandboxNode::File(file.clone()),
                SandboxFlushLevel::DataAndMetadata,
            )
            .await
            .unwrap();
        filesystem
            .close(SandboxNode::File(file.clone()))
            .await
            .unwrap();
        filesystem
            .seed(Box::new([seed_entry(
                seed_source,
                "seed-destination",
                SeedAccess::ReadWrite,
                OnExisting::Fail,
            )]))
            .await
            .unwrap();
        assert_eq!(filesystem.observe_allocation().await.unwrap(), allocation);
        assert_eq!(
            filesystem.install_limits(limits).await.unwrap(),
            InstalledLimits { limits, allocation }
        );
        <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();

        assert_eq!(
            control.calls(),
            vec![
                "create_fresh(name=environment/component/filesystem, limits=None)",
                "open(target=SandboxPath { base: Root, path: \"opened\" }, options=Existing { expected: File, access: Read, follow: Yes })",
                "read(file=file(7), range=SandboxReadRange { offset: 2, length: 10 })",
                "read_directory(directory=directory(9))",
                "read_link(path=SandboxPath { base: Directory(directory(9)), path: \"link\" })",
                "write(file=file(7), placement=At(3), bytes=b\"data\")",
                "get_node_attributes(node=File(file(7)))",
                "set_size(file=file(7), size=20)",
                "set_node_times(node=File(file(7)), times=SandboxTimeChanges { accessed: Keep, modified: Now })",
                "create_directory(path=SandboxPath { base: Directory(directory(9)), path: \"new-dir\" })",
                "create_symlink(path=SandboxPath { base: Directory(directory(9)), path: \"new-link\" }, target=SandboxSymlinkTarget(\"target\"))",
                "hard_link(source=SandboxPath { base: Directory(directory(9)), path: \"source\" }, destination=SandboxPath { base: Directory(directory(9)), path: \"hard\" })",
                "rename(source=SandboxPath { base: Directory(directory(9)), path: \"before\" }, destination=SandboxPath { base: Directory(directory(9)), path: \"after\" })",
                "remove_directory(path=SandboxPath { base: Directory(directory(9)), path: \"old-dir\" })",
                "unlink_file(path=SandboxPath { base: Directory(directory(9)), path: \"old-file\" })",
                "flush(node=File(file(7)), level=DataAndMetadata)",
                "close(node=File(file(7)))",
                seed_call.as_str(),
                "observe_allocation()",
                "install_limits(limits=FilesystemLimits { allocated_bytes: 4096, filesystem_objects: 8 })",
                "delete_and_verify()",
            ]
        );
    }

    #[test]
    async fn scripted_adapter_surfaces_each_programmed_native_outcome() {
        let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
        control.push_read(Ok(Bytes::from_static(b"first")));
        control.push_read(Err(scripted_error("changed read outcome")));
        let filesystem = create_scripted(provisioning).await;
        let file = SandboxFile::scripted(1);
        let range = SandboxReadRange {
            offset: 0,
            length: 5,
        };

        assert_eq!(
            filesystem.read(&file, range).await.unwrap(),
            Bytes::from_static(b"first")
        );
        assert_eq!(
            filesystem.read(&file, range).await.unwrap_err().to_string(),
            "failed to changed read outcome filesystem <scripted-test>"
        );
        assert_eq!(
            control.calls(),
            vec![
                "create_fresh(name=environment/component/filesystem, limits=None)",
                "read(file=file(1), range=SandboxReadRange { offset: 0, length: 5 })",
                "read(file=file(1), range=SandboxReadRange { offset: 0, length: 5 })",
            ]
        );
    }

    #[test]
    async fn scripted_namespace_resolution_exposes_only_opaque_semantic_facts() {
        let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
        control.push_namespace_resolution(41, "equivalent", Some(42));
        control.push_namespace_resolution(41, "equivalent", Some(42));
        let filesystem = create_scripted(provisioning).await;

        let first = filesystem
            .resolve_namespace_target(SandboxPath::at_root("First"))
            .await
            .unwrap();
        let second = filesystem
            .resolve_namespace_target(SandboxPath::at_root("second"))
            .await
            .unwrap();

        assert!(first.coordination_key() == second.coordination_key());
        assert!(first.final_directory_key() == second.final_directory_key());
        assert_eq!(first.target().path, PathBuf::from("First"));
    }

    fn coordination_key_hash(key: &SandboxNamespaceCoordinationKey) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn exact_native_name_mode_distinguishes_siblings() {
        let first = SandboxNamespaceCoordinationKey::scripted_exact(1, "first");
        let same = SandboxNamespaceCoordinationKey::scripted_exact(1, "first");
        let sibling = SandboxNamespaceCoordinationKey::scripted_exact(1, "second");

        assert!(first == same);
        assert_eq!(coordination_key_hash(&first), coordination_key_hash(&same));
        assert!(first != sibling);
        assert!(!first.may_conflict_with(&sibling));
    }

    #[test]
    fn windows_insensitive_native_name_mode_compares_equivalent_names() {
        let first = SandboxNamespaceCoordinationKey::scripted_windows_insensitive(1, "CaseName");
        let equivalent =
            SandboxNamespaceCoordinationKey::scripted_windows_insensitive(1, "casename");
        let distinct = SandboxNamespaceCoordinationKey::scripted_windows_insensitive(1, "other");

        assert!(first == equivalent);
        assert_eq!(
            coordination_key_hash(&first),
            coordination_key_hash(&equivalent)
        );
        assert!(first != distinct);
        assert!(!first.may_conflict_with(&distinct));
    }

    #[test]
    fn conservative_native_name_mode_is_scoped_to_one_parent() {
        let first = SandboxNamespaceCoordinationKey::scripted_conservative(1, "first");
        let sibling = SandboxNamespaceCoordinationKey::scripted_conservative(1, "second");
        let other_parent = SandboxNamespaceCoordinationKey::scripted_conservative(2, "first");
        let exact = SandboxNamespaceCoordinationKey::scripted_exact(1, "third");

        assert!(first == sibling);
        assert_eq!(
            coordination_key_hash(&first),
            coordination_key_hash(&sibling)
        );
        assert!(first != other_parent);
        assert!(!first.may_conflict_with(&other_parent));
        assert!(first != exact);
        assert!(first.may_conflict_with(&exact));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validated_managed_xfs_name_mode_skips_native_detection() {
        let parent = linux_parent_key(17);
        let source = NativeNameModeSource::ValidatedManagedXfs(
            xfs::validated_managed_xfs_name_mode_for_test(17),
        );
        let probes = std::cell::Cell::new(0);

        assert_eq!(
            linux_name_comparison_mode(source, &parent, true, || {
                probes.set(probes.get() + 1);
                None
            }),
            NativeNameComparisonMode::Exact
        );
        assert_eq!(probes.get(), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unmanaged_linux_name_mode_retains_native_detection() {
        let parent = linux_parent_key(17);
        let probes = std::cell::Cell::new(0);

        assert_eq!(
            linux_name_comparison_mode(
                NativeNameModeSource::NativeDetection,
                &parent,
                true,
                || {
                    probes.set(probes.get() + 1);
                    Some(0)
                },
            ),
            NativeNameComparisonMode::Exact
        );
        assert_eq!(probes.get(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unknown_linux_name_mode_is_conservative_after_native_detection() {
        let parent = linux_parent_key(17);
        let probes = std::cell::Cell::new(0);

        assert_eq!(
            linux_name_comparison_mode(
                NativeNameModeSource::NativeDetection,
                &parent,
                true,
                || {
                    probes.set(probes.get() + 1);
                    None
                },
            ),
            NativeNameComparisonMode::Conservative
        );
        assert_eq!(probes.get(), 1);
        assert_eq!(
            linux_name_comparison_mode(
                NativeNameModeSource::NativeDetection,
                &parent,
                true,
                || Some(0x4000_0000),
            ),
            NativeNameComparisonMode::Conservative
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn benchmark_disable_control_restores_managed_xfs_native_detection() {
        let parent = linux_parent_key(17);
        let source = NativeNameModeSource::ValidatedManagedXfs(
            xfs::validated_managed_xfs_name_mode_for_test(17),
        );
        let probes = std::cell::Cell::new(0);

        assert_eq!(
            linux_name_comparison_mode(source, &parent, false, || {
                probes.set(probes.get() + 1);
                Some(0)
            }),
            NativeNameComparisonMode::Exact
        );
        assert_eq!(probes.get(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn sandbox_filesystem_constructor_preserves_validated_name_mode_for_same_filesystem_only()
    {
        let filesystem = validated_name_mode_test_filesystem().await;

        filesystem
            .resolve_namespace_target(SandboxPath::at_root("same-filesystem"))
            .await
            .unwrap();
        assert_eq!(filesystem.name_mode_probe_count(), 0);

        let foreign = host_directory(Path::new("/proc"));
        filesystem
            .resolve_namespace_target(SandboxPath::at(foreign, "self"))
            .await
            .unwrap();
        assert_eq!(filesystem.name_mode_probe_count(), 1);

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn unmanaged_provisioning_reaches_real_namespace_name_mode_detection() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();

        filesystem
            .resolve_namespace_target(SandboxPath::at_root("unmanaged-name-mode-probe"))
            .await
            .unwrap();
        assert_eq!(filesystem.name_mode_probe_count(), 1);

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn scripted_adapter_programs_creation_through_associated_provisioning() {
        let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
        control.set_creation(Err(scripted_error("programmed creation failure")));

        let error = <ScriptedSandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .expect_err("scripted creation must return the programmed failure");

        assert_eq!(
            error.to_string(),
            "failed to programmed creation failure filesystem <scripted-test>"
        );
        assert_eq!(
            control.calls(),
            vec!["create_fresh(name=environment/component/filesystem, limits=None)"]
        );
    }

    #[test]
    async fn scripted_adapter_programs_copy_contents_with_gates() {
        let parent = tempfile::tempdir().unwrap();
        let copies = host_directory_at(parent.path(), ".copies").await;
        let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
        control.push_copy_contents(Ok(Box::new([])));
        control.push_copy_contents(Err(scripted_error("programmed copy failure")));
        let filesystem = create_scripted(provisioning).await;
        let target = host_child(&copies, "copy");
        let excluded = Arc::new(TreeExclusions::new([
            PathBuf::from("lib/b.txt"),
            PathBuf::from("a.txt"),
        ]));

        let gate = control.block("copy_contents");
        let copying = tokio::spawn({
            let filesystem = filesystem.clone();
            let excluded = Arc::clone(&excluded);
            let target = target.clone();
            async move {
                filesystem
                    .copy_contents(SandboxPath::at_root("data"), excluded, &target)
                    .await
            }
        });
        gate.wait_started().await;
        assert!(!copying.is_finished());
        gate.release();
        gate.wait_completed().await;
        copying.await.unwrap().unwrap();
        let failed = filesystem
            .copy_contents(
                SandboxPath::at_root(""),
                Arc::new(TreeExclusions::default()),
                &target,
            )
            .await
            .unwrap_err();
        let unprogrammed = filesystem
            .copy_contents(SandboxPath::at_root(""), excluded, &target)
            .await
            .unwrap_err();

        assert_eq!(
            failed.to_string(),
            "failed to programmed copy failure filesystem <scripted-test>"
        );
        assert_eq!(
            unprogrammed.to_string(),
            "failed to consume programmed scripted sandbox filesystem outcome filesystem <scripted-sandbox-filesystem>"
        );
        let target_path = target.as_path().display();
        assert_eq!(
            control.calls(),
            vec![
                "create_fresh(name=environment/component/filesystem, limits=None)".to_string(),
                format!(
                    "copy_contents(source=SandboxPath {{ base: Root, path: \"data\" }}, excluded=[\"a.txt\", \"lib/b.txt\"], target={target_path})"
                ),
                format!(
                    "copy_contents(source=SandboxPath {{ base: Root, path: \"\" }}, excluded=[], target={target_path})"
                ),
                format!(
                    "copy_contents(source=SandboxPath {{ base: Root, path: \"\" }}, excluded=[\"a.txt\", \"lib/b.txt\"], target={target_path})"
                ),
            ]
        );
    }

    #[test]
    async fn copy_contents_copies_what_is_under_the_source_minus_the_exclusions() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let copies = host_directory_at(parent.path(), ".copies").await;
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        std::fs::create_dir_all(root.join("data/nested")).unwrap();
        std::fs::write(root.join("data/nested/note"), b"note").unwrap();
        std::fs::write(root.join("data/db"), b"db").unwrap();
        std::fs::write(root.join("static.bin"), b"static").unwrap();
        std::fs::set_permissions(root.join("data/db"), std::fs::Permissions::from_mode(0o640))
            .unwrap();
        std::os::unix::fs::symlink("data/db", root.join("link")).unwrap();
        let whole = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("whole"))
            .await
            .unwrap();
        let part = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("part"))
            .await
            .unwrap();

        <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
            &filesystem,
            SandboxPath::at_root(""),
            Arc::new(TreeExclusions::new([
                PathBuf::from("static.bin"),
                PathBuf::from("data/nested"),
            ])),
            whole.path(),
        )
        .await
        .unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
            &filesystem,
            SandboxPath::at_root("data"),
            Arc::new(TreeExclusions::new([PathBuf::from("db")])),
            part.path(),
        )
        .await
        .unwrap();

        let whole_root = whole.path().as_path();
        assert_eq!(
            tree_copy::tree_listing(whole_root),
            std::collections::BTreeSet::from(["data", "data/db", "link"].map(String::from))
        );
        assert_eq!(std::fs::read(whole_root.join("data/db")).unwrap(), b"db");
        assert_eq!(mode(&whole_root.join("data/db")), 0o640);
        assert_eq!(
            std::fs::read_link(whole_root.join("link")).unwrap(),
            PathBuf::from("data/db")
        );
        assert_eq!(
            tree_copy::tree_listing(part.path().as_path()),
            std::collections::BTreeSet::from(["nested", "nested/note"].map(String::from))
        );
        assert_eq!(
            std::fs::read(root.join("static.bin")).unwrap(),
            b"static",
            "the sandbox must stay as it is"
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn path_attributes_report_write_permission_and_object_identity() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        std::fs::write(root.join("file"), b"file").unwrap();
        std::fs::set_permissions(root.join("file"), std::fs::Permissions::from_mode(0o444))
            .unwrap();
        std::fs::hard_link(root.join("file"), root.join("alias")).unwrap();
        std::fs::write(root.join("other"), b"other").unwrap();
        let read = |path: &'static str| {
            let filesystem = &filesystem;
            async move {
                <SandboxFilesystem as SandboxFilesystemAdapter>::get_path_attributes(
                    filesystem,
                    SandboxPath::at_root(path),
                    SandboxFollow::No,
                )
                .await
                .unwrap()
            }
        };

        let (file, alias, other) = (read("file").await, read("alias").await, read("other").await);

        assert!(file.read_only);
        assert!(!other.read_only);
        assert_eq!(file.link_count, 2);
        assert_eq!(file.object, alias.object);
        assert_ne!(file.object, other.object);
        assert_eq!(file.created, alias.created);
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn copy_contents_refuses_a_target_that_is_not_an_empty_directory() {
        let parent = tempfile::tempdir().unwrap();
        let copies = host_directory_at(parent.path(), ".copies").await;
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        std::fs::write(filesystem.root().join("file"), b"file").unwrap();
        let copies_root = copies.path().as_path().to_path_buf();
        std::fs::create_dir(copies_root.join("full")).unwrap();
        std::fs::write(copies_root.join("full/kept"), b"kept").unwrap();
        std::fs::write(copies_root.join("plain-file"), b"plain").unwrap();
        std::fs::create_dir(copies_root.join("real")).unwrap();
        std::os::unix::fs::symlink("real", copies_root.join("alias")).unwrap();

        let results = futures::future::join_all(
            [
                ("full", std::io::ErrorKind::DirectoryNotEmpty),
                ("absent", std::io::ErrorKind::NotFound),
                ("plain-file", std::io::ErrorKind::NotADirectory),
                ("alias", std::io::ErrorKind::NotADirectory),
            ]
            .into_iter()
            .map(|(name, expected)| {
                let target = host_child(&copies, name);
                let filesystem = &filesystem;
                async move {
                    let result = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
                        filesystem,
                        SandboxPath::at_root(""),
                        Arc::new(TreeExclusions::default()),
                        &target,
                    )
                    .await;
                    (name, expected, result)
                }
            }),
        )
        .await;

        results.into_iter().for_each(|(name, expected, result)| {
            assert_eq!(
                result.unwrap_err().io_kind(),
                Some(expected),
                "a copy into {name} must be refused"
            );
        });
        assert_eq!(
            tree_copy::tree_listing(&copies_root),
            std::collections::BTreeSet::from(
                ["alias", "full", "full/kept", "plain-file", "real"].map(String::from)
            )
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn scripted_adapter_programs_seed_with_gates() {
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
        control.push_seed(Ok(()));
        control.push_seed(Err(scripted_error("programmed seed failure")));
        let filesystem = create_scripted(provisioning).await;
        let entries = || -> Box<[SeedEntry]> {
            Box::new([
                seed_entry(
                    host_child(&sources, "tree"),
                    "",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "file"),
                    "lib/file",
                    SeedAccess::ReadOnly,
                    OnExisting::Replace,
                ),
            ])
        };

        let gate = control.block("seed");
        let seeding = tokio::spawn({
            let filesystem = filesystem.clone();
            let entries = entries();
            async move { filesystem.seed(entries).await }
        });
        gate.wait_started().await;
        assert!(!seeding.is_finished());
        gate.release();
        gate.wait_completed().await;
        seeding.await.unwrap().unwrap();
        let failed = filesystem.seed(entries()).await.unwrap_err();
        let unprogrammed = filesystem.seed(entries()).await.unwrap_err();

        assert_eq!(
            failed.to_string(),
            "failed to programmed seed failure filesystem <scripted-test>"
        );
        assert_eq!(
            unprogrammed.to_string(),
            "failed to consume programmed scripted sandbox filesystem outcome filesystem <scripted-sandbox-filesystem>"
        );
        let call = format!(
            "seed(entries=[{{source={}, target=SandboxPath {{ base: Root, path: \"\" }}, access=FromSource, existing=Fail}}, {{source={}, target=SandboxPath {{ base: Root, path: \"lib/file\" }}, access=ReadOnly, existing=Replace}}])",
            sources.path().as_path().join("tree").display(),
            sources.path().as_path().join("file").display()
        );
        assert_eq!(
            control.calls(),
            vec![
                "create_fresh(name=environment/component/filesystem, limits=None)".to_string(),
                call.clone(),
                call.clone(),
                call,
            ]
        );
    }

    #[test]
    async fn seed_puts_entries_in_order_and_stops_at_the_first_failure() {
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        std::fs::write(sources.path().as_path().join("file"), b"first").unwrap();
        std::fs::write(sources.path().as_path().join("after"), b"after").unwrap();
        std::os::unix::fs::symlink("file", sources.path().as_path().join("link")).unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();

        let error = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
            &filesystem,
            Box::new([
                seed_entry(
                    host_child(&sources, "file"),
                    "nested/file",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "link"),
                    "nested/link",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "after"),
                    "nested/file",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "after"),
                    "after",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
            ]),
        )
        .await
        .unwrap_err();

        let root = filesystem.root();
        assert_eq!(error.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        assert!(
            error.to_string().contains("nested/file"),
            "the error must name the target of the failed entry: {error}"
        );
        assert_eq!(std::fs::read(root.join("nested/file")).unwrap(), b"first");
        assert_eq!(
            std::fs::read_link(root.join("nested/link")).unwrap(),
            PathBuf::from("file")
        );
        assert!(
            !root.join("after").exists(),
            "an entry after the failed entry must not be seeded"
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_access_sets_the_write_permission_of_seeded_files() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_010);
        [
            ("tool", 0o755),
            ("locked", 0o555),
            ("private", 0o640),
            ("writable", 0o644),
        ]
        .into_iter()
        .for_each(|(name, mode)| {
            let path = sources.path().as_path().join(name);
            std::fs::write(&path, name).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        });
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();

        <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
            &filesystem,
            Box::new([
                seed_entry(
                    host_child(&sources, "tool"),
                    "tool",
                    SeedAccess::ReadOnly,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "locked"),
                    "locked",
                    SeedAccess::ReadWrite,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "private"),
                    "private",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "writable"),
                    "writable",
                    SeedAccess::ReadWrite,
                    OnExisting::Fail,
                ),
            ]),
        )
        .await
        .unwrap();

        let root = filesystem.root();
        [
            ("tool", 0o555),
            ("locked", 0o755),
            ("private", 0o640),
            ("writable", 0o644),
        ]
        .into_iter()
        .for_each(|(name, expected)| {
            assert_eq!(
                mode(&root.join(name)),
                expected,
                "{name} must have mode {expected:o}"
            );
            assert_eq!(
                std::fs::metadata(root.join(name))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified,
                "{name} must have the modification time of its source"
            );
        });
        assert_eq!(std::fs::read(root.join("tool")).unwrap(), b"tool");
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_gives_a_made_target_directory_the_source_attributes_and_keeps_a_merged_one() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        let tree = sources.path().as_path().join("tree");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("file"), b"file").unwrap();
        std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o750)).unwrap();
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        File::open(&tree).unwrap().set_modified(modified).unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        std::fs::create_dir(root.join("merged")).unwrap();
        std::fs::set_permissions(root.join("merged"), std::fs::Permissions::from_mode(0o700))
            .unwrap();

        <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
            &filesystem,
            Box::new([
                seed_entry(
                    host_child(&sources, "tree"),
                    "made",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "tree"),
                    "merged",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
            ]),
        )
        .await
        .unwrap();

        assert_eq!(
            mode(&root.join("made")),
            0o750,
            "the made directory must have the mode of its source"
        );
        assert_eq!(
            std::fs::metadata(root.join("made"))
                .unwrap()
                .modified()
                .unwrap(),
            modified,
            "the made directory must have the modification time of its source"
        );
        assert_eq!(
            mode(&root.join("merged")),
            0o700,
            "the merged directory must keep its own mode"
        );
        assert_eq!(std::fs::read(root.join("merged/file")).unwrap(), b"file");
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_reports_the_error_of_a_directory_that_it_cannot_make() {
        use std::os::unix::fs::PermissionsExt as _;

        if rustix::process::geteuid().is_root() {
            return;
        }
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        std::fs::create_dir(sources.path().as_path().join("tree")).unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let locked = filesystem.root().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let error = seed_one(
            &filesystem,
            seed_entry(
                host_child(&sources, "tree"),
                "locked/new",
                SeedAccess::FromSource,
                OnExisting::Fail,
            ),
        )
        .await
        .unwrap_err();

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            error.io_kind(),
            Some(std::io::ErrorKind::PermissionDenied),
            "{error}"
        );
        assert!(!locked.join("new").exists());
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_file_entries_follow_the_existing_rule() {
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        std::fs::write(sources.path().as_path().join("new"), b"new").unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        ["fail", "replace"].into_iter().for_each(|name| {
            std::fs::write(root.join(name), b"old").unwrap();
        });
        std::fs::create_dir(root.join("directory")).unwrap();
        std::fs::write(root.join("directory/child"), b"child").unwrap();
        let new = || host_child(&sources, "new");

        let failed = seed_one(
            &filesystem,
            seed_entry(new(), "fail", SeedAccess::ReadWrite, OnExisting::Fail),
        )
        .await
        .unwrap_err();
        seed_one(
            &filesystem,
            seed_entry(new(), "absent", SeedAccess::ReadWrite, OnExisting::Fail),
        )
        .await
        .unwrap();
        seed_one(
            &filesystem,
            seed_entry(new(), "replace", SeedAccess::ReadOnly, OnExisting::Replace),
        )
        .await
        .unwrap();
        let failed_over_directory = seed_one(
            &filesystem,
            seed_entry(new(), "directory", SeedAccess::ReadWrite, OnExisting::Fail),
        )
        .await
        .unwrap_err();
        assert!(root.join("directory/child").is_file());
        seed_one(
            &filesystem,
            seed_entry(
                new(),
                "directory",
                SeedAccess::ReadWrite,
                OnExisting::Replace,
            ),
        )
        .await
        .unwrap();

        assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(root.join("fail")).unwrap(), b"old");
        assert_eq!(std::fs::read(root.join("absent")).unwrap(), b"new");
        assert_eq!(std::fs::read(root.join("replace")).unwrap(), b"new");
        assert_eq!(mode(&root.join("replace")) & 0o222, 0);
        assert_eq!(
            failed_over_directory.io_kind(),
            Some(std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(root.join("directory")).unwrap(), b"new");
        assert!(
            std::fs::read_dir(&root).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".golem-copy-")),
            "a temporary file must not stay in the root"
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_symlink_entries_follow_the_existing_rule() {
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        std::os::unix::fs::symlink("new-target", sources.path().as_path().join("link")).unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        ["fail", "replace"].into_iter().for_each(|name| {
            std::os::unix::fs::symlink("old-target", root.join(name)).unwrap();
        });
        std::fs::create_dir(root.join("directory")).unwrap();
        std::fs::write(root.join("directory/child"), b"child").unwrap();
        let link = || host_child(&sources, "link");

        let failed = seed_one(
            &filesystem,
            seed_entry(link(), "fail", SeedAccess::FromSource, OnExisting::Fail),
        )
        .await
        .unwrap_err();

        seed_one(
            &filesystem,
            seed_entry(
                link(),
                "replace",
                SeedAccess::FromSource,
                OnExisting::Replace,
            ),
        )
        .await
        .unwrap();
        seed_one(
            &filesystem,
            seed_entry(link(), "absent", SeedAccess::FromSource, OnExisting::Fail),
        )
        .await
        .unwrap();
        seed_one(
            &filesystem,
            seed_entry(
                link(),
                "directory",
                SeedAccess::FromSource,
                OnExisting::Replace,
            ),
        )
        .await
        .unwrap();

        assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        [
            ("fail", "old-target"),
            ("replace", "new-target"),
            ("absent", "new-target"),
            ("directory", "new-target"),
        ]
        .into_iter()
        .for_each(|(name, expected)| {
            assert_eq!(
                std::fs::read_link(root.join(name)).unwrap(),
                PathBuf::from(expected),
                "{name} must point to {expected}"
            );
        });
        assert!(
            std::fs::read_dir(&root).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".golem-copy-")),
            "a temporary symlink must not stay in the root"
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_reads_a_source_symlink_without_following_it() {
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"secret").unwrap();
        std::fs::create_dir(sources.path().as_path().join("real")).unwrap();
        std::fs::write(sources.path().as_path().join("real/file"), b"file").unwrap();
        std::os::unix::fs::symlink("real", sources.path().as_path().join("alias")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            sources.path().as_path().join("outside"),
        )
        .unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();

        <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
            &filesystem,
            Box::new([
                seed_entry(
                    host_child(&sources, "alias"),
                    "alias",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
                seed_entry(
                    host_child(&sources, "outside"),
                    "outside",
                    SeedAccess::FromSource,
                    OnExisting::Fail,
                ),
            ]),
        )
        .await
        .unwrap();

        let root = filesystem.root();
        assert!(
            std::fs::symlink_metadata(root.join("alias"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(root.join("alias")).unwrap(),
            PathBuf::from("real")
        );
        assert!(
            std::fs::symlink_metadata(root.join("outside"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(root.join("outside")).unwrap(),
            outside.path().join("secret")
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_cannot_write_outside_the_root() {
        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        std::fs::write(sources.path().as_path().join("file"), b"file").unwrap();
        std::os::unix::fs::symlink("file", sources.path().as_path().join("link")).unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("data")).unwrap();

        let errors = futures::future::join_all(
            [
                ("file", "../escaped", OnExisting::Fail),
                ("file", "data/file", OnExisting::Fail),
                ("file", "data/file", OnExisting::Replace),
                ("file", "data/nested/file", OnExisting::Fail),
                ("link", "data/link", OnExisting::Fail),
                ("link", "data/link", OnExisting::Replace),
            ]
            .into_iter()
            .map(|(source, target, existing)| {
                let entry = seed_entry(
                    host_child(&sources, source),
                    target,
                    SeedAccess::FromSource,
                    existing,
                );
                let filesystem = &filesystem;
                async move { (target, existing, seed_one(filesystem, entry).await) }
            }),
        )
        .await;

        errors.into_iter().for_each(|(target, existing, result)| {
            assert_eq!(
                result.unwrap_err().io_kind(),
                Some(std::io::ErrorKind::PermissionDenied),
                "seed to {target} with {existing:?} must be refused"
            );
        });
        let tree = host_child(&sources, "tree");
        std::fs::create_dir_all(tree.as_path().join("data/nested")).unwrap();
        std::fs::write(tree.as_path().join("data/nested/file"), b"file").unwrap();
        let fail = seed_one(
            &filesystem,
            seed_entry(tree.clone(), "", SeedAccess::FromSource, OnExisting::Fail),
        )
        .await
        .unwrap_err();
        assert!(
            std::fs::symlink_metadata(root.join("data"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "a failed seed must leave the symlink in place"
        );
        seed_one(
            &filesystem,
            seed_entry(tree, "", SeedAccess::FromSource, OnExisting::Replace),
        )
        .await
        .unwrap();

        assert_eq!(fail.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        assert!(root.join("data/nested/file").is_file());
        assert!(
            !std::fs::symlink_metadata(root.join("data"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "replace must put a directory in place of the symlink"
        );
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        assert!(!root.parent().unwrap().join("escaped").exists());
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn seed_directory_entries_merge_and_follow_the_existing_rule() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let sources = host_directory_at(parent.path(), ".sources").await;
        let tree = sources.path().as_path().join("tree");
        std::fs::create_dir_all(tree.join("data")).unwrap();
        std::fs::write(tree.join("data/added"), b"added").unwrap();
        std::fs::write(tree.join("data/same"), b"new").unwrap();
        std::fs::create_dir_all(tree.join("fresh")).unwrap();
        std::fs::write(tree.join("fresh/deep"), b"deep").unwrap();
        std::fs::set_permissions(tree.join("fresh"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        std::fs::set_permissions(tree.join("data"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        std::os::unix::fs::symlink("new", tree.join("link")).unwrap();
        std::fs::write(tree.join("was-directory"), b"file").unwrap();
        std::fs::create_dir(tree.join("was-file")).unwrap();
        std::fs::write(tree.join("was-file/inner"), b"inner").unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();
        ["fail", "replace"].into_iter().for_each(|rule| {
            let target = root.join(rule);
            std::fs::create_dir_all(target.join("data")).unwrap();
            std::fs::set_permissions(target.join("data"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            std::fs::write(target.join("data/same"), b"old").unwrap();
            std::fs::write(target.join("data/only-sandbox"), b"sandbox").unwrap();
            std::os::unix::fs::symlink("old", target.join("link")).unwrap();
            std::fs::create_dir(target.join("was-directory")).unwrap();
            std::fs::write(target.join("was-directory/child"), b"child").unwrap();
            std::fs::write(target.join("was-file"), b"file").unwrap();
        });
        std::fs::write(root.join("top-file"), b"top").unwrap();
        let seed = |target: &'static str, existing| {
            let entry = seed_entry(
                host_child(&sources, "tree"),
                target,
                SeedAccess::FromSource,
                existing,
            );
            let filesystem = &filesystem;
            async move { seed_one(filesystem, entry).await }
        };

        let failed = seed("fail", OnExisting::Fail).await.unwrap_err();

        seed("replace", OnExisting::Replace).await.unwrap();
        seed("absent", OnExisting::Fail).await.unwrap();
        let failed_top = seed("top-file", OnExisting::Fail).await.unwrap_err();
        assert_eq!(std::fs::read(root.join("top-file")).unwrap(), b"top");
        seed("top-file", OnExisting::Replace).await.unwrap();

        assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(root.join("fail/data/same")).unwrap(), b"old");
        assert_eq!(
            std::fs::read(root.join("fail/data/added")).unwrap(),
            b"added",
            "a path before the failed path must stay"
        );
        assert!(
            !root.join("fail/fresh").exists(),
            "a path after the failed path must not be seeded"
        );
        assert_eq!(
            failed_top.io_kind(),
            Some(std::io::ErrorKind::AlreadyExists)
        );

        let read = |path: &str| std::fs::read(root.join(path)).unwrap();

        assert_eq!(read("replace/data/same"), b"new");
        assert_eq!(read("replace/data/added"), b"added");
        assert_eq!(read("replace/data/only-sandbox"), b"sandbox");
        assert_eq!(read("replace/fresh/deep"), b"deep");
        assert_eq!(read("replace/was-file/inner"), b"inner");
        assert_eq!(read("replace/was-directory"), b"file");
        assert_eq!(
            std::fs::read_link(root.join("replace/link")).unwrap(),
            PathBuf::from("new")
        );

        assert_eq!(
            tree_copy::tree_listing(&root.join("absent")),
            tree_copy::tree_listing(&tree)
        );
        assert_eq!(read("top-file/data/same"), b"new");
        assert_eq!(
            mode(&root.join("replace/data")),
            0o700,
            "a merged directory must keep its permissions"
        );
        assert_eq!(
            mode(&root.join("replace/fresh")),
            0o750,
            "a made directory must get the permissions of its source"
        );
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn production_adapter_uses_concrete_static_dispatch_for_creation_and_deletion() {
        async fn create<A: SandboxFilesystemAdapter>(
            provisioning: A::Provisioning,
            name: SandboxFilesystemName,
            limits: Option<FilesystemLimits>,
        ) -> Result<A, FilesystemStorageError> {
            A::create_fresh(provisioning, name, limits).await
        }

        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = create::<SandboxFilesystem>(provisioning, name(), None)
            .await
            .unwrap();
        let root = filesystem.root().to_path_buf();

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
        assert!(!root.exists());

        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let rejected_root = parent.path().join(name().relative_path());
        let error = create::<SandboxFilesystem>(
            provisioning,
            name(),
            Some(FilesystemLimits {
                allocated_bytes: 4096,
                filesystem_objects: 8,
            }),
        )
        .await
        .err()
        .expect("unmanaged creation must reject finite limits");
        assert_eq!(
            error.to_string(),
            format!(
                "failed to install limits without quota authority filesystem {}",
                rejected_root.display()
            )
        );
        assert!(!rejected_root.exists());
    }

    #[test]
    async fn production_adapter_executes_the_filesystem_method_families() {
        let parent = tempfile::tempdir().unwrap();
        let seed_sources = host_directory_at(parent.path(), ".seed-sources").await;
        std::fs::write(seed_sources.path().as_path().join("file"), b"seeded").unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let root = filesystem.root().to_path_buf();

        let directory = match <SandboxFilesystem as SandboxFilesystemAdapter>::open(
            &filesystem,
            SandboxPath::at_root("."),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::Directory,
                access: SandboxAccessMode::Read,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
        .into_node()
        {
            SandboxNode::Directory(directory) => directory,
            SandboxNode::File(_) => panic!("root must open as a directory"),
        };
        let file = match <SandboxFilesystem as SandboxFilesystemAdapter>::open(
            &filesystem,
            SandboxPath::at_root("file"),
            SandboxOpenOptions::File {
                access: SandboxAccessMode::ReadWrite,
                disposition: SandboxFileDisposition::CreateOrTruncate,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
        .into_node()
        {
            SandboxNode::File(file) => file,
            SandboxNode::Directory(_) => panic!("file must open as a file"),
        };

        let attempt = <SandboxFilesystem as SandboxFilesystemAdapter>::write(
            &filesystem,
            &file,
            SandboxWritePlacement::At(0),
            Bytes::from_static(b"contents"),
        )
        .await
        .unwrap();
        assert_eq!(attempt.written, 8);
        assert!(attempt.result.is_ok());
        assert_eq!(
            <SandboxFilesystem as SandboxFilesystemAdapter>::read(
                &filesystem,
                &file,
                SandboxReadRange {
                    offset: 2,
                    length: 4,
                },
            )
            .await
            .unwrap(),
            Bytes::from_static(b"nten")
        );
        <SandboxFilesystem as SandboxFilesystemAdapter>::set_size(&filesystem, &file, 5)
            .await
            .unwrap();
        let attributes = <SandboxFilesystem as SandboxFilesystemAdapter>::get_node_attributes(
            &filesystem,
            SandboxNode::File(file.clone()),
        )
        .await
        .unwrap();
        assert_eq!(attributes.kind, SandboxObjectKind::File);
        assert_eq!(attributes.size, 5);
        <SandboxFilesystem as SandboxFilesystemAdapter>::set_node_times(
            &filesystem,
            SandboxNode::File(file.clone()),
            SandboxTimeChanges {
                accessed: SandboxTimeChange::Keep,
                modified: SandboxTimeChange::Now,
            },
        )
        .await
        .unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::flush(
            &filesystem,
            &SandboxNode::File(file.clone()),
            SandboxFlushLevel::DataAndMetadata,
        )
        .await
        .unwrap();

        <SandboxFilesystem as SandboxFilesystemAdapter>::create_directory(
            &filesystem,
            SandboxPath::at(directory.clone(), "directory"),
        )
        .await
        .unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::create_symlink(
            &filesystem,
            SandboxPath::at(directory.clone(), "link"),
            SandboxSymlinkTarget(PathBuf::from("file")),
        )
        .await
        .unwrap();
        assert_eq!(
            <SandboxFilesystem as SandboxFilesystemAdapter>::read_link(
                &filesystem,
                SandboxPath::at(directory.clone(), "link"),
            )
            .await
            .unwrap(),
            SandboxSymlinkTarget(PathBuf::from("file"))
        );
        <SandboxFilesystem as SandboxFilesystemAdapter>::hard_link(
            &filesystem,
            SandboxPath::at_root("file"),
            SandboxPath::at_root("hard"),
        )
        .await
        .unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::rename(
            &filesystem,
            SandboxPath::at_root("hard"),
            SandboxPath::at_root("renamed"),
        )
        .await
        .unwrap();
        seed_one(
            &filesystem,
            seed_entry(
                host_child(&seed_sources, "file"),
                "seeded",
                SeedAccess::ReadWrite,
                OnExisting::Fail,
            ),
        )
        .await
        .unwrap();

        let mut entries = <SandboxFilesystem as SandboxFilesystemAdapter>::read_directory(
            &filesystem,
            &directory,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            vec![
                OsString::from("directory"),
                OsString::from("file"),
                OsString::from("link"),
                OsString::from("renamed"),
                OsString::from("seeded"),
            ]
        );
        assert!(
            <SandboxFilesystem as SandboxFilesystemAdapter>::observe_allocation(&filesystem)
                .await
                .is_err()
        );

        <SandboxFilesystem as SandboxFilesystemAdapter>::close(
            &filesystem,
            SandboxNode::File(file),
        )
        .await
        .unwrap();
        for path in ["file", "link", "renamed", "seeded"] {
            <SandboxFilesystem as SandboxFilesystemAdapter>::unlink_file(
                &filesystem,
                SandboxPath::at(directory.clone(), path),
            )
            .await
            .unwrap();
        }
        <SandboxFilesystem as SandboxFilesystemAdapter>::remove_directory(
            &filesystem,
            SandboxPath::at(directory.clone(), "directory"),
        )
        .await
        .unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::close(
            &filesystem,
            SandboxNode::Directory(directory),
        )
        .await
        .unwrap();
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
        assert!(!root.exists());
    }

    #[test]
    async fn hard_link_and_rename_keep_exact_roles_across_distinct_resolved_parents() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::create_dir(filesystem.root().join("source-parent")).unwrap();
        std::fs::create_dir(filesystem.root().join("destination-parent")).unwrap();
        std::fs::write(
            filesystem.root().join("source-parent/hard-source"),
            b"hard-link contents",
        )
        .unwrap();
        std::fs::write(
            filesystem.root().join("source-parent/rename-source"),
            b"rename contents",
        )
        .unwrap();
        let source_parent = open_native_directory(&filesystem, "source-parent").await;
        let destination_parent = open_native_directory(&filesystem, "destination-parent").await;

        let hard_source = filesystem
            .resolve_namespace_target(SandboxPath::at(source_parent.clone(), "hard-source"))
            .await
            .unwrap();
        let hard_destination = filesystem
            .resolve_namespace_target(SandboxPath::at(
                destination_parent.clone(),
                "hard-destination",
            ))
            .await
            .unwrap();
        filesystem
            .hard_link(hard_source.target(), hard_destination.target())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(
                filesystem
                    .root()
                    .join("destination-parent/hard-destination")
            )
            .unwrap(),
            b"hard-link contents"
        );
        assert!(filesystem.root().join("source-parent/hard-source").exists());

        let rename_source = filesystem
            .resolve_namespace_target(SandboxPath::at(source_parent.clone(), "rename-source"))
            .await
            .unwrap();
        let rename_destination = filesystem
            .resolve_namespace_target(SandboxPath::at(
                destination_parent.clone(),
                "rename-destination",
            ))
            .await
            .unwrap();
        filesystem
            .rename(rename_source.target(), rename_destination.target())
            .await
            .unwrap();
        assert!(
            !filesystem
                .root()
                .join("source-parent/rename-source")
                .exists()
        );
        assert_eq!(
            std::fs::read(
                filesystem
                    .root()
                    .join("destination-parent/rename-destination")
            )
            .unwrap(),
            b"rename contents"
        );

        drop((
            hard_source,
            hard_destination,
            rename_source,
            rename_destination,
            source_parent,
            destination_parent,
        ));
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn capability_relative_path_operations_never_fall_back_to_root() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::create_dir(filesystem.root().join("base")).unwrap();
        std::fs::write(filesystem.root().join("same-name"), b"root").unwrap();
        std::fs::write(filesystem.root().join("base/same-name"), b"capability").unwrap();
        let base = open_native_directory(&filesystem, "base").await;

        let file = match filesystem
            .open(
                SandboxPath::at(base.clone(), "same-name"),
                SandboxOpenOptions::Existing {
                    expected: SandboxObjectKind::File,
                    access: SandboxAccessMode::Read,
                    follow: SandboxFollow::Yes,
                },
            )
            .await
            .unwrap()
            .into_node()
        {
            SandboxNode::File(file) => file,
            SandboxNode::Directory(_) => panic!("same-name must open as a file"),
        };
        assert_eq!(
            filesystem
                .read(
                    &file,
                    SandboxReadRange {
                        offset: 0,
                        length: 10,
                    },
                )
                .await
                .unwrap(),
            Bytes::from_static(b"capability")
        );

        drop((file, base));
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn external_seed_keeps_the_pinned_destination_after_directory_rename() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let root = filesystem.root().to_path_buf();
        std::fs::create_dir(root.join("pinned")).unwrap();
        let pinned = open_native_directory(&filesystem, "pinned").await;
        std::fs::rename(root.join("pinned"), root.join("renamed")).unwrap();
        std::fs::create_dir(root.join("pinned")).unwrap();

        let host_source = host_directory_at(parent.path(), ".seed-source").await;
        std::fs::write(host_source.path().as_path().join("source"), b"seeded").unwrap();
        let source = host_child(&host_source, "source");
        seed_one(
            &filesystem,
            SeedEntry {
                source: source.clone(),
                target: SandboxPath::at(pinned.clone(), "destination"),
                access: SeedAccess::ReadOnly,
                existing: OnExisting::Fail,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(root.join("renamed/destination")).unwrap(),
            b"seeded"
        );
        assert!(
            std::fs::metadata(root.join("renamed/destination"))
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(!root.join("pinned/destination").exists());
        let existing = seed_one(
            &filesystem,
            SeedEntry {
                source,
                target: SandboxPath::at(pinned, "destination"),
                access: SeedAccess::ReadOnly,
                existing: OnExisting::Fail,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn empty_root_path_attributes_address_root() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();

        for follow in [SandboxFollow::No, SandboxFollow::Yes] {
            let attributes = filesystem
                .get_path_attributes(SandboxPath::at_root(""), follow)
                .await
                .unwrap();
            assert_eq!(attributes.kind, SandboxObjectKind::Directory);
        }
    }

    #[cfg(unix)]
    #[test]
    async fn attribute_path_follow_mode_is_precise() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(filesystem.root().join("target"), b"contents").unwrap();
        std::os::unix::fs::symlink("target", filesystem.root().join("link")).unwrap();

        let link_attributes = filesystem
            .get_path_attributes(SandboxPath::at_root("link"), SandboxFollow::No)
            .await
            .unwrap();
        let followed_attributes = filesystem
            .get_path_attributes(SandboxPath::at_root("link"), SandboxFollow::Yes)
            .await
            .unwrap();
        assert_eq!(link_attributes.kind, SandboxObjectKind::Symlink);
        assert_eq!(followed_attributes.kind, SandboxObjectKind::File);

        let target_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        let link_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20_000);
        filesystem
            .set_path_times(
                SandboxPath::at_root("link"),
                SandboxFollow::Yes,
                SandboxTimeChanges {
                    accessed: SandboxTimeChange::Keep,
                    modified: SandboxTimeChange::Set(target_time),
                },
            )
            .await
            .unwrap();
        filesystem
            .set_path_times(
                SandboxPath::at_root("link"),
                SandboxFollow::No,
                SandboxTimeChanges {
                    accessed: SandboxTimeChange::Keep,
                    modified: SandboxTimeChange::Set(link_time),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(filesystem.root().join("target"))
                .unwrap()
                .modified()
                .unwrap(),
            target_time
        );
        assert_eq!(
            std::fs::symlink_metadata(filesystem.root().join("link"))
                .unwrap()
                .modified()
                .unwrap(),
            link_time
        );

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn root_relative_operations_share_one_capability_without_descriptor_duplication() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let root_directory = filesystem.root_directory_state();
        let storage_profile = filesystem.storage_profile();
        let (first, second) = execute_native(storage_profile, NativeOperation::Open, move || {
            let target = SandboxPath::at_root(".");
            Ok::<_, std::io::Error>((
                directory_for(&root_directory, &target)?,
                directory_for(&root_directory, &target)?,
            ))
        })
        .await
        .unwrap()
        .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        drop((first, second));
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn root_relative_seed_reuses_root_without_descriptor_duplication() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let host_source = host_directory_at(parent.path(), ".seed-source").await;
        std::fs::write(
            host_source.path().as_path().join("source"),
            b"seeded contents",
        )
        .unwrap();
        let source = || host_child(&host_source, "source");
        let root_directory = directory_for(
            &filesystem.root_directory_state(),
            &SandboxPath::at_root("."),
        )
        .unwrap();
        let probe = CapabilityCopyParentProbe::install(root_directory);

        seed_one(
            &filesystem,
            seed_entry(
                source(),
                "destination",
                SeedAccess::ReadOnly,
                OnExisting::Fail,
            ),
        )
        .await
        .unwrap();

        assert_eq!(
            probe.reused_base(),
            Some(true),
            "root-relative seed duplicated the root capability"
        );
        assert_eq!(
            std::fs::read(filesystem.root().join("destination")).unwrap(),
            b"seeded contents"
        );
        assert!(
            std::fs::metadata(filesystem.root().join("destination"))
                .unwrap()
                .permissions()
                .readonly()
        );

        let existing = seed_one(
            &filesystem,
            seed_entry(
                source(),
                "destination",
                SeedAccess::ReadWrite,
                OnExisting::Fail,
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
        assert_eq!(
            std::fs::read(filesystem.root().join("destination")).unwrap(),
            b"seeded contents"
        );
        let escaped_path = filesystem.root().parent().unwrap().join("escaped-seed");
        let escaped = seed_one(
            &filesystem,
            seed_entry(
                source(),
                "../escaped-seed",
                SeedAccess::ReadWrite,
                OnExisting::Fail,
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(
            escaped.io_kind(),
            Some(std::io::ErrorKind::PermissionDenied)
        );
        assert!(!escaped_path.exists());
        assert!(std::fs::read_dir(filesystem.root()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".golem-copy-")
        }));

        drop(probe);
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn root_close_linearizes_access_and_preserves_admitted_capability_lifetime() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(filesystem.root().join("existing"), b"contents").unwrap();
        let descriptor = open_native_directory(&filesystem, ".").await;
        let root_directory = filesystem.root_directory_state();
        let lookup_root = Arc::clone(&root_directory);
        let (admitted_tx, admitted_rx) = std::sync::mpsc::sync_channel(0);
        let (continue_tx, continue_rx) = std::sync::mpsc::sync_channel(0);
        let admitted_lookup = std::thread::spawn(move || {
            let admitted = directory_for(&lookup_root, &SandboxPath::at_root(".")).unwrap();
            admitted_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
            admitted.metadata("existing").is_ok()
        });
        admitted_rx.recv().unwrap();

        filesystem.root.close();

        let rejected = directory_for(&root_directory, &SandboxPath::at_root("."));
        continue_tx.send(()).unwrap();
        let admitted_remained_usable = admitted_lookup.join().unwrap();
        assert_eq!(
            rejected.unwrap_err().kind(),
            std::io::ErrorKind::NotConnected,
            "lookup after close must fail"
        );
        assert!(
            admitted_remained_usable,
            "lookup admitted before close lost its capability"
        );
        filesystem
            .create_directory(SandboxPath::at(descriptor.clone(), "descriptor-relative"))
            .await
            .unwrap();
        let rejected = filesystem
            .create_directory(SandboxPath::at_root("root-relative"))
            .await
            .unwrap_err();
        assert_eq!(rejected.io_kind(), Some(std::io::ErrorKind::NotConnected));
        assert!(
            descriptor
                .host()
                .unwrap()
                .metadata("descriptor-relative")
                .is_ok()
        );

        drop(descriptor);
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn shared_root_capability_keeps_root_relative_paths_confined() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let escaped = filesystem.root().parent().unwrap().join("escaped");

        let error = filesystem
            .create_directory(SandboxPath::at_root("../escaped"))
            .await
            .unwrap_err();

        assert!(matches!(
            error.io_kind(),
            Some(std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied)
        ));
        assert!(!escaped.exists());
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn concurrent_reads_and_independent_namespace_operations_share_the_root_capability() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(filesystem.root().join("first"), b"first").unwrap();
        std::fs::write(filesystem.root().join("second"), b"second").unwrap();

        let (first, second, _, _) = tokio::try_join!(
            filesystem.get_path_attributes(SandboxPath::at_root("first"), SandboxFollow::Yes),
            filesystem.get_path_attributes(SandboxPath::at_root("second"), SandboxFollow::Yes),
            filesystem.create_directory(SandboxPath::at_root("left")),
            filesystem.create_directory(SandboxPath::at_root("right")),
        )
        .unwrap();

        assert_eq!(first.size, 5);
        assert_eq!(second.size, 6);
        assert!(filesystem.root().join("left").is_dir());
        assert!(filesystem.root().join("right").is_dir());
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn deletion_revokes_only_its_generation_root_and_retains_exclusive_ownership() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning.clone(),
            name(),
            None,
        )
        .await
        .unwrap();
        let released_root = filesystem.root_directory_state();
        let lease_probe = FilesystemLeaseProbe::install(filesystem.root());
        let second = tokio::spawn(async move {
            <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
                provisioning,
                name(),
                None,
            )
            .await
        });
        lease_probe.wait_attempted().await;
        assert!(
            lease_probe.acquisition_is_pending(),
            "second provisioning acquired ownership before verified deletion"
        );

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
        lease_probe.wait_acquired().await;
        let second = second.await.unwrap().unwrap();

        let released = directory_for(&released_root, &SandboxPath::at_root(".")).unwrap_err();
        assert_eq!(released.kind(), std::io::ErrorKind::NotConnected);
        assert!(directory_for(&second.root_directory_state(), &SandboxPath::at_root(".")).is_ok());
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(second)
            .await
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn deferred_followed_permissions_preserve_raw_terminal_errno() {
        let mut target =
            scripted::default_scripted_namespace_resolution(SandboxPath::at_root("link")).unwrap();
        let source = std::io::Error::from_raw_os_error(libc::EIO);
        target.followed_read_only_file = NativeReadOnlyResolution::Failed {
            kind: source.kind(),
            raw_os_error: source.raw_os_error(),
            message: source.to_string(),
        };

        assert!(target.is_read_only_file(SandboxFollow::No).is_ok());
        let Err(error) = target.is_read_only_file(SandboxFollow::Yes) else {
            panic!("followed permissions unexpectedly discarded their deferred failure")
        };
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(libc::EIO)
        );
        assert!(error.is_terminal_failure());

        let source = include_str!("adapter.rs");
        assert!(source.contains(concat!("raw_os_error: error.", "raw_os_error()")));
        assert!(source.contains(concat!("std::io::Error::from_", "raw_os_error")));
    }

    #[test]
    async fn positioned_write_preserves_the_shared_file_cursor() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let file = match <SandboxFilesystem as SandboxFilesystemAdapter>::open(
            &filesystem,
            SandboxPath::at_root("positioned-write"),
            SandboxOpenOptions::File {
                access: SandboxAccessMode::ReadWrite,
                disposition: SandboxFileDisposition::CreateOrTruncate,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
        .into_node()
        {
            SandboxNode::File(file) => file,
            SandboxNode::Directory(_) => panic!("positioned-write must open as a file"),
        };
        let mut cursor = file
            .host()
            .unwrap()
            .as_ref()
            .try_clone()
            .unwrap()
            .into_std();
        cursor.seek(SeekFrom::Start(11)).unwrap();
        assert_eq!(
            filesystem.append_coordinators.counts(),
            AppendCoordinationCounts {
                lookups: 0,
                allocations: 0,
                lock_acquisitions: 0,
                registered: 0,
                live: 0,
            }
        );

        let attempt = <SandboxFilesystem as SandboxFilesystemAdapter>::write(
            &filesystem,
            &file,
            SandboxWritePlacement::At(2),
            Bytes::from_static(b"data"),
        )
        .await
        .unwrap();

        assert_eq!(attempt.written, 4);
        assert!(attempt.result.is_ok());
        assert_eq!(cursor.stream_position().unwrap(), 11);
        assert_eq!(
            std::fs::read(filesystem.root().join("positioned-write")).unwrap(),
            b"\0\0data"
        );
        assert_eq!(
            filesystem.append_coordinators.counts(),
            AppendCoordinationCounts {
                lookups: 0,
                allocations: 0,
                lock_acquisitions: 0,
                registered: 0,
                live: 0,
            }
        );

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    async fn production_namespace_resolution_pins_semantic_parents_and_does_not_follow_final_symlinks()
     {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::create_dir_all(filesystem.root().join("real/parent")).unwrap();
        std::os::unix::fs::symlink("real/parent", filesystem.root().join("alias")).unwrap();

        let direct_absent = filesystem
            .resolve_namespace_target(SandboxPath::at_root("real/parent/child"))
            .await
            .unwrap();
        let alias_absent = filesystem
            .resolve_namespace_target(SandboxPath::at_root("alias/child"))
            .await
            .unwrap();
        assert!(direct_absent.coordination_key() == alias_absent.coordination_key());
        assert!(direct_absent.final_directory_key().is_none());

        std::fs::create_dir(filesystem.root().join("real/parent/child")).unwrap();
        let opened_parent = filesystem
            .open(
                SandboxPath::at_root("real/parent"),
                SandboxOpenOptions::Existing {
                    expected: SandboxObjectKind::Directory,
                    access: SandboxAccessMode::Read,
                    follow: SandboxFollow::Yes,
                },
            )
            .await
            .unwrap();
        let parent_directory = match opened_parent.into_node() {
            SandboxNode::Directory(directory) => directory,
            SandboxNode::File(_) => panic!("parent must open as a directory"),
        };
        let descriptor_relative = filesystem
            .resolve_namespace_target(SandboxPath::at(parent_directory.clone(), "child"))
            .await
            .unwrap();
        let alias_present = filesystem
            .resolve_namespace_target(SandboxPath::at_root("alias/child"))
            .await
            .unwrap();
        assert!(
            descriptor_relative
                .coordination_key()
                .may_conflict_with(&alias_present.coordination_key())
        );
        assert!(descriptor_relative.final_directory_key() == alias_present.final_directory_key());
        assert!(descriptor_relative.final_directory_key().is_some());

        std::fs::write(filesystem.root().join("real/parent/plain-file"), b"").unwrap();
        std::fs::set_permissions(
            filesystem.root().join("real/parent/plain-file"),
            std::fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        std::fs::write(filesystem.root().join("real/parent/writable-file"), b"").unwrap();
        let final_file = filesystem
            .resolve_namespace_target(SandboxPath::at_root("alias/plain-file"))
            .await
            .unwrap();
        assert!(final_file.final_directory_key().is_none());
        let descriptor_file = filesystem
            .resolve_namespace_target(SandboxPath::at(parent_directory, "plain-file"))
            .await
            .unwrap();
        std::os::unix::fs::symlink(
            "real/parent/plain-file",
            filesystem.root().join("plain-file-alias"),
        )
        .unwrap();
        let final_file_alias = filesystem
            .resolve_namespace_target(SandboxPath::at_root("plain-file-alias"))
            .await
            .unwrap();
        let writable_file = filesystem
            .resolve_namespace_target(SandboxPath::at_root("alias/writable-file"))
            .await
            .unwrap();
        assert!(final_file.is_read_only_file(SandboxFollow::Yes).unwrap());
        assert!(
            descriptor_file
                .is_read_only_file(SandboxFollow::No)
                .unwrap(),
            "a descriptor-relative target must give the permissions of the file"
        );
        assert!(
            final_file_alias
                .is_read_only_file(SandboxFollow::Yes)
                .unwrap(),
            "a followed final symlink must give the permissions of its referent"
        );
        assert!(
            !final_file_alias
                .is_read_only_file(SandboxFollow::No)
                .unwrap(),
            "a symlink that is not followed is not a read-only file"
        );
        assert!(!writable_file.is_read_only_file(SandboxFollow::Yes).unwrap());
        assert!(!alias_present.is_read_only_file(SandboxFollow::No).unwrap());

        std::os::unix::fs::symlink("child", filesystem.root().join("real/parent/final-link"))
            .unwrap();
        let final_symlink = filesystem
            .resolve_namespace_target(SandboxPath::at_root("alias/final-link"))
            .await
            .unwrap();
        assert!(final_symlink.final_directory_key().is_none());

        drop(direct_absent);
        drop(alias_absent);
        drop(descriptor_relative);
        drop(alias_present);
        drop(final_file);
        drop(descriptor_file);
        drop(final_file_alias);
        drop(writable_file);
        drop(final_symlink);
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    async fn production_namespace_resolution_does_not_require_final_directory_search_permission() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let target = filesystem.root().join("locked-directory");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();

        let search_error = std::fs::symlink_metadata(target.join("child")).unwrap_err();
        if search_error.kind() != std::io::ErrorKind::PermissionDenied {
            // Privileged test users may bypass the target directory's mode bits.
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::remove_dir(&target).unwrap();
            <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
                .await
                .unwrap();
            return;
        }

        let resolved = filesystem
            .resolve_namespace_target(SandboxPath::at_root("locked-directory"))
            .await
            .unwrap();
        assert!(resolved.final_directory_key().is_some());
        filesystem
            .rename(resolved.target(), SandboxPath::at_root("renamed-directory"))
            .await
            .unwrap();

        let renamed = filesystem
            .resolve_namespace_target(SandboxPath::at_root("renamed-directory"))
            .await
            .unwrap();
        filesystem.remove_directory(renamed.target()).await.unwrap();

        drop(resolved);
        drop(renamed);
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn namespace_resolution_fails_when_the_parent_cannot_be_searched() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        let locked = filesystem.root().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();

        let resolved = filesystem
            .resolve_namespace_target(SandboxPath::at_root("locked/child"))
            .await;

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            resolved.is_err(),
            "a metadata error other than not found must fail the resolution"
        );
        drop(resolved);
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn following_a_symlink_loop_fails_and_a_dangling_symlink_is_not_read_only() {
        let parent = tempfile::tempdir().unwrap();
        let filesystem = unmanaged_provisioning(parent.path().to_path_buf())
            .create_fresh(name())
            .await
            .unwrap();
        std::os::unix::fs::symlink("loop", filesystem.root().join("loop")).unwrap();
        std::os::unix::fs::symlink("missing", filesystem.root().join("dangling")).unwrap();

        let looped = filesystem
            .resolve_namespace_target(SandboxPath::at_root("loop"))
            .await
            .unwrap();
        let dangling = filesystem
            .resolve_namespace_target(SandboxPath::at_root("dangling"))
            .await
            .unwrap();

        assert!(
            looped.is_read_only_file(SandboxFollow::Yes).is_err(),
            "following a symlink loop must fail"
        );
        assert!(!looped.is_read_only_file(SandboxFollow::No).unwrap());
        assert!(
            !dangling
                .is_read_only_file(SandboxFollow::Yes)
                .expect("a dangling symlink must resolve to no file"),
        );
        drop(looped);
        drop(dangling);
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
    }

    #[test]
    fn namespace_resolution_final_identity_probe_is_metadata_only() {
        let source = include_str!("adapter.rs");
        let resolver = source
            .split_once("fn resolve_host_namespace_target(")
            .unwrap()
            .1
            .split_once("fn split_namespace_target(")
            .unwrap()
            .0;

        assert!(resolver.contains("parent.symlink_metadata(&name)"));
        assert!(!resolver.contains("parent.open_dir_nofollow(&name)"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    async fn concurrent_first_append_coordination_converges_across_hard_links() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(filesystem.root().join("file"), b"").unwrap();
        std::fs::hard_link(
            filesystem.root().join("file"),
            filesystem.root().join("hard-link"),
        )
        .unwrap();
        let files = vec![
            open_native_file(&filesystem, "file").await,
            open_native_file(&filesystem, "file").await,
            open_native_file(&filesystem, "hard-link").await,
            open_native_file(&filesystem, "hard-link").await,
        ];
        assert_eq!(filesystem.append_coordinators.counts().lookups, 0);
        assert_eq!(filesystem.append_coordinators.counts().allocations, 0);

        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let tasks = files
            .into_iter()
            .map(|file| {
                let release = Arc::clone(&release);
                tokio::spawn(async move {
                    let _guard = file.coordinate_append().await;
                    release.acquire().await.unwrap().forget();
                })
            })
            .collect::<Vec<_>>();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while filesystem.append_coordinators.counts().lookups < tasks.len() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let racing = filesystem.append_coordinators.counts();
        assert_eq!(racing.allocations, 1);
        assert_eq!(racing.lock_acquisitions, 1);
        assert_eq!(racing.registered, 1);
        assert_eq!(racing.live, 1);

        release.add_permits(tasks.len());
        for task in tasks {
            task.await.unwrap();
        }
        let completed = filesystem.append_coordinators.counts();
        assert_eq!(completed.allocations, 1);
        assert_eq!(completed.lock_acquisitions, 4);
        assert_eq!(completed.live, 0);

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    async fn append_coordination_tracks_identity_through_rename_unlink_and_path_reuse() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        let original_path = filesystem.root().join("object");
        let reused_path = filesystem.root().join("reused");
        std::fs::write(&original_path, b"original").unwrap();
        let original = open_native_file(&filesystem, "object").await;
        let original_guard = original.coordinate_append().await;

        std::fs::rename(&original_path, &reused_path).unwrap();
        let renamed = open_native_file(&filesystem, "reused").await;
        let renamed_acquired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let renamed_task = tokio::spawn({
            let renamed_acquired = Arc::clone(&renamed_acquired);
            async move {
                let _guard = renamed.coordinate_append().await;
                renamed_acquired.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while filesystem.append_coordinators.counts().lookups < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!renamed_acquired.load(std::sync::atomic::Ordering::Relaxed));

        std::fs::remove_file(&reused_path).unwrap();
        std::fs::write(&reused_path, b"replacement").unwrap();
        let replacement = open_native_file(&filesystem, "reused").await;
        let replacement_guard = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            replacement.coordinate_append(),
        )
        .await
        .expect("path reuse must not inherit the unlinked object's coordinator");
        assert_eq!(filesystem.append_coordinators.counts().allocations, 2);

        drop(replacement_guard);
        drop(original_guard);
        renamed_task.await.unwrap();
        let counts = filesystem.append_coordinators.counts();
        assert_eq!(counts.lookups, 3);
        assert_eq!(counts.allocations, 2);
        assert_eq!(counts.lock_acquisitions, 3);

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    async fn distinct_append_identities_are_independent() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(filesystem.root().join("first"), b"").unwrap();
        std::fs::write(filesystem.root().join("second"), b"").unwrap();
        let first = open_native_file(&filesystem, "first").await;
        let second = open_native_file(&filesystem, "second").await;

        let first_guard = first.coordinate_append().await;
        let second_guard = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            second.coordinate_append(),
        )
        .await
        .expect("distinct native identities must not share append coordination");
        let counts = filesystem.append_coordinators.counts();
        assert_eq!(counts.allocations, 2);
        assert_eq!(counts.lock_acquisitions, 2);

        drop((first_guard, second_guard));
        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    async fn append_coordinator_registry_cleans_expired_weak_entries() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
            provisioning,
            name(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(filesystem.root().join("first"), b"").unwrap();
        std::fs::write(filesystem.root().join("second"), b"").unwrap();
        let first = open_native_file(&filesystem, "first").await;
        let second = open_native_file(&filesystem, "second").await;

        let first_guard = first.coordinate_append().await;
        assert_eq!(filesystem.append_coordinators.counts().live, 1);
        drop(first_guard);
        let expired = filesystem.append_coordinators.counts();
        assert_eq!(expired.registered, 1);
        assert_eq!(expired.live, 0);

        let second_guard = second.coordinate_append().await;
        let cleaned = filesystem.append_coordinators.counts();
        assert_eq!(cleaned.allocations, 2);
        assert_eq!(cleaned.registered, 1);
        assert_eq!(cleaned.live, 1);
        drop(second_guard);

        let first_guard = first.coordinate_append().await;
        let reused_descriptor = filesystem.append_coordinators.counts();
        assert_eq!(reused_descriptor.allocations, 3);
        assert_eq!(reused_descriptor.registered, 1);
        assert_eq!(reused_descriptor.live, 1);
        drop(first_guard);

        <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
            .await
            .unwrap();
    }
}
