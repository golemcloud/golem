use std::sync::Arc;

use golem_worker_executor_base::services::compiled_template::CompiledTemplateService;
use tokio::sync::mpsc;

use crate::{config::UploadWorkerConfig, model::*};

// Worker that uploads compiled templates to the cloud.
#[derive(Clone)]
pub struct UploadWorker {
    compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
}

impl UploadWorker {
    pub fn start(
        _: UploadWorkerConfig,
        compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
        mut recv: mpsc::Receiver<CompiledTemplate>,
    ) {
        let worker = Self {
            compiled_template_service,
        };

        tokio::spawn(async move {
            loop {
                while let Some(request) = recv.recv().await {
                    worker.upload_template(request).await
                }
            }
        });
    }

    // Don't need retries because they're baked into CompiledTemplateService.
    async fn upload_template(&self, template: CompiledTemplate) {
        let CompiledTemplate {
            template,
            component,
        } = template;

        let upload_result = self
            .compiled_template_service
            .put(&template.id, template.version, &component)
            .await
            .map_err(|err| CompilationError::TemplateUploadFailed(err.to_string()));

        if let Err(ref err) = upload_result {
            tracing::warn!("Failed to upload compiled template {template:?}: {err:?}");
        }
    }
}
