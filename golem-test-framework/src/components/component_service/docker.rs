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
use crate::components::NETWORK;
use async_trait::async_trait;
use std::borrow::Cow;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, Image, ImageExt};

use tracing::{info, Level};

pub struct DockerComponentService {
    container: Container<GolemComponentServiceImage>,
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

        let image = GolemComponentServiceImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
            .with_container_name(Self::NAME)
            .with_network(NETWORK);
        let container = image.start().expect("Failed to start container");

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
        self.container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .expect("HTTP port not found")
    }

    fn public_grpc_port(&self) -> u16 {
        self.container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .expect("gRPC port not found")
    }

    fn kill(&self) {
        self.container.stop().expect("Failed to stop container")
    }
}

impl Drop for DockerComponentService {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct GolemComponentServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl GolemComponentServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemComponentServiceImage {
        GolemComponentServiceImage {
            env_vars,
            expose_ports: [ContainerPort::Tcp(grpc_port), ContainerPort::Tcp(http_port)],
        }
    }
}

impl Image for GolemComponentServiceImage {
    fn name(&self) -> &str {
        "golemservices/golem-component-service"
    }

    fn tag(&self) -> &str {
        "latest"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("server started")]
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
