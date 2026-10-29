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

use crate::blocking::BlockingTask;
use crate::path_removal;
pub(crate) use errors::HostDirectoryError;
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;

/// A temporary parent shared by executor directories, which can be kept after failed shutdown.
#[derive(Debug)]
pub(crate) struct TemporaryRoot {
    path: PathBuf,
    directory: std::sync::Mutex<Option<tempfile::TempDir>>,
}

impl TemporaryRoot {
    pub(crate) fn new(directory: tempfile::TempDir) -> Self {
        Self {
            path: directory.path().to_path_buf(),
            directory: std::sync::Mutex::new(Some(directory)),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn retain(&self) {
        if let Some(directory) = self
            .directory
            .lock()
            .expect("temporary root lock poisoned")
            .take()
        {
            let _ = directory.keep();
        }
    }
}

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
    fallback_cleanup: AtomicBool,
    removed: Arc<Mutex<bool>>,
    root: Option<Arc<File>>,
    temporary_root: Option<Arc<TemporaryRoot>>,
}

impl HostDirectory {
    #[cfg(test)]
    async fn create_fresh(
        path: PathBuf,
        root: Option<Arc<File>>,
        temporary_root: Option<Arc<TemporaryRoot>>,
    ) -> Result<Self, HostDirectoryError> {
        let error_path = path.clone();
        BlockingTask::Spawn
            .run(move || Self::create_fresh_blocking(path, root, temporary_root))
            .await
            .map_err(|error| {
                HostDirectoryError::task_failure("create host directory", &error_path, error)
            })?
    }

    /// Replaces a stale path with an empty directory, creating missing parents. The optional
    /// root owners keep descriptor-relative paths and temporary parents valid through creation
    /// and removal. The caller must run this on a blocking thread.
    pub(crate) fn create_fresh_blocking(
        path: PathBuf,
        root: Option<Arc<File>>,
        temporary_root: Option<Arc<TemporaryRoot>>,
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
                HostDirectoryError::task_failure(
                    "create host directory",
                    error_path.as_path(),
                    error,
                )
            })?
    }

    fn owned(
        path: PathBuf,
        root: Option<Arc<File>>,
        temporary_root: Option<Arc<TemporaryRoot>>,
    ) -> Self {
        Self {
            path: HostPath(Arc::from(path)),
            fallback_cleanup: AtomicBool::new(true),
            removed: Arc::new(Mutex::new(false)),
            root,
            temporary_root,
        }
    }

    pub(crate) fn path(&self) -> &HostPath {
        &self.path
    }

    /// Keeps this directory on disk instead of running fallback cleanup when its owner drops.
    pub(crate) fn retain(&self) {
        self.fallback_cleanup.store(false, Ordering::Release);
        if let Some(root) = &self.temporary_root {
            root.retain();
        }
    }

    /// Removes the directory after its owner has stopped and drained its users. Concurrent
    /// removals join the same serialized work, including when an earlier caller stopped waiting.
    pub(crate) async fn discard(&self) -> Result<(), HostDirectoryError> {
        BlockingTask::Spawn
            .run(self.discard_task())
            .await
            .map_err(|error| {
                HostDirectoryError::task_failure(
                    "discard host directory",
                    self.path.as_path(),
                    error,
                )
            })?
    }

    /// Explicit removal for an owner already running on a blocking thread.
    pub(crate) fn discard_blocking(&self) -> Result<(), HostDirectoryError> {
        self.discard_task()()
    }

    fn discard_task(&self) -> impl FnOnce() -> Result<(), HostDirectoryError> + Send + 'static {
        self.fallback_cleanup.store(false, Ordering::Release);
        let path = self.path.clone();
        let removed = Arc::clone(&self.removed);
        let owners = (self.root.clone(), self.temporary_root.clone());
        move || {
            let _owners = owners;
            let mut removed = removed.blocking_lock();
            if !*removed {
                remove_and_verify(path.as_path(), "discard host directory")?;
                *removed = true;
            }
            Ok(())
        }
    }
}

impl Drop for HostDirectory {
    fn drop(&mut self) {
        if !self.fallback_cleanup.load(Ordering::Acquire) {
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

fn remove_and_verify(path: &Path, operation: &'static str) -> Result<(), HostDirectoryError> {
    path_removal::remove_and_verify(path)
        .map_err(|failure| HostDirectoryError::new(operation, path, failure.into()))
}

#[cfg(test)]
mod tests;
