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

use crate::components::redis::Redis;
use crate::components::worker_executor::{new_client, WorkerExecutor, WorkerExecutorEnvVars};
use crate::components::{GolemEnvVars, DOCKER, NETWORK};
use async_trait::async_trait;

use crate::components::component_service::ComponentService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct DockerWorkerExecutor {
    name: String,
    http_port: u16,
    grpc_port: u16,
    public_http_port: u16,
    public_grpc_port: u16,
    container: Container<'static, WorkerExecutorImage>,
    client: Option<WorkerExecutorClient<Channel>>,
}

impl DockerWorkerExecutor {
    pub async fn new(
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
        shared_client: bool,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            http_port,
            grpc_port,
            redis,
            component_service,
            shard_manager,
            worker_service,
            verbosity,
            shared_client,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn WorkerExecutorEnvVars + Send + Sync + 'static>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
        shared_client: bool,
    ) -> Self {
        info!("Starting golem-worker-executor container");

        let env_vars = env_vars
            .env_vars(
                http_port,
                grpc_port,
                component_service,
                shard_manager,
                worker_service,
                redis,
                verbosity,
            )
            .await;

        let name = format!("golem-worker-executor-{grpc_port}");

        let image = RunnableImage::from(WorkerExecutorImage::new(grpc_port, http_port, env_vars))
            .with_container_name(&name)
            .with_network(NETWORK);
        let container = DOCKER.run(image);

        let public_http_port = container.get_host_port_ipv4(http_port);
        let public_grpc_port = container.get_host_port_ipv4(grpc_port);

        Self {
            name,
            http_port,
            grpc_port,
            public_http_port,
            public_grpc_port,
            container,
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
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
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

    fn kill(&self) {
        self.container.stop();
    }

    async fn restart(&self) {
        self.container.start();
    }
}

impl Drop for DockerWorkerExecutor {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct WorkerExecutorImage {
    env_vars: HashMap<String, String>,
    expose_ports: [u16; 2],
}

impl WorkerExecutorImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> WorkerExecutorImage {
        WorkerExecutorImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for WorkerExecutorImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-worker-executor".to_string()
    }

    fn tag(&self) -> String {
        "latest".to_string()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Registering worker executor")]
    }

    fn env_vars(&self) -> Box<dyn Iterator<Item = (&String, &String)> + '_> {
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> Vec<u16> {
        self.expose_ports.to_vec()
    }
}
