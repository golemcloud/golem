// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::components::ChildProcessLogger;
use crate::components::rdb::Rdb;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::{WorkerExecutor, wait_for_startup};
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::Level;
use tracing::info;

pub struct SpawnedWorkerExecutor {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    logger: Arc<Mutex<Option<ChildProcessLogger>>>,
    executable: PathBuf,
    working_directory: PathBuf,
    rdb: Arc<dyn Rdb>,
    shard_manager: Arc<dyn ShardManager>,
    worker_service: Arc<dyn WorkerService>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
    registry_service: Arc<dyn RegistryService>,
    environment_state_cache_capacity: Option<usize>,
    otlp: bool,
}

impl SpawnedWorkerExecutor {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        rdb: Arc<dyn Rdb>,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        registry_service: Arc<dyn RegistryService>,
        environment_state_cache_capacity: Option<usize>,
        otlp: bool,
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
            &rdb,
            &shard_manager,
            &worker_service,
            verbosity,
            out_level,
            err_level,
            &registry_service,
            environment_state_cache_capacity,
            otlp,
        )
        .await;

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            logger: Arc::new(Mutex::new(Some(logger))),
            executable: executable.to_path_buf(),
            working_directory: working_directory.to_path_buf(),
            rdb,
            shard_manager,
            worker_service,
            verbosity,
            out_level,
            err_level,
            registry_service,
            environment_state_cache_capacity,
            otlp,
        }
    }

    async fn start(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        rdb: &Arc<dyn Rdb>,
        shard_manager: &Arc<dyn ShardManager>,
        worker_service: &Arc<dyn WorkerService>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        registry_service: &Arc<dyn RegistryService>,
        environment_state_cache_capacity: Option<usize>,
        otlp: bool,
    ) -> (Child, ChildProcessLogger) {
        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                super::env_vars(
                    http_port,
                    grpc_port,
                    shard_manager,
                    worker_service,
                    rdb,
                    registry_service,
                    environment_state_cache_capacity,
                    verbosity,
                    otlp,
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

        wait_for_startup(
            "localhost",
            grpc_port,
            Duration::from_secs(90),
            Some(&mut child),
        )
        .await;

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
    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }
    fn grpc_port(&self) -> u16 {
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
            &self.rdb,
            &self.shard_manager,
            &self.worker_service,
            self.verbosity,
            self.out_level,
            self.err_level,
            &self.registry_service,
            self.environment_state_cache_capacity,
            self.otlp,
        )
        .await;

        info!("Restarted golem-worker-executor {}", self.grpc_port);

        let mut child_field = self.child.lock().unwrap();
        let mut logger_field = self.logger.lock().unwrap();

        assert!(child_field.is_none());
        assert!(logger_field.is_none());

        *child_field = Some(child);
        *logger_field = Some(logger);
    }

    async fn is_running(&self) -> bool {
        let mut child_field = self.child.lock().unwrap();
        if let Some(mut child) = child_field.take() {
            let result = matches!(child.try_wait(), Ok(None));
            *child_field = Some(child);
            result
        } else {
            false
        }
    }
}

impl Drop for SpawnedWorkerExecutor {
    fn drop(&mut self) {
        self.blocking_kill();
    }
}
