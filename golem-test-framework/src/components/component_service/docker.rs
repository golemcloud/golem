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
use crate::components::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::docker::{get_docker_container_name, network, ContainerHandle};
use crate::components::new_reqwest_client;
use crate::components::rdb::Rdb;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{Image, ImageExt};
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct DockerComponentService {
    component_directory: PathBuf,
    container: ContainerHandle<GolemComponentServiceImage>,
    private_host: String,
    public_http_port: u16,
    public_grpc_port: u16,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    cloud_service: Arc<dyn CloudService>,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    component_grpc_client: OnceCell<ComponentServiceGrpcClient<Channel>>,
    plugin_grpc_client: OnceCell<PluginServiceGrpcClient<Channel>>,
}

impl DockerComponentService {
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8081);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9091);

    pub async fn new(
        unique_network_id: &str,
        component_directory: PathBuf,
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb>,
        verbosity: Level,
        client_protocol: GolemClientProtocol,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
        cloud_service: Arc<dyn CloudService>,
    ) -> Self {
        info!("Starting golem-component-service container");

        let env_vars = super::env_vars(
            Self::HTTP_PORT.as_u16(),
            Self::GRPC_PORT.as_u16(),
            component_compilation_service,
            rdb,
            verbosity,
            true,
            &cloud_service,
        )
        .await;

        let container = GolemComponentServiceImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
            .with_network(network(unique_network_id))
            .start()
            .await
            .expect("Failed to start golem-component-service container");

        let private_host = get_docker_container_name(unique_network_id, container.id()).await;

        let public_http_port = container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .await
            .expect("Failed to get public HTTP port");

        let public_grpc_port = container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .await
            .expect("Failed to get public gRPC port");

        Self {
            component_directory,
            container: ContainerHandle::new(container),
            private_host,
            public_http_port,
            public_grpc_port,
            plugin_wasm_files_service,
            cloud_service,
            client_protocol,
            base_http_client: OnceCell::new(),
            component_grpc_client: OnceCell::new(),
            plugin_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl ComponentService for DockerComponentService {
    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn cloud_service(&self) -> Arc<dyn CloudService> {
        self.cloud_service.clone()
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
        self.private_host.to_string()
    }

    fn private_http_port(&self) -> u16 {
        Self::HTTP_PORT.as_u16()
    }

    fn private_grpc_port(&self) -> u16 {
        Self::GRPC_PORT.as_u16()
    }

    fn public_host(&self) -> String {
        "localhost".to_string()
    }

    fn public_http_port(&self) -> u16 {
        self.public_http_port
    }

    fn public_grpc_port(&self) -> u16 {
        self.public_grpc_port
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

#[derive(Debug)]
struct GolemComponentServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl GolemComponentServiceImage {
    pub fn new(
        grpc_port: ContainerPort,
        http_port: ContainerPort,
        env_vars: HashMap<String, String>,
    ) -> GolemComponentServiceImage {
        GolemComponentServiceImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for GolemComponentServiceImage {
    fn name(&self) -> &str {
        "golemservices/golem-component-service"
    }

    fn tag(&self) -> &str {
        "latest"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("server started")]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        self.env_vars.iter()
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}
