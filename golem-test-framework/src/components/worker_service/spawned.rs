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

use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{
    new_api_definition_client, new_api_deployment_client, new_api_security_client,
    new_worker_client, wait_for_startup, ApiDefinitionServiceClient, ApiDeploymentServiceClient,
    ApiSecurityServiceClient, WorkerService, WorkerServiceClient,
};
use crate::components::ChildProcessLogger;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;
use tracing::Level;

pub struct SpawnedWorkerService {
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    client_protocol: GolemClientProtocol,
    worker_client: WorkerServiceClient,
    api_definition_client: ApiDefinitionServiceClient,
    api_deployment_client: ApiDeploymentServiceClient,
    api_security_client: ApiSecurityServiceClient,
}

impl SpawnedWorkerService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        Self::new_base(
            executable,
            working_directory,
            http_port,
            grpc_port,
            custom_request_port,
            component_service,
            shard_manager,
            rdb,
            verbosity,
            out_level,
            err_level,
            client_protocol,
        )
        .await
    }

    pub async fn new_base(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Starting golem-worker-service process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-worker-service at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                super::env_vars(
                    http_port,
                    grpc_port,
                    custom_request_port,
                    component_service,
                    shard_manager,
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
            .expect("Failed to start golem-worker-service");

        let logger =
            ChildProcessLogger::log_child_process("[workersvc]", out_level, err_level, &mut child);

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
            custom_request_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            client_protocol,
            worker_client: new_worker_client(client_protocol, "localhost", grpc_port, http_port)
                .await,
            api_definition_client: new_api_definition_client(
                client_protocol,
                "localhost",
                grpc_port,
                http_port,
            )
            .await,
            api_deployment_client: new_api_deployment_client(
                client_protocol,
                "localhost",
                grpc_port,
                http_port,
            )
            .await,
            api_security_client: new_api_security_client(
                client_protocol,
                "localhost",
                grpc_port,
                http_port,
            )
            .await,
        }
    }

    fn blocking_kill(&self) {
        info!("Stopping golem-worker-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

#[async_trait]
impl WorkerService for SpawnedWorkerService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    fn worker_client(&self) -> WorkerServiceClient {
        self.worker_client.clone()
    }

    fn api_definition_client(&self) -> ApiDefinitionServiceClient {
        self.api_definition_client.clone()
    }

    fn api_deployment_client(&self) -> ApiDeploymentServiceClient {
        self.api_deployment_client.clone()
    }

    fn api_security_client(&self) -> ApiSecurityServiceClient {
        self.api_security_client.clone()
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

    fn private_custom_request_port(&self) -> u16 {
        self.custom_request_port
    }

    async fn kill(&self) {
        self.blocking_kill()
    }
}

impl Drop for SpawnedWorkerService {
    fn drop(&mut self) {
        self.blocking_kill()
    }
}
