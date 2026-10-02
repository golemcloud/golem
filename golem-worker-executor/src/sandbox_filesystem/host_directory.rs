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
/// gone. An owner that knows the end of the directory calls it. A drop without a discard removes
/// the directory as a best effort, does not check, and only logs a failure.
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
    pub(crate) async fn discard(mut self) -> Result<(), FilesystemStorageError> {
        self.removed = true;
        let path = self.path.clone();
        execute_native(
            NativeStorageProfile::Unknown,
            NativeOperation::RecursiveCleanup,
            move || remove_and_verify_blocking(path.as_path(), "discard host directory"),
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
/// Without a configured root on unmanaged storage, the root is a new temporary directory that the
/// two host directories keep while they live. Removes what an earlier process left under each
/// name first. On managed storage, a host directory that has a project identity gives an error.
/// When `.initial-files` cannot be made, the function removes the `.scratch` that it made, and a
/// failure of that removal is the error. A leftover that the function could not remove stays.
pub(super) async fn make_host_directories(
    provisioning: &SandboxFilesystemProvisioning,
) -> Result<HostDirectories, FilesystemStorageError> {
    let (root, anchor, verify_no_project) = match &provisioning.mode {
        SandboxFilesystemProvisioningMode::Unmanaged(unmanaged) => {
            (unmanaged.deterministic_root().map(Arc::from), None, false)
        }
        #[cfg(target_os = "linux")]
        SandboxFilesystemProvisioningMode::Managed(managed) => (
            Some(Arc::from(managed.root())),
            provisioning.volume.managed_root().cloned(),
            true,
        ),
    };
    let error_root: Option<Arc<Path>> = root.clone();
    let (scratch, initial_files, temporary_root) = execute_native(
        NativeStorageProfile::Unknown,
        NativeOperation::RecursiveCleanup,
        move || make_host_directories_blocking(root, verify_no_project),
    )
    .await
    .map_err(|error| {
        FilesystemStorageError::task_failure(
            "create host directory",
            error_root.as_deref().unwrap_or(Path::new("<temp>")),
            error,
        )
    })??;
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

/// The paths of `.scratch` and `.initial-files`, with the temporary root that holds them when no
/// root was given.
type MadeHostDirectories = (HostPath, HostPath, Option<Arc<tempfile::TempDir>>);

/// Makes the root, or a temporary root when `root` is `None`, and then `.scratch` and
/// `.initial-files` in it, as [`make_host_directories`] says.
fn make_host_directories_blocking(
    root: Option<Arc<Path>>,
    verify_no_project: bool,
) -> Result<MadeHostDirectories, FilesystemStorageError> {
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
    Ok((scratch, initial_files, temporary_root))
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
            root.map(Path::to_path_buf),
            None,
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
            Some(root.path().to_path_buf()),
            None,
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
}
