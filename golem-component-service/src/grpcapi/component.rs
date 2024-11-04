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

use std::pin::Pin;
use std::sync::Arc;
use tracing::Instrument;

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentService;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_constraints_response, create_component_request,
    create_component_response, download_component_files_response, download_component_response,
    get_component_metadata_all_versions_response, get_component_metadata_response,
    get_components_response, update_component_request, update_component_response, ComponentError,
    CreateComponentConstraintsRequest, CreateComponentConstraintsResponse,
    CreateComponentConstraintsSuccessResponse, CreateComponentRequest,
    CreateComponentRequestHeader, CreateComponentResponse, DownloadComponentFilesRequest,
    DownloadComponentFilesResponse, DownloadComponentRequest, DownloadComponentResponse,
    GetComponentMetadataAllVersionsResponse, GetComponentMetadataResponse,
    GetComponentMetadataSuccessResponse, GetComponentRequest, GetComponentSuccessResponse,
    GetComponentsRequest, GetComponentsResponse, GetComponentsSuccessResponse,
    GetLatestComponentRequest, GetVersionedComponentRequest, UpdateComponentRequest,
    UpdateComponentRequestHeader, UpdateComponentResponse,
};
use golem_api_grpc::proto::golem::component::Component;
use golem_api_grpc::proto::golem::component::ComponentConstraints as ComponentConstraintsProto;
use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
use golem_common::grpc::proto_component_id_string;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::{ComponentId, ComponentType};
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_component_service_base::model::ComponentConstraints;
use golem_component_service_base::service::component;
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::stream::ByteStream;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

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
        Ok(result.into_iter().map(Component::from).collect())
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
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>> + Send + Sync>>,
        ComponentError,
    > {
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

    async fn download_files(
        &self,
        request: DownloadComponentFilesRequest,
    ) -> Result<ByteStream, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let version = request.version;
        let result = self
            .component_service
            .download_files_stream(&id, version, &DefaultNamespace::default())
            .await?;
        Ok(result)
    }

    async fn create(
        &self,
        component_id: ComponentId,
        request: CreateComponentRequestHeader,
        data: Vec<u8>,
        initial_fs_archive: Option<File>,
    ) -> Result<Component, ComponentError> {
        let name = golem_service_base::model::ComponentName(request.component_name.clone());
        let result = self
            .component_service
            .create(
                &component_id,
                &name,
                request.component_type().into(),
                data,
                initial_fs_archive,
                &DefaultNamespace::default(),
            )
            .await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: UpdateComponentRequestHeader,
        data: Vec<u8>,
        initial_fs_archive: Option<File>,
    ) -> Result<Component, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let component_type = match request.component_type {
            Some(n) => Some(
                ComponentType::try_from(n)
                    .map_err(|_| bad_request_error("Invalid component type"))?,
            ),
            None => None,
        };
        let result = self
            .component_service
            .update(
                &id,
                data,
                component_type,
                initial_fs_archive,
                &DefaultNamespace::default(),
            )
            .await?;
        Ok(result.into())
    }

    async fn create_component_constraints(
        &self,
        component_constraint: &ComponentConstraints<DefaultNamespace>,
    ) -> Result<ComponentConstraintsProto, ComponentError> {
        let response = self
            .component_service
            .create_or_update_constraint(component_constraint)
            .await
            .map(|v| ComponentConstraintsProto {
                component_id: Some(v.component_id.into()),
                constraints: Some(FunctionConstraintCollectionProto::from(v.constraints)),
            })?;

        Ok(response)
    }
}

#[async_trait::async_trait]
impl ComponentService for ComponentGrpcApi {
    async fn get_components(
        &self,
        request: Request<GetComponentsRequest>,
    ) -> Result<Response<GetComponentsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("get_components",);

        let response = match self.get_all(request).instrument(record.span.clone()).await {
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
        let mut maybe_header = None;
        let mut component_data = Vec::new();

        let mut initial_fs_archive = None;
        let mut temp_dir_for_archive = None;

        let mut request_stream = request.into_inner();
        while let Some(CreateComponentRequest { data: Some(data) }) =
            request_stream.message().await?
        {
            match data {
                create_component_request::Data::Header(header) => {
                    maybe_header = Some(header);
                }
                create_component_request::Data::Chunk(mut component_chunk) => {
                    component_data.append(&mut component_chunk.component_chunk);
                }
                create_component_request::Data::FilesChunk(files_chunk) => {
                    if temp_dir_for_archive.is_none() {
                        temp_dir_for_archive =
                            Some(tempfile::Builder::new().prefix("golem").tempdir()?);
                    }
                    if initial_fs_archive.is_none() {
                        initial_fs_archive = Some(
                            File::create_new(
                                temp_dir_for_archive
                                    .as_ref()
                                    .unwrap()
                                    .path()
                                    .join("initial_fs_archive.zip"),
                            )
                            .await?,
                        );
                    }
                    initial_fs_archive
                        .as_mut()
                        .unwrap()
                        .write_all(&files_chunk.files_chunk)
                        .await?;
                }
            }
        }

        let component_id = ComponentId::new_v4();
        let record = recorded_grpc_api_request!(
            "create_component",
            component_name = maybe_header
                .as_ref()
                .map(|header| header.component_name.clone()),
            component_id = component_id.to_string(),
        );

        let result = if let Some(request_header) = maybe_header {
            self.create(
                component_id,
                request_header,
                component_data,
                initial_fs_archive,
            )
            .instrument(record.span.clone())
            .await
        } else {
            Err(bad_request_error("Missing request header"))
        };

        let result = match result {
            Ok(component) => record.succeed(create_component_response::Result::Success(component)),
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component",
            component_id = proto_component_id_string(&request.component_id),
            component_version = request.version.unwrap_or_default().to_string(),
        );
        let stream: Self::DownloadComponentStream =
            match self.download(request).instrument(record.span.clone()).await {
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
                        Box::pin(tokio_stream::once(Ok(res)));
                    record.fail(stream, &ComponentTraceErrorKind(&err))
                }
            };

        Ok(Response::new(stream))
    }

    type DownloadComponentFilesStream =
        BoxStream<'static, Result<DownloadComponentFilesResponse, Status>>;

    async fn download_component_files(
        &self,
        request: Request<DownloadComponentFilesRequest>,
    ) -> Result<Response<Self::DownloadComponentFilesStream>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component_files",
            component_id = proto_component_id_string(&request.component_id)
        );
        let stream: Self::DownloadComponentFilesStream = match self
            .download_files(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(response) => {
                let stream = response.map(|content| {
                    let res = match content {
                        Ok(content) => DownloadComponentFilesResponse {
                            result: Some(download_component_files_response::Result::SuccessChunk(
                                content,
                            )),
                        },
                        Err(_) => DownloadComponentFilesResponse {
                            result: Some(download_component_files_response::Result::Error(
                                internal_error("Internal error"),
                            )),
                        },
                    };
                    Ok(res)
                });
                let stream: Self::DownloadComponentFilesStream = Box::pin(stream);
                record.succeed(stream)
            }
            Err(err) => {
                let res = DownloadComponentFilesResponse {
                    result: Some(download_component_files_response::Result::Error(
                        err.clone(),
                    )),
                };

                let stream: Self::DownloadComponentFilesStream =
                    Box::pin(tokio_stream::once(Ok(res)));
                record.fail(stream, &ComponentTraceErrorKind(&err))
            }
        };

        Ok(Response::new(stream))
    }

    async fn get_component_metadata_all_versions(
        &self,
        request: Request<GetComponentRequest>,
    ) -> Result<Response<GetComponentMetadataAllVersionsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_component_metadata_all_versions",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self.get(request).instrument(record.span.clone()).await {
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_latest_component_metadata",
            component_id = proto_component_id_string(&request.component_id),
        );

        let response = match self
            .get_latest_component_metadata(request)
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
        let mut maybe_header = None;
        let mut component_data = Vec::new();

        let mut initial_fs_archive = None;
        let mut temp_dir_for_archive = None;

        let mut request_stream = request.into_inner();
        while let Some(UpdateComponentRequest { data: Some(data) }) =
            request_stream.message().await?
        {
            match data {
                update_component_request::Data::Header(header) => {
                    maybe_header = Some(header);
                }
                update_component_request::Data::Chunk(mut component_chunk) => {
                    component_data.append(&mut component_chunk.component_chunk);
                }
                update_component_request::Data::FilesChunk(files_chunk) => {
                    if temp_dir_for_archive.is_none() {
                        temp_dir_for_archive =
                            Some(tempfile::Builder::new().prefix("golem").tempdir()?);
                    }
                    if initial_fs_archive.is_none() {
                        initial_fs_archive = Some(
                            File::create_new(
                                temp_dir_for_archive
                                    .as_ref()
                                    .unwrap()
                                    .path()
                                    .join("initial_fs_archive.zip"),
                            )
                            .await?,
                        );
                    }
                    initial_fs_archive
                        .as_mut()
                        .unwrap()
                        .write_all(&files_chunk.files_chunk)
                        .await?;
                }
            }
        }

        let record = recorded_grpc_api_request!(
            "update_component",
            component_id = proto_component_id_string(
                &maybe_header
                    .as_ref()
                    .and_then(|header| header.component_id.clone())
            )
        );

        let result = if let Some(request_header) = maybe_header {
            self.update(request_header, component_data, initial_fs_archive)
                .instrument(record.span.clone())
                .await
        } else {
            Err(bad_request_error("Missing request"))
        };

        let result = match result {
            Ok(component) => record.succeed(update_component_response::Result::Success(component)),
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_component_metadata",
            component_id = proto_component_id_string(&request.component_id),
            component_version = request.version.to_string(),
        );

        let response = match self
            .get_component_metadata(request)
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

    async fn create_component_constraints(
        &self,
        request: Request<CreateComponentConstraintsRequest>,
    ) -> Result<Response<CreateComponentConstraintsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("create_component_constraints",);

        match request.component_constraints {
            Some(proto_constraints) => {
                let component_id = match proto_constraints
                    .component_id
                    .and_then(|id| id.try_into().ok())
                    .ok_or_else(|| bad_request_error("Missing component id"))
                {
                    Ok(id) => id,
                    Err(fail) => {
                        return Ok(Response::new(CreateComponentConstraintsResponse {
                            result: Some(record.fail(
                                create_component_constraints_response::Result::Error(fail.clone()),
                                &ComponentTraceErrorKind(&fail),
                            )),
                        }))
                    }
                };

                let constraints = if let Some(worker_functions_in_rib) =
                    proto_constraints.constraints
                {
                    let result = FunctionConstraintCollection::try_from(worker_functions_in_rib)
                        .map_err(|err| bad_request_error(err.as_str()));

                    match result {
                        Ok(worker_functions_in_rib) => worker_functions_in_rib,
                        Err(fail) => {
                            return Ok(Response::new(CreateComponentConstraintsResponse {
                                result: Some(record.fail(
                                    create_component_constraints_response::Result::Error(
                                        fail.clone(),
                                    ),
                                    &ComponentTraceErrorKind(&fail),
                                )),
                            }))
                        }
                    }
                } else {
                    let error = internal_error("Failed to create constraints");
                    return Ok(Response::new(CreateComponentConstraintsResponse {
                        result: Some(record.fail(
                            create_component_constraints_response::Result::Error(error.clone()),
                            &ComponentTraceErrorKind(&error),
                        )),
                    }));
                };

                let component_constraint = ComponentConstraints {
                    namespace: DefaultNamespace::default(),
                    component_id,
                    constraints,
                };

                let response = match self
                    .create_component_constraints(&component_constraint)
                    .instrument(record.span.clone())
                    .await
                {
                    Ok(v) => {
                        record.succeed(create_component_constraints_response::Result::Success(
                            CreateComponentConstraintsSuccessResponse {
                                components: Some(v),
                            },
                        ))
                    }
                    Err(error) => record.fail(
                        create_component_constraints_response::Result::Error(error.clone()),
                        &ComponentTraceErrorKind(&error),
                    ),
                };

                Ok(Response::new(CreateComponentConstraintsResponse {
                    result: Some(response),
                }))
            }

            None => {
                let bad_request = bad_request_error("Missing component constraints");
                let error = record.fail(
                    create_component_constraints_response::Result::Error(bad_request.clone()),
                    &ComponentTraceErrorKind(&bad_request),
                );
                Ok(Response::new(CreateComponentConstraintsResponse {
                    result: Some(error),
                }))
            }
        }
    }
}
