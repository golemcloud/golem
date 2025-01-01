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

use crate::components::component_service::{
    new_client, new_plugins_client, wait_for_startup, ComponentService, ComponentServiceEnvVars,
};
use crate::components::rdb::Rdb;
use crate::components::{ChildProcessLogger, GolemEnvVars};
use async_trait::async_trait;

use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::transport::Channel;
use tracing::info;
use tracing::Level;

pub struct SpawnedComponentService {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    client: Option<ComponentServiceClient<Channel>>,
    plugins_client: Option<PluginServiceClient<Channel>>,
}

impl SpawnedComponentService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        component_compilation_service_port: Option<u16>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        shared_client: bool,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            executable,
            working_directory,
            http_port,
            grpc_port,
            component_compilation_service_port,
            rdb,
            verbosity,
            out_level,
            err_level,
            shared_client,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn ComponentServiceEnvVars + Send + Sync + 'static>,
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        component_compilation_service_port: Option<u16>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        shared_client: bool,
    ) -> Self {
        info!("Starting golem-component-service process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-component-service at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                env_vars
                    .env_vars(
                        http_port,
                        grpc_port,
                        component_compilation_service_port.map(|p| ("localhost", p)),
                        rdb,
                        verbosity,
                    )
                    .await,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start golem-component-service");

        let logger = ChildProcessLogger::log_child_process(
            "[componentsvc]",
            out_level,
            err_level,
            &mut child,
        );

        wait_for_startup("localhost", grpc_port, Duration::from_secs(90)).await;

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            client: if shared_client {
                Some(new_client("localhost", grpc_port).await)
            } else {
                None
            },
            plugins_client: if shared_client {
                Some(new_plugins_client("localhost", grpc_port).await)
            } else {
                None
            },
        }
    }
}

#[async_trait]
impl ComponentService for SpawnedComponentService {
    async fn client(&self) -> ComponentServiceClient<Channel> {
        match &self.client {
            Some(client) => client.clone(),
            None => new_client("localhost", self.grpc_port).await,
        }
    }

    async fn plugins_client(&self) -> PluginServiceClient<Channel> {
        match &self.plugins_client {
            Some(client) => client.clone(),
            None => new_plugins_client("localhost", self.grpc_port).await,
        }
    }

    fn private_host(&self) -> String {
        "localhost".to_string()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {
        info!("Stopping golem-component-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedComponentService {
    fn drop(&mut self) {
        info!("Stopping golem-component-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
