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

use crate::components::redis::Redis;
use crate::components::ChildProcessLogger;
use async_trait::async_trait;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, Level};

pub struct SpawnedRedis {
    port: u16,
    prefix: String,
    child: Arc<Mutex<Option<Child>>>,
    valid: AtomicBool,
    _logger: ChildProcessLogger,
}

impl SpawnedRedis {
    pub fn new_default() -> Self {
        Self::new(
            super::DEFAULT_PORT,
            "".to_string(),
            Level::DEBUG,
            Level::ERROR,
        )
    }

    pub fn new(port: u16, prefix: String, out_level: Level, err_level: Level) -> Self {
        info!("Starting Redis on port {}", port);

        let host = "localhost".to_string();
        let mut child = Command::new("redis-server")
            .arg("--port")
            .arg(port.to_string())
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn redis server");

        let logger =
            ChildProcessLogger::log_child_process("[redis]", out_level, err_level, &mut child);

        super::wait_for_startup(&host, port, Duration::from_secs(10));

        Self {
            port,
            prefix,
            child: Arc::new(Mutex::new(Some(child))),
            valid: AtomicBool::new(true),
            _logger: logger,
        }
    }

    fn blocking_kill(&self) {
        info!("Stopping Redis");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            self.valid.store(false, Ordering::Release);
            let _ = child.kill();
        }
    }
}

#[async_trait]
impl Redis for SpawnedRedis {
    fn assert_valid(&self) {
        if !self.valid.load(Ordering::Acquire) {
            std::panic!("Redis has been closed")
        }
    }

    fn private_host(&self) -> String {
        "localhost".to_string()
    }

    fn private_port(&self) -> u16 {
        self.port
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    async fn kill(&self) {
        self.blocking_kill();
    }
}

impl Drop for SpawnedRedis {
    fn drop(&mut self) {
        self.blocking_kill();
    }
}
