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

use async_trait::async_trait;
use golem_api_grpc::proto::golem::componentcompilation::v1::{
    component_compilation_service_client::ComponentCompilationServiceClient,
    ComponentCompilationRequest,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::ComponentId;
use http::Uri;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

#[async_trait]
pub trait ComponentCompilationService {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64);
}

pub struct ComponentCompilationServiceDefault {
    client: GrpcClient<ComponentCompilationServiceClient<Channel>>,
}

impl ComponentCompilationServiceDefault {
    pub fn new(uri: Uri) -> Self {
        let client = GrpcClient::new(
            "component-compilation-service",
            |channel| {
                ComponentCompilationServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            uri,
            GrpcClientConfig::default(), // TODO
        );
        Self { client }
    }
}

#[async_trait]
impl ComponentCompilationService for ComponentCompilationServiceDefault {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64) {
        let component_id_clone = component_id.clone();
        let result = self
            .client
            .call("enqueue-compilation", move |client| {
                let component_id_clone = component_id_clone.clone();
                Box::pin(async move {
                    let request = ComponentCompilationRequest {
                        component_id: Some(component_id_clone.into()),
                        component_version,
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

#[async_trait]
impl ComponentCompilationService for ComponentCompilationServiceDisabled {
    async fn enqueue_compilation(&self, _: &ComponentId, _: u64) {}
}
