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

use crate::components::redis::Redis;
use crate::components::worker_executor::{env_vars, wait_for_startup, WorkerExecutor};
use crate::components::ChildProcessLogger;
use async_trait::async_trait;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::components::shard_manager::ShardManager;
use crate::components::template_service::TemplateService;
use crate::components::worker_service::WorkerService;
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
    template_service: Arc<dyn TemplateService + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
}

impl SpawnedWorkerExecutor {
    pub fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        template_service: Arc<dyn TemplateService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> Self {
        println!("Starting golem-worker-executor process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-worker-executor at {executable:?}");
        }

        let (child, logger) = Self::start(
            executable,
            working_directory,
            http_port,
            grpc_port,
            redis.clone(),
            template_service.clone(),
            shard_manager.clone(),
            worker_service.clone(),
            verbosity,
            out_level,
            err_level,
        );

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            logger: Arc::new(Mutex::new(Some(logger))),
            executable: executable.to_path_buf(),
            working_directory: working_directory.to_path_buf(),
            redis,
            template_service,
            shard_manager,
            worker_service,
            verbosity,
            out_level,
            err_level,
        }
    }

    fn start(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        template_service: Arc<dyn TemplateService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> (Child, ChildProcessLogger) {
        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(env_vars(
                grpc_port,
                http_port,
                template_service,
                shard_manager,
                worker_service,
                redis,
                verbosity,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start golem-shard-manager");

        let logger = ChildProcessLogger::log_child_process(
            &format!("[worker-{grpc_port}]"),
            out_level,
            err_level,
            &mut child,
        );

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(wait_for_startup("localhost", grpc_port));

        (child, logger)
    }
}

#[async_trait]
impl WorkerExecutor for SpawnedWorkerExecutor {
    fn host(&self) -> &str {
        "localhost"
    }

    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    fn kill(&self) {
        info!("Stopping golem-worker-executor {}", self.grpc_port);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
        let _logger = self.logger.lock().unwrap().take();
    }

    fn restart(&self) {
        info!("Restarting golem-worker-executor {}", self.grpc_port);

        let mut child_field = self.child.lock().unwrap();
        let mut logger_field = self.logger.lock().unwrap();

        assert!(child_field.is_none());
        assert!(logger_field.is_none());

        let (child, logger) = Self::start(
            &self.executable,
            &self.working_directory,
            self.http_port,
            self.grpc_port,
            self.redis.clone(),
            self.template_service.clone(),
            self.shard_manager.clone(),
            self.worker_service.clone(),
            self.verbosity,
            self.out_level,
            self.err_level,
        );

        *child_field = Some(child);
        *logger_field = Some(logger);
    }
}

impl Drop for SpawnedWorkerExecutor {
    fn drop(&mut self) {
        self.kill();
    }
}
