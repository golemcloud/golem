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
use crate::components::shard_manager::{env_vars, ShardManager};
use crate::components::NETWORK;
use async_trait::async_trait;
use std::borrow::Cow;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, Image, ImageExt};

use tracing::{info, Level};

pub struct DockerShardManager {
    container: Container<ShardManagerImage>,
}

impl DockerShardManager {
    const NAME: &'static str = "golem_shard_manager";
    const HTTP_PORT: u16 = 9021;
    const GRPC_PORT: u16 = 9020;

    pub fn new(redis: Arc<dyn Redis + Send + Sync + 'static>, verbosity: Level) -> Self {
        info!("Starting golem-shard-manager container");

        let env_vars = env_vars(Self::HTTP_PORT, Self::GRPC_PORT, redis, verbosity);

        let image = ShardManagerImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
            .with_container_name(Self::NAME)
            .with_network(NETWORK);
        let container = image.start().expect("Failed to start container");

        Self { container }
    }
}

#[async_trait]
impl ShardManager for DockerShardManager {
    fn private_host(&self) -> String {
        Self::NAME.to_string()
    }

    fn private_http_port(&self) -> u16 {
        Self::HTTP_PORT
    }

    fn private_grpc_port(&self) -> u16 {
        Self::GRPC_PORT
    }

    fn public_host(&self) -> String {
        "localhost".to_string()
    }

    fn public_http_port(&self) -> u16 {
        self.container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .expect("Failed to get HTTP port")
    }

    fn public_grpc_port(&self) -> u16 {
        self.container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .expect("Failed to get gRPC port")
    }

    fn kill(&self) {
        self.container.stop().expect("Failed to stop container");
    }

    async fn restart(&self) {
        self.container.start().expect("Failed to start container");
    }
}

impl Drop for DockerShardManager {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct ShardManagerImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl ShardManagerImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> ShardManagerImage {
        ShardManagerImage {
            env_vars,
            expose_ports: [ContainerPort::Tcp(grpc_port), ContainerPort::Tcp(http_port)],
        }
    }
}

impl Image for ShardManagerImage {
    fn name(&self) -> &str {
        "golemservices/golem-shard-manager"
    }

    fn tag(&self) -> &str {
        "latest"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout(
            "Shard Manager is fully operational",
        )]
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
