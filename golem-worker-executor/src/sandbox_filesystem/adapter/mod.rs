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
use cap_fs_ext::{
    FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt, OpenOptionsMaybeDirExt,
    OpenOptionsSyncExt,
};
use cap_std::fs::FileExt as _;
use fs_set_times::{SetTimes as _, SystemTimeSpec};
use std::ffi::OsString;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::{Seek, SeekFrom, Write};
use std::time::SystemTime;

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
    read_only_file: bool,
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

    /// Tells whether the opened object is a regular file without write permission.
    pub(crate) fn is_read_only_file(&self) -> bool {
        self.read_only_file
    }

    #[cfg(test)]
    pub(crate) fn scripted_file(id: u64) -> Self {
        Self {
            node: SandboxNode::File(SandboxFile::scripted(id)),
            read_only_file: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_read_only_file(id: u64) -> Self {
        Self {
            node: SandboxNode::File(SandboxFile::scripted(id)),
            read_only_file: true,
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
                read_only_file: false,
            },
            Self {
                node: SandboxNode::File(file(second_id, coordinators)),
                read_only_file: false,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn scripted_directory(id: u64) -> Self {
        Self {
            node: SandboxNode::Directory(SandboxDirectory::scripted(id)),
            read_only_file: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_directory_with_identity(id: u64, identity: u64) -> Self {
        Self {
            node: SandboxNode::Directory(SandboxDirectory::scripted_with_identity(
                id,
                format!("programmed-directory-{identity}"),
            )),
            read_only_file: false,
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
    name: Box<std::ffi::OsStr>,
    coordination_key: SandboxNamespaceCoordinationKey,
    parent_byte_exact_lookup_proven: bool,
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
    /// Reports a positive proof that lookup in this parent requires the exact stored spelling.
    /// False means no such proof. This does not establish any ancestor's spelling.
    pub(crate) fn has_byte_exact_lookup_proof(&self) -> bool {
        self.parent_byte_exact_lookup_proven
    }

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
        SandboxPath::at(self.parent.clone(), PathBuf::from(self.name.as_ref()))
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
                    &self.parent.path.join(self.name.as_ref()),
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

/// What an open asks for, from its options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpenRequest {
    /// The kind of object that the open expects.
    expected: SandboxObjectKind,
    access: SandboxAccessMode,
    follow: SandboxFollow,
    /// The creation or truncation rule of a file open, and `None` when the open creates and
    /// truncates nothing.
    disposition: Option<SandboxFileDisposition>,
    /// Whether the open is an inspection.
    inspection: bool,
}

/// Gives what `options` ask for.
///
/// An inspection reads its object, does not follow a symlink, and creates nothing. An open of an
/// existing object creates nothing. A file open expects a regular file and has a disposition.
fn open_request(options: SandboxOpenOptions) -> OpenRequest {
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
fn is_single_normal_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(name)) if name == path.as_os_str())
        && components.next().is_none()
}

/// The flags of a native open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeOpenFlags {
    read: bool,
    write: bool,
    create: bool,
    create_new: bool,
    truncate: bool,
    nonblock: bool,
    follow: bool,
}

/// Gives the native open flags of `options`.
///
/// A directory is opened for read only, whatever access the options ask for. A file is opened
/// for read with `Read` and `ReadAndSetTimes`, for write with `Write`, and for both with
/// `ReadWrite`. Each disposition writes: `CreateIfMissing` creates a missing file,
/// `CreateExclusive` creates a new file and fails on an existing one, `TruncateExisting` truncates
/// an existing file, and `CreateOrTruncate` does both of the first and the third. An inspection
/// does not block on a special file.
fn native_open_flags(options: SandboxOpenOptions) -> NativeOpenFlags {
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
fn check_opened_kind(
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

pub(crate) struct DeleteError<Adapter> {
    adapter: Adapter,
    source: FilesystemStorageError,
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
    fn root_directory_state(&self) -> Arc<Mutex<Option<NativeRootHandles>>> {
        Arc::clone(&self.root.directory)
    }
}

fn node_path(node: &SandboxNode) -> PathBuf {
    match node {
        SandboxNode::File(file) => file.path.clone(),
        SandboxNode::Directory(directory) => directory.path.clone(),
    }
}

fn admit_root(root: &Arc<Mutex<Option<NativeRootHandles>>>) -> std::io::Result<NativeRootHandles> {
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

fn directory_for(
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
        name: name.into_boxed_os_str(),
        parent_byte_exact_lookup_proven: coordination_key.name.mode
            == NativeNameComparisonMode::Exact,
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
            xfs_name_mode_shortcut_enabled(),
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
        // Case sensitivity does not prove byte-exact lookup. APFS also aliases Unicode spellings.
        let _ = (directory, parent, source, probe);
        Ok(NativeNameComparisonMode::Conservative)
    }
}

#[cfg(target_os = "linux")]
const XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED: bool = true;

#[cfg(target_os = "linux")]
fn xfs_name_mode_shortcut_enabled() -> bool {
    let enabled = XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED;
    #[cfg(test)]
    let enabled = enabled && !xfs_name_mode_shortcut_disabled_for_test();
    enabled
}

#[cfg(all(test, target_os = "linux"))]
fn xfs_name_mode_shortcut_disabled_for_test() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_DISABLE_XFS_NAME_MODE_SHORTCUT").as_deref() == Ok("1")
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
        && matches!(source, NativeNameModeSource::ValidatedXfs(proof) if proof.matches_device(parent_device))
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
                        "{{source={}, target={:?}, access={:?}, placement={:?}}}",
                        entry.source.as_path().display(),
                        entry.target,
                        entry.access,
                        entry.placement
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

        async fn delete_and_verify(self) -> Result<(), DeleteError<Self>> {
            match self
                .outcome("delete_and_verify()".to_string(), |state| {
                    &mut state.delete_and_verify
                })
                .await
            {
                Ok(()) => Ok(()),
                Err(source) => Err(DeleteError::new(self, source)),
            }
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
            name: name.into_boxed_os_str(),
            // A programmed canonical label proves equivalence, not byte-exact lookup.
            parent_byte_exact_lookup_proven: false,
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
            parent_byte_exact_lookup_proven: true,
            coordination_key: SandboxNamespaceCoordinationKey {
                parent: parent_key,
                name: NativeNameCoordinationKey {
                    name: name.clone(),
                    mode: NativeNameComparisonMode::Exact,
                },
            },
            name: name.into_boxed_os_str(),
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
mod tests;
