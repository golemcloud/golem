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

use super::{new_worker_grpc_client, WorkerServiceGrpcClient};
use crate::components::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::new_reqwest_client;
use crate::components::worker_service::WorkerService;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;

pub struct ProvidedWorkerService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    client_protocol: GolemClientProtocol,
    component_service: Arc<dyn ComponentService>,
    cloud_service: Arc<dyn CloudService>,
    base_http_client: OnceCell<reqwest::Client>,
    worker_grpc_client: OnceCell<WorkerServiceGrpcClient<Channel>>,
}

impl ProvidedWorkerService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        client_protocol: GolemClientProtocol,
        component_service: Arc<dyn ComponentService>,
        cloud_service: Arc<dyn CloudService>,
    ) -> Self {
        info!("Using already running golem-worker-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            custom_request_port,
            client_protocol,
            component_service,
            cloud_service,
            base_http_client: OnceCell::new(),
            worker_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl WorkerService for ProvidedWorkerService {
    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
    }

    fn cloud_service(&self) -> &Arc<dyn CloudService> {
        &self.cloud_service
    }

    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn worker_grpc_client(&self) -> WorkerServiceGrpcClient<Channel> {
        self.worker_grpc_client
            .get_or_init(async || {
                new_worker_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    fn private_host(&self) -> String {
        self.host.clone()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    fn private_custom_request_port(&self) -> u16 {
        self.custom_request_port
    }

    async fn kill(&self) {}
}
