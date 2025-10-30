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

use crate::services::component::ComponentService;
use crate::services::golem_config::AgentTypesServiceConfig;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::agent::RegisteredAgentType;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::clients::registry::GrpcRegistryService;
use golem_service_base::clients::RemoteServiceConfig;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait AgentTypesService: Send + Sync {
    async fn get_all(
        &self,
        owner_environment: &EnvironmentId,
    ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError>;
    async fn get(
        &self,
        owner_environment: &EnvironmentId,
        name: &str,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError>;
}

pub fn configured(
    config: &AgentTypesServiceConfig,
    component_service: Arc<dyn ComponentService>,
) -> Arc<dyn AgentTypesService> {
    match config {
        AgentTypesServiceConfig::Grpc(config) => {
            let client = CachedAgentTypes::new(
                Arc::new(self::grpc::RemoteAgentTypesService::new(Arc::new(
                    GrpcRegistryService::new(&RemoteServiceConfig {
                        host: config.host.clone(),
                        port: config.port,
                        retries: config.retries.clone(),
                    }),
                ))),
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
    cached_registered_agent_types:
        Cache<(EnvironmentId, String), (), RegisteredAgentType, Option<WorkerExecutorError>>,
}

impl CachedAgentTypes {
    pub fn new(inner: Arc<dyn AgentTypesService>, cache_time_to_idle: std::time::Duration) -> Self {
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
        owner_environment: &EnvironmentId,
    ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError> {
        // Full agent discovery is not cached
        self.inner.get_all(owner_environment).await
    }

    async fn get(
        &self,
        owner_environment: &EnvironmentId,
        name: &str,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
        // Getting a particular agent type is cached with a short TTL because
        // it is used in RPC to find the invocation target
        let key = (owner_environment.clone(), name.to_string());
        let result = self
            .cached_registered_agent_types
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    match self.inner.get(owner_environment, name).await {
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
}

mod grpc {
    use crate::services::agent_types::AgentTypesService;
    use async_trait::async_trait;

    use golem_common::model::agent::RegisteredAgentType;
    use golem_common::model::environment::EnvironmentId;

    use golem_service_base::error::worker_executor::WorkerExecutorError;

    use golem_service_base::clients::registry::RegistryService;
    use golem_service_base::model::auth::AuthCtx;
    use std::sync::Arc;

    #[derive(Clone)]
    pub struct RemoteAgentTypesService {
        client: Arc<dyn RegistryService>,
    }

    impl RemoteAgentTypesService {
        pub fn new(client: Arc<dyn RegistryService>) -> Self {
            Self { client }
        }
    }

    #[async_trait]
    impl AgentTypesService for RemoteAgentTypesService {
        async fn get_all(
            &self,
            owner_environment: &EnvironmentId,
        ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError> {
            self.client
                .get_all_agent_types(owner_environment, &AuthCtx::System)
                .await
                .map_err(|e| {
                    WorkerExecutorError::runtime(format!("Failed to get agent types: {e}"))
                })
        }

        async fn get(
            &self,
            owner_environment: &EnvironmentId,
            name: &str,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            self.client
                .get_agent_type(owner_environment, name, &AuthCtx::System)
                .await
                .map_err(|e| WorkerExecutorError::runtime(format!("Failed to get agent type: {e}")))
        }
    }
}

mod local {
    use crate::services::agent_types::AgentTypesService;
    use crate::services::component::ComponentService;
    use async_trait::async_trait;
    use golem_common::model::agent::RegisteredAgentType;
    use golem_common::model::environment::EnvironmentId;
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
            owner_environment: &EnvironmentId,
        ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError> {
            Ok(self
                .component_service
                .all_cached_metadata()
                .await
                .iter()
                .filter(|component| component.environment_id == *owner_environment)
                .flat_map(|component| {
                    component
                        .metadata
                        .agent_types()
                        .iter()
                        .map(|agent_type| RegisteredAgentType {
                            agent_type: agent_type.clone(),
                            implemented_by: component.id.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect())
        }

        async fn get(
            &self,
            owner_environment: &EnvironmentId,
            name: &str,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            Ok(self
                .get_all(owner_environment)
                .await?
                .iter()
                .find(|r| r.agent_type.type_name == name)
                .cloned())
        }
    }
}
