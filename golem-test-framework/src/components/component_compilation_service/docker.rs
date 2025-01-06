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

use crate::components::component_compilation_service::{
    ComponentCompilationService, ComponentCompilationServiceEnvVars,
};
use crate::components::component_service::ComponentService;
use crate::components::docker::KillContainer;
use crate::components::{GolemEnvVars, NETWORK};

pub struct DockerComponentCompilationService {
    container: Arc<Mutex<Option<ContainerAsync<GolemComponentCompilationServiceImage>>>>,
    keep_container: bool,
    public_http_port: u16,
    public_grpc_port: u16,
}

impl DockerComponentCompilationService {
    pub const NAME: &'static str = "golem_component_compilation_service";
    pub const HTTP_PORT: ContainerPort = ContainerPort::Tcp(8083);
    pub const GRPC_PORT: ContainerPort = ContainerPort::Tcp(9094);

    pub async fn new(
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        keep_container: bool,
        verbosity: Level,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            component_service,
            keep_container,
            verbosity,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn ComponentCompilationServiceEnvVars + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        keep_container: bool,
        verbosity: Level,
    ) -> Self {
        info!("Starting golem-component-compilation-service container");

        let env_vars = env_vars
            .env_vars(
                Self::HTTP_PORT.as_u16(),
                Self::GRPC_PORT.as_u16(),
                component_service,
                verbosity,
            )
            .await;

        let container =
            GolemComponentCompilationServiceImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
                .with_container_name(Self::NAME)
                .with_network(NETWORK)
                .start()
                .await
                .expect("Failed to start golem-component-compilation-service container");

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
        }
    }
}

#[async_trait]
impl ComponentCompilationService for DockerComponentCompilationService {
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
}

#[derive(Debug)]
struct GolemComponentCompilationServiceImage {
    env_vars: HashMap<String, String>,
    expose_ports: [ContainerPort; 2],
}

impl GolemComponentCompilationServiceImage {
    pub fn new(
        grpc_port: ContainerPort,
        http_port: ContainerPort,
        env_vars: HashMap<String, String>,
    ) -> GolemComponentCompilationServiceImage {
        GolemComponentCompilationServiceImage {
            env_vars,
            expose_ports: [grpc_port, http_port],
        }
    }
}

impl Image for GolemComponentCompilationServiceImage {
    fn name(&self) -> &str {
        "golemservices/golem-component-compilation-service"
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
        self.env_vars.iter()
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}
