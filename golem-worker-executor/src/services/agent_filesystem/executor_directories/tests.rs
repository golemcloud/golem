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
use crate::filesystem::sandbox::SandboxFilesystemName;
use std::ffi::OsStr;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path};
use test_r::test;
#[cfg(unix)]
fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}
fn is_one_normal_component(name: &OsStr) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(value)) if value == name)
        && components.next().is_none()
}
async fn provision(root: Option<&Path>) -> (SandboxFilesystemProvisioning, ExecutorDirectories) {
    provision_executor_directories(
        &root.map_or(FilesystemStorageMode::Temporary, |root| {
            FilesystemStorageMode::Directory { root: root.into() }
        }),
        RetryConfig::default(),
    )
    .await
    .unwrap()
}
fn is_empty_directory(path: &Path) -> bool {
    std::fs::read_dir(path).unwrap().next().is_none()
}
#[test]
fn a_failed_removal_is_the_error_after_a_failed_step() {
    let step = || {
        FilesystemStorageError::io(
            "make a host directory",
            Some(Path::new("/step")),
            std::io::Error::from(ErrorKind::PermissionDenied),
        )
    };
    let removal = FilesystemStorageError::cleanup_io(
        "remove a host directory",
        Some(Path::new("/removal")),
        std::io::Error::from(ErrorKind::PermissionDenied),
    );

    assert_eq!(
        error_after_removal(step(), Ok(())).to_string(),
        step().to_string()
    );
    let failed = error_after_removal(step(), Err(removal));
    assert!(failed.cleanup_failed(), "{failed}");
    assert!(
        failed.to_string().contains("make a host directory"),
        "{failed}"
    );
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
            SandboxFilesystemName::new(name.to_string(), "c".to_string(), "f".to_string()).is_err(),
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
async fn provision_removes_the_scratch_directory_when_the_initial_files_directory_cannot_be_made() {
    if running_as_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join(".initial-files/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("file"), b"stale").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

    let failure = crate::services::agent_filesystem::provision_executor_directories(
        &FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        RetryConfig::default(),
    )
    .await
    .err()
    .expect("a .initial-files that cannot be made must fail the provisioning");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(
        !failure.cleanup_failed(),
        "the initial removal failed, not the rollback: {failure}"
    );
    assert_eq!(failure.io_kind(), Some(ErrorKind::PermissionDenied));
    assert!(
        !root.path().join(".scratch").exists(),
        "a failed .initial-files must leave no .scratch"
    );
}

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

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
    pub(super) fn descriptor_rooted(root: &Path) -> (FilesystemVolume, RawFd) {
        let descriptor = File::open(root).unwrap();
        flock(&descriptor, FlockOperation::NonBlockingLockExclusive).unwrap();
        let number = descriptor.as_raw_fd();
        let provisioning = FilesystemVolume::descriptor_rooted_for_test(Arc::new(descriptor));
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
        let reused = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, number) };
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
            let queued = make_executor_directories(&provisioning).now_or_never();
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
    let task = executor_directories_task(&provisioning);
    runtime.block_on(async {
        let running = BlockingTask::Spawn
            .run(move || {
                started.send(()).unwrap();
                let _ = released.recv();
                task()
            })
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
fn reflink_xfs_a_provision_dropped_while_its_setup_is_queued_keeps_its_root_until_the_setup_ends() {
    use futures::FutureExt as _;
    let root = std::env::var_os("GOLEM_REFLINK_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_REFLINK_XFS_TEST_ROOT must name the mounted XFS test root without quotas");
    let storage = FilesystemStorageMode::ReflinkXfs {
        root: root.clone().into(),
    };
    let runtime = cancelled_setup::one_blocking_thread();
    let release = cancelled_setup::hold_the_blocking_thread(&runtime);
    let descriptors_before = std::fs::read_dir("/proc/self/fd").unwrap().count();
    runtime.block_on(async {
        let provisioned = crate::services::agent_filesystem::provision_executor_directories(
            &storage,
            RetryConfig::default(),
        )
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
    let bound_again = SandboxFilesystemProvisioning::new(&storage, RetryConfig::default()).is_ok();

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
    let ExecutorDirectories {
        scratch,
        initial_files,
    } = runtime
        .block_on(make_executor_directories(&provisioning))
        .unwrap();
    let made = cancelled_setup::has_host_directories(&original);
    let release = cancelled_setup::hold_the_blocking_thread(&runtime);
    runtime.block_on(async {
        let queued =
            futures::future::join_all([scratch.discard(), initial_files.discard()]).now_or_never();
        assert!(
            queued.is_none(),
            "the discards must wait for the blocking thread"
        );
    });
    drop((scratch, initial_files, provisioning));

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

#[cfg(target_os = "linux")]
mod xfs_directory_checks {
    use super::*;
    use crate::filesystem::volume::{get_fsxattr, set_fsxattr};
    use test_r::{test, timeout};

    fn reflink_test_root() -> PathBuf {
        std::env::var_os("GOLEM_REFLINK_XFS_TEST_ROOT")
            .map(PathBuf::from)
            .expect(
                "GOLEM_REFLINK_XFS_TEST_ROOT must name the mounted XFS test root without quotas",
            )
    }

    fn reflink_storage(root: PathBuf) -> FilesystemStorageMode {
        FilesystemStorageMode::ReflinkXfs { root: root.into() }
    }

    /// Restores the project attributes of a directory when it is dropped, also after a failed
    /// assertion.
    struct RestoredProjectAttributes {
        directory: File,
        attributes: linux_raw_sys::general::fsxattr,
    }

    /// A host directory that inherits a project identity from the root after the binding fails
    /// the setup of the host directories, because reflink XFS storage checks that each host
    /// directory has no project identity.
    #[test]
    #[ignore = "requires the unprivileged reflink XFS test runner"]
    #[timeout("60s")]
    async fn reflink_xfs_refuses_a_host_directory_with_a_project_identity() {
        let root = reflink_test_root();
        let provisioning =
            FilesystemVolume::open(&reflink_storage(root.clone()), RetryConfig::default()).unwrap();
        let directory = File::open(&root).unwrap();
        let original = get_fsxattr(&directory).unwrap();
        let _restored = RestoredProjectAttributes {
            directory: directory.try_clone().unwrap(),
            attributes: original,
        };
        let mut with_project = original;
        with_project.fsx_projid = 7;
        with_project.fsx_xflags |= linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
        set_fsxattr(&directory, with_project).unwrap();

        let error = match make_executor_directories(&provisioning).await {
            Ok(_) => panic!("a host directory with a project identity must be refused"),
            Err(error) => error.to_string(),
        };

        assert!(
            error.contains("verify XFS host directory has no project identity"),
            "{error}"
        );
    }

    impl Drop for RestoredProjectAttributes {
        fn drop(&mut self) {
            if let Err(error) = set_fsxattr(&self.directory, self.attributes) {
                eprintln!("failed to restore the project attributes of the test root: {error}");
            }
        }
    }
}
