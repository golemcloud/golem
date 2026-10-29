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

use crate::blocking::BlockingTask;
use crate::host_directories::{HostDirectory, TemporaryRoot};
use crate::operation_failure::OperationContext;
use crate::sandbox_filesystem::{FilesystemStorageError, SandboxFilesystemProvisioning};

use crate::services::golem_config::FilesystemStorageMode;
use crate::storage_volume::FilesystemVolume;
use golem_common::model::RetryConfig;
use std::sync::Arc;

const SCRATCH: &str = ".scratch";
const INITIAL_FILES: &str = ".initial-files";

/// Executor-owned directories, outside every agent filesystem generation.
#[derive(Debug)]
pub(crate) struct ExecutorDirectories {
    pub(crate) scratch: HostDirectory,
    pub(crate) initial_files: HostDirectory,
}

pub(crate) async fn provision_executor_directories(
    storage: &FilesystemStorageMode,
    cleanup_retry: RetryConfig,
) -> Result<(SandboxFilesystemProvisioning, ExecutorDirectories), FilesystemStorageError> {
    let volume = FilesystemVolume::open(storage, cleanup_retry.clone())?;
    let directories = make_executor_directories(&volume).await?;
    let provisioning = SandboxFilesystemProvisioning::on_volume(volume, cleanup_retry);
    Ok((provisioning, directories))
}

async fn make_executor_directories(
    volume: &FilesystemVolume,
) -> Result<ExecutorDirectories, FilesystemStorageError> {
    let (root, _) = volume.root_location();
    BlockingTask::Spawn
        .run(executor_directories_task(volume))
        .await
        .map_err(|error| {
            FilesystemStorageError::from(OperationContext::task_failure(
                "create executor directories",
                root.as_deref(),
                error,
            ))
        })?
}

fn executor_directories_task(
    volume: &FilesystemVolume,
) -> impl FnOnce() -> Result<ExecutorDirectories, FilesystemStorageError> + Send + 'static {
    let volume = volume.clone();
    let (root, anchor) = volume.root_location();
    move || {
        let (root, temporary_root) = match root {
            Some(root) => (root, None),
            None => {
                let directory = tempfile::Builder::new()
                    .prefix("golem-host-directories")
                    .tempdir()
                    .map_err(|error| {
                        FilesystemStorageError::io("create executor directory root", None, error)
                    })?;
                let directory = Arc::new(TemporaryRoot::new(directory));
                (Arc::from(directory.path()), Some(directory))
            }
        };
        let create = |name| -> Result<HostDirectory, FilesystemStorageError> {
            let directory = HostDirectory::create_fresh_blocking(
                root.join(name),
                anchor.clone(),
                temporary_root.clone(),
            )?;
            if let Err(error) = volume.validate_host_directory(directory.path().as_path()) {
                return Err(error_after_removal(
                    error.into(),
                    directory.discard_blocking().map_err(Into::into),
                ));
            }
            Ok(directory)
        };
        let scratch = create(SCRATCH)?;
        let initial_files = match create(INITIAL_FILES) {
            Ok(directory) => directory,
            Err(error) => {
                return Err(error_after_removal(
                    error,
                    scratch.discard_blocking().map_err(Into::into),
                ));
            }
        };
        Ok(ExecutorDirectories {
            scratch,
            initial_files,
        })
    }
}

fn error_after_removal(
    error: FilesystemStorageError,
    removal: Result<(), FilesystemStorageError>,
) -> FilesystemStorageError {
    match removal {
        Ok(()) => error,
        Err(cleanup) => error.with_cleanup_failure(cleanup),
    }
}

#[cfg(test)]
mod tests;
