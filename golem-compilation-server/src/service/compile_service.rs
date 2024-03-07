use super::*;
use crate::config::{CompileWorkerConfig, UploadWorkerConfig};
use crate::model::*;
use async_trait::async_trait;
use golem_common::model::TemplateId;
use golem_worker_executor_base::services::compiled_template::CompiledTemplateService;
use http::Uri;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wasmtime::Engine;

#[async_trait]
pub trait CompilationService {
    async fn enqueue_compilation(
        &self,
        template_id: TemplateId,
        template_version: i32,
    ) -> Result<(), CompilationError>;
}

#[derive(Clone)]
pub struct CompilationServiceDefault {
    queue: mpsc::Sender<CompilationRequest>,
}

impl CompilationServiceDefault {
    pub fn new(
        upload_worker: UploadWorkerConfig,
        compile_worker: CompileWorkerConfig,

        uri: Uri,
        access_token: Uuid,

        engine: Engine,
        cancel: CancellationToken,

        compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
    ) -> Self {
        let (compile_tx, compile_rx) = mpsc::channel(100);
        let (upload_tx, upload_rx) = mpsc::channel(100);

        CompileWorker::start(
            uri.clone(),
            access_token,
            compile_worker,
            engine.clone(),
            compiled_template_service.clone(),
            upload_tx,
            compile_rx,
            cancel.clone(),
        );

        UploadWorker::start(
            upload_worker,
            compiled_template_service.clone(),
            upload_rx,
            cancel.clone(),
        );

        Self { queue: compile_tx }
    }
}

#[async_trait]
impl CompilationService for CompilationServiceDefault {
    async fn enqueue_compilation(
        &self,
        template_id: TemplateId,
        template_version: i32,
    ) -> Result<(), CompilationError> {
        let request = CompilationRequest {
            template: TemplateWithVersion {
                id: template_id,
                version: template_version,
            },
        };
        self.queue.send(request).await?;
        Ok(())
    }
}
