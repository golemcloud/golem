use crate::empty_worker_metadata;
use async_trait::async_trait;
use golem_common::model::{ComponentId, ComponentVersion};
use golem_service_base::model::WorkerId;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::evaluator::{
    ComponentDetails, ComponentMetadataService, MetadataFetchError,
};
use golem_worker_service_base::service::component::ComponentService;
use golem_worker_service_base::service::worker::{WorkerService, WorkerServiceError};
use std::sync::Arc;

pub struct DefaultComponentMetadataFetch {
    component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send>,
    worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>,
}

impl DefaultComponentMetadataFetch {
    pub fn new(
        component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send>,
        worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>,
    ) -> Self {
        Self {
            component_service,
            worker_service,
        }
    }
}

#[async_trait]
impl ComponentMetadataService for DefaultComponentMetadataFetch {
    async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentDetails, MetadataFetchError> {
        self.component_service
            .get_latest(component_id, &EmptyAuthCtx::default())
            .await
            .map(|component| ComponentDetails {
                version: component.versioned_component_id.version,
                metadata: component.metadata,
            })
            .map_err(|e| MetadataFetchError::Internal(e.to_string()))
    }

    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
        version: ComponentVersion,
    ) -> Result<ComponentDetails, MetadataFetchError> {
        self.component_service
            .get_by_version(component_id, version, &EmptyAuthCtx::default())
            .await
            .map(|component| ComponentDetails {
                version: component.versioned_component_id.version,
                metadata: component.metadata,
            })
            .map_err(|e| MetadataFetchError::Internal(e.to_string()))
    }

    async fn get_active_component_in_worker(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentVersion, MetadataFetchError> {
        let worker = self
            .worker_service
            .get_metadata(worker_id, empty_worker_metadata(), &EmptyAuthCtx::default())
            .await;

        match worker {
            Ok(worker) => Ok(worker.component_version),
            Err(WorkerServiceError::WorkerNotFound(_)) => Err(MetadataFetchError::WorkerNotFound),
            Err(e) => Err(MetadataFetchError::Internal(e.to_string())),
        }
    }
}
