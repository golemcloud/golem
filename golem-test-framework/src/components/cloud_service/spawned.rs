// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::wait_for_startup;
use super::{CloudService, CloudServiceInternal, ProjectServiceClient};
use crate::components::cloud_service::new_project_client;
use crate::components::rdb::Rdb;
use crate::components::ChildProcessLogger;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;
use tracing::Level;

pub struct SpawnedCloudService {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    project_client: ProjectServiceClient,
}

impl SpawnedCloudService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Starting cloud-serfvice process");

        if !executable.exists() {
            panic!("Expected to have precompiled cloud-service at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(super::env_vars(http_port, grpc_port, rdb, verbosity, false).await)
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
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            project_client: new_project_client(client_protocol, "localhost", grpc_port, http_port)
                .await,
        }
    }
}

#[async_trait]
impl CloudServiceInternal for SpawnedCloudService {
    fn project_client(&self) -> ProjectServiceClient {
        self.project_client.clone()
    }
}

#[async_trait]
impl CloudService for SpawnedCloudService {
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
        info!("Stopping cloud-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedCloudService {
    fn drop(&mut self) {
        info!("Stopping golem-component-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
