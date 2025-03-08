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

use crate::components::component_service::{
    new_component_client, new_plugin_client, ComponentService, ComponentServiceClient,
    PluginServiceClient,
};
use crate::components::docker::NETWORK;
use crate::components::docker::{get_docker_container_name, ContainerHandle};
use crate::components::rdb::Rdb;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{Image, ImageExt};
use tracing::{info, Level};

pub struct DockerComponentService {
    component_directory: PathBuf,
    container: ContainerHandle<GolemComponentServiceImage>,
    private_host: String,
    public_http_port: u16,
    public_grpc_port: u16,
    client_protocol: GolemClientProtocol,
    component_client: ComponentServiceClient,
    plugin_client: PluginServiceClient,
}

impl DockerComponentService {
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8081);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9091);

    pub async fn new(
        component_directory: PathBuf,
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        Self::new_base(
            component_directory,
            component_compilation_service,
            rdb,
            verbosity,
            client_protocol,
        )
        .await
    }

    pub async fn new_base(
        component_directory: PathBuf,
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Starting golem-component-service container");

        let env_vars = super::env_vars(
            Self::HTTP_PORT.as_u16(),
            Self::GRPC_PORT.as_u16(),
            component_compilation_service,
            rdb,
            verbosity,
            true,
        )
        .await;

        let container = GolemComponentServiceImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
            .with_network(NETWORK)
            .start()
            .await
            .expect("Failed to start golem-component-service container");

        let private_host = get_docker_container_name(container.id()).await;

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
            client_protocol,
            component_client: new_component_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await,
            plugin_client: new_plugin_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await,
        }
    }
}

#[async_trait]
impl ComponentService for DockerComponentService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    fn component_client(&self) -> ComponentServiceClient {
        self.component_client.clone()
    }

    fn plugin_client(&self) -> PluginServiceClient {
        self.plugin_client.clone()
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
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
