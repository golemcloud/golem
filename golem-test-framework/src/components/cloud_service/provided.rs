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

use super::{CloudService, CloudServiceInternal, ProjectServiceClient};
use crate::components::cloud_service::new_project_client;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use tracing::info;

pub struct ProvidedCloudService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    project_client: ProjectServiceClient,
}

impl ProvidedCloudService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Using already running cloud-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            project_client: new_project_client(client_protocol, &host, grpc_port, http_port).await,
        }
    }
}

#[async_trait]
impl CloudServiceInternal for ProvidedCloudService {
    fn project_client(&self) -> ProjectServiceClient {
        self.project_client.clone()
    }
}

#[async_trait]
impl CloudService for ProvidedCloudService {
    fn private_host(&self) -> String {
        self.host.clone()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {}
}
