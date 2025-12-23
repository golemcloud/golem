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

use crate::components::new_reqwest_client;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use tokio::sync::OnceCell;
use tracing::info;

pub struct ProvidedWorkerService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    base_http_client: OnceCell<reqwest::Client>,
}

impl ProvidedWorkerService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
    ) -> Self {
        info!("Using already running golem-worker-service on {host}, http port: {http_port}, grpc port: {grpc_port}, custom request port: {custom_request_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            custom_request_port,
            base_http_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl WorkerService for ProvidedWorkerService {
    fn http_host(&self) -> String {
        self.host.clone()
    }
    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_host(&self) -> String {
        self.host.clone()
    }
    fn gprc_port(&self) -> u16 {
        self.grpc_port
    }

    fn custom_request_host(&self) -> String {
        self.host.clone()
    }
    fn custom_request_port(&self) -> u16 {
        self.custom_request_port
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn kill(&self) {}
}
