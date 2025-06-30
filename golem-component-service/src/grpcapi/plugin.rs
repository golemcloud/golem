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

use crate::api::common::ComponentTraceErrorKind;
use crate::authed::plugin::AuthedPluginService;
use crate::grpcapi::{auth, bad_request_error};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody};
use golem_api_grpc::proto::golem::component::v1::plugin_service_server::PluginService;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_plugin_response, delete_plugin_response, ComponentError,
    CreatePluginResponse, DeletePluginRequest, DeletePluginResponse, GetPluginRequest,
    ListPluginVersionsRequest,
};
use golem_api_grpc::proto::golem::component::v1::{
    get_plugin_by_id_response, get_plugin_response, list_plugins_response, CreatePluginRequest,
    GetPluginByIdRequest, GetPluginByIdResponse, GetPluginResponse, GetPluginSuccessResponse,
    ListPluginsRequest, ListPluginsResponse, ListPluginsSuccessResponse,
};
use golem_api_grpc::proto::golem::component::PluginDefinition;
use golem_common::recorded_grpc_api_request;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

pub struct PluginGrpcApi {
    plugin_service: Arc<AuthedPluginService>,
}

impl PluginGrpcApi {
    pub fn new(plugin_service: Arc<AuthedPluginService>) -> Self {
        Self { plugin_service }
    }

    async fn list_plugins(
        &self,
        request: &ListPluginsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let auth = auth(metadata)?;

        let scope = request
            .scope
            .ok_or(bad_request_error("no scope found in request"))?
            .try_into()
            .map_err(|err| bad_request_error(&format!("Invalid plugin scope: {err}")))?;

        let plugins = self.plugin_service
            .list_plugins_for_scope(&auth, &scope)
            .await?;

        Ok(plugins.into_iter().map(|pd| pd.into()).collect())
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

        Ok(plugins.into_iter().map(|pd| pd.into()).collect())
    }

    async fn create_plugin(
        &self,
        request: &CreatePluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), ComponentError> {
        let auth = auth(metadata)?;

        let plugin = request
            .clone()
            .try_into()
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

        let account_id = request
            .account_id
            .clone()
            .ok_or(bad_request_error("Missing account id"))?
            .into();

        let plugin = self
            .plugin_service
            .get(&auth, account_id, &request.name, &request.version)
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
            Some(plugin) => Ok(plugin.into()),
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
