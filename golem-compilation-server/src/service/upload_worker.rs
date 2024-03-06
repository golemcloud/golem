use std::sync::Arc;

use futures::stream::StreamExt;
use golem_common::config::RetryConfig;
use golem_worker_executor_base::services::compiled_template::CompiledTemplateService;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::model::*;

// Worker that uploads compiled templates to the cloud.
#[derive(Clone)]
pub struct UploadWorker {
    retry_config: RetryConfig,
    compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
}

impl UploadWorker {
    pub fn start(
        retry_config: RetryConfig,
        num_workers: usize,
        compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
        recv: mpsc::Receiver<CompiledTemplate>,
        cancellation: CancellationToken,
    ) {
        let worker = Self {
            retry_config,
            compiled_template_service,
        };

        let mut stream = ReceiverStream::new(recv)
            .map(move |request| {
                let worker = worker.clone();
                async move { worker.upload_template(request).await }
            })
            .buffer_unordered(num_workers);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        break;
                    }
                    _ = stream.next() => {}
                }
            }
        });
    }

    async fn upload_template(&self, template: CompiledTemplate) {
        let CompiledTemplate {
            template,
            component,
            result,
        } = template;

        let upload_result = self
            .compiled_template_service
            .put(&template.id, template.version, &component)
            .await
            .map_err(|err| CompilationError::TemplateUploadFailed(err.to_string()));

        if let Err(ref err) = upload_result {
            tracing::warn!("Failed to upload compiled template {template:?}: {err:?}");
        }

        result.send(upload_result).unwrap_or_else(|_| {
            tracing::warn!("Failed to send upload result");
        });
    }
}
