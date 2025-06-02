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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cloud::CloudGolemTypes;
use crate::error::GolemError;
use crate::metrics::component::record_compilation_time;
use crate::services::compiled_component::CompiledComponentService;
use async_lock::{RwLock, Semaphore};
use async_trait::async_trait;
use cloud_common::model::CloudComponentOwner;
use futures_util::TryStreamExt;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::component_metadata::{DynamicLinkedInstance, LinearMemory};
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::{
    AccountId, ComponentId, ComponentType, ComponentVersion, InitialComponentFile,
};
use golem_common::testing::LocalFileSystemComponentMetadata;
use golem_service_base::storage::blob::BlobStorage;
use golem_wasm_ast::analysis::AnalysedExport;
use serde::Deserialize;
use tokio::task::spawn_blocking;
use tracing::{debug, warn, Instrument};
use wasmtime::component::Component;
use wasmtime::Engine;

use crate::GolemTypes;

#[derive(Debug, Clone)]
pub struct ComponentMetadataPoly<ComponentOwner> {
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub plugin_installations: Vec<PluginInstallation>,
    pub component_owner: ComponentOwner,
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
    pub env: HashMap<String, String>,
}

pub type ComponentMetadata<T> = ComponentMetadataPoly<<T as GolemTypes>::ComponentOwner>;

impl From<LocalFileSystemComponentMetadata> for ComponentMetadata<CloudGolemTypes> {
    fn from(value: LocalFileSystemComponentMetadata) -> Self {
        Self {
            version: value.version,
            size: value.size,
            memories: value.memories,
            exports: value.exports,
            component_type: value.component_type,
            files: value.files,
            plugin_installations: vec![],
            component_owner: CloudComponentOwner {
                account_id: value.account_id,
                project_id: value.project_id,
            },
            dynamic_linking: value.dynamic_linking,
            env: value.env,
        }
    }
}

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService<T: GolemTypes>: Send + Sync {
    async fn get(
        &self,
        engine: &Engine,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(Component, ComponentMetadata<T>), GolemError>;

    async fn get_metadata(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        forced_version: Option<ComponentVersion>,
    ) -> Result<ComponentMetadata<T>, GolemError>;

    /// Resolve a component given a user provided string. The syntax of the provided string is allowed to vary between implementations.
    /// Resolving component is the component in whoose context the resolution is being performed
    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_component: T::ComponentOwner,
    ) -> Result<Option<ComponentId>, GolemError>;
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_version: ComponentVersion,
}

fn create_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), Component, GolemError> {
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

impl From<std::io::Error> for GolemError {
    fn from(value: std::io::Error) -> Self {
        GolemError::Unknown {
            details: format!("{}", value),
        }
    }
}

struct ComponentMetadataIndex {
    processed_files: HashSet<String>,
    metadata: HashMap<ComponentKey, LocalFileSystemComponentMetadata>,
    latest_versions: HashMap<ComponentId, u64>,
    id_by_name: HashMap<String, ComponentId>,
}

impl ComponentMetadataIndex {
    fn new() -> Self {
        Self {
            processed_files: HashSet::new(),
            metadata: HashMap::new(),
            latest_versions: HashMap::new(),
            id_by_name: HashMap::new(),
        }
    }
}

pub struct ComponentServiceLocalFileSystem {
    root: PathBuf,
    component_cache: Cache<ComponentKey, (), Component, GolemError>,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    index: RwLock<ComponentMetadataIndex>,
    updating_index: Semaphore,
}

impl ComponentServiceLocalFileSystem {
    pub fn new(
        root: &Path,
        max_capacity: usize,
        time_to_idle: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    ) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local component store");
        }
        Self {
            root: root.to_path_buf(),
            component_cache: create_component_cache(max_capacity, time_to_idle),
            compiled_component_service,
            index: RwLock::new(ComponentMetadataIndex::new()),
            updating_index: Semaphore::new(1),
        }
    }

    async fn refresh_index(&self) -> Result<(), GolemError> {
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
                                .map_err(|e| GolemError::Unknown {
                                    details: format!(
                                        "Failed to read content from file {file_name}: {e}"
                                    ),
                                })?;

                        let metadata = serde_json::from_str(&file_content).map_err(|e| {
                            GolemError::Unknown {
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
                let component_id = metadata.component_id.clone();
                let component_version = metadata.version;
                let component_name = metadata.component_name.clone();

                current
                    .latest_versions
                    .entry(component_id.clone())
                    .and_modify(|e| *e = (*e).max(component_version))
                    .or_insert(component_version);

                current
                    .id_by_name
                    .entry(component_name)
                    .or_insert(component_id.clone());

                let key = ComponentKey {
                    component_id,
                    component_version,
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
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Component, GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let component_id = component_id.clone();
        let engine = engine.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let path = wasm_path.to_path_buf();

        self.component_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_component_service
                        .get(&component_id, component_version, &engine)
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
                                let component_id = component_id.clone();
                                move || {
                                    Component::from_binary(&engine, &bytes).map_err(|e| {
                                        GolemError::ComponentParseFailed {
                                            component_id: component_id.clone(),
                                            component_version,
                                            reason: format!("{}", e),
                                        }
                                    })
                                }
                            })
                            .instrument(tracing::Span::current())
                            .await
                            .map_err(|join_err| GolemError::unknown(join_err.to_string()))??;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
                            debug!(
                                "Compiled {} in {}ms",
                                component_id,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_component_service
                                .put(&component_id, component_version, &component)
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
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<ComponentMetadata<CloudGolemTypes>, GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let metadata = self.index.read().await.metadata.get(&key).cloned();

        let metadata = if let Some(metadata) = metadata {
            metadata
        } else {
            self.refresh_index().await?;
            let metadata = self.index.read().await.metadata.get(&key).cloned();
            metadata.ok_or(GolemError::unknown(format!(
                "No such component found: {}/{}",
                component_id, component_version
            )))?
        };

        Ok(metadata.into())
    }

    async fn get_latest_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentMetadata<CloudGolemTypes>, GolemError> {
        self.refresh_index().await?;

        let index = self.index.read().await;

        let latest_version = index.latest_versions.get(component_id);

        let metadata = match latest_version {
            Some(component_version) => {
                let key = ComponentKey {
                    component_id: component_id.clone(),
                    component_version: *component_version,
                };
                let metadata = index.metadata.get(&key).cloned();
                metadata.ok_or(GolemError::unknown(format!(
                    "No such component found: {}/{}",
                    component_id, component_version
                )))?
            }
            None => Err(GolemError::unknown(
                "Could not find any component with the given id",
            ))?,
        };

        Ok(metadata.into())
    }
}

#[async_trait]
impl ComponentService<CloudGolemTypes> for ComponentServiceLocalFileSystem {
    async fn get(
        &self,
        engine: &Engine,
        _account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(Component, ComponentMetadata<CloudGolemTypes>), GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let metadata = self.index.read().await.metadata.get(&key).cloned();

        let metadata = if let Some(metadata) = metadata {
            metadata
        } else {
            self.refresh_index().await?;
            let metadata = self.index.read().await.metadata.get(&key).cloned();
            metadata.ok_or(GolemError::unknown(format!(
                "No such component found: {}/{}",
                component_id, component_version
            )))?
        };

        let wasm_path = self.root.join(metadata.wasm_filename.clone());

        let component = self
            .get_component_from_path(&wasm_path, engine, component_id, component_version)
            .await?;

        Ok((component, metadata.into()))
    }

    async fn get_metadata(
        &self,
        _account_id: &AccountId,
        component_id: &ComponentId,
        forced_version: Option<ComponentVersion>,
    ) -> Result<ComponentMetadata<CloudGolemTypes>, GolemError> {
        match forced_version {
            Some(version) => self.get_metadata_for_version(component_id, version).await,
            None => self.get_latest_metadata(component_id).await,
        }
    }

    async fn resolve_component(
        &self,
        component_reference: String,
        _resolving_component: CloudComponentOwner,
    ) -> Result<Option<ComponentId>, GolemError> {
        Ok(self
            .index
            .read()
            .await
            .id_by_name
            .get(&component_reference)
            .cloned())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentProperties {
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
}
