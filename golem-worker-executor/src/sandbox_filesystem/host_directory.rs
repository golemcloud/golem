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

/// A directory on the host that this value owns. Discard or drop removes the directory with all
/// that is in it.
///
/// A host directory keeps the volume root that it is in usable while it lives, also after its
/// provisioning is dropped.
#[derive(Debug)]
pub(crate) struct HostDirectory {
    path: HostPath,
    removed: bool,
    _root: Option<Arc<File>>,
    _temporary_root: Option<Arc<tempfile::TempDir>>,
}

impl HostDirectory {
    /// Makes an empty directory with this name directly under the volume root. Removes what an
    /// earlier process left under the name first. Fails if this process already made the name, or
    /// if the name is not one normal component that starts with a dot.
    pub(crate) async fn create_at_root(
        provisioning: &SandboxFilesystemProvisioning,
        name: &OsStr,
    ) -> Result<HostDirectory, FilesystemStorageError> {
        let root = provisioning.host_root();
        if !is_one_normal_component(name) || !name.as_encoded_bytes().starts_with(b".") {
            return Err(FilesystemStorageError::io(
                "validate host directory name",
                root.path,
                std::io::Error::from(std::io::ErrorKind::InvalidInput),
            ));
        }
        let path = HostPath(Arc::from(tree_copy::child_path(root.path, name)));
        if !root
            .names
            .lock()
            .expect("host directory name registry poisoned")
            .insert(Box::from(name))
        {
            return Err(FilesystemStorageError::io(
                "create a host directory that this provisioning already made",
                path.as_path(),
                std::io::Error::from(std::io::ErrorKind::AlreadyExists),
            ));
        }
        let root_path: Box<Path> = root.path.into();
        let verify_no_project = root.verify_no_project;
        let outcome = execute_native(
            NativeStorageProfile::Unknown,
            NativeOperation::RecursiveCleanup,
            move || {
                let result =
                    make_empty_directory_at_root(&root_path, path.as_path(), verify_no_project);
                (path, result)
            },
        )
        .await;
        match outcome {
            Ok((path, Ok(()))) => Ok(HostDirectory {
                path,
                removed: false,
                _root: root.anchor,
                _temporary_root: root.temporary_root,
            }),
            Ok((_, Err(error))) => {
                forget_name(root.names, name);
                Err(error)
            }
            Err(error) => {
                forget_name(root.names, name);
                Err(FilesystemStorageError::task_failure(
                    "create host directory",
                    root.path,
                    error,
                ))
            }
        }
    }

    /// Makes an empty directory with this name in `parent`. Fails if the name exists, or if the
    /// name is not one normal component.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
            failure.cleanup_failed = true;
            failure
        })?
    }
}

impl Drop for HostDirectory {
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        if let Err(error) =
            remove_and_verify_blocking(self.path.as_path(), "discard host directory")
        {
            tracing::error!(error = %error, "Failed to remove a dropped host directory");
        }
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

fn make_empty_directory_at_root(
    root: &Path,
    path: &Path,
    verify_no_project: bool,
) -> Result<(), FilesystemStorageError> {
    std::fs::create_dir_all(root)
        .map_err(|error| FilesystemStorageError::io("create host directory root", root, error))?;
    remove_and_verify_blocking(
        path,
        "remove what an earlier process left under a host directory name",
    )?;
    std::fs::create_dir(path)
        .map_err(|error| FilesystemStorageError::io("create host directory", path, error))?;
    #[cfg(target_os = "linux")]
    if verify_no_project && let Err(error) = xfs::verify_host_directory_has_no_project(path) {
        return Err(
            match remove_and_verify_blocking(
                path,
                "remove a host directory that has a project identity",
            ) {
                Ok(()) => error,
                Err(cleanup_error) => cleanup_error,
            },
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = verify_no_project;
    Ok(())
}

fn forget_name(names: &Mutex<HashSet<Box<OsStr>>>, name: &OsStr) {
    names
        .lock()
        .expect("host directory name registry poisoned")
        .remove(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt as _;
    use test_r::test;

    fn provisioning(root: &Path) -> SandboxFilesystemProvisioning {
        SandboxFilesystemProvisioning::new(Some(root.to_path_buf()), None, RetryConfig::default())
            .unwrap()
    }

    #[test]
    async fn create_at_root_removes_what_an_earlier_process_left() {
        let root = tempfile::tempdir().unwrap();
        let leftover = root.path().join(".downloads");
        std::fs::create_dir_all(leftover.join("nested")).unwrap();
        std::fs::write(leftover.join("nested/garbage"), b"stale").unwrap();

        let directory =
            HostDirectory::create_at_root(&provisioning(root.path()), OsStr::new(".downloads"))
                .await
                .unwrap();

        assert_eq!(directory.path().as_path(), leftover);
        assert!(std::fs::read_dir(&leftover).unwrap().next().is_none());
    }

    #[test]
    async fn create_at_root_refuses_a_name_that_this_provisioning_already_made() {
        let root = tempfile::tempdir().unwrap();
        let provisioning = provisioning(root.path());
        let first = HostDirectory::create_at_root(&provisioning, OsStr::new(".downloads"))
            .await
            .unwrap();
        std::fs::write(first.path().as_path().join("kept"), b"kept").unwrap();

        let error = HostDirectory::create_at_root(&provisioning.clone(), OsStr::new(".downloads"))
            .await
            .unwrap_err();

        assert_eq!(error.io_kind(), Some(ErrorKind::AlreadyExists));
        assert!(first.path().as_path().join("kept").is_file());
        HostDirectory::create_at_root(&provisioning, OsStr::new(".other"))
            .await
            .unwrap();
    }

    #[test]
    async fn create_at_root_refuses_a_name_that_is_not_one_component_with_a_leading_dot() {
        let root = tempfile::tempdir().unwrap();
        let provisioning = provisioning(root.path());
        let names = ["downloads", ".", "..", "", ".a/b", "/.a", ".a/"];

        let results = futures::future::join_all(
            names
                .iter()
                .map(|name| HostDirectory::create_at_root(&provisioning, OsStr::new(name))),
        )
        .await;

        names.iter().zip(results).for_each(|(name, result)| {
            assert_eq!(
                result.unwrap_err().io_kind(),
                Some(ErrorKind::InvalidInput),
                "{name:?} must be refused"
            );
        });
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    async fn create_at_root_accepts_the_name_again_after_a_failed_creation() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let provisioning = provisioning(root.path());
        let locked = root.path().join(".downloads/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("file"), b"stale").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

        let failure = HostDirectory::create_at_root(&provisioning, OsStr::new(".downloads"))
            .await
            .unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = HostDirectory::create_at_root(&provisioning, OsStr::new(".downloads"))
            .await
            .unwrap();

        assert!(failure.cleanup_failed(), "{failure}");
        assert!(
            std::fs::read_dir(directory.path().as_path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    async fn a_temporary_root_lives_while_its_provisioning_or_a_host_directory_lives() {
        let provisioning =
            SandboxFilesystemProvisioning::new(None, None, RetryConfig::default()).unwrap();

        let directory = HostDirectory::create_at_root(&provisioning, OsStr::new(".downloads"))
            .await
            .unwrap();

        let root = directory.path().as_path().parent().unwrap().to_path_buf();
        assert_ne!(root, std::env::temp_dir());
        drop(provisioning);
        assert!(
            directory.path().as_path().is_dir(),
            "the host directory must keep the temporary root after the provisioning is dropped"
        );
        drop(directory);
        assert!(
            !root.exists(),
            "the temporary root must go away with its last user"
        );
    }

    #[test]
    async fn create_in_refuses_an_existing_name() {
        let root = tempfile::tempdir().unwrap();
        let parent = HostDirectory::create_at_root(&provisioning(root.path()), OsStr::new(".work"))
            .await
            .unwrap();
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

        assert_eq!(child.path().as_path(), root.path().join(".work/child"));
        assert!(child.path().as_path().is_dir());
        assert_eq!(existing_directory.io_kind(), Some(ErrorKind::AlreadyExists));
        assert_eq!(existing_file.io_kind(), Some(ErrorKind::AlreadyExists));
        assert_eq!(invalid.io_kind(), Some(ErrorKind::InvalidInput));
    }

    #[test]
    async fn child_refuses_a_name_that_is_not_one_normal_component() {
        let root = tempfile::tempdir().unwrap();
        let directory =
            HostDirectory::create_at_root(&provisioning(root.path()), OsStr::new(".work"))
                .await
                .unwrap();

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
            root.path().join(".work/a")
        );
        assert_eq!(
            directory.path().child(OsStr::new(".a")).unwrap().as_path(),
            root.path().join(".work/.a")
        );
    }

    #[test]
    async fn discard_and_drop_remove_the_directory_with_its_contents() {
        let root = tempfile::tempdir().unwrap();
        let provisioning = provisioning(root.path());
        let discarded = HostDirectory::create_at_root(&provisioning, OsStr::new(".discarded"))
            .await
            .unwrap();
        let dropped = HostDirectory::create_at_root(&provisioning, OsStr::new(".dropped"))
            .await
            .unwrap();
        std::fs::create_dir(discarded.path().as_path().join("nested")).unwrap();
        std::fs::write(discarded.path().as_path().join("nested/file"), b"data").unwrap();
        std::fs::write(dropped.path().as_path().join("file"), b"data").unwrap();

        discarded.discard().await.unwrap();
        drop(dropped);

        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    async fn a_child_removed_with_its_parent_discards_without_error() {
        let root = tempfile::tempdir().unwrap();
        let parent = HostDirectory::create_at_root(&provisioning(root.path()), OsStr::new(".work"))
            .await
            .unwrap();
        let child = HostDirectory::create_in(parent.path(), OsStr::new("child"))
            .await
            .unwrap();
        std::fs::write(child.path().as_path().join("file"), b"data").unwrap();

        drop(parent);

        child.discard().await.unwrap();
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    async fn discard_reports_a_removal_failure() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let directory =
            HostDirectory::create_at_root(&provisioning(root.path()), OsStr::new(".work"))
                .await
                .unwrap();
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
