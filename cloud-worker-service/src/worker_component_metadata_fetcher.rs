use crate::service::auth::CloudAuthCtx;
use crate::service::worker::WorkerService;
use async_trait::async_trait;
use cloud_common::model::TokenSecret;
use golem_service_base::model::{ComponentMetadata, WorkerId};
use golem_worker_service_base::evaluator::{MetadataFetchError, WorkerMetadataFetcher};
use std::sync::Arc;
use uuid::Uuid;

pub struct DefaultWorkerComponentMetadataFetcher {
    pub worker_service: Arc<dyn WorkerService + Sync + Send>,
    pub access_token: Uuid,
}

impl DefaultWorkerComponentMetadataFetcher {
    pub fn new(worker_service: Arc<dyn WorkerService + Sync + Send>, access_token: Uuid) -> Self {
        Self {
            worker_service,
            access_token,
        }
    }
}

#[async_trait]
impl WorkerMetadataFetcher for DefaultWorkerComponentMetadataFetcher {
    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentMetadata, MetadataFetchError> {
        let auth = CloudAuthCtx::new(TokenSecret::new(self.access_token));

        self.worker_service
            .get_component_for_worker(worker_id, &auth)
            .await
            .map(|component| component.metadata)
            .map_err(|e| MetadataFetchError(e.to_string()))
    }
}
