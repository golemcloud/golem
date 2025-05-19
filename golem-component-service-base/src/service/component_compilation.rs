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
    component_compilation_service_client::ComponentCompilationServiceClient,
    ComponentCompilationRequest,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{ComponentId, RetryConfig};
use http::Uri;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

#[async_trait]
pub trait ComponentCompilationService: Debug + Send + Sync {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64);

    fn set_self_grpc_port(&self, grpc_port: u16);
}

pub struct ComponentCompilationServiceDefault {
    client: GrpcClient<ComponentCompilationServiceClient<Channel>>,
    component_service_port: AtomicU16,
}

impl ComponentCompilationServiceDefault {
    pub fn new(uri: Uri, retries: RetryConfig, connect_timeout: Duration) -> Self {
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
            component_service_port: AtomicU16::new(0),
        }
    }
}

impl Debug for ComponentCompilationServiceDefault {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCompilationServiceDefault")
            .finish()
    }
}

#[async_trait]
impl ComponentCompilationService for ComponentCompilationServiceDefault {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64) {
        let component_id_clone = component_id.clone();

        let component_service_port = match self.component_service_port.load(Ordering::Acquire) {
            0 => None,
            port => Some(port as u32),
        };

        let result = self
            .client
            .call("enqueue-compilation", move |client| {
                let component_id_clone = component_id_clone.clone();
                Box::pin(async move {
                    let request = ComponentCompilationRequest {
                        component_id: Some(component_id_clone.into()),
                        component_version,
                        component_service_port,
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

    fn set_self_grpc_port(&self, grpc_port: u16) {
        self.component_service_port
            .store(grpc_port, Ordering::Release);
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
    async fn enqueue_compilation(&self, _: &ComponentId, _: u64) {}

    fn set_self_grpc_port(&self, _grpc_port: u16) {}
}
