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

use crate::components::component_service::ComponentService;
use crate::components::docker::{get_docker_container_name, network, ContainerHandle};
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{
    new_api_definition_client, new_api_deployment_client, new_api_security_client,
    new_worker_client, ApiDefinitionServiceClient, ApiDeploymentServiceClient,
    ApiSecurityServiceClient, WorkerService, WorkerServiceClient,
};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{Image, ImageExt};
use tracing::{info, Level};

use super::WorkerServiceInternal;

pub struct DockerWorkerService {
    container: ContainerHandle<GolemWorkerServiceImage>,
    private_host: String,
    public_http_port: u16,
    public_grpc_port: u16,
    public_custom_request_port: u16,
    client_protocol: GolemClientProtocol,
    worker_client: WorkerServiceClient,
    api_definition_client: ApiDefinitionServiceClient,
    api_deployment_client: ApiDeploymentServiceClient,
    api_security_client: ApiSecurityServiceClient,
    component_service: Arc<dyn ComponentService>,
}

impl DockerWorkerService {
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8082);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9092);
    const CUSTOM_REQUEST_PORT: ContainerPort = ContainerPort::Tcp(9093);

    pub async fn new(
        unique_network_id: &str,
        component_service: Arc<dyn ComponentService>,
        shard_manager: Arc<dyn ShardManager + Send + Sync>,
        rdb: Arc<dyn Rdb + Send + Sync>,
        verbosity: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Starting golem-worker-service container");

        let env_vars = super::env_vars(
            Self::HTTP_PORT.as_u16(),
            Self::GRPC_PORT.as_u16(),
            Self::CUSTOM_REQUEST_PORT.as_u16(),
            &component_service,
            &shard_manager,
            &rdb,
            verbosity,
            true,
        )
        .await;

        let container = GolemWorkerServiceImage::new(
            Self::GRPC_PORT,
            Self::HTTP_PORT,
            Self::CUSTOM_REQUEST_PORT,
            env_vars,
        )
        .with_network(network(unique_network_id))
        .start()
        .await
        .expect("Failed to start golem-worker-service container");

        let private_host = get_docker_container_name(unique_network_id, container.id()).await;

        let public_http_port = container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .await
            .expect("Failed to get public HTTP port");

        let public_grpc_port = container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .await
            .expect("Failed to get public gRPC port");

        let public_custom_request_port = container
            .get_host_port_ipv4(Self::CUSTOM_REQUEST_PORT)
            .await
            .expect("Failed to get public custom request port");

        Self {
            container: ContainerHandle::new(container),
            private_host,
            public_http_port,
            public_grpc_port,
            public_custom_request_port,
            client_protocol,
            worker_client: new_worker_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await,
            api_definition_client: new_api_definition_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await,
            api_deployment_client: new_api_deployment_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await,
            api_security_client: new_api_security_client(
                client_protocol,
                "localhost",
                public_grpc_port,
                public_http_port,
            )
            .await,
            component_service: component_service.clone(),
        }
    }
}

impl WorkerServiceInternal for DockerWorkerService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    fn worker_client(&self) -> WorkerServiceClient {
        self.worker_client.clone()
    }

    fn api_definition_client(&self) -> ApiDefinitionServiceClient {
        self.api_definition_client.clone()
    }

    fn api_deployment_client(&self) -> ApiDeploymentServiceClient {
        self.api_deployment_client.clone()
    }

    fn api_security_client(&self) -> ApiSecurityServiceClient {
        self.api_security_client.clone()
    }

    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
    }
}

#[async_trait]
impl WorkerService for DockerWorkerService {
    fn private_host(&self) -> String {
        self.private_host.clone()
    }

    fn private_http_port(&self) -> u16 {
        Self::HTTP_PORT.as_u16()
    }

    fn private_grpc_port(&self) -> u16 {
        Self::GRPC_PORT.as_u16()
    }

    fn private_custom_request_port(&self) -> u16 {
        Self::CUSTOM_REQUEST_PORT.as_u16()
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

    fn public_custom_request_port(&self) -> u16 {
        self.public_custom_request_port
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

#[derive(Debug)]
struct GolemWorkerServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 3],
}

impl GolemWorkerServiceImage {
    pub fn new(
        grpc_port: ContainerPort,
        http_port: ContainerPort,
        custom_request_port: ContainerPort,
        env_vars: HashMap<String, String>,
    ) -> GolemWorkerServiceImage {
        GolemWorkerServiceImage {
            env_vars,
            expose_ports: [grpc_port, http_port, custom_request_port],
        }
    }
}

impl Image for GolemWorkerServiceImage {
    fn name(&self) -> &str {
        "golemservices/golem-worker-service"
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
