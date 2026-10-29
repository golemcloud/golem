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

mod errors;
pub(crate) use errors::{HostDirectoryError, HostDirectoryFailure};

use crate::blocking::BlockingTask;
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;

/// A path below an owned host directory. It does not keep that directory alive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostPath(Arc<Path>);

impl HostPath {
    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    /// Adds exactly one normal component, never an absolute path or parent traversal.
    pub(crate) fn child(&self, name: &OsStr) -> Result<Self, HostDirectoryError> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(component)) if component == name)
            || components.next().is_some()
        {
            return Err(HostDirectoryError::io(
                "validate host path name",
                self.as_path(),
                std::io::ErrorKind::InvalidInput.into(),
            ));
        }
        Ok(Self(Arc::from(self.as_path().join(name))))
    }
}

/// Owns a host directory. Explicit removal verifies absence; Drop is a best-effort safety net.
#[derive(Debug)]
pub(crate) struct HostDirectory {
    path: HostPath,
    explicit_cleanup: AtomicBool,
    removed: Arc<Mutex<bool>>,
    root: Option<Arc<File>>,
    temporary_root: Option<Arc<tempfile::TempDir>>,
}

impl HostDirectory {
    #[cfg(test)]
    async fn create_fresh(
        path: PathBuf,
        root: Option<Arc<File>>,
        temporary_root: Option<Arc<tempfile::TempDir>>,
    ) -> Result<Self, HostDirectoryError> {
        let error_path = path.clone();
        BlockingTask::Spawn
            .run(move || Self::create_fresh_blocking(path, root, temporary_root))
            .await
            .map_err(|error| {
                HostDirectoryError::task("create host directory", &error_path, error)
            })?
    }

    /// Replaces a stale path with an empty directory, creating missing parents. The optional
    /// root owners keep descriptor-relative paths and temporary parents valid through creation
    /// and removal. The caller must run this on a blocking thread.
    pub(crate) fn create_fresh_blocking(
        path: PathBuf,
        root: Option<Arc<File>>,
        temporary_root: Option<Arc<tempfile::TempDir>>,
    ) -> Result<Self, HostDirectoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                HostDirectoryError::io("create host directory parent", parent, error)
            })?;
        }
        remove_and_verify(&path, "remove stale host directory")?;
        std::fs::create_dir(&path)
            .map_err(|error| HostDirectoryError::io("create host directory", &path, error))?;
        Ok(Self::owned(path, root, temporary_root))
    }

    /// Creates one new child. An existing name is an error.
    pub(crate) async fn create_in(
        parent: &HostPath,
        name: &OsStr,
    ) -> Result<Self, HostDirectoryError> {
        let path = parent.child(name)?;
        let error_path = path.clone();
        BlockingTask::Spawn
            .run(move || {
                std::fs::create_dir(path.as_path()).map_err(|error| {
                    HostDirectoryError::io("create host directory", path.as_path(), error)
                })?;
                Ok(Self::owned(path.as_path().into(), None, None))
            })
            .await
            .map_err(|error| {
                HostDirectoryError::task("create host directory", error_path.as_path(), error)
            })?
    }

    fn owned(
        path: PathBuf,
        root: Option<Arc<File>>,
        temporary_root: Option<Arc<tempfile::TempDir>>,
    ) -> Self {
        Self {
            path: HostPath(Arc::from(path)),
            explicit_cleanup: AtomicBool::new(false),
            removed: Arc::new(Mutex::new(false)),
            root,
            temporary_root,
        }
    }

    pub(crate) fn path(&self) -> &HostPath {
        &self.path
    }

    /// Removes the directory after its owner has stopped and drained its users. Concurrent
    /// removals join the same serialized work, including when an earlier caller stopped waiting.
    pub(crate) async fn remove(&self) -> Result<(), HostDirectoryError> {
        let mut removed = Arc::clone(&self.removed).lock_owned().await;
        if *removed {
            return Ok(());
        }
        self.explicit_cleanup.store(true, Ordering::Release);
        let path = self.path.clone();
        let owners = (self.root.clone(), self.temporary_root.clone());
        BlockingTask::Spawn
            .run(move || {
                let _owners = owners;
                remove_and_verify(path.as_path(), "discard host directory")?;
                *removed = true;
                Ok(())
            })
            .await
            .map_err(|error| {
                HostDirectoryError::task("discard host directory", self.path.as_path(), error)
                    .during_cleanup()
            })?
    }

    pub(crate) async fn discard(self) -> Result<(), HostDirectoryError> {
        self.remove().await
    }

    /// Explicit removal for an owner already running on a blocking thread.
    pub(crate) fn discard_blocking(self) -> Result<(), HostDirectoryError> {
        let mut removed = self.removed.blocking_lock();
        if *removed {
            return Ok(());
        }
        self.explicit_cleanup.store(true, Ordering::Release);
        remove_and_verify(self.path.as_path(), "discard host directory")?;
        *removed = true;
        Ok(())
    }
}

impl Drop for HostDirectory {
    fn drop(&mut self) {
        if self.explicit_cleanup.load(Ordering::Acquire) {
            return;
        }
        match std::fs::remove_dir_all(self.path.as_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::error!(error = %error, path = %self.path.as_path().display(), "Failed to remove a dropped host directory")
            }
        }
    }
}

pub(crate) fn remove_and_verify(
    path: &Path,
    operation: &'static str,
) -> Result<(), HostDirectoryError> {
    let result = (|| {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {
                std::fs::remove_dir_all(path)
            }
            Ok(_) => std::fs::remove_file(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(HostDirectoryError::io(operation, path, error)),
        }
        .map_err(|error| HostDirectoryError::io(operation, path, error))?;
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(HostDirectoryError::io(operation, path, error)),
            Ok(_) => Err(HostDirectoryError::verification(operation, path)),
        }
    })();
    result.map_err(HostDirectoryError::during_cleanup)
}

#[cfg(test)]
mod tests;
