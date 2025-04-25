use crate::grpcapi::{auth, bad_request_error};
use crate::service::plugin::CloudPluginService;
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::component::v1::plugin_service_server::PluginService;
use cloud_api_grpc::proto::golem::cloud::component::v1::{
    get_plugin_by_id_response, get_plugin_response, list_plugins_response, CreatePluginRequest,
    GetPluginByIdRequest, GetPluginByIdResponse, GetPluginResponse, GetPluginSuccessResponse,
    ListPluginsRequest, ListPluginsResponse, ListPluginsSuccessResponse,
};
use cloud_api_grpc::proto::golem::cloud::component::PluginDefinition;
use cloud_common::grpc::plugin_definition_to_grpc;
use cloud_common::model::CloudPluginScope;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody};
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_plugin_response, delete_plugin_response, ComponentError,
    CreatePluginResponse, DeletePluginRequest, DeletePluginResponse, GetPluginRequest,
    ListPluginVersionsRequest,
};
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_component_service_base::model::plugin::PluginDefinitionCreation;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

pub struct PluginGrpcApi {
    plugin_service: Arc<CloudPluginService>,
}

impl PluginGrpcApi {
    pub fn new(plugin_service: Arc<CloudPluginService>) -> Self {
        Self { plugin_service }
    }

    async fn list_plugins(
        &self,
        request: &ListPluginsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let auth = auth(metadata)?;

        let plugins = match &request.scope {
            Some(scope) => {
                let scope = scope
                    .clone()
                    .try_into()
                    .map_err(|err| bad_request_error(&format!("Invalid plugin scope: {err}")))?;

                self.plugin_service
                    .list_plugins_for_scope(&auth, &scope)
                    .await?
            }
            None => self.plugin_service.list_plugins(&auth).await?,
        };

        Ok(plugins.into_iter().map(plugin_definition_to_grpc).collect())
    }

    async fn list_plugin_versions(
        &self,
        request: &ListPluginVersionsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let auth = auth(metadata)?;

        let plugins = self
            .plugin_service
            .list_plugin_versions(&auth, &request.name)
            .await?;

        Ok(plugins.into_iter().map(plugin_definition_to_grpc).collect())
    }

    async fn create_plugin(
        &self,
        request: &CreatePluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), ComponentError> {
        let auth = auth(metadata)?;

        let plugin = grpc_to_plugin_creation(request.clone())
            .map_err(|err| bad_request_error(&format!("Invalid plugin specification: {err}")))?;

        self.plugin_service.create_plugin(&auth, plugin).await?;

        Ok(())
    }

    async fn get_plugin(
        &self,
        request: &GetPluginRequest,
        metadata: MetadataMap,
    ) -> Result<PluginDefinition, ComponentError> {
        let auth = auth(metadata)?;

        let plugin = self
            .plugin_service
            .get(&auth, &request.name, &request.version)
            .await?;

        match plugin {
            Some(plugin) => Ok(plugin_definition_to_grpc(plugin)),
            None => Err(ComponentError {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: "Plugin not found".to_string(),
                })),
            }),
        }
    }

    async fn delete_plugin(
        &self,
        request: &DeletePluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), ComponentError> {
        let auth = auth(metadata)?;

        self.plugin_service
            .delete(&auth, &request.name, &request.version)
            .await?;

        Ok(())
    }

    async fn get_plugin_by_id(
        &self,
        request: &GetPluginByIdRequest,
        metadata: MetadataMap,
    ) -> Result<PluginDefinition, ComponentError> {
        let auth = auth(metadata)?;

        let plugin_id = &request
            .id
            .ok_or(bad_request_error("Missing plugin id"))?
            .try_into()
            .map_err(|err| bad_request_error(&format!("Invalid plugin id: {err}")))?;

        let plugin = self.plugin_service.get_by_id(&auth, plugin_id).await?;

        match plugin {
            Some(plugin) => Ok(plugin_definition_to_grpc(plugin)),
            None => Err(ComponentError {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: "Plugin not found".to_string(),
                })),
            }),
        }
    }
}

#[async_trait]
impl PluginService for PluginGrpcApi {
    async fn list_plugins(
        &self,
        request: Request<ListPluginsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!("list_plugins",);

        let response = match self
            .list_plugins(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(plugins) => record.succeed(list_plugins_response::Result::Success(
                ListPluginsSuccessResponse { plugins },
            )),
            Err(error) => record.fail(
                list_plugins_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ListPluginsResponse {
            result: Some(response),
        }))
    }

    async fn list_plugin_versions(
        &self,
        request: Request<ListPluginVersionsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!("list_plugin_versions",);

        let response = match self
            .list_plugin_versions(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(plugins) => record.succeed(list_plugins_response::Result::Success(
                ListPluginsSuccessResponse { plugins },
            )),
            Err(error) => record.fail(
                list_plugins_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ListPluginsResponse {
            result: Some(response),
        }))
    }

    async fn create_plugin(
        &self,
        request: Request<CreatePluginRequest>,
    ) -> Result<Response<CreatePluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!("create_plugin",);

        let response = match self
            .create_plugin(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(CreatePluginResponse {
                result: Some(create_plugin_response::Result::Success(Empty {})),
            }),
            Err(error) => record.fail(
                CreatePluginResponse {
                    result: Some(create_plugin_response::Result::Error(error.clone())),
                },
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(response))
    }

    async fn get_plugin(
        &self,
        request: Request<GetPluginRequest>,
    ) -> Result<Response<GetPluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!("get_plugin",);

        let response = match self
            .get_plugin(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(plugin) => record.succeed(GetPluginResponse {
                result: Some(get_plugin_response::Result::Success(
                    GetPluginSuccessResponse {
                        plugin: Some(plugin),
                    },
                )),
            }),
            Err(error) => record.fail(
                GetPluginResponse {
                    result: Some(get_plugin_response::Result::Error(error.clone())),
                },
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(response))
    }

    async fn delete_plugin(
        &self,
        request: Request<DeletePluginRequest>,
    ) -> Result<Response<DeletePluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!("delete_plugin",);

        let response = match self
            .delete_plugin(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(DeletePluginResponse {
                result: Some(delete_plugin_response::Result::Success(Empty {})),
            }),
            Err(error) => record.fail(
                DeletePluginResponse {
                    result: Some(delete_plugin_response::Result::Error(error.clone())),
                },
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(response))
    }

    async fn get_plugin_by_id(
        &self,
        request: Request<GetPluginByIdRequest>,
    ) -> Result<Response<GetPluginByIdResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!("get_plugin_by_id",);

        let response = match self
            .get_plugin_by_id(&request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(plugin) => record.succeed(GetPluginByIdResponse {
                result: Some(get_plugin_by_id_response::Result::Success(
                    GetPluginSuccessResponse {
                        plugin: Some(plugin),
                    },
                )),
            }),
            Err(error) => record.fail(
                GetPluginByIdResponse {
                    result: Some(get_plugin_by_id_response::Result::Error(error.clone())),
                },
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(response))
    }
}

pub fn grpc_to_plugin_creation(
    value: cloud_api_grpc::proto::golem::cloud::component::v1::CreatePluginRequest,
) -> Result<PluginDefinitionCreation<CloudPluginScope>, String> {
    let plugin = value.plugin.ok_or("missing plugin definition")?;

    let converted = PluginDefinitionCreation {
        name: plugin.name,
        version: plugin.version,
        description: plugin.description,
        icon: plugin.icon,
        homepage: plugin.homepage,
        specs: plugin.specs.ok_or("missing specs")?.try_into()?,
        scope: plugin.scope.ok_or("missing scope")?.try_into()?,
    };

    Ok(converted)
}
