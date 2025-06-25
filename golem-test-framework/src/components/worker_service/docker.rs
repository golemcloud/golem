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

use super::{new_worker_grpc_client, WorkerServiceGrpcClient};
use crate::components::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::docker::{get_docker_container_name, network, ContainerHandle};
use crate::components::new_reqwest_client;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{Image, ImageExt};
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct DockerWorkerService {
    container: ContainerHandle<GolemWorkerServiceImage>,
    private_host: String,
    public_http_port: u16,
    public_grpc_port: u16,
    public_custom_request_port: u16,
    component_service: Arc<dyn ComponentService>,
    cloud_service: Arc<dyn CloudService>,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    worker_grpc_client: OnceCell<WorkerServiceGrpcClient<Channel>>,
}

impl DockerWorkerService {
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8082);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9092);
    const CUSTOM_REQUEST_PORT: ContainerPort = ContainerPort::Tcp(9093);

    pub async fn new(
        unique_network_id: &str,
        component_service: Arc<dyn ComponentService>,
        shard_manager: Arc<dyn ShardManager>,
        rdb: Arc<dyn Rdb>,
        verbosity: Level,
        client_protocol: GolemClientProtocol,
        cloud_service: Arc<dyn CloudService>,
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
            &cloud_service,
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
            component_service,
            cloud_service,
            base_http_client: OnceCell::new(),
            worker_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl WorkerService for DockerWorkerService {
    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
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

    async fn worker_grpc_client(&self) -> WorkerServiceGrpcClient<Channel> {
        self.worker_grpc_client
            .get_or_init(async || {
                new_worker_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

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
