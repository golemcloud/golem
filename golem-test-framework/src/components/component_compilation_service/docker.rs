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

use crate::components::component_compilation_service::{
    ComponentCompilationService, ComponentCompilationServiceEnvVars,
};
use crate::components::{GolemEnvVars, DOCKER, NETWORK};
use async_trait::async_trait;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::WaitFor;
use testcontainers::{Container, Image, RunnableImage};

use crate::components::component_service::ComponentService;
use tracing::{info, Level};

pub struct DockerComponentCompilationService {
    container: Container<'static, GolemComponentCompilationServiceImage>,
    public_http_port: u16,
    public_grpc_port: u16,
}

impl DockerComponentCompilationService {
    pub const NAME: &'static str = "golem_component_compilation_service";
    pub const HTTP_PORT: u16 = 8083;
    pub const GRPC_PORT: u16 = 9094;

    pub async fn new(
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        verbosity: Level,
    ) -> Self {
        Self::new_base(Box::new(GolemEnvVars()), component_service, verbosity).await
    }

    pub async fn new_base(
        env_vars: Box<dyn ComponentCompilationServiceEnvVars + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-component-compilation-service container");

        let env_vars = env_vars
            .env_vars(
                Self::HTTP_PORT,
                Self::GRPC_PORT,
                component_service,
                verbosity,
            )
            .await;

        let image = RunnableImage::from(GolemComponentCompilationServiceImage::new(
            Self::GRPC_PORT,
            Self::HTTP_PORT,
            env_vars,
        ))
        .with_container_name(Self::NAME)
        .with_network(NETWORK);
        let container = DOCKER.run(image);

        let public_http_port = container.get_host_port_ipv4(Self::HTTP_PORT);
        let public_grpc_port = container.get_host_port_ipv4(Self::GRPC_PORT);

        Self {
            container,
            public_http_port,
            public_grpc_port,
        }
    }
}

#[async_trait]
impl ComponentCompilationService for DockerComponentCompilationService {
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
        self.container.stop();
    }
}

impl Drop for DockerComponentCompilationService {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug)]
struct GolemComponentCompilationServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [u16; 2],
}

impl GolemComponentCompilationServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemComponentCompilationServiceImage {
        GolemComponentCompilationServiceImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for GolemComponentCompilationServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-component-compilation-service".to_string()
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
        self.expose_ports.to_vec()
    }
}
