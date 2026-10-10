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

use cap_fs_ext::DirExt as _;
use golem_common::model::RetryConfig;
use golem_common::retries::RetryState;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub(crate) use crate::services::golem_config::FilesystemStorageMode;

mod adapter;
#[cfg(target_os = "macos")]
mod apfs;
mod directories;
mod host_directory;
mod tree_copy;

#[allow(unused_imports)]
pub(crate) use adapter::*;
pub(crate) use host_directory::{HostDirectories, HostDirectory, HostPath};
pub(crate) use tree_copy::TreeExclusions;

#[cfg(target_os = "linux")]
mod xfs;

static FILESYSTEM_LEASES: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilesystemStorageErrorKind {
    General,
    AllocationUnsupported,
    SandboxEscape,
}

pub struct FilesystemStorageError {
    inner: Box<FilesystemStorageErrorInner>,
}

/// The facts of one [`FilesystemStorageError`]. They stay behind one box, so that the error is one
/// pointer wide.
struct FilesystemStorageErrorInner {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
    task_failed: bool,
    kind: FilesystemStorageErrorKind,
}

impl FilesystemStorageError {
    pub(crate) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        // cap-primitives exposes its escape refusal as a private string payload. OS permission
        // failures retain their errno and must not be classified as sandbox refusals.
        let kind = if source.kind() == std::io::ErrorKind::PermissionDenied
            && source.raw_os_error().is_none()
            && source.to_string() == "a path led outside of the filesystem"
        {
            FilesystemStorageErrorKind::SandboxEscape
        } else {
            FilesystemStorageErrorKind::General
        };
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: Some(source),
                cleanup_failed: false,
                task_failed: false,
                kind,
            }),
        }
    }

    pub(crate) fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: None,
                cleanup_failed: false,
                task_failed: false,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    pub(crate) fn allocation_unsupported(path: &Path) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation: "observe allocation without quota authority",
                path: path.to_path_buf(),
                source: None,
                cleanup_failed: false,
                task_failed: false,
                kind: FilesystemStorageErrorKind::AllocationUnsupported,
            }),
        }
    }

    pub(crate) fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: Some(source),
                cleanup_failed: true,
                task_failed: false,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    fn cleanup_verification(operation: &'static str, path: &Path) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: None,
                cleanup_failed: true,
                task_failed: false,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    fn task_failure(operation: &'static str, path: &Path, source: NativeExecutionError) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: Some(std::io::Error::other(source)),
                cleanup_failed: false,
                task_failed: true,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    /// Gives the same error about `path`, for an operation that reached the path under another
    /// name, such as a path below `/proc/self/fd/`.
    fn about(mut self, path: &Path) -> Self {
        self.inner.path = path.to_path_buf();
        self
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.inner.cleanup_failed
    }

    pub(crate) fn is_storage_exhaustion(&self) -> bool {
        self.inner.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        })
    }

    pub(crate) fn is_terminal_failure(&self) -> bool {
        self.inner.task_failed
            || (!self.is_sandbox_escape()
                && self.inner.source.as_ref().is_some_and(|source| {
                    matches!(
                        source.kind(),
                        std::io::ErrorKind::InvalidData
                            | std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::ReadOnlyFilesystem
                    ) || is_terminal_storage_errno(source)
                }))
    }

    pub(crate) fn is_sandbox_escape(&self) -> bool {
        self.inner.kind == FilesystemStorageErrorKind::SandboxEscape
    }

    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.inner.source.as_ref().map(std::io::Error::kind)
    }

    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.inner.source.as_ref()
    }

    pub(crate) fn allocation_is_unsupported(&self) -> bool {
        self.inner.kind == FilesystemStorageErrorKind::AllocationUnsupported
    }
}

const MAX_SHORT_TRANSFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeStorageProfile {
    KnownLocal,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeOperation {
    Metadata,
    Open,
    Namespace,
    Read(usize),
    Write(usize),
    DirectoryEnumeration,
    RecursiveCleanup,
    Flush,
    Quota,
    TreeCopy,
}

impl NativeOperation {
    fn is_short(self) -> bool {
        match self {
            Self::Metadata | Self::Open | Self::Namespace => true,
            Self::Read(bytes) | Self::Write(bytes) => bytes <= MAX_SHORT_TRANSFER_BYTES,
            Self::DirectoryEnumeration
            | Self::RecursiveCleanup
            | Self::Flush
            | Self::Quota
            | Self::TreeCopy => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeExecutionClass {
    BlockInPlace,
    SpawnBlocking,
}

fn select_native_execution(
    profile: NativeStorageProfile,
    operation: NativeOperation,
    multi_thread_runtime: bool,
) -> NativeExecutionClass {
    if !operation.is_short() {
        return NativeExecutionClass::SpawnBlocking;
    }
    if profile == NativeStorageProfile::KnownLocal && multi_thread_runtime {
        NativeExecutionClass::BlockInPlace
    } else {
        NativeExecutionClass::SpawnBlocking
    }
}

#[derive(Debug)]
pub(crate) struct NativeExecutionError {
    message: String,
}

impl NativeExecutionError {
    fn panic() -> Self {
        Self {
            message: "sandbox filesystem task panicked".to_string(),
        }
    }

    fn join(error: tokio::task::JoinError) -> Self {
        Self {
            message: format!("sandbox filesystem task failed: {error}"),
        }
    }
}

impl Display for NativeExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NativeExecutionError {}

pub(crate) async fn execute_native<F, R>(
    profile: NativeStorageProfile,
    operation: NativeOperation,
    task: F,
) -> Result<R, NativeExecutionError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let multi_thread_runtime = tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    match select_native_execution(profile, operation, multi_thread_runtime) {
        NativeExecutionClass::BlockInPlace => tokio::task::block_in_place(|| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                .map_err(|_| NativeExecutionError::panic())
        }),
        NativeExecutionClass::SpawnBlocking => tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                .map_err(|_| NativeExecutionError::panic())
        })
        .await
        .map_err(NativeExecutionError::join)?,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn is_terminal_storage_errno(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(errno) if matches!(errno, libc::EIO | libc::ESTALE | libc::ENODEV))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn is_terminal_storage_errno(_error: &std::io::Error) -> bool {
    false
}

impl Debug for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemStorageError")
            .field("operation", &self.inner.operation)
            .field("path", &self.inner.path)
            .field("source", &self.inner.source)
            .field("cleanup_failed", &self.inner.cleanup_failed)
            .field("task_failed", &self.inner.task_failed)
            .field("kind", &self.inner.kind)
            .finish()
    }
}

impl Display for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to {} filesystem {}",
            self.inner.operation,
            self.inner.path.display()
        )?;
        if let Some(source) = &self.inner.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner
            .source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone)]
pub(crate) struct FilesystemVolume {
    mode: FilesystemVolumeMode,
}

#[derive(Clone)]
enum FilesystemVolumeMode {
    UnmanagedDevelopment,
    /// The checked XFS root on Linux, or the probed APFS development root on macOS.
    CopyOnWrite {
        root: Arc<File>,
        #[cfg(target_os = "linux")]
        identity: FilesystemIdentity,
    },
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FilesystemIdentity {
    device: u64,
}

impl FilesystemVolume {
    fn unmanaged_development() -> Self {
        Self {
            mode: FilesystemVolumeMode::UnmanagedDevelopment,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn copy_on_write(
        root: Arc<File>,
        #[cfg(target_os = "linux")] identity: FilesystemIdentity,
    ) -> Self {
        Self {
            mode: FilesystemVolumeMode::CopyOnWrite {
                root,
                #[cfg(target_os = "linux")]
                identity,
            },
        }
    }

    /// Whether the volume makes copy-on-write copies of files: `copy_contents` and `seed` then
    /// share extents instead of copying bytes.
    pub(crate) fn copies_on_write(&self) -> bool {
        volume_facts(&self.mode).copies_on_write
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn copy_on_write_root(&self) -> Option<&Arc<File>> {
        match &self.mode {
            FilesystemVolumeMode::CopyOnWrite { root, .. } => Some(root),
            FilesystemVolumeMode::UnmanagedDevelopment => None,
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
pub(crate) struct FilesystemAllocation {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
}

#[derive(Clone)]
pub(crate) struct SandboxFilesystemAllocationObserver {
    root: PathBuf,
    #[cfg(target_os = "linux")]
    volume: FilesystemVolume,
    quota_authority: QuotaAuthority,
}

impl SandboxFilesystemAllocationObserver {
    fn new(filesystem: &SandboxFilesystem) -> Self {
        Self {
            root: filesystem.root().to_path_buf(),
            #[cfg(target_os = "linux")]
            volume: filesystem.volume.clone(),
            quota_authority: filesystem.quota_authority,
        }
    }

    async fn observe(&self) -> Result<FilesystemAllocation, FilesystemStorageError> {
        let QuotaAuthority::Project {
            project_id: _project_id,
            ..
        } = self.quota_authority
        else {
            return Err(FilesystemStorageError::allocation_unsupported(&self.root));
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root.clone();
            let volume = self.volume.clone();
            execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Quota,
                move || xfs::project_allocation(&volume, _project_id),
            )
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "observe managed XFS project allocation",
                    &root,
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io(
                    "observe managed XFS project allocation",
                    &self.root,
                    error,
                )
            })
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemLimits {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InstalledLimits {
    pub limits: FilesystemLimits,
    pub allocation: FilesystemAllocation,
}

pub(crate) struct SandboxFilesystem {
    root: NativeRoot,
    lease: ExclusiveFilesystemLease,
    volume: FilesystemVolume,
    quota_authority: QuotaAuthority,
    name_mode_source: NativeNameModeSource,
    name_mode_probe: NativeNameModeProbe,
    append_coordinators: Arc<AppendCoordinatorRegistry>,
}

#[derive(Clone, Copy)]
enum NativeNameModeSource {
    NativeDetection,
    #[cfg(target_os = "linux")]
    ValidatedXfs(xfs::ValidatedXfsNameMode),
}

#[derive(Clone, Default)]
struct NativeNameModeProbe {
    #[cfg(test)]
    count: Arc<std::sync::atomic::AtomicUsize>,
}

impl NativeNameModeProbe {
    fn record(&self) {
        #[cfg(test)]
        self.count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum NativeFileIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows {
        volume_serial_number: u32,
        file_index: u64,
    },
    #[cfg(test)]
    Scripted(String),
}

enum AppendCoordinatorAction {
    Reuse(Arc<AsyncMutex<()>>),
    Allocate,
}

fn keep_append_coordinator(strong_count: usize) -> bool {
    strong_count != 0
}

fn decide_append_coordinator(existing: Option<Arc<AsyncMutex<()>>>) -> AppendCoordinatorAction {
    match existing {
        Some(coordinator) => AppendCoordinatorAction::Reuse(coordinator),
        None => AppendCoordinatorAction::Allocate,
    }
}

#[cfg(test)]
fn live_append_coordinators(strong_counts: impl Iterator<Item = usize>) -> usize {
    strong_counts
        .filter(|count| keep_append_coordinator(*count))
        .count()
}

#[derive(Default)]
struct AppendCoordinatorRegistry {
    coordinators: Mutex<HashMap<NativeFileIdentity, Weak<AsyncMutex<()>>>>,
    #[cfg(test)]
    lookups: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    allocations: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    lock_acquisitions: std::sync::atomic::AtomicUsize,
}

impl AppendCoordinatorRegistry {
    fn coordinator(&self, identity: NativeFileIdentity) -> Arc<AsyncMutex<()>> {
        let mut coordinators = self
            .coordinators
            .lock()
            .expect("sandbox filesystem append coordinator lock poisoned");
        #[cfg(test)]
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        coordinators.retain(|_, coordinator| keep_append_coordinator(coordinator.strong_count()));
        let observed = coordinators.get(&identity).and_then(Weak::upgrade);
        match decide_append_coordinator(observed) {
            AppendCoordinatorAction::Reuse(coordinator) => coordinator,
            AppendCoordinatorAction::Allocate => {
                let coordinator = Arc::new(AsyncMutex::new(()));
                coordinators.insert(identity, Arc::downgrade(&coordinator));
                #[cfg(test)]
                self.allocations
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                coordinator
            }
        }
    }

    #[cfg(test)]
    fn record_lock_acquisition(&self) {
        self.lock_acquisitions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    fn counts(&self) -> AppendCoordinationCounts {
        use std::sync::atomic::Ordering;

        let coordinators = self
            .coordinators
            .lock()
            .expect("sandbox filesystem append coordinator lock poisoned");
        AppendCoordinationCounts {
            lookups: self.lookups.load(Ordering::Relaxed),
            allocations: self.allocations.load(Ordering::Relaxed),
            lock_acquisitions: self.lock_acquisitions.load(Ordering::Relaxed),
            registered: coordinators.len(),
            live: live_append_coordinators(coordinators.values().map(Weak::strong_count)),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AppendCoordinationCounts {
    pub(crate) lookups: usize,
    pub(crate) allocations: usize,
    pub(crate) lock_acquisitions: usize,
    pub(crate) registered: usize,
    pub(crate) live: usize,
}

struct NativeRoot {
    path: PathBuf,
    directory: Arc<Mutex<Option<Arc<cap_std::fs::Dir>>>>,
}

impl NativeRoot {
    fn new(path: PathBuf, directory: File) -> Self {
        Self {
            path,
            directory: Arc::new(Mutex::new(Some(Arc::new(cap_std::fs::Dir::from_std_file(
                directory,
            ))))),
        }
    }

    fn close(&self) {
        self.directory
            .lock()
            .expect("sandbox filesystem root descriptor lock poisoned")
            .take();
    }
}

struct ExclusiveFilesystemLease {
    state: Mutex<Option<LeaseState>>,
}

struct LeaseState {
    lifecycle: OwnedMutexGuard<()>,
    cleanup: NativeCleanup,
}

struct RestoringLeaseState<'a> {
    slot: &'a Mutex<Option<LeaseState>>,
    state: Option<LeaseState>,
}

impl<'a> RestoringLeaseState<'a> {
    fn take(slot: &'a Mutex<Option<LeaseState>>) -> Option<Self> {
        let state = slot
            .lock()
            .expect("sandbox filesystem lease lock poisoned")
            .take()?;
        Some(Self {
            slot,
            state: Some(state),
        })
    }

    fn cleanup(&mut self) -> &mut NativeCleanup {
        &mut self
            .state
            .as_mut()
            .expect("armed lease state must be present")
            .cleanup
    }

    fn disarm(mut self) {
        drop(
            self.state
                .take()
                .expect("armed lease state must be present")
                .lifecycle,
        );
    }
}

impl Drop for RestoringLeaseState<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            *self
                .slot
                .lock()
                .expect("sandbox filesystem lease lock poisoned") = Some(state);
        }
    }
}

enum NativeCleanup {
    Directory {
        path: PathBuf,
        cleanup_retry: RetryConfig,
    },
    #[cfg(target_os = "linux")]
    Managed(Box<xfs::ManagedProjectCleanup>),
}

impl NativeCleanup {
    async fn delete(&mut self) -> Result<(), FilesystemStorageError> {
        match self {
            Self::Directory {
                path,
                cleanup_retry,
            } => remove_and_verify(path, "delete runtime directory", cleanup_retry).await,
            #[cfg(target_os = "linux")]
            Self::Managed(cleanup) => cleanup.delete().await,
        }
    }

    fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError> {
        match self {
            Self::Directory { path, .. } => {
                remove_and_verify_blocking(path, "delete runtime directory")
            }
            #[cfg(target_os = "linux")]
            Self::Managed(cleanup) => cleanup.delete_blocking(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileCopyMode {
    Reflink,
    Buffered,
}

/// Who enforces and measures the limits of a sandbox. A sandbox with a project identity charges
/// its project. A sandbox without one has no per-sandbox accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaAuthority {
    Unsupported,
    Project {
        project_id: NonZeroU32,
        filesystem_block_bytes: NonZeroU64,
    },
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

/// Gives the storage profile of the native calls on a sandbox on `volume`.
fn storage_profile(volume: &FilesystemVolume) -> NativeStorageProfile {
    volume_facts(&volume.mode).storage_profile
}

/// Gives how the files of a sandbox on `volume` are copied.
fn file_copy_mode(volume: &FilesystemVolume) -> FileCopyMode {
    volume_facts(&volume.mode).file_copy_mode
}

/// How one seeded file gets its contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedTransfer {
    /// The bytes are copied.
    Bytes,
    /// The file shares the extents of the source.
    Reflink,
    /// The file shares the extents of the source and must belong to this project, which is then
    /// charged for them.
    ReflinkIntoProject(NonZeroU32),
}

/// Gives how a seeded file gets its contents. `mode` is how the sandbox copies files, and
/// `authority` tells whether the sandbox has a project. A copy-on-write sandbox makes a reflink.
/// The reflink goes into the project of the sandbox when it has one.
fn seed_transfer(mode: FileCopyMode, authority: QuotaAuthority) -> SeedTransfer {
    match (mode, authority) {
        (FileCopyMode::Buffered, _) => SeedTransfer::Bytes,
        (FileCopyMode::Reflink, QuotaAuthority::Unsupported) => SeedTransfer::Reflink,
        (FileCopyMode::Reflink, QuotaAuthority::Project { project_id, .. }) => {
            SeedTransfer::ReflinkIntoProject(project_id)
        }
    }
}

/// How the storage accounts for the files of each agent. Each storage mode states it when it is
/// bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentAccounting {
    /// Project quotas enforce the disk limits of each agent and measure its usage.
    ProjectQuotas,
    /// Development storage. It has no per-agent accounting, and a finite disk limit fails the start
    /// of the agent.
    Development,
    /// Production storage without per-agent accounting. It enforces no per-agent disk limit and
    /// measures no per-agent usage.
    Unaccounted,
}

/// Whether the host directories of a storage mode must be checked for a project identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostDirectoryCheck {
    /// Development storage. The host directories are not checked for a project.
    None,
    /// The volume is XFS. A host directory with a project id or the project-inherit flag is
    /// refused.
    NoXfsProject,
}

#[derive(Clone)]
pub(crate) struct SandboxFilesystemProvisioning {
    volume: FilesystemVolume,
    mode: SandboxFilesystemProvisioningMode,
    accounting: AgentAccounting,
    host_directory_check: HostDirectoryCheck,
}

#[derive(Clone)]
enum SandboxFilesystemProvisioningMode {
    Directories(directories::DirectoryProvisioning),
    #[cfg(target_os = "linux")]
    ProjectQuotas(xfs::ManagedProvisioning),
}

impl SandboxFilesystemProvisioning {
    /// Binds the storage mode `storage` and makes the host directories `.scratch` and
    /// `.initial-files` directly under the volume root.
    ///
    /// The binding comes first, so on XFS storage a root that another provisioning holds, or that
    /// fails a startup check, gives an error before any host directory changes. What an earlier
    /// process left under the two names is removed. See [`HostDirectories`].
    pub(crate) async fn provision(
        storage: &FilesystemStorageMode,
        cleanup_retry: RetryConfig,
    ) -> Result<(Self, HostDirectories), FilesystemStorageError> {
        let provisioning = Self::bind(storage, cleanup_retry)?;
        let directories = host_directory::make_host_directories(&provisioning).await?;
        Ok((provisioning, directories))
    }

    /// Binds the storage mode `storage` without host directories.
    #[cfg(test)]
    pub(crate) fn new(
        storage: &FilesystemStorageMode,
        cleanup_retry: RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        Self::bind(storage, cleanup_retry)
    }

    fn bind(
        storage: &FilesystemStorageMode,
        cleanup_retry: RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        let (volume, mode) = match storage {
            FilesystemStorageMode::Temporary => development(None, cleanup_retry),
            FilesystemStorageMode::Directory { root } => {
                development(Some(Arc::from(&**root)), cleanup_retry)
            }
            FilesystemStorageMode::ManagedXfs { root } => configured_managed(root, &cleanup_retry)?,
            FilesystemStorageMode::ReflinkXfs { root } => configured_reflink(root, cleanup_retry)?,
            FilesystemStorageMode::Apfs { root } => configured_apfs(root, cleanup_retry)?,
        };
        let (accounting, host_directory_check) = storage_facts(storage);
        Ok(Self {
            volume,
            mode,
            accounting,
            host_directory_check,
        })
    }

    pub(crate) fn volume(&self) -> &FilesystemVolume {
        &self.volume
    }

    /// How this storage accounts for the files of each agent.
    pub(crate) fn agent_accounting(&self) -> AgentAccounting {
        self.accounting
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
        project_id: NonZeroU32,
    ) -> std::io::Result<FilesystemAllocation> {
        xfs::project_allocation(&self.volume, project_id)
    }
}

/// The facts that the storage mode `storage` states when it is bound: how it accounts for the
/// files of each agent, and whether its host directories are checked for a project.
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

/// Development storage: plain directories under `root`, or in temporary directories without one.
fn development(
    root: Option<Arc<Path>>,
    cleanup_retry: RetryConfig,
) -> (FilesystemVolume, SandboxFilesystemProvisioningMode) {
    (
        FilesystemVolume::unmanaged_development(),
        SandboxFilesystemProvisioningMode::Directories(directories::DirectoryProvisioning::new(
            root,
            cleanup_retry,
            NativeNameModeSource::NativeDetection,
        )),
    )
}

#[cfg(target_os = "linux")]
fn configured_managed(
    root: &Path,
    cleanup_retry: &RetryConfig,
) -> Result<(FilesystemVolume, SandboxFilesystemProvisioningMode), FilesystemStorageError> {
    let managed = xfs::ManagedProvisioning::new(xfs::XfsRoot::open(root)?, cleanup_retry)?;
    let volume = managed.volume().clone();
    Ok((
        volume,
        SandboxFilesystemProvisioningMode::ProjectQuotas(managed),
    ))
}

#[cfg(target_os = "linux")]
fn configured_reflink(
    root: &Path,
    cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, SandboxFilesystemProvisioningMode), FilesystemStorageError> {
    let (volume, directories) = xfs::bind_reflink(xfs::XfsRoot::open(root)?, cleanup_retry)?;
    Ok((
        volume,
        SandboxFilesystemProvisioningMode::Directories(directories),
    ))
}

#[cfg(not(target_os = "linux"))]
fn configured_managed(
    root: &Path,
    _cleanup_retry: &RetryConfig,
) -> Result<(FilesystemVolume, SandboxFilesystemProvisioningMode), FilesystemStorageError> {
    Err(FilesystemStorageError::verification(
        "initialize XFS storage on a non-Linux platform",
        root,
    ))
}

#[cfg(not(target_os = "linux"))]
fn configured_reflink(
    root: &Path,
    _cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, SandboxFilesystemProvisioningMode), FilesystemStorageError> {
    Err(FilesystemStorageError::verification(
        "initialize XFS storage on a non-Linux platform",
        root,
    ))
}

#[cfg(target_os = "macos")]
fn configured_apfs(
    root: &Path,
    cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, SandboxFilesystemProvisioningMode), FilesystemStorageError> {
    let (volume, directories) = apfs::bind(root, cleanup_retry)?;
    Ok((
        volume,
        SandboxFilesystemProvisioningMode::Directories(directories),
    ))
}

#[cfg(not(target_os = "macos"))]
fn configured_apfs(
    root: &Path,
    _cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, SandboxFilesystemProvisioningMode), FilesystemStorageError> {
    Err(FilesystemStorageError::verification(
        "initialize APFS storage on a non-macOS platform",
        root,
    ))
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
            Err(FilesystemStorageError::verification(
                "validate sandbox filesystem name",
                Path::new("<filesystem-name>"),
            ))
        }
    }

    fn relative_path(&self) -> PathBuf {
        self.components.iter().collect()
    }

    #[cfg(target_os = "linux")]
    fn components(&self) -> [&str; 3] {
        self.components.each_ref().map(String::as_str)
    }
}

impl SandboxFilesystem {
    fn new(
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

    #[cfg(test)]
    fn name_mode_probe_count(&self) -> usize {
        self.name_mode_probe.count()
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root.path
    }

    pub(crate) async fn observe_allocation(
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
            execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Quota,
                move || xfs::project_allocation(&volume, _project_id),
            )
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "observe managed XFS project allocation",
                    &root,
                    error,
                )
            })?
            .map(Some)
            .map_err(|error| {
                FilesystemStorageError::io("observe managed XFS project allocation", &root, error)
            })
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }

    pub(crate) async fn install_limits(
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
                self.root(),
            ));
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root().to_path_buf();
            let volume = self.volume.clone();
            let allocation = execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Quota,
                move || {
                    xfs::install_project_limits(
                        &volume,
                        _project_id,
                        _filesystem_block_bytes,
                        _limits,
                    )?;
                    xfs::project_allocation(&volume, _project_id)
                },
            )
            .await
            .map_err(|error| {
                FilesystemStorageError::task_failure(
                    "install managed XFS project limits",
                    &root,
                    error,
                )
            })?
            .map_err(|error| {
                FilesystemStorageError::io("install managed XFS project limits", &root, error)
            })?;
            Ok(InstalledLimits {
                limits: _limits,
                allocation,
            })
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }

    pub(crate) async fn delete_and_verify(self) -> Result<(), DeleteError<Self>> {
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

pub(crate) async fn observe_space(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, FilesystemStorageError> {
    if matches!(&volume.mode, FilesystemVolumeMode::UnmanagedDevelopment) {
        return Ok(FilesystemSpace::Unlimited);
    }
    let volume = volume.clone();
    execute_native(
        NativeStorageProfile::KnownLocal,
        NativeOperation::Quota,
        move || observe_space_blocking(&volume),
    )
    .await
    .map_err(|error| {
        FilesystemStorageError::task_failure(
            "observe filesystem volume space",
            Path::new("<filesystem-volume>"),
            error,
        )
    })?
}

pub(crate) fn observe_space_blocking(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, FilesystemStorageError> {
    let observed = match &volume.mode {
        FilesystemVolumeMode::UnmanagedDevelopment => Ok(FilesystemSpace::Unlimited),
        FilesystemVolumeMode::CopyOnWrite {
            root,
            #[cfg(target_os = "linux")]
            identity,
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
        FilesystemStorageError::io(
            "observe filesystem volume space",
            Path::new("<filesystem-volume>"),
            error,
        )
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

async fn acquire_filesystem_lease(path: &Path) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = FILESYSTEM_LEASES
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .expect("sandbox filesystem lease registry poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(path).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(AsyncMutex::new(()));
                locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    #[cfg(test)]
    let probe = filesystem_lease_probe(path);
    #[cfg(test)]
    if let Some(probe) = &probe {
        probe.attempted.add_permits(1);
    }
    let lifecycle = lock.lock_owned().await;
    #[cfg(test)]
    if let Some(probe) = probe {
        probe.acquired.add_permits(1);
    }
    lifecycle
}

#[cfg(test)]
struct FilesystemLeaseProbeState {
    attempted: tokio::sync::Semaphore,
    acquired: tokio::sync::Semaphore,
}

#[cfg(test)]
struct FilesystemLeaseProbe {
    state: Arc<FilesystemLeaseProbeState>,
}

#[cfg(test)]
impl FilesystemLeaseProbe {
    fn install(path: &Path) -> Self {
        let state = Arc::new(FilesystemLeaseProbeState {
            attempted: tokio::sync::Semaphore::new(0),
            acquired: tokio::sync::Semaphore::new(0),
        });
        filesystem_lease_probes()
            .lock()
            .expect("sandbox filesystem lease probe registry poisoned")
            .insert(path.to_path_buf(), Arc::downgrade(&state));
        Self { state }
    }

    async fn wait_attempted(&self) {
        self.state
            .attempted
            .acquire()
            .await
            .expect("sandbox filesystem lease attempt probe closed")
            .forget();
    }

    fn acquisition_is_pending(&self) -> bool {
        self.state.acquired.available_permits() == 0
    }

    async fn wait_acquired(&self) {
        self.state
            .acquired
            .acquire()
            .await
            .expect("sandbox filesystem lease acquisition probe closed")
            .forget();
    }
}

#[cfg(test)]
fn filesystem_lease_probes() -> &'static Mutex<HashMap<PathBuf, Weak<FilesystemLeaseProbeState>>> {
    static PROBES: OnceLock<Mutex<HashMap<PathBuf, Weak<FilesystemLeaseProbeState>>>> =
        OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn filesystem_lease_probe(path: &Path) -> Option<Arc<FilesystemLeaseProbeState>> {
    filesystem_lease_probes()
        .lock()
        .expect("sandbox filesystem lease probe registry poisoned")
        .get(path)
        .and_then(Weak::upgrade)
}

enum CapabilityCopyParent<'a> {
    Borrowed(&'a cap_std::fs::Dir),
    Owned(cap_std::fs::Dir),
}

impl CapabilityCopyParent<'_> {
    fn as_dir(&self) -> &cap_std::fs::Dir {
        match self {
            Self::Borrowed(directory) => directory,
            Self::Owned(directory) => directory,
        }
    }

    #[cfg(test)]
    fn borrows(&self, directory: &cap_std::fs::Dir) -> bool {
        matches!(self, Self::Borrowed(parent) if std::ptr::eq(*parent, directory))
    }
}

fn create_capability_copy_parent<'a>(
    base: &'a cap_std::fs::Dir,
    target: &Path,
    transfer: SeedTransfer,
) -> std::io::Result<(CapabilityCopyParent<'a>, Box<Path>)> {
    let mut components = target.components().peekable();
    let mut parent = CapabilityCopyParent::Borrowed(base);
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "file-copy target contains an invalid path component",
            ));
        };
        if components.peek().is_none() {
            #[cfg(test)]
            record_capability_copy_parent(base, &parent);
            return Ok((parent, Path::new(component).into()));
        }
        match parent.as_dir().symlink_metadata(component) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                parent = CapabilityCopyParent::Owned(parent.as_dir().open_dir_nofollow(component)?);
            }
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "file-copy parent is not a directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let child = parent
                    .as_dir()
                    .create_dir(component)
                    .and_then(|()| parent.as_dir().open_dir_nofollow(component))
                    .and_then(|child| {
                        tree_copy::sync_seed_directory(transfer, &child).map(|()| child)
                    });
                // The parent may have changed even if opening or syncing the new child failed.
                let synced = tree_copy::sync_seed_directory(transfer, parent.as_dir());
                parent =
                    CapabilityCopyParent::Owned(child.and_then(|child| synced.map(|()| child))?);
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "file-copy target has no file name",
    ))
}

#[cfg(test)]
type CapabilityCopyParentObservationState = Mutex<Option<bool>>;

#[cfg(test)]
type CapabilityCopyParentObservation = Arc<CapabilityCopyParentObservationState>;

#[cfg(test)]
type CapabilityCopyParentProbeRegistry =
    Mutex<HashMap<usize, Weak<CapabilityCopyParentObservationState>>>;

#[cfg(test)]
struct CapabilityCopyParentProbe {
    _directory: Arc<cap_std::fs::Dir>,
    observation: CapabilityCopyParentObservation,
}

#[cfg(test)]
impl CapabilityCopyParentProbe {
    fn install(directory: Arc<cap_std::fs::Dir>) -> Self {
        let observation = Arc::new(Mutex::new(None));
        capability_copy_parent_probes()
            .lock()
            .expect("capability copy-parent probe registry poisoned")
            .insert(
                Arc::as_ptr(&directory) as usize,
                Arc::downgrade(&observation),
            );
        Self {
            _directory: directory,
            observation,
        }
    }

    fn reused_base(&self) -> Option<bool> {
        *self
            .observation
            .lock()
            .expect("capability copy-parent probe observation poisoned")
    }
}

#[cfg(test)]
fn capability_copy_parent_probes() -> &'static CapabilityCopyParentProbeRegistry {
    static PROBES: OnceLock<CapabilityCopyParentProbeRegistry> = OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn record_capability_copy_parent(base: &cap_std::fs::Dir, parent: &CapabilityCopyParent<'_>) {
    let observation = capability_copy_parent_probes()
        .lock()
        .expect("capability copy-parent probe registry poisoned")
        .get(&(base as *const cap_std::fs::Dir as usize))
        .and_then(Weak::upgrade);
    if let Some(observation) = observation {
        *observation
            .lock()
            .expect("capability copy-parent probe observation poisoned") =
            Some(parent.borrows(base));
    }
}

struct CapabilityTempFile<'a> {
    directory: &'a cap_std::fs::Dir,
    name: Option<Box<Path>>,
    file: cap_std::fs::File,
}

impl<'a> CapabilityTempFile<'a> {
    fn new(directory: &'a cap_std::fs::Dir) -> std::io::Result<Self> {
        loop {
            let name =
                PathBuf::from(format!(".golem-copy-{}", uuid::Uuid::new_v4())).into_boxed_path();
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            match directory.open_with(&name, &options) {
                Ok(file) => {
                    return Ok(Self {
                        directory,
                        name: Some(name),
                        file,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn from_clone(directory: &'a cap_std::fs::Dir, source: &File) -> std::io::Result<Self> {
        let name = std::iter::repeat_with(|| {
            PathBuf::from(format!(".golem-copy-{}", uuid::Uuid::new_v4())).into_boxed_path()
        })
        .find_map(|name| match apfs::clone_file(source, directory, &name) {
            Ok(()) => Some(Ok(name)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
            Err(error) => Some(Err(error)),
        })
        .expect("an unbounded clone-name iterator must return a result")?;
        match directory.open(&name) {
            Ok(file) => Ok(Self {
                directory,
                name: Some(name),
                file,
            }),
            Err(error) => {
                directory.remove_file(&name)?;
                Err(error)
            }
        }
    }

    fn as_file(&self) -> &cap_std::fs::File {
        &self.file
    }

    fn as_file_mut(&mut self) -> &mut cap_std::fs::File {
        &mut self.file
    }

    fn persist_noclobber(mut self, destination: &Path) -> std::io::Result<()> {
        let name = self
            .name
            .as_ref()
            .expect("capability temporary file name missing");
        self.directory
            .hard_link(name, self.directory, destination)?;
        self.directory.remove_file(name)?;
        self.name = None;
        Ok(())
    }

    /// Gives the file the name `destination`, in place of what is at that name, as
    /// [`tree_copy::clear_for_replacement`] decides.
    fn persist_replacing(mut self, destination: &Path) -> std::io::Result<()> {
        let name = self
            .name
            .as_ref()
            .expect("capability temporary file name missing");
        tree_copy::clear_for_replacement(self.directory, destination)?;
        self.directory.rename(name, self.directory, destination)?;
        self.name = None;
        Ok(())
    }
}

impl Drop for CapabilityTempFile<'_> {
    fn drop(&mut self) {
        if let Some(name) = self.name.take() {
            let _ = self.directory.remove_file(name);
        }
    }
}

pub(crate) fn set_file_permissions(file: &File, read_only: bool) -> std::io::Result<()> {
    let mut permissions = file.metadata()?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if read_only { 0o444 } else { 0o644 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(read_only);
    file.set_permissions(permissions)
}

async fn verify_fresh_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }
    verify_empty_directory(path).await
}

async fn verify_fresh_open_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }
    verify_empty_directory(path).await
}

async fn verify_empty_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", path, error)
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| FilesystemStorageError::io("verify empty runtime directory", path, error))?
        .is_some()
    {
        return Err(FilesystemStorageError::verification(
            "verify empty runtime directory",
            path,
        ));
    }
    Ok(())
}

async fn rollback_created_filesystem(
    filesystem: SandboxFilesystem,
    creation_error: FilesystemStorageError,
) -> FilesystemStorageError {
    match SandboxFilesystem::delete_and_verify(filesystem).await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error.into_source(),
    }
}

async fn rollback_creation(
    path: &Path,
    creation_error: FilesystemStorageError,
    cleanup_retry: &RetryConfig,
) -> FilesystemStorageError {
    match remove_and_verify(path, "roll back runtime directory", cleanup_retry).await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

async fn remove_and_verify(
    path: &Path,
    operation: &'static str,
    cleanup_retry: &RetryConfig,
) -> Result<(), FilesystemStorageError> {
    let mut retry = RetryState::new(cleanup_retry);
    loop {
        retry.start_attempt();
        match remove_and_verify_once(path, operation).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !retry.failed_attempt().await {
                    return Err(error);
                }
            }
        }
    }
}

async fn remove_and_verify_once(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Ok(_) => {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }

    verify_absent(path, operation)
}

fn remove_and_verify_blocking(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path)
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Ok(_) => {
            std::fs::remove_file(path)
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }
    verify_absent(path, operation)
}

fn verify_absent(path: &Path, operation: &'static str) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            operation, path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }
}

#[cfg(all(test, unix))]
fn running_as_root() -> bool {
    // SAFETY: `geteuid` has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_storage_errors_are_terminal_only_when_the_storage_is_unavailable() {
        [libc::EIO, libc::ESTALE, libc::ENODEV]
            .into_iter()
            .for_each(|errno| {
                assert!(is_terminal_storage_errno(
                    &std::io::Error::from_raw_os_error(errno)
                ));
            });
        [libc::ENOSPC, libc::EAGAIN, libc::EINTR, libc::EXDEV]
            .into_iter()
            .for_each(|errno| {
                assert!(!is_terminal_storage_errno(
                    &std::io::Error::from_raw_os_error(errno)
                ));
            });
    }

    #[test]
    fn each_storage_mode_states_its_accounting_and_its_host_directory_check() {
        let root: Box<Path> = Box::from(Path::new("/var/lib/golem/agents"));
        assert_eq!(
            [
                FilesystemStorageMode::Temporary,
                FilesystemStorageMode::Directory { root: root.clone() },
                FilesystemStorageMode::ManagedXfs { root: root.clone() },
                FilesystemStorageMode::ReflinkXfs { root },
            ]
            .iter()
            .map(storage_facts)
            .collect::<Vec<_>>(),
            [
                (AgentAccounting::Development, HostDirectoryCheck::None),
                (AgentAccounting::Development, HostDirectoryCheck::None),
                (
                    AgentAccounting::ProjectQuotas,
                    HostDirectoryCheck::NoXfsProject
                ),
                (
                    AgentAccounting::Unaccounted,
                    HostDirectoryCheck::NoXfsProject
                ),
            ]
        );
    }

    #[test]
    fn an_error_about_another_path_keeps_its_operation_and_cause() {
        let error = FilesystemStorageError::io(
            "remove XFS reflink probe",
            Path::new("/proc/self/fd/7/.golem-xfs-reflink-probe"),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        )
        .about(Path::new("/var/lib/golem/agents/.golem-xfs-reflink-probe"));

        assert_eq!(
            error.to_string(),
            format!(
                "failed to remove XFS reflink probe filesystem \
                 /var/lib/golem/agents/.golem-xfs-reflink-probe: {}",
                std::io::Error::from(std::io::ErrorKind::PermissionDenied)
            )
        );
        assert_eq!(error.io_kind(), Some(std::io::ErrorKind::PermissionDenied));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn apfs_startup_refuses_other_platforms_without_creating_the_root() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("agents").into_boxed_path();
        let storage = FilesystemStorageMode::Apfs { root: root.clone() };
        let error = SandboxFilesystemProvisioning::new(&storage, RetryConfig::default())
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("initialize APFS storage on a non-macOS platform")
        );
        assert!(!root.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apfs_startup_refuses_a_file_as_its_root_with_a_clear_error() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("agents").into_boxed_path();
        std::fs::write(&root, b"not a directory").unwrap();
        let storage = FilesystemStorageMode::Apfs { root: root.clone() };
        let error = SandboxFilesystemProvisioning::new(&storage, RetryConfig::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("create APFS development root"));
        assert!(error.to_string().contains(root.to_str().unwrap()));
        assert_eq!(std::fs::read(&root).unwrap(), b"not a directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apfs_startup_makes_and_probes_a_development_root() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("agents").into_boxed_path();
        let storage = FilesystemStorageMode::Apfs { root: root.clone() };
        let provisioning = SandboxFilesystemProvisioning::new(&storage, RetryConfig::default())
            .expect("APFS must start on the existing Mac volume");

        assert!(root.is_dir());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(
            provisioning.agent_accounting(),
            AgentAccounting::Unaccounted
        );
        assert_eq!(provisioning.host_directory_check, HostDirectoryCheck::None);
        assert!(provisioning.volume().copies_on_write());
        assert!(matches!(
            observe_space_blocking(provisioning.volume()).unwrap(),
            FilesystemSpace::Observed { total_bytes, available_bytes, .. }
                if total_bytes > 0 && available_bytes > 0 && available_bytes <= total_bytes
        ));
        assert!(SandboxFilesystemProvisioning::new(&storage, RetryConfig::default()).is_ok());
    }

    #[test]
    fn development_storage_has_development_accounting_and_no_project_check() {
        let root = tempfile::tempdir().unwrap();
        [
            FilesystemStorageMode::Temporary,
            FilesystemStorageMode::Directory {
                root: root.path().into(),
            },
        ]
        .iter()
        .for_each(|storage| {
            let provisioning =
                SandboxFilesystemProvisioning::new(storage, RetryConfig::default()).unwrap();
            assert_eq!(
                provisioning.agent_accounting(),
                AgentAccounting::Development
            );
            assert_eq!(provisioning.host_directory_check, HostDirectoryCheck::None);
        });
    }

    /// A volume on a copy-on-write root, for the rules that read only the mode of the volume.
    #[cfg(target_os = "linux")]
    fn copy_on_write_volume() -> FilesystemVolume {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        FilesystemVolume::copy_on_write(Arc::new(root), FilesystemIdentity { device: 1 })
    }

    #[test]
    fn a_development_volume_mode_states_its_facts() {
        assert_eq!(
            volume_facts(&FilesystemVolumeMode::UnmanagedDevelopment),
            VolumeFacts {
                copies_on_write: false,
                storage_profile: NativeStorageProfile::Unknown,
                file_copy_mode: FileCopyMode::Buffered,
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_on_write_volume_mode_states_its_facts() {
        assert_eq!(
            volume_facts(&copy_on_write_volume().mode),
            VolumeFacts {
                copies_on_write: true,
                storage_profile: NativeStorageProfile::KnownLocal,
                file_copy_mode: FileCopyMode::Reflink,
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn storage_profile_follows_the_volume() {
        assert_eq!(
            storage_profile(&FilesystemVolume::unmanaged_development()),
            NativeStorageProfile::Unknown
        );
        assert_eq!(
            storage_profile(&copy_on_write_volume()),
            NativeStorageProfile::KnownLocal
        );
    }

    #[test]
    fn a_development_volume_makes_no_copy_on_write_copies() {
        assert!(!FilesystemVolume::unmanaged_development().copies_on_write());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_on_write_volume_makes_copy_on_write_copies() {
        assert!(copy_on_write_volume().copies_on_write());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_copy_mode_follows_the_volume() {
        assert_eq!(
            file_copy_mode(&FilesystemVolume::unmanaged_development()),
            FileCopyMode::Buffered
        );
        assert_eq!(
            file_copy_mode(&copy_on_write_volume()),
            FileCopyMode::Reflink
        );
    }

    #[test]
    fn seed_transfer_reflinks_on_copy_on_write_storage_into_the_project_when_there_is_one() {
        let project_id = NonZeroU32::new(7).unwrap();
        let project = QuotaAuthority::Project {
            project_id,
            filesystem_block_bytes: NonZeroU64::new(4096).unwrap(),
        };
        assert_eq!(
            [
                seed_transfer(FileCopyMode::Buffered, QuotaAuthority::Unsupported),
                seed_transfer(FileCopyMode::Buffered, project),
                seed_transfer(FileCopyMode::Reflink, QuotaAuthority::Unsupported),
                seed_transfer(FileCopyMode::Reflink, project),
            ],
            [
                SeedTransfer::Bytes,
                SeedTransfer::Bytes,
                SeedTransfer::Reflink,
                SeedTransfer::ReflinkIntoProject(project_id),
            ]
        );
    }

    fn name() -> SandboxFilesystemName {
        SandboxFilesystemName::new(
            "environment".to_string(),
            "component".to_string(),
            "filesystem".to_string(),
        )
        .unwrap()
    }

    fn unmanaged_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
        SandboxFilesystemProvisioning::new(
            &FilesystemStorageMode::Directory { root: root.into() },
            RetryConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn unsupported_allocation_classification_is_typed() {
        let path = Path::new("<test>");
        let unsupported = FilesystemStorageError::allocation_unsupported(path);
        let same_message = FilesystemStorageError::verification(
            "observe allocation without quota authority",
            path,
        );

        assert!(unsupported.allocation_is_unsupported());
        assert!(!same_message.allocation_is_unsupported());
        assert_eq!(unsupported.to_string(), same_message.to_string());
    }

    #[test]
    fn native_execution_classification_is_conservative() {
        for operation in [
            NativeOperation::Metadata,
            NativeOperation::Open,
            NativeOperation::Namespace,
            NativeOperation::Read(MAX_SHORT_TRANSFER_BYTES),
            NativeOperation::Write(MAX_SHORT_TRANSFER_BYTES),
        ] {
            assert_eq!(
                select_native_execution(NativeStorageProfile::KnownLocal, operation, true,),
                NativeExecutionClass::BlockInPlace,
                "{operation:?} must use block_in_place"
            );
        }
        for operation in [
            NativeOperation::Read(MAX_SHORT_TRANSFER_BYTES + 1),
            NativeOperation::Write(MAX_SHORT_TRANSFER_BYTES + 1),
            NativeOperation::DirectoryEnumeration,
            NativeOperation::RecursiveCleanup,
            NativeOperation::Flush,
            NativeOperation::Quota,
            NativeOperation::TreeCopy,
        ] {
            assert_eq!(
                select_native_execution(NativeStorageProfile::KnownLocal, operation, true,),
                NativeExecutionClass::SpawnBlocking,
                "{operation:?} must stay on spawn_blocking"
            );
        }
        assert_eq!(
            select_native_execution(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Open,
                false,
            ),
            NativeExecutionClass::SpawnBlocking
        );
        assert_eq!(
            select_native_execution(
                NativeStorageProfile::Unknown,
                NativeOperation::Namespace,
                true,
            ),
            NativeExecutionClass::SpawnBlocking
        );
    }

    #[test]
    fn current_thread_runtime_falls_back_without_block_in_place_panic() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let caller = std::thread::current().id();
            let worker = execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Metadata,
                || std::thread::current().id(),
            )
            .await
            .unwrap();
            assert_ne!(worker, caller);
        });
    }

    #[test]
    fn multi_thread_runtime_uses_block_in_place_for_short_known_local_work() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let caller = std::thread::current().id();
            let worker = execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Metadata,
                || std::thread::current().id(),
            )
            .await
            .unwrap();
            assert_eq!(worker, caller);
        });
    }

    #[test]
    fn native_task_panic_is_caught_and_classified_as_terminal() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let execution_error = runtime
            .block_on(execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Metadata,
                || -> () { panic!("scripted native panic") },
            ))
            .unwrap_err();
        let error = FilesystemStorageError::task_failure(
            "run panicking native operation",
            Path::new("<scripted>"),
            execution_error,
        );

        assert!(error.is_terminal_failure());
        assert_eq!(error.io_kind(), Some(std::io::ErrorKind::Other));
    }

    #[test]
    async fn unmanaged_volume_is_unlimited_without_an_existing_root() {
        let provisioning = unmanaged_provisioning(PathBuf::from("/definitely/not/observed"));

        assert_eq!(
            observe_space(provisioning.volume()).await.unwrap(),
            FilesystemSpace::Unlimited
        );
    }

    #[test]
    async fn unmanaged_creation_replaces_stale_contents_and_deletes_verified() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let stale = parent.path().join(name().relative_path());
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("garbage"), b"stale").unwrap();

        let filesystem = provisioning.create_fresh(name()).await.unwrap();

        assert!(
            std::fs::read_dir(filesystem.root())
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(file_copy_mode(&filesystem.volume), FileCopyMode::Buffered);
        assert!(matches!(
            filesystem.quota_authority,
            QuotaAuthority::Unsupported
        ));
        assert_eq!(
            observe_space(&filesystem.volume).await.unwrap(),
            FilesystemSpace::Unlimited
        );

        let root = filesystem.root().to_path_buf();
        SandboxFilesystem::delete_and_verify(filesystem)
            .await
            .unwrap();
        assert!(!root.exists());
    }

    #[test]
    async fn cancelling_armed_cleanup_restores_lease_state() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let filesystem = provisioning.create_fresh(name()).await.unwrap();

        let mut cleanup = Box::pin(async {
            let _state = RestoringLeaseState::take(&filesystem.lease.state).unwrap();
            std::future::pending::<()>().await;
        });
        assert!(futures::poll!(cleanup.as_mut()).is_pending());
        assert!(filesystem.lease.state.lock().unwrap().is_none());
        drop(cleanup);
        assert!(filesystem.lease.state.lock().unwrap().is_some());

        SandboxFilesystem::delete_and_verify(filesystem)
            .await
            .unwrap();
    }

    #[test]
    async fn unmanaged_creation_serializes_the_same_native_name() {
        let parent = tempfile::tempdir().unwrap();
        let provisioning = unmanaged_provisioning(parent.path().to_path_buf());
        let first = provisioning.create_fresh(name()).await.unwrap();
        let second = tokio::spawn({
            let provisioning = provisioning.clone();
            async move { provisioning.create_fresh(name()).await }
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());

        SandboxFilesystem::delete_and_verify(first).await.unwrap();
        let second = second.await.unwrap().unwrap();
        SandboxFilesystem::delete_and_verify(second).await.unwrap();
    }

    #[test]
    fn storage_error_is_one_pointer_wide() {
        assert_eq!(
            std::mem::size_of::<FilesystemStorageError>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn storage_error_debug_output_names_each_fact() {
        let error =
            FilesystemStorageError::cleanup_verification("remove directory", Path::new("/root/a"));

        assert_eq!(
            format!("{error:?}"),
            "FilesystemStorageError { operation: \"remove directory\", path: \"/root/a\", source: None, cleanup_failed: true, task_failed: false, kind: General }"
        );
    }

    #[test]
    fn native_name_rejects_a_component_that_starts_with_a_dot() {
        [
            [".environment", "component", "filesystem"],
            ["environment", ".component", "filesystem"],
            ["environment", "component", ".filesystem"],
        ]
        .into_iter()
        .for_each(|[environment, component, filesystem]| {
            assert!(
                SandboxFilesystemName::new(
                    environment.to_string(),
                    component.to_string(),
                    filesystem.to_string()
                )
                .is_err(),
                "{environment}/{component}/{filesystem} must be refused"
            );
        });
        assert!(
            SandboxFilesystemName::new(
                "environment.".to_string(),
                "compo.nent".to_string(),
                "filesystem".to_string()
            )
            .is_ok()
        );
    }

    #[test]
    fn native_name_rejects_path_components() {
        assert!(
            SandboxFilesystemName::new("..".to_string(), "component".to_string(), "fs".to_string())
                .is_err()
        );
        assert!(
            SandboxFilesystemName::new(
                "environment".to_string(),
                "a/b".to_string(),
                "fs".to_string()
            )
            .is_err()
        );
    }
}
