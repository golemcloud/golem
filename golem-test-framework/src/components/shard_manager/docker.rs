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

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::components::docker::{get_docker_container_name, network, ContainerHandle};
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use async_trait::async_trait;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{Image, ImageExt};
use tracing::{info, Level};

pub struct DockerShardManager {
    container: ContainerHandle<ShardManagerImage>,
    container_name: String,
    public_http_port: u16,
    public_grpc_port: u16,
}

impl DockerShardManager {
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(9021);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9020);

    pub async fn new(
        unique_network_id: &str,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        number_of_shards_override: Option<usize>,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-shard-manager container");

        let env_vars = super::env_vars(
            number_of_shards_override,
            Self::HTTP_PORT.as_u16(),
            Self::GRPC_PORT.as_u16(),
            redis,
            verbosity,
        )
        .await;

        let mut image = ShardManagerImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars.clone())
            .with_network(network(unique_network_id));

        if let Some(number_of_shards) = number_of_shards_override {
            image = image.with_env_var("GOLEM__NUMBER_OF_SHARDS", number_of_shards.to_string())
        }

        let container = image
            .start()
            .await
            .expect("Failed to start golem-shard-manager container");

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
            container_name: private_host,
            public_http_port,
            public_grpc_port,
        }
    }
}

#[async_trait]
impl ShardManager for DockerShardManager {
    fn private_host(&self) -> String {
        self.container_name.clone()
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

    async fn restart(&self, number_of_shards_override: Option<usize>) {
        if number_of_shards_override.is_some() {
            panic!("number_of_shards_override not supported for docker")
        }
        self.container.restart().await
    }
}

#[derive(Debug)]
struct ShardManagerImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl ShardManagerImage {
    pub fn new(
        grpc_port: ContainerPort,
        http_port: ContainerPort,
        env_vars: HashMap<String, String>,
    ) -> ShardManagerImage {
        ShardManagerImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
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
        self.env_vars.iter()
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}
