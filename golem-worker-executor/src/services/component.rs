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

use super::golem_config::ComponentCacheConfig;
use crate::metrics::component::record_compilation_time;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::component::Component;
use golem_service_base::service::compiled_component::CompiledComponentService;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use golem_service_base::storage::blob::BlobStorage;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::task::spawn_blocking;
use tracing::{debug, info_span, warn};
use wasmtime::Engine;

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService: Send + Sync {
    async fn get(
        &self,
        engine: &Engine,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError>;

    // If a version is provided, deleted components will also be returned.
    // If no version is provided, only the latest non-deleted version is returned.
    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError>;

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
    async fn all_cached_metadata(&self) -> Vec<Component>;

    /// Invalidates cached "latest deployed" component metadata.
    async fn invalidate_latest_deployed_metadata(&self) {}

    /// Invalidates cached "latest deployed" component metadata for a specific environment.
    async fn invalidate_latest_deployed_metadata_for_environment(
        &self,
        _environment_id: EnvironmentId,
    ) {
        self.invalidate_latest_deployed_metadata().await
    }

    /// Invalidates all cached component metadata.
    async fn invalidate_all(&self) {
        self.invalidate_latest_deployed_metadata().await
    }
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
    component_cache: Cache<ComponentKey, (), wasmtime::component::Component, WorkerExecutorError>,
    component_metadata_cache: Cache<ComponentKey, (), Component, WorkerExecutorError>,
    latest_component_metadata_cache: Cache<ComponentId, (), Component, WorkerExecutorError>,
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
            latest_component_metadata_cache: create_latest_component_metadata_cache(
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
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
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
                                wasmtime::component::Component::from_binary(&engine, &bytes)
                                    .map_err(|e| WorkerExecutorError::ComponentParseFailed {
                                        component_id,
                                        component_revision,
                                        reason: format!("{e}"),
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
    ) -> Result<Component, WorkerExecutorError> {
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
                let client = self.registry_client.clone();
                let metadata = self
                    .latest_component_metadata_cache
                    .get_or_insert_simple(&component_id, || {
                        Box::pin(async move {
                            client
                                .get_deployed_component_metadata(component_id)
                                .await
                                .map_err(|e| {
                                    WorkerExecutorError::runtime(format!(
                                        "Failed getting component metadata: {}",
                                        e.to_safe_string()
                                    ))
                                })
                        })
                    })
                    .await?;

                self.component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id,
                            component_revision: metadata.revision,
                        },
                        || {
                            let metadata = metadata.clone();
                            Box::pin(async move { Ok(metadata) })
                        },
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

    async fn all_cached_metadata(&self) -> Vec<Component> {
        self.component_metadata_cache
            .iter()
            .await
            .into_iter()
            .map(|(_, v)| v)
            .collect()
    }

    async fn invalidate_latest_deployed_metadata(&self) {
        let keys = self.latest_component_metadata_cache.keys().await;
        for key in keys {
            self.latest_component_metadata_cache.remove(&key).await;
        }
    }

    async fn invalidate_latest_deployed_metadata_for_environment(
        &self,
        environment_id: EnvironmentId,
    ) {
        let keys = self
            .latest_component_metadata_cache
            .iter()
            .await
            .into_iter()
            .filter_map(|(component_id, component)| {
                (component.environment_id == environment_id).then_some(component_id)
            })
            .collect::<Vec<_>>();

        for key in keys {
            self.latest_component_metadata_cache.remove(&key).await;
        }
    }

    async fn invalidate_all(&self) {
        self.invalidate_latest_deployed_metadata().await;

        let metadata_keys = self.component_metadata_cache.keys().await;
        for key in metadata_keys {
            self.component_metadata_cache.remove(&key).await;
        }
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
) -> Cache<ComponentKey, (), Component, WorkerExecutorError> {
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

fn create_latest_component_metadata_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentId, (), Component, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "latest_component_metadata",
    )
}

fn create_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), wasmtime::component::Component, WorkerExecutorError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType, ResolvedAgentType};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::TokenSecret;
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
    use golem_service_base::clients::registry::RegistryServiceError;
    use golem_service_base::clients::registry::{RegistryInvalidationHandler, ResourceUsageUpdate};
    use golem_service_base::custom_api::CompiledRoutes;
    use golem_service_base::mcp::CompiledMcp;
    use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment};
    use golem_service_base::model::component::Component;
    use golem_service_base::model::environment::EnvironmentState;
    use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
    use golem_service_base::service::compiled_component::CompiledComponentServiceDisabled;
    use std::collections::HashMap;
    use test_r::test;

    struct MockRegistryService {
        components: HashMap<ComponentId, Component>,
        deployed_component_calls: Arc<std::sync::Mutex<HashMap<ComponentId, usize>>>,
    }

    impl MockRegistryService {
        fn new(components: impl IntoIterator<Item = Component>) -> Self {
            Self {
                components: components
                    .into_iter()
                    .map(|component| (component.id, component))
                    .collect(),
                deployed_component_calls: Arc::new(std::sync::Mutex::new(HashMap::new())),
            }
        }

        fn deployed_component_calls(&self) -> Arc<std::sync::Mutex<HashMap<ComponentId, usize>>> {
            self.deployed_component_calls.clone()
        }
    }

    #[async_trait]
    impl RegistryService for MockRegistryService {
        async fn authenticate_token(
            &self,
            _token: &TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_auth_details_for_environment(
            &self,
            _environment_id: EnvironmentId,
            _include_deleted: bool,
            _auth_ctx: &AuthCtx,
        ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _account_id: golem_common::model::account::AccountId,
        ) -> Result<ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_connection_limit(
            &self,
            _account_id: golem_common::model::account::AccountId,
            _agent_id: &golem_common::model::AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_resource_usage(
            &self,
            _updates: HashMap<golem_common::model::account::AccountId, ResourceUsageUpdate>,
        ) -> Result<AccountResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn download_component(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_component_metadata(
            &self,
            component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            self.components
                .get(&component_id)
                .cloned()
                .ok_or_else(|| RegistryServiceError::NotFound("missing component".to_string()))
        }

        async fn get_deployed_component_metadata(
            &self,
            component_id: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            let mut calls = self.deployed_component_calls.lock().unwrap();
            *calls.entry(component_id).or_default() += 1;
            drop(calls);

            self.components
                .get(&component_id)
                .cloned()
                .ok_or_else(|| RegistryServiceError::NotFound("missing component".to_string()))
        }

        async fn get_all_deployed_component_revisions(
            &self,
            _component_id: ComponentId,
        ) -> Result<Vec<Component>, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_component(
            &self,
            _resolving_account_id: golem_common::model::account::AccountId,
            _resolving_application_id: ApplicationId,
            _resolving_environment_id: EnvironmentId,
            _component_slug: &str,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_agent_types(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_agent_type(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
            _name: &AgentTypeName,
        ) -> Result<RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_by_names(
            &self,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
            _deployment_revision: Option<DeploymentRevision>,
            _owner_account_email: Option<&str>,
            _auth_ctx: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_routes_for_domain(
            &self,
            _domain: &Domain,
        ) -> Result<CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_compiled_mcps_for_domain(
            &self,
            _domain: &Domain,
        ) -> Result<CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_current_environment_state(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<EnvironmentState, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_id(
            &self,
            _resource_definition_id: ResourceDefinitionId,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_name(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn subscribe_registry_invalidations(
            &self,
            _last_seen_event_id: Option<u64>,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                golem_common::model::agent::RegistryInvalidationEvent,
                                RegistryServiceError,
                            >,
                        > + Send,
                >,
            >,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn run_registry_invalidation_event_subscriber(
            &self,
            _service_name: &'static str,
            _shutdown_token: Option<tokio_util::sync::CancellationToken>,
            _handler: Arc<dyn RegistryInvalidationHandler>,
        ) {
            unimplemented!()
        }
    }

    fn make_component(component_id: ComponentId, environment_id: EnvironmentId) -> Component {
        Component {
            id: component_id,
            revision: ComponentRevision::new(1).unwrap(),
            environment_id,
            component_name: ComponentName("test:component".to_string()),
            hash: golem_common::model::diff::Hash::new(blake3::hash(b"component-hash")),
            application_id: ApplicationId::new(),
            account_id: golem_common::model::account::AccountId::new(),
            component_size: 1,
            metadata: golem_common::model::component_metadata::ComponentMetadata::default(),
            created_at: chrono::Utc::now(),
            wasm_hash: golem_common::model::diff::Hash::new(blake3::hash(b"wasm-hash")),
            object_store_key: "test-object".to_string(),
        }
    }

    #[test]
    async fn caches_latest_deployed_component_metadata_by_component_id() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new([make_component(
            component_id,
            environment_id,
        )]));
        let calls = registry.deployed_component_calls();
        let service = ComponentServiceDefault::new(
            registry,
            1,
            8,
            Duration::from_secs(60),
            Arc::new(CompiledComponentServiceDisabled::new()),
        );

        let first = service.get_metadata(component_id, None).await.unwrap();
        let second = service.get_metadata(component_id, None).await.unwrap();

        assert_eq!(first.environment_id, environment_id);
        assert_eq!(second.environment_id, environment_id);
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.get(&component_id).copied().unwrap_or_default(),
            1,
            "latest deployed metadata should be fetched once per component id"
        );
    }

    #[test]
    async fn invalidating_latest_deployed_component_metadata_forces_refresh() {
        let component_id = ComponentId::new();
        let registry = Arc::new(MockRegistryService::new([make_component(
            component_id,
            EnvironmentId::new(),
        )]));
        let calls = registry.deployed_component_calls();
        let service = ComponentServiceDefault::new(
            registry,
            1,
            8,
            Duration::from_secs(60),
            Arc::new(CompiledComponentServiceDisabled::new()),
        );

        let _ = service.get_metadata(component_id, None).await.unwrap();
        service.invalidate_latest_deployed_metadata().await;
        let _ = service.get_metadata(component_id, None).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.get(&component_id).copied().unwrap_or_default(),
            2,
            "invalidating latest deployed metadata should force a fresh registry lookup"
        );
    }

    #[test]
    async fn invalidating_latest_deployed_component_metadata_for_environment_only_evicts_matching_entries()
     {
        let first_component_id = ComponentId::new();
        let second_component_id = ComponentId::new();
        let first_environment_id = EnvironmentId::new();
        let second_environment_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new([
            make_component(first_component_id, first_environment_id),
            make_component(second_component_id, second_environment_id),
        ]));
        let calls = registry.deployed_component_calls();
        let service = ComponentServiceDefault::new(
            registry,
            1,
            8,
            Duration::from_secs(60),
            Arc::new(CompiledComponentServiceDisabled::new()),
        );

        let _ = service
            .get_metadata(first_component_id, None)
            .await
            .unwrap();
        let _ = service
            .get_metadata(second_component_id, None)
            .await
            .unwrap();

        service
            .invalidate_latest_deployed_metadata_for_environment(first_environment_id)
            .await;

        let _ = service
            .get_metadata(first_component_id, None)
            .await
            .unwrap();
        let _ = service
            .get_metadata(second_component_id, None)
            .await
            .unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.get(&first_component_id).copied().unwrap_or_default(),
            2,
            "matching environment should be refreshed"
        );
        assert_eq!(
            calls.get(&second_component_id).copied().unwrap_or_default(),
            1,
            "other environments should stay cached"
        );
    }
}
