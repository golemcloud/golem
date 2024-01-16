use std::sync::Arc;

use futures_util::stream::BoxStream;
use futures_util::TryStreamExt;
use golem_common::model::{TemplateId};
use golem_common::proto::golem::cloudservices::templateservice::template_service_server::TemplateService;
use golem_common::proto::golem::cloudservices::templateservice::{
    create_template_request, create_template_response, download_template_response,
    get_latest_template_version_response, get_template_response,
    update_template_request, update_template_response, CreateTemplateRequest,
    CreateTemplateRequestHeader, CreateTemplateResponse, DownloadTemplateRequest,
    DownloadTemplateResponse, GetLatestTemplateVersionRequest, GetLatestTemplateVersionResponse,
    GetTemplateRequest, GetTemplateResponse, GetTemplateSuccessResponse,
    UpdateTemplateRequest,
    UpdateTemplateRequestHeader, UpdateTemplateResponse,
};
use golem_common::proto::golem::{
    template_error, ErrorBody, ErrorsBody, Template, TemplateError, TokenSecret,
};
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};

use crate::service::template;

impl From<template::TemplateError> for TemplateError {
    fn from(value: template::TemplateError) -> Self {
        let error = match value {
            template::TemplateError::Internal(error) => {
                template_error::Error::InternalError(ErrorBody { error })
            }
            template::TemplateError::IOError(error) => {
                template_error::Error::InternalError(ErrorBody { error })
            }
            template::TemplateError::TemplateProcessingError(error) => {
                template_error::Error::BadRequest(ErrorsBody {
                    errors: vec![error],
                })
            }
            template::TemplateError::AlreadyExists(_) => {
                template_error::Error::AlreadyExists(ErrorBody {
                    error: "Template already exists".to_string(),
                })
            }
            template::TemplateError::UnknownTemplateId(_) => {
                template_error::Error::NotFound(ErrorBody {
                    error: "Template not found".to_string(),
                })
            }
            template::TemplateError::UnknownVersionedTemplateId(_) => {
                template_error::Error::NotFound(ErrorBody {
                    error: "Template not found".to_string(),
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

pub struct TemplateGrpcApi {
    pub template_service: Arc<dyn template::TemplateService + Sync + Send>,
}

impl TemplateGrpcApi {
    async fn get(
        &self,
        request: GetTemplateRequest,
    ) -> Result<Vec<Template>, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let result = self.template_service.get(&id).await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_latest_version(
        &self,
        request: GetLatestTemplateVersionRequest,
    ) -> Result<i32, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let result = self.template_service.get_latest_version(&id).await?;
        match result {
            Some(template) => Ok(template.versioned_template_id.version),
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
    ) -> Result<Vec<u8>, TemplateError> {
        let id: TemplateId = request
            .template_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing template id"))?;
        let version = request.version;
        let result = self.template_service.download(&id, version).await?;
        Ok(result)
    }

    async fn create(
        &self,
        request: CreateTemplateRequestHeader,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        let name = golem_cloud_server_base::model::TemplateName(request.template_name);
        let result = self
            .template_service
            .create(&name, data)
            .await?;
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
    async fn create_template(
        &self,
        request: Request<Streaming<CreateTemplateRequest>>,
    ) -> Result<Response<CreateTemplateResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let chunks: Vec<CreateTemplateRequest> = r.into_stream().try_collect().await?;
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
        let (m, _, r) = request.into_parts();
        let res = match self.download(r).await {
            Ok(content) => DownloadTemplateResponse {
                result: Some(download_template_response::Result::SuccessChunk(content)),
            },
            Err(err) => DownloadTemplateResponse {
                result: Some(download_template_response::Result::Error(err)),
            },
        };

        let stream: Self::DownloadTemplateStream = Box::pin(tokio_stream::iter([Ok(res)]));
        Ok(Response::new(stream))
    }

    async fn get_template(
        &self,
        request: Request<GetTemplateRequest>,
    ) -> Result<Response<GetTemplateResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r).await {
            Ok(templates) => Ok(Response::new(GetTemplateResponse {
                result: Some(get_template_response::Result::Success(
                    GetTemplateSuccessResponse { templates },
                )),
            })),
            Err(err) => Ok(Response::new(GetTemplateResponse {
                result: Some(get_template_response::Result::Error(err)),
            })),
        }
    }

    async fn get_latest_template_version(
        &self,
        request: Request<GetLatestTemplateVersionRequest>,
    ) -> Result<Response<GetLatestTemplateVersionResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get_latest_version(r).await {
            Ok(v) => Ok(Response::new(GetLatestTemplateVersionResponse {
                result: Some(get_latest_template_version_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(GetLatestTemplateVersionResponse {
                result: Some(get_latest_template_version_response::Result::Error(err)),
            })),
        }
    }

    async fn update_template(
        &self,
        request: Request<Streaming<UpdateTemplateRequest>>,
    ) -> Result<Response<UpdateTemplateResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let chunks: Vec<UpdateTemplateRequest> = r.into_stream().try_collect().await?;

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
