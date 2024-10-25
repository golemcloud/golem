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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::Instrument;

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentService;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_request, create_component_response,
    download_component_initial_file_response, download_component_response,
    get_component_initial_file_response, get_component_metadata_all_versions_response,
    get_component_metadata_response, get_components_response, update_component_request,
    update_component_response, ComponentError, CreateComponentRequest,
    CreateComponentRequestHeader, CreateComponentResponse, DownloadComponentInitialFileRequest,
    DownloadComponentInitialFileResponse, DownloadComponentRequest, DownloadComponentResponse,
    GetComponentInitialFileResponse, GetComponentInitialFileSuccessResponse,
    GetComponentInitialFilesRequest, GetComponentMetadataAllVersionsResponse,
    GetComponentMetadataResponse, GetComponentMetadataSuccessResponse, GetComponentRequest,
    GetComponentSuccessResponse, GetComponentsRequest, GetComponentsResponse,
    GetComponentsSuccessResponse, GetLatestComponentInitialFilesRequest, GetLatestComponentRequest,
    GetVersionedComponentRequest, UpdateComponentRequest, UpdateComponentRequestHeader,
    UpdateComponentResponse,
};
use golem_api_grpc::proto::golem::component::{Component, ComponentInitialFile};
use golem_common::grpc::proto_component_id_string;
use golem_common::model::{ComponentId, ComponentType};
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_component_service_base::service::component;
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::model::InitialFile;
use golem_service_base::stream::ByteStream;
use tap::TapFallible;
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
        initial_files: Vec<InitialFile>,
    ) -> Result<Component, ComponentError> {
        let name = golem_service_base::model::ComponentName(request.component_name.clone());
        let result = self
            .component_service
            .create(
                &ComponentId::new_v4(),
                &name,
                request.component_type().into(),
                data,
                &DefaultNamespace::default(),
                initial_files,
            )
            .await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: UpdateComponentRequestHeader,
        data: Vec<u8>,
        initial_files: Vec<InitialFile>,
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
                &DefaultNamespace::default(),
                initial_files,
            )
            .await?;
        Ok(result.into())
    }

    async fn get_version_component_initial_files(
        &self,
        request: GetComponentInitialFilesRequest,
    ) -> Result<Vec<ComponentInitialFile>, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let versioned_component_id = golem_service_base::model::VersionedComponentId {
            component_id: id,
            version: request.version,
        };
        let result = self
            .component_service
            .get_version_initial_files(&versioned_component_id, &DefaultNamespace::default())
            .await?;
        result
            .into_iter()
            .map(|f| {
                f.try_into().map_err(|e| ComponentError {
                    error: Some(component_error::Error::InternalError(ErrorBody {
                        error: e,
                    })),
                })
            })
            .try_fold(vec![], |mut acc, result| {
                let initial_file = result?;
                acc.push(initial_file);
                Ok(acc)
            })
    }

    async fn get_latest_component_initial_files(
        &self,
        request: GetLatestComponentInitialFilesRequest,
    ) -> Result<Vec<ComponentInitialFile>, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let result = self
            .component_service
            .get_latest_version_initial_files(&id, &DefaultNamespace::default())
            .await?;
        result
            .into_iter()
            .map(|f| {
                f.try_into().map_err(|e| ComponentError {
                    error: Some(component_error::Error::InternalError(ErrorBody {
                        error: e,
                    })),
                })
            })
            .try_fold(vec![], |mut acc, result| {
                let initial_file = result?;
                acc.push(initial_file);
                Ok(acc)
            })
    }

    async fn download_initial_file(
        &self,
        request: DownloadComponentInitialFileRequest,
    ) -> Result<ByteStream, ComponentError> {
        let id: ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;
        let version = request.version;
        let file_path = PathBuf::from(request.file_path);
        let result = self
            .component_service
            .download_initial_file_stream(
                &id,
                version,
                file_path.as_path(),
                &DefaultNamespace::default(),
            )
            .await?;
        Ok(result)
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
        let chunks: Vec<CreateComponentRequest> =
            request.into_inner().into_stream().try_collect().await?;
        let header = chunks.iter().find_map(|c| {
            c.clone().data.and_then(|d| match d {
                create_component_request::Data::Header(d) => Some(d),
                _ => None,
            })
        });

        let record = recorded_grpc_api_request!(
            "create_component",
            component_name = header.as_ref().map(|r| r.component_name.clone())
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
                let initial_file_data_map =
                    chunks.iter().try_fold(HashMap::new(), |mut acc_map, c| {
                        if let Some(create_component_request::Data::FileHeader(d)) = c.clone().data
                        {
                            if acc_map.contains_key(&d.file_path) {
                                Err(bad_request_error("Duplicated file path"))
                            } else {
                                acc_map.insert(
                                    d.file_path.clone(),
                                    InitialFile {
                                        file_path: d.file_path.clone(),
                                        file_permission: d
                                            .file_permission
                                            .try_into()
                                            .map_err(|e: String| bad_request_error(&e))?,
                                        file_content: Vec::new(),
                                    },
                                );
                                Ok(acc_map)
                            }
                        } else {
                            Ok(acc_map)
                        }
                    });
                let initial_file_data_map =
                    initial_file_data_map.tap_ok_mut(|initial_file_data_map| {
                        chunks.iter().for_each(|c| {
                            if let Some(create_component_request::Data::FileChunk(mut d)) =
                                c.clone().data
                            {
                                if let Some(initial_file) =
                                    initial_file_data_map.get_mut(&d.file_path)
                                {
                                    initial_file.file_content.append(&mut d.file_chunk)
                                }
                            }
                        });
                    });
                match initial_file_data_map {
                    Ok(initial_file_data_map) => {
                        let initial_file_data = initial_file_data_map.into_values().collect();
                        self.create(request, data, initial_file_data)
                            .instrument(record.span.clone())
                            .await
                    }
                    Err(error) => Err(error),
                }
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component",
            component_id = proto_component_id_string(&request.component_id)
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
            component_id = proto_component_id_string(&request.component_id)
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
        let chunks: Vec<UpdateComponentRequest> =
            request.into_inner().into_stream().try_collect().await?;

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
                let initial_file_data_map =
                    chunks.iter().try_fold(HashMap::new(), |mut acc_map, c| {
                        if let Some(update_component_request::Data::FileHeader(d)) = c.clone().data
                        {
                            if acc_map.contains_key(&d.file_path) {
                                Err(bad_request_error("Duplicated file path"))
                            } else {
                                acc_map.insert(
                                    d.file_path.clone(),
                                    InitialFile {
                                        file_path: d.file_path.clone(),
                                        file_permission: d
                                            .file_permission
                                            .try_into()
                                            .map_err(|e: String| bad_request_error(&e))?,
                                        file_content: Vec::new(),
                                    },
                                );
                                Ok(acc_map)
                            }
                        } else {
                            Ok(acc_map)
                        }
                    });
                let initial_file_data_map =
                    initial_file_data_map.tap_ok_mut(|initial_file_data_map| {
                        chunks.iter().for_each(|c| {
                            if let Some(update_component_request::Data::FileChunk(mut d)) =
                                c.clone().data
                            {
                                if let Some(initial_file) =
                                    initial_file_data_map.get_mut(&d.file_path)
                                {
                                    initial_file.file_content.append(&mut d.file_chunk)
                                }
                            }
                        });
                    });
                match initial_file_data_map {
                    Ok(initial_file_data_map) => {
                        let initial_file_data = initial_file_data_map.into_values().collect();
                        self.update(request, data, initial_file_data)
                            .instrument(record.span.clone())
                            .await
                    }
                    Err(error) => Err(error),
                }
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_component_metadata",
            component_id = proto_component_id_string(&request.component_id)
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

    async fn get_latest_component_initial_files(
        &self,
        request: Request<GetLatestComponentInitialFilesRequest>,
    ) -> Result<Response<GetComponentInitialFileResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_latest_component_initial_files",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self
            .get_latest_component_initial_files(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(initial_files) => {
                record.succeed(get_component_initial_file_response::Result::Success(
                    GetComponentInitialFileSuccessResponse { initial_files },
                ))
            }
            Err(error) => record.fail(
                get_component_initial_file_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetComponentInitialFileResponse {
            result: Some(response),
        }))
    }

    async fn get_component_initial_files(
        &self,
        request: Request<GetComponentInitialFilesRequest>,
    ) -> Result<Response<GetComponentInitialFileResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_component_initial_files",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self
            .get_version_component_initial_files(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(initial_files) => {
                record.succeed(get_component_initial_file_response::Result::Success(
                    GetComponentInitialFileSuccessResponse { initial_files },
                ))
            }
            Err(error) => record.fail(
                get_component_initial_file_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetComponentInitialFileResponse {
            result: Some(response),
        }))
    }

    type DownloadComponentInitialFileStream =
        BoxStream<'static, Result<DownloadComponentInitialFileResponse, Status>>;

    async fn download_component_initial_file(
        &self,
        request: Request<DownloadComponentInitialFileRequest>,
    ) -> Result<Response<Self::DownloadComponentInitialFileStream>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component_initial_file",
            component_id = proto_component_id_string(&request.component_id)
        );
        let stream: Self::DownloadComponentInitialFileStream = match self
            .download_initial_file(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(response) => {
                let stream = response.map(|content| {
                    let res = match content {
                        Ok(content) => DownloadComponentInitialFileResponse {
                            result: Some(
                                download_component_initial_file_response::Result::SuccessChunk(
                                    content,
                                ),
                            ),
                        },
                        Err(_) => DownloadComponentInitialFileResponse {
                            result: Some(download_component_initial_file_response::Result::Error(
                                internal_error("Internal error"),
                            )),
                        },
                    };
                    Ok(res)
                });
                let stream: Self::DownloadComponentInitialFileStream = Box::pin(stream);
                record.succeed(stream)
            }
            Err(err) => {
                let res = DownloadComponentInitialFileResponse {
                    result: Some(download_component_initial_file_response::Result::Error(
                        err.clone(),
                    )),
                };

                let stream: Self::DownloadComponentInitialFileStream =
                    Box::pin(tokio_stream::iter([Ok(res)]));
                record.fail(stream, &ComponentTraceErrorKind(&err))
            }
        };

        Ok(Response::new(stream))
    }
}
