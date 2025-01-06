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

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tracing::info;
use tracing::Level;

use crate::components::redis::Redis;
use crate::components::shard_manager::{wait_for_startup, ShardManager, ShardManagerEnvVars};
use crate::components::{ChildProcessLogger, GolemEnvVars};

pub struct SpawnedShardManager {
    http_port: u16,
    grpc_port: u16,
    number_of_shards_override: std::sync::RwLock<Option<usize>>,
    child: Arc<Mutex<Option<Child>>>,
    logger: Arc<Mutex<Option<ChildProcessLogger>>>,
    executable: PathBuf,
    working_directory: PathBuf,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    env_vars: Box<dyn ShardManagerEnvVars + Send + Sync + 'static>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
}

impl SpawnedShardManager {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        number_of_shards_override: Option<usize>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            executable,
            working_directory,
            number_of_shards_override,
            http_port,
            grpc_port,
            redis,
            verbosity,
            out_level,
            err_level,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn ShardManagerEnvVars + Send + Sync + 'static>,
        executable: &Path,
        working_directory: &Path,
        number_of_shards_override: Option<usize>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> Self {
        info!("Starting golem-shard-manager process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-shard-manager at {executable:?}");
        }

        let (child, logger) = Self::start(
            env_vars.as_ref(),
            executable,
            working_directory,
            number_of_shards_override,
            http_port,
            grpc_port,
            redis.clone(),
            verbosity,
            out_level,
            err_level,
        )
        .await;

        Self {
            http_port,
            grpc_port,
            number_of_shards_override: std::sync::RwLock::new(number_of_shards_override),
            child: Arc::new(Mutex::new(Some(child))),
            logger: Arc::new(Mutex::new(Some(logger))),
            executable: executable.to_path_buf(),
            working_directory: working_directory.to_path_buf(),
            redis,
            env_vars,
            verbosity,
            out_level,
            err_level,
        }
    }

    async fn start(
        env_vars: &(dyn ShardManagerEnvVars + Send + Sync + 'static),
        executable: &Path,
        working_directory: &Path,
        number_of_shards_override: Option<usize>,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> (Child, ChildProcessLogger) {
        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                env_vars
                    .env_vars(
                        number_of_shards_override,
                        http_port,
                        grpc_port,
                        redis,
                        verbosity,
                    )
                    .await,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start golem-shard-manager");

        let logger = ChildProcessLogger::log_child_process(
            "[shardmanager]",
            out_level,
            err_level,
            &mut child,
        );

        wait_for_startup("localhost", grpc_port, Duration::from_secs(90)).await;

        (child, logger)
    }

    fn blocking_kill(&self) {
        info!("Stopping golem-shard-manager");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
        let _logger = self.logger.lock().unwrap().take();
    }
}

#[async_trait]
impl ShardManager for SpawnedShardManager {
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

    async fn restart(&self, number_of_shards_override: Option<usize>) {
        info!("Restarting golem-shard-manager");

        if let Some(number_of_shards) = number_of_shards_override {
            *self.number_of_shards_override.write().unwrap() = Some(number_of_shards);
        }
        let number_of_shards_override: Option<usize> =
            *self.number_of_shards_override.read().unwrap();

        let (child, logger) = Self::start(
            self.env_vars.as_ref(),
            &self.executable,
            &self.working_directory,
            number_of_shards_override,
            self.http_port,
            self.grpc_port,
            self.redis.clone(),
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

impl Drop for SpawnedShardManager {
    fn drop(&mut self) {
        self.blocking_kill();
    }
}
