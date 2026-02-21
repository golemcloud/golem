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

use crate::components::rdb::Rdb;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{wait_for_startup, WorkerService};
use crate::components::{new_reqwest_client, ChildProcessLogger};
use async_trait::async_trait;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::info;
use tracing::Level;

pub struct SpawnedWorkerService {
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    base_http_client: OnceCell<reqwest::Client>,
}

impl SpawnedWorkerService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        shard_manager: &Arc<dyn ShardManager>,
        rdb: &Arc<dyn Rdb>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        registry_service: &Arc<dyn RegistryService>,
        enable_fs_cache: bool,
        otlp: bool,
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
                    shard_manager,
                    rdb,
                    verbosity,
                    false,
                    registry_service,
                    enable_fs_cache,
                    otlp,
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
            "localhost",
            grpc_port,
            http_port,
            custom_request_port,
            Duration::from_secs(90),
        )
        .await;

        Self {
            http_port,
            grpc_port,
            custom_request_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            base_http_client: OnceCell::new(),
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
    fn http_host(&self) -> String {
        "localhost".to_string()
    }
    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }
    fn gprc_port(&self) -> u16 {
        self.grpc_port
    }

    fn custom_request_host(&self) -> String {
        "localhost".to_string()
    }
    fn custom_request_port(&self) -> u16 {
        self.custom_request_port
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
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
