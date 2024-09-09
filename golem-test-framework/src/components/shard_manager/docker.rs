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
use tracing::{info, Level};

use crate::components::docker::KillContainer;
use crate::components::redis::Redis;
use crate::components::shard_manager::{ShardManager, ShardManagerEnvVars};
use crate::components::{GolemEnvVars, DOCKER, NETWORK};

pub struct DockerShardManager {
    container: Container<'static, ShardManagerImage>,
    keep_container: bool,
    public_http_port: u16,
    public_grpc_port: u16,
}

impl DockerShardManager {
    const NAME: &'static str = "golem_shard_manager";
    const HTTP_PORT: u16 = 9021;
    const GRPC_PORT: u16 = 9020;

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
                Self::HTTP_PORT,
                Self::GRPC_PORT,
                redis,
                verbosity,
            )
            .await;

        let mut image = RunnableImage::from(ShardManagerImage::new(
            Self::GRPC_PORT,
            Self::HTTP_PORT,
            env_vars,
        ))
        .with_container_name(Self::NAME)
        .with_network(NETWORK);

        if let Some(number_of_shards) = number_of_shards_override {
            image = image.with_env_var(("GOLEM__NUMBER_OF_SHARDS", number_of_shards.to_string()))
        }

        let container = DOCKER.run(image);

        let public_http_port = container.get_host_port_ipv4(Self::HTTP_PORT);
        let public_grpc_port = container.get_host_port_ipv4(Self::GRPC_PORT);

        Self {
            container,
            keep_container,
            public_http_port,
            public_grpc_port,
        }
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
        self.public_http_port
    }

    fn public_grpc_port(&self) -> u16 {
        self.public_grpc_port
    }

    fn kill(&self) {
        self.container.kill(self.keep_container);
    }

    async fn restart(&self, number_of_shards_override: Option<usize>) {
        if number_of_shards_override.is_some() {
            panic!("number_of_shards_override not supported for docker")
        }
        self.container.start();
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
    expose_ports: [u16; 2],
}

impl ShardManagerImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> ShardManagerImage {
        ShardManagerImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for ShardManagerImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-shard-manager".to_string()
    }

    fn tag(&self) -> String {
        "latest".to_string()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout(
            "Shard Manager is fully operational",
        )]
    }

    fn env_vars(&self) -> Box<dyn Iterator<Item = (&String, &String)> + '_> {
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> Vec<u16> {
        self.expose_ports.to_vec()
    }
}
