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
use golem_api_grpc::proto::golem::templatecompilation::{
    template_compilation_service_client::TemplateCompilationServiceClient,
    TemplateCompilationRequest,
};
use golem_common::model::TemplateId;

#[async_trait]
pub trait TemplateCompilationService {
    async fn enqueue_compilation(&self, template_id: &TemplateId, template_version: i32);
}

pub struct TemplateCompilationServiceDefault {
    uri: http_02::Uri,
}

impl TemplateCompilationServiceDefault {
    pub fn new(uri: http_02::Uri) -> Self {
        Self { uri }
    }
}

#[async_trait]
impl TemplateCompilationService for TemplateCompilationServiceDefault {
    async fn enqueue_compilation(&self, template_id: &TemplateId, template_version: i32) {
        let mut client = match TemplateCompilationServiceClient::connect(self.uri.clone()).await {
            Ok(client) => client,
            Err(e) => {
                tracing::error!("Failed to connect to TemplateCompilationService: {e:?}");
                return;
            }
        };

        let request = TemplateCompilationRequest {
            template_id: Some(template_id.clone().into()),
            template_version,
        };

        match client.enqueue_compilation(request).await {
            Ok(_) => tracing::info!(
                "Enqueued compilation for template {template_id} version {template_version}",
            ),
            Err(e) => tracing::error!("Failed to enqueue compilation: {e:?}"),
        }
    }
}

pub struct TemplateCompilationServiceDisabled;

#[async_trait]
impl TemplateCompilationService for TemplateCompilationServiceDisabled {
    async fn enqueue_compilation(&self, _: &TemplateId, _: i32) {}
}
