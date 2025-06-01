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

use crate::components::cloud_service::new_project_client;
use crate::components::docker::{get_docker_container_name, network, ContainerHandle};
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
use tracing::{info, Level};

use super::{CloudService, CloudServiceInternal, ProjectServiceClient};

pub struct DockerCloudService {
    container: ContainerHandle<CloudServiceImage>,
    private_host: String,
    public_http_port: u16,
    public_grpc_port: u16,
    project_client: ProjectServiceClient,
}

impl DockerCloudService {
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8081);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9091);

    pub async fn new(
        unique_network_id: &str,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        client_protocol: GolemClientProtocol
    ) -> Self {
        info!("Starting golem-component-service container");

        let env_vars = super::env_vars(
            Self::HTTP_PORT.as_u16(),
            Self::GRPC_PORT.as_u16(),
            rdb,
            verbosity,
            true,
        )
        .await;

        let container = CloudServiceImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
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
            container: ContainerHandle::new(container),
            private_host,
            public_http_port,
            public_grpc_port,
            project_client: new_project_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await
        }
    }
}

#[async_trait]
impl CloudServiceInternal for DockerCloudService {
    fn project_client(&self) -> ProjectServiceClient {
        self.project_client.clone()
    }
}

#[async_trait]
impl CloudService for DockerCloudService {
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
struct CloudServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl CloudServiceImage {
    pub fn new(
        grpc_port: ContainerPort,
        http_port: ContainerPort,
        env_vars: HashMap<String, String>,
    ) -> CloudServiceImage {
        CloudServiceImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for CloudServiceImage {
    fn name(&self) -> &str {
        "golemservices/cloud-service"
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
