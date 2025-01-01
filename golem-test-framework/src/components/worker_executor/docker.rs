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

use crate::components::redis::Redis;
use crate::components::worker_executor::{new_client, WorkerExecutor, WorkerExecutorEnvVars};
use crate::components::{GolemEnvVars, NETWORK};
use async_trait::async_trait;
use std::borrow::Cow;

use crate::components::component_service::ComponentService;
use crate::components::docker::KillContainer;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image, ImageExt};
use tokio::sync::Mutex;
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct DockerWorkerExecutor {
    name: String,
    http_port: u16,
    grpc_port: u16,
    public_http_port: u16,
    public_grpc_port: u16,
    container: Arc<Mutex<Option<ContainerAsync<WorkerExecutorImage>>>>,
    keep_container: bool,
    client: Option<WorkerExecutorClient<Channel>>,
    env_vars: HashMap<String, String>,
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
        keep_container: bool,
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
            keep_container,
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
        keep_container: bool,
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

        let container = WorkerExecutorImage::new(
            ContainerPort::Tcp(grpc_port),
            ContainerPort::Tcp(http_port),
            env_vars.clone(),
        )
        .with_container_name(&name)
        .with_network(NETWORK)
        .start()
        .await
        .expect("Failed to start golem-worker-executor container");

        let public_http_port = container
            .get_host_port_ipv4(http_port)
            .await
            .expect("Failed to get public HTTP port");
        let public_grpc_port = container
            .get_host_port_ipv4(grpc_port)
            .await
            .expect("Failed to get public gRPC port");

        Self {
            name,
            http_port,
            grpc_port,
            public_http_port,
            public_grpc_port,
            container: Arc::new(Mutex::new(Some(container))),
            keep_container,
            client: if shared_client {
                Some(
                    new_client("localhost", public_grpc_port)
                        .await
                        .expect("Failed to create client"),
                )
            } else {
                None
            },
            env_vars,
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

    async fn kill(&self) {
        self.container.kill(self.keep_container).await;
    }

    async fn restart(&self) {
        let container = WorkerExecutorImage::new(
            ContainerPort::Tcp(self.grpc_port),
            ContainerPort::Tcp(self.http_port),
            self.env_vars.clone(),
        )
        .with_container_name(&self.name)
        .with_network(NETWORK)
        .start()
        .await
        .expect("Failed to start golem-worker-executor container");

        self.container.lock().await.replace(container);
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
