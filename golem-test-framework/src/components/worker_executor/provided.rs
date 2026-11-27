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

use crate::components::worker_executor::WorkerExecutor;
use async_trait::async_trait;
use tracing::info;

pub struct ProvidedWorkerExecutor {
    host: String,
    grpc_port: u16,
    http_port: Option<u16>,
}

impl ProvidedWorkerExecutor {
    pub fn new(host: String, grpc_port: u16, http_port: Option<u16>) -> Self {
        info!("Using already running golem-worker-executor on {host}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            grpc_port,
            http_port,
        }
    }
}

#[async_trait]
impl WorkerExecutor for ProvidedWorkerExecutor {
    fn grpc_host(&self) -> String {
        self.host.clone()
    }
    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }
    fn http_port(&self) -> Option<u16> {
        self.http_port
    }

    /// The executor is not restarted by this process, so its generation stays 0.
    fn metrics_endpoint(&self) -> Option<(String, u64)> {
        self.http_port
            .map(|port| (format!("http://{}:{port}/metrics", self.host), 0))
    }

    async fn restart(&self) {
        panic!("Cannot restart provided worker-executor");
    }

    async fn is_running(&self) -> bool {
        true
    }
}
