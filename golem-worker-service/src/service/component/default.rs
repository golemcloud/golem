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

use crate::service::component::ComponentServiceError;
use crate::service::with_metadata;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    create_component_constraints_response, delete_component_constraints_response,
    get_component_metadata_response, get_components_response, CreateComponentConstraintsRequest,
    CreateComponentConstraintsResponse, DeleteComponentConstraintsRequest,
    DeleteComponentConstraintsResponse, GetComponentMetadataResponse, GetComponentsRequest,
    GetComponentsResponse, GetLatestComponentRequest, GetVersionedComponentRequest,
};
use golem_api_grpc::proto::golem::component::ComponentConstraints;
use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::component::ComponentRevision;
use golem_service_base::model::auth::AuthCtx;
use golem_common::model::component_constraint::{
    FunctionConstraints, FunctionSignature, FunctionUsageConstraint,
};
use golem_common::model::component::ComponentId;
use golem_common::model::RetryConfig;
use golem_common::retries::with_retries;
use golem_common::model::component::{ComponentDto, ComponentName};
use http::Uri;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::component::ComponentRevision;

pub type ComponentResult<T> = Result<T, ComponentServiceError>;

#[async_trait]
pub trait ComponentService: Send + Sync {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<ComponentDto>;

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<ComponentDto>;

    async fn get_latest_by_name(
        &self,
        component_id: &ComponentName,
        environment_id: &EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<ComponentDto>;

    async fn get_all_by_name(
        &self,
        component_id: &ComponentName,
        environment_id: &EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<Vec<ComponentDto>>;

    async fn create_or_update_constraints(
        &self,
        component_id: &ComponentId,
        constraints: FunctionConstraints,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints>;

    // Delete some constraints from the component
    // returning the remaining constraints
    // The way to invoke delete constraints is to delete a public deployed API
    // that uses the component which will internally compute the function signatures
    // that shouldn't be part of the signature anymore.
    async fn delete_constraints(
        &self,
        component_id: &ComponentId,
        constraints: &[FunctionSignature],
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints>;
}

pub struct CachedComponentService {
    inner: Arc<dyn ComponentService>,
    cache: Cache<(ComponentId, ComponentRevision), (), ComponentDto, ComponentServiceError>,
}

#[async_trait]
impl ComponentService for CachedComponentService {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<ComponentDto> {
        let inner_clone = self.inner.clone();
        self.cache
            .get_or_insert_simple(
                &(component_id.clone(), version.clone()),
                || async {
                    inner_clone
                        .get_by_version(component_id, version, auth_ctx)
                        .await
                },
            )
            .await
    }

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<ComponentDto> {
        self.inner.get_latest_by_id(component_id, auth_ctx).await
    }

    async fn get_latest_by_name(
        &self,
        component_id: &ComponentName,
        environment_id: &EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<ComponentDto> {
        self.inner
            .get_latest_by_name(component_id, environment_id, auth_ctx)
            .await
    }

    async fn get_all_by_name(
        &self,
        component_id: &ComponentName,
        environment_id: &EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<Vec<ComponentDto>> {
        self.inner
            .get_all_by_name(component_id, environment_id, auth_ctx)
            .await
    }

    async fn create_or_update_constraints(
        &self,
        component_id: &ComponentId,
        constraints: FunctionConstraints,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints> {
        self.inner
            .create_or_update_constraints(component_id, constraints, auth_ctx)
            .await
    }

    async fn delete_constraints(
        &self,
        component_id: &ComponentId,
        constraints: &[FunctionSignature],
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints> {
        self.inner
            .delete_constraints(component_id, constraints, auth_ctx)
            .await
    }
}

impl CachedComponentService {
    pub fn new(inner: Arc<dyn ComponentService>, cache_capacity: usize) -> Self {
        Self {
            inner,
            cache: Cache::new(
                Some(cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "component-metadata-cache",
            ),
        }
    }
}

#[derive(Clone)]
pub struct RemoteComponentService {
    client: GrpcClient<ComponentServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl RemoteComponentService {
    pub fn new(uri: Uri, retry_config: RetryConfig, connect_timeout: Duration) -> Self {
        Self {
            client: GrpcClient::new(
                "component_service",
                |channel| {
                    ComponentServiceClient::new(channel)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                uri,
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    connect_timeout,
                },
            ),
            retry_config,
        }
    }

    fn process_metadata_response_opt(
        response: GetComponentMetadataResponse,
    ) -> Result<Option<ComponentDto>, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Empty response".to_string(),
            )),

            Some(get_component_metadata_response::Result::Success(response)) => {
                response.component.map(Self::process_component).transpose()
            }
            Some(get_component_metadata_response::Result::Error(error)) => Err(error.into()),
        }
    }

    fn process_metadata_response(
        response: GetComponentMetadataResponse,
    ) -> Result<ComponentDto, ComponentServiceError> {
        match Self::process_metadata_response_opt(response)? {
            Some(component) => Ok(component),
            None => Err(ComponentServiceError::NotFound(
                "Component not found".to_string(),
            )),
        }
    }

    fn process_get_components_response(
        response: GetComponentsResponse,
    ) -> Result<Vec<ComponentDto>, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Empty response".to_string(),
            )),

            Some(get_components_response::Result::Success(response)) => response
                .components
                .into_iter()
                .map(Self::process_component)
                .collect::<Result<Vec<_>, _>>(),
            Some(get_components_response::Result::Error(error)) => Err(error.into()),
        }
    }

    fn process_get_components_response_and_get_last(
        response: GetComponentsResponse,
    ) -> Result<ComponentDto, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Empty response".to_string(),
            )),

            Some(get_components_response::Result::Success(mut response)) => {
                match response.components.pop() {
                    Some(component) => Self::process_component(component),
                    None => Err(ComponentServiceError::NotFound(
                        "Component not found".to_string(),
                    )),
                }
            }
            Some(get_components_response::Result::Error(error)) => Err(error.into()),
        }
    }

    fn process_component(
        component: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<ComponentDto, ComponentServiceError> {
        component.try_into().map_err(|err| {
            ComponentServiceError::Internal(format!(
                "Response conversion error for component: {err}"
            ))
        })
    }

    fn process_create_component_constraint_response(
        response: CreateComponentConstraintsResponse,
    ) -> Result<FunctionConstraints, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Failed to create component constraints. Empty results".to_string(),
            )),
            Some(create_component_constraints_response::Result::Success(response)) => {
                match response.components {
                    Some(constraints) => {
                        if let Some(constraints) = constraints.constraints {
                            let constraints =
                                FunctionConstraints::try_from(constraints).map_err(|err| {
                                    ComponentServiceError::Internal(format!(
                                        "Response conversion error: {err}"
                                    ))
                                })?;

                            Ok(constraints)
                        } else {
                            Err(ComponentServiceError::Internal(
                                "Failed component constraint creation".to_string(),
                            ))
                        }
                    }
                    None => Err(ComponentServiceError::Internal(
                        "Empty component constraint create response".to_string(),
                    )),
                }
            }
            Some(create_component_constraints_response::Result::Error(error)) => Err(error.into()),
        }
    }

    fn process_delete_component_metadata_response(
        response: DeleteComponentConstraintsResponse,
    ) -> Result<FunctionConstraints, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Failed to create component constraints. Empty results".to_string(),
            )),
            Some(delete_component_constraints_response::Result::Success(response)) => {
                match response.components {
                    Some(remaining_constraints) => {
                        if let Some(remaining_constraints_proto) = remaining_constraints.constraints
                        {
                            let remaining_constraints =
                                FunctionConstraints::try_from(remaining_constraints_proto)
                                    .map_err(|err| {
                                        ComponentServiceError::Internal(format!(
                                            "Response conversion error: {err}"
                                        ))
                                    })?;

                            Ok(remaining_constraints)
                        } else {
                            Err(ComponentServiceError::Internal(
                                "Failed component constraint deletion".to_string(),
                            ))
                        }
                    }
                    None => Err(ComponentServiceError::Internal(
                        "Empty component constraint delete response".to_string(),
                    )),
                }
            }
            Some(delete_component_constraints_response::Result::Error(error)) => Err(error.into()),
        }
    }

    fn is_retriable(error: &ComponentServiceError) -> bool {
        matches!(
            error,
            ComponentServiceError::FailedGrpcStatus(_) | ComponentServiceError::FailedTransport(_)
        )
    }
}

#[async_trait]
impl ComponentService for RemoteComponentService {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        _auth: &AuthCtx,
    ) -> ComponentResult<ComponentDto> {
        with_retries(
            "component",
            "get_component",
            Some(component_id.to_string()),
            &self.retry_config,
            &(self.client.clone(), component_id.clone(), metadata.clone()),
            |(client, id, metadata)| {
                Box::pin(async move {
                    let response = client
                        .call("get_component_metadata", move |client| {
                            let request = GetVersionedComponentRequest {
                                component_id: Some(id.clone().into()),
                                version: version.0,
                            };

                            Box::pin(client.get_component_metadata(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_metadata_response(response)
                })
            },
            Self::is_retriable,
        )
        .await
    }

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        metadata: &AuthCtx,
    ) -> ComponentResult<ComponentDto> {
        with_retries(
            "component",
            "get_latest",
            Some(component_id.to_string()),
            &self.retry_config,
            &(self.client.clone(), component_id.clone(), metadata.clone()),
            |(client, id, metadata)| {
                Box::pin(async move {
                    let response = client
                        .call("get_latest_component_metadata", move |client| {
                            let request = GetLatestComponentRequest {
                                component_id: Some(id.clone().into()),
                            };
                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.get_latest_component_metadata(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_metadata_response(response)
                })
            },
            Self::is_retriable,
        )
        .await
    }

    async fn get_latest_by_name(
        &self,
        component_name: &ComponentName,
        namespace: &Namespace,
        metadata: &AuthCtx,
    ) -> ComponentResult<ComponentDto> {
        with_retries(
            "component",
            "get_latest_by_name",
            Some(component_name.to_string()),
            &self.retry_config,
            &(
                self.client.clone(),
                component_name.0.clone(),
                namespace.project_id.clone(),
                metadata.clone(),
            ),
            |(client, name, project_id, metadata)| {
                Box::pin(async move {
                    let response = client
                        .call("get_components", move |client| {
                            let request = GetComponentsRequest {
                                project_id: Some(project_id.clone().into()),
                                component_name: Some(name.clone()),
                            };

                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.get_components(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_get_components_response_and_get_last(response)
                })
            },
            Self::is_retriable,
        )
        .await
    }

    async fn get_all_by_name(
        &self,
        component_name: &ComponentName,
        namespace: &Namespace,
        metadata: &AuthCtx,
    ) -> ComponentResult<Vec<ComponentDto>> {
        with_retries(
            "component",
            "get_all_by_name",
            Some(component_name.to_string()),
            &self.retry_config,
            &(
                self.client.clone(),
                component_name.0.clone(),
                namespace.project_id.clone(),
                metadata.clone(),
            ),
            |(client, name, project_id, metadata)| {
                Box::pin(async move {
                    let response = client
                        .call("get_components", move |client| {
                            let request = GetComponentsRequest {
                                project_id: Some(project_id.clone().into()),
                                component_name: Some(name.clone()),
                            };

                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.get_components(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_get_components_response(response)
                })
            },
            Self::is_retriable,
        )
        .await
    }

    async fn create_or_update_constraints(
        &self,
        component_id: &ComponentId,
        constraints: FunctionConstraints,
        metadata: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints> {
        let constraints_proto = FunctionConstraintCollectionProto::from(constraints);

        let value = with_retries(
            "component",
            "create_component_constraints",
            Some(component_id.to_string()),
            &self.retry_config,
            &(
                self.client.clone(),
                component_id.clone(),
                metadata.clone(),
                constraints_proto.clone(),
            ),
            |(client, id, metadata, function_constraints)| {
                Box::pin(async move {
                    let response = client
                        .call("create_component_constraints", move |client| {
                            let request = CreateComponentConstraintsRequest {
                                component_constraints: Some(ComponentConstraints {
                                    component_id: Some(
                                        golem_api_grpc::proto::golem::component::ComponentId::from(
                                            id.clone(),
                                        ),
                                    ),
                                    constraints: Some(function_constraints.clone()),
                                }),
                            };
                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.create_component_constraints(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_create_component_constraint_response(response)
                })
            },
            Self::is_retriable,
        )
        .await?;

        Ok(value)
    }

    async fn delete_constraints(
        &self,
        component_id: &ComponentId,
        constraints: &[FunctionSignature],
        metadata: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints> {
        let constraint = constraints
            .iter()
            .map(|x| FunctionUsageConstraint {
                function_signature: x.clone(),
                usage_count: 1, // this is to only reuse the existing grpc types
            })
            .collect::<Vec<_>>();

        let constraints_proto = FunctionConstraintCollectionProto::from(FunctionConstraints {
            constraints: constraint,
        });

        let value = with_retries(
            "component",
            "delete_component_constraints",
            Some(component_id.to_string()),
            &self.retry_config,
            &(
                self.client.clone(),
                component_id.clone(),
                metadata.clone(),
                constraints_proto.clone(),
            ),
            |(client, id, metadata, function_constraints)| {
                Box::pin(async move {
                    let response = client
                        .call("delete_component_constraints", move |client| {
                            let request = DeleteComponentConstraintsRequest {
                                component_constraints: Some(ComponentConstraints {
                                    component_id: Some(
                                        golem_api_grpc::proto::golem::component::ComponentId::from(
                                            id.clone(),
                                        ),
                                    ),
                                    constraints: Some(function_constraints.clone()),
                                }),
                            };
                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.delete_component_constraint(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_delete_component_metadata_response(response)
                })
            },
            Self::is_retriable,
        )
        .await?;

        Ok(value)
    }
}
