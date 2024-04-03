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

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};

use crate::components::shard_manager::ShardManager;
use crate::components::template_service::TemplateService;
use crate::components::worker_service::WorkerService;
use tracing::{info, Level};

pub struct DockerWorkerExecutor<'d> {
    http_port: u16,
    grpc_port: u16,
    container: Container<'d, WorkerExecutorImage>,
}

impl<'d> DockerWorkerExecutor<'d> {
    const NAME: &'static str = "golem_worker_executor";

    pub fn new(
        http_port: u16,
        grpc_port: u16,
        docker: &'d testcontainers::clients::Cli,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        template_service: Arc<dyn TemplateService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-worker-executor container");

        let env_vars = env_vars(
            http_port,
            grpc_port,
            template_service,
            shard_manager,
            worker_service,
            redis,
            verbosity,
        );

        let image = RunnableImage::from(
            WorkerExecutorImage::new(
                grpc_port,
                http_port,
                env_vars,
            ),
        )
        .with_container_name(Self::NAME)
        .with_network(NETWORK);
        let container = docker.run(image);

        Self {
            http_port,
            grpc_port,
            container
        }
    }
}

#[async_trait]
impl<'d> WorkerExecutor for DockerWorkerExecutor<'d> {
    fn host(&self) -> &str {
        "localhost"
    }

    fn http_port(&self) -> u16 {
        self.container.get_host_port_ipv4(self.http_port)
    }

    fn grpc_port(&self) -> u16 {
        self.container.get_host_port_ipv4(self.grpc_port)
    }

    fn kill(&self) {
        self.container.stop()
    }

    fn restart(&self) {
        self.container.start();
    }
}

impl<'d> Drop for DockerWorkerExecutor<'d> {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct WorkerExecutorImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl WorkerExecutorImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> WorkerExecutorImage {
        WorkerExecutorImage {
            grpc_port,
            http_port,
            env_vars,
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
        vec![self.grpc_port, self.http_port]
    }
}
