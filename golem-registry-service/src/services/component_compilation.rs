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

use crate::config::{ComponentCompilationConfig, ComponentCompilationEnabledConfig};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::componentcompilation::v1::{
    ComponentCompilationRequest,
    component_compilation_service_client::ComponentCompilationServiceClient,
};
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::grpc::client::GrpcClient;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

#[async_trait]
pub trait ComponentCompilationService: Debug + Send + Sync {
    async fn enqueue_compilation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    );

    fn set_own_grpc_port(&self, grpc_port: u16);
}

pub fn configured(config: &ComponentCompilationConfig) -> Arc<dyn ComponentCompilationService> {
    match config {
        ComponentCompilationConfig::Disabled(_) => Arc::new(DisabledComponentCompilationService),
        ComponentCompilationConfig::Enabled(inner) => {
            Arc::new(GrpcComponentCompilationService::new(inner))
        }
    }
}

pub struct GrpcComponentCompilationService {
    client: GrpcClient<ComponentCompilationServiceClient<OtelGrpcService<Channel>>>,
    own_grpc_port: AtomicU16,
}

impl GrpcComponentCompilationService {
    pub fn new(config: &ComponentCompilationEnabledConfig) -> Self {
        let client = GrpcClient::new(
            "component-compilation-service",
            |channel| {
                ComponentCompilationServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            config.uri(),
            config.client_config.clone(),
        );
        Self {
            client,
            own_grpc_port: AtomicU16::new(0),
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
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) {
        let component_service_port = match self.own_grpc_port.load(Ordering::Acquire) {
            0 => None,
            port => Some(port as u32),
        };

        let result = self
            .client
            .call("enqueue-compilation", move |client| {
                Box::pin(async move {
                    let request = ComponentCompilationRequest {
                        component_id: Some(component_id.into()),
                        component_revision: component_revision.into(),
                        component_service_port,
                        environment_id: Some(environment_id.into()),
                    };

                    client.enqueue_compilation(request).await
                })
            })
            .await;
        match result {
            Ok(_) => tracing::info!(
                component_id = component_id.to_string(),
                component_revision = component_revision.to_string(),
                "Enqueued compilation of uploaded component",
            ),
            Err(e) => tracing::error!(
                component_id = component_id.to_string(),
                component_revision = component_revision.to_string(),
                "Failed to enqueue compilation: {e:?}"
            ),
        }
    }

    fn set_own_grpc_port(&self, grpc_port: u16) {
        self.own_grpc_port.store(grpc_port, Ordering::Release);
    }
}

pub struct DisabledComponentCompilationService;

impl Debug for DisabledComponentCompilationService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCompilationServiceDisabled")
            .finish()
    }
}

#[async_trait]
impl ComponentCompilationService for DisabledComponentCompilationService {
    async fn enqueue_compilation(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) {
    }
    fn set_own_grpc_port(&self, _grpc_port: u16) {}
}
