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

use super::*;
use crate::config::{CompileWorkerConfig, RegistryServiceConfig, StaticRegistryServiceConfig};
use crate::model::*;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::service::compiled_component::CompiledComponentService;
use std::sync::Arc;
use tokio::sync::mpsc;
use wasmtime::Engine;

#[derive(Clone)]
pub struct ComponentCompilationService {
    queue: mpsc::Sender<CompilationRequest>,
}

impl ComponentCompilationService {
    pub async fn new(
        compile_worker: CompileWorkerConfig,
        registry_service: RegistryServiceConfig,
        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService>,
    ) -> Self {
        let (compile_tx, compile_rx) = mpsc::channel(100);
        let (upload_tx, upload_rx) = mpsc::channel(100);

        CompileWorker::start(
            registry_service.static_config(),
            compile_worker,
            engine.clone(),
            compiled_component_service.clone(),
            upload_tx,
            compile_rx,
        )
        .await;

        UploadWorker::start(compiled_component_service.clone(), upload_rx);

        Self { queue: compile_tx }
    }

    pub async fn enqueue_compilation(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        environment_id: EnvironmentId,
        sender: Option<StaticRegistryServiceConfig>,
    ) -> Result<(), CompilationError> {
        tracing::info!(
            component_id = component_id.to_string(),
            component_revision = component_revision.to_string(),
            "Enqueueing compilation for component",
        );
        let request = CompilationRequest {
            component: ComponentIdAndRevision {
                id: component_id,
                revision: component_revision,
            },
            environment_id,
            sender,
        };
        self.queue.send(request).await?;
        crate::metrics::increment_queue_length();
        Ok(())
    }
}
