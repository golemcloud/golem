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

use crate::components::rdb::Rdb;
use crate::components::template_service::{env_vars, TemplateService};
use crate::components::{DOCKER, NETWORK};
use async_trait::async_trait;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};

use tracing::{info, Level};

pub struct DockerTemplateService {
    container: Container<'static, GolemTemplateServiceImage>,
}

impl DockerTemplateService {
    const NAME: &'static str = "golem_template_service";
    const HTTP_PORT: u16 = 8081;
    const GRPC_PORT: u16 = 9091;

    pub fn new(rdb: Arc<dyn Rdb + Send + Sync + 'static>, verbosity: Level) -> Self {
        info!("Starting golem-template-service container");

        let env_vars = env_vars(Self::HTTP_PORT, Self::GRPC_PORT, rdb, verbosity);

        let image = RunnableImage::from(GolemTemplateServiceImage::new(
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
impl TemplateService for DockerTemplateService {
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

impl Drop for DockerTemplateService {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct GolemTemplateServiceImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl GolemTemplateServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemTemplateServiceImage {
        GolemTemplateServiceImage {
            grpc_port,
            http_port,
            env_vars,
        }
    }
}

impl Image for GolemTemplateServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-template-service".to_string()
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
