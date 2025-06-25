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

use super::ComponentServiceGrpcClient;
use super::PluginServiceGrpcClient;
use super::{new_component_grpc_client, new_plugin_grpc_client};
use crate::components::component_service::ComponentService;
use crate::components::new_reqwest_client;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;

pub struct ProvidedComponentService {
    component_directory: PathBuf,
    host: String,
    http_port: u16,
    grpc_port: u16,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    component_grpc_client: OnceCell<ComponentServiceGrpcClient<Channel>>,
    plugin_grpc_client: OnceCell<PluginServiceGrpcClient<Channel>>,
}

impl ProvidedComponentService {
    pub async fn new(
        component_directory: PathBuf,
        host: String,
        http_port: u16,
        grpc_port: u16,
        client_protocol: GolemClientProtocol,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    ) -> Self {
        info!("Using already running golem-component-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            component_directory,
            host: host.clone(),
            http_port,
            grpc_port,
            plugin_wasm_files_service,
            client_protocol,
            base_http_client: OnceCell::new(),
            component_grpc_client: OnceCell::new(),
            plugin_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl ComponentService for ProvidedComponentService {
    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }

    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn component_grpc_client(&self) -> ComponentServiceGrpcClient<Channel> {
        self.component_grpc_client
            .get_or_init(async || {
                new_component_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn plugin_grpc_client(&self) -> PluginServiceGrpcClient<Channel> {
        self.plugin_grpc_client
            .get_or_init(async || {
                new_plugin_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
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
