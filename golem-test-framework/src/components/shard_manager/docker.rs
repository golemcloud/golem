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

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image, ImageExt};
use tokio::sync::Mutex;
use tracing::{info, Level};

use crate::components::docker::KillContainer;
use crate::components::redis::Redis;
use crate::components::shard_manager::{ShardManager, ShardManagerEnvVars};
use crate::components::{GolemEnvVars, NETWORK};

pub struct DockerShardManager {
    container: Arc<Mutex<Option<ContainerAsync<ShardManagerImage>>>>,
    keep_container: bool,
    public_http_port: u16,
    public_grpc_port: u16,
    env_vars: HashMap<String, String>,
}

impl DockerShardManager {
    const NAME: &'static str = "golem_shard_manager";
    const HTTP_PORT: ContainerPort = ContainerPort::Tcp(9021);
    const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9020);

    pub async fn new(
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        number_of_shards_override: Option<usize>,
        verbosity: Level,
        keep_container: bool,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            number_of_shards_override,
            redis,
            verbosity,
            keep_container,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn ShardManagerEnvVars + Send + Sync + 'static>,
        number_of_shards_override: Option<usize>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
        keep_container: bool,
    ) -> Self {
        info!("Starting golem-shard-manager container");

        let env_vars = env_vars
            .env_vars(
                number_of_shards_override,
                Self::HTTP_PORT.as_u16(),
                Self::GRPC_PORT.as_u16(),
                redis,
                verbosity,
            )
            .await;

        let mut image = ShardManagerImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars.clone())
            .with_container_name(Self::NAME)
            .with_network(NETWORK);

        if let Some(number_of_shards) = number_of_shards_override {
            image = image.with_env_var("GOLEM__NUMBER_OF_SHARDS", number_of_shards.to_string())
        }

        let container = image
            .start()
            .await
            .expect("Failed to start golem-shard-manager container");

        let public_http_port = container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .await
            .expect("Failed to get public HTTP port");
        let public_grpc_port = container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .await
            .expect("Failed to get public gRPC port");

        Self {
            container: Arc::new(Mutex::new(Some(container))),
            keep_container,
            public_http_port,
            public_grpc_port,
            env_vars,
        }
    }
}

#[async_trait]
impl ShardManager for DockerShardManager {
    fn private_host(&self) -> String {
        Self::NAME.to_string()
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
        self.container.kill(self.keep_container).await;
    }

    async fn restart(&self, number_of_shards_override: Option<usize>) {
        if number_of_shards_override.is_some() {
            panic!("number_of_shards_override not supported for docker")
        }

        let mut image =
            ShardManagerImage::new(Self::GRPC_PORT, Self::HTTP_PORT, self.env_vars.clone())
                .with_container_name(Self::NAME)
                .with_network(NETWORK);

        if let Some(number_of_shards) = number_of_shards_override {
            image = image.with_env_var("GOLEM__NUMBER_OF_SHARDS", number_of_shards.to_string())
        }

        let container = image
            .start()
            .await
            .expect("Failed to start golem-shard-manager container");

        self.container.lock().await.replace(container);
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
