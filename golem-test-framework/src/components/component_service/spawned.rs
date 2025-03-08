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
    new_component_client, new_plugin_client, wait_for_startup, ComponentService,
    ComponentServiceClient, PluginServiceClient,
};
use crate::components::rdb::Rdb;
use crate::components::ChildProcessLogger;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;
use tracing::Level;

pub struct SpawnedComponentService {
    component_directory: PathBuf,
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    client_protocol: GolemClientProtocol,
    component_client: ComponentServiceClient,
    plugin_client: PluginServiceClient,
}

impl SpawnedComponentService {
    pub async fn new(
        component_directory: PathBuf,
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        component_compilation_service_port: Option<u16>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        Self::new_base(
            component_directory,
            executable,
            working_directory,
            http_port,
            grpc_port,
            component_compilation_service_port,
            rdb,
            verbosity,
            out_level,
            err_level,
            client_protocol,
        )
        .await
    }

    pub async fn new_base(
        component_directory: PathBuf,
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        component_compilation_service_port: Option<u16>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Starting golem-component-service process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-component-service at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                super::env_vars(
                    http_port,
                    grpc_port,
                    component_compilation_service_port.map(|p| ("localhost", p)),
                    rdb,
                    verbosity,
                    false,
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

        wait_for_startup(
            client_protocol,
            "localhost",
            grpc_port,
            http_port,
            Duration::from_secs(90),
        )
        .await;

        Self {
            component_directory,
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            client_protocol,
            component_client: new_component_client(
                client_protocol,
                "localhost",
                grpc_port,
                http_port,
            )
            .await,
            plugin_client: new_plugin_client(client_protocol, "localhost", grpc_port, http_port)
                .await,
        }
    }
}

#[async_trait]
impl ComponentService for SpawnedComponentService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    fn component_client(&self) -> ComponentServiceClient {
        self.component_client.clone()
    }

    fn plugin_client(&self) -> PluginServiceClient {
        self.plugin_client.clone()
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
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
