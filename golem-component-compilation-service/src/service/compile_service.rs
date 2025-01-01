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

use super::*;
use crate::config::{CompileWorkerConfig, ComponentServiceConfig};
use crate::model::*;
use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_worker_executor_base::services::compiled_component::CompiledComponentService;
use std::sync::Arc;
use tokio::sync::mpsc;
use wasmtime::Engine;

#[async_trait]
pub trait CompilationService {
    async fn enqueue_compilation(
        &self,
        component_id: ComponentId,
        component_version: u64,
    ) -> Result<(), CompilationError>;
}

#[derive(Clone)]
pub struct ComponentCompilationServiceImpl {
    queue: mpsc::Sender<CompilationRequest>,
}

impl ComponentCompilationServiceImpl {
    pub fn new(
        compile_worker: CompileWorkerConfig,
        component_service: ComponentServiceConfig,

        engine: Engine,

        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    ) -> Self {
        let (compile_tx, compile_rx) = mpsc::channel(100);
        let (upload_tx, upload_rx) = mpsc::channel(100);

        CompileWorker::start(
            component_service.uri(),
            component_service.access_token,
            compile_worker,
            engine.clone(),
            compiled_component_service.clone(),
            upload_tx,
            compile_rx,
        );

        UploadWorker::start(compiled_component_service.clone(), upload_rx);

        Self { queue: compile_tx }
    }
}

#[async_trait]
impl CompilationService for ComponentCompilationServiceImpl {
    async fn enqueue_compilation(
        &self,
        component_id: ComponentId,
        component_version: u64,
    ) -> Result<(), CompilationError> {
        tracing::info!(
            "Enqueueing compilation for component {}@{}",
            component_id,
            component_version
        );
        let request = CompilationRequest {
            component: ComponentWithVersion {
                id: component_id,
                version: component_version,
            },
        };
        self.queue.send(request).await?;
        crate::metrics::increment_queue_length();
        Ok(())
    }
}
