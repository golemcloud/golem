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
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

mod adapter;
mod file_update;
mod unmanaged;

#[allow(unused_imports)]
pub(crate) use adapter::*;

#[cfg(target_os = "linux")]
mod xfs;

static FILESYSTEM_LEASES: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilesystemStorageErrorKind {
    General,
    AllocationUnsupported,
}

#[derive(Debug)]
pub struct FilesystemStorageError {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
    task_failed: bool,
    kind: FilesystemStorageErrorKind,
}

impl FilesystemStorageError {
    pub(crate) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: false,
            task_failed: false,
            kind: FilesystemStorageErrorKind::General,
        }
    }

    pub(crate) fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: false,
            task_failed: false,
            kind: FilesystemStorageErrorKind::General,
        }
    }

    pub(crate) fn allocation_unsupported(path: &Path) -> Self {
        Self {
            operation: "observe allocation without quota authority",
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: false,
            task_failed: false,
            kind: FilesystemStorageErrorKind::AllocationUnsupported,
        }
    }

    pub(crate) fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: true,
            task_failed: false,
            kind: FilesystemStorageErrorKind::General,
        }
    }

    fn cleanup_verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: true,
            task_failed: false,
            kind: FilesystemStorageErrorKind::General,
        }
    }

    fn task_failure(operation: &'static str, path: &Path, source: NativeExecutionError) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(std::io::Error::other(source)),
            cleanup_failed: false,
            task_failed: true,
            kind: FilesystemStorageErrorKind::General,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted_task_failure(operation: &'static str) -> Self {
        Self::task_failure(
            operation,
            Path::new("<scripted-native-task>"),
            NativeExecutionError::panic(),
        )
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }

    pub(crate) fn is_storage_exhaustion(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        })
    }

    pub(crate) fn is_terminal_failure(&self) -> bool {
        self.task_failed
            || self.source.as_ref().is_some_and(|source| {
                matches!(
                    source.kind(),
                    std::io::ErrorKind::InvalidData
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::ReadOnlyFilesystem
                ) || is_terminal_storage_errno(source)
            })
    }

    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.source.as_ref().map(std::io::Error::kind)
    }

    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.source.as_ref()
    }

    pub(crate) fn allocation_is_unsupported(&self) -> bool {
        self.kind == FilesystemStorageErrorKind::AllocationUnsupported
    }
}

const MAX_SHORT_TRANSFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeStorageProfile {
    KnownLocal,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeOperation {
    Metadata,
    Open,
    Namespace,
    Read(usize),
    Write(usize),
    DirectoryEnumeration,
    SeedFile,
    FileUpdate,
    RecursiveCleanup,
    Flush,
    Quota,
}

impl NativeOperation {
    fn is_short(self) -> bool {
        match self {
            Self::Metadata | Self::Open | Self::Namespace => true,
            Self::Read(bytes) | Self::Write(bytes) => bytes <= MAX_SHORT_TRANSFER_BYTES,
            Self::DirectoryEnumeration
            | Self::SeedFile
            | Self::FileUpdate
            | Self::RecursiveCleanup
            | Self::Flush
            | Self::Quota => false,
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
struct NativeExecutionError {
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

async fn execute_native<F, R>(
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

#[cfg(target_os = "linux")]
fn is_terminal_storage_errno(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(errno) if matches!(errno, libc::EIO | libc::ESTALE | libc::ENODEV))
}

#[cfg(not(target_os = "linux"))]
fn is_terminal_storage_errno(_error: &std::io::Error) -> bool {
    false
}

impl Display for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to {} filesystem {}",
            self.operation,
            self.path.display()
        )?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
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
    Managed {
        root: Arc<File>,
        identity: FilesystemIdentity,
    },
}

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

    #[cfg(target_os = "linux")]
    fn managed(root: Arc<File>, identity: FilesystemIdentity) -> Self {
        Self {
            mode: FilesystemVolumeMode::Managed { root, identity },
        }
    }

    #[cfg(target_os = "linux")]
    fn managed_root(&self) -> Option<&Arc<File>> {
        match &self.mode {
            FilesystemVolumeMode::Managed { root, .. } => Some(root),
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
        let QuotaAuthority::Project { project_id, .. } = self.quota_authority else {
            return Err(FilesystemStorageError::allocation_unsupported(&self.root));
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root.clone();
            let volume = self.volume.clone();
            execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Quota,
                move || xfs::project_allocation(&volume, project_id),
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
        {
            let _ = project_id;
            unreachable!("managed XFS is unavailable on this platform")
        }
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
    file_copy_mode: FileCopyMode,
    quota_authority: QuotaAuthority,
    name_mode_source: NativeNameModeSource,
    name_mode_probe: NativeNameModeProbe,
    append_coordinators: Arc<AppendCoordinatorRegistry>,
}

#[derive(Clone, Copy)]
enum NativeNameModeSource {
    NativeDetection,
    #[cfg(target_os = "linux")]
    ValidatedManagedXfs(xfs::ValidatedManagedXfsNameMode),
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
        #[cfg(test)]
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut coordinators = self
            .coordinators
            .lock()
            .expect("sandbox filesystem append coordinator lock poisoned");
        coordinators.retain(|_, coordinator| coordinator.strong_count() != 0);
        match coordinators.get(&identity).and_then(Weak::upgrade) {
            Some(coordinator) => coordinator,
            None => {
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
            live: coordinators
                .values()
                .filter(|coordinator| coordinator.strong_count() != 0)
                .count(),
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
    Unmanaged {
        path: PathBuf,
        cleanup_retry: RetryConfig,
    },
    #[cfg(target_os = "linux")]
    Managed(xfs::ManagedProjectCleanup),
}

impl NativeCleanup {
    async fn delete(&mut self) -> Result<(), FilesystemStorageError> {
        match self {
            Self::Unmanaged {
                path,
                cleanup_retry,
            } => remove_and_verify(path, "delete unmanaged runtime directory", cleanup_retry).await,
            #[cfg(target_os = "linux")]
            Self::Managed(cleanup) => cleanup.delete().await,
        }
    }

    fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError> {
        match self {
            Self::Unmanaged { path, .. } => {
                remove_and_verify_blocking(path, "delete unmanaged runtime directory")
            }
            #[cfg(target_os = "linux")]
            Self::Managed(cleanup) => cleanup.delete_blocking(),
        }
    }
}

#[derive(Clone, Copy)]
enum FileCopyMode {
    Reflink,
    Buffered,
}

#[derive(Clone, Copy)]
enum QuotaAuthority {
    Unsupported,
    Project {
        project_id: NonZeroU32,
        filesystem_block_bytes: NonZeroU64,
    },
}

#[derive(Clone)]
pub(crate) struct SandboxFilesystemProvisioning {
    volume: FilesystemVolume,
    mode: SandboxFilesystemProvisioningMode,
}

#[derive(Clone)]
enum SandboxFilesystemProvisioningMode {
    Unmanaged(unmanaged::UnmanagedProvisioning),
    #[cfg(target_os = "linux")]
    Managed(xfs::ManagedProvisioning),
}

impl SandboxFilesystemProvisioning {
    pub(crate) fn new(
        deterministic_root_dir: Option<PathBuf>,
        managed_xfs_root_dir: Option<PathBuf>,
        cleanup_retry: RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        if deterministic_root_dir.is_some() && managed_xfs_root_dir.is_some() {
            return Err(FilesystemStorageError::verification(
                "select exactly one filesystem storage mode",
                Path::new("<configuration>"),
            ));
        }

        match managed_xfs_root_dir.as_deref() {
            Some(root) => configured_managed(root, &cleanup_retry),
            None => {
                let volume = FilesystemVolume::unmanaged_development();
                Ok(Self {
                    volume: volume.clone(),
                    mode: SandboxFilesystemProvisioningMode::Unmanaged(
                        unmanaged::UnmanagedProvisioning::new(
                            deterministic_root_dir,
                            cleanup_retry,
                        ),
                    ),
                })
            }
        }
    }

    pub(crate) fn initial_file_cache_root(&self) -> Option<&Path> {
        match &self.mode {
            SandboxFilesystemProvisioningMode::Unmanaged(_) => None,
            #[cfg(target_os = "linux")]
            SandboxFilesystemProvisioningMode::Managed(managed) => Some(managed.root()),
        }
    }

    pub(crate) fn volume(&self) -> &FilesystemVolume {
        &self.volume
    }

    pub(crate) async fn create_fresh(
        &self,
        name: SandboxFilesystemName,
    ) -> Result<Arc<SandboxFilesystem>, FilesystemStorageError> {
        match &self.mode {
            SandboxFilesystemProvisioningMode::Unmanaged(unmanaged) => {
                unmanaged.create_fresh(self.volume.clone(), name).await
            }
            #[cfg(target_os = "linux")]
            SandboxFilesystemProvisioningMode::Managed(managed) => {
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

#[cfg(target_os = "linux")]
fn configured_managed(
    root: &Path,
    cleanup_retry: &RetryConfig,
) -> Result<SandboxFilesystemProvisioning, FilesystemStorageError> {
    let managed = xfs::ManagedProvisioning::new(root, cleanup_retry)?;
    let volume = managed.volume().clone();
    Ok(SandboxFilesystemProvisioning {
        volume,
        mode: SandboxFilesystemProvisioningMode::Managed(managed),
    })
}

#[cfg(not(target_os = "linux"))]
fn configured_managed(
    root: &Path,
    _cleanup_retry: &RetryConfig,
) -> Result<SandboxFilesystemProvisioning, FilesystemStorageError> {
    Err(FilesystemStorageError::verification(
        "initialize managed XFS storage on a non-Linux platform",
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
            matches!(
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

pub(crate) struct SandboxFileUpdate {
    target: PathBuf,
    source: PathBuf,
    permissions: SandboxFilePermissions,
}

impl SandboxFileUpdate {
    /// Describes one host-file replacement for [`SandboxFilesystem::update_files`].
    ///
    /// `target` is relative to the sandbox filesystem root and `permissions` controls the installed
    /// file's resulting write permission.
    pub(crate) fn new(
        target: PathBuf,
        source: PathBuf,
        permissions: SandboxFilePermissions,
    ) -> Self {
        Self {
            target,
            source,
            permissions,
        }
    }
}

impl SandboxFilesystem {
    fn new(
        root: NativeRoot,
        lease: LeaseState,
        volume: FilesystemVolume,
        file_copy_mode: FileCopyMode,
        quota_authority: QuotaAuthority,
        name_mode_source: NativeNameModeSource,
    ) -> Self {
        Self {
            root,
            lease: ExclusiveFilesystemLease {
                state: Mutex::new(Some(lease)),
            },
            volume,
            file_copy_mode,
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

    /// Returns which candidate root-relative paths currently contain regular files.
    ///
    /// Callers use this before a transactional update when existing writable files should be
    /// preserved rather than replaced.
    pub(crate) async fn existing_file_targets(
        &self,
        targets: Vec<PathBuf>,
    ) -> Result<HashSet<PathBuf>, FilesystemStorageError> {
        let root = self.root().to_path_buf();
        let operation_path = root.clone();
        execute_native(
            self.storage_profile(),
            NativeOperation::FileUpdate,
            move || {
                targets
                    .into_iter()
                    .filter(|target| {
                        std::fs::symlink_metadata(root.join(target))
                            .is_ok_and(|metadata| metadata.is_file())
                    })
                    .collect()
            },
        )
        .await
        .map_err(|error| {
            FilesystemStorageError::task_failure(
                "inspect file update targets",
                &operation_path,
                error,
            )
        })
    }

    /// Applies a transactional set of file replacements and removals.
    ///
    /// `current` identifies targets owned by the previous update. Existing paths outside that set
    /// are preserved. The operation stages replacements, rolls back on failure, and keeps quota
    /// inheritance and destination permissions intact.
    pub(crate) async fn update_files(
        &self,
        current: HashSet<PathBuf>,
        updates: Vec<SandboxFileUpdate>,
        removals: Vec<PathBuf>,
    ) -> Result<(), FilesystemStorageError> {
        let root = self.root().to_path_buf();
        let operation_path = root.clone();
        let copy_mode = self.file_copy_mode;
        let quota_authority = self.quota_authority;
        execute_native(
            self.storage_profile(),
            NativeOperation::FileUpdate,
            move || {
                file_update::apply_update(
                    root,
                    copy_mode,
                    quota_authority,
                    current,
                    updates,
                    removals,
                )
            },
        )
        .await
        .map_err(|error| {
            FilesystemStorageError::task_failure("apply file update", &operation_path, error)
        })?
    }

    pub(crate) async fn observe_allocation(
        &self,
    ) -> Result<Option<FilesystemAllocation>, FilesystemStorageError> {
        let QuotaAuthority::Project { project_id, .. } = self.quota_authority else {
            return Ok(None);
        };
        #[cfg(target_os = "linux")]
        {
            let root = self.root().to_path_buf();
            let volume = self.volume.clone();
            execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::Quota,
                move || xfs::project_allocation(&volume, project_id),
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
        limits: FilesystemLimits,
    ) -> Result<InstalledLimits, FilesystemStorageError> {
        let QuotaAuthority::Project {
            project_id,
            filesystem_block_bytes,
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
                        project_id,
                        filesystem_block_bytes,
                        limits,
                    )?;
                    xfs::project_allocation(&volume, project_id)
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
            Ok(InstalledLimits { limits, allocation })
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!("managed XFS is unavailable on this platform");
    }

    pub(crate) async fn delete_and_verify(&self) -> Result<(), FilesystemStorageError> {
        self.root.close();
        let Some(mut state) = RestoringLeaseState::take(&self.lease.state) else {
            return Ok(());
        };
        state.cleanup().delete().await?;
        state.disarm();
        Ok(())
    }

    pub(crate) fn delete_and_verify_blocking(&self) -> Result<(), FilesystemStorageError> {
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

    fn storage_profile(&self) -> NativeStorageProfile {
        match self.quota_authority {
            QuotaAuthority::Project { .. } => NativeStorageProfile::KnownLocal,
            QuotaAuthority::Unsupported => NativeStorageProfile::Unknown,
        }
    }
}

fn copy_file_blocking(
    copy_mode: FileCopyMode,
    quota_authority: QuotaAuthority,
    materialization_root: &Path,
    source: &Path,
    target: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    match copy_mode {
        FileCopyMode::Buffered => {
            unmanaged::copy_file(materialization_root, source, target, read_only)
        }
        FileCopyMode::Reflink => {
            let QuotaAuthority::Project { project_id, .. } = quota_authority else {
                unreachable!("reflink copy requires project quota authority")
            };
            #[cfg(target_os = "linux")]
            {
                xfs::reflink_file(materialization_root, project_id, source, target, read_only)
            }
            #[cfg(not(target_os = "linux"))]
            unreachable!("managed XFS is unavailable on this platform");
        }
    }
}

fn copy_file_at_blocking(
    copy_mode: FileCopyMode,
    quota_authority: QuotaAuthority,
    materialization_root: &Path,
    source: &Path,
    destination_directory: &cap_std::fs::Dir,
    destination: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    match copy_mode {
        FileCopyMode::Buffered => {
            unmanaged::copy_file_at(destination_directory, source, destination, read_only)
        }
        FileCopyMode::Reflink => {
            let QuotaAuthority::Project { project_id, .. } = quota_authority else {
                unreachable!("reflink copy requires project quota authority")
            };
            #[cfg(target_os = "linux")]
            {
                xfs::reflink_file_at(
                    materialization_root,
                    project_id,
                    destination_directory,
                    source,
                    destination,
                    read_only,
                )
            }
            #[cfg(not(target_os = "linux"))]
            unreachable!("managed XFS is unavailable on this platform");
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
    match &volume.mode {
        FilesystemVolumeMode::UnmanagedDevelopment => Ok(FilesystemSpace::Unlimited),
        FilesystemVolumeMode::Managed { root, identity } => {
            #[cfg(target_os = "linux")]
            {
                let root = Arc::clone(root);
                let identity = *identity;
                execute_native(
                    NativeStorageProfile::KnownLocal,
                    NativeOperation::Quota,
                    move || xfs::observe_space(&root, identity),
                )
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "observe managed filesystem space",
                        Path::new("<managed-volume>"),
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed filesystem space",
                        Path::new("<managed-volume>"),
                        error,
                    )
                })
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (root, identity);
                unreachable!("managed XFS is unavailable on this platform")
            }
        }
    }
}

pub(crate) fn observe_space_blocking(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, FilesystemStorageError> {
    match &volume.mode {
        FilesystemVolumeMode::UnmanagedDevelopment => Ok(FilesystemSpace::Unlimited),
        FilesystemVolumeMode::Managed { root, identity } => {
            #[cfg(target_os = "linux")]
            {
                xfs::observe_space(root, *identity).map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed filesystem space",
                        Path::new("<managed-volume>"),
                        error,
                    )
                })
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (root, identity);
                unreachable!("managed XFS is unavailable on this platform")
            }
        }
    }
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

fn create_copy_parent<'a>(root: &Path, target: &'a Path) -> std::io::Result<&'a Path> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file-copy target has no parent",
        )
    })?;
    let relative = parent.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file-copy target escapes the sandbox filesystem",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "file-copy target contains an invalid path component",
            ));
        };
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "file-copy parent is not a directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(parent)
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
) -> std::io::Result<(CapabilityCopyParent<'a>, PathBuf)> {
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
            return Ok((parent, PathBuf::from(component)));
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
                parent.as_dir().create_dir(component)?;
                parent = CapabilityCopyParent::Owned(parent.as_dir().open_dir_nofollow(component)?);
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
    directory: CapabilityCopyParent<'a>,
    name: Option<PathBuf>,
    file: cap_std::fs::File,
}

impl<'a> CapabilityTempFile<'a> {
    fn new(directory: CapabilityCopyParent<'a>) -> std::io::Result<Self> {
        loop {
            let name = PathBuf::from(format!(".golem-copy-{}", uuid::Uuid::new_v4()));
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            match directory.as_dir().open_with(&name, &options) {
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
            .as_dir()
            .hard_link(name, self.directory.as_dir(), destination)?;
        self.directory.as_dir().remove_file(name)?;
        self.name = None;
        Ok(())
    }
}

impl Drop for CapabilityTempFile<'_> {
    fn drop(&mut self) {
        if let Some(name) = self.name.take() {
            let _ = self.directory.as_dir().remove_file(name);
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
            NativeOperation::SeedFile,
            NativeOperation::FileUpdate,
            NativeOperation::RecursiveCleanup,
            NativeOperation::Flush,
            NativeOperation::Quota,
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
        assert!(matches!(filesystem.file_copy_mode, FileCopyMode::Buffered));
        assert!(matches!(
            filesystem.quota_authority,
            QuotaAuthority::Unsupported
        ));
        assert_eq!(
            observe_space(&filesystem.volume).await.unwrap(),
            FilesystemSpace::Unlimited
        );

        let root = filesystem.root().to_path_buf();
        SandboxFilesystem::delete_and_verify(&filesystem)
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

        SandboxFilesystem::delete_and_verify(&filesystem)
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

        SandboxFilesystem::delete_and_verify(&first).await.unwrap();
        let second = second.await.unwrap().unwrap();
        SandboxFilesystem::delete_and_verify(&second).await.unwrap();
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
