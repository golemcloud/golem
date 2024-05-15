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

use crate::components::component_service::{env_vars, ComponentService};
use crate::components::rdb::Rdb;
use crate::components::{DOCKER, NETWORK};
use async_trait::async_trait;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};

use tracing::{info, Level};

pub struct DockerComponentService {
    container: Container<'static, GolemComponentServiceImage>,
}

impl DockerComponentService {
    const NAME: &'static str = "golem_component_service";
    const HTTP_PORT: u16 = 8081;
    const GRPC_PORT: u16 = 9091;

    pub fn new(
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-component-service container");

        let env_vars = env_vars(
            Self::HTTP_PORT,
            Self::GRPC_PORT,
            component_compilation_service,
            rdb,
            verbosity,
        );

        let image = RunnableImage::from(GolemComponentServiceImage::new(
            Self::GRPC_PORT,
            Self::HTTP_PORT,
            env_vars,
        ))
        .with_container_name(Self::NAME)
        .with_network(NETWORK);
        let container = DOCKER.run(image);

        Self { container }
    }
}

#[async_trait]
impl ComponentService for DockerComponentService {
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
        self.container.get_host_port_ipv4(Self::HTTP_PORT)
    }

    fn public_grpc_port(&self) -> u16 {
        self.container.get_host_port_ipv4(Self::GRPC_PORT)
    }

    fn kill(&self) {
        self.container.stop()
    }
}

impl Drop for DockerComponentService {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct GolemComponentServiceImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl GolemComponentServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemComponentServiceImage {
        GolemComponentServiceImage {
            grpc_port,
            http_port,
            env_vars,
        }
    }
}

impl Image for GolemComponentServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-component-service".to_string()
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
        vec![self.grpc_port, self.http_port]
    }
}
