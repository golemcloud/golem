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
use std::ffi::OsStr;

/// The name of the host directory in which each capture and each restore gets its own directory.
const SCRATCH: &str = ".scratch";

/// The name of the host directory that holds the downloads of initial files.
const INITIAL_FILES: &str = ".initial-files";

/// A path on the host under a [`HostDirectory`], on the volume of the sandboxes and outside every
/// agent project.
///
/// A host path does not own what is at the path, and it does not keep the host directory alive.
/// What is at the path can be a file, a directory, a symlink, or nothing. It goes away when the
/// host directory that the path starts at is discarded or dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostPath(Arc<Path>);

impl HostPath {
    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    /// Extends this path by one name.
    ///
    /// The name must be one normal component. An empty name, `.`, `..`, or a name with a
    /// separator gives an `InvalidInput` error, so the new path stays under this path.
    pub(crate) fn child(&self, name: &OsStr) -> Result<HostPath, FilesystemStorageError> {
        if !is_one_normal_component(name) {
            return Err(FilesystemStorageError::io(
                "validate host path name",
                self.as_path(),
                std::io::Error::from(std::io::ErrorKind::InvalidInput),
            ));
        }
        Ok(HostPath(Arc::from(tree_copy::child_path(
            self.as_path(),
            name,
        ))))
    }
}

/// The host directories of a volume. [`SandboxFilesystemProvisioning::provision`] makes both.
///
/// Each is a plain directory directly under the volume root, outside every agent project, with no
/// project id, no quota and no metering.
#[derive(Debug)]
pub(crate) struct HostDirectories {
    /// `.scratch`: each capture and each restore gets its own directory in it.
    pub(crate) scratch: HostDirectory,
    /// `.initial-files`: the downloads of initial files.
    pub(crate) initial_files: HostDirectory,
}

/// A directory on the host that this value owns.
///
/// [`HostDirectory::discard`] removes the directory with all that is in it and checks that it is
/// gone, and gives the failure of either step. A drop without a discard removes the directory as a
/// best effort, does not check, and only logs a failure.
///
/// `.scratch` and `.initial-files` keep the volume root that they are in usable while they live,
/// also after their provisioning is dropped. A directory that [`HostDirectory::create_in`] makes
/// holds no root; it lives in its parent.
#[derive(Debug)]
pub(crate) struct HostDirectory {
    path: HostPath,
    removed: bool,
    _root: Option<Arc<File>>,
    _temporary_root: Option<Arc<tempfile::TempDir>>,
}

impl HostDirectory {
    /// Makes an empty directory with this name in `parent`. Fails if the name exists, or if the
    /// name is not one normal component.
    pub(crate) async fn create_in(
        parent: &HostPath,
        name: &OsStr,
    ) -> Result<HostDirectory, FilesystemStorageError> {
        let path = parent.child(name)?;
        let outcome = execute_native(
            NativeStorageProfile::Unknown,
            NativeOperation::Namespace,
            move || {
                let result = std::fs::create_dir(path.as_path());
                (path, result)
            },
        )
        .await
        .map_err(|error| {
            FilesystemStorageError::task_failure("create host directory", parent.as_path(), error)
        })?;
        match outcome {
            (path, Ok(())) => Ok(HostDirectory {
                path,
                removed: false,
                _root: None,
                _temporary_root: None,
            }),
            (path, Err(error)) => Err(FilesystemStorageError::io(
                "create host directory",
                path.as_path(),
                error,
            )),
        }
    }

    pub(crate) fn path(&self) -> &HostPath {
        &self.path
    }

    /// Removes the directory with all that is in it.
    ///
    /// A directory that is already absent, for example because its parent was removed first, gives
    /// success.
    ///
    /// The native work owns the root that the directory keeps, so a drop of the call never frees
    /// the descriptor of the root while the work can still use the path through it.
    pub(crate) async fn discard(mut self) -> Result<(), FilesystemStorageError> {
        self.removed = true;
        let path = self.path.clone();
        let root = (self._root.clone(), self._temporary_root.clone());
        execute_native(
            NativeStorageProfile::Unknown,
            NativeOperation::RecursiveCleanup,
            move || {
                let _root = root;
                remove_and_verify_blocking(path.as_path(), "discard host directory")
            },
        )
        .await
        .map_err(|error| {
            let mut failure = FilesystemStorageError::task_failure(
                "discard host directory",
                self.path.as_path(),
                error,
            );
            failure.inner.cleanup_failed = true;
            failure
        })?
    }
}

impl Drop for HostDirectory {
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        match std::fs::remove_dir_all(self.path.as_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::error!(
                error = %error,
                path = %self.path.as_path().display(),
                "Failed to remove a dropped host directory"
            ),
        }
    }
}

/// Makes `.scratch` and `.initial-files` directly under the volume root of `provisioning`.
///
/// Without a configured root on development storage, the root is a new temporary directory that the
/// two host directories keep while they live. Removes what an earlier process left under each
/// name first. On XFS storage, a host directory that has a project identity gives an error.
/// When `.initial-files` cannot be made, the function removes the `.scratch` that it made, and a
/// failure of that removal is the error. A leftover that the function could not remove stays.
///
/// The native work owns the descriptor of the volume root, and with it the lock of the root, from
/// the call until it ends, and its result owns them after that. So a drop of the call never lets
/// another open take the number of the descriptor while the work can still use the path through
/// it. A result whose caller is gone is dropped, and that removes both host directories.
pub(super) async fn make_host_directories(
    provisioning: &SandboxFilesystemProvisioning,
) -> Result<HostDirectories, FilesystemStorageError> {
    let error_root = volume_root(provisioning);
    execute_native(
        NativeStorageProfile::Unknown,
        NativeOperation::RecursiveCleanup,
        host_directories_task(provisioning),
    )
    .await
    .map_err(|error| {
        FilesystemStorageError::task_failure(
            "create host directory",
            error_root.as_deref().unwrap_or(Path::new("<temp>")),
            error,
        )
    })?
}

/// The volume root of `provisioning`, or `None` when the host directories get a temporary root.
fn volume_root(provisioning: &SandboxFilesystemProvisioning) -> Option<Arc<Path>> {
    match &provisioning.mode {
        SandboxFilesystemProvisioningMode::Directories(directories) => {
            directories.deterministic_root().cloned()
        }
        #[cfg(target_os = "linux")]
        SandboxFilesystemProvisioningMode::ProjectQuotas(managed) => {
            Some(Arc::clone(managed.root()))
        }
    }
}

/// The native work of [`make_host_directories`]. It owns the root, the descriptor of the volume
/// root and the check of the provisioning.
fn host_directories_task(
    provisioning: &SandboxFilesystemProvisioning,
) -> impl FnOnce() -> Result<HostDirectories, FilesystemStorageError> + Send + 'static {
    let root = volume_root(provisioning);
    #[cfg(target_os = "linux")]
    let anchor = provisioning.volume.copy_on_write_root().cloned();
    #[cfg(not(target_os = "linux"))]
    let anchor = None;
    let verify_no_project = provisioning.host_directory_check == HostDirectoryCheck::NoXfsProject;
    move || make_host_directories_blocking(root, anchor, verify_no_project)
}

/// Makes the root, or a temporary root when `root` is `None`, and then `.scratch` and
/// `.initial-files` in it, as [`make_host_directories`] says. The host directories hold `anchor`.
fn make_host_directories_blocking(
    root: Option<Arc<Path>>,
    anchor: Option<Arc<File>>,
    verify_no_project: bool,
) -> Result<HostDirectories, FilesystemStorageError> {
    let (root, temporary_root) = match root {
        Some(root) => {
            std::fs::create_dir_all(&root).map_err(|error| {
                FilesystemStorageError::io("create host directory root", &root, error)
            })?;
            (root, None)
        }
        None => {
            let temporary = tempfile::Builder::new()
                .prefix("golem-host-directories")
                .tempdir()
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "create temporary host directory root",
                        Path::new("<temp>"),
                        error,
                    )
                })?;
            (Arc::from(temporary.path()), Some(Arc::new(temporary)))
        }
    };
    let scratch = HostPath(Arc::from(root.join(SCRATCH)));
    let initial_files = HostPath(Arc::from(root.join(INITIAL_FILES)));
    make_empty_host_directory(scratch.as_path(), verify_no_project)?;
    if let Err(error) = make_empty_host_directory(initial_files.as_path(), verify_no_project) {
        return Err(error_after_removal(
            error,
            remove_and_verify_blocking(
                scratch.as_path(),
                "remove the scratch directory after a failed host directory",
            ),
        ));
    }
    let directory = |path: HostPath| HostDirectory {
        path,
        removed: false,
        _root: anchor.clone(),
        _temporary_root: temporary_root.clone(),
    };
    Ok(HostDirectories {
        scratch: directory(scratch),
        initial_files: directory(initial_files),
    })
}

/// Gives the error of a step that failed and then removed what it had made. A failed removal is
/// the error, because it leaves a directory behind. After a removal that succeeded, the error of
/// the step is the error.
fn error_after_removal(
    error: FilesystemStorageError,
    removal: Result<(), FilesystemStorageError>,
) -> FilesystemStorageError {
    match removal {
        Ok(()) => error,
        Err(removal_error) => removal_error,
    }
}

fn is_one_normal_component(name: &OsStr) -> bool {
    !name
        .as_encoded_bytes()
        .iter()
        .any(|byte| byte.is_ascii() && std::path::is_separator(char::from(*byte)))
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
}

/// Removes what an earlier process left at `path`, and makes an empty directory there. With
/// `verify_no_project`, a directory that has a project identity is removed again and gives an
/// error.
fn make_empty_host_directory(
    path: &Path,
    verify_no_project: bool,
) -> Result<(), FilesystemStorageError> {
    remove_and_verify_blocking(
        path,
        "remove what an earlier process left under a host directory name",
    )?;
    std::fs::create_dir(path)
        .map_err(|error| FilesystemStorageError::io("create host directory", path, error))?;
    #[cfg(target_os = "linux")]
    if verify_no_project && let Err(error) = xfs::verify_host_directory_has_no_project(path) {
        return Err(error_after_removal(
            error,
            remove_and_verify_blocking(path, "remove a host directory that has a project identity"),
        ));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = verify_no_project;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    use test_r::test;

    async fn provision(root: Option<&Path>) -> (SandboxFilesystemProvisioning, HostDirectories) {
        SandboxFilesystemProvisioning::provision(
            &root.map_or(FilesystemStorageMode::Temporary, |root| {
                FilesystemStorageMode::Directory { root: root.into() }
            }),
            RetryConfig::default(),
        )
        .await
        .unwrap()
    }

    async fn scratch_at(root: &Path) -> HostDirectory {
        provision(Some(root)).await.1.scratch
    }

    fn is_empty_directory(path: &Path) -> bool {
        std::fs::read_dir(path).unwrap().next().is_none()
    }

    #[test]
    fn a_failed_removal_is_the_error_after_a_failed_step() {
        let step = || {
            FilesystemStorageError::io(
                "make a host directory",
                Path::new("/step"),
                std::io::Error::from(ErrorKind::PermissionDenied),
            )
        };
        let removal = FilesystemStorageError::cleanup_io(
            "remove a host directory",
            Path::new("/removal"),
            std::io::Error::from(ErrorKind::PermissionDenied),
        );

        assert_eq!(
            error_after_removal(step(), Ok(())).to_string(),
            step().to_string()
        );
        let failed = error_after_removal(step(), Err(removal));
        assert!(failed.cleanup_failed(), "{failed}");
        assert!(
            failed.to_string().contains("remove a host directory"),
            "{failed}"
        );
    }

    #[test]
    async fn provision_makes_a_root_that_does_not_exist_yet() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("not/yet/there");

        let (_provisioning, directories) = provision(Some(&root)).await;

        assert!(is_empty_directory(directories.scratch.path().as_path()));
        assert!(is_empty_directory(
            directories.initial_files.path().as_path()
        ));
        assert_eq!(directories.scratch.path().as_path(), root.join(".scratch"));
    }

    #[test]
    fn host_directory_names_are_one_component_that_starts_with_a_dot() {
        [SCRATCH, INITIAL_FILES].into_iter().for_each(|name| {
            assert!(is_one_normal_component(OsStr::new(name)), "{name}");
            assert!(name.starts_with('.'), "{name}");
            assert!(
                SandboxFilesystemName::new(name.to_string(), "c".to_string(), "f".to_string())
                    .is_err(),
                "an agent directory must not be able to take the name {name}"
            );
        });
        assert_ne!(SCRATCH, INITIAL_FILES);
    }

    #[test]
    async fn provision_makes_both_host_directories_and_removes_what_an_earlier_process_left() {
        let root = tempfile::tempdir().unwrap();
        [SCRATCH, INITIAL_FILES].into_iter().for_each(|name| {
            let leftover = root.path().join(name).join("nested");
            std::fs::create_dir_all(&leftover).unwrap();
            std::fs::write(leftover.join("garbage"), b"stale").unwrap();
        });

        let (_provisioning, directories) = provision(Some(root.path())).await;

        assert_eq!(
            directories.scratch.path().as_path(),
            root.path().join(".scratch")
        );
        assert_eq!(
            directories.initial_files.path().as_path(),
            root.path().join(".initial-files")
        );
        assert!(is_empty_directory(&root.path().join(".scratch")));
        assert!(is_empty_directory(&root.path().join(".initial-files")));
    }

    #[cfg(unix)]
    #[test]
    async fn provision_removes_the_scratch_directory_when_the_initial_files_directory_cannot_be_made()
     {
        if running_as_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join(".initial-files/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("file"), b"stale").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

        let failure = SandboxFilesystemProvisioning::provision(
            &FilesystemStorageMode::Directory {
                root: root.path().into(),
            },
            RetryConfig::default(),
        )
        .await
        .err()
        .expect("a .initial-files that cannot be made must fail the provisioning");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(failure.cleanup_failed(), "{failure}");
        assert!(
            !root.path().join(".scratch").exists(),
            "a failed .initial-files must leave no .scratch"
        );
    }

    #[test]
    async fn a_temporary_root_lives_while_a_host_directory_lives() {
        let (provisioning, directories) = provision(None).await;
        let HostDirectories {
            scratch,
            initial_files,
        } = directories;

        let root = scratch.path().as_path().parent().unwrap().to_path_buf();
        assert_ne!(root, std::env::temp_dir());
        assert_eq!(
            initial_files.path().as_path().parent(),
            Some(root.as_path())
        );
        drop(provisioning);
        drop(initial_files);
        assert!(
            scratch.path().as_path().is_dir(),
            "a host directory must keep the temporary root after the provisioning is dropped"
        );
        drop(scratch);
        assert!(
            !root.exists(),
            "the temporary root must go away with its last host directory"
        );
    }

    #[test]
    async fn create_in_refuses_an_existing_name() {
        let root = tempfile::tempdir().unwrap();
        let parent = scratch_at(root.path()).await;
        std::fs::write(parent.path().as_path().join("file"), b"").unwrap();

        let child = HostDirectory::create_in(parent.path(), OsStr::new("child"))
            .await
            .unwrap();
        let existing_directory = HostDirectory::create_in(parent.path(), OsStr::new("child"))
            .await
            .unwrap_err();
        let existing_file = HostDirectory::create_in(parent.path(), OsStr::new("file"))
            .await
            .unwrap_err();
        let invalid = HostDirectory::create_in(parent.path(), OsStr::new("a/b"))
            .await
            .unwrap_err();

        assert_eq!(child.path().as_path(), root.path().join(".scratch/child"));
        assert!(child.path().as_path().is_dir());
        assert_eq!(existing_directory.io_kind(), Some(ErrorKind::AlreadyExists));
        assert_eq!(existing_file.io_kind(), Some(ErrorKind::AlreadyExists));
        assert_eq!(invalid.io_kind(), Some(ErrorKind::InvalidInput));
    }

    #[test]
    async fn child_refuses_a_name_that_is_not_one_normal_component() {
        let root = tempfile::tempdir().unwrap();
        let directory = scratch_at(root.path()).await;

        ["", ".", "..", "a/b", "/a", "a/"]
            .into_iter()
            .for_each(|name| {
                assert_eq!(
                    directory
                        .path()
                        .child(OsStr::new(name))
                        .unwrap_err()
                        .io_kind(),
                    Some(ErrorKind::InvalidInput),
                    "{name:?} must be refused"
                );
            });
        assert_eq!(
            directory.path().child(OsStr::new("a")).unwrap().as_path(),
            root.path().join(".scratch/a")
        );
        assert_eq!(
            directory.path().child(OsStr::new(".a")).unwrap().as_path(),
            root.path().join(".scratch/.a")
        );
    }

    #[test]
    async fn discard_and_drop_remove_the_directory_with_its_contents() {
        let root = tempfile::tempdir().unwrap();
        let (_provisioning, directories) = provision(Some(root.path())).await;
        let HostDirectories {
            scratch: discarded,
            initial_files: dropped,
        } = directories;
        std::fs::create_dir(discarded.path().as_path().join("nested")).unwrap();
        std::fs::write(discarded.path().as_path().join("nested/file"), b"data").unwrap();
        std::fs::create_dir(dropped.path().as_path().join("nested")).unwrap();
        std::fs::write(dropped.path().as_path().join("nested/file"), b"data").unwrap();

        discarded.discard().await.unwrap();
        drop(dropped);

        assert!(is_empty_directory(root.path()));
    }

    #[test]
    async fn a_child_removed_with_its_parent_discards_without_error() {
        let root = tempfile::tempdir().unwrap();
        let (_provisioning, directories) = provision(Some(root.path())).await;
        let child = HostDirectory::create_in(directories.scratch.path(), OsStr::new("child"))
            .await
            .unwrap();
        std::fs::write(child.path().as_path().join("file"), b"data").unwrap();

        drop(directories);

        child.discard().await.unwrap();
        assert!(is_empty_directory(root.path()));
    }

    #[cfg(unix)]
    #[test]
    async fn discard_reports_a_removal_failure() {
        if running_as_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let directory = scratch_at(root.path()).await;
        let locked = directory.path().as_path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("file"), b"contents").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error = directory.discard().await.unwrap_err();

        assert!(error.cleanup_failed(), "{error}");
        assert!(
            error
                .to_string()
                .starts_with("failed to discard host directory"),
            "{error}"
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(locked.join("file").is_file());
    }

    /// The native setup of the host directories runs on the one blocking thread of a runtime,
    /// behind a gate, so a test can drop its caller while the work waits or runs.
    #[cfg(target_os = "linux")]
    mod cancelled_setup {
        use super::*;
        use futures::FutureExt as _;
        use rustix::fs::{FlockOperation, flock};
        use std::os::fd::{AsRawFd as _, RawFd};

        /// A provisioning with the shape of XFS storage over an ordinary directory: the volume
        /// owns the descriptor of `root` with its exclusive lock, and the root of the agent
        /// directories is the path of that descriptor. Gives the number of the descriptor.
        pub(super) fn descriptor_rooted(root: &Path) -> (SandboxFilesystemProvisioning, RawFd) {
            let descriptor = File::open(root).unwrap();
            flock(&descriptor, FlockOperation::NonBlockingLockExclusive).unwrap();
            let number = descriptor.as_raw_fd();
            let provisioning = SandboxFilesystemProvisioning {
                volume: FilesystemVolume::copy_on_write(
                    Arc::new(descriptor),
                    FilesystemIdentity { device: 0 },
                ),
                mode: SandboxFilesystemProvisioningMode::Directories(
                    directories::DirectoryProvisioning::new(
                        Some(Arc::from(Path::new(&format!("/proc/self/fd/{number}")))),
                        RetryConfig::default(),
                        NativeNameModeSource::NativeDetection,
                    ),
                ),
                accounting: AgentAccounting::Unaccounted,
                host_directory_check: HostDirectoryCheck::None,
            };
            (provisioning, number)
        }

        /// A current-thread runtime with one blocking thread.
        pub(super) fn one_blocking_thread() -> tokio::runtime::Runtime {
            tokio::runtime::Builder::new_current_thread()
                .max_blocking_threads(1)
                .enable_all()
                .build()
                .unwrap()
        }

        /// Holds the one blocking thread of `runtime` until the sender gets a message or drops.
        pub(super) fn hold_the_blocking_thread(
            runtime: &tokio::runtime::Runtime,
        ) -> std::sync::mpsc::Sender<()> {
            let (release, released) = std::sync::mpsc::channel::<()>();
            drop(runtime.spawn_blocking(move || {
                let _ = released.recv();
            }));
            release
        }

        /// Waits until the blocking work queued before this call has ended.
        pub(super) fn wait_for_the_blocking_thread(runtime: &tokio::runtime::Runtime) {
            runtime.block_on(runtime.spawn_blocking(|| ())).unwrap();
        }

        /// Whether the descriptor `number` is open and names the directory at `path`. Another
        /// test of the process can take a free number, so the check compares the directory too.
        pub(super) fn names(number: RawFd, path: &Path) -> bool {
            use std::os::unix::fs::MetadataExt as _;
            let identity = |metadata: std::fs::Metadata| (metadata.dev(), metadata.ino());
            std::fs::metadata(format!("/proc/self/fd/{number}"))
                .is_ok_and(|found| identity(found) == identity(std::fs::metadata(path).unwrap()))
        }

        /// Whether a lock of `root` is held through another open of it.
        pub(super) fn lock_is_held(root: &Path) -> bool {
            flock(
                File::open(root).unwrap(),
                FlockOperation::NonBlockingLockExclusive,
            )
            .is_err()
        }

        /// Makes a new descriptor of `directory` at the lowest free number from `number` on: the
        /// number itself when it is free.
        pub(super) fn reuse(directory: &File, number: RawFd) -> RawFd {
            // SAFETY: `F_DUPFD_CLOEXEC` never replaces an open descriptor.
            let reused =
                unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, number) };
            assert!(reused >= 0, "{}", std::io::Error::last_os_error());
            reused
        }

        pub(super) fn close(descriptor: RawFd) {
            // SAFETY: the test owns the descriptor that `reuse` made.
            unsafe { libc::close(descriptor) };
        }

        /// A directory `unrelated` whose `.scratch` and `.initial-files` hold a file each.
        pub(super) fn unrelated_with_host_directory_names(parent: &Path) -> PathBuf {
            let unrelated = parent.join("unrelated");
            [SCRATCH, INITIAL_FILES].into_iter().for_each(|name| {
                std::fs::create_dir_all(unrelated.join(name)).unwrap();
                std::fs::write(unrelated.join(name).join("data"), b"unrelated").unwrap();
            });
            unrelated
        }

        pub(super) fn holds_its_data(unrelated: &Path) -> [bool; 2] {
            [SCRATCH, INITIAL_FILES].map(|name| unrelated.join(name).join("data").is_file())
        }

        pub(super) fn has_host_directories(root: &Path) -> [bool; 2] {
            [SCRATCH, INITIAL_FILES].map(|name| root.join(name).exists())
        }

        /// What a setup whose caller drops it while its native work waits in the queue does: whether
        /// the number of the root descriptor stays taken and the root stays locked until the work
        /// ends, whether the unrelated data survives, which host directories stay in the original
        /// root, and whether the descriptor and the lock are free after the work.
        pub(super) fn cancel_while_queued(
            original: &Path,
            unrelated: &Path,
        ) -> (bool, bool, [bool; 2], [bool; 2], bool, bool) {
            let runtime = one_blocking_thread();
            let unrelated_descriptor = File::open(unrelated).unwrap();
            let (provisioning, number) = descriptor_rooted(original);
            let release = hold_the_blocking_thread(&runtime);
            runtime.block_on(async {
                let queued = make_host_directories(&provisioning).now_or_never();
                assert!(
                    queued.is_none(),
                    "the setup must wait for the blocking thread"
                );
            });
            drop(provisioning);

            let reused = reuse(&unrelated_descriptor, number);
            let kept = reused != number && names(number, original);
            let locked = lock_is_held(original);
            release.send(()).unwrap();
            wait_for_the_blocking_thread(&runtime);
            close(reused);

            (
                kept,
                locked,
                holds_its_data(unrelated),
                has_host_directories(original),
                !names(number, original),
                !lock_is_held(original),
            )
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_setup_dropped_while_queued_keeps_its_root_until_its_native_work_ends_and_leaves_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let unrelated = cancelled_setup::unrelated_with_host_directory_names(parent.path());

        let outcome = cancelled_setup::cancel_while_queued(&original, &unrelated);

        assert_eq!(
            outcome,
            (true, true, [true, true], [false, false], true, true)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_setup_dropped_while_queued_removes_its_scratch_and_keeps_unrelated_data() {
        if running_as_root() {
            return;
        }
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        let locked = original.join(".initial-files/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("file"), b"stale").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let unrelated = cancelled_setup::unrelated_with_host_directory_names(parent.path());

        let outcome = cancelled_setup::cancel_while_queued(&original, &unrelated);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            outcome,
            (true, true, [true, true], [false, true], true, true)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_setup_dropped_while_its_native_work_runs_keeps_its_root_until_the_work_ends() {
        use futures::FutureExt as _;
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let unrelated = cancelled_setup::unrelated_with_host_directory_names(parent.path());
        let unrelated_descriptor = File::open(&unrelated).unwrap();
        let runtime = cancelled_setup::one_blocking_thread();
        let (provisioning, number) = cancelled_setup::descriptor_rooted(&original);
        let (started, starts) = std::sync::mpsc::channel::<()>();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let task = host_directories_task(&provisioning);
        runtime.block_on(async {
            let running = execute_native(
                NativeStorageProfile::Unknown,
                NativeOperation::RecursiveCleanup,
                move || {
                    started.send(()).unwrap();
                    let _ = released.recv();
                    task()
                },
            )
            .now_or_never();
            assert!(running.is_none(), "the setup must wait at its gate");
        });
        starts.recv().unwrap();
        drop(provisioning);

        let reused = cancelled_setup::reuse(&unrelated_descriptor, number);
        let kept = reused != number && cancelled_setup::names(number, &original);
        let locked = cancelled_setup::lock_is_held(&original);
        release.send(()).unwrap();
        cancelled_setup::wait_for_the_blocking_thread(&runtime);
        cancelled_setup::close(reused);

        assert_eq!(
            (
                kept,
                locked,
                cancelled_setup::holds_its_data(&unrelated),
                cancelled_setup::has_host_directories(&original),
                !cancelled_setup::names(number, &original),
                !cancelled_setup::lock_is_held(&original),
            ),
            (true, true, [true, true], [false, false], true, true)
        );
    }

    /// A provision of XFS storage with reflink whose caller drops it while the setup of its host
    /// directories waits for the blocking thread: the root descriptor stays open and the root stays
    /// locked until the setup ends, the setup leaves no host directory, and then the root binds
    /// again.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires the unprivileged reflink XFS test runner"]
    fn reflink_xfs_a_provision_dropped_while_its_setup_is_queued_keeps_its_root_until_the_setup_ends()
     {
        use futures::FutureExt as _;
        let root = std::env::var_os("GOLEM_REFLINK_XFS_TEST_ROOT")
            .map(PathBuf::from)
            .expect(
                "GOLEM_REFLINK_XFS_TEST_ROOT must name the mounted XFS test root without quotas",
            );
        let storage = FilesystemStorageMode::ReflinkXfs {
            root: root.clone().into(),
        };
        let runtime = cancelled_setup::one_blocking_thread();
        let release = cancelled_setup::hold_the_blocking_thread(&runtime);
        let descriptors_before = std::fs::read_dir("/proc/self/fd").unwrap().count();
        runtime.block_on(async {
            let provisioned =
                SandboxFilesystemProvisioning::provision(&storage, RetryConfig::default())
                    .now_or_never();
            assert!(
                provisioned.is_none(),
                "the setup must wait for the blocking thread"
            );
        });

        let held = std::fs::read_dir("/proc/self/fd").unwrap().count() > descriptors_before
            && SandboxFilesystemProvisioning::new(&storage, RetryConfig::default()).is_err();
        release.send(()).unwrap();
        cancelled_setup::wait_for_the_blocking_thread(&runtime);
        let left = cancelled_setup::has_host_directories(&root);
        let bound_again =
            SandboxFilesystemProvisioning::new(&storage, RetryConfig::default()).is_ok();

        assert_eq!((held, left, bound_again), (true, [false, false], true));
    }

    /// The discards of both host directories, which a caller drops while they wait for the blocking
    /// thread, keep the root descriptor and its lock until they end, and remove only the host
    /// directories of their root.
    #[cfg(target_os = "linux")]
    #[test]
    fn discards_dropped_while_queued_keep_their_root_until_their_native_work_ends() {
        use futures::FutureExt as _;
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let unrelated = cancelled_setup::unrelated_with_host_directory_names(parent.path());
        let unrelated_descriptor = File::open(&unrelated).unwrap();
        let runtime = cancelled_setup::one_blocking_thread();
        let (provisioning, number) = cancelled_setup::descriptor_rooted(&original);
        let HostDirectories {
            scratch,
            initial_files,
        } = runtime
            .block_on(make_host_directories(&provisioning))
            .unwrap();
        let made = cancelled_setup::has_host_directories(&original);
        let release = cancelled_setup::hold_the_blocking_thread(&runtime);
        runtime.block_on(async {
            let queued = futures::future::join_all([scratch.discard(), initial_files.discard()])
                .now_or_never();
            assert!(
                queued.is_none(),
                "the discards must wait for the blocking thread"
            );
        });
        drop(provisioning);

        let reused = cancelled_setup::reuse(&unrelated_descriptor, number);
        let kept = reused != number && cancelled_setup::names(number, &original);
        let locked = cancelled_setup::lock_is_held(&original);
        release.send(()).unwrap();
        cancelled_setup::wait_for_the_blocking_thread(&runtime);
        cancelled_setup::close(reused);

        assert_eq!(
            (
                made,
                kept,
                locked,
                cancelled_setup::holds_its_data(&unrelated),
                cancelled_setup::has_host_directories(&original),
                !cancelled_setup::names(number, &original),
                !cancelled_setup::lock_is_held(&original),
            ),
            (
                [true, true],
                true,
                true,
                [true, true],
                [false, false],
                true,
                true
            )
        );
    }
}
