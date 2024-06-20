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

use crate::components::component_compilation_service::{env_vars, ComponentCompilationService};
use crate::components::NETWORK;
use async_trait::async_trait;
use std::borrow::Cow;

use std::collections::HashMap;
use std::sync::Arc;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image, ImageExt};

use crate::components::component_service::ComponentService;
use tracing::{info, Level};

pub struct DockerComponentCompilationService {
    container: ContainerAsync<GolemComponentCompilationServiceImage>,
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
        info!("Starting golem-component-compilation-service container");

        let env_vars = env_vars(
            Self::HTTP_PORT,
            Self::GRPC_PORT,
            component_service,
            verbosity,
        );

        let image =
            GolemComponentCompilationServiceImage::new(Self::GRPC_PORT, Self::HTTP_PORT, env_vars)
                .with_container_name(Self::NAME)
                .with_network(NETWORK);
        let container = image.start().await.expect("Failed to start container");

        let public_http_port = container
            .get_host_port_ipv4(Self::HTTP_PORT)
            .await
            .expect("Failed to get HTTP port");
        let public_grpc_port = container
            .get_host_port_ipv4(Self::GRPC_PORT)
            .await
            .expect("Failed to get gRPC port");

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
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                self.container
                    .stop()
                    .await
                    .expect("Failed to stop container")
            });
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
    expose_ports: [ContainerPort; 2],
}

impl GolemComponentCompilationServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemComponentCompilationServiceImage {
        GolemComponentCompilationServiceImage {
            env_vars,
            expose_ports: [ContainerPort::Tcp(grpc_port), ContainerPort::Tcp(http_port)],
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
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}
