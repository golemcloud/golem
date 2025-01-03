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

use crate::components::component_service::{new_client, new_plugins_client, ComponentService};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient;
use tonic::transport::Channel;
use tracing::info;

pub struct ProvidedComponentService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    client: Option<ComponentServiceClient<Channel>>,
    plugins_client: Option<PluginServiceClient<Channel>>,
}

impl ProvidedComponentService {
    pub async fn new(host: String, http_port: u16, grpc_port: u16, shared_client: bool) -> Self {
        info!("Using already running golem-component-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            client: if shared_client {
                Some(new_client(&host, grpc_port).await)
            } else {
                None
            },
            plugins_client: if shared_client {
                Some(new_plugins_client(&host, grpc_port).await)
            } else {
                None
            },
        }
    }
}

#[async_trait]
impl ComponentService for ProvidedComponentService {
    async fn client(&self) -> ComponentServiceClient<Channel> {
        match &self.client {
            Some(client) => client.clone(),
            None => new_client(&self.host, self.grpc_port).await,
        }
    }

    async fn plugins_client(&self) -> PluginServiceClient<Channel> {
        match &self.plugins_client {
            Some(client) => client.clone(),
            None => new_plugins_client(&self.host, self.grpc_port).await,
        }
    }

    fn private_host(&self) -> String {
        self.host.clone()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {}
}
