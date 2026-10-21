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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let anchor = provisioning.volume.copy_on_write_root().cloned();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
mod tests;
