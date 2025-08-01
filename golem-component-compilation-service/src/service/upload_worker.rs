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

use golem_worker_executor::services::compiled_component::CompiledComponentService;
use tokio::sync::mpsc;
use tracing::Instrument;

use crate::model::*;

// Worker that uploads compiled components to the cloud.
#[derive(Clone)]
pub struct UploadWorker {
    compiled_component_service: Arc<dyn CompiledComponentService>,
}

impl UploadWorker {
    pub fn start(
        compiled_component_service: Arc<dyn CompiledComponentService>,
        mut recv: mpsc::Receiver<CompiledComponent>,
    ) {
        let worker = Self {
            compiled_component_service,
        };

        tokio::spawn(
            async move {
                loop {
                    while let Some(request) = recv.recv().await {
                        worker.upload_component(request).await
                    }
                }
            }
            .in_current_span(),
        );
    }

    // Don't need retries because they're baked into CompiledComponentService.
    async fn upload_component(&self, compiled_component: CompiledComponent) {
        let CompiledComponent {
            component_and_version,
            component,
            project_id,
        } = compiled_component;

        let upload_result = self
            .compiled_component_service
            .put(
                &project_id,
                &component_and_version.id,
                component_and_version.version,
                &component,
            )
            .await
            .map_err(|err| CompilationError::ComponentUploadFailed(err.to_string()));

        if let Err(ref err) = upload_result {
            tracing::warn!(
                component_id = component_and_version.id.to_string(),
                component_version = component_and_version.version.to_string(),
                error = err.to_string(),
                "Failed to upload compiled component"
            );
        } else {
            tracing::info!(
                component_id = component_and_version.id.to_string(),
                component_version = component_and_version.version.to_string(),
                "Successfully uploaded compiled component"
            );
        }
    }
}
