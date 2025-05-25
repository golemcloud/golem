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

use crate::components::redis::Redis;
use crate::components::worker_executor::{new_client, wait_for_startup, WorkerExecutor};
use crate::components::ChildProcessLogger;
use async_trait::async_trait;

use crate::components::component_service::ComponentService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::transport::Channel;
use tracing::info;
use tracing::Level;

pub struct SpawnedWorkerExecutor {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    logger: Arc<Mutex<Option<ChildProcessLogger>>>,
    executable: PathBuf,
    working_directory: PathBuf,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + 'static>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
    client: Option<WorkerExecutorClient<Channel>>,
}

impl SpawnedWorkerExecutor {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        shared_client: bool,
    ) -> Self {
        info!("Starting golem-worker-executor process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-worker-executor at {executable:?}");
        }

        let (child, logger) = Self::start(
            executable,
            working_directory,
            http_port,
            grpc_port,
            redis.clone(),
            component_service.clone(),
            shard_manager.clone(),
            worker_service.clone(),
            verbosity,
            out_level,
            err_level,
        )
        .await;

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            logger: Arc::new(Mutex::new(Some(logger))),
            executable: executable.to_path_buf(),
            working_directory: working_directory.to_path_buf(),
            redis,
            component_service,
            shard_manager,
            worker_service,
            verbosity,
            out_level,
            err_level,
            client: if shared_client {
                Some(
                    new_client("localhost", grpc_port)
                        .await
                        .expect("Failed to create client"),
                )
            } else {
                None
            },
        }
    }

    async fn start(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> (Child, ChildProcessLogger) {
        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                super::env_vars(
                    http_port,
                    grpc_port,
                    component_service,
                    shard_manager,
                    worker_service,
                    redis,
                    verbosity,
                )
                .await,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start worker");

        let logger = ChildProcessLogger::log_child_process(
            &format!("[worker-{grpc_port}]"),
            out_level,
            err_level,
            &mut child,
        );

        wait_for_startup("localhost", grpc_port, Duration::from_secs(90)).await;

        (child, logger)
    }

    fn blocking_kill(&self) {
        info!("Stopping golem-worker-executor {}", self.grpc_port);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
        let _logger = self.logger.lock().unwrap().take();
    }
}

#[async_trait]
impl WorkerExecutor for SpawnedWorkerExecutor {
    async fn client(&self) -> crate::Result<WorkerExecutorClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => Ok(new_client("localhost", self.grpc_port).await?),
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
        self.blocking_kill();
    }

    async fn restart(&self) {
        info!("Restarting golem-worker-executor {}", self.grpc_port);

        let (child, logger) = Self::start(
            &self.executable,
            &self.working_directory,
            self.http_port,
            self.grpc_port,
            self.redis.clone(),
            self.component_service.clone(),
            self.shard_manager.clone(),
            self.worker_service.clone(),
            self.verbosity,
            self.out_level,
            self.err_level,
        )
        .await;

        let mut child_field = self.child.lock().unwrap();
        let mut logger_field = self.logger.lock().unwrap();

        assert!(child_field.is_none());
        assert!(logger_field.is_none());

        *child_field = Some(child);
        *logger_field = Some(logger);
    }
}

impl Drop for SpawnedWorkerExecutor {
    fn drop(&mut self) {
        self.blocking_kill();
    }
}
