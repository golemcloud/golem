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
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::{new_client, WorkerExecutor};
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{Image, ImageExt};
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct DockerWorkerExecutor {
    name: String,
    public_http_port: u16,
    public_grpc_port: u16,
    container: ContainerHandle<WorkerExecutorImage>,
    client: Option<WorkerExecutorClient<Channel>>,
}

impl DockerWorkerExecutor {
    pub const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8082);
    pub const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9000);

    pub async fn new(
        unique_network_id: &str,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + 'static>,
        verbosity: Level,
        shared_client: bool,
    ) -> Self {
        info!("Starting golem-worker-executor container");

        let env_vars = super::env_vars(
            Self::HTTP_PORT.as_u16(),
            Self::GRPC_PORT.as_u16(),
            component_service,
            shard_manager,
            worker_service,
            redis,
            verbosity,
        )
        .await;

        let container =
            WorkerExecutorImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars.clone())
                .with_network(network(unique_network_id))
                .start()
                .await
                .expect("Failed to start golem-worker-executor container");

        let name = get_docker_container_name(unique_network_id, container.id()).await;

        let public_http_port = container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .await
            .expect("Failed to get public HTTP port");

        let public_grpc_port = container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .await
            .expect("Failed to get public gRPC port");

        Self {
            name,
            public_http_port,
            public_grpc_port,
            container: ContainerHandle::new(container),
            client: if shared_client {
                Some(
                    new_client("localhost", public_grpc_port)
                        .await
                        .expect("Failed to create client"),
                )
            } else {
                None
            },
        }
    }

    pub async fn stop(&self) {
        self.container.stop().await
    }

    pub async fn start(&self) {
        self.container.start().await
    }
}

#[async_trait]
impl WorkerExecutor for DockerWorkerExecutor {
    async fn client(&self) -> crate::Result<WorkerExecutorClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => Ok(new_client("localhost", self.public_grpc_port).await?),
        }
    }

    fn private_host(&self) -> String {
        self.name.clone()
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

    async fn restart(&self) {
        self.container.restart().await
    }
}

#[derive(Debug)]
struct WorkerExecutorImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl WorkerExecutorImage {
    pub fn new(
        grpc_port: ContainerPort,
        http_port: ContainerPort,
        env_vars: HashMap<String, String>,
    ) -> WorkerExecutorImage {
        WorkerExecutorImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for WorkerExecutorImage {
    fn name(&self) -> &str {
        "golemservices/golem-worker-executor"
    }

    fn tag(&self) -> &str {
        "latest"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Registering worker executor")]
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
