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

use crate::components::rdb::Rdb;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::{ChildProcessLogger, is_serving_grpc};
use async_trait::async_trait;
use futures::FutureExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tracing::Level;
use tracing::{info, warn};

pub struct SpawnedShardManager {
    http_port: u16,
    grpc_port: u16,
    number_of_shards_override: Arc<RwLock<Option<usize>>>,
    state_write_timeout_override: Option<Duration>,
    process: Arc<Mutex<ProcessState>>,
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    executable: PathBuf,
    working_directory: PathBuf,
    rdb: Arc<dyn Rdb>,
    registry_service: Arc<dyn RegistryService>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
    otlp: bool,
}

struct ProcessState {
    shutting_down: bool,
    shutdown: tokio::sync::watch::Sender<bool>,
    child: Option<Child>,
    logger: Option<ChildProcessLogger>,
}

struct StartingChild(Option<Child>);

impl StartingChild {
    fn release(mut self) -> Child {
        self.0.take().unwrap()
    }
}

impl std::ops::Deref for StartingChild {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for StartingChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}

impl Drop for StartingChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
            None,
        )
        .await
        .expect("Failed to start golem-shard-manager");

        let shard_manager = Self {
            http_port,
            grpc_port,
            number_of_shards_override: Arc::new(RwLock::new(number_of_shards_override)),
            state_write_timeout_override,
            process: Arc::new(Mutex::new(ProcessState {
                shutting_down: false,
                shutdown: tokio::sync::watch::channel(false).0,
                child: Some(child),
                logger: Some(logger),
            })),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
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
        mut shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<(Child, ChildProcessLogger)> {
        let mut child = StartingChild(Some(
            Command::new(executable)
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
                .map_err(|error| anyhow::anyhow!("Failed to spawn golem-shard-manager: {error}"))?,
        ));

        let logger = ChildProcessLogger::log_child_process(
            "[shardmanager]",
            out_level,
            err_level,
            &mut child,
        );

        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if shutdown.as_ref().is_some_and(|shutdown| *shutdown.borrow()) {
                let _ = child.kill();
                anyhow::bail!("Shard manager shutdown requested during startup");
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    anyhow::bail!("golem-shard-manager exited during startup with {status}")
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    anyhow::bail!("Failed to inspect golem-shard-manager process: {error}");
                }
            }
            let serving = is_serving_grpc("localhost", grpc_port, Duration::from_millis(500));
            let serving = match shutdown.as_mut() {
                Some(shutdown) => tokio::select! {
                    biased;
                    _ = shutdown.changed() => {
                        let _ = child.kill();
                        anyhow::bail!("Shard manager shutdown requested during startup");
                    }
                    serving = serving => serving,
                },
                None => serving.await,
            };
            if serving {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                anyhow::bail!("Timed out waiting for golem-shard-manager startup");
            }
            match shutdown.as_mut() {
                Some(shutdown) => tokio::select! {
                    biased;
                    _ = shutdown.changed() => {
                        let _ = child.kill();
                        anyhow::bail!("Shard manager shutdown requested during startup");
                    }
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                },
                None => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }

        Ok((child.release(), logger))
    }

    fn blocking_kill(&self) {
        info!("Stopping golem-shard-manager");
        let mut process = self.process.lock().unwrap();
        if let Some(mut child) = process.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        process.logger.take();
    }

    fn start_automatic_restart_monitor(&self) {
        let process = self.process.clone();
        let lifecycle = self.lifecycle.clone();
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
        let shutdown = self.process.lock().unwrap().shutdown.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if process.lock().unwrap().shutting_down {
                    return;
                }

                let _lifecycle = lifecycle.lock().await;
                if process.lock().unwrap().shutting_down {
                    return;
                }

                let exited = {
                    let mut state = process.lock().unwrap();
                    match state.child.as_mut().map(Child::try_wait) {
                        Some(Ok(Some(status))) => {
                            warn!(?status, "Spawned shard manager exited; restarting it");
                            state.child.take();
                            state.logger.take();
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
                let current_number_of_shards_override = *number_of_shards_override.read().unwrap();

                let (mut restarted_child, restarted_logger) = loop {
                    let restart = std::panic::AssertUnwindSafe(Self::start(
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
                        Some(shutdown.clone()),
                    ))
                    .catch_unwind()
                    .await;
                    match restart {
                        Ok(Ok(restarted)) => break restarted,
                        Ok(Err(error)) => {
                            if process.lock().unwrap().shutting_down {
                                return;
                            }
                            warn!(%error, "Failed to restart spawned shard manager; retrying");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Err(_) => {
                            if process.lock().unwrap().shutting_down {
                                return;
                            }
                            warn!("Restarting the spawned shard manager panicked; retrying");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                };

                let mut state = process.lock().unwrap();
                if state.shutting_down {
                    drop(state);
                    let _ = restarted_child.kill();
                    let _ = restarted_child.wait();
                    return;
                }
                state.child = Some(restarted_child);
                state.logger = Some(restarted_logger);
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
        let shutdown = self.process.lock().unwrap().shutdown.subscribe();

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
            Some(shutdown),
        )
        .await
        .expect("Failed to restart golem-shard-manager");

        let mut process = self.process.lock().unwrap();

        assert!(process.child.is_none());
        assert!(process.logger.is_none());

        process.child = Some(child);
        process.logger = Some(logger);
    }
}

impl Drop for SpawnedShardManager {
    fn drop(&mut self) {
        info!("Stopping golem-shard-manager");
        let mut process = self.process.lock().unwrap();
        process.shutting_down = true;
        process.shutdown.send_replace(true);
        if let Some(mut child) = process.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        process.logger.take();
    }
}
