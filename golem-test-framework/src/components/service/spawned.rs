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

use crate::components::ChildProcessLogger;
use async_trait::async_trait;
use std::collections::HashMap;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::components::service::{env_vars, Service};
use tracing::info;
use tracing::Level;

pub struct SpawnedService {
    name: String,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
}

impl SpawnedService {
    pub fn new(
        name: String,
        executable: &Path,
        working_directory: &Path,
        vars: HashMap<String, String>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> Self {
        info!("Starting {name} process");

        if !executable.exists() {
            panic!("Expected to have precompiled {name} at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(env_vars(vars, verbosity))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|_| panic!("Failed to start {name}"));

        let logger = ChildProcessLogger::log_child_process(
            &format!("[{name}]"),
            out_level,
            err_level,
            &mut child,
        );

        Self {
            name,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
        }
    }
}

#[async_trait]
impl Service for SpawnedService {
    fn kill(&self) {
        info!("Stopping {}", self.name);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedService {
    fn drop(&mut self) {
        self.kill();
    }
}
