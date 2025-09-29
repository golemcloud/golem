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
use golem_common::model::ProjectId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[async_trait]
pub trait AgentTypesService: Send + Sync {
    async fn get_all(
        &self,
        owner_project: &ProjectId,
    ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError>;
    async fn get(
        &self,
        owner_project: &ProjectId,
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
                Arc::new(self::grpc::AgentTypesServiceGrpc::new(
                    config.uri(),
                    config
                        .access_token
                        .parse::<Uuid>()
                        .expect("Access token must be an UUID"),
                    config.retries.clone(),
                    config.connect_timeout,
                )),
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
        Cache<(ProjectId, String), (), RegisteredAgentType, Option<WorkerExecutorError>>,
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
        owner_project: &ProjectId,
    ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError> {
        // Full agent discovery is not cached
        self.inner.get_all(owner_project).await
    }

    async fn get(
        &self,
        owner_project: &ProjectId,
        name: &str,
    ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
        // Getting a particular agent type is cached with a short TTL because
        // it is used in RPC to find the invocation target
        let key = (owner_project.clone(), name.to_string());
        let result = self
            .cached_registered_agent_types
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    match self.inner.get(owner_project, name).await {
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
    use crate::grpc::authorised_grpc_request;
    use crate::services::agent_types::AgentTypesService;
    use async_trait::async_trait;
    use golem_api_grpc::proto::golem::component::v1::agent_types_service_client::AgentTypesServiceClient;
    use golem_api_grpc::proto::golem::component::v1::{
        component_error, get_all_response, get_response, ComponentError, GetAllRequest,
        GetAllSuccessResponse, GetRequest,
    };
    use golem_common::client::{GrpcClient, GrpcClientConfig};
    use golem_common::model::agent::RegisteredAgentType;
    use golem_common::model::{ProjectId, RetryConfig};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use http::Uri;
    use std::time::Duration;
    use tonic::codec::CompressionEncoding;
    use tonic::transport::Channel;
    use uuid::Uuid;

    #[derive(Clone)]
    pub struct AgentTypesServiceGrpc {
        agent_types_client: GrpcClient<AgentTypesServiceClient<Channel>>,
        access_token: Uuid,
    }

    impl AgentTypesServiceGrpc {
        pub fn new(
            endpoint: Uri,
            access_token: Uuid,
            retry_config: RetryConfig,
            connect_timeout: Duration,
        ) -> Self {
            Self {
                agent_types_client: GrpcClient::new(
                    "agent types service",
                    move |channel| {
                        AgentTypesServiceClient::new(channel)
                            .send_compressed(CompressionEncoding::Gzip)
                            .accept_compressed(CompressionEncoding::Gzip)
                    },
                    endpoint.clone(),
                    GrpcClientConfig {
                        retries_on_unavailable: retry_config.clone(),
                        connect_timeout,
                    },
                ),

                access_token,
            }
        }
    }

    #[async_trait]
    impl AgentTypesService for AgentTypesServiceGrpc {
        async fn get_all(
            &self,
            owner_project: &ProjectId,
        ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError> {
            let response = self
                .agent_types_client
                .call("get_all_agent_types", move |client| {
                    let request = authorised_grpc_request(
                        GetAllRequest {
                            project_id: Some(owner_project.clone().into()),
                        },
                        &self.access_token,
                    );
                    Box::pin(client.get_all(request))
                })
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!("Failed to get agent types: {err:?}"))
                })?
                .into_inner();

            match response.result {
                None => Err(WorkerExecutorError::runtime("Empty response")),
                Some(get_all_response::Result::Success(GetAllSuccessResponse { agent_types })) => {
                    Ok(agent_types
                        .into_iter()
                        .map(|agent_type| agent_type.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "Unexpected protobuf message format for RegisteredAgentType: {err:?}"
                            ))
                        })?)
                }
                Some(get_all_response::Result::Error(err)) => Err(WorkerExecutorError::runtime(
                    format!("Failed to get agent types: {err:?}"),
                )),
            }
        }

        async fn get(
            &self,
            owner_project: &ProjectId,
            name: &str,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            let response = self
                .agent_types_client
                .call("get_agent_type", move |client| {
                    let request = authorised_grpc_request(
                        GetRequest {
                            project_id: Some(owner_project.clone().into()),
                            agent_type: name.to_string(),
                        },
                        &self.access_token,
                    );
                    Box::pin(client.get(request))
                })
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!("Failed to get agent types: {err:?}"))
                })?
                .into_inner();

            match response.result {
                None => Err(WorkerExecutorError::runtime("Empty response")),
                Some(get_response::Result::Success(agent_type)) => {
                    Ok(Some(agent_type.try_into().map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "Unexpected protobuf message format for RegisteredAgentType: {err:?}"
                        ))
                    })?))
                }
                Some(get_response::Result::Error(ComponentError {
                    error: Some(component_error::Error::NotFound(_)),
                })) => Ok(None),
                Some(get_response::Result::Error(err)) => Err(WorkerExecutorError::runtime(
                    format!("Failed to get agent type {name}: {err:?}"),
                )),
            }
        }
    }
}

mod local {
    use crate::services::agent_types::AgentTypesService;
    use crate::services::component::ComponentService;
    use async_trait::async_trait;
    use golem_common::base_model::ProjectId;
    use golem_common::model::agent::RegisteredAgentType;
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
            owner_project: &ProjectId,
        ) -> Result<Vec<RegisteredAgentType>, WorkerExecutorError> {
            Ok(self
                .component_service
                .all_cached_metadata()
                .await
                .iter()
                .filter(|component| &component.owner.project_id == owner_project)
                .flat_map(|component| {
                    component
                        .metadata
                        .native_agent_types()
                        .iter()
                        .map(|agent_type| RegisteredAgentType {
                            agent_type: agent_type.clone(),
                            implemented_by: component.versioned_component_id.component_id.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect())
        }

        async fn get(
            &self,
            owner_project: &ProjectId,
            name: &str,
        ) -> Result<Option<RegisteredAgentType>, WorkerExecutorError> {
            Ok(self
                .get_all(owner_project)
                .await?
                .iter()
                .find(|r| r.agent_type.type_name == name)
                .cloned())
        }
    }
}
