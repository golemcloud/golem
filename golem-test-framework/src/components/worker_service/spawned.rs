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

use super::{new_worker_grpc_client, WorkerServiceGrpcClient};
use crate::components::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{wait_for_startup, WorkerService};
use crate::components::{new_reqwest_client, ChildProcessLogger};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;
use tracing::Level;

pub struct SpawnedWorkerService {
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    client_protocol: GolemClientProtocol,
    component_service: Arc<dyn ComponentService>,
    base_http_client: OnceCell<reqwest::Client>,
    worker_grpc_client: OnceCell<WorkerServiceGrpcClient<Channel>>,
}

impl SpawnedWorkerService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        component_service: Arc<dyn ComponentService>,
        shard_manager: Arc<dyn ShardManager>,
        rdb: Arc<dyn Rdb>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        client_protocol: GolemClientProtocol,
        cloud_service: Arc<dyn CloudService>,
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
                    &component_service,
                    &shard_manager,
                    &rdb,
                    verbosity,
                    false,
                    &cloud_service,
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
            component_service,
            base_http_client: OnceCell::new(),
            worker_grpc_client: OnceCell::new(),
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
    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
    }

    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn worker_grpc_client(&self) -> WorkerServiceGrpcClient<Channel> {
        self.worker_grpc_client
            .get_or_init(async || {
                new_worker_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
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
