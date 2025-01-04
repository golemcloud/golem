// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
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
    create_component_constraints_response, get_component_metadata_response,
    CreateComponentConstraintsRequest, CreateComponentConstraintsResponse,
    GetComponentMetadataResponse, GetLatestComponentRequest, GetVersionedComponentRequest,
};
use golem_api_grpc::proto::golem::component::ComponentConstraints;
use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::ComponentId;
use golem_common::model::RetryConfig;
use golem_common::retries::with_retries;
use golem_service_base::model::Component;
use http::Uri;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

pub type ComponentResult<T> = Result<T, ComponentServiceError>;

#[async_trait]
pub trait ComponentService<AuthCtx> {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: u64,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component>;

    async fn get_latest(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component>;

    async fn create_or_update_constraints(
        &self,
        component_id: &ComponentId,
        constraints: FunctionConstraintCollection,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraintCollection>;
}

#[derive(Clone)]
pub struct RemoteComponentService {
    client: GrpcClient<ComponentServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl RemoteComponentService {
    pub fn new(uri: Uri, retry_config: RetryConfig) -> Self {
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
                    ..Default::default() // TODO
                },
            ),
            retry_config,
        }
    }

    fn process_metadata_response(
        response: GetComponentMetadataResponse,
    ) -> Result<Component, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Empty response".to_string(),
            )),

            Some(get_component_metadata_response::Result::Success(response)) => {
                let component_view: Result<Component, ComponentServiceError> = match response
                    .component
                {
                    Some(component) => {
                        let component: Component = component.clone().try_into().map_err(|err| {
                            ComponentServiceError::Internal(format!(
                                "Response conversion error: {err}"
                            ))
                        })?;
                        Ok(component)
                    }
                    None => Err(ComponentServiceError::Internal(
                        "Empty component response".to_string(),
                    )),
                };
                Ok(component_view?)
            }
            Some(get_component_metadata_response::Result::Error(error)) => Err(error.into()),
        }
    }

    fn process_create_component_metadata_response(
        response: CreateComponentConstraintsResponse,
    ) -> Result<FunctionConstraintCollection, ComponentServiceError> {
        match response.result {
            None => Err(ComponentServiceError::Internal(
                "Failed to create component constraints. Empty results".to_string(),
            )),
            Some(create_component_constraints_response::Result::Success(response)) => {
                match response.components {
                    Some(constraints) => {
                        if let Some(constraints) = constraints.constraints {
                            let constraints = FunctionConstraintCollection::try_from(constraints)
                                .map_err(|err| {
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
                        "Empty component response".to_string(),
                    )),
                }
            }
            Some(create_component_constraints_response::Result::Error(error)) => Err(error.into()),
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
impl<AuthCtx> ComponentService<AuthCtx> for RemoteComponentService
where
    AuthCtx: IntoIterator<Item = (String, String)> + Clone + Send + Sync,
{
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: u64,
        metadata: &AuthCtx,
    ) -> ComponentResult<Component> {
        let value = with_retries(
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
                                version,
                            };

                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.get_component_metadata(request))
                        })
                        .await?
                        .into_inner();

                    Self::process_metadata_response(response)
                })
            },
            Self::is_retriable,
        )
        .await?;

        Ok(value)
    }

    async fn get_latest(
        &self,
        component_id: &ComponentId,
        metadata: &AuthCtx,
    ) -> ComponentResult<Component> {
        let value = with_retries(
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
        .await?;

        Ok(value)
    }

    async fn create_or_update_constraints(
        &self,
        component_id: &ComponentId,
        constraints: FunctionConstraintCollection,
        metadata: &AuthCtx,
    ) -> ComponentResult<FunctionConstraintCollection> {
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
                                project_id: None,
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

                    Self::process_create_component_metadata_response(response)
                })
            },
            Self::is_retriable,
        )
        .await?;

        Ok(value)
    }
}
