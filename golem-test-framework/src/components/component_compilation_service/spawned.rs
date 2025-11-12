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

use crate::components::component_compilation_service::{
    wait_for_startup, ComponentCompilationService,
};
use crate::components::ChildProcessLogger;
use async_trait::async_trait;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;
use tracing::Level;

pub struct SpawnedComponentCompilationService {
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
}

impl SpawnedComponentCompilationService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        enable_fs_cache: bool,
        otlp: bool,
    ) -> Self {
        info!("Starting golem-component-compilation-service process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-component-compilation-service at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(super::env_vars(http_port, grpc_port, verbosity, enable_fs_cache, otlp).await)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start golem-component-compilation-service");

        let logger = ChildProcessLogger::log_child_process(
            "[componentcompilationsvc]",
            out_level,
            err_level,
            &mut child,
        );

        wait_for_startup("localhost", grpc_port, Duration::from_secs(90)).await;

        Self {
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
        }
    }
}

#[async_trait]
impl ComponentCompilationService for SpawnedComponentCompilationService {
    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }

    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {
        info!("Stopping golem-component-compilation-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedComponentCompilationService {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
