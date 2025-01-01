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

use crate::components::worker_service::{new_client, WorkerService};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use tonic::transport::Channel;
use tracing::info;

pub struct ProvidedWorkerService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    client: Option<WorkerServiceClient<Channel>>,
}

impl ProvidedWorkerService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        shared_client: bool,
    ) -> Self {
        info!("Using already running golem-worker-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            custom_request_port,
            client: if shared_client {
                Some(
                    new_client(&host, grpc_port)
                        .await
                        .expect("Failed to create client"),
                )
            } else {
                None
            },
        }
    }
}

#[async_trait]
impl WorkerService for ProvidedWorkerService {
    async fn client(&self) -> crate::Result<WorkerServiceClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => Ok(new_client(&self.host, self.grpc_port).await?),
        }
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
