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

use std::sync::Arc;

use crate::components::component_service::ComponentService;
use crate::components::worker_service::{
    new_api_definition_client, new_api_deployment_client, new_api_security_client,
    new_worker_client, ApiDefinitionServiceClient, ApiDeploymentServiceClient,
    ApiSecurityServiceClient, WorkerService, WorkerServiceClient,
};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use tracing::info;

use super::WorkerServiceInternal;

pub struct ProvidedWorkerService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    client_protocol: GolemClientProtocol,
    worker_client: WorkerServiceClient,
    api_definition_client: ApiDefinitionServiceClient,
    api_deployment_client: ApiDeploymentServiceClient,
    api_security_client: ApiSecurityServiceClient,
    component_service: Arc<dyn ComponentService>,
}

impl ProvidedWorkerService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        client_protocol: GolemClientProtocol,
        component_service: Arc<dyn ComponentService>,
    ) -> Self {
        info!("Using already running golem-worker-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            custom_request_port,
            client_protocol,
            worker_client: new_worker_client(client_protocol, &host, grpc_port, http_port).await,
            api_definition_client: new_api_definition_client(
                client_protocol,
                &host,
                grpc_port,
                http_port,
            )
            .await,
            api_deployment_client: new_api_deployment_client(
                client_protocol,
                &host,
                grpc_port,
                http_port,
            )
            .await,
            api_security_client: new_api_security_client(
                client_protocol,
                &host,
                grpc_port,
                http_port,
            )
            .await,
            component_service: component_service.clone(),
        }
    }
}

impl WorkerServiceInternal for ProvidedWorkerService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    fn worker_client(&self) -> WorkerServiceClient {
        self.worker_client.clone()
    }

    fn api_definition_client(&self) -> ApiDefinitionServiceClient {
        self.api_definition_client.clone()
    }

    fn api_deployment_client(&self) -> ApiDeploymentServiceClient {
        self.api_deployment_client.clone()
    }

    fn api_security_client(&self) -> ApiSecurityServiceClient {
        self.api_security_client.clone()
    }

    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
    }
}

#[async_trait]
impl WorkerService for ProvidedWorkerService {
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
