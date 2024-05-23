use async_trait::async_trait;
use golem_service_base::model::{Export, WorkerId};
use golem_wasm_ast::analysis::AnalysedFunction;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::evaluator::{MetadataFetchError, WorkerMetadataFetcher};
use golem_worker_service_base::service::worker::WorkerService;
use std::sync::Arc;

pub struct DefaultWorkerComponentMetadataFetcher {
    pub worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>,
}

impl DefaultWorkerComponentMetadataFetcher {
    pub fn new(worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl WorkerMetadataFetcher for DefaultWorkerComponentMetadataFetcher {
    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> Result<Vec<AnalysedFunction>, MetadataFetchError> {
        let result = self
            .worker_service
            .get_component_for_worker(worker_id, &EmptyAuthCtx {})
            .await
            .map_err(|e| MetadataFetchError(e.to_string()))?;

        let functions = result
            .metadata
            .exports
            .iter()
            .flat_map(|x| match x {
                Export::Function(function) => vec![function.clone().into()],
                _ => vec![],
            })
            .collect::<Vec<AnalysedFunction>>();

        Ok(functions)
    }
}
