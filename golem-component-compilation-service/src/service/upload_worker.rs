use std::sync::Arc;

use golem_worker_executor_base::services::compiled_component::CompiledComponentService;
use tokio::sync::mpsc;

use crate::{config::UploadWorkerConfig, model::*};

// Worker that uploads compiled components to the cloud.
#[derive(Clone)]
pub struct UploadWorker {
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
}

impl UploadWorker {
    pub fn start(
        _: UploadWorkerConfig,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
        mut recv: mpsc::Receiver<CompiledComponent>,
    ) {
        let worker = Self {
            compiled_component_service,
        };

        tokio::spawn(async move {
            loop {
                while let Some(request) = recv.recv().await {
                    worker.upload_component(request).await
                }
            }
        });
    }

    // Don't need retries because they're baked into CompiledComponentService.
    async fn upload_component(&self, compiled_component: CompiledComponent) {
        let CompiledComponent {
            component_and_version,
            component,
        } = compiled_component;

        let upload_result = self
            .compiled_component_service
            .put(
                &component_and_version.id,
                component_and_version.version,
                &component,
            )
            .await
            .map_err(|err| CompilationError::ComponentUploadFailed(err.to_string()));

        if let Err(ref err) = upload_result {
            tracing::warn!("Failed to upload compiled component {component_and_version}: {err:?}");
        } else {
            tracing::info!("Successfully uploaded compiled component {component_and_version}");
        }
    }
}
