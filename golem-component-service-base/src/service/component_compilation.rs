// Copyright 2024 Golem Cloud
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
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::log::info;

#[async_trait]
pub trait ComponentCompilationService {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64, ifs_data: Vec<u8>);
}

pub struct ComponentCompilationServiceDefault {
    client: GrpcClient<ComponentCompilationServiceClient<Channel>>,
}

impl ComponentCompilationServiceDefault {
    pub fn new(uri: http_02::Uri) -> Self {
        let client = GrpcClient::new(
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
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64, ifs_data: Vec<u8>) {
        let component_id_clone = component_id.clone();
        let ifs_data_clone = ifs_data.clone();
        let result = self
            .client
            .call(move |client| {
                let component_id_clone = component_id_clone.clone();
                let ifs_data_clone = ifs_data_clone.clone();
                Box::pin(async move {
                    let request = ComponentCompilationRequest {
                        component_id: Some(component_id_clone.into()),
                        component_version,
                        ifs_data: ifs_data_clone,

                    };

                    client.enqueue_compilation(request).await
                })
            })
            .await;
        match result {
            Ok(_) => tracing::info!(
                "Enqueued compilation for component {component_id} version {component_version}",
            ),
            Err(e) => tracing::error!("Failed to enqueue compilation: {e:?}"),
        }
    }
}

pub struct ComponentCompilationServiceDisabled;

#[async_trait]
impl ComponentCompilationService for ComponentCompilationServiceDisabled {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64, ifs_data: Vec<u8>) {}
}
