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

use super::golem_config::ComponentCacheConfig;
use crate::metrics::component::record_compilation_time;
use async_trait::async_trait;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentDto, ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::SafeDisplay;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::service::compiled_component::CompiledComponentService;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use golem_service_base::storage::blob::BlobStorage;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::task::spawn_blocking;
use tracing::{debug, info_span, warn};
use wasmtime::component::Component;
use wasmtime::Engine;

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService: Send + Sync {
    async fn get(
        &self,
        engine: &Engine,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<(Component, ComponentDto), WorkerExecutorError>;

    // If a version is provided, deleted components will also be returned.
    // If no version is provided, only the latest non-deleted version is returned.
    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<ComponentRevision>,
    ) -> Result<ComponentDto, WorkerExecutorError>;

    /// Resolve a component given a user-provided string. The syntax of the provided string is allowed to vary between implementations.
    /// Resolving component is the component in whose context the resolution is being performed
    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_environment: EnvironmentId,
        resolving_application: ApplicationId,
        resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError>;

    /// Returns all the component metadata the implementation has cached.
    /// This is useful for some mock/local implementations.
    async fn all_cached_metadata(&self) -> Vec<ComponentDto>;
}

pub fn configured(
    cache_config: &ComponentCacheConfig,
    compiled_config: &CompiledComponentServiceConfig,
    registry_service: Arc<dyn RegistryService>,
    blob_storage: Arc<dyn BlobStorage>,
) -> Arc<dyn ComponentService> {
    let compiled_component_service =
        golem_service_base::service::compiled_component::configured(compiled_config, blob_storage);

    Arc::new(ComponentServiceDefault::new(
        registry_service,
        cache_config.max_capacity,
        cache_config.max_metadata_capacity,
        cache_config.time_to_idle,
        compiled_component_service,
    ))
}

pub struct ComponentServiceDefault {
    component_cache: Cache<ComponentKey, (), Component, WorkerExecutorError>,
    component_metadata_cache: Cache<ComponentKey, (), ComponentDto, WorkerExecutorError>,
    compiled_component_service: Arc<dyn CompiledComponentService>,
    registry_client: Arc<dyn RegistryService>,
}

impl ComponentServiceDefault {
    pub fn new(
        registry_client: Arc<dyn RegistryService>,
        max_component_capacity: usize,
        max_metadata_capacity: usize,
        time_to_idle: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService>,
    ) -> Self {
        Self {
            registry_client,
            component_cache: create_component_cache(max_component_capacity, time_to_idle),
            component_metadata_cache: create_component_metadata_cache(
                max_metadata_capacity,
                time_to_idle,
            ),
            compiled_component_service,
        }
    }
}

#[async_trait]
impl ComponentService for ComponentServiceDefault {
    async fn get(
        &self,
        engine: &Engine,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<(Component, ComponentDto), WorkerExecutorError> {
        let key = ComponentKey {
            component_id,
            component_revision,
        };
        let engine = engine.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let metadata = self
            .get_metadata(component_id, Some(component_revision))
            .await?;
        let environment_id = metadata.environment_id;

        let component = self
            .component_cache
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
                            let bytes = self
                                .registry_client
                                .download_component(component_id, component_revision)
                                .await
                                .map_err(|e| WorkerExecutorError::ComponentDownloadFailed {
                                    component_id,
                                    component_revision,
                                    reason: e.to_safe_string(),
                                })?;

                            let start = Instant::now();
                            let span = info_span!("Loading WASM component");
                            let component = spawn_blocking(move || {
                                let _enter = span.enter();
                                Component::from_binary(&engine, &bytes).map_err(|e| {
                                    WorkerExecutorError::ComponentParseFailed {
                                        component_id,
                                        component_revision,
                                        reason: format!("{e}"),
                                    }
                                })
                            })
                            .await
                            .map_err(|join_err| {
                                WorkerExecutorError::unknown(join_err.to_string())
                            })??;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
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
            .await?;

        Ok((component, metadata))
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<ComponentRevision>,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        match forced_revision {
            Some(component_revision) => {
                let client = self.registry_client.clone();
                self.component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id,
                            component_revision,
                        },
                        || {
                            Box::pin(async move {
                                let metadata = client
                                    .get_component_metadata(component_id, component_revision)
                                    .await
                                    .map_err(|e| {
                                        WorkerExecutorError::runtime(format!(
                                            "Failed getting component metadata: {}",
                                            e.to_safe_string()
                                        ))
                                    })?;
                                Ok(metadata)
                            })
                        },
                    )
                    .await
            }
            None => {
                let metadata = self
                    .registry_client
                    .get_deployed_component_metadata(component_id)
                    .await
                    .map_err(|e| {
                        WorkerExecutorError::runtime(format!(
                            "Failed getting component metadata: {}",
                            e.to_safe_string()
                        ))
                    })?;

                let metadata = self
                    .component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id,
                            component_revision: metadata.revision,
                        },
                        || Box::pin(async move { Ok(metadata) }),
                    )
                    .await?;

                Ok(metadata)
            }
        }
    }

    async fn resolve_component(
        &self,
        component_slug: String,
        resolving_environment: EnvironmentId,
        resolving_application: ApplicationId,
        resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        let result = self
            .registry_client
            .resolve_component(
                resolving_account,
                resolving_application,
                resolving_environment,
                &component_slug,
            )
            .await;

        match result {
            Ok(component) => Ok(Some(component.id)),
            Err(RegistryServiceError::NotFound(_)) => Ok(None),
            Err(other) => Err(WorkerExecutorError::runtime(format!(
                "Resolving component failed: {}",
                other.to_safe_string()
            ))),
        }
    }

    async fn all_cached_metadata(&self) -> Vec<ComponentDto> {
        self.component_metadata_cache
            .iter()
            .await
            .into_iter()
            .map(|(_, v)| v)
            .collect()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_revision: ComponentRevision,
}

fn create_component_metadata_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), ComponentDto, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "component_metadata",
    )
}

fn create_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), Component, WorkerExecutorError> {
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
