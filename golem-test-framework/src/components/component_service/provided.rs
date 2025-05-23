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

use crate::components::component_service::{
    new_component_client, new_plugin_client, ComponentService, ComponentServiceClient,
    PluginServiceClient,
};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

use super::ComponentServiceInternal;

pub struct ProvidedComponentService {
    component_directory: PathBuf,
    host: String,
    http_port: u16,
    grpc_port: u16,
    component_client: ComponentServiceClient,
    plugin_client: PluginServiceClient,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
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
            component_client: new_component_client(client_protocol, &host, grpc_port, http_port)
                .await,
            plugin_client: new_plugin_client(client_protocol, &host, grpc_port, http_port).await,
            plugin_wasm_files_service,
        }
    }
}

#[async_trait]
impl ComponentServiceInternal for ProvidedComponentService {
    fn component_client(&self) -> ComponentServiceClient {
        self.component_client.clone()
    }

    fn plugin_client(&self) -> PluginServiceClient {
        self.plugin_client.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }
}

#[async_trait]
impl ComponentService for ProvidedComponentService {
    fn component_directory(&self) -> &Path {
        &self.component_directory
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
