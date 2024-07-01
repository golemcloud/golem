use async_trait::async_trait;
use golem_service_base::model::ComponentMetadata;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::evaluator::{MetadataFetchError, ComponentMetadataFetch};
use std::sync::Arc;
use golem_common::model::ComponentId;
use golem_worker_service_base::service::component::ComponentService;

pub struct DefaultComponentMetadataFetch {
    component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send>,
}

impl DefaultComponentMetadataFetch {
    pub fn new(component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send>) -> Self {
        Self { component_service }
    }
}

#[async_trait]
impl ComponentMetadataFetch for DefaultComponentMetadataFetch {
    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentMetadata, MetadataFetchError> {
        self.component_service
            .get_latest(component_id,  &EmptyAuthCtx::default())
            .await
            .map(|component| component.metadata)
            .map_err(|e| MetadataFetchError(e.to_string()))
    }
}
