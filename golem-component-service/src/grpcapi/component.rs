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

use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tracing::Instrument;

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentService;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_constraints_response, create_component_request,
    create_component_response, download_component_response,
    get_component_metadata_all_versions_response, get_component_metadata_response,
    get_components_response, get_installed_plugins_response, install_plugin_response,
    uninstall_plugin_response, update_component_request, update_component_response,
    update_installed_plugin_response, ComponentError, CreateComponentConstraintsRequest,
    CreateComponentConstraintsResponse, CreateComponentConstraintsSuccessResponse,
    CreateComponentRequest, CreateComponentRequestHeader, CreateComponentResponse,
    DownloadComponentRequest, DownloadComponentResponse, GetComponentMetadataAllVersionsResponse,
    GetComponentMetadataResponse, GetComponentMetadataSuccessResponse, GetComponentRequest,
    GetComponentSuccessResponse, GetComponentsRequest, GetComponentsResponse,
    GetComponentsSuccessResponse, GetInstalledPluginsRequest, GetInstalledPluginsResponse,
    GetInstalledPluginsSuccessResponse, GetLatestComponentRequest, GetVersionedComponentRequest,
    InstallPluginRequest, InstallPluginResponse, InstallPluginSuccessResponse,
    UninstallPluginRequest, UninstallPluginResponse, UpdateComponentRequest,
    UpdateComponentRequestHeader, UpdateComponentResponse, UpdateInstalledPluginRequest,
    UpdateInstalledPluginResponse,
};
use golem_api_grpc::proto::golem::component::ComponentConstraints as ComponentConstraintsProto;
use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
use golem_api_grpc::proto::golem::component::{Component, PluginInstallation};
use golem_common::grpc::{proto_component_id_string, proto_plugin_installation_id_string};
use golem_common::model::component::DefaultComponentOwner;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginInstallationCreation, PluginInstallationUpdate,
};
use golem_common::model::{ComponentId, ComponentType};
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_component_service_base::model::ComponentConstraints;
use golem_component_service_base::service::component;
use golem_component_service_base::service::plugin::{PluginError, PluginService};
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

pub(crate) fn bad_request_error(error: &str) -> ComponentError {
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
    pub component_service:
        Arc<dyn component::ComponentService<DefaultComponentOwner> + Sync + Send>,
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send>,
}

impl ComponentGrpcApi {
    fn require_component_id(
        source: &Option<golem_api_grpc::proto::golem::component::ComponentId>,
    ) -> Result<ComponentId, ComponentError> {
        match source {
            Some(id) => (*id)
                .try_into()
                .map_err(|err| bad_request_error(&format!("Invalid component id: {err}"))),
            None => Err(bad_request_error("Missing component id")),
        }
    }

    async fn get(&self, request: GetComponentRequest) -> Result<Vec<Component>, ComponentError> {
        let id = Self::require_component_id(&request.component_id)?;
        let result = self
            .component_service
            .get(&id, &DefaultComponentOwner)
            .await?;
        Ok(result.into_iter().map(Component::from).collect())
    }

    async fn get_component_metadata(
        &self,
        request: GetVersionedComponentRequest,
    ) -> Result<Option<Component>, ComponentError> {
        let id = Self::require_component_id(&request.component_id)?;

        let version = request.version;

        let versioned_component_id = golem_service_base::model::VersionedComponentId {
            component_id: id,
            version,
        };

        let result = self
            .component_service
            .get_by_version(&versioned_component_id, &DefaultComponentOwner)
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
            .find_by_name(name, &DefaultComponentOwner)
            .await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }

    async fn get_latest_component_metadata(
        &self,
        request: GetLatestComponentRequest,
    ) -> Result<Component, ComponentError> {
        let id = Self::require_component_id(&request.component_id)?;
        let result = self
            .component_service
            .get_latest_version(&id, &DefaultComponentOwner)
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
        let id = Self::require_component_id(&request.component_id)?;
        let version = request.version;
        let result = self
            .component_service
            .download_stream(&id, version, &DefaultComponentOwner)
            .await?;
        Ok(result)
    }

    async fn create(
        &self,
        component_id: ComponentId,
        request: CreateComponentRequestHeader,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError> {
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
                &component_id,
                &name,
                request.component_type().into(),
                data,
                files,
                vec![],
                dynamic_linking,
                &DefaultComponentOwner,
            )
            .await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: UpdateComponentRequestHeader,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError> {
        let id = Self::require_component_id(&request.component_id)?;

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
                &DefaultComponentOwner,
            )
            .await?;
        Ok(result.into())
    }

    async fn create_component_constraints(
        &self,
        component_constraint: &ComponentConstraints<DefaultComponentOwner>,
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

    async fn get_installed_plugins(
        &self,
        request: &GetInstalledPluginsRequest,
    ) -> Result<Vec<PluginInstallation>, ComponentError> {
        let component_id = Self::require_component_id(&request.component_id)?;

        let version = match &request.version {
            Some(version) => *version,
            None => self
                .component_service
                .get_latest_version(&component_id, &DefaultComponentOwner)
                .await?
                .map(|v| v.versioned_component_id.version)
                .ok_or_else(|| bad_request_error("Component not found"))?,
        };

        let response = self
            .component_service
            .get_plugin_installations_for_component(&DefaultComponentOwner, &component_id, version)
            .await?;

        Ok(response.into_iter().map(|v| v.into()).collect())
    }

    async fn install_plugin(
        &self,
        request: &InstallPluginRequest,
    ) -> Result<PluginInstallation, ComponentError> {
        let component_id = Self::require_component_id(&request.component_id)?;

        let plugin_installation_creation: PluginInstallationCreation = PluginInstallationCreation {
            name: request.name.clone(),
            version: request.version.clone(),
            priority: request.priority,
            parameters: request.parameters.clone(),
        };

        let plugin_definition = self
            .plugin_service
            .get(
                &DefaultPluginOwner,
                &plugin_installation_creation.name,
                &plugin_installation_creation.version,
            )
            .await?;

        let response = if let Some(plugin_definition) = plugin_definition {
            if plugin_definition.scope.valid_in_component(&component_id) {
                self.component_service
                    .create_plugin_installation_for_component(
                        &DefaultComponentOwner,
                        &component_id,
                        plugin_installation_creation.clone(),
                    )
                    .await
            } else {
                Err(PluginError::InvalidScope {
                    plugin_name: plugin_installation_creation.name,
                    plugin_version: plugin_installation_creation.version,
                    details: format!("not available for component {}", component_id),
                })
            }
        } else {
            Err(PluginError::PluginNotFound {
                plugin_name: plugin_installation_creation.name,
                plugin_version: plugin_installation_creation.version,
            })
        }?;

        Ok(response.into())
    }

    async fn update_installed_plugin(
        &self,
        request: &UpdateInstalledPluginRequest,
    ) -> Result<(), ComponentError> {
        let component_id = Self::require_component_id(&request.component_id)?;

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
                &DefaultComponentOwner,
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
    ) -> Result<(), ComponentError> {
        let component_id = Self::require_component_id(&request.component_id)?;

        let installation_id = request
            .installation_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing installation id"))?;

        self.component_service
            .delete_plugin_installation_for_component(
                &DefaultComponentOwner,
                &installation_id,
                &component_id,
            )
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

        let component_id = ComponentId::new_v4();
        let record = recorded_grpc_api_request!(
            "create_component",
            component_name = header.as_ref().map(|r| r.component_name.clone()),
            component_id = component_id.to_string(),
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
                self.create(component_id, request, data)
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
                self.update(request, data)
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
                    owner: DefaultComponentOwner,
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

    async fn get_installed_plugins(
        &self,
        request: Request<GetInstalledPluginsRequest>,
    ) -> Result<Response<GetInstalledPluginsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_installed_plugins",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self
            .get_installed_plugins(&request)
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "install_plugin",
            component_id = proto_component_id_string(&request.component_id),
            plugin_name = request.name.clone(),
            plugin_version = request.version.clone()
        );

        let response = match self
            .install_plugin(&request)
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_installed_plugin",
            component_id = proto_component_id_string(&request.component_id),
            installation_id = proto_plugin_installation_id_string(&request.installation_id)
        );

        let response = match self
            .update_installed_plugin(&request)
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "uninstall_plugin",
            component_id = proto_component_id_string(&request.component_id),
            installation_id = proto_plugin_installation_id_string(&request.installation_id)
        );

        let response = match self
            .uninstall_plugin(&request)
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
