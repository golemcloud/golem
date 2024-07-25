// Copyright 2024 Golem Cloud
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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};
use tonic::transport::Channel;
use tracing::{info, Level};

use golem_api_grpc::proto::golem::worker::worker_service_client::WorkerServiceClient;

use crate::components::component_service::ComponentService;
use crate::components::docker::KillContainer;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{new_client, WorkerService, WorkerServiceEnvVars};
use crate::components::{GolemEnvVars, DOCKER, NETWORK};

pub struct DockerWorkerService {
    container: Container<'static, GolemWorkerServiceImage>,
    keep_container: bool,
    public_http_port: u16,
    public_grpc_port: u16,
    public_custom_request_port: u16,
    client: Option<WorkerServiceClient<Channel>>,
}

impl DockerWorkerService {
    const NAME: &'static str = "golem_worker_service";
    const HTTP_PORT: u16 = 8082;
    const GRPC_PORT: u16 = 9092;
    const CUSTOM_REQUEST_PORT: u16 = 9093;

    pub async fn new(
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        shared_client: bool,
        keep_container: bool,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            component_service,
            shard_manager,
            rdb,
            verbosity,
            shared_client,
            keep_container,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn WorkerServiceEnvVars + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        shared_client: bool,
        keep_container: bool,
    ) -> Self {
        info!("Starting golem-worker-service container");

        let env_vars = env_vars
            .env_vars(
                Self::HTTP_PORT,
                Self::GRPC_PORT,
                Self::CUSTOM_REQUEST_PORT,
                component_service,
                shard_manager,
                rdb,
                verbosity,
            )
            .await;

        let image = RunnableImage::from(GolemWorkerServiceImage::new(
            Self::GRPC_PORT,
            Self::HTTP_PORT,
            Self::CUSTOM_REQUEST_PORT,
            env_vars,
        ))
        .with_container_name(Self::NAME)
        .with_network(NETWORK);
        let container = DOCKER.run(image);

        let public_http_port = container.get_host_port_ipv4(Self::HTTP_PORT);
        let public_grpc_port = container.get_host_port_ipv4(Self::GRPC_PORT);
        let public_custom_request_port = container.get_host_port_ipv4(Self::CUSTOM_REQUEST_PORT);

        Self {
            container,
            public_http_port,
            public_grpc_port,
            public_custom_request_port,
            client: if shared_client {
                Some(
                    new_client("localhost", public_grpc_port)
                        .await
                        .expect("Failed to create client"),
                )
            } else {
                None
            },
            keep_container,
        }
    }
}

#[async_trait]
impl WorkerService for DockerWorkerService {
    async fn client(&self) -> crate::Result<WorkerServiceClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => Ok(new_client("localhost", self.public_grpc_port).await?),
        }
    }

    fn private_host(&self) -> String {
        Self::NAME.to_string()
    }

    fn private_http_port(&self) -> u16 {
        Self::HTTP_PORT
    }

    fn private_grpc_port(&self) -> u16 {
        Self::GRPC_PORT
    }

    fn private_custom_request_port(&self) -> u16 {
        Self::CUSTOM_REQUEST_PORT
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

    fn kill(&self) {
        self.container.kill(self.keep_container);
    }
}

impl Drop for DockerWorkerService {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct GolemWorkerServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [u16; 3],
}

impl GolemWorkerServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        custom_request_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemWorkerServiceImage {
        GolemWorkerServiceImage {
            env_vars,
            expose_ports: [grpc_port, http_port, custom_request_port],
        }
    }
}

impl Image for GolemWorkerServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-worker-service".to_string()
    }

    fn tag(&self) -> String {
        "latest".to_string()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("server started")]
    }

    fn env_vars(&self) -> Box<dyn Iterator<Item = (&String, &String)> + '_> {
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> Vec<u16> {
        self.expose_ports.to_vec()
    }
}
