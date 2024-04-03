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

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};


use tracing::{info, Level};

pub struct DockerShardManager<'d> {
    container: Container<'d, ShardManagerImage>,
}

impl<'d> DockerShardManager<'d> {
    const NAME: &'static str = "golem_shard_manager";
    const HTTP_PORT: u16 = 9021;
    const GRPC_PORT: u16 = 9020;

    pub fn new(
        docker: &'d testcontainers::clients::Cli,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-shard-manager container");

        let env_vars = env_vars(Self::HTTP_PORT, Self::GRPC_PORT, redis, verbosity);

        let image = RunnableImage::from(ShardManagerImage::new(
            Self::GRPC_PORT,
            Self::HTTP_PORT,
            env_vars,
        ))
        .with_container_name(Self::NAME)
        .with_network(NETWORK);
        let container = docker.run(image);

        Self { container }
    }
}

#[async_trait]
impl<'d> ShardManager for DockerShardManager<'d> {
    fn host(&self) -> &str {
        "localhost"
    }

    fn http_port(&self) -> u16 {
        self.container.get_host_port_ipv4(Self::HTTP_PORT)
    }

    fn grpc_port(&self) -> u16 {
        self.container.get_host_port_ipv4(Self::GRPC_PORT)
    }

    fn kill(&self) {
        self.container.stop()
    }

    fn restart(&self) {
        self.container.start();
    }
}

impl<'d> Drop for DockerShardManager<'d> {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct ShardManagerImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl ShardManagerImage {
    pub fn new(port: u16, http_port: u16, env_vars: HashMap<String, String>) -> ShardManagerImage {
        ShardManagerImage {
            grpc_port: port,
            http_port,
            env_vars,
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
        vec![self.grpc_port, self.http_port]
    }
}
