// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::grpcapi::component::bad_request_error;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody};
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_plugin_response, delete_plugin_response, get_plugin_by_id_response,
    get_plugin_response, list_plugins_response, ComponentError, CreatePluginRequest,
    CreatePluginResponse, DeletePluginRequest, DeletePluginResponse, GetPluginByIdRequest,
    GetPluginByIdResponse, GetPluginRequest, GetPluginResponse, GetPluginSuccessResponse,
    ListPluginVersionsRequest, ListPluginsRequest, ListPluginsResponse, ListPluginsSuccessResponse,
};
use golem_api_grpc::proto::golem::component::PluginDefinition;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_component_service_base::service::plugin::PluginService;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::Instrument;

pub struct PluginGrpcApi {
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send>,
}

impl PluginGrpcApi {
    async fn list_plugins(
        &self,
        request: &ListPluginsRequest,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let plugins = match &request.scope {
            Some(scope) => {
                let scope = (*scope)
                    .try_into()
                    .map_err(|err| bad_request_error(&format!("Invalid plugin scope: {err}")))?;

                self.plugin_service
                    .list_plugins_for_scope(&DefaultPluginOwner, &scope, ())
                    .await?
            }
            None => {
                self.plugin_service
                    .list_plugins(&DefaultPluginOwner)
                    .await?
            }
        };

        Ok(plugins.into_iter().map(|p| p.into()).collect())
    }

    async fn list_plugin_versions(
        &self,
        request: &ListPluginVersionsRequest,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let plugins = self
            .plugin_service
            .list_plugin_versions(&DefaultPluginOwner, &request.name)
            .await?;

        Ok(plugins.into_iter().map(|p| p.into()).collect())
    }

    async fn create_plugin(&self, request: &CreatePluginRequest) -> Result<(), ComponentError> {
        let plugin = request
            .plugin
            .clone()
            .ok_or(bad_request_error("Missing plugin specification"))?
            .try_into()
            .map_err(|err| bad_request_error(&format!("Invalid plugin specification: {err}")))?;

        self.plugin_service
            .create_plugin(&DefaultPluginOwner, plugin)
            .await?;

        Ok(())
    }

    async fn get_plugin(
        &self,
        request: &GetPluginRequest,
    ) -> Result<PluginDefinition, ComponentError> {
        let plugin = self
            .plugin_service
            .get(&DefaultPluginOwner, &request.name, &request.version)
            .await?;

        match plugin {
            Some(plugin) => Ok(plugin.into()),
            None => Err(ComponentError {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: "Plugin not found".to_string(),
                })),
            }),
        }
    }

    async fn get_plugin_by_id(
        &self,
        request: &GetPluginByIdRequest,
    ) -> Result<PluginDefinition, ComponentError> {
        let plugin_id = &request
            .id
            .ok_or(bad_request_error("Missing plugin id"))?
            .try_into()
            .map_err(|err| bad_request_error(&format!("Invalid plugin id: {err}")))?;

        let plugin = self
            .plugin_service
            .get_by_id(&DefaultPluginOwner, plugin_id)
            .await?;

        match plugin {
            Some(plugin) => Ok(plugin.into()),
            None => Err(ComponentError {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: "Plugin not found".to_string(),
                })),
            }),
        }
    }

    async fn delete_plugin(&self, request: &DeletePluginRequest) -> Result<(), ComponentError> {
        self.plugin_service
            .delete(&DefaultPluginOwner, &request.name, &request.version)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl golem_api_grpc::proto::golem::component::v1::plugin_service_server::PluginService
    for PluginGrpcApi
{
    async fn list_plugins(
        &self,
        request: Request<ListPluginsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("list_plugins",);

        let response = match self
            .list_plugins(&request)
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("list_plugin_versions",);

        let response = match self
            .list_plugin_versions(&request)
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("create_plugin",);

        let response = match self
            .create_plugin(&request)
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
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("get_plugin",);

        let response = match self
            .get_plugin(&request)
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

    async fn get_plugin_by_id(
        &self,
        request: Request<GetPluginByIdRequest>,
    ) -> Result<Response<GetPluginByIdResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("get_plugin",);

        let response = match self
            .get_plugin_by_id(&request)
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

    async fn delete_plugin(
        &self,
        request: Request<DeletePluginRequest>,
    ) -> Result<Response<DeletePluginResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("delete_plugin",);

        let response = match self
            .delete_plugin(&request)
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
}
