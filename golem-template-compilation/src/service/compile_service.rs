use super::*;
use crate::config::{CompileWorkerConfig, TemplateServiceConfig, UploadWorkerConfig};
use crate::model::*;
use async_trait::async_trait;
use golem_common::model::TemplateId;
use golem_worker_executor_base::services::compiled_template::CompiledTemplateService;
use std::sync::Arc;
use tokio::sync::mpsc;
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
pub struct TemplateCompilationServiceImpl {
    queue: mpsc::Sender<CompilationRequest>,
}

impl TemplateCompilationServiceImpl {
    pub fn new(
        upload_worker: UploadWorkerConfig,
        compile_worker: CompileWorkerConfig,
        template_service: TemplateServiceConfig,

        engine: Engine,

        compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
    ) -> Self {
        let (compile_tx, compile_rx) = mpsc::channel(100);
        let (upload_tx, upload_rx) = mpsc::channel(100);

        CompileWorker::start(
            template_service.uri(),
            template_service.access_token,
            compile_worker,
            engine.clone(),
            compiled_template_service.clone(),
            upload_tx,
            compile_rx,
        );

        UploadWorker::start(upload_worker, compiled_template_service.clone(), upload_rx);

        Self { queue: compile_tx }
    }
}

#[async_trait]
impl CompilationService for TemplateCompilationServiceImpl {
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
