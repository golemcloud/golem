// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::error::GolemError;
use crate::services::component::{ComponentKey, ComponentService};
use crate::services::worker::WorkerService;
use anyhow::anyhow;
use async_trait::async_trait;
use futures::Stream;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::{FilePermission, OwnedWorkerId, WorkerId};
use std::collections::VecDeque;
use std::os::unix::prelude::MetadataExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::SystemTime;
use tempfile::TempDir;
use tracing::debug;

#[derive(Debug, Clone)]
pub enum NodeType {
    File,
    Directory,
}

impl From<NodeType> for i32 {
    fn from(node_type: NodeType) -> Self {
        match node_type {
            NodeType::File => 0,
            NodeType::Directory => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerFileSystemNode {
    pub name: String,
    pub node_type: NodeType,
    pub size: u64,
    pub permission: FilePermission,
    pub last_modified: chrono::DateTime<chrono::Utc>,
}

impl From<WorkerFileSystemNode> for golem_api_grpc::proto::golem::common::FileSystemNode {
    fn from(value: WorkerFileSystemNode) -> Self {
        Self {
            name: value.name,
            r#type: value.node_type.into(),
            size: value.size,
            permissions: value.permission.into(),
            last_modified: Some(prost_types::Timestamp::from(SystemTime::from(
                value.last_modified,
            ))),
        }
    }
}

pub struct WorkerFileContentStream(aws_sdk_s3::primitives::ByteStream);

impl WorkerFileContentStream {
    async fn from_path(path: &Path) -> anyhow::Result<Self> {
        Ok(WorkerFileContentStream(
            aws_sdk_s3::primitives::ByteStream::from_path(path)
                .await
                .map_err(|e| anyhow!("Cannot get file content: {}", e.to_string()))?,
        ))
    }
}

impl Stream for WorkerFileContentStream {
    type Item = Result<Vec<u8>, anyhow::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map_ok(|b| b.to_vec())
            .map_err(|e| e.into())
    }
}

pub enum DirectoryOrFile {
    Dir,
    File,
}

impl From<DirectoryOrFile> for i32 {
    fn from(dir_or_file: DirectoryOrFile) -> Self {
        match dir_or_file {
            DirectoryOrFile::Dir => 0,
            DirectoryOrFile::File => 1,
        }
    }
}

#[async_trait]
pub trait WorkerFileService {
    async fn get_worker_read_only_dir(
        &self,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Arc<TempDir>, GolemError>;

    async fn get_worker_dir(
        &self,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Arc<TempDir>, GolemError>;

    async fn list_directory(
        &self,
        owned_worker_id: &OwnedWorkerId,
        path: &Path,
    ) -> Result<Vec<WorkerFileSystemNode>, GolemError>;

    async fn get_file_content(
        &self,
        owned_worker_id: &OwnedWorkerId,
        file_path: &Path,
    ) -> Result<WorkerFileContentStream, GolemError>;

    async fn check_file_or_directory(
        &self,
        owned_worker_id: &OwnedWorkerId,
        file_path: &Path,
    ) -> Result<DirectoryOrFile, GolemError>;
}

pub struct DefaultWorkerFileService {
    component_service: Arc<dyn ComponentService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    read_only_dir_cache: Cache<ComponentKey, (), Arc<TempDir>, GolemError>,
    worker_dir_cache: Cache<WorkerId, (), Arc<TempDir>, GolemError>,
}

impl DefaultWorkerFileService {
    pub fn new(
        component_service: Arc<dyn ComponentService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
    ) -> Self {
        Self {
            component_service: component_service.clone(),
            worker_service: worker_service.clone(),
            read_only_dir_cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "read_only_dir_cache",
            ),
            worker_dir_cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "worker_dir_cache",
            ),
        }
    }
}

#[async_trait]
impl WorkerFileService for DefaultWorkerFileService {
    async fn get_worker_read_only_dir(
        &self,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Arc<TempDir>, GolemError> {
        match self.worker_service.get(owned_worker_id).await {
            Some(worker) => {
                let worker_id = worker.worker_id.clone();
                let component_id = worker_id.component_id.clone();
                let component_version = worker.last_known_status.component_version;
                let key = ComponentKey {
                    component_id: component_id.clone(),
                    component_version,
                };
                let component_id_clone = component_id.clone();
                let component_service = self.component_service.clone();
                let temp_dir = self
                    .read_only_dir_cache
                    .get_or_insert_simple(
                        &key.clone(), || {
                            Box::pin(async move {
                                let read_only_temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
                                    |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
                                )?);
                                debug!(
                                    "Populate temporary read only file system with read only files. Component: {}, version: {}",
                                    component_id_clone.clone(), component_version
                                );
                                let component_initial_files = component_service
                                    .get_initial_files(&component_id_clone, Some(component_version))
                                    .await?;
                                for initial_file in component_initial_files.initial_files {
                                    match initial_file.file_permission {
                                        FilePermission::ReadOnly => {
                                            if initial_file.file_path.is_absolute() {
                                                let mut cur_path = read_only_temp_dir.path().to_path_buf();
                                                for path_component in initial_file.file_path.components() {
                                                    if !cur_path.exists() {
                                                        tokio::fs::create_dir(&cur_path).await?;
                                                    }
                                                    cur_path.push(path_component);
                                                }
                                                let file_content = component_service.get_initial_file_data(
                                                    &component_id_clone,
                                                    component_version,
                                                    cur_path.as_path()
                                                ).await?;
                                                tokio::fs::write(cur_path, file_content).await?;
                                            } else {
                                                return Err(GolemError::runtime(format!(
                                                    "Failed to populate temporary directory: {} is not an absolute path",
                                                    initial_file.file_path.display()
                                                )));
                                            }
                                        },
                                        FilePermission::ReadWrite => {},
                                    }
                                }
                                Ok(read_only_temp_dir)
                            })
                        })
                    .await?;
                Ok(temp_dir)
            }
            None => Err(GolemError::runtime(format!(
                "Worker with id {} not found",
                owned_worker_id.worker_id
            ))),
        }
    }

    async fn get_worker_dir(
        &self,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Arc<TempDir>, GolemError> {
        match self.worker_service.get(owned_worker_id).await {
            Some(worker) => {
                let worker_id = worker.worker_id.clone();
                let component_id = worker_id.component_id.clone();
                let component_version = worker.last_known_status.component_version;
                let read_only_temp_dir = self.get_worker_read_only_dir(owned_worker_id).await?;
                let component_service = self.component_service.clone();
                self
                    .worker_dir_cache
                    .get_or_insert_simple(
                        &worker_id.clone(),
                        || {
                            Box::pin(async move {
                                let temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
                                    |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
                                )?);

                                debug!(
                                    "Created temporary file system root at {:?}",
                                    temp_dir.path()
                                );

                                let component_initial_files = component_service
                                    .get_initial_files(
                                        &component_id,
                                        Some(component_version),
                                    )
                                    .await?;

                                for initial_file in component_initial_files.initial_files {
                                    if initial_file.file_path.is_absolute() {
                                        let mut cur_path = temp_dir.path().to_path_buf();
                                        let mut cur_read_only_path = read_only_temp_dir.path().to_path_buf();
                                        for path_component in initial_file.file_path.components() {
                                            if !cur_path.exists() {
                                                tokio::fs::create_dir(&cur_path).await?;
                                            }
                                            cur_path.push(path_component);
                                            cur_read_only_path.push(path_component);
                                        }
                                        match initial_file.file_permission {
                                            FilePermission::ReadOnly => {
                                                if cur_read_only_path.exists() {
                                                    tokio::fs::symlink(cur_path.as_path(), cur_read_only_path.as_path())
                                                        .await?;
                                                } else {
                                                    return Err(GolemError::runtime(format!(
                                                        "Failed to populate temporary directory: try link {} with a non existence {}",
                                                        cur_path.display(),
                                                        cur_read_only_path.display()
                                                    )));
                                                }
                                            }
                                            FilePermission::ReadWrite => {
                                                let file_content = component_service
                                                    .get_initial_file_data(
                                                        &component_id,
                                                        component_version,
                                                        cur_path.as_path(),
                                                    )
                                                    .await?;
                                                tokio::fs::write(cur_path, file_content).await?;
                                            }
                                        }
                                    } else {
                                        return Err(GolemError::runtime(format!(
                                            "Failed to populate temporary directory: {} is not an absolute path",
                                            initial_file.file_path.display()
                                        )));
                                    }
                                }

                                debug!("Populated temporary file system with files");

                                Ok(temp_dir)
                            })
                        }
                    )
                    .await
            }
            None => Err(GolemError::runtime(format!(
                "Worker with id {} not found",
                owned_worker_id.worker_id
            ))),
        }
    }

    async fn list_directory(
        &self,
        owned_worker_id: &OwnedWorkerId,
        path: &Path,
    ) -> Result<Vec<WorkerFileSystemNode>, GolemError> {
        let worker_temp_dir = self.get_worker_dir(owned_worker_id).await?;
        let mut absolute_path = worker_temp_dir.path().to_path_buf();
        for path_component in path.components() {
            absolute_path.push(path_component);
        }
        let mut read_dir = tokio::fs::read_dir(absolute_path).await?;
        let mut paths = VecDeque::new();
        while let Some(entry) = read_dir.next_entry().await? {
            paths.push_back(entry.path());
        }
        let mut result = Vec::new();
        while !paths.is_empty() {
            let path = paths.pop_front().unwrap();
            let metadata = path.metadata()?;
            if metadata.is_symlink() {
                let linked_path = tokio::fs::read_link(path).await?;
                paths.push_front(linked_path);
                continue;
            }
            let name = path
                .file_name()
                .ok_or(GolemError::runtime(format!(
                    "Cannot get file name of {}",
                    path.to_string_lossy()
                )))?
                .to_os_string()
                .into_string()
                .map_err(|e| {
                    GolemError::runtime(format!(
                        "Cannot convert file name {} into string",
                        e.to_string_lossy()
                    ))
                })?;
            let node_type = if metadata.is_dir() {
                NodeType::Directory
            } else {
                NodeType::File
            };
            let size = metadata.size();
            let permission = if metadata.permissions().readonly() {
                FilePermission::ReadOnly
            } else {
                FilePermission::ReadWrite
            };
            let last_modified = metadata
                .modified()
                .map_err(|e| {
                    GolemError::runtime(format!("Cannot convert file name {} into string", e))
                })?
                .into();
            result.push(WorkerFileSystemNode {
                name,
                node_type,
                size,
                permission,
                last_modified,
            })
        }
        Ok(result)
    }

    async fn get_file_content(
        &self,
        owned_worker_id: &OwnedWorkerId,
        file_path: &Path,
    ) -> Result<WorkerFileContentStream, GolemError> {
        let worker_temp_dir = self.get_worker_dir(owned_worker_id).await?;
        let mut absolute_path = worker_temp_dir.path().to_path_buf();
        for path_component in file_path.components() {
            absolute_path.push(path_component);
        }
        while absolute_path.is_symlink() {
            absolute_path = tokio::fs::read_link(absolute_path)
                .await
                .map_err(|e| GolemError::runtime(e.to_string()))?;
        }
        WorkerFileContentStream::from_path(absolute_path.as_path())
            .await
            .map_err(|e| GolemError::runtime(e.to_string()))
    }

    async fn check_file_or_directory(
        &self,
        owned_worker_id: &OwnedWorkerId,
        path: &Path,
    ) -> Result<DirectoryOrFile, GolemError> {
        let worker_temp_dir = self.get_worker_dir(owned_worker_id).await?;
        let mut absolute_path = worker_temp_dir.path().to_path_buf();
        for path_component in path.components() {
            absolute_path.push(path_component);
        }
        while absolute_path.is_symlink() {
            absolute_path = tokio::fs::read_link(absolute_path)
                .await
                .map_err(|e| GolemError::runtime(e.to_string()))?;
        }
        if absolute_path.is_dir() {
            return Ok(DirectoryOrFile::Dir);
        }
        if absolute_path.is_file() {
            return Ok(DirectoryOrFile::File);
        }
        Err(GolemError::runtime(format!(
            "{} is not a file nor directory",
            path.to_string_lossy()
        )))
    }
}
