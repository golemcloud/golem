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

use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::{
    CreatePluginRequest, CreatePluginResponse, DeletePluginRequest, DeletePluginResponse,
    GetPluginRequest, GetPluginResponse, ListPluginVersionsRequest, ListPluginsRequest,
    ListPluginsResponse,
};
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_component_service_base::service::plugin::PluginService;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct PluginGrpcApi {
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send>,
}

impl PluginGrpcApi {}

#[async_trait]
impl golem_api_grpc::proto::golem::component::v1::plugin_service_server::PluginService
    for PluginGrpcApi
{
    async fn list_plugins(
        &self,
        request: Request<ListPluginsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        todo!()
    }

    async fn list_plugin_versions(
        &self,
        request: Request<ListPluginVersionsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        todo!()
    }

    async fn create_plugin(
        &self,
        request: Request<CreatePluginRequest>,
    ) -> Result<Response<CreatePluginResponse>, Status> {
        todo!()
    }

    async fn get_plugin(
        &self,
        request: Request<GetPluginRequest>,
    ) -> Result<Response<GetPluginResponse>, Status> {
        todo!()
    }

    async fn delete_plugin(
        &self,
        request: Request<DeletePluginRequest>,
    ) -> Result<Response<DeletePluginResponse>, Status> {
        todo!()
    }
}
