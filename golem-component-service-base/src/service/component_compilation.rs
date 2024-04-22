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
use golem_api_grpc::proto::golem::componentcompilation::{
    component_compilation_service_client::ComponentCompilationServiceClient,
    ComponentCompilationRequest,
};
use golem_common::model::ComponentId;

#[async_trait]
pub trait ComponentCompilationService {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64);
}

pub struct ComponentCompilationServiceDefault {
    uri: http_02::Uri,
}

impl ComponentCompilationServiceDefault {
    pub fn new(uri: http_02::Uri) -> Self {
        Self { uri }
    }
}

#[async_trait]
impl ComponentCompilationService for ComponentCompilationServiceDefault {
    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64) {
        let mut client = match ComponentCompilationServiceClient::connect(self.uri.clone()).await {
            Ok(client) => client,
            Err(e) => {
                tracing::error!("Failed to connect to ComponentCompilationService: {e:?}");
                return;
            }
        };

        let request = ComponentCompilationRequest {
            component_id: Some(component_id.clone().into()),
            component_version,
        };

        match client.enqueue_compilation(request).await {
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
    async fn enqueue_compilation(&self, _: &ComponentId, _: u64) {}
}
