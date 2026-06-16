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
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::agent::RegisteredAgentTypeSchema;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
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

    async fn get(
        &self,
        owner_environment: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        name: &AgentTypeName,
    ) -> Result<Option<RegisteredAgentTypeSchema>, WorkerExecutorError>;

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
        RegisteredAgentTypeSchema,
        Option<WorkerExecutorError>,
    >,
}

impl CachedAgentTypes {
    pub fn new(inner: Arc<dyn AgentTypesService>, cache_time_to_idle: Duration) -> Self {
        Self {
            inner,
            cached_registered_agent_types: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::OlderThan {
                    ttl: cache_time_to_idle,
                    period: Duration::from_secs(2),
                },
                "agent types",
            ),
        }
    }
}

#[async_trait]
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
    ) -> Result<Option<RegisteredAgentTypeSchema>, WorkerExecutorError> {
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

    async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        let keys = self.cached_registered_agent_types.keys().await;
        for key in keys
            .into_iter()
            .filter(|(env_id, _, _, _)| *env_id == environment_id)
        {
            self.cached_registered_agent_types.remove(&key).await;
        }
    }

    async fn invalidate_all(&self) {
        let keys = self.cached_registered_agent_types.keys().await;
        for key in keys {
            self.cached_registered_agent_types.remove(&key).await;
        }
    }
}

mod grpc {
    use crate::services::agent_types::AgentTypesService;
    use async_trait::async_trait;
    use golem_common::SafeDisplay;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::schema::adapters::agent::agent_type_to_schema;
    use golem_common::schema::agent::RegisteredAgentTypeSchema;
    use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
    use golem_service_base::error::worker_executor::WorkerExecutorError;

    use golem_common::model::component::{ComponentId, ComponentRevision};
    use std::sync::Arc;

    /// Up-convert a legacy [`RegisteredAgentType`] (decoded from the legacy
    /// proto registry surface) into the schema-native model. This direction is
    /// lossless: every legacy `AnalysedType` has a schema counterpart.
    fn registered_agent_type_to_schema(
        registered: RegisteredAgentType,
    ) -> Result<RegisteredAgentTypeSchema, WorkerExecutorError> {
        let agent_type = agent_type_to_schema(&registered.agent_type).map_err(|err| {
            WorkerExecutorError::runtime(format!("Invalid agent metadata: {err}"))
        })?;
        Ok(RegisteredAgentTypeSchema {
            agent_type,
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
        ) -> Result<Option<RegisteredAgentTypeSchema>, WorkerExecutorError> {
            let result = self
                .client
                .get_agent_type(owner_environment, component_id, component_revision, name)
                .await;

            match result {
                Ok(agent_type) => Ok(Some(registered_agent_type_to_schema(agent_type)?)),
                Err(RegistryServiceError::NotFound(_)) => Ok(None),
                Err(other) => Err(WorkerExecutorError::runtime(format!(
                    "Failed to get agent type: {}",
                    other.to_safe_string()
                ))),
            }
        }
    }
}

mod local {
    use crate::services::agent_types::AgentTypesService;
    use crate::services::component::ComponentService;
    use async_trait::async_trait;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentTypeImplementer};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::environment::EnvironmentId;
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
    }

    #[async_trait]
    impl AgentTypesService for AgentTypesServiceLocal {
        async fn get_all(
            &self,
            owner_environment: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentTypeSchema>, WorkerExecutorError> {
            // NOTE: we can't filter the component metadata by component revision because in local mode we don't have a concept of components deployed together

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
            Ok(result)
        }

        async fn get(
            &self,
            owner_environment: EnvironmentId,
            component_id: ComponentId,
            component_revision: ComponentRevision,
            name: &AgentTypeName,
        ) -> Result<Option<RegisteredAgentTypeSchema>, WorkerExecutorError> {
            Ok(self
                .get_all(owner_environment, component_id, component_revision)
                .await?
                .iter()
                .find(|r| &r.agent_type.type_name == name)
                .cloned())
        }
    }
}
