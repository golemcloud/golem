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

use async_trait::async_trait;
use golem_api_grpc::proto::golem::componentcompilation::v1::{
    ComponentCompilationRequest,
    component_compilation_service_client::ComponentCompilationServiceClient,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{ComponentId, RetryConfig};
use http::Uri;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

#[async_trait]
pub trait ComponentCompilationService: Debug + Send + Sync {
    async fn enqueue_compilation(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        component_version: u64,
    );
}

pub struct GrpcComponentCompilationService {
    client: GrpcClient<ComponentCompilationServiceClient<Channel>>,
    component_service_port: u16,
}

impl GrpcComponentCompilationService {
    pub fn new(
        uri: Uri,
        retries: RetryConfig,
        connect_timeout: Duration,
        component_service_port: u16,
    ) -> Self {
        let client = GrpcClient::new(
            "component-compilation-service",
            |channel| {
                ComponentCompilationServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            uri,
            GrpcClientConfig {
                retries_on_unavailable: retries,
                connect_timeout,
            },
        );
        Self {
            client,
            component_service_port,
        }
    }
}

impl Debug for GrpcComponentCompilationService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCompilationServiceDefault")
            .finish()
    }
}

#[async_trait]
impl ComponentCompilationService for GrpcComponentCompilationService {
    async fn enqueue_compilation(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        component_version: u64,
    ) {
        let component_id_clone = component_id.clone();
        let environment_id_clone = environment_id.clone();
        let component_service_port = self.component_service_port;

        let result = self
            .client
            .call("enqueue-compilation", move |client| {
                let component_id_clone = component_id_clone.clone();
                let environment_id_clone = environment_id_clone.clone();
                Box::pin(async move {
                    let request = ComponentCompilationRequest {
                        component_id: Some(component_id_clone.into()),
                        component_version,
                        component_service_port: Some(component_service_port.into()),
                        environment_id: Some(environment_id_clone.into()),
                    };

                    client.enqueue_compilation(request).await
                })
            })
            .await;
        match result {
            Ok(_) => tracing::info!(
                component_id = component_id.to_string(),
                component_version = component_version.to_string(),
                "Enqueued compilation of uploaded component",
            ),
            Err(e) => tracing::error!(
                component_id = component_id.to_string(),
                component_version = component_version.to_string(),
                "Failed to enqueue compilation: {e:?}"
            ),
        }
    }
}

pub struct ComponentCompilationServiceDisabled;

impl Debug for ComponentCompilationServiceDisabled {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCompilationServiceDisabled")
            .finish()
    }
}

#[async_trait]
impl ComponentCompilationService for ComponentCompilationServiceDisabled {
    async fn enqueue_compilation(&self, _: &EnvironmentId, _: &ComponentId, _: u64) {}
}
