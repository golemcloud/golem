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

use super::SandboxFilesystem;
#[cfg(target_os = "linux")]
use super::sandbox_provisioning::managed;
use crate::operation_failure::OperationContext;
use crate::sandbox_filesystem::FilesystemStorageError;
use golem_common::model::RetryConfig;
use golem_common::retries::RetryState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub(in crate::sandbox_filesystem) static FILESYSTEM_LEASES: OnceLock<
    std::sync::Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>,
> = OnceLock::new();

pub(in crate::sandbox_filesystem) struct ExclusiveFilesystemLease {
    pub(in crate::sandbox_filesystem) state: Mutex<Option<LeaseState>>,
}

pub(in crate::sandbox_filesystem) struct LeaseState {
    pub(in crate::sandbox_filesystem) lifecycle: OwnedMutexGuard<()>,
    pub(in crate::sandbox_filesystem) cleanup: NativeCleanup,
}

pub(in crate::sandbox_filesystem) struct RestoringLeaseState<'a> {
    pub(in crate::sandbox_filesystem) slot: &'a Mutex<Option<LeaseState>>,
    pub(in crate::sandbox_filesystem) state: Option<LeaseState>,
}

impl<'a> RestoringLeaseState<'a> {
    pub(super) fn take(slot: &'a Mutex<Option<LeaseState>>) -> Option<Self> {
        let state = slot
            .lock()
            .expect("sandbox filesystem lease lock poisoned")
            .take()?;
        Some(Self {
            slot,
            state: Some(state),
        })
    }

    pub(super) fn cleanup(&mut self) -> &mut NativeCleanup {
        &mut self
            .state
            .as_mut()
            .expect("armed lease state must be present")
            .cleanup
    }

    pub(super) fn disarm(mut self) {
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

pub(in crate::sandbox_filesystem) enum NativeCleanup {
    Directory {
        path: PathBuf,
        cleanup_retry: RetryConfig,
    },
    #[cfg(target_os = "linux")]
    Managed(Box<managed::ManagedProjectCleanup>),
}

impl NativeCleanup {
    pub(super) async fn delete(&mut self) -> Result<(), FilesystemStorageError> {
        match self {
            Self::Directory {
                path,
                cleanup_retry,
            } => remove_and_verify(path, "delete runtime directory", cleanup_retry).await,
            #[cfg(target_os = "linux")]
            Self::Managed(cleanup) => cleanup.delete().await,
        }
    }

    pub(super) fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError> {
        match self {
            Self::Directory { path, .. } => {
                remove_and_verify_blocking(path, "delete runtime directory")
            }
            #[cfg(target_os = "linux")]
            Self::Managed(cleanup) => cleanup.delete_blocking(),
        }
    }
}

pub(in crate::sandbox_filesystem) async fn acquire_filesystem_lease(
    path: &Path,
) -> OwnedMutexGuard<()> {
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
pub(in crate::sandbox_filesystem) struct FilesystemLeaseProbeState {
    pub(in crate::sandbox_filesystem) attempted: tokio::sync::Semaphore,
    pub(in crate::sandbox_filesystem) acquired: tokio::sync::Semaphore,
}

#[cfg(test)]
pub(in crate::sandbox_filesystem) struct FilesystemLeaseProbe {
    pub(in crate::sandbox_filesystem) state: Arc<FilesystemLeaseProbeState>,
}

#[cfg(test)]
impl FilesystemLeaseProbe {
    pub(in crate::sandbox_filesystem) fn install(path: &Path) -> Self {
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

    pub(in crate::sandbox_filesystem) async fn wait_attempted(&self) {
        self.state
            .attempted
            .acquire()
            .await
            .expect("sandbox filesystem lease attempt probe closed")
            .forget();
    }

    pub(in crate::sandbox_filesystem) fn acquisition_is_pending(&self) -> bool {
        self.state.acquired.available_permits() == 0
    }

    pub(in crate::sandbox_filesystem) async fn wait_acquired(&self) {
        self.state
            .acquired
            .acquire()
            .await
            .expect("sandbox filesystem lease acquisition probe closed")
            .forget();
    }
}

#[cfg(test)]
pub(in crate::sandbox_filesystem) fn filesystem_lease_probes()
-> &'static Mutex<HashMap<PathBuf, Weak<FilesystemLeaseProbeState>>> {
    static PROBES: OnceLock<Mutex<HashMap<PathBuf, Weak<FilesystemLeaseProbeState>>>> =
        OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
pub(in crate::sandbox_filesystem) fn filesystem_lease_probe(
    path: &Path,
) -> Option<Arc<FilesystemLeaseProbeState>> {
    filesystem_lease_probes()
        .lock()
        .expect("sandbox filesystem lease probe registry poisoned")
        .get(path)
        .and_then(Weak::upgrade)
}

pub(in crate::sandbox_filesystem) async fn verify_fresh_directory(
    path: &Path,
) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        FilesystemStorageError::io("verify runtime directory", Some(path), error)
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FilesystemStorageError::from(
            OperationContext::verification("verify fresh runtime directory", Some(path)),
        ));
    }
    verify_empty_directory(path).await
}

#[cfg(target_os = "linux")]
pub(in crate::sandbox_filesystem) async fn verify_fresh_open_directory(
    path: &Path,
) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::metadata(path).await.map_err(|error| {
        FilesystemStorageError::io("verify runtime directory", Some(path), error)
    })?;
    if !metadata.is_dir() {
        return Err(FilesystemStorageError::from(
            OperationContext::verification("verify fresh runtime directory", Some(path)),
        ));
    }
    verify_empty_directory(path).await
}

pub(in crate::sandbox_filesystem) async fn verify_empty_directory(
    path: &Path,
) -> Result<(), FilesystemStorageError> {
    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", Some(path), error)
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| {
            FilesystemStorageError::io("verify empty runtime directory", Some(path), error)
        })?
        .is_some()
    {
        return Err(FilesystemStorageError::from(
            OperationContext::verification("verify empty runtime directory", Some(path)),
        ));
    }
    Ok(())
}

pub(in crate::sandbox_filesystem) async fn rollback_created_filesystem(
    filesystem: SandboxFilesystem,
    creation_error: FilesystemStorageError,
) -> FilesystemStorageError {
    creation_error_after_cleanup(
        creation_error,
        SandboxFilesystem::delete_owned_filesystem(filesystem)
            .await
            .map_err(|error| error.into_source()),
    )
}

pub(in crate::sandbox_filesystem) fn creation_error_after_cleanup(
    creation_error: FilesystemStorageError,
    cleanup: Result<(), FilesystemStorageError>,
) -> FilesystemStorageError {
    match cleanup {
        Ok(()) => creation_error,
        Err(cleanup_error) => creation_error.with_cleanup_failure(cleanup_error),
    }
}

pub(in crate::sandbox_filesystem) async fn rollback_creation(
    path: &Path,
    creation_error: FilesystemStorageError,
    cleanup_retry: &RetryConfig,
) -> FilesystemStorageError {
    let cleanup = remove_and_verify(path, "roll back runtime directory", cleanup_retry).await;
    creation_error_after_cleanup(creation_error, cleanup)
}

pub(in crate::sandbox_filesystem) async fn remove_and_verify(
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

pub(in crate::sandbox_filesystem) async fn remove_and_verify_once(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir_all(path).await.map_err(|error| {
                FilesystemStorageError::cleanup_io(operation, Some(path), error)
            })?;
        }
        Ok(_) => {
            tokio::fs::remove_file(path).await.map_err(|error| {
                FilesystemStorageError::cleanup_io(operation, Some(path), error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilesystemStorageError::cleanup_io(
                operation,
                Some(path),
                error,
            ));
        }
    }

    verify_absent(path, operation)
}

pub(in crate::sandbox_filesystem) fn remove_and_verify_blocking(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|error| {
                FilesystemStorageError::cleanup_io(operation, Some(path), error)
            })?;
        }
        Ok(_) => {
            std::fs::remove_file(path).map_err(|error| {
                FilesystemStorageError::cleanup_io(operation, Some(path), error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilesystemStorageError::cleanup_io(
                operation,
                Some(path),
                error,
            ));
        }
    }
    verify_absent(path, operation)
}

pub(in crate::sandbox_filesystem) fn verify_absent(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            operation,
            Some(path),
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(
            operation,
            Some(path),
            error,
        )),
    }
}

#[cfg(test)]
mod tests;
