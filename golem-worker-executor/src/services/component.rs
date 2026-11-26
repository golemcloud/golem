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
use crate::services::compilation_limiter::CompilationLimiter;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff::Hash;
use golem_common::model::environment::EnvironmentId;
use golem_common::tracing::TraceOrigin;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::component::Component;
use golem_service_base::service::compiled_component::CompiledComponentService;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use golem_service_base::service::type_id_resolution::InvalidationGenerations;
use golem_service_base::storage::blob::BlobStorage;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::task::spawn_blocking;
use tracing::{Instrument, debug, info_span, warn};
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
    // If no version is provided, only the current deployed non-deleted version is returned.
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

    /// Invalidates cached "current deployed" component metadata.
    async fn invalidate_current_deployed_metadata(&self) {}

    /// Invalidates cached "current deployed" component metadata for a specific environment.
    async fn invalidate_current_deployed_metadata_for_environment(
        &self,
        _environment_id: EnvironmentId,
    ) {
        self.invalidate_current_deployed_metadata().await
    }

    /// Invalidates all cached component metadata (both current-deployed and version-pinned)
    /// for a specific environment.
    async fn invalidate_all_metadata_for_environment(&self, environment_id: EnvironmentId);

    /// Invalidates all cached component metadata.
    async fn invalidate_all(&self) {
        self.invalidate_current_deployed_metadata().await
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
        cache_config.max_concurrent_compilations,
        compiled_component_service,
    ))
}

#[derive(Clone)]
pub struct ComponentServiceDefault {
    component_cache: LoadedComponentCache,
    component_metadata_cache: Cache<ComponentKey, (), Component, WorkerExecutorError>,
    current_component_metadata_cache:
        Cache<ComponentId, (), FetchedCurrentMetadata, WorkerExecutorError>,
    current_metadata_generations: Arc<InvalidationGenerations>,
    compiled_component_service: Arc<dyn CompiledComponentService>,
    registry_client: Arc<dyn RegistryService>,
    compilation_limiter: CompilationLimiter,
}

#[derive(Clone)]
struct FetchedCurrentMetadata {
    generation: u64,
    metadata: Component,
}

impl ComponentServiceDefault {
    pub fn new(
        registry_client: Arc<dyn RegistryService>,
        max_component_capacity: usize,
        max_metadata_capacity: usize,
        time_to_idle: Duration,
        max_concurrent_compilations: usize,
        compiled_component_service: Arc<dyn CompiledComponentService>,
    ) -> Self {
        Self {
            registry_client,
            component_cache: create_component_cache(max_component_capacity, time_to_idle),
            component_metadata_cache: create_component_metadata_cache(
                max_metadata_capacity,
                time_to_idle,
            ),
            current_component_metadata_cache: create_current_component_metadata_cache(
                max_metadata_capacity,
                time_to_idle,
            ),
            current_metadata_generations: Arc::default(),
            compiled_component_service,
            compilation_limiter: CompilationLimiter::new(max_concurrent_compilations),
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl ComponentService for ComponentServiceDefault {
    async fn get(
        &self,
        engine: &Engine,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        let metadata = self
            .get_metadata(component_id, Some(component_revision))
            .await?;
        let wasm_hash = metadata.wasm_hash;
        let key = LoadedComponentKey(wasm_hash, engine.clone());
        let revision_key = ComponentKey {
            component_id,
            component_revision,
        };

        let service = self.clone();
        let load_engine = engine.clone();
        let load_metadata = metadata.clone();
        let load_revision = revision_key.clone();
        let origin = TraceOrigin::capture_current();
        let result = self
            .component_cache
            .get_or_insert_simple_spawned(&key, move || {
                let span = golem_common::related_span!(origin, tracing::Level::INFO, "Load shared component", %wasm_hash);
                async move {
                    service
                        .load_component(&load_engine, &load_metadata)
                        .await
                        .map_err(|error| (load_revision, error))
                }.instrument(span)
            })
            .await;
        let component = match result {
            Ok(component) => component,
            Err((owner, error)) if owner == revision_key => return Err(error),
            // A download failure for another revision must not prevent this one from loading.
            Err(_) => self.load_component(engine, &metadata).await?,
        };
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
                                    .map_err(|error| {
                                        registry_error(
                                            RegistryCall::Metadata,
                                            component_id,
                                            Some(component_revision),
                                            error,
                                        )
                                    })?;
                                Ok(metadata)
                            })
                        },
                    )
                    .await
            }
            None => loop {
                let client = self.registry_client.clone();
                let generations = self.current_metadata_generations.clone();
                let fetched = self
                    .current_component_metadata_cache
                    .get_or_insert_simple_spawned(&component_id, move || async move {
                        let generation = generations.current();
                        let metadata = client
                            .get_deployed_component_metadata(component_id)
                            .await
                            .map_err(|error| {
                            registry_error(RegistryCall::Metadata, component_id, None, error)
                        })?;
                        Ok(FetchedCurrentMetadata {
                            generation,
                            metadata,
                        })
                    })
                    .await?;

                if self
                    .current_metadata_generations
                    .is_current(fetched.metadata.environment_id, fetched.generation)
                {
                    self.component_metadata_cache
                        .get_or_insert_simple(
                            &ComponentKey {
                                component_id,
                                component_revision: fetched.metadata.revision,
                            },
                            || {
                                let metadata = fetched.metadata.clone();
                                Box::pin(async move { Ok(metadata) })
                            },
                        )
                        .await?;

                    // Invalidation can run while the revision-keyed cache is populated.
                    if self
                        .current_metadata_generations
                        .is_current(fetched.metadata.environment_id, fetched.generation)
                    {
                        return Ok(fetched.metadata);
                    }
                }
                self.current_component_metadata_cache
                    .remove_if_cached(&component_id, |cached| {
                        cached.generation == fetched.generation
                    })
                    .await;
            },
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

    async fn invalidate_current_deployed_metadata(&self) {
        let generation = self.current_metadata_generations.invalidate_all();
        for (key, _) in self.current_component_metadata_cache.iter().await {
            self.current_component_metadata_cache
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
    }

    async fn invalidate_current_deployed_metadata_for_environment(
        &self,
        environment_id: EnvironmentId,
    ) {
        let generation = self.current_metadata_generations.invalidate(environment_id);
        for (key, fetched) in self.current_component_metadata_cache.iter().await {
            if fetched.metadata.environment_id == environment_id {
                self.current_component_metadata_cache
                    .remove_if_cached(&key, |cached| {
                        cached.metadata.environment_id == environment_id
                            && cached.generation < generation
                    })
                    .await;
            }
        }
    }

    async fn invalidate_all_metadata_for_environment(&self, environment_id: EnvironmentId) {
        self.invalidate_current_deployed_metadata_for_environment(environment_id)
            .await;

        let keys = self
            .component_metadata_cache
            .iter()
            .await
            .into_iter()
            .filter_map(|(key, component)| {
                (component.environment_id == environment_id).then_some(key)
            })
            .collect::<Vec<_>>();

        for key in keys {
            self.component_metadata_cache.remove(&key).await;
        }
    }

    async fn invalidate_all(&self) {
        self.invalidate_current_deployed_metadata().await;

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

impl ComponentServiceDefault {
    async fn load_component(
        &self,
        engine: &Engine,
        metadata: &Component,
    ) -> Result<wasmtime::component::Component, WorkerExecutorError> {
        let Component {
            id: component_id,
            revision: component_revision,
            wasm_hash,
            environment_id,
            ..
        } = *metadata;
        match self
            .compiled_component_service
            .get(wasm_hash, component_id, component_revision, engine)
            .await
        {
            Ok(Some(component)) => return Ok(component),
            Ok(None) => (),
            Err(error) => {
                warn!(%component_id, %component_revision, %error, "Failed to download compiled component")
            }
        }

        // Bound the local compile fallback working set when both caches miss.
        self.compilation_limiter.run(async {
            let bytes = self.registry_client.download_component(component_id, component_revision).await
                .map_err(|error| registry_error(RegistryCall::DownloadComponent, component_id, Some(component_revision), error))?;
            let start = Instant::now();
            let span = info_span!("Loading WASM component");
            let engine = engine.clone();
            let component = spawn_blocking(move || {
                let _enter = span.enter();
                if blake3::hash(&bytes) != *wasm_hash.as_blake3_hash() {
                    return Err(WorkerExecutorError::component_download_failed(component_id, component_revision, "Wasm hash mismatch".to_string()));
                }
                wasmtime::component::Component::from_binary(&engine, &bytes)
                    .map_err(|error| WorkerExecutorError::ComponentParseFailed { component_id, component_revision, reason: error.to_string() })
            }).await.map_err(compilation_join_error)??;
            let compilation_time = start.elapsed();
            record_compilation_time(compilation_time);
            debug!(%component_id, compilation_time_ms = compilation_time.as_millis(), "Compiled component");

            if let Err(error) = self.compiled_component_service.put(environment_id, wasm_hash, component_id, component_revision, &component).await {
                warn!(%component_id, %component_revision, %error, "Failed to upload compiled component");
            }
            Ok(component)
        }).await
    }
}

// Loaded components belong to an engine instance, not just a compatible engine configuration.
#[derive(Clone, Debug)]
struct LoadedComponentKey(Hash, Engine);

type LoadedComponentCache = Cache<
    LoadedComponentKey,
    (),
    wasmtime::component::Component,
    (ComponentKey, WorkerExecutorError),
>;

impl PartialEq for LoadedComponentKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && Engine::same(&self.1, &other.1)
    }
}

impl Eq for LoadedComponentKey {}

impl std::hash::Hash for LoadedComponentKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.0, state);
    }
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

fn create_current_component_metadata_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentId, (), FetchedCurrentMetadata, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "current_component_metadata",
    )
}

fn create_component_cache(max_capacity: usize, time_to_idle: Duration) -> LoadedComponentCache {
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

/// The call to the component service that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegistryCall {
    /// The metadata of a component revision.
    Metadata,
    /// The binary of a component revision.
    DownloadComponent,
}

/// Metadata-detectable restrictions checked before reserving memory or loading a target.
/// Wasmtime still validates features that are not represented in component metadata.
pub(crate) fn component_support_error(metadata: &ComponentMetadata) -> Option<&'static str> {
    if metadata.has_shared_linear_memory() {
        Some("The target revision uses WebAssembly threads, which Golem does not support")
    } else {
        None
    }
}

pub(crate) fn compilation_join_error(error: tokio::task::JoinError) -> WorkerExecutorError {
    if error.is_cancelled() {
        WorkerExecutorError::Interrupted {
            kind: golem_service_base::error::worker_executor::InterruptKind::Restart,
        }
    } else {
        WorkerExecutorError::runtime(format!("Component compilation task failed: {error}"))
    }
}

/// The executor error of a failed component service call. It keeps the kind of the failure: the
/// service did not answer (`ComponentServiceUnavailable`, a later call can succeed), the revision
/// does not exist (`ComponentNotFound`), or the service refused the call
/// (`ComponentServiceRefused`). A revision without a binary is a refusal of the download.
fn registry_error(
    call: RegistryCall,
    component_id: ComponentId,
    component_revision: Option<ComponentRevision>,
    error: RegistryServiceError,
) -> WorkerExecutorError {
    use golem_service_base::error::worker_executor::ComponentServiceRefusal;
    let refused = |kind| WorkerExecutorError::ComponentServiceRefused {
        component_id,
        component_revision,
        kind,
        reason: error.to_safe_string(),
    };
    match (&error, call) {
        (
            RegistryServiceError::InternalClientError(_)
            | RegistryServiceError::InternalServerError(_),
            _,
        ) => WorkerExecutorError::ComponentServiceUnavailable {
            component_id,
            component_revision,
            reason: error.to_safe_string(),
        },
        (RegistryServiceError::NotFound(_), RegistryCall::Metadata) => {
            WorkerExecutorError::ComponentNotFound { component_id }
        }
        (RegistryServiceError::NotFound(_), RegistryCall::DownloadComponent) => {
            refused(ComponentServiceRefusal::MissingBinary)
        }
        (RegistryServiceError::Unauthorized(_), _) => {
            refused(ComponentServiceRefusal::Unauthorized)
        }
        (RegistryServiceError::CouldNotAuthenticate(_), _) => {
            refused(ComponentServiceRefusal::CouldNotAuthenticate)
        }
        (RegistryServiceError::BadRequest(_), _) => refused(ComponentServiceRefusal::BadRequest),
        (RegistryServiceError::LimitExceeded(_), _) => {
            refused(ComponentServiceRefusal::LimitExceeded)
        }
        (
            RegistryServiceError::AlreadyExists(_) | RegistryServiceError::RuntimeCardRevoked(_),
            _,
        ) => refused(ComponentServiceRefusal::Other),
    }
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
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::model::component::Component;
    use golem_service_base::model::environment::EnvironmentState;
    use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
    use golem_service_base::service::compiled_component::CompiledComponentServiceDisabled;
    use golem_service_base::service::compiled_component::DefaultCompiledComponentService;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use test_r::{test, timeout};
    use tokio::sync::Notify;

    #[test]
    fn component_support_validation_distinguishes_shared_and_unshared_memory() {
        for (memory, expected) in [
            ("(memory 1 1)", None),
            (
                "(memory 1 1 shared)",
                Some("The target revision uses WebAssembly threads, which Golem does not support"),
            ),
        ] {
            let bytes = wat::parse_str(format!("(component (core module {memory}))")).unwrap();
            let metadata = ComponentMetadata::analyse_component(
                &bytes,
                Vec::new(),
                Default::default(),
                Default::default(),
                Default::default(),
            )
            .unwrap();
            assert_eq!(component_support_error(&metadata), expected);
        }
    }

    #[derive(Default)]
    struct MetadataLookupGate {
        started: Notify,
        release: Notify,
    }

    struct MockRegistryService {
        components: Mutex<HashMap<(ComponentId, ComponentRevision), Component>>,
        deployed_revisions: Mutex<HashMap<ComponentId, ComponentRevision>>,
        deployed_lookup_gates: Mutex<HashMap<ComponentId, VecDeque<Arc<MetadataLookupGate>>>>,
        deployed_component_calls: Arc<std::sync::Mutex<HashMap<ComponentId, usize>>>,
        wasm: HashMap<ComponentId, Vec<u8>>,
        missing_wasm_gate: Option<(tokio::sync::Notify, tokio::sync::Notify)>,
    }

    impl MockRegistryService {
        fn new(components: impl IntoIterator<Item = Component>) -> Self {
            let components = components.into_iter().collect::<Vec<_>>();
            Self {
                deployed_revisions: Mutex::new(
                    components
                        .iter()
                        .map(|component| (component.id, component.revision))
                        .collect(),
                ),
                components: Mutex::new(
                    components
                        .into_iter()
                        .map(|component| ((component.id, component.revision), component))
                        .collect(),
                ),
                deployed_lookup_gates: Mutex::default(),
                deployed_component_calls: Arc::new(std::sync::Mutex::new(HashMap::new())),
                wasm: HashMap::new(),
                missing_wasm_gate: None,
            }
        }

        fn set_deployed_revision(&self, component_id: ComponentId, revision: u64) {
            let mut deployed = self.deployed_revisions.lock().unwrap();
            let previous = deployed[&component_id];
            let mut components = self.components.lock().unwrap();
            let mut component = components[&(component_id, previous)].clone();
            component.revision = ComponentRevision::new(revision).unwrap();
            deployed.insert(component_id, component.revision);
            components.insert((component_id, component.revision), component);
        }

        fn gate_deployed_lookup(&self, component_id: ComponentId) -> Arc<MetadataLookupGate> {
            let gate = Arc::new(MetadataLookupGate::default());
            self.deployed_lookup_gates
                .lock()
                .unwrap()
                .entry(component_id)
                .or_default()
                .push_back(gate.clone());
            gate
        }

        fn deployed_component_calls(&self) -> Arc<std::sync::Mutex<HashMap<ComponentId, usize>>> {
            self.deployed_component_calls.clone()
        }
    }

    #[async_trait]
    impl RegistryService for MockRegistryService {
        async fn resolve_mcp_import(
            &self,
            _: &golem_common::model::mcp_import::McpImportSource,
            _: &AuthCtx,
            _: bool,
        ) -> Result<golem_service_base::model::mcp_import::McpImportObservation, RegistryServiceError>
        {
            panic!("unexpected MCP discovery")
        }
        async fn get_mcp_runtime_credential(
            &self,
            _: &golem_common::model::mcp_import::McpImportSource,
            _: &AuthCtx,
        ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
        {
            panic!("unexpected MCP credential request")
        }
        async fn report_mcp_resource_unauthorized(
            &self,
            _: &golem_common::model::mcp_import::McpImportSource,
            _: &AuthCtx,
            _: Option<uuid::Uuid>,
        ) -> Result<(), RegistryServiceError> {
            panic!("unexpected MCP feedback")
        }
        async fn authenticate_token(
            &self,
            _token: &TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _account_id: golem_common::model::account::AccountId,
            _metering: golem_service_base::clients::registry::ResourceUsageMetering,
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
            component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            if let Some(wasm) = self.wasm.get(&component_id) {
                return Ok(wasm.clone());
            }
            let (entered, release) = self
                .missing_wasm_gate
                .as_ref()
                .expect("unexpected Wasm download");
            entered.notify_one();
            release.notified().await;
            Err(RegistryServiceError::NotFound("missing Wasm".into()))
        }

        async fn get_component_metadata(
            &self,
            component_id: ComponentId,
            component_revision: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            self.components
                .lock()
                .unwrap()
                .get(&(component_id, component_revision))
                .cloned()
                .ok_or_else(|| RegistryServiceError::NotFound("missing component".to_string()))
        }

        async fn get_deployed_component_metadata(
            &self,
            component_id: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            {
                let mut calls = self.deployed_component_calls.lock().unwrap();
                *calls.entry(component_id).or_default() += 1;
            }
            let component = {
                let deployed = self.deployed_revisions.lock().unwrap();
                let revision = deployed.get(&component_id).ok_or_else(|| {
                    RegistryServiceError::NotFound("missing component".to_string())
                })?;
                self.components
                    .lock()
                    .unwrap()
                    .get(&(component_id, *revision))
                    .cloned()
                    .unwrap()
            };
            let gate = self
                .deployed_lookup_gates
                .lock()
                .unwrap()
                .get_mut(&component_id)
                .and_then(VecDeque::pop_front);
            if let Some(gate) = gate {
                gate.started.notify_one();
                gate.release.notified().await;
            }
            Ok(component)
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

        async fn get_agent_secret_revision(
            &self,
            _environment_id: EnvironmentId,
            _agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
            _path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
            _revision: golem_common::model::agent_secret::AgentSecretRevision,
        ) -> Result<
            Option<golem_service_base::model::agent_secret::AgentSecret>,
            RegistryServiceError,
        > {
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
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
            application_name: ApplicationName::try_from("test-app".to_string()).unwrap(),
            environment_name: EnvironmentName::try_from("test-env").unwrap(),
            component_size: 1,
            metadata: golem_common::model::component_metadata::ComponentMetadata::default(),
            created_at: jiff::Timestamp::now(),
            wasm_hash: golem_common::model::diff::Hash::new(blake3::hash(b"wasm-hash")),
            package_metadata: None,
            object_store_key: "test-object".to_string(),
        }
    }

    #[test]
    async fn loads_shared_wasm_once_per_engine_and_keeps_revision_metadata() -> anyhow::Result<()> {
        let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
        let engine = Engine::new(&config)?;
        let other_engine = Engine::new(&config)?;
        let mut first = make_component(ComponentId::new(), EnvironmentId::new());
        let mut second = make_component(ComponentId::new(), EnvironmentId::new());
        let original = wat::parse_str("(component)")?;
        let different = wat::parse_str("(component (core module (memory 1)))")?;
        first.wasm_hash = blake3::hash(&original).into();
        second.wasm_hash = first.wasm_hash;
        let mut updated = first.clone();
        updated.revision = ComponentRevision::new(2).unwrap();
        updated.wasm_hash = blake3::hash(&different).into();
        let compiled = Arc::new(DefaultCompiledComponentService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        for (metadata, bytes) in [(&first, &original), (&updated, &different)] {
            let artifact = wasmtime::component::Component::from_binary(&engine, bytes)?;
            compiled
                .put(
                    metadata.environment_id,
                    metadata.wasm_hash,
                    metadata.id,
                    metadata.revision,
                    &artifact,
                )
                .await?;
        }
        // The registry mock does not implement Wasm downloads: a persistent cache miss fails the test.
        let registry = Arc::new(MockRegistryService::new([
            first.clone(),
            second.clone(),
            updated.clone(),
        ]));
        let service = ComponentServiceDefault::new(
            registry.clone(),
            8,
            8,
            Duration::from_secs(60),
            1,
            compiled.clone(),
        );
        let (first_loaded, _) = service.get(&engine, first.id, first.revision).await?;
        let (second_loaded, second_metadata) =
            service.get(&engine, second.id, second.revision).await?;
        assert!(wasmtime::component::Component::same(
            &first_loaded,
            &second_loaded
        ));
        assert_eq!(second_metadata.id, second.id);
        assert_eq!(second_metadata.environment_id, second.environment_id);
        let (updated_loaded, _) = service.get(&engine, updated.id, updated.revision).await?;
        assert!(!wasmtime::component::Component::same(
            &first_loaded,
            &updated_loaded
        ));
        let (still_original, _) = service.get(&engine, second.id, second.revision).await?;
        assert!(wasmtime::component::Component::same(
            &first_loaded,
            &still_original
        ));
        let (other_loaded, _) = service
            .get(&other_engine, second.id, second.revision)
            .await?;
        assert!(Engine::same(other_loaded.engine(), &other_engine));
        let fresh =
            ComponentServiceDefault::new(registry, 8, 8, Duration::from_secs(60), 1, compiled);
        fresh.get(&engine, second.id, second.revision).await?;
        Ok(())
    }

    #[test]
    async fn rejects_mismatched_wasm_without_populating_compiled_cache() -> anyhow::Result<()> {
        let engine =
            Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
        let mut metadata = make_component(ComponentId::new(), EnvironmentId::new());
        metadata.wasm_hash = blake3::hash(&wat::parse_str("(component)")?).into();
        let mut registry = MockRegistryService::new([metadata.clone()]);
        registry.wasm.insert(
            metadata.id,
            wat::parse_str("(component (core module (memory 1)))")?,
        );
        let compiled = Arc::new(DefaultCompiledComponentService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        let service = ComponentServiceDefault::new(
            Arc::new(registry),
            8,
            8,
            Duration::from_secs(60),
            1,
            compiled.clone(),
        );

        assert!(matches!(
            service.get(&engine, metadata.id, metadata.revision).await,
            Err(WorkerExecutorError::ComponentDownloadFailed { component_id, .. })
                if component_id == metadata.id
        ));
        assert!(!compiled.exists(metadata.wasm_hash, &engine).await?);
        Ok(())
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn shared_cache_miss_does_not_share_revision_download_errors() -> anyhow::Result<()> {
        let engine =
            Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
        let bytes = wat::parse_str("(component)")?;
        let mut missing = make_component(ComponentId::new(), EnvironmentId::new());
        let mut present = make_component(ComponentId::new(), EnvironmentId::new());
        missing.wasm_hash = blake3::hash(&bytes).into();
        present.wasm_hash = missing.wasm_hash;
        let mut registry = MockRegistryService::new([missing.clone(), present.clone()]);
        registry.wasm.insert(present.id, bytes);
        registry.missing_wasm_gate = Some(Default::default());
        let registry = Arc::new(registry);
        let (entered, release) = registry.missing_wasm_gate.as_ref().unwrap();
        let service = ComponentServiceDefault::new(
            registry.clone(),
            8,
            8,
            Duration::from_secs(60),
            1,
            Arc::new(CompiledComponentServiceDisabled::new()),
        );

        let miss = service.get(&engine, missing.id, missing.revision);
        let hit = async {
            entered.notified().await;
            let mut pending = std::pin::pin!(service.get(&engine, present.id, present.revision));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            release.notify_one();
            pending.await
        };
        let (miss, hit) = tokio::join!(miss, hit);
        assert!(
            matches!(miss, Err(WorkerExecutorError::ComponentServiceRefused { component_id, .. }) if component_id == missing.id)
        );
        assert_eq!(hit?.1.id, present.id);
        Ok(())
    }

    #[test]
    async fn caches_current_deployed_component_metadata_by_component_id() {
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
            0,
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
            "current deployed metadata should be fetched once per component id"
        );
    }

    #[test]
    async fn invalidating_current_deployed_component_metadata_forces_refresh() {
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
            0,
            Arc::new(CompiledComponentServiceDisabled::new()),
        );

        let _ = service.get_metadata(component_id, None).await.unwrap();
        service.invalidate_current_deployed_metadata().await;
        let _ = service.get_metadata(component_id, None).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.get(&component_id).copied().unwrap_or_default(),
            2,
            "invalidating current deployed metadata should force a fresh registry lookup"
        );
    }

    #[test]
    async fn invalidating_current_deployed_component_metadata_for_environment_only_evicts_matching_entries()
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
            0,
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
            .invalidate_current_deployed_metadata_for_environment(first_environment_id)
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

    #[test]
    #[timeout("10s")]
    async fn current_metadata_lookup_retries_a_fill_crossing_invalidation() {
        for global in [false, true] {
            let component_id = ComponentId::new();
            let environment_id = EnvironmentId::new();
            let registry = Arc::new(MockRegistryService::new([make_component(
                component_id,
                environment_id,
            )]));
            let gate = registry.gate_deployed_lookup(component_id);
            let service = Arc::new(ComponentServiceDefault::new(
                registry.clone(),
                1,
                8,
                Duration::from_secs(60),
                0,
                Arc::new(CompiledComponentServiceDisabled::new()),
            ));
            let lookup = tokio::spawn({
                let service = service.clone();
                async move { service.get_metadata(component_id, None).await.unwrap() }
            });
            gate.started.notified().await;
            registry.set_deployed_revision(component_id, 2);
            if global {
                service.invalidate_all().await;
            } else {
                service
                    .invalidate_current_deployed_metadata_for_environment(environment_id)
                    .await;
            }
            gate.release.notify_one();

            assert_eq!(lookup.await.unwrap().revision.get(), 2);
            assert_eq!(
                service
                    .get_metadata(component_id, None)
                    .await
                    .unwrap()
                    .revision
                    .get(),
                2
            );
            assert_eq!(
                service
                    .get_metadata(component_id, Some(ComponentRevision::new(1).unwrap()))
                    .await
                    .unwrap()
                    .revision
                    .get(),
                1
            );
            assert_eq!(
                registry.deployed_component_calls.lock().unwrap()[&component_id],
                2
            );
        }
    }

    #[test]
    #[timeout("10s")]
    async fn current_metadata_lookup_retries_successive_invalidations() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new([make_component(
            component_id,
            environment_id,
        )]));
        let first = registry.gate_deployed_lookup(component_id);
        let second = registry.gate_deployed_lookup(component_id);
        let service = Arc::new(ComponentServiceDefault::new(
            registry.clone(),
            1,
            8,
            Duration::from_secs(60),
            0,
            Arc::new(CompiledComponentServiceDisabled::new()),
        ));
        let lookup = tokio::spawn({
            let service = service.clone();
            async move { service.get_metadata(component_id, None).await.unwrap() }
        });
        for (gate, revision) in [(first, 2), (second, 3)] {
            gate.started.notified().await;
            registry.set_deployed_revision(component_id, revision);
            service
                .invalidate_current_deployed_metadata_for_environment(environment_id)
                .await;
            gate.release.notify_one();
        }
        assert_eq!(lookup.await.unwrap().revision.get(), 3);
        assert_eq!(
            service
                .get_metadata(component_id, None)
                .await
                .unwrap()
                .revision
                .get(),
            3
        );
        assert_eq!(
            registry.deployed_component_calls.lock().unwrap()[&component_id],
            3
        );
    }

    #[test]
    #[timeout("10s")]
    async fn current_metadata_lookup_ignores_other_environment_invalidations() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new([make_component(
            component_id,
            environment_id,
        )]));
        let gate = registry.gate_deployed_lookup(component_id);
        let service = Arc::new(ComponentServiceDefault::new(
            registry.clone(),
            1,
            8,
            Duration::from_secs(60),
            0,
            Arc::new(CompiledComponentServiceDisabled::new()),
        ));
        let lookup = tokio::spawn({
            let service = service.clone();
            async move { service.get_metadata(component_id, None).await.unwrap() }
        });
        gate.started.notified().await;
        registry.set_deployed_revision(component_id, 2);
        service
            .invalidate_current_deployed_metadata_for_environment(EnvironmentId::new())
            .await;
        gate.release.notify_one();
        assert_eq!(lookup.await.unwrap().revision.get(), 1);
        assert_eq!(
            service
                .get_metadata(component_id, None)
                .await
                .unwrap()
                .revision
                .get(),
            1
        );
        assert_eq!(
            registry.deployed_component_calls.lock().unwrap()[&component_id],
            1
        );
    }

    #[test]
    #[timeout("10s")]
    async fn current_metadata_lookup_recovers_after_cancellation_and_invalidation() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new([make_component(
            component_id,
            environment_id,
        )]));
        let gate = registry.gate_deployed_lookup(component_id);
        let service = Arc::new(ComponentServiceDefault::new(
            registry.clone(),
            1,
            8,
            Duration::from_secs(60),
            0,
            Arc::new(CompiledComponentServiceDisabled::new()),
        ));
        let lookup = tokio::spawn({
            let service = service.clone();
            async move { service.get_metadata(component_id, None).await.unwrap() }
        });
        gate.started.notified().await;
        lookup.abort();
        assert!(lookup.await.unwrap_err().is_cancelled());
        registry.set_deployed_revision(component_id, 2);
        service
            .invalidate_current_deployed_metadata_for_environment(environment_id)
            .await;
        gate.release.notify_one();
        assert_eq!(
            service
                .get_metadata(component_id, None)
                .await
                .unwrap()
                .revision
                .get(),
            2
        );
        assert_eq!(
            registry.deployed_component_calls.lock().unwrap()[&component_id],
            2
        );
    }

    /// Each kind of component service failure keeps its kind for the metadata call and for the
    /// download of the binary.
    #[test]
    fn a_registry_error_keeps_its_kind_for_each_call() {
        use golem_service_base::error::worker_executor::ComponentServiceRefusal;
        let component_id = ComponentId::new();
        let revision = Some(ComponentRevision::new(3).unwrap());
        let unavailable = |reason: &str| WorkerExecutorError::ComponentServiceUnavailable {
            component_id,
            component_revision: revision,
            reason: reason.to_string(),
        };
        let refused = |kind, reason: &str| WorkerExecutorError::ComponentServiceRefused {
            component_id,
            component_revision: revision,
            kind,
            reason: reason.to_string(),
        };
        let errors = || {
            [
                RegistryServiceError::InternalClientError("transport".to_string()),
                RegistryServiceError::InternalServerError("server".to_string()),
                RegistryServiceError::NotFound("missing".to_string()),
                RegistryServiceError::Unauthorized("token".to_string()),
                RegistryServiceError::CouldNotAuthenticate("token".to_string()),
                RegistryServiceError::BadRequest(vec!["bad".to_string()]),
                RegistryServiceError::LimitExceeded("limit".to_string()),
                RegistryServiceError::AlreadyExists("exists".to_string()),
            ]
        };
        let reasons = errors().map(|error| error.to_safe_string());
        let shared = |not_found| {
            [
                unavailable(&reasons[0]),
                unavailable(&reasons[1]),
                not_found,
                refused(ComponentServiceRefusal::Unauthorized, &reasons[3]),
                refused(ComponentServiceRefusal::CouldNotAuthenticate, &reasons[4]),
                refused(ComponentServiceRefusal::BadRequest, &reasons[5]),
                refused(ComponentServiceRefusal::LimitExceeded, &reasons[6]),
                refused(ComponentServiceRefusal::Other, &reasons[7]),
            ]
        };

        assert_eq!(
            errors().map(|error| registry_error(
                RegistryCall::Metadata,
                component_id,
                revision,
                error
            )),
            shared(WorkerExecutorError::ComponentNotFound { component_id })
        );
        assert_eq!(
            errors().map(|error| registry_error(
                RegistryCall::DownloadComponent,
                component_id,
                revision,
                error
            )),
            shared(refused(ComponentServiceRefusal::MissingBinary, &reasons[2]))
        );
    }
}
