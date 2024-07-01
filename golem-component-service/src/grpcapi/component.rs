// Copyright 2024 Golem Cloud
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

use std::sync::Arc;

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::component_service_server::ComponentService;
use golem_api_grpc::proto::golem::component::{component_error, Component, ComponentError};
use golem_api_grpc::proto::golem::component::{
    create_component_request, create_component_response, download_component_response,
    get_component_metadata_all_versions_response, get_component_metadata_response,
    get_components_response, update_component_request, update_component_response,
    CreateComponentRequest, CreateComponentRequestHeader, CreateComponentResponse,
    DownloadComponentRequest, DownloadComponentResponse, GetComponentMetadataAllVersionsResponse,
    GetComponentMetadataResponse, GetComponentMetadataSuccessResponse, GetComponentRequest,
    GetComponentSuccessResponse, GetComponentsRequest, GetComponentsResponse,
    GetComponentsSuccessResponse, GetLatestComponentRequest, GetVersionedComponentRequest,
    UpdateComponentRequest, UpdateComponentRequestHeader, UpdateComponentResponse,
};
use golem_common::model::ComponentId;
use golem_service_base::stream::ByteStream;
use tonic::{Request, Response, Status, Streaming};

use golem_component_service_base::service::component;
use golem_service_base::auth::DefaultNamespace;

fn bad_request_error(error: &str) -> ComponentError {
    ComponentError {
        error: Some(component_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

fn internal_error(error: &str) -> ComponentError {
    ComponentError {
        error: Some(component_error::Error::InternalError(ErrorBody {
            error: error.to_string(),
        })),
    }
}

pub struct ComponentGrpcApi {
    pub component_service: Arc<dyn component::ComponentService<DefaultNamespace> + Sync + Send>,
}

impl ComponentGrpcApi {
    async fn get(&self, request: GetComponentRequest) -> Result<Vec<Component>, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self
            .component_service
            .get(&id, &DefaultNamespace::default())
            .await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_component_metadata(
        &self,
        request: GetVersionedComponentRequest,
    ) -> Result<Option<Component>, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;

        let version = request.version;

        let versioned_component_id = golem_service_base::model::VersionedComponentId {
            component_id: id,
            version,
        };

        let result = self
            .component_service
            .get_by_version(&versioned_component_id, &DefaultNamespace::default())
            .await?;
        Ok(result.map(|p| p.into()))
    }

    async fn get_all(
        &self,
        request: GetComponentsRequest,
    ) -> Result<Vec<Component>, ComponentError> {
        let name: Option<golem_service_base::model::ComponentName> = request
            .component_name
            .map(golem_service_base::model::ComponentName);
        let result = self
            .component_service
            .find_by_name(name, &DefaultNamespace::default())
            .await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_latest_component_metadata(
        &self,
        request: GetLatestComponentRequest,
    ) -> Result<Component, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self
            .component_service
            .get_latest_version(&id, &DefaultNamespace::default())
            .await?;
        match result {
            Some(component) => Ok(component.into()),
            None => Err(ComponentError {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: "Component not found".to_string(),
                })),
            }),
        }
    }

    async fn download(
        &self,
        request: DownloadComponentRequest,
    ) -> Result<ByteStream, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let version = request.version;
        let result = self
            .component_service
            .download_stream(&id, version, &DefaultNamespace::default())
            .await?;
        Ok(result)
    }

    async fn create(
        &self,
        request: CreateComponentRequestHeader,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError> {
        let name = golem_service_base::model::ComponentName(request.component_name);
        let result = self
            .component_service
            .create(
                &ComponentId::new_v4(),
                &name,
                data,
                &DefaultNamespace::default(),
            )
            .await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: UpdateComponentRequestHeader,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self
            .component_service
            .update(&id, data, &DefaultNamespace::default())
            .await?;
        Ok(result.into())
    }
}

#[async_trait::async_trait]
impl ComponentService for ComponentGrpcApi {
    async fn get_components(
        &self,
        request: Request<GetComponentsRequest>,
    ) -> Result<Response<GetComponentsResponse>, Status> {
        match self.get_all(request.into_inner()).await {
            Ok(components) => Ok(Response::new(GetComponentsResponse {
                result: Some(get_components_response::Result::Success(
                    GetComponentsSuccessResponse { components },
                )),
            })),
            Err(err) => Ok(Response::new(GetComponentsResponse {
                result: Some(get_components_response::Result::Error(err)),
            })),
        }
    }

    async fn create_component(
        &self,
        request: Request<Streaming<CreateComponentRequest>>,
    ) -> Result<Response<CreateComponentResponse>, Status> {
        let chunks: Vec<CreateComponentRequest> =
            request.into_inner().into_stream().try_collect().await?;
        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                create_component_request::Data::Header(d) => Some(d),
                _ => None,
            })
        });

        let result = match header {
            Some(request) => {
                let data: Vec<u8> = chunks
                    .iter()
                    .flat_map(|c| {
                        c.clone()
                            .data
                            .map(|d| match d {
                                create_component_request::Data::Chunk(d) => d.component_chunk,
                                _ => vec![],
                            })
                            .unwrap_or_default()
                    })
                    .collect();
                self.create(request, data).await
            }
            None => Err(bad_request_error("Missing request")),
        };

        match result {
            Ok(v) => Ok(Response::new(CreateComponentResponse {
                result: Some(create_component_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(CreateComponentResponse {
                result: Some(create_component_response::Result::Error(err)),
            })),
        }
    }

    type DownloadComponentStream = BoxStream<'static, Result<DownloadComponentResponse, Status>>;

    async fn download_component(
        &self,
        request: Request<DownloadComponentRequest>,
    ) -> Result<Response<Self::DownloadComponentStream>, Status> {
        match self.download(request.into_inner()).await {
            Ok(response) => {
                let stream = response.map(|content| {
                    let res = match content {
                        Ok(content) => DownloadComponentResponse {
                            result: Some(download_component_response::Result::SuccessChunk(
                                content,
                            )),
                        },
                        Err(_) => DownloadComponentResponse {
                            result: Some(download_component_response::Result::Error(
                                internal_error("Internal error"),
                            )),
                        },
                    };
                    Ok(res)
                });
                let stream: Self::DownloadComponentStream = Box::pin(stream);
                Ok(Response::new(stream))
            }
            Err(err) => {
                let res = DownloadComponentResponse {
                    result: Some(download_component_response::Result::Error(err)),
                };

                let stream: Self::DownloadComponentStream = Box::pin(tokio_stream::iter([Ok(res)]));
                Ok(Response::new(stream))
            }
        }
    }

    async fn get_component_metadata_all_versions(
        &self,
        request: Request<GetComponentRequest>,
    ) -> Result<Response<GetComponentMetadataAllVersionsResponse>, Status> {
        match self.get(request.into_inner()).await {
            Ok(components) => Ok(Response::new(GetComponentMetadataAllVersionsResponse {
                result: Some(
                    get_component_metadata_all_versions_response::Result::Success(
                        GetComponentSuccessResponse { components },
                    ),
                ),
            })),
            Err(err) => Ok(Response::new(GetComponentMetadataAllVersionsResponse {
                result: Some(get_component_metadata_all_versions_response::Result::Error(
                    err,
                )),
            })),
        }
    }

    async fn get_latest_component_metadata(
        &self,
        request: Request<GetLatestComponentRequest>,
    ) -> Result<Response<GetComponentMetadataResponse>, Status> {
        match self
            .get_latest_component_metadata(request.into_inner())
            .await
        {
            Ok(component) => Ok(Response::new(GetComponentMetadataResponse {
                result: Some(get_component_metadata_response::Result::Success(
                    GetComponentMetadataSuccessResponse {
                        component: Some(component),
                    },
                )),
            })),
            Err(err) => Ok(Response::new(GetComponentMetadataResponse {
                result: Some(get_component_metadata_response::Result::Error(err)),
            })),
        }
    }

    async fn update_component(
        &self,
        request: Request<Streaming<UpdateComponentRequest>>,
    ) -> Result<Response<UpdateComponentResponse>, Status> {
        let chunks: Vec<UpdateComponentRequest> =
            request.into_inner().into_stream().try_collect().await?;

        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                update_component_request::Data::Header(d) => Some(d),
                _ => None,
            })
        });

        let result = match header {
            Some(request) => {
                let data: Vec<u8> = chunks
                    .iter()
                    .flat_map(|c| {
                        c.clone()
                            .data
                            .map(|d| match d {
                                update_component_request::Data::Chunk(d) => d.component_chunk,
                                _ => vec![],
                            })
                            .unwrap_or_default()
                    })
                    .collect();
                self.update(request, data).await
            }
            None => Err(bad_request_error("Missing request")),
        };

        match result {
            Ok(v) => Ok(Response::new(UpdateComponentResponse {
                result: Some(update_component_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(UpdateComponentResponse {
                result: Some(update_component_response::Result::Error(err)),
            })),
        }
    }

    async fn get_component_metadata(
        &self,
        request: Request<GetVersionedComponentRequest>,
    ) -> Result<Response<GetComponentMetadataResponse>, Status> {
        match self.get_component_metadata(request.into_inner()).await {
            Ok(optional_component) => Ok(Response::new(GetComponentMetadataResponse {
                result: Some(get_component_metadata_response::Result::Success(
                    GetComponentMetadataSuccessResponse {
                        component: optional_component,
                    },
                )),
            })),
            Err(err) => Ok(Response::new(GetComponentMetadataResponse {
                result: Some(get_component_metadata_response::Result::Error(err)),
            })),
        }
    }
}
