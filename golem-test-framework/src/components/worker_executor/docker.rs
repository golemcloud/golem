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
use crate::components::worker_executor::{env_vars, WorkerExecutor};
use crate::components::NETWORK;
use async_trait::async_trait;
use std::borrow::Cow;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image, ImageExt};

use crate::components::component_service::ComponentService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use tracing::{info, Level};

pub struct DockerWorkerExecutor {
    name: String,
    http_port: u16,
    grpc_port: u16,
    public_http_port: u16,
    public_grpc_port: u16,
    container: ContainerAsync<WorkerExecutorImage>,
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
    ) -> Self {
        info!("Starting golem-worker-executor container");

        let env_vars = env_vars(
            http_port,
            grpc_port,
            component_service,
            shard_manager,
            worker_service,
            redis,
            verbosity,
        );

        let name = format!("golem-worker-executor-{grpc_port}");

        let image = WorkerExecutorImage::new(grpc_port, http_port, env_vars)
            .with_container_name(&name)
            .with_network(NETWORK);
        let container = image.start().await.expect("Failed to start container");

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
            container,
        }
    }
}

#[async_trait]
impl WorkerExecutor for DockerWorkerExecutor {
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
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move { self.container.stop().await.expect("Failed to stop container") });
    }

    async fn restart(&self) {
        self.container
            .start()
            .await
            .expect("Failed to start container");
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
    expose_ports: [ContainerPort; 2],
}

impl WorkerExecutorImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> WorkerExecutorImage {
        WorkerExecutorImage {
            env_vars,
            expose_ports: [ContainerPort::Tcp(grpc_port), ContainerPort::Tcp(http_port)],
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
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}
