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
use golem_api_grpc::proto::golem::template::template_service_server::TemplateService;
use golem_api_grpc::proto::golem::template::{
    create_template_request, create_template_response, download_template_response,
    get_template_metadata_all_versions_response, get_template_metadata_response,
    get_templates_response, update_template_request, update_template_response,
    CreateTemplateRequest, CreateTemplateRequestHeader, CreateTemplateResponse,
    DownloadTemplateRequest, DownloadTemplateResponse, GetLatestTemplateRequest,
    GetTemplateMetadataAllVersionsResponse, GetTemplateMetadataResponse,
    GetTemplateMetadataSuccessResponse, GetTemplateRequest, GetTemplateSuccessResponse,
    GetTemplatesRequest, GetTemplatesResponse, GetTemplatesSuccessResponse,
    GetVersionedTemplateRequest, UpdateTemplateRequest, UpdateTemplateRequestHeader,
    UpdateTemplateResponse,
};
use golem_api_grpc::proto::golem::template::{template_error, Template, TemplateError};
use golem_common::model::TemplateId;
use golem_service_base::service::template_object_store::GetTemplateStream;
use tonic::{Request, Response, Status, Streaming};

use crate::service::template;

impl From<template::TemplateError> for TemplateError {
    fn from(value: template::TemplateError) -> Self {
        let error = match value {
            template::TemplateError::AlreadyExists(_) => {
                template_error::Error::AlreadyExists(ErrorBody {
                    error: value.to_string(),
                })
            }
            template::TemplateError::UnknownTemplateId(_)
            | template::TemplateError::UnknownVersionedTemplateId(_) => {
                template_error::Error::NotFound(ErrorBody {
                    error: value.to_string(),
                })
            }
            template::TemplateError::TemplateProcessingError(error) => {
                template_error::Error::BadRequest(ErrorsBody {
                    errors: vec![error.to_string()],
                })
            }
            template::TemplateError::Internal(error) => {
                template_error::Error::InternalError(ErrorBody {
                    error: error.to_string(),
                })
            }
        };
        TemplateError { error: Some(error) }
    }
}

fn bad_request_error(error: &str) -> TemplateError {
    TemplateError {
        error: Some(template_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

fn internal_error(error: &str) -> TemplateError {
    TemplateError {
        error: Some(template_error::Error::InternalError(ErrorBody {
            error: error.to_string(),
        })),
    }
}

pub struct TemplateGrpcApi {
    pub template_service: Arc<dyn template::TemplateService + Sync + Send>,
}

impl TemplateGrpcApi {
    async fn get(&self, request: GetTemplateRequest) -> Result<Vec<Template>, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let result = self.template_service.get(&id).await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_template_metadata(
        &self,
        request: GetVersionedTemplateRequest,
    ) -> Result<Option<Template>, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;

        let version = request.version;

        let versioned_template_id = golem_service_base::model::VersionedTemplateId {
            template_id: id,
            version,
        };

        let result = self
            .template_service
            .get_by_version(&versioned_template_id)
            .await?;
        Ok(result.map(|p| p.into()))
    }

    async fn get_all(&self, request: GetTemplatesRequest) -> Result<Vec<Template>, TemplateError> {
        let name: Option<golem_service_base::model::TemplateName> = request
            .template_name
            .map(golem_service_base::model::TemplateName);
        let result = self.template_service.find_by_name(name).await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_latest_template_metadata(
        &self,
        request: GetLatestTemplateRequest,
    ) -> Result<Template, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let result = self.template_service.get_latest_version(&id).await?;
        match result {
            Some(template) => Ok(template.into()),
            None => Err(TemplateError {
                error: Some(template_error::Error::NotFound(ErrorBody {
                    error: "Template not found".to_string(),
                })),
            }),
        }
    }

    async fn download(
        &self,
        request: DownloadTemplateRequest,
    ) -> Result<GetTemplateStream, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let version = request.version;
        let result = self.template_service.download_stream(&id, version).await?;
        Ok(result)
    }

    async fn create(
        &self,
        request: CreateTemplateRequestHeader,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        let name = golem_service_base::model::TemplateName(request.template_name);
        let result = self.template_service.create(&name, data).await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: UpdateTemplateRequestHeader,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let result = self.template_service.update(&id, data).await?;
        Ok(result.into())
    }
}

#[async_trait::async_trait]
impl TemplateService for TemplateGrpcApi {
    async fn get_templates(
        &self,
        request: Request<GetTemplatesRequest>,
    ) -> Result<Response<GetTemplatesResponse>, Status> {
        match self.get_all(request.into_inner()).await {
            Ok(templates) => Ok(Response::new(GetTemplatesResponse {
                result: Some(get_templates_response::Result::Success(
                    GetTemplatesSuccessResponse { templates },
                )),
            })),
            Err(err) => Ok(Response::new(GetTemplatesResponse {
                result: Some(get_templates_response::Result::Error(err)),
            })),
        }
    }

    async fn create_template(
        &self,
        request: Request<Streaming<CreateTemplateRequest>>,
    ) -> Result<Response<CreateTemplateResponse>, Status> {
        let chunks: Vec<CreateTemplateRequest> =
            request.into_inner().into_stream().try_collect().await?;
        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                create_template_request::Data::Header(d) => Some(d),
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
                                create_template_request::Data::Chunk(d) => d.template_chunk,
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
            Ok(v) => Ok(Response::new(CreateTemplateResponse {
                result: Some(create_template_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(CreateTemplateResponse {
                result: Some(create_template_response::Result::Error(err)),
            })),
        }
    }

    type DownloadTemplateStream = BoxStream<'static, Result<DownloadTemplateResponse, Status>>;

    async fn download_template(
        &self,
        request: Request<DownloadTemplateRequest>,
    ) -> Result<Response<Self::DownloadTemplateStream>, Status> {
        // TODO: Add a log here for error.
        match self.download(request.into_inner()).await {
            Ok(response) => {
                let stream = response.map(|content| {
                    let res = match content {
                        Ok(content) => DownloadTemplateResponse {
                            result: Some(download_template_response::Result::SuccessChunk(
                                content.to_vec(),
                            )),
                        },
                        Err(_) => DownloadTemplateResponse {
                            result: Some(download_template_response::Result::Error(
                                internal_error("Internal error"),
                            )),
                        },
                    };
                    Ok(res)
                });
                let stream: Self::DownloadTemplateStream = Box::pin(stream);
                Ok(Response::new(stream))
            }
            Err(err) => {
                let res = DownloadTemplateResponse {
                    result: Some(download_template_response::Result::Error(err.into())),
                };

                let stream: Self::DownloadTemplateStream = Box::pin(tokio_stream::iter([Ok(res)]));
                Ok(Response::new(stream))
            }
        }
    }

    async fn get_template_metadata_all_versions(
        &self,
        request: Request<GetTemplateRequest>,
    ) -> Result<Response<GetTemplateMetadataAllVersionsResponse>, Status> {
        match self.get(request.into_inner()).await {
            Ok(templates) => Ok(Response::new(GetTemplateMetadataAllVersionsResponse {
                result: Some(
                    get_template_metadata_all_versions_response::Result::Success(
                        GetTemplateSuccessResponse { templates },
                    ),
                ),
            })),
            Err(err) => Ok(Response::new(GetTemplateMetadataAllVersionsResponse {
                result: Some(get_template_metadata_all_versions_response::Result::Error(
                    err,
                )),
            })),
        }
    }

    async fn get_template_metadata(
        &self,
        request: Request<GetVersionedTemplateRequest>,
    ) -> Result<Response<GetTemplateMetadataResponse>, Status> {
        match self.get_template_metadata(request.into_inner()).await {
            Ok(optional_template) => Ok(Response::new(GetTemplateMetadataResponse {
                result: Some(get_template_metadata_response::Result::Success(
                    GetTemplateMetadataSuccessResponse {
                        template: optional_template,
                    },
                )),
            })),
            Err(err) => Ok(Response::new(GetTemplateMetadataResponse {
                result: Some(get_template_metadata_response::Result::Error(err)),
            })),
        }
    }

    async fn get_latest_template_metadata(
        &self,
        request: Request<GetLatestTemplateRequest>,
    ) -> Result<Response<GetTemplateMetadataResponse>, Status> {
        match self
            .get_latest_template_metadata(request.into_inner())
            .await
        {
            Ok(template) => Ok(Response::new(GetTemplateMetadataResponse {
                result: Some(get_template_metadata_response::Result::Success(
                    GetTemplateMetadataSuccessResponse {
                        template: Some(template),
                    },
                )),
            })),
            Err(err) => Ok(Response::new(GetTemplateMetadataResponse {
                result: Some(get_template_metadata_response::Result::Error(err)),
            })),
        }
    }

    async fn update_template(
        &self,
        request: Request<Streaming<UpdateTemplateRequest>>,
    ) -> Result<Response<UpdateTemplateResponse>, Status> {
        let chunks: Vec<UpdateTemplateRequest> =
            request.into_inner().into_stream().try_collect().await?;

        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                update_template_request::Data::Header(d) => Some(d),
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
                                update_template_request::Data::Chunk(d) => d.template_chunk,
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
            Ok(v) => Ok(Response::new(UpdateTemplateResponse {
                result: Some(update_template_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(UpdateTemplateResponse {
                result: Some(update_template_response::Result::Error(err)),
            })),
        }
    }
}
