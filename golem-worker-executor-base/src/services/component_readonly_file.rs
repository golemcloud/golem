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
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::{ComponentId, ComponentVersion, InitialFilePermission};
use std::sync::Arc;
use tempfile::TempDir;
use tracing::debug;

#[async_trait]
pub trait ComponentReadOnlyFileService {
    async fn get_component_read_only_dir(
        &self,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Arc<TempDir>, GolemError>;
}

pub struct DefaultComponentReadOnlyFileService {
    component_service: Arc<dyn ComponentService + Send + Sync>,
    read_only_dir_cache: Cache<ComponentKey, (), Arc<TempDir>, GolemError>,
}

impl DefaultComponentReadOnlyFileService {
    pub fn new(component_service: Arc<dyn ComponentService + Send + Sync>) -> Self {
        Self {
            component_service: component_service.clone(),
            read_only_dir_cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "read_only_dir_cache",
            ),
        }
    }
}

#[async_trait]
impl ComponentReadOnlyFileService for DefaultComponentReadOnlyFileService {
    async fn get_component_read_only_dir(
        &self,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Arc<TempDir>, GolemError> {
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
                    let read_only_temp_dir = tempfile::Builder::new().prefix("golem").tempdir().map_err(
                        |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
                    )?;
                    debug!(
                        "Populate temporary read only file system with read only files. Component: {}, version: {}",
                        component_id_clone.clone(), component_version
                    );
                    let component_initial_files = component_service
                        .get_initial_files(&component_id_clone, Some(component_version))
                        .await?;
                    for initial_file in component_initial_files.initial_files {
                        match initial_file.file_permission {
                            InitialFilePermission::ReadOnly => {
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
                            InitialFilePermission::ReadWrite => {},
                        }
                    }
                    Ok(Arc::new(read_only_temp_dir))
                })
            })
            .await?;
        Ok(temp_dir)
    }
}
