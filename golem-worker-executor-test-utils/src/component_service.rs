// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::component_writer::LocalFileSystemComponentMetadata;
use async_lock::{RwLock, Semaphore};
use async_trait::async_trait;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::ComponentDto;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::service::compiled_component::CompiledComponentService;
use golem_worker_executor::services::component::ComponentService;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::spawn_blocking;
use tracing::{debug, warn, Instrument};
use wasmtime::component::Component;
use wasmtime::Engine;

pub struct ComponentServiceLocalFileSystem {
    root: PathBuf,
    component_cache: Cache<CacheKey, (), Component, WorkerExecutorError>,
    compiled_component_service: Arc<dyn CompiledComponentService>,
    index: RwLock<ComponentMetadataIndex>,
    updating_index: Semaphore,
}

impl ComponentServiceLocalFileSystem {
    pub fn new(
        root: &Path,
        max_capacity: usize,
        time_to_idle: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService>,
    ) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local component store");
        }

        tracing::info!(
            "Created local file system component service with root {}",
            root.to_string_lossy()
        );

        Self {
            root: root.to_path_buf(),
            component_cache: create_component_cache(max_capacity, time_to_idle),
            compiled_component_service,
            index: RwLock::new(ComponentMetadataIndex::new()),
            updating_index: Semaphore::new(1),
        }
    }

    async fn refresh_index(&self) -> Result<(), WorkerExecutorError> {
        let permit = self.updating_index.acquire().await;

        let mut new_processed_files: Vec<String> = vec![];
        let mut new_metadata: Vec<LocalFileSystemComponentMetadata> = vec![];
        {
            let current = self.index.read().await;

            let mut reader = tokio::fs::read_dir(&self.root).await?;
            while let Some(entry) = reader.next_entry().await? {
                if let Ok(file_name) = entry.file_name().into_string() {
                    if !current.processed_files.contains(&file_name) && file_name.ends_with(".json")
                    {
                        new_processed_files.push(file_name.clone());

                        let file_content =
                            tokio::fs::read_to_string(self.root.join(file_name.clone()))
                                .await
                                .map_err(|e| WorkerExecutorError::Unknown {
                                    details: format!(
                                        "Failed to read content from file {file_name}: {e}"
                                    ),
                                })?;

                        let metadata = serde_json::from_str(&file_content).map_err(|e| {
                            WorkerExecutorError::Unknown {
                                details: format!("Failed to deserialize properties of component from {file_name}: {e}")
                            }
                        })?;

                        new_metadata.push(metadata);
                    };
                };
            }
        }

        {
            let mut current = self.index.write().await;

            for file in new_processed_files {
                current.processed_files.insert(file);
            }

            for metadata in new_metadata {
                let component_id = metadata.component_id;
                let component_revision = metadata.revision;
                let component_name = metadata.component_name.clone();

                current
                    .latest_revision
                    .entry(component_id)
                    .and_modify(|e| *e = (*e).max(component_revision))
                    .or_insert(component_revision);

                current
                    .id_by_name
                    .entry(component_name)
                    .or_insert(component_id);

                let key = CacheKey {
                    component_id,
                    component_revision,
                };

                current.metadata.entry(key).or_insert(metadata);
            }
        }

        drop(permit);
        Ok(())
    }

    async fn get_component_from_path(
        &self,
        wasm_path: &Path,
        engine: &Engine,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Component, WorkerExecutorError> {
        let key = CacheKey {
            component_id,
            component_revision,
        };
        let engine = engine.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let path = wasm_path.to_path_buf();

        self.component_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_component_service
                        .get(environment_id, component_id, component_revision, &engine)
                        .await;

                    let component = match result {
                        Ok(component) => component,
                        Err(err) => {
                            warn!("Failed to download compiled component {:?}: {}", key, err);
                            None
                        }
                    };

                    match component {
                        Some(component) => Ok(component),
                        None => {
                            let bytes = tokio::fs::read(path).await?;

                            let start = Instant::now();
                            let component = spawn_blocking({
                                let component_id = component_id;
                                move || {
                                    Component::from_binary(&engine, &bytes).map_err(|e| {
                                        WorkerExecutorError::ComponentParseFailed {
                                            component_id,
                                            component_revision,
                                            reason: format!("{e}"),
                                        }
                                    })
                                }
                            })
                            .instrument(tracing::Span::current())
                            .await
                            .map_err(|join_err| {
                                WorkerExecutorError::unknown(join_err.to_string())
                            })??;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            debug!(
                                "Compiled {} in {}ms",
                                component_id,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_component_service
                                .put(environment_id, component_id, component_revision, &component)
                                .await;

                            match result {
                                Ok(_) => Ok(component),
                                Err(err) => {
                                    warn!("Failed to upload compiled component {:?}: {}", key, err);
                                    Ok(component)
                                }
                            }
                        }
                    }
                })
            })
            .await
    }

    async fn get_metadata_for_version(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        let key = CacheKey {
            component_id,
            component_revision,
        };
        let metadata = self.index.read().await.metadata.get(&key).cloned();

        let metadata = if let Some(metadata) = metadata {
            metadata
        } else {
            self.refresh_index().await?;
            let metadata = self.index.read().await.metadata.get(&key).cloned();
            metadata.ok_or(WorkerExecutorError::unknown(format!(
                "No such component found: {component_id}/{component_revision}"
            )))?
        };

        Ok(metadata.into())
    }

    async fn get_latest_metadata(
        &self,
        component_id: ComponentId,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        self.refresh_index().await?;

        let index = self.index.read().await;

        let latest_revision = index.latest_revision.get(&component_id);

        let metadata = match latest_revision {
            Some(component_revision) => {
                let key = CacheKey {
                    component_id,
                    component_revision: *component_revision,
                };
                let metadata = index.metadata.get(&key).cloned();
                metadata.ok_or(WorkerExecutorError::unknown(format!(
                    "No such component found: {component_id}/{component_revision}"
                )))?
            }
            None => Err(WorkerExecutorError::unknown(
                "Could not find any component with the given id",
            ))?,
        };

        Ok(metadata.into())
    }
}

#[async_trait]
impl ComponentService for ComponentServiceLocalFileSystem {
    async fn get(
        &self,
        engine: &Engine,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<(Component, ComponentDto), WorkerExecutorError> {
        let key = CacheKey {
            component_id,
            component_revision,
        };
        let metadata = self.index.read().await.metadata.get(&key).cloned();

        let metadata = if let Some(metadata) = metadata {
            metadata
        } else {
            self.refresh_index().await?;
            let metadata = self.index.read().await.metadata.get(&key).cloned();
            metadata.ok_or(WorkerExecutorError::unknown(format!(
                "No such component found: {component_id}/{component_revision}"
            )))?
        };

        let wasm_path = self.root.join(metadata.wasm_filename.clone());

        let component = self
            .get_component_from_path(
                &wasm_path,
                engine,
                metadata.environment_id,
                component_id,
                component_revision,
            )
            .await?;

        Ok((component, ComponentDto::from(metadata)))
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<ComponentRevision>,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        let result = match forced_revision {
            Some(version) => self.get_metadata_for_version(component_id, version).await?,
            None => self.get_latest_metadata(component_id).await?,
        };
        Ok(result)
    }

    async fn resolve_component(
        &self,
        component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        Ok(self
            .index
            .read()
            .await
            .id_by_name
            .get(&component_reference)
            .cloned())
    }

    async fn all_cached_metadata(&self) -> Vec<ComponentDto> {
        self.index
            .read()
            .await
            .metadata
            .values()
            .map(|local_metadata| ComponentDto::from(local_metadata.clone()))
            .collect()
    }
}

struct ComponentMetadataIndex {
    processed_files: HashSet<String>,
    metadata: HashMap<CacheKey, LocalFileSystemComponentMetadata>,
    latest_revision: HashMap<ComponentId, ComponentRevision>,
    id_by_name: HashMap<String, ComponentId>,
}

impl ComponentMetadataIndex {
    fn new() -> Self {
        Self {
            processed_files: HashSet::new(),
            metadata: HashMap::new(),
            latest_revision: HashMap::new(),
            id_by_name: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    component_id: ComponentId,
    component_revision: ComponentRevision,
}

fn create_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<CacheKey, (), Component, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "component",
    )
}
