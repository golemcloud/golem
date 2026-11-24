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

use crate::services::component::ComponentService;
use crate::services::golem_config::AgentTypesServiceConfig;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::type_id::TypeId;
use golem_common::schema::agent::RegisteredAgentTypeSchema;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::service::type_id_resolution::InvalidationGenerations;
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait AgentTypesService: Send + Sync {
    async fn get_all(
        &self,
        owner_environment: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentTypeSchema>, WorkerExecutorError>;

    /// The agent type with its type ID, which keyed agent IDs derive from.
    async fn get(
        &self,
        owner_environment: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        name: &AgentTypeName,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError>;

    /// The deployed agent type that the type ID names in the environment, or `None` when it
    /// names no deployed agent type. An agent ID's type tag resolves through this.
    async fn get_by_type_id(
        &self,
        owner_environment: EnvironmentId,
        type_id: TypeId,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError>;

    async fn invalidate_environment(&self, _environment_id: EnvironmentId) {}
    async fn invalidate_all(&self) {}
}

pub fn configured(
    config: &AgentTypesServiceConfig,
    component_service: Arc<dyn ComponentService>,
    registry_service: Arc<dyn RegistryService>,
) -> Arc<dyn AgentTypesService> {
    match config {
        AgentTypesServiceConfig::Grpc(config) => {
            let client = CachedAgentTypes::new(
                Arc::new(grpc::AgentTypesServiceGrpc::new(registry_service)),
                config.cache_time_to_idle,
            );
            Arc::new(client)
        }
        AgentTypesServiceConfig::Local(_) => {
            Arc::new(local::AgentTypesServiceLocal::new(component_service))
        }
    }
}

struct CachedAgentTypes {
    inner: Arc<dyn AgentTypesService>,
    cached_registered_agent_types: Cache<
        (EnvironmentId, ComponentId, ComponentRevision, String),
        (),
        RegisteredAgentType,
        Option<WorkerExecutorError>,
    >,
    cached_agent_types_by_type_id:
        Cache<(EnvironmentId, TypeId), (), FetchedAgentType, WorkerExecutorError>,
    generations: Arc<InvalidationGenerations>,
}

#[derive(Clone)]
struct FetchedAgentType {
    generation: u64,
    agent_type: Option<RegisteredAgentType>,
}

impl CachedAgentTypes {
    pub fn new(inner: Arc<dyn AgentTypesService>, cache_time_to_idle: Duration) -> Self {
        Self {
            inner,
            generations: Arc::default(),
            cached_registered_agent_types: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::OlderThan {
                    ttl: cache_time_to_idle,
                    period: Duration::from_secs(2),
                },
                "agent types",
            ),
            cached_agent_types_by_type_id: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::OlderThan {
                    ttl: cache_time_to_idle,
                    period: Duration::from_secs(2),
                },
                "agent types by type id",
            ),
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl AgentTypesService for CachedAgentTypes {
    async fn get_all(
        &self,
        owner_environment: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentTypeSchema>, WorkerExecutorError> {
        // Full agent discovery is not cached
        self.inner
            .get_all(owner_environment, component_id, component_revision)
            .await
    }

    async fn get(
        &self,
        owner_environment: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        name: &AgentTypeName,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
        // Getting a particular agent type is cached with a short TTL because
        // it is used in RPC to find the invocation target
        let key = (
            owner_environment,
            component_id,
            component_revision,
            name.to_string(),
        );
        let result = self
            .cached_registered_agent_types
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    match self
                        .inner
                        .get(owner_environment, component_id, component_revision, name)
                        .await
                    {
                        Ok(Some(r)) => Ok(r),
                        Ok(None) => Err(None),
                        Err(err) => Err(Some(err)),
                    }
                })
            })
            .await;
        match result {
            Ok(result) => Ok(Some(result)),
            Err(None) => Ok(None),
            Err(Some(err)) => Err(err),
        }
    }

    async fn get_by_type_id(
        &self,
        owner_environment: EnvironmentId,
        type_id: TypeId,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
        let key = (owner_environment, type_id);
        loop {
            let inner = self.inner.clone();
            let generations = self.generations.clone();
            let fetched = self
                .cached_agent_types_by_type_id
                .get_or_insert_simple_spawned(&key, move || async move {
                    let generation = generations.current();
                    let agent_type = inner.get_by_type_id(owner_environment, type_id).await?;
                    Ok(FetchedAgentType {
                        generation,
                        agent_type,
                    })
                })
                .await?;
            if self
                .generations
                .is_current(owner_environment, fetched.generation)
            {
                // A type that has not been assigned can appear in a later deployment.
                if fetched.agent_type.is_none() {
                    self.cached_agent_types_by_type_id
                        .remove_if_cached(&key, |cached| cached.generation == fetched.generation)
                        .await;
                }
                return Ok(fetched.agent_type);
            }
            self.cached_agent_types_by_type_id
                .remove_if_cached(&key, |cached| cached.generation == fetched.generation)
                .await;
        }
    }

    async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        let generation = self.generations.invalidate(environment_id);
        let keys = self.cached_registered_agent_types.keys().await;
        for key in keys
            .into_iter()
            .filter(|(env_id, _, _, _)| *env_id == environment_id)
        {
            self.cached_registered_agent_types.remove(&key).await;
        }
        for (key, _) in self.cached_agent_types_by_type_id.iter().await {
            if key.0 == environment_id {
                self.cached_agent_types_by_type_id
                    .remove_if_cached(&key, |cached| cached.generation < generation)
                    .await;
            }
        }
    }

    async fn invalidate_all(&self) {
        let generation = self.generations.invalidate_all();
        let keys = self.cached_registered_agent_types.keys().await;
        for key in keys {
            self.cached_registered_agent_types.remove(&key).await;
        }
        for (key, _) in self.cached_agent_types_by_type_id.iter().await {
            self.cached_agent_types_by_type_id
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
    }
}

mod grpc {
    use crate::services::agent_types::AgentTypesService;
    use async_trait::async_trait;
    use golem_common::SafeDisplay;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::type_id::{TypeId, TypeIdKind};
    use golem_common::schema::agent::RegisteredAgentTypeSchema;
    use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
    use golem_service_base::error::worker_executor::WorkerExecutorError;

    use golem_common::model::component::{ComponentId, ComponentRevision};
    use std::sync::Arc;

    /// Project a [`RegisteredAgentType`] into the ID-free discovery model.
    fn registered_agent_type_to_schema(
        registered: RegisteredAgentType,
    ) -> Result<RegisteredAgentTypeSchema, WorkerExecutorError> {
        Ok(RegisteredAgentTypeSchema {
            agent_type: registered.agent_type,
            implemented_by: registered.implemented_by,
        })
    }

    #[derive(Clone)]
    pub struct AgentTypesServiceGrpc {
        client: Arc<dyn RegistryService>,
    }

    impl AgentTypesServiceGrpc {
        pub fn new(client: Arc<dyn RegistryService>) -> Self {
            Self { client }
        }
    }

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl AgentTypesService for AgentTypesServiceGrpc {
        async fn get_all(
            &self,
            owner_environment: EnvironmentId,
            component_id: ComponentId,
            component_revision: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentTypeSchema>, WorkerExecutorError> {
            self.client
                .get_all_agent_types(owner_environment, component_id, component_revision)
                .await
                .map_err(|e| {
                    WorkerExecutorError::runtime(format!("Failed to get agent types: {e}"))
                })?
                .into_iter()
                .map(registered_agent_type_to_schema)
                .collect()
        }

        async fn get(
            &self,
            owner_environment: EnvironmentId,
            component_id: ComponentId,
            component_revision: ComponentRevision,
            name: &AgentTypeName,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            let result = self
                .client
                .get_agent_type(owner_environment, component_id, component_revision, name)
                .await;

            match result {
                Ok(agent_type) => Ok(Some(agent_type)),
                Err(RegistryServiceError::NotFound(_)) => Ok(None),
                Err(other) => Err(WorkerExecutorError::runtime(format!(
                    "Failed to get agent type: {}",
                    other.to_safe_string()
                ))),
            }
        }

        async fn get_by_type_id(
            &self,
            owner_environment: EnvironmentId,
            type_id: TypeId,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            let resolved = self
                .client
                .resolve_type_id(owner_environment, type_id)
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to resolve type ID: {}",
                        err.to_safe_string()
                    ))
                })?;
            let Some((name, implementer)) = resolved
                .filter(|resolved| resolved.kind == TypeIdKind::AgentType)
                .and_then(|resolved| resolved.implemented_by.map(|by| (resolved.name, by)))
            else {
                return Ok(None);
            };
            let found = self
                .get(
                    owner_environment,
                    implementer.component_id,
                    implementer.component_revision,
                    &AgentTypeName(name),
                )
                .await?;
            Ok(found.filter(|found| found.type_id == type_id))
        }
    }
}

pub use local::local_type_id;

#[cfg(test)]
mod cache_tests {
    use super::*;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{RegisteredAgentTypeImplementer, Snapshotting};
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
    };
    use golem_schema::schema::SchemaGraph;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};
    use tokio::sync::Notify;

    struct GatedAgentTypes {
        value: Mutex<RegisteredAgentType>,
        calls: AtomicUsize,
        started: Notify,
        release: Notify,
    }

    #[async_trait]
    impl AgentTypesService for GatedAgentTypes {
        async fn get_all(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentTypeSchema>, WorkerExecutorError> {
            unreachable!()
        }
        async fn get(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
            _: &AgentTypeName,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            unreachable!()
        }
        async fn get_by_type_id(
            &self,
            _: EnvironmentId,
            _: TypeId,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            let value = self.value.lock().unwrap().clone();
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(Some(value))
        }
    }

    fn source() -> Arc<GatedAgentTypes> {
        Arc::new(GatedAgentTypes {
            value: Mutex::new(RegisteredAgentType {
                type_id: TypeId::try_from(7u32).unwrap(),
                agent_type: AgentTypeSchema {
                    type_name: AgentTypeName("Counter".into()),
                    kind: AgentTypeKind::Regular,
                    description: String::new(),
                    source_language: "rust".into(),
                    schema: SchemaGraph::empty(),
                    constructor: AgentConstructorSchema {
                        name: None,
                        description: String::new(),
                        prompt_hint: None,
                        input_schema: InputSchema::Parameters(vec![]),
                    },
                    methods: vec![],
                    dependencies: vec![],
                    mode: golem_common::model::agent::AgentMode::Durable,
                    http_mount: None,
                    snapshotting: Snapshotting::Disabled(golem_schema::Empty {}),
                    config: vec![],
                },
                implemented_by: RegisteredAgentTypeImplementer {
                    component_id: ComponentId::new(),
                    component_revision: ComponentRevision::INITIAL,
                    component_name: "A".into(),
                    account_id: AccountId::new(),
                    account_email: AccountEmail::new("owner@example.com"),
                },
            }),
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        })
    }

    #[test]
    #[timeout("10s")]
    async fn type_id_lookup_retries_an_invalidated_in_flight_result() {
        for invalidate_all in [false, true] {
            let source = source();
            let cache = Arc::new(CachedAgentTypes::new(
                source.clone(),
                Duration::from_secs(60),
            ));
            let env = EnvironmentId::new();
            let id = source.value.lock().unwrap().type_id;
            let first = tokio::spawn({
                let cache = cache.clone();
                async move { cache.get_by_type_id(env, id).await }
            });
            source.started.notified().await;
            source.value.lock().unwrap().implemented_by.component_name = "B".into();
            if invalidate_all {
                cache.invalidate_all().await;
            } else {
                cache.invalidate_environment(env).await;
            }
            source.release.notify_one();
            assert_eq!(
                first
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .implemented_by
                    .component_name,
                "B"
            );
            assert_eq!(
                cache
                    .get_by_type_id(env, id)
                    .await
                    .unwrap()
                    .unwrap()
                    .implemented_by
                    .component_name,
                "B"
            );
            assert_eq!(source.calls.load(Ordering::SeqCst), 2);
        }
    }

    #[test]
    #[timeout("10s")]
    async fn type_id_lookup_producer_survives_its_first_callers_cancellation() {
        let source = source();
        let cache = Arc::new(CachedAgentTypes::new(
            source.clone(),
            Duration::from_secs(60),
        ));
        let env = EnvironmentId::new();
        let id = source.value.lock().unwrap().type_id;
        let first = tokio::spawn({
            let cache = cache.clone();
            async move { cache.get_by_type_id(env, id).await }
        });
        source.started.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        source.release.notify_one();
        let actual = cache.get_by_type_id(env, id).await.unwrap().unwrap();
        assert_eq!(actual.implemented_by.component_name, "A");
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }
}

mod local {
    use crate::services::agent_types::AgentTypesService;
    use crate::services::component::ComponentService;
    use async_trait::async_trait;
    use golem_common::model::agent::{
        AgentTypeName, RegisteredAgentType, RegisteredAgentTypeImplementer,
    };
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::type_id::TypeId;
    use golem_common::schema::agent::RegisteredAgentTypeSchema;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use std::sync::Arc;

    pub struct AgentTypesServiceLocal {
        component_service: Arc<dyn ComponentService>,
    }

    impl AgentTypesServiceLocal {
        pub fn new(component_service: Arc<dyn ComponentService>) -> Self {
            Self { component_service }
        }

        /// Local mode has no deployments, so every agent type of every cached component of the
        /// environment counts as registered, whatever the component revision.
        async fn registered(
            &self,
            owner_environment: EnvironmentId,
        ) -> Vec<RegisteredAgentTypeSchema> {
            let mut result = Vec::new();
            for component in self
                .component_service
                .all_cached_metadata()
                .await
                .iter()
                .filter(|component| component.environment_id == owner_environment)
            {
                for agent_type in component.metadata.agent_types() {
                    result.push(RegisteredAgentTypeSchema {
                        agent_type: agent_type.clone(),
                        implemented_by: RegisteredAgentTypeImplementer {
                            component_id: component.id,
                            component_revision: component.revision,
                            component_name: component.component_name.0.clone(),
                            account_id: component.account_id,
                            account_email: component.account_email.clone(),
                        },
                    });
                }
            }
            result
        }
    }

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl AgentTypesService for AgentTypesServiceLocal {
        async fn get_all(
            &self,
            owner_environment: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentTypeSchema>, WorkerExecutorError> {
            Ok(self.registered(owner_environment).await)
        }

        async fn get(
            &self,
            owner_environment: EnvironmentId,
            component_id: ComponentId,
            component_revision: ComponentRevision,
            name: &AgentTypeName,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            Ok(self
                .get_all(owner_environment, component_id, component_revision)
                .await?
                .into_iter()
                .find(|r| &r.agent_type.type_name == name)
                .map(|r| RegisteredAgentType {
                    type_id: local_type_id(owner_environment, name),
                    agent_type: r.agent_type,
                    implemented_by: r.implemented_by,
                }))
        }

        async fn get_by_type_id(
            &self,
            owner_environment: EnvironmentId,
            type_id: TypeId,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            Ok(self
                .registered(owner_environment)
                .await
                .into_iter()
                .find(|r| local_type_id(owner_environment, &r.agent_type.type_name) == type_id)
                .map(|r| RegisteredAgentType {
                    type_id,
                    agent_type: r.agent_type,
                    implemented_by: r.implemented_by,
                }))
        }
    }

    /// Without a registry no environment assigns type IDs, so local mode derives a stable,
    /// non-zero one from the environment and the type name.
    pub fn local_type_id(environment_id: EnvironmentId, name: &AgentTypeName) -> TypeId {
        let hash = blake3::Hasher::new()
            .update(environment_id.0.as_bytes())
            .update(name.0.as_bytes())
            .finalize();
        let value = u32::from_be_bytes(hash.as_bytes()[..4].try_into().unwrap()).max(1);
        TypeId::try_from(value).expect("a non-zero type ID is valid")
    }

    #[cfg(test)]
    mod tests {
        use super::local_type_id;
        use golem_common::model::agent::AgentTypeName;
        use golem_common::model::environment::EnvironmentId;
        use test_r::test;

        #[test]
        fn local_type_ids_are_stable_and_per_environment() {
            let name = AgentTypeName("counter".to_string());
            let env = EnvironmentId::new();
            assert_eq!(local_type_id(env, &name), local_type_id(env, &name));
            assert_ne!(
                local_type_id(env, &name),
                local_type_id(EnvironmentId::new(), &name)
            );
        }
    }
}
