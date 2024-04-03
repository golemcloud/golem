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

use crate::components::worker_executor::{WorkerExecutor};
use async_trait::async_trait;

use tracing::info;

pub struct ProvidedWorkerExecutor {
    host: String,
    http_port: u16,
    grpc_port: u16,
}

impl ProvidedWorkerExecutor {
    pub fn new(host: String, http_port: u16, grpc_port: u16) -> Self {
        info!("Using already running golem-worker-executor on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host,
            http_port,
            grpc_port,
        }
    }
}

#[async_trait]
impl WorkerExecutor for ProvidedWorkerExecutor {
    fn host(&self) -> &str {
        &self.host
    }

    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    fn kill(&self) {
        panic!("Cannot kill provided worker executor");
    }

    fn restart(&self) {
        panic!("Cannot restart provided worker-executor");
    }
}
