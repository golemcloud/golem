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

use super::wait_for_startup;
use crate::components::ChildProcessLogger;
use crate::components::rdb::Rdb;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tracing::Level;
use tracing::{info, warn};

pub struct SpawnedShardManager {
    http_port: u16,
    grpc_port: u16,
    number_of_shards_override: Arc<RwLock<Option<usize>>>,
    state_write_timeout_override: Option<Duration>,
    child: Arc<Mutex<Option<Child>>>,
    logger: Arc<Mutex<Option<ChildProcessLogger>>>,
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    shutting_down: Arc<AtomicBool>,
    executable: PathBuf,
    working_directory: PathBuf,
    rdb: Arc<dyn Rdb>,
    registry_service: Arc<dyn RegistryService>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
    otlp: bool,
}

impl SpawnedShardManager {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        number_of_shards_override: Option<usize>,
        state_write_timeout_override: Option<Duration>,
        automatic_restart: bool,
        http_port: u16,
        grpc_port: u16,
        rdb: Arc<dyn Rdb>,
        registry_service: Arc<dyn RegistryService>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        otlp: bool,
    ) -> Self {
        info!("Starting golem-shard-manager process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-shard-manager at {executable:?}");
        }

        let (child, logger) = Self::start(
            executable,
            working_directory,
            number_of_shards_override,
            state_write_timeout_override,
            http_port,
            grpc_port,
            &rdb,
            &registry_service,
            verbosity,
            out_level,
            err_level,
            otlp,
        )
        .await;

        let shard_manager = Self {
            http_port,
            grpc_port,
            number_of_shards_override: Arc::new(RwLock::new(number_of_shards_override)),
            state_write_timeout_override,
            child: Arc::new(Mutex::new(Some(child))),
            logger: Arc::new(Mutex::new(Some(logger))),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            executable: executable.to_path_buf(),
            working_directory: working_directory.to_path_buf(),
            rdb,
            registry_service,
            verbosity,
            out_level,
            err_level,
            otlp,
        };

        if automatic_restart {
            shard_manager.start_automatic_restart_monitor();
        }

        shard_manager
    }

    async fn start(
        executable: &Path,
        working_directory: &Path,
        number_of_shards_override: Option<usize>,
        state_write_timeout_override: Option<Duration>,
        http_port: u16,
        grpc_port: u16,
        rdb: &Arc<dyn Rdb>,
        registry_service: &Arc<dyn RegistryService>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        otlp: bool,
    ) -> (Child, ChildProcessLogger) {
        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                super::env_vars(
                    number_of_shards_override,
                    state_write_timeout_override,
                    http_port,
                    grpc_port,
                    rdb,
                    false,
                    registry_service,
                    verbosity,
                    otlp,
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
        info!("Stopping golem-shard-manager");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
        let _logger = self.logger.lock().unwrap().take();
    }

    fn start_automatic_restart_monitor(&self) {
        let child = self.child.clone();
        let logger = self.logger.clone();
        let lifecycle = self.lifecycle.clone();
        let shutting_down = self.shutting_down.clone();
        let executable = self.executable.clone();
        let working_directory = self.working_directory.clone();
        let number_of_shards_override = self.number_of_shards_override.clone();
        let state_write_timeout_override = self.state_write_timeout_override;
        let http_port = self.http_port;
        let grpc_port = self.grpc_port;
        let rdb = self.rdb.clone();
        let registry_service = self.registry_service.clone();
        let verbosity = self.verbosity;
        let out_level = self.out_level;
        let err_level = self.err_level;
        let otlp = self.otlp;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if shutting_down.load(Ordering::Acquire) {
                    return;
                }

                let _lifecycle = lifecycle.lock().await;
                if shutting_down.load(Ordering::Acquire) {
                    return;
                }

                let exited = {
                    let mut child = child.lock().unwrap();
                    match child.as_mut().map(Child::try_wait) {
                        Some(Ok(Some(status))) => {
                            warn!(?status, "Spawned shard manager exited; restarting it");
                            child.take();
                            true
                        }
                        Some(Err(error)) => {
                            warn!(%error, "Failed to inspect spawned shard manager process");
                            false
                        }
                        Some(Ok(None)) | None => false,
                    }
                };

                if !exited {
                    continue;
                }
                logger.lock().unwrap().take();
                let current_number_of_shards_override = *number_of_shards_override.read().unwrap();

                let (mut restarted_child, restarted_logger) = Self::start(
                    &executable,
                    &working_directory,
                    current_number_of_shards_override,
                    state_write_timeout_override,
                    http_port,
                    grpc_port,
                    &rdb,
                    &registry_service,
                    verbosity,
                    out_level,
                    err_level,
                    otlp,
                )
                .await;

                if shutting_down.load(Ordering::Acquire) {
                    let _ = restarted_child.kill();
                    return;
                }

                *child.lock().unwrap() = Some(restarted_child);
                *logger.lock().unwrap() = Some(restarted_logger);
                info!("Restarted spawned shard manager after an unexpected exit");
            }
        });
    }
}

#[async_trait]
impl ShardManager for SpawnedShardManager {
    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }

    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        self.blocking_kill();
    }

    async fn restart(&self, number_of_shards_override: Option<usize>) {
        let _lifecycle = self.lifecycle.lock().await;
        info!("Restarting golem-shard-manager");

        if let Some(number_of_shards) = number_of_shards_override {
            *self.number_of_shards_override.write().unwrap() = Some(number_of_shards);
        }
        let number_of_shards_override: Option<usize> =
            *self.number_of_shards_override.read().unwrap();

        let (child, logger) = Self::start(
            &self.executable,
            &self.working_directory,
            number_of_shards_override,
            self.state_write_timeout_override,
            self.http_port,
            self.grpc_port,
            &self.rdb,
            &self.registry_service,
            self.verbosity,
            self.out_level,
            self.err_level,
            self.otlp,
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
        self.shutting_down.store(true, Ordering::Release);
        self.blocking_kill();
    }
}
