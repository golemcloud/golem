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
use crate::components::rdb::Rdb;
use crate::components::{new_reqwest_client, ChildProcessLogger};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;
use tracing::Level;
use tokio::task::JoinSet;

pub struct SpawnedComponentService {
    component_directory: PathBuf,
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    component_grpc_client: OnceCell<ComponentServiceGrpcClient<Channel>>,
    plugin_grpc_client: OnceCell<PluginServiceGrpcClient<Channel>>,
}

impl SpawnedComponentService {
    pub async fn new(
        rdb: Arc<dyn Rdb>,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-registry-service process");

        let join_set = JoinSet::new();

        golem_registry_service::




        wait_for_startup(
            client_protocol,
            "localhost",
            grpc_port,
            http_port,
            Duration::from_secs(90),
        )
        .await;

        Self {
            component_directory,
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            plugin_wasm_files_service,
            client_protocol,
            base_http_client: OnceCell::new(),
            component_grpc_client: OnceCell::new(),
            plugin_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl ComponentService for SpawnedComponentService {
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
        "localhost".to_string()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {
        info!("Stopping golem-component-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedComponentService {
    fn drop(&mut self) {
        info!("Stopping golem-component-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
