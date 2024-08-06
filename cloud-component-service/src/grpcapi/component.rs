use std::sync::Arc;
use tracing::Instrument;

use crate::service::auth::{get_authorisation_token, CloudAuthCtx};
use crate::service::component;
use cloud_common::grpc::proto_project_id_string;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentService;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_request, create_component_response,
    download_component_response, get_component_metadata_all_versions_response,
    get_component_metadata_response, get_components_response, update_component_request,
    update_component_response, ComponentError, CreateComponentRequest,
    CreateComponentRequestHeader, CreateComponentResponse, DownloadComponentRequest,
    DownloadComponentResponse, GetComponentMetadataAllVersionsResponse,
    GetComponentMetadataResponse, GetComponentMetadataSuccessResponse, GetComponentRequest,
    GetComponentSuccessResponse, GetComponentsRequest, GetComponentsResponse,
    GetComponentsSuccessResponse, GetLatestComponentRequest, GetVersionedComponentRequest,
    UpdateComponentRequest, UpdateComponentRequestHeader, UpdateComponentResponse,
};
use golem_api_grpc::proto::golem::component::Component;
use golem_common::grpc::proto_component_id_string;
use golem_common::model::ComponentId;
use golem_common::model::ProjectId;
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_service_base::stream::ByteStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};

impl From<component::ComponentError> for ComponentError {
    fn from(value: component::ComponentError) -> Self {
        let error = match value {
            component::ComponentError::Unauthorized(error) => {
                component_error::Error::Unauthorized(ErrorBody { error })
            }
            component::ComponentError::Internal(error) => {
                component_error::Error::InternalError(ErrorBody {
                    error: error.to_string(),
                })
            }
            component::ComponentError::ComponentProcessing(error) => {
                component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![error.to_string()],
                })
            }
            component::ComponentError::LimitExceeded(error) => {
                component_error::Error::LimitExceeded(ErrorBody { error })
            }
            component::ComponentError::AlreadyExists(_) => {
                component_error::Error::AlreadyExists(ErrorBody {
                    error: value.to_string(),
                })
            }
            component::ComponentError::UnknownComponentId(_)
            | component::ComponentError::UnknownVersionedComponentId(_)
            | component::ComponentError::UnknownProject(_) => {
                component_error::Error::NotFound(ErrorBody {
                    error: value.to_string(),
                })
            }
        };
        ComponentError { error: Some(error) }
    }
}

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
    component_service: Arc<dyn component::ComponentService + Sync + Send>,
}

impl ComponentGrpcApi {
    pub fn new(
        component_service: Arc<dyn crate::service::component::ComponentService + Sync + Send>,
    ) -> Self {
        Self { component_service }
    }

    fn auth(&self, metadata: MetadataMap) -> Result<CloudAuthCtx, ComponentError> {
        match get_authorisation_token(metadata) {
            Some(t) => Ok(CloudAuthCtx::new(t)),
            None => Err(ComponentError {
                error: Some(component_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn get(
        &self,
        request: GetComponentRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<Component>, ComponentError> {
        let auth = self.auth(metadata)?;
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self.component_service.get(&id, &auth).await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_component_metadata(
        &self,
        request: GetVersionedComponentRequest,
        metadata: MetadataMap,
    ) -> Result<Option<Component>, ComponentError> {
        let auth = self.auth(metadata)?;

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
            .get_by_version(&versioned_component_id, &auth)
            .await?;
        Ok(result.map(|p| p.into()))
    }

    async fn get_all(
        &self,
        request: GetComponentsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<Component>, ComponentError> {
        let auth = self.auth(metadata)?;
        let project_id: Option<ProjectId> = request.project_id.and_then(|id| id.try_into().ok());
        let name: Option<golem_service_base::model::ComponentName> = request
            .component_name
            .map(golem_service_base::model::ComponentName);
        let result = self
            .component_service
            .find_by_project_and_name(project_id, name, &auth)
            .await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_latest_component_metadata(
        &self,
        request: GetLatestComponentRequest,
        metadata: MetadataMap,
    ) -> Result<Component, ComponentError> {
        let auth = self.auth(metadata)?;
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self
            .component_service
            .get_latest_version(&id, &auth)
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
        metadata: MetadataMap,
    ) -> Result<ByteStream, ComponentError> {
        let auth = self.auth(metadata)?;
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let version = request.version;
        let result = self
            .component_service
            .download_stream(&id, version, &auth)
            .await?;
        Ok(result)
    }

    async fn create(
        &self,
        request: CreateComponentRequestHeader,
        data: Vec<u8>,
        metadata: MetadataMap,
    ) -> Result<Component, ComponentError> {
        let auth = self.auth(metadata)?;
        let project_id: Option<ProjectId> = request.project_id.and_then(|id| id.try_into().ok());
        let name = golem_service_base::model::ComponentName(request.component_name);
        let result = self
            .component_service
            .create(project_id, &name, data, &auth)
            .await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: UpdateComponentRequestHeader,
        data: Vec<u8>,
        metadata: MetadataMap,
    ) -> Result<Component, ComponentError> {
        let auth = self.auth(metadata)?;
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self.component_service.update(&id, data, &auth).await?;
        Ok(result.into())
    }
}

#[async_trait::async_trait]
impl ComponentService for ComponentGrpcApi {
    async fn get_components(
        &self,
        request: Request<GetComponentsRequest>,
    ) -> Result<Response<GetComponentsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_components",
            project_id = proto_project_id_string(&r.project_id)
        );

        let response = match self.get_all(r, m).instrument(record.span.clone()).await {
            Ok(components) => record.succeed(get_components_response::Result::Success(
                GetComponentsSuccessResponse { components },
            )),
            Err(error) => record.fail(
                get_components_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetComponentsResponse {
            result: Some(response),
        }))
    }

    async fn create_component(
        &self,
        request: Request<Streaming<CreateComponentRequest>>,
    ) -> Result<Response<CreateComponentResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let chunks: Vec<CreateComponentRequest> = r.into_stream().try_collect().await?;
        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                create_component_request::Data::Header(d) => Some(d),
                _ => None,
            })
        });

        let record = recorded_grpc_api_request!(
            "create_component",
            component_name = header.as_ref().map(|r| r.component_name.clone()),
            project_id =
                proto_project_id_string(&header.as_ref().and_then(|r| r.project_id.clone()))
        );

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
                self.create(request, data, m)
                    .instrument(record.span.clone())
                    .await
            }
            None => Err(bad_request_error("Missing request")),
        };

        let result = match result {
            Ok(v) => record.succeed(create_component_response::Result::Success(v)),
            Err(error) => record.fail(
                create_component_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CreateComponentResponse {
            result: Some(result),
        }))
    }

    type DownloadComponentStream = BoxStream<'static, Result<DownloadComponentResponse, Status>>;

    async fn download_component(
        &self,
        request: Request<DownloadComponentRequest>,
    ) -> Result<Response<Self::DownloadComponentStream>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "download_component",
            component_id = proto_component_id_string(&r.component_id)
        );
        let stream: Self::DownloadComponentStream =
            match self.download(r, m).instrument(record.span.clone()).await {
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
                    record.succeed(stream)
                }
                Err(err) => {
                    let res = DownloadComponentResponse {
                        result: Some(download_component_response::Result::Error(err.clone())),
                    };

                    let stream: Self::DownloadComponentStream =
                        Box::pin(tokio_stream::iter([Ok(res)]));
                    record.fail(stream, &ComponentTraceErrorKind(&err))
                }
            };

        Ok(Response::new(stream))
    }

    async fn get_component_metadata_all_versions(
        &self,
        request: Request<GetComponentRequest>,
    ) -> Result<Response<GetComponentMetadataAllVersionsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_component_metadata_all_versions",
            component_id = proto_component_id_string(&r.component_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(components) => record.succeed(
                get_component_metadata_all_versions_response::Result::Success(
                    GetComponentSuccessResponse { components },
                ),
            ),
            Err(error) => record.fail(
                get_component_metadata_all_versions_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetComponentMetadataAllVersionsResponse {
            result: Some(response),
        }))
    }

    async fn get_latest_component_metadata(
        &self,
        request: Request<GetLatestComponentRequest>,
    ) -> Result<Response<GetComponentMetadataResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_latest_component_metadata",
            component_id = proto_component_id_string(&r.component_id)
        );

        let response = match self
            .get_latest_component_metadata(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(component) => record.succeed(get_component_metadata_response::Result::Success(
                GetComponentMetadataSuccessResponse {
                    component: Some(component),
                },
            )),
            Err(error) => record.fail(
                get_component_metadata_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetComponentMetadataResponse {
            result: Some(response),
        }))
    }

    async fn update_component(
        &self,
        request: Request<Streaming<UpdateComponentRequest>>,
    ) -> Result<Response<UpdateComponentResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let chunks: Vec<UpdateComponentRequest> = r.into_stream().try_collect().await?;

        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                update_component_request::Data::Header(d) => Some(d),
                _ => None,
            })
        });

        let record = recorded_grpc_api_request!(
            "update_component",
            component_id =
                proto_component_id_string(&header.as_ref().and_then(|r| r.component_id.clone()))
        );

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
                self.update(request, data, m)
                    .instrument(record.span.clone())
                    .await
            }
            None => Err(bad_request_error("Missing request")),
        };

        let result = match result {
            Ok(v) => record.succeed(update_component_response::Result::Success(v)),
            Err(error) => record.fail(
                update_component_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateComponentResponse {
            result: Some(result),
        }))
    }

    async fn get_component_metadata(
        &self,
        request: Request<GetVersionedComponentRequest>,
    ) -> Result<Response<GetComponentMetadataResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_component_metadata",
            component_id = proto_component_id_string(&r.component_id)
        );

        let response = match self
            .get_component_metadata(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(component) => record.succeed(get_component_metadata_response::Result::Success(
                GetComponentMetadataSuccessResponse { component },
            )),
            Err(error) => record.fail(
                get_component_metadata_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetComponentMetadataResponse {
            result: Some(response),
        }))
    }
}
