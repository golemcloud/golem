use crate::grpcapi::{auth, bad_request_error, internal_error, require_component_id};
use crate::service;
use crate::service::component::CloudComponentService;
use async_trait::async_trait;
use cloud_common::grpc::proto_project_id_string;
use cloud_common::model::CloudComponentOwner;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentService;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_constraints_response, create_component_request,
    create_component_response, delete_component_constraints_response, download_component_response,
    get_component_metadata_all_versions_response, get_component_metadata_response,
    get_components_response, get_installed_plugins_response, install_plugin_response,
    uninstall_plugin_response, update_component_request, update_component_response,
    update_installed_plugin_response, ComponentError, CreateComponentConstraintsRequest,
    CreateComponentConstraintsResponse, CreateComponentConstraintsSuccessResponse,
    CreateComponentRequest, CreateComponentRequestHeader, CreateComponentResponse,
    DeleteComponentConstraintsRequest, DeleteComponentConstraintsResponse,
    DeleteComponentConstraintsSuccessResponse, DownloadComponentRequest, DownloadComponentResponse,
    GetComponentMetadataAllVersionsResponse, GetComponentMetadataResponse,
    GetComponentMetadataSuccessResponse, GetComponentRequest, GetComponentSuccessResponse,
    GetComponentsRequest, GetComponentsResponse, GetComponentsSuccessResponse,
    GetInstalledPluginsRequest, GetInstalledPluginsResponse, GetInstalledPluginsSuccessResponse,
    GetLatestComponentRequest, GetVersionedComponentRequest, InstallPluginRequest,
    InstallPluginResponse, InstallPluginSuccessResponse, UninstallPluginRequest,
    UninstallPluginResponse, UpdateComponentRequest, UpdateComponentRequestHeader,
    UpdateComponentResponse, UpdateInstalledPluginRequest, UpdateInstalledPluginResponse,
};
use golem_api_grpc::proto::golem::component::{Component, PluginInstallation};
use golem_common::grpc::{proto_component_id_string, proto_plugin_installation_id_string};
use golem_common::model::component_constraint::FunctionConstraints;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::{PluginInstallationCreation, PluginInstallationUpdate};
use golem_common::model::ProjectId;
use golem_common::model::{ComponentId, ComponentType};
use golem_common::recorded_grpc_api_request;
use golem_common::SafeDisplay;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_component_service_base::service::component::ComponentError as BaseComponentError;
use golem_component_service_base::service::plugin::PluginError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};
use tracing::Instrument;

impl From<service::CloudComponentError> for ComponentError {
    fn from(value: service::CloudComponentError) -> Self {
        let error = match value {
            service::CloudComponentError::Unauthorized(_) => {
                component_error::Error::Unauthorized(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            service::CloudComponentError::BaseComponentError(
                BaseComponentError::ComponentProcessingError(_),
            ) => component_error::Error::BadRequest(ErrorsBody {
                errors: vec![value.to_safe_string()],
            }),
            service::CloudComponentError::LimitExceeded(_) => {
                component_error::Error::LimitExceeded(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            service::CloudComponentError::BaseComponentError(
                BaseComponentError::AlreadyExists(_),
            ) => component_error::Error::AlreadyExists(ErrorBody {
                error: value.to_safe_string(),
            }),
            service::CloudComponentError::BaseComponentError(
                BaseComponentError::UnknownComponentId(_),
            )
            | service::CloudComponentError::BaseComponentError(
                BaseComponentError::UnknownVersionedComponentId(_),
            )
            | service::CloudComponentError::UnknownProject(_)
            | service::CloudComponentError::BasePluginError(PluginError::ComponentNotFound {
                ..
            }) => component_error::Error::NotFound(ErrorBody {
                error: value.to_safe_string(),
            }),
            service::CloudComponentError::InternalAuthServiceError(_)
            | service::CloudComponentError::BaseComponentError(_)
            | service::CloudComponentError::BasePluginError(_)
            | service::CloudComponentError::InternalLimitError(_)
            | service::CloudComponentError::InternalProjectError(_) => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        ComponentError { error: Some(error) }
    }
}

pub struct ComponentGrpcApi {
    component_service: Arc<CloudComponentService>,
}

impl ComponentGrpcApi {
    pub fn new(component_service: Arc<CloudComponentService>) -> Self {
        Self { component_service }
    }

    async fn get(
        &self,
        request: GetComponentRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<Component>, ComponentError> {
        let auth = auth(metadata)?;
        let id = require_component_id(&request.component_id)?;
        let result = self.component_service.get(&id, &auth).await?;
        Ok(result.into_iter().map(component_to_grpc).collect())
    }

    async fn get_component_metadata(
        &self,
        request: GetVersionedComponentRequest,
        metadata: MetadataMap,
    ) -> Result<Option<Component>, ComponentError> {
        let auth = auth(metadata)?;

        let id = require_component_id(&request.component_id)?;
        let version = request.version;

        let versioned_component_id = golem_common::model::component::VersionedComponentId {
            component_id: id,
            version,
        };

        let result = self
            .component_service
            .get_by_version(&versioned_component_id, &auth)
            .await?;
        Ok(result.map(component_to_grpc))
    }

    async fn get_all(
        &self,
        request: GetComponentsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<Component>, ComponentError> {
        let auth = auth(metadata)?;
        let project_id: Option<ProjectId> = request.project_id.and_then(|id| id.try_into().ok());
        let name: Option<golem_service_base::model::ComponentName> = request
            .component_name
            .map(golem_service_base::model::ComponentName);
        let result = self
            .component_service
            .find_by_project_and_name(project_id, name, &auth)
            .await?;
        Ok(result.into_iter().map(component_to_grpc).collect())
    }

    async fn get_latest_component_metadata(
        &self,
        request: GetLatestComponentRequest,
        metadata: MetadataMap,
    ) -> Result<Component, ComponentError> {
        let auth = auth(metadata)?;
        let id = require_component_id(&request.component_id)?;
        let result = self
            .component_service
            .get_latest_version(&id, &auth)
            .await?;
        match result {
            Some(component) => Ok(component_to_grpc(component)),
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
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
        let auth = auth(metadata)?;
        let id = require_component_id(&request.component_id)?;
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
        let auth = auth(metadata)?;
        let project_id: Option<ProjectId> = request.project_id.and_then(|id| id.try_into().ok());
        let name = golem_service_base::model::ComponentName(request.component_name.clone());
        let files = request
            .files
            .iter()
            .map(|f| f.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e: String| bad_request_error(&format!("Failed reading files: {e}")))?;

        let dynamic_linking: HashMap<String, DynamicLinkedInstance> = HashMap::from_iter(
            request
                .dynamic_linking
                .iter()
                .map(|(k, v)| v.clone().try_into().map(|v| (k.clone(), v)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e: String| {
                    bad_request_error(&format!("Invalid dynamic linking information: {e}"))
                })?,
        );

        let result = self
            .component_service
            .create_internal(
                project_id,
                &name,
                request.component_type().into(),
                data,
                files,
                dynamic_linking,
                &auth,
                request.env,
            )
            .await?;
        Ok(component_to_grpc(result))
    }

    async fn update(
        &self,
        request: UpdateComponentRequestHeader,
        data: Vec<u8>,
        metadata: MetadataMap,
    ) -> Result<Component, ComponentError> {
        let auth = auth(metadata)?;
        let id = require_component_id(&request.component_id)?;
        let component_type = match request.component_type {
            Some(n) => Some(
                ComponentType::try_from(n)
                    .map_err(|_| bad_request_error("Invalid component type"))?,
            ),
            None => None,
        };
        let files = if request.update_files {
            let value = request
                .files
                .iter()
                .map(|f| f.clone().try_into())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e: String| bad_request_error(&format!("Failed reading files: {e}")))?;
            Some(value)
        } else {
            None
        };

        let dynamic_linking: HashMap<String, DynamicLinkedInstance> = HashMap::from_iter(
            request
                .dynamic_linking
                .into_iter()
                .map(|(k, v)| v.try_into().map(|v| (k, v)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e: String| {
                    bad_request_error(&format!("Invalid dynamic linking information: {e}"))
                })?,
        );

        let result = self
            .component_service
            .update_internal(
                &id,
                data,
                component_type,
                files,
                dynamic_linking,
                &auth,
                request.env,
            )
            .await?;
        Ok(component_to_grpc(result))
    }

    async fn create_component_constraints(
        &self,
        component_id: ComponentId,
        constraints: FunctionConstraints,
        metadata: MetadataMap,
    ) -> Result<golem_api_grpc::proto::golem::component::ComponentConstraints, ComponentError> {
        let auth = auth(metadata)?;

        let response = self
            .component_service
            .create_or_update_constraint(component_id, constraints, &auth)
            .await
            .map(
                |v| golem_api_grpc::proto::golem::component::ComponentConstraints {
                    component_id: Some(v.component_id.into()),
                    constraints: Some(
                        golem_api_grpc::proto::golem::component::FunctionConstraintCollection::from(
                            v.constraints,
                        ),
                    ),
                },
            )?;

        Ok(response)
    }

    async fn delete_component_constraints(
        &self,
        component_id: ComponentId,
        constraints: FunctionConstraints,
        metadata: MetadataMap,
    ) -> Result<golem_api_grpc::proto::golem::component::ComponentConstraints, ComponentError> {
        let auth = auth(metadata)?;

        let response = self
            .component_service
            .delete_constraints(component_id, constraints, &auth)
            .await
            .map(
                |v| golem_api_grpc::proto::golem::component::ComponentConstraints {
                    component_id: Some(v.component_id.into()),
                    constraints: Some(
                        golem_api_grpc::proto::golem::component::FunctionConstraintCollection::from(
                            v.constraints,
                        ),
                    ),
                },
            )?;

        Ok(response)
    }

    async fn get_installed_plugins(
        &self,
        request: &GetInstalledPluginsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<PluginInstallation>, ComponentError> {
        let auth = auth(metadata)?;
        let component_id = require_component_id(&request.component_id)?;

        let version = match &request.version {
            Some(version) => *version,
            None => self
                .component_service
                .get_latest_version(&component_id, &auth)
                .await?
                .map(|v| v.versioned_component_id.version)
                .ok_or_else(|| bad_request_error("Component not found"))?,
        };

        let (_, response) = self
            .component_service
            .get_plugin_installations_for_component(&auth, &component_id, version)
            .await?;

        Ok(response.into_iter().map(|v| v.into()).collect())
    }

    async fn install_plugin(
        &self,
        request: &InstallPluginRequest,
        metadata: MetadataMap,
    ) -> Result<PluginInstallation, ComponentError> {
        let auth = auth(metadata)?;

        let component_id = require_component_id(&request.component_id)?;

        let plugin_installation_creation: PluginInstallationCreation = PluginInstallationCreation {
            name: request.name.clone(),
            version: request.version.clone(),
            priority: request.priority,
            parameters: request.parameters.clone(),
        };

        let (_, response) = self
            .component_service
            .create_plugin_installation_for_component(
                &auth,
                &component_id,
                plugin_installation_creation,
            )
            .await?;

        Ok(response.into())
    }

    async fn update_installed_plugin(
        &self,
        request: &UpdateInstalledPluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), ComponentError> {
        let auth = auth(metadata)?;

        let component_id = require_component_id(&request.component_id)?;

        let installation_id = request
            .installation_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing installation id"))?;

        let update = PluginInstallationUpdate {
            priority: request.updated_priority,
            parameters: request.updated_parameters.clone(),
        };

        self.component_service
            .update_plugin_installation_for_component(
                &auth,
                &installation_id,
                &component_id,
                update,
            )
            .await?;

        Ok(())
    }

    async fn uninstall_plugin(
        &self,
        request: &UninstallPluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), ComponentError> {
        let auth = auth(metadata)?;

        let component_id = require_component_id(&request.component_id)?;

        let installation_id = request
            .installation_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing installation id"))?;

        self.component_service
            .delete_plugin_installation_for_component(&auth, &installation_id, &component_id)
            .await?;

        Ok(())
    }
}

#[async_trait]
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
            project_id = proto_project_id_string(&header.as_ref().and_then(|r| r.project_id))
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
            component_id = proto_component_id_string(&header.as_ref().and_then(|r| r.component_id))
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

    async fn create_component_constraints(
        &self,
        request: Request<CreateComponentConstraintsRequest>,
    ) -> Result<Response<CreateComponentConstraintsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "create_constraints",
            component_id = &proto_component_id_string(
                &r.component_constraints
                    .as_ref()
                    .and_then(|c| c.component_id)
            )
        );

        match r.component_constraints {
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

                let constraints =
                    if let Some(worker_functions_in_rib) = proto_constraints.constraints {
                        let result = FunctionConstraints::try_from(worker_functions_in_rib)
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

                let response = match self
                    .create_component_constraints(component_id, constraints, m)
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

    async fn delete_component_constraint(
        &self,
        request: Request<DeleteComponentConstraintsRequest>,
    ) -> Result<Response<DeleteComponentConstraintsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "delete_component_constraints",
            component_id = &proto_component_id_string(
                &r.component_constraints
                    .as_ref()
                    .and_then(|c| c.component_id)
            )
        );

        match r.component_constraints {
            Some(proto_constraints) => {
                let component_id = match proto_constraints
                    .component_id
                    .and_then(|id| id.try_into().ok())
                    .ok_or_else(|| bad_request_error("Missing component id"))
                {
                    Ok(id) => id,
                    Err(fail) => {
                        return Ok(Response::new(DeleteComponentConstraintsResponse {
                            result: Some(record.fail(
                                delete_component_constraints_response::Result::Error(fail.clone()),
                                &ComponentTraceErrorKind(&fail),
                            )),
                        }))
                    }
                };

                let constraints = if let Some(function_constraints) = proto_constraints.constraints
                {
                    let result = FunctionConstraints::try_from(function_constraints)
                        .map_err(|err| bad_request_error(err.as_str()));

                    match result {
                        Ok(function_constraints) => function_constraints,
                        Err(fail) => {
                            return Ok(Response::new(DeleteComponentConstraintsResponse {
                                result: Some(record.fail(
                                    delete_component_constraints_response::Result::Error(
                                        fail.clone(),
                                    ),
                                    &ComponentTraceErrorKind(&fail),
                                )),
                            }))
                        }
                    }
                } else {
                    let error = internal_error("Failed to create constraints");
                    return Ok(Response::new(DeleteComponentConstraintsResponse {
                        result: Some(record.fail(
                            delete_component_constraints_response::Result::Error(error.clone()),
                            &ComponentTraceErrorKind(&error),
                        )),
                    }));
                };

                let response = match self
                    .delete_component_constraints(component_id, constraints, m)
                    .instrument(record.span.clone())
                    .await
                {
                    Ok(v) => {
                        record.succeed(delete_component_constraints_response::Result::Success(
                            DeleteComponentConstraintsSuccessResponse {
                                components: Some(v),
                            },
                        ))
                    }
                    Err(error) => record.fail(
                        delete_component_constraints_response::Result::Error(error.clone()),
                        &ComponentTraceErrorKind(&error),
                    ),
                };

                Ok(Response::new(DeleteComponentConstraintsResponse {
                    result: Some(response),
                }))
            }

            None => {
                let bad_request = bad_request_error("Missing component constraints");
                let error = record.fail(
                    delete_component_constraints_response::Result::Error(bad_request.clone()),
                    &ComponentTraceErrorKind(&bad_request),
                );
                Ok(Response::new(DeleteComponentConstraintsResponse {
                    result: Some(error),
                }))
            }
        }
    }

    async fn get_installed_plugins(
        &self,
        request: Request<GetInstalledPluginsRequest>,
    ) -> Result<Response<GetInstalledPluginsResponse>, Status> {
        let (metadata, _, request) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_installed_plugins",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self
            .get_installed_plugins(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(installations) => record.succeed(get_installed_plugins_response::Result::Success(
                GetInstalledPluginsSuccessResponse { installations },
            )),
            Err(error) => record.fail(
                get_installed_plugins_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetInstalledPluginsResponse {
            result: Some(response),
        }))
    }

    async fn install_plugin(
        &self,
        request: Request<InstallPluginRequest>,
    ) -> Result<Response<InstallPluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "install_plugin",
            component_id = proto_component_id_string(&request.component_id),
            plugin_name = request.name.clone(),
            plugin_version = request.version.clone()
        );

        let response = match self
            .install_plugin(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(installation) => record.succeed(install_plugin_response::Result::Success(
                InstallPluginSuccessResponse {
                    installation: Some(installation),
                },
            )),
            Err(error) => record.fail(
                install_plugin_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InstallPluginResponse {
            result: Some(response),
        }))
    }

    async fn update_installed_plugin(
        &self,
        request: Request<UpdateInstalledPluginRequest>,
    ) -> Result<Response<UpdateInstalledPluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "update_installed_plugin",
            component_id = proto_component_id_string(&request.component_id),
            installation_id = proto_plugin_installation_id_string(&request.installation_id)
        );

        let response = match self
            .update_installed_plugin(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(update_installed_plugin_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                update_installed_plugin_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateInstalledPluginResponse {
            result: Some(response),
        }))
    }

    async fn uninstall_plugin(
        &self,
        request: Request<UninstallPluginRequest>,
    ) -> Result<Response<UninstallPluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "uninstall_plugin",
            component_id = proto_component_id_string(&request.component_id),
            installation_id = proto_plugin_installation_id_string(&request.installation_id)
        );

        let response = match self
            .uninstall_plugin(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(uninstall_plugin_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                uninstall_plugin_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UninstallPluginResponse {
            result: Some(response),
        }))
    }
}

fn component_to_grpc(
    value: golem_component_service_base::model::Component<CloudComponentOwner>,
) -> golem_api_grpc::proto::golem::component::Component {
    let component_type: golem_api_grpc::proto::golem::component::ComponentType =
        value.component_type.into();

    golem_api_grpc::proto::golem::component::Component {
        versioned_component_id: Some(value.versioned_component_id.into()),
        component_name: value.component_name.0,
        component_size: value.component_size,
        metadata: Some(value.metadata.into()),
        account_id: Some(value.owner.account_id.into()),
        project_id: Some(value.owner.project_id.into()),
        created_at: Some(prost_types::Timestamp::from(SystemTime::from(
            value.created_at,
        ))),
        component_type: Some(component_type.into()),
        files: value.files.into_iter().map(|file| file.into()).collect(),
        installed_plugins: value
            .installed_plugins
            .into_iter()
            .map(|plugin| plugin.into())
            .collect(),
        env: value.env,
    }
}
